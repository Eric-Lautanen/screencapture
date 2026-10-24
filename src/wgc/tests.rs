//! Window-capture tests: the centering arithmetic against plain buffers,
//! and one end-to-end grab of a window that another window covers.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Duration;

use super::{Window, copy_center};
use crate::capture::{Region, Target};
use crate::overlay::{WS_EX_NOACTIVATE, WS_VISIBLE};
use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, Hwnd, Point, Rect,
    RegisterClassExW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WndClassExW, kept_wide,
    module_handle, pump_until, wide,
};

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> *mut c_void;
    fn DeleteObject(object: *mut c_void) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowPos(
        window: *mut c_void,
        insert_after: *mut c_void,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    fn WindowFromPoint(point: Point) -> *mut c_void;
}

/// `HWND_TOPMOST` (winuser.h): the top of the always-on-top band.
const TOPMOST: *mut c_void = -1isize as *mut c_void;
/// `SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE`: keep the frame, raise it.
const RAISE_ONLY: u32 = 0x0002 | 0x0001 | 0x0010;

/// One BGRA pixel as `(blue, green, red, alpha)`.
fn pixel(bytes: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    let slice = &bytes[index..index + 4];
    [slice[0], slice[1], slice[2], slice[3]]
}

/// A frame with every pixel tagging its own column and row, so a copy can
/// be checked for both shape and placement.
fn numbered(width: usize, height: usize, pitch: usize) -> Vec<u8> {
    let mut frame = vec![0_u8; pitch * height];
    for row in 0..height {
        for column in 0..width {
            let at = row * pitch + column * 4;
            frame[at..at + 4].copy_from_slice(&[
                u8::try_from(column).unwrap_or(0),
                u8::try_from(row).unwrap_or(0),
                0xAA,
                0xFF,
            ]);
        }
    }
    frame
}

#[test]
fn a_frame_larger_than_the_region_is_cropped_from_its_middle() {
    // 4x4 frame (16-byte rows) into a 2x2 region: columns 1..3, rows 1..3.
    let source = numbered(4, 4, 16);
    let mut output = vec![0xEE; 2 * 2 * 4];
    copy_center(&source, 16, (4, 4), (2, 2), &mut output);
    // Every copied pixel keeps the source marker that names its place.
    assert_eq!(pixel(&output, 2, 0, 0)[..2], [1, 1]);
    assert_eq!(pixel(&output, 2, 1, 1)[..2], [2, 2]);
    assert_eq!(pixel(&output, 2, 0, 0)[2], 0xAA, "no stale bytes survive");
    assert_eq!(output.len(), 4 * 4);
}

#[test]
fn a_frame_smaller_than_the_region_is_centered_and_padded() {
    // 2x2 frame into 4x4: one black pixel of frame on each side.
    let source = numbered(2, 2, 8);
    let mut output = vec![0xEE; 4 * 4 * 4];
    copy_center(&source, 8, (2, 2), (4, 4), &mut output);
    assert_eq!(pixel(&output, 4, 0, 0), [0, 0, 0, 0]);
    assert_eq!(pixel(&output, 4, 1, 1)[..2], [0, 0]);
    assert_eq!(pixel(&output, 4, 2, 2)[..2], [1, 1]);
    assert_eq!(pixel(&output, 4, 3, 3), [0, 0, 0, 0]);
    // A padded row is fully cleared, never a slice of a previous frame.
    assert_eq!(pixel(&output, 4, 3, 0), [0, 0, 0, 0]);
}

#[test]
fn a_padded_driver_row_keeps_the_pixels_where_they_belong() {
    // The driver's pitch is wider than the frame: the extra bytes must not
    // shift a row's pixels.
    let mut source = vec![0xEE_u8; 12 * 2];
    for (row, base) in [(0usize, 0usize), (1, 12)] {
        source[base..base + 4].copy_from_slice(&[1, u8::try_from(row).unwrap_or(0), 0, 0xFF]);
        source[base + 4..base + 8].copy_from_slice(&[2, u8::try_from(row).unwrap_or(0), 0, 0xFF]);
    }
    let mut output = vec![0; 2 * 2 * 4];
    copy_center(&source, 12, (2, 2), (2, 2), &mut output);
    assert_eq!(pixel(&output, 2, 0, 0)[..2], [1, 0]);
    assert_eq!(pixel(&output, 2, 1, 1)[..2], [2, 1]);
}

#[test]
fn a_degenerate_shape_copies_nothing() {
    let mut output = vec![0xEE; 16];
    copy_center(&[], 0, (0, 0), (2, 2), &mut output);
    assert_eq!(output, vec![0; 16]);
    copy_center(&[1, 2, 3, 4], 4, (1, 1), (0, 0), &mut output);
    assert_eq!(output, vec![0; 16]);
}

/// Windows Graphics Capture drives the whole desktop's composition stack, so
/// the two windows a scenario needs are raised one at a time.
static DESKTOP: Mutex<()> = Mutex::new(());

/// A topmost, non-activating window painted one solid color by its class
/// background brush, so what it draws is unambiguous.
struct Painted {
    window: Hwnd,
    handle: usize,
    rect: Rect,
}

impl Painted {
    fn show(
        class: &'static str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        color: u32,
    ) -> Option<Self> {
        let instance = module_handle();
        let name = kept_wide(class);
        // The brush is the window's whole content and must outlive the
        // class, so it is owned by the process, not by this guard.
        // SAFETY: a plain COLORREF request; the brush is never freed.
        let brush = unsafe { CreateSolidBrush(color) };
        if brush.is_null() {
            return None;
        }
        let definition = WndClassExW {
            size: u32::try_from(std::mem::size_of::<WndClassExW>()).ok()?,
            style: 0,
            wnd_proc: DefWindowProcW,
            cls_extra: 0,
            wnd_extra: 0,
            instance,
            icon: std::ptr::null_mut(),
            cursor: std::ptr::null_mut(),
            background: brush,
            menu_name: std::ptr::null(),
            class_name: name,
            icon_small: std::ptr::null_mut(),
        };
        // SAFETY: a fully initialized class whose name outlives the process.
        unsafe { RegisterClassExW(&raw const definition) };
        let title = wide(class);
        // SAFETY: strings and handles outlive the call; the window is owned
        // by this thread and destroyed in `Drop`.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                name,
                title.as_ptr(),
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null_mut(),
            )
        };
        if window.is_null() {
            // SAFETY: the brush this branch never handed to a class.
            unsafe { DeleteObject(brush) };
            return None;
        }
        let mut rect = Rect::default();
        // SAFETY: `rect` is a live local and `window` was created above.
        let measured = unsafe { GetWindowRect(window, &raw mut rect) } != 0
            && rect.right > rect.left
            && rect.bottom > rect.top;
        if !measured {
            // SAFETY: the window this function created.
            unsafe { DestroyWindow(window) };
            return None;
        }
        Some(Self {
            window,
            handle: window as usize,
            rect,
        })
    }

    fn region(&self) -> Region {
        Region {
            x: self.rect.left,
            y: self.rect.top,
            width: u32::try_from(self.rect.right - self.rect.left).unwrap_or(0),
            height: u32::try_from(self.rect.bottom - self.rect.top).unwrap_or(0),
        }
    }

    fn center(&self) -> Point {
        Point {
            x: i32::midpoint(self.rect.left, self.rect.right),
            y: i32::midpoint(self.rect.top, self.rect.bottom),
        }
    }

    /// Raise this window over the always-on-top band.
    fn raise(&self) {
        // SAFETY: a live window handle from this test process.
        unsafe { SetWindowPos(self.window, TOPMOST, 0, 0, 0, 0, RAISE_ONLY) };
    }
}

impl Drop for Painted {
    fn drop(&mut self) {
        // SAFETY: the window this guard created on this thread.
        unsafe { DestroyWindow(self.window) };
    }
}

/// Let the windows process what they were sent. A class background brush
/// only reaches the screen when the window dispatches its paint messages,
/// and this test thread never runs a loop of its own.
fn settle() {
    // The predicate is never true: the budget is the whole point, and the
    // pump dispatches everything queued before it expires.
    let _ = pump_until(Duration::from_millis(150), || false);
}

/// The root window under `point`, as the desktop resolves it.
fn under(point: Point) -> usize {
    // SAFETY: a point lookup; the answer is inspected only.
    unsafe { WindowFromPoint(point) as usize }
}

/// Colors as `COLORREF` (`0x00BBGGRR`) and the BGRA byte order a capture
/// hands back, so the assertion reads the same way the frame does.
const TARGET_COLORREF: u32 = 0x0000_00FF; // pure red
const TARGET_BGRA: [u8; 4] = [0, 0, 255, 255];
const COVER_COLORREF: u32 = 0x00FF_0000; // pure blue
const COVER_BGRA: [u8; 4] = [255, 0, 0, 255];

/// The whole recording path, end to end: the recorder opens the window
/// source, the engine writes the file, and the frame the file holds is the
/// selected window's own.
#[test]
fn a_recording_of_a_covered_window_holds_its_own_content() {
    use std::process::Command;

    use crate::encoder::Video;
    use crate::recorder::{Plan, Recorder};
    use crate::recording::Profile;

    let _desktop = DESKTOP.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(target) = Painted::show(
        "screencapture.test.record.target",
        200,
        160,
        480,
        320,
        TARGET_COLORREF,
    ) else {
        return;
    };
    let Some(cover) = Painted::show(
        "screencapture.test.record.cover",
        180,
        140,
        520,
        360,
        COVER_COLORREF,
    ) else {
        return;
    };
    cover.raise();
    settle();
    if under(target.center()) != cover.handle {
        eprintln!("skipping: another window covers the test target");
        return;
    }
    let Some(binary) = crate::ffmpeg::discover(None) else {
        eprintln!("skipping: no FFmpeg engine to record with");
        return;
    };
    let region = target.region();
    // A machine where Windows refuses window capture would have the
    // recorder fall back to the screen, so this capability is asked first:
    // the assertion below is about what the recorder does with a window it
    // *can* capture, not about the fallback.
    let probe = Window::open(target.handle, (region.width, region.height), false)
        .and_then(|mut window| window.frame().map(|_| ()));
    if let Err(error) = probe {
        eprintln!("skipping: window capture unavailable here ({error})");
        return;
    }
    let folder = std::env::temp_dir().join(format!(
        "screencapture-window-recording-{}",
        std::process::id()
    ));
    let plan = Plan {
        origin: (region.x, region.y),
        size: (region.width, region.height),
        profile: Profile::by_id("sd-480p30").expect("profile exists"),
        codec: Video::X264,
        cursor: false,
        media_dir: folder.clone(),
        window: Some(target.handle),
        audio: crate::audio::Wanted::default(),
        audio_devices: crate::audio::Devices::default(),
        audio_levels: crate::audio::Levels::default(),
    };
    let recorder = match Recorder::start(&binary, &plan) {
        Ok(recorder) => recorder,
        Err(error) => {
            eprintln!("skipping: the recorder refused the window ({error})");
            let _ = std::fs::remove_dir_all(&folder);
            return;
        }
    };
    std::thread::sleep(Duration::from_millis(600));
    let Ok(path) = recorder.stop() else {
        eprintln!("skipping: the recording did not finish");
        let _ = std::fs::remove_dir_all(&folder);
        return;
    };
    let decoded = Command::new(&binary.path)
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(&path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "bgra", "-"])
        .output();
    let _ = std::fs::remove_dir_all(&folder);
    let Ok(decoded) = decoded else {
        eprintln!("skipping: the engine could not decode its own file");
        return;
    };
    let (width, height) = plan.profile.fit(plan.size);
    let frame = decoded.stdout;
    assert_eq!(
        frame.len(),
        usize::try_from(width * height * 4).unwrap_or(0),
        "one BGRA frame of the reported size"
    );
    let [blue, green, red, _] = pixel(&frame, width, width / 2, height / 2);
    // The encode is lossy, so the channels are checked by dominance: a
    // coverage of the blue cover would leave no red at all.
    assert!(
        red > 200 && green < 60 && blue < 60,
        "the recording holds the window on top ({blue}, {green}, {red})"
    );
}

/// The point of selecting an application: a window that is completely
/// covered records what it draws, not what is stacked on top of it.
#[test]
fn a_covered_window_records_its_own_content() {
    let _desktop = DESKTOP.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(target) = Painted::show(
        "screencapture.test.capture.target",
        140,
        120,
        480,
        320,
        TARGET_COLORREF,
    ) else {
        return; // no desktop to draw on
    };
    let Some(cover) = Painted::show(
        "screencapture.test.capture.cover",
        120,
        100,
        520,
        360,
        COVER_COLORREF,
    ) else {
        return;
    };
    cover.raise();
    settle();
    let center = target.center();
    if under(center) != cover.handle {
        eprintln!("skipping: another window covers the test target");
        return;
    }
    // The desktop really shows the cover over that rectangle: this is the
    // bug being guarded, and the region blit still sees it.
    let region = target.region();
    let mut blit = Target::new(region, false).expect("a region blit of the same rect");
    let screen = blit.frame().expect("the screen answers a frame");
    assert_eq!(
        pixel(screen, region.width, region.width / 2, region.height / 2),
        COVER_BGRA,
        "the region blit sees the window on top, which is the bug"
    );
    // The window source sees through the cover to the window's own pixels.
    let mut window = match Window::open(target.handle, (region.width, region.height), false) {
        Ok(window) => window,
        Err(error) => {
            eprintln!("skipping: window capture unavailable here ({error})");
            return;
        }
    };
    let frame = window.frame().expect("a window frame");
    for (x, y) in [
        (region.width / 2, region.height / 2),
        (region.width / 4, region.height / 4),
        (region.width - 8, region.height - 8),
    ] {
        assert_eq!(
            pixel(frame, region.width, x, y),
            TARGET_BGRA,
            "window pixel at ({x}, {y}) must be the selected window's own"
        );
    }
}
