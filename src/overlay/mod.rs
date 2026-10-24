//! The frame drawn around the area a recording is capturing, and the
//! two-tone dotted selection frame used during interactive picking. Both
//! use click-through, non-activating, topmost layered windows with
//! `WDA_EXCLUDEFROMCAPTURE` so the chrome never appears in captured frames.
//!
//! The recording frame is quiet on purpose: a two-tone hairline that reads
//! on any content, closed into one continuous line all the way round, and a
//! slow breath in the window's own opacity. The breath is a window
//! attribute, not a repaint, so announcing a live take costs the capture
//! nothing.

use std::cell::Cell;
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::time::{Duration, Instant};

use crate::capture::Region;
use crate::pin_layout;
use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, Hcursor, Hdc, Hinstance, Hwnd, Lparam, Lresult,
    Rect, RegisterClassExW, WndClassExW, Wparam,
};

pub(super) use crate::winapi::{kept_wide, module_handle, wide};

mod bar;
mod crosshair;
#[cfg(test)]
mod tests;

pub(crate) use crosshair::Crosshair;

use bar::show_bar;
use crosshair::{CROSSHAIR_BLACK, CROSSHAIR_WHITE};

/// Window class registered for the frame.
const CLASS_NAME: &str = "screencapture.recording.frame";
/// Window class for the picker's selection outline.
const SELECTION_CLASS: &str = "screencapture.selection.frame";

pub(super) const WS_POPUP: u32 = 0x8000_0000;
pub(super) const WS_VISIBLE: u32 = 0x1000_0000;
pub(super) const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub(super) const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
pub(super) const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub(super) const WS_EX_LAYERED: u32 = 0x0008_0000;
pub(super) const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

/// `LWA_COLORKEY` and `LWA_ALPHA` (winuser.h): one colour becomes fully
/// transparent, and everything else is drawn at the window's own opacity.
pub(super) const LWA_COLORKEY: u32 = 1;
pub(super) const LWA_ALPHA: u32 = 2;
/// `WDA_EXCLUDEFROMCAPTURE` (winuser.h): hide the window from captures.
pub(super) const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
/// `COLORREF` magenta used as the transparent interior.
pub(super) const COLOR_KEY: u32 = 0x00FF_00FF;

pub(super) const WM_PAINT: u32 = 0x000F;
pub(super) const WM_ERASEBKGND: u32 = 0x0014;
pub(super) const WM_MOUSEMOVE: u32 = 0x0200;
pub(super) const WM_LBUTTONDOWN: u32 = 0x0201;
pub(super) const WM_LBUTTONUP: u32 = 0x0202;
pub(super) const WM_TIMER: u32 = 0x0113;

/// Selection outline dot pattern, in physical pixels.
pub(super) const SELECTION_DOT: i32 = 2;
const SELECTION_PITCH: i32 = 6;

pub(super) type StopAction = fn();

pub(super) const SWP_NOACTIVATE: u32 = 0x0010;
pub(super) const SWP_SHOWWINDOW: u32 = 0x0040;
pub(super) const SWP_NOMOVE: u32 = 0x0002;
pub(super) const SWP_NOSIZE: u32 = 0x0001;
pub(super) const HWND_TOPMOST: Hwnd = -1isize as Hwnd;

/// A colour as `COLORREF`, the layout GDI takes and a 32-bit DIB keeps.
pub(super) const fn rgb(red: u32, green: u32, blue: u32) -> u32 {
    (blue << 16) | (green << 8) | red
}

// The recording frame's palette: a hairline drawn dark outside and light
// inside so it reads on any content.
const FRAME_SHADE: u32 = rgb(0x12, 0x12, 0x16);
const FRAME_LIGHT: u32 = rgb(0xEC, 0xED, 0xF1);

/// The breath: one slow cycle, kept well inside the readable range so the
/// frame never becomes a stripe and never disappears.
const BREATH_MS: u64 = 3000;
const BREATH_STEP: u32 = 80;
const BREATH_LOW: u8 = 178;
const BREATH_HIGH: u8 = 216;
/// Fixed-point denominator for the breath's easing.
const BREATH_SCALE: u64 = 1024;
/// The frame's animation timer.
const BREATH_TIMER: usize = 1;
/// `DEFAULT_DPI` (wingdi.h): 100% scale.
const DEFAULT_DPI: u32 = 96;

/// The DPI setting's scale as an integer percentage (100 at 96 DPI) for the
/// monitor `window` is on.
///
/// The process is per-monitor DPI aware, so Windows will not scale these
/// windows for us: every designed pixel goes through [`scaled`] instead. This
/// is only half of the answer — [`chrome_percent`] raises it to the panel's
/// real density — but it is the half that survives a driver that lies about
/// its size.
pub(super) fn dpi_percent(window: Hwnd) -> i32 {
    // SAFETY: parameterless query about a window this process owns.
    let dpi = unsafe { GetDpiForWindow(window) };
    let dpi = if dpi == 0 { DEFAULT_DPI } else { dpi };
    let dpi = i32::try_from(dpi).unwrap_or(i32::try_from(DEFAULT_DPI).unwrap_or(96));
    (dpi * 100 / 96).clamp(100, 400)
}

/// One designed pixel value at `percent` scale, rounded to nearest.
pub(super) const fn scaled(value: i32, percent: i32) -> i32 {
    (value * percent + 50) / 100
}

/// `HORZSIZE` and `HORZRES` (wingdi.h): a display's width in millimetres and
/// in pixels, the pair that gives a panel's real pixels per inch.
const HORZSIZE: i32 = 4;
const HORZRES: i32 = 8;
/// `MONITOR_DEFAULTTONEAREST` (winuser.h): the display the window is on.
const MONITOR_DEFAULTTONEAREST: u32 = 2;
/// The pill's smallest physical width: 24 mm, which is exactly what the
/// designed 96-pixel pill measures on a 100 PPI panel and what it was drawn to
/// feel like under the pointer.
const MIN_PILL_MM: i32 = 24;
/// Plausible physical density, in tenths of a dot per inch: 36 to 320 DPI.
/// A driver outside that range is reporting something that is not this
/// panel's size, and obeying it would make the chrome enormous.
const MIN_DPI_X10: i64 = 360;
const MAX_DPI_X10: i64 = 3200;

/// `MONITORINFOEXW` (winuser.h): the display's bounds and its device name —
/// the name `CreateDCW` needs before `GetDeviceCaps` answers for that panel
/// rather than for the desktop.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct MonitorInfoEx {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
    device: [u16; 32],
}

pin_layout!(MonitorInfoEx, 104; size = 0, monitor = 4, work = 20, flags = 36, device = 40);

/// The scale the frame and the bar are drawn at.
///
/// [`dpi_percent`] alone keeps the *logical* size constant, and Windows' scale
/// steps are too coarse for that to keep the *physical* size constant: the
/// same pill is 24 mm wide on a 15.6-inch 1366x768 panel at 100%, and only
/// 15 mm on a 14-inch 1920x1200 panel at 100%. The bar is aimed at, so it is
/// never allowed under [`MIN_PILL_MM`]: whichever is larger, the DPI scale or
/// the answer from the panel's own millimetres, wins.
pub(super) fn chrome_percent(window: Hwnd) -> i32 {
    let dpi = dpi_percent(window);
    monitor_size(window)
        .and_then(|(millimetres, pixels)| percent_for(millimetres, pixels))
        .map_or(dpi, |physical| dpi.max(physical))
}

/// The percent that makes the designed pill at least [`MIN_PILL_MM`] wide on
/// a panel `millimetres` across showing `pixels` pixels, or `None` when the
/// answer cannot be believed.
///
/// Integer arithmetic on purpose: the straight
/// `100 * MIN_PILL_MM * pixels / (design width * millimetres)`, with the
/// rounding added once before the divide. At the design's own 96 pixels and
/// 24 mm that is `25 * pixels / millimetres`.
pub(super) fn percent_for(millimetres: i32, pixels: i32) -> Option<i32> {
    if millimetres <= 0 || pixels <= 0 {
        return None;
    }
    let (millimetres, pixels) = (i64::from(millimetres), i64::from(pixels));
    // Physical DPI in tenths: pixels / millimetres * 25.4, kept integral.
    let dpi_x10 = pixels * 254 / millimetres;
    if !(MIN_DPI_X10..=MAX_DPI_X10).contains(&dpi_x10) {
        return None;
    }
    let design = i64::from(bar::DESIGN_WIDTH);
    let wanted = i64::from(MIN_PILL_MM) * 100 * pixels;
    let denominator = design * millimetres;
    let percent = (wanted * 2 + denominator) / (2 * denominator);
    let percent = i32::try_from(percent).unwrap_or(i32::MAX);
    Some(percent.clamp(100, 400))
}

/// The window's display: its width in millimetres and in pixels.
fn monitor_size(window: Hwnd) -> Option<(i32, i32)> {
    // SAFETY: a read-only query about a window this process owns.
    let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info = MonitorInfoEx {
        size: u32::try_from(size_of::<MonitorInfoEx>()).ok()?,
        monitor: Rect::default(),
        work: Rect::default(),
        flags: 0,
        device: [0; 32],
    };
    // SAFETY: `info` carries its own size and is filled in place; the monitor
    // handle came from a live window.
    if unsafe { GetMonitorInfoW(monitor, &raw mut info) } == 0 {
        return None;
    }
    let driver = wide("DISPLAY");
    // SAFETY: the driver and device names are NUL-terminated and outlive the
    // call; a display DC needs no port or initialisation data.
    let dc = unsafe {
        CreateDCW(
            driver.as_ptr(),
            info.device.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if dc.is_null() {
        return None;
    }
    // SAFETY: `dc` was just created and nothing between these reads can
    // unwind; the caps are plain integers for that device.
    let (millimetres, pixels) =
        unsafe { (GetDeviceCaps(dc, HORZSIZE), GetDeviceCaps(dc, HORZRES)) };
    // SAFETY: the same DC, released exactly once.
    unsafe { DeleteDC(dc) };
    (millimetres > 0 && pixels > 0).then_some((millimetres, pixels))
}

thread_local! {
    /// When the live frame appeared: its breath counts from here.
    static BORN: Cell<Option<Instant>> = const { Cell::new(None) };
}

#[link(name = "user32")]
unsafe extern "system" {
    pub(super) fn BeginPaint(window: Hwnd, paint: *mut c_void) -> Hdc;
    pub(super) fn LoadCursorW(instance: *mut c_void, cursor: *const u16) -> Hcursor;
    pub(super) fn EndPaint(window: Hwnd, paint: *const c_void) -> i32;
    pub(super) fn GetClientRect(window: Hwnd, rect: *mut Rect) -> i32;
    pub(super) fn GetDpiForWindow(window: Hwnd) -> u32;
    pub(super) fn MonitorFromWindow(window: Hwnd, flags: u32) -> *mut c_void;
    pub(super) fn GetMonitorInfoW(monitor: *mut c_void, info: *mut MonitorInfoEx) -> i32;
    pub(super) fn SetLayeredWindowAttributes(window: Hwnd, key: u32, alpha: u8, flags: u32) -> i32;
    pub(super) fn SetTimer(window: Hwnd, id: usize, elapse: u32, callback: *const c_void) -> usize;
    pub(super) fn KillTimer(window: Hwnd, id: usize) -> i32;
    pub(super) fn SetWindowDisplayAffinity(window: Hwnd, affinity: u32) -> i32;
    pub(super) fn SetWindowPos(
        window: Hwnd,
        insert_after: Hwnd,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    pub(super) fn InvalidateRect(window: Hwnd, rect: *const Rect, erase: i32) -> i32;
    pub(super) fn UpdateWindow(window: Hwnd) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    pub(super) fn CreateSolidBrush(color: u32) -> *mut c_void;
    /// A display DC, which is what answers a panel's own physical size.
    pub(super) fn CreateDCW(
        driver: *const u16,
        device: *const u16,
        port: *const u16,
        mode: *const c_void,
    ) -> Hdc;
    pub(super) fn DeleteDC(dc: Hdc) -> i32;
    pub(super) fn GetDeviceCaps(dc: Hdc, index: i32) -> i32;
    pub(super) fn DeleteObject(object: *mut c_void) -> i32;
    pub(super) fn FillRect(dc: Hdc, rect: *const Rect, brush: *mut c_void) -> i32;
    pub(super) fn SelectObject(dc: Hdc, object: *mut c_void) -> *mut c_void;
}

/// What a border means, which also decides how it paints.
#[derive(Clone)]
pub(crate) enum Style {
    /// The picker's selection box: a neutral dotted line around the
    /// hovered window or dragged rectangle. Screen-only — the frame is
    /// capture-excluded so it never lands in the image being picked.
    Selection,
    /// A live recording: the breathing frame, the control bar whose Stop
    /// button runs the carried action, and the audio switches of the take.
    Recording {
        /// What the Stop chip runs.
        action: StopAction,
        /// The take's live audio switches; the chips appear only for the
        /// sources that are actually capturing.
        audio: crate::audio::Live,
    },
}

/// A live frame; destroying it removes the outline and its control bar.
pub(crate) struct Border {
    window: Hwnd,
    /// The control bar; `None` for the picker's highlight, which must not
    /// offer a way to stop anything.
    bar: Option<Hwnd>,
}

impl Border {
    /// Draw the outline for `style` around `region` for as long as the
    /// guard lives.
    ///
    /// # Errors
    /// Windows refused the window; recording continues without a frame.
    pub(crate) fn show(region: Region, style: Style) -> io::Result<Self> {
        let width = i32::try_from(region.width).unwrap_or(0);
        let height = i32::try_from(region.height).unwrap_or(0);
        if width <= 0 || height <= 0 {
            return Err(io::Error::other("the capture region is empty"));
        }
        let instance = module_handle();
        let (class_name, proc_fn): (
            &str,
            unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult,
        ) = match style {
            Style::Recording { .. } => (CLASS_NAME, procedure),
            Style::Selection => (SELECTION_CLASS, selection_procedure),
        };
        register_border_class(instance, class_name, proc_fn);
        let class = wide(class_name);
        let style_ex =
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE;
        // SAFETY: strings and handles outlive the call; owned by this guard.
        let window = unsafe {
            CreateWindowExW(
                style_ex,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP | WS_VISIBLE,
                region.x,
                region.y,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null_mut(),
            )
        };
        if window.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `window` was created above and belongs to this thread.
        unsafe {
            // The picker's outline is fully opaque; the recording frame
            // breathes in a slower, quieter opacity of its own.
            match style {
                Style::Recording { .. } => {
                    BORN.with(|born| born.set(Some(Instant::now())));
                    SetLayeredWindowAttributes(
                        window,
                        COLOR_KEY,
                        breath(Duration::ZERO),
                        LWA_COLORKEY | LWA_ALPHA,
                    );
                    SetTimer(window, BREATH_TIMER, BREATH_STEP, std::ptr::null());
                }
                Style::Selection => {
                    SetLayeredWindowAttributes(window, COLOR_KEY, 0, LWA_COLORKEY);
                }
            }
            let _ = SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE);
        }
        let bar = match style {
            Style::Recording { action, audio } => show_bar(region, action, &audio),
            Style::Selection => None,
        };
        let border = Self { window, bar };
        border.place(region);
        Ok(border)
    }

    /// Move and resize the live outline; no-op if Windows refuses.
    pub(crate) fn place(&self, region: Region) {
        let (Ok(width), Ok(height)) = (i32::try_from(region.width), i32::try_from(region.height))
        else {
            return;
        };
        // SAFETY: the window is this guard's live topmost frame.
        unsafe {
            SetWindowPos(
                self.window,
                HWND_TOPMOST,
                region.x,
                region.y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            InvalidateRect(self.window, std::ptr::null(), 1);
            UpdateWindow(self.window);
            // Both windows are topmost, and within that band the one raised
            // last wins: without this the outline's `HWND_TOPMOST` promotion
            // puts the frame's edge over the control bar whenever it
            // is placed or moved. Re-raising the bar keeps it on top.
            if let Some(bar) = self.bar {
                SetWindowPos(
                    bar,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }
}

impl Drop for Border {
    fn drop(&mut self) {
        // Tearing the bar down disarms its action and audio switches, so a
        // stale click cannot reach the next take.
        if let Some(bar) = self.bar.take() {
            bar::destroy(bar);
        }
        BORN.with(|born| born.set(None));
        // SAFETY: the frame this guard created, and its own timer.
        unsafe {
            KillTimer(self.window, BREATH_TIMER);
            DestroyWindow(self.window);
        }
    }
}

/// Register a border window class; repeats answer "already registered".
///
/// The name buffer is kept for the process lifetime: `RegisterClassExW`
/// stores the pointer rather than a copy, so a stack buffer would dangle as
/// soon as this returns and the OS would read freed memory whenever the
/// class is looked up or unregistered.
fn register_border_class(
    instance: Hinstance,
    name: &'static str,
    wnd_proc: unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult,
) {
    let name = kept_wide(name);
    let class = WndClassExW {
        size: u32::try_from(std::mem::size_of::<WndClassExW>()).expect("class size fits u32"),
        style: 0,
        wnd_proc,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: std::ptr::null_mut(),
        cursor: std::ptr::null_mut(),
        background: std::ptr::null_mut(),
        menu_name: std::ptr::null(),
        class_name: name,
        icon_small: std::ptr::null_mut(),
    };
    // SAFETY: the class struct is a fully initialized local and the name
    // buffer outlives the registration.
    unsafe {
        RegisterClassExW(&raw const class);
    }
}

/// Paint the colour-key background and the frame, and breathe its opacity.
unsafe extern "system" fn procedure(
    window: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    match message {
        WM_PAINT => {
            // SAFETY: the paint state buffer is large enough for the
            // platform `PAINTSTRUCT` (72 bytes on 64-bit, 64 on 32-bit).
            unsafe { paint(window) };
            0
        }
        WM_TIMER => {
            breathe(window);
            0
        }
        // The whole surface is repainted in `WM_PAINT`.
        WM_ERASEBKGND => 1,
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Step the frame's opacity for one tick of its breath.
///
/// This changes a window attribute instead of repainting, so the live
/// pulse costs the capture nothing at all.
fn breathe(window: Hwnd) {
    let elapsed = BORN
        .with(Cell::get)
        .map(|born| born.elapsed())
        .unwrap_or_default();
    // SAFETY: a live layered window this process owns.
    unsafe {
        SetLayeredWindowAttributes(window, COLOR_KEY, breath(elapsed), LWA_COLORKEY | LWA_ALPHA);
    }
}

/// The frame's opacity `elapsed` into the take: [`BREATH_LOW`] to
/// [`BREATH_HIGH`] and back across [`BREATH_MS`].
///
/// The ramp is eased rather than linear, so the turn at each end settles
/// instead of ticking like a metronome, and the frame spends most of the
/// cycle near one of its two tones. That dwell is what makes it read as a
/// calm pulse at the edge of vision rather than a blinking border.
pub(super) fn breath(elapsed: Duration) -> u8 {
    let half = BREATH_MS / 2;
    let phase = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX) % BREATH_MS;
    // The half-cycle: out to the bright end, then back.
    let wave = if phase < half {
        phase
    } else {
        BREATH_MS - phase
    };
    // Smoothstep in fixed point: zero slope at both ends, steepest midway.
    let t = wave.saturating_mul(BREATH_SCALE) / half;
    let eased = (3 * t * t - 2 * t * t * t / BREATH_SCALE) / BREATH_SCALE;
    let span = u64::from(BREATH_HIGH - BREATH_LOW);
    let step = (span * eased + BREATH_SCALE / 2) / BREATH_SCALE;
    BREATH_LOW + u8::try_from(step).unwrap_or(0)
}

/// # Safety
/// `window` must be a live window owned by the calling thread.
unsafe fn paint(window: Hwnd) {
    let mut state = [0u8; 128];
    // SAFETY: `state` is at least `sizeof(PAINTSTRUCT)` for this ABI.
    let dc = unsafe { BeginPaint(window, state.as_mut_ptr().cast()) };
    if dc.is_null() {
        return;
    }
    let mut client = Rect::default();
    // SAFETY: writes only the local `client`.
    unsafe { GetClientRect(window, &raw mut client) };
    // SAFETY: fresh brushes are released after use; `FillRect` reads only
    // the local rects.
    unsafe {
        let key = CreateSolidBrush(COLOR_KEY);
        if !key.is_null() {
            FillRect(dc, &raw const client, key);
            DeleteObject(key);
        }
        let shade = CreateSolidBrush(FRAME_SHADE);
        let light = CreateSolidBrush(FRAME_LIGHT);
        let percent = chrome_percent(window);
        for (rect, tone) in frame_rects(client.right, client.bottom, percent) {
            let brush = match tone {
                Tone::Shade => shade,
                Tone::Light => light,
            };
            if !brush.is_null() {
                FillRect(dc, &raw const rect, brush);
            }
        }
        for brush in [shade, light] {
            if !brush.is_null() {
                DeleteObject(brush);
            }
        }
        EndPaint(window, state.as_ptr().cast());
    }
}

/// Which of the frame's two tones a rectangle is drawn in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Tone {
    /// The outer hairline: light, and flush with the app's edge, so the
    /// frame lands exactly where the window ends instead of leaving a dark
    /// strip of its own between the line and the edge.
    Light,
    /// The inner hairline: dark, so the light line has an edge to read
    /// against on pale content.
    Shade,
}

/// The recording frame for a `width` by `height` client area: a hairline
/// drawn light on the outer edge and dark just inside it. The light line is
/// what the eye reads, so it hugs the app's own boundary; the shaded inner
/// line is inset on every edge, which is what lines the corners up as one
/// continuous frame rather than four arms that each overshoot. Every
/// dimension is a designed pixel at 96 DPI and goes through [`scaled`], so
/// the frame keeps its weight on a high-DPI display instead of thinning to
/// half a point.
pub(super) fn frame_rects(width: i32, height: i32, percent: i32) -> Vec<(Rect, Tone)> {
    let mut rects = Vec::new();
    if width <= 0 || height <= 0 {
        return rects;
    }
    let percent = percent.clamp(100, 400);
    let hair = scaled(1, percent).max(1);
    let mut add = |left: i32, top: i32, right: i32, bottom: i32, tone: Tone| {
        // Each rect is normalized and clipped to the frame: a frame
        // thinner than its own hairline keeps only the part of each line
        // that exists.
        let (left, right) = (left.min(right), left.max(right));
        let (top, bottom) = (top.min(bottom), top.max(bottom));
        // A frame thinner than its own hairline keeps only the part of
        // each line that exists.
        let (left, top) = (left.max(0), top.max(0));
        let (right, bottom) = (right.min(width), bottom.min(height));
        if right > left && bottom > top {
            rects.push((
                Rect {
                    left,
                    top,
                    right,
                    bottom,
                },
                tone,
            ));
        }
    };
    // The hairline: light on the outer edge, where it lands on the app's
    // own boundary, and dark just inside it for contrast on pale content.
    // Each line is inset on all four edges, so its arms stop at the same
    // corner instead of each running out past the others.
    let inner = hair.saturating_mul(2);
    add(0, 0, width, hair, Tone::Light);
    add(0, height - hair, width, height, Tone::Light);
    add(0, 0, hair, height, Tone::Light);
    add(width - hair, 0, width, height, Tone::Light);
    add(hair, hair, width - hair, inner, Tone::Shade);
    add(
        hair,
        height - inner,
        width - hair,
        height - hair,
        Tone::Shade,
    );
    add(hair, hair, inner, height - hair, Tone::Shade);
    add(
        width - inner,
        hair,
        width - hair,
        height - hair,
        Tone::Shade,
    );
    rects
}

/// Paint the colour-key background and the dotted selection outline.
unsafe extern "system" fn selection_procedure(
    window: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    match message {
        WM_PAINT => {
            // SAFETY: the paint state buffer is large enough for the
            // platform `PAINTSTRUCT`.
            unsafe { paint_selection(window) };
            0
        }
        // The whole surface is repainted in `WM_PAINT`.
        WM_ERASEBKGND => 1,
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// # Safety
/// `window` must be a live window owned by the calling thread.
unsafe fn paint_selection(window: Hwnd) {
    let mut state = [0u8; 128];
    // SAFETY: `state` is at least `sizeof(PAINTSTRUCT)` for this ABI.
    let dc = unsafe { BeginPaint(window, state.as_mut_ptr().cast()) };
    if dc.is_null() {
        return;
    }
    let mut client = Rect::default();
    // SAFETY: writes only the local `client`.
    unsafe { GetClientRect(window, &raw mut client) };
    // SAFETY: fresh brushes are released after use; `FillRect` reads only
    // the local rects.
    unsafe {
        let key = CreateSolidBrush(COLOR_KEY);
        if !key.is_null() {
            FillRect(dc, &raw const client, key);
            DeleteObject(key);
        }
        let black = CreateSolidBrush(CROSSHAIR_BLACK);
        let white = CreateSolidBrush(CROSSHAIR_WHITE);
        if !black.is_null() || !white.is_null() {
            for (dot, is_white) in selection_dots(client.right, client.bottom) {
                let brush = if is_white { white } else { black };
                if !brush.is_null() {
                    FillRect(dc, &raw const dot, brush);
                }
            }
        }
        if !black.is_null() {
            DeleteObject(black);
        }
        if !white.is_null() {
            DeleteObject(white);
        }
        EndPaint(window, state.as_ptr().cast());
    }
}

/// Two-tone dots along a `width` by `height` client area.
pub(super) fn selection_dots(width: i32, height: i32) -> Vec<(Rect, bool)> {
    let mut dots = Vec::new();
    if width <= 0 || height <= 0 {
        return dots;
    }
    let size = SELECTION_DOT.min(width).min(height);
    let mut x = 0;
    while x < width {
        for (offset, white) in [(0, true), (SELECTION_DOT + 1, false)] {
            let left = x + offset;
            if left >= width {
                break;
            }
            let right = (left + size).min(width);
            dots.push((
                Rect {
                    left,
                    top: 0,
                    right,
                    bottom: size,
                },
                white,
            ));
            dots.push((
                Rect {
                    left,
                    top: height - size,
                    right,
                    bottom: height,
                },
                white,
            ));
        }
        x += SELECTION_PITCH;
    }
    let mut y = 0;
    while y < height {
        for (offset, white) in [(0, true), (SELECTION_DOT + 1, false)] {
            let top = y + offset;
            if top >= height {
                break;
            }
            let bottom = (top + size).min(height);
            dots.push((
                Rect {
                    left: 0,
                    top,
                    right: size,
                    bottom,
                },
                white,
            ));
            dots.push((
                Rect {
                    left: width - size,
                    top,
                    right: width,
                    bottom,
                },
                white,
            ));
        }
        y += SELECTION_PITCH;
    }
    dots
}
