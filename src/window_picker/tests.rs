use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use super::grabber::{CLASS_NAME as GRABBER_CLASS, handles};
use super::{
    Action, DRAG_THRESHOLD_PX, Hover, Input, Pick, Rect, Region, TRACK, Track, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, abandon, beyond_threshold, confirm_click,
    confirm_drag, drag_geometry, pick_within, region_from_rect, step,
};
use crate::overlay::{WS_EX_NOACTIVATE, WS_VISIBLE};
use crate::screen::win::window_bounds;
use crate::screen::window_region;
use crate::winapi::{
    CURSOR_SHOWING, CreateWindowExW, CursorInfo, DefWindowProcW, DestroyWindow, GA_ROOT,
    GetAncestor, GetCursorInfo, GetCursorPos, GetDC, GetSystemMetrics, GetWindowRect, Hcursor, Hdc,
    Hwnd, Msg, PeekMessageW, Point, RegisterClassExW, ReleaseDC, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP, WindowFromPoint, WndClassExW, kept_wide, module_handle, pump_until, wide,
};

#[link(name = "kernel32")]
unsafe extern "system" {
    /// The console host window this test process usually owns; a
    /// headless runner answers null and the test steps aside.
    fn GetConsoleWindow() -> *mut c_void;
}

#[link(name = "user32")]
unsafe extern "system" {
    /// Injects pointer events as if the hardware had produced them, so
    /// the real hook procedure receives them.
    fn SendInput(count: u32, inputs: *const InjectedInput, size: i32) -> u32;
    /// Parks the pointer without synthesizing any event.
    fn SetCursorPos(x: i32, y: i32) -> i32;
    /// Raises a window over the always-on-top band, the way an
    /// always-on-top application brings itself back to the front.
    fn SetWindowPos(
        window: Hwnd,
        insert_after: Hwnd,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    /// Puts a shape on the desktop's pointer, as a cursor-setting window
    /// does; the handle is a shared stock resource, nothing freed here.
    fn SetCursor(cursor: *mut c_void) -> *mut c_void;
}

/// `HWND_TOPMOST` and the `SWP_*` flags one raise needs (winuser.h).
const HWND_TOPMOST_RAISE: isize = -1;
const SWP_NOSIZE_RAISE: u32 = 0x0001;
const SWP_NOMOVE_RAISE: u32 = 0x0002;
const SWP_NOACTIVATE_RAISE: u32 = 0x0010;

/// `INPUT_MOUSE` and the `MOUSEEVENTF_*` flags (winuser.h).
const INPUT_MOUSE: u32 = 0;
const MOUSEEVENTF_MOVE: u32 = 0x0001;
const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;

/// `MOUSEINPUT` (winuser.h): one pointer event payload.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MouseInput {
    dx: i32,
    dy: i32,
    mouse_data: u32,
    flags: u32,
    time: u32,
    extra: usize,
}

/// `INPUT` (winuser.h) for the mouse arm of its union; named for its
/// one use here so it cannot be confused with the picker's `Input`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InjectedInput {
    kind: u32,
    padding: u32,
    mouse: MouseInput,
}

// The desktop reads both structures by size; a wrong one injects
// nothing (or lands somewhere else entirely).
const _: () = assert!(std::mem::size_of::<MouseInput>() == 32);
const _: () = assert!(std::mem::size_of::<InjectedInput>() == 40);

/// Synthetic input owns the pointer and every hook on the desktop, so
/// sessions that use it run one at a time.
static DESKTOP: Mutex<()> = Mutex::new(());
/// Set once the desktop accepted a synthetic event.
static INJECTED: AtomicBool = AtomicBool::new(false);
/// How long an injected session may idle: long enough for hook
/// installation and a click, short enough to fail rather than hang.
const INJECTED_IDLE: Duration = Duration::from_secs(5);
/// Time the session's window has to exist before the pointer starts moving.
const SETTLE: Duration = Duration::from_millis(250);
/// A real click on the desktop can end a session early, so a scenario
/// runs until it resolves. A deterministic mistake resolves the same
/// wrong way every time, so retries cannot hide one.
const ATTEMPTS: usize = 4;

/// Hand the desktop one pointer event; `false` when it refused it (no
/// interactive desktop to drive). Motion is optional: without
/// [`MOUSEEVENTF_MOVE`] the event lands wherever the pointer already
/// is and synthesizes no movement at all.
fn send(flags: u32, point: Point) -> bool {
    let (dx, dy) = normalized(point);
    let event = InjectedInput {
        kind: INPUT_MOUSE,
        padding: 0,
        mouse: MouseInput {
            dx,
            dy,
            mouse_data: 0,
            flags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            time: 0,
            extra: 0,
        },
    };
    let size = i32::try_from(std::mem::size_of::<InjectedInput>()).unwrap_or(0);
    // SAFETY: one fully initialized INPUT described by its own size.
    let sent = unsafe { SendInput(1, &raw const event, size) } == 1;
    if sent {
        INJECTED.store(true, Ordering::SeqCst);
    }
    sent
}

fn move_to(point: Point) -> bool {
    send(MOUSEEVENTF_MOVE, point)
}

fn press_at(point: Point) -> bool {
    send(MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTDOWN, point)
}

fn release_at(point: Point) -> bool {
    send(MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTUP, point)
}

/// One pointer event where the pointer already is.
///
/// No absolute coordinates and no motion flag, so the event carries the
/// position and moves nothing: a button pressed and released "here" stays
/// over whatever window the test parked the pointer on.
fn send_here(flags: u32) -> bool {
    let event = InjectedInput {
        kind: INPUT_MOUSE,
        padding: 0,
        mouse: MouseInput {
            dx: 0,
            dy: 0,
            mouse_data: 0,
            flags,
            time: 0,
            extra: 0,
        },
    };
    let size = i32::try_from(std::mem::size_of::<InjectedInput>()).unwrap_or(0);
    // SAFETY: one fully initialized INPUT described by its own size.
    let sent = unsafe { SendInput(1, &raw const event, size) } == 1;
    if sent {
        INJECTED.store(true, Ordering::SeqCst);
    }
    sent
}

fn press_here() -> bool {
    send_here(MOUSEEVENTF_LEFTDOWN)
}

fn release_here() -> bool {
    send_here(MOUSEEVENTF_LEFTUP)
}

/// Desktop coordinates as the `0..=65535` absolute range the flags ask
/// for, across the whole virtual desktop.
fn normalized(point: Point) -> (i32, i32) {
    // SAFETY: four parameterless queries about the display layout.
    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN) - 1,
            GetSystemMetrics(SM_CYVIRTUALSCREEN) - 1,
        )
    };
    (
        proportion(point.x, left, width),
        proportion(point.y, top, height),
    )
}

/// One axis of the absolute range; `extent` is the last addressable
/// pixel, so the two extremes map to `0` and `65535`.
fn proportion(value: i32, origin: i32, extent: i32) -> i32 {
    if extent <= 0 {
        return 0;
    }
    let scaled = i64::from(value - origin) * 65535 / i64::from(extent);
    i32::try_from(scaled).unwrap_or(0)
}

/// A topmost window the test owns: something known to click, with a
/// frame the recorded region can be compared against.
struct Target {
    window: Hwnd,
    /// The window as plain bits, the way a pick answers it.
    handle: usize,
    /// What `GetWindowRect` answered when it was created.
    rect: Rect,
}

impl Target {
    fn show(class: &'static str, x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
        Self::shown(class, std::ptr::null_mut(), x, y, width, height)
    }

    /// A target that asks for a pointer of its own, the way a text field asks
    /// for the I-beam: the shape on screen is that one unless the pick's own
    /// window is what the pointer is over.
    fn with_cursor(
        class: &'static str,
        id: usize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Option<Self> {
        // SAFETY: a shared stock cursor resource; nothing is freed here and
        // the handle outlives the class that carries it.
        let cursor = unsafe { LoadCursorW(std::ptr::null_mut(), id as *const u16) };
        Self::shown(class, cursor, x, y, width, height)
    }

    fn shown(
        class: &'static str,
        cursor: Hcursor,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Option<Self> {
        let instance = module_handle();
        let name = kept_wide(class);
        let definition = WndClassExW {
            size: u32::try_from(std::mem::size_of::<WndClassExW>()).ok()?,
            style: 0,
            wnd_proc: DefWindowProcW,
            cls_extra: 0,
            wnd_extra: 0,
            instance,
            icon: std::ptr::null_mut(),
            cursor,
            background: std::ptr::null_mut(),
            menu_name: std::ptr::null(),
            class_name: name,
            icon_small: std::ptr::null_mut(),
        };
        // SAFETY: the class struct is a fully initialized local and the
        // kept name outlives the process.
        unsafe { RegisterClassExW(&raw const definition) };
        let title = wide(class);
        // SAFETY: strings and handles outlive the call; the window is
        // owned by this thread and destroyed in `Drop`.
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
            return None;
        }
        let mut rect = Rect::default();
        // SAFETY: `rect` is a live local; `window` was created above.
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

    fn center(&self) -> Point {
        Point {
            x: i32::midpoint(self.rect.left, self.rect.right),
            y: i32::midpoint(self.rect.top, self.rect.bottom),
        }
    }

    /// The region a capture of this window saves.
    fn frame(&self) -> Region {
        region_from_rect(window_bounds(self.window).expect("the target has bounds"))
    }

    /// The region the picker answers for this window.
    fn recorded(&self) -> Region {
        window_region(Some(self.handle)).expect("the target is recordable")
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        // SAFETY: the window this guard created on this thread.
        unsafe { DestroyWindow(self.window) };
    }
}

/// Put the pointer back where the user left it.
struct PointerGuard(Point);

impl PointerGuard {
    fn take() -> Self {
        let mut point = Point::default();
        // SAFETY: writes only the local point.
        unsafe { GetCursorPos(&raw mut point) };
        Self(point)
    }
}

impl Drop for PointerGuard {
    fn drop(&mut self) {
        // SAFETY: the session that owned the pointer has ended.
        unsafe { SetCursorPos(self.0.x, self.0.y) };
    }
}

/// One real pick session, driven by `inject` from another thread
/// because the session thread is busy pumping its own messages.
fn pick_with(inject: impl Fn() + Send + Copy + 'static) -> Option<Pick> {
    let injector = std::thread::spawn(move || {
        // The session's window exists before the pump starts; let that happen
        // instead of racing it.
        sleep(SETTLE);
        inject();
    });
    let answer = pick_within(INJECTED_IDLE).expect("the picker installs its hooks");
    let _ = injector.join();
    answer
}

/// The root window under `point`, as the picker resolves it.
fn root_at(point: Point) -> usize {
    // SAFETY: point lookup and handle walk; the window is inspected
    // only.
    unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) as usize }
}
/// Whether both points still name the windows this test owns.
fn quiet(points: &[(Point, usize)]) -> bool {
    points
        .iter()
        .all(|(point, handle)| root_at(*point) == *handle)
}

/// Whether the pointer actually reached `point`: a session without an
/// interactive desktop takes injected events and delivers none.
///
/// Absolute pointer input is quantized to 0..=65535, so a position can land a
/// pixel away from the one asked for; the test is about the pointer having
/// travelled, not about the last bit of the rounding.
fn cursor_at(point: Point) -> bool {
    let mut at = Point::default();
    // SAFETY: writes only the local point.
    let found = unsafe { GetCursorPos(&raw mut at) } != 0;
    found && (at.x - point.x).abs() <= 2 && (at.y - point.y).abs() <= 2
}

/// The exact regression, end to end: the session starts with the
/// pointer over one window (the tray, in the report) and the click
/// lands on another. Only the clicked window may be recorded.
#[test]
fn injected_input_records_the_window_under_the_click() {
    let _desktop = DESKTOP.lock().unwrap_or_else(PoisonError::into_inner);
    let _pointer = PointerGuard::take();
    let Some(decoy) = Target::show("screencapture.test.decoy", 40, 40, 240, 180) else {
        return; // no desktop to drive
    };
    let Some(target) = Target::show("screencapture.test.target", 420, 140, 420, 320) else {
        return;
    };
    let parked = decoy.center();
    let clicked = target.center();
    if !quiet(&[(parked, decoy.handle), (clicked, target.handle)]) {
        eprintln!("skipping: another window covers the test target");
        return;
    }
    // Park the pointer on the decoy and hold the button there: the
    // session starts with the decoy highlighted, the way a pick begun
    // over the tray highlights the taskbar, and its press predates the
    // hooks.
    INJECTED.store(false, Ordering::SeqCst);
    let mut answer = None;
    for _ in 0..ATTEMPTS {
        // SAFETY: no session is live between attempts.
        unsafe { SetCursorPos(parked.x, parked.y) };
        press_here();
        answer = pick_with(move || {
            // Relocate without motion, then release: this is the event
            // burst a slow hook callback loses. The picker sees a
            // release with no press and no move behind it.
            // SAFETY: the session owns the pointer while it runs.
            unsafe { SetCursorPos(clicked.x, clicked.y) };
            sleep(Duration::from_millis(80));
            release_here();
        });
        if matches!(answer, Some(Pick::Window(handle)) if handle == target.handle) {
            break;
        }
    }
    if !cursor_at(clicked) || (answer.is_none() && !INJECTED.load(Ordering::SeqCst)) {
        eprintln!("skipping: this desktop does not take synthetic input");
        return;
    }
    assert_eq!(
        answer,
        Some(Pick::Window(target.handle)),
        "the click answers the window under it, not the highlight from session start"
    );
    assert_eq!(
        target.recorded(),
        target.frame(),
        "the recorded region is the frame the pick answered"
    );
    assert_ne!(
        target.recorded(),
        decoy.frame(),
        "and it is not the window the session started on"
    );
    assert_eq!(
        root_at(clicked),
        target.handle,
        "the pick's overlay is gone, so what a capture sees there is the window it answered"
    );
}

/// A real drag through the same session answers the rectangle it drew, so the
/// press, motion and release path stays wired to the state machine.
#[test]
fn injected_input_records_the_dragged_rectangle() {
    let _desktop = DESKTOP.lock().unwrap_or_else(PoisonError::into_inner);
    let _pointer = PointerGuard::take();
    let Some(target) = Target::show("screencapture.test.drag", 300, 200, 520, 380) else {
        return;
    };
    let from = Point {
        x: target.rect.left + 40,
        y: target.rect.top + 40,
    };
    let to = Point {
        x: target.rect.left + 300,
        y: target.rect.top + 260,
    };
    if !quiet(&[(from, target.handle), (to, target.handle)]) {
        eprintln!("skipping: another window covers the test target");
        return;
    }
    // What a faithful drag has to produce. A busy desktop can drop the
    // press or one of the moves, so an attempt only counts once the region
    // matches; the retry exists for that, not to paper over wrong geometry.
    let expected = Region {
        x: from.x.min(to.x),
        y: from.y.min(to.y),
        width: u32::try_from((to.x - from.x).abs()).unwrap_or(0),
        height: u32::try_from((to.y - from.y).abs()).unwrap_or(0),
    };
    // Absolute pointer input is quantized to 0..=65535, so an edge can
    // land a pixel away from the point that was asked for.
    let close_enough = |region: &Region| {
        (region.x - expected.x).abs() <= 2
            && (region.y - expected.y).abs() <= 2
            && region.width.abs_diff(expected.width) <= 2
            && region.height.abs_diff(expected.height) <= 2
    };
    INJECTED.store(false, Ordering::SeqCst);
    let mut answer = None;
    for _ in 0..ATTEMPTS {
        // Another test's window can land over the target while this binary
        // runs its other cases; driving the drag then would record that
        // window instead, so step aside rather than assert on it.
        if !quiet(&[(from, target.handle), (to, target.handle)]) {
            eprintln!("skipping: the desktop changed under the test target");
            return;
        }
        answer = pick_with(move || {
            move_to(from);
            sleep(Duration::from_millis(80));
            press_at(from);
            // Walk to the far corner: the marquee follows real motion.
            for step_count in 1..=4 {
                move_to(Point {
                    x: from.x + (to.x - from.x) * step_count / 4,
                    y: from.y + (to.y - from.y) * step_count / 4,
                });
                sleep(Duration::from_millis(40));
            }
            move_to(to);
            sleep(Duration::from_millis(60));
            release_at(to);
        });
        if matches!(&answer, Some(Pick::Region(region)) if close_enough(region)) {
            break;
        }
    }
    if !cursor_at(to) || (answer.is_none() && !INJECTED.load(Ordering::SeqCst)) {
        eprintln!("skipping: this desktop does not take synthetic input");
        return;
    }
    let Some(Pick::Region(region)) = answer else {
        panic!("expected a dragged region, got {answer:?}");
    };
    assert!(
        close_enough(&region),
        "dragged {region:?}, expected about {expected:?}"
    );
    assert_eq!(
        target.recorded(),
        target.frame(),
        "a click on the same window still records its frame"
    );
}

/// The regression that came back from Task Manager: an always-on-top
/// application that raises itself while the pick is live lands over the
/// pick's own surface, and the session then hears nothing at all — no move,
/// no click — until the idle limit ends it, which reads as a hang. The
/// session claims the top of the band back while it runs.
#[test]
fn a_pick_survives_a_window_raising_itself_over_the_surface() {
    let _desktop = DESKTOP.lock().unwrap_or_else(PoisonError::into_inner);
    let _pointer = PointerGuard::take();
    let Some(target) = Target::show("screencapture.test.raised", 300, 200, 520, 380) else {
        return;
    };
    let at = target.center();
    if !quiet(&[(at, target.handle)]) {
        eprintln!("skipping: another window covers the test target");
        return;
    }
    INJECTED.store(false, Ordering::SeqCst);
    let raised = target.handle;
    let mut answer = None;
    for _ in 0..ATTEMPTS {
        answer = pick_with(move || {
            // The raise an always-on-top application performs on its own,
            // with the session already running.
            // SAFETY: a live window this process owns; z-order only.
            unsafe {
                SetWindowPos(
                    std::ptr::with_exposed_provenance_mut::<c_void>(raised),
                    HWND_TOPMOST_RAISE as Hwnd,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOSIZE_RAISE | SWP_NOMOVE_RAISE | SWP_NOACTIVATE_RAISE,
                );
            }
            // Long enough for the session's own pump to notice the raise and
            // claim the band back before the click is aimed.
            sleep(Duration::from_millis(400));
            move_to(at);
            sleep(Duration::from_millis(120));
            press_at(at);
            sleep(Duration::from_millis(80));
            release_at(at);
        });
        if matches!(answer, Some(Pick::Window(handle)) if handle == raised) {
            break;
        }
    }
    if !cursor_at(at) || (answer.is_none() && !INJECTED.load(Ordering::SeqCst)) {
        eprintln!("skipping: this desktop does not take synthetic input");
        return;
    }
    assert_eq!(
        answer,
        Some(Pick::Window(target.handle)),
        "a session a window raised itself over still hears the click and answers the window under it"
    );
}

/// A window whose thread never pumps: the stand-in for one this process may
/// not send messages to. Windows drops a message aimed at a window of a
/// higher integrity before any pump could see it, so such a window answers
/// exactly as this one does — nothing.
struct Unpumped {
    /// The window, as plain bits.
    handle: usize,
    /// Released when the guard drops, so the thread destroys the window.
    release: Option<std::sync::mpsc::Sender<()>>,
    /// The thread that owns it.
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Unpumped {
    /// Show one offscreen on a thread of its own, which then waits without
    /// pumping anything; `None` when the desktop refuses the window.
    fn show(class: &'static str, cursor: usize) -> Option<Self> {
        let (ready, shown) = std::sync::mpsc::channel();
        let (release, hold) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let target = Target::with_cursor(class, cursor, -600, -600, 120, 90);
            let _ = ready.send(target.as_ref().map(|target| target.handle));
            let _ = hold.recv();
            drop(target);
        });
        let handle = shown.recv().ok().flatten()?;
        Some(Self {
            handle,
            release: Some(release),
            thread: Some(thread),
        })
    }

    fn window(&self) -> Hwnd {
        std::ptr::with_exposed_provenance_mut::<c_void>(self.handle)
    }
}

impl Drop for Unpumped {
    fn drop(&mut self) {
        self.release = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A window that cannot answer a cursor request would leave the session's own
/// pointer on screen: the shape stays what the last `WM_SETCURSOR` set, and
/// nothing asks this one again. The hand-back installs the shape itself.
#[test]
fn a_window_that_cannot_answer_gets_its_pointer_back_anyway() {
    let _desktop = DESKTOP.lock().unwrap_or_else(PoisonError::into_inner);
    let _pointer = PointerGuard::take();
    let (Some(arrow), Some(crosshair), Some(beam)) = (
        stock_shape(ARROW),
        stock_shape(CROSSHAIR),
        stock_shape(IDC_IBEAM),
    ) else {
        eprintln!("skipping: this desktop cannot render a cursor");
        return;
    };
    let Some(unpumped) = Unpumped::show("screencapture.test.unpumped", IDC_IBEAM) else {
        eprintln!("skipping: the desktop refuses a window for the stand-in");
        return;
    };
    let window = unpumped.window();
    assert!(
        !crate::winapi::request_cursor(window),
        "a window whose thread never pumps cannot answer a cursor request"
    );
    // Park the pointer over a window of this process: another application's
    // window re-asks for its own shape while the test runs, and the sample
    // below is about the shape this one leaves behind.
    let Some(holder) = Target::show("screencapture.test.holder", 200, 200, 400, 300) else {
        return;
    };
    let at = holder.center();
    if !quiet(&[(at, holder.handle)]) {
        eprintln!("skipping: another window covers the test window");
        return;
    }
    move_to(at);
    sleep(Duration::from_millis(200));
    // Put the session's own pointer up, the shape a pick over that window
    // would leave behind.
    // SAFETY: a shared stock resource; nothing is freed here.
    let cross = unsafe { LoadCursorW(std::ptr::null_mut(), CROSSHAIR as *const u16) };
    // SAFETY: the shape is a live stock handle.
    unsafe { SetCursor(cross) };
    if wait_for_cursor(&crosshair).as_deref() != Some(crosshair.as_slice()) {
        eprintln!("skipping: the desktop did not keep the shape a test set");
        return;
    }
    super::grabber::restore_cursor(window);
    let drawn = drawn_cursor();
    // Put the desktop's ordinary pointer back, whatever the answer was, so a
    // failure here is not a stuck I-beam for whoever is using the machine.
    // SAFETY: a shared stock resource; nothing is freed here.
    let plain = unsafe { LoadCursorW(std::ptr::null_mut(), ARROW as *const u16) };
    // SAFETY: the shape is a live stock handle.
    unsafe { SetCursor(plain) };
    // Counted rather than printed: a whole cursor is thousands of pixels of
    // assertion noise, and the count tells the two shapes apart just as well.
    let pixels = |shape: &Option<Vec<bool>>| {
        shape
            .as_ref()
            .map_or(0, |shape| shape.iter().filter(|on| **on).count())
    };
    assert!(
        drawn.as_deref() == Some(beam.as_slice()),
        "a window that cannot be asked still gets its own pointer back: the desktop drew {} pixels, its own pointer is {}",
        pixels(&drawn),
        beam.iter().filter(|on| **on).count()
    );
    // `arrow` is the shape the cleanup put back; nothing is asserted about it,
    // so a window that answers late cannot fail the case on its way out.
    drop(arrow);
}

/// A window at a known frame, as the hook resolves it.
fn window(handle: usize, x: i32, y: i32, width: u32, height: u32) -> Hover {
    Hover {
        handle,
        region: Region {
            x,
            y,
            width,
            height,
        },
    }
}

fn point(x: i32, y: i32) -> super::Point {
    super::Point { x, y }
}

#[test]
fn every_pointer_message_reaches_the_session() {
    // The overlay is the window under the pointer for the whole session, so
    // all of these are delivered to it, and the desktop moves the cursor
    // itself: nothing has to be passed through to keep the pointer moving.
    for message in [WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN] {
        assert!(
            handles(message),
            "message {message:#06x} has to reach the picker"
        );
    }
    assert!(!handles(0), "and nothing else is a pointer event");
}

/// The regression that put the taskbar in recordings: Windows skips a
/// hook callback that runs long instead of queueing it, so the press
/// can go missing. The picker then answered the window it had
/// highlighted at session start — a pick begun over the tray saved the
/// taskbar however carefully the window itself was clicked.
#[test]
fn a_release_without_its_press_answers_the_release_point() {
    let taskbar = window(1, 1, 1153, 1918, 46);
    let editor = window(2, 240, 126, 1440, 900);
    let mut track = Track {
        handle: Some(taskbar.handle),
        ..Track::default()
    };
    assert_eq!(
        step(
            &mut track,
            Input::Release {
                point: point(300, 300),
                under: Some(editor)
            }
        ),
        Action::Clear
    );
    assert_eq!(track.picked, Some(Pick::Window(editor.handle)));
    assert!(track.done);
}

#[test]
fn a_click_answers_the_window_under_the_release() {
    let left = window(1, 0, 0, 100, 100);
    let right = window(2, 100, 0, 100, 100);
    let mut track = Track::default();
    assert_eq!(
        step(
            &mut track,
            Input::Press {
                point: point(50, 50),
                under: Some(left),
            }
        ),
        Action::Outline(left.region)
    );
    // A press that barely moves still answers where the release landed.
    assert_eq!(
        step(
            &mut track,
            Input::Release {
                point: point(120, 50),
                under: Some(right),
            }
        ),
        Action::Clear
    );
    assert_eq!(track.picked, Some(Pick::Window(right.handle)));
}

#[test]
fn a_press_past_the_threshold_drags_and_the_release_answers_it() {
    let origin = point(600, 300);
    let held = window(1, 0, 0, 1920, 1200);
    let mut track = Track::default();
    assert_eq!(
        step(
            &mut track,
            Input::Press {
                point: origin,
                under: Some(held)
            }
        ),
        Action::Outline(held.region)
    );
    // Inside the threshold the press is still a click candidate.
    assert_eq!(
        step(
            &mut track,
            Input::Move {
                point: point(600 + DRAG_THRESHOLD_PX, 300),
                under: Some(held),
            }
        ),
        Action::Keep
    );
    assert!(!track.dragging);
    // Past it the marquee takes over; the window under the pointer no
    // longer decides anything.
    let dragged = Region {
        x: 600,
        y: 300,
        width: 300,
        height: 300,
    };
    assert_eq!(
        step(
            &mut track,
            Input::Move {
                point: point(900, 600),
                under: None,
            }
        ),
        Action::Outline(dragged)
    );
    assert!(track.dragging);
    assert_eq!(
        step(
            &mut track,
            Input::Release {
                point: point(900, 600),
                under: Some(window(7, 0, 0, 10, 10))
            }
        ),
        Action::Clear
    );
    assert_eq!(track.picked, Some(Pick::Region(dragged)));
}

#[test]
fn a_press_during_a_live_drag_starts_a_new_gesture() {
    // With the release skipped, `dragging` used to stay armed: the next
    // press was ignored, so clicking did nothing and only that click's
    // release ended the pick — with the stale marquee as the answer.
    let stale = Region {
        x: 10,
        y: 10,
        width: 40,
        height: 40,
    };
    let mut track = Track {
        dragging: true,
        origin: Some(point(10, 10)),
        marquee: Some(stale),
        handle: Some(9),
        ..Track::default()
    };
    let restarted = window(3, 100, 100, 200, 200);
    assert_eq!(
        step(
            &mut track,
            Input::Press {
                point: point(800, 600),
                under: Some(restarted)
            }
        ),
        Action::Outline(restarted.region)
    );
    assert!(!track.dragging, "the stale drag is over");
    assert!(track.marquee.is_none(), "its outline is gone");
    assert_eq!(track.handle, Some(restarted.handle));
    assert_eq!(
        step(
            &mut track,
            Input::Release {
                point: point(820, 620),
                under: Some(restarted)
            }
        ),
        Action::Clear
    );
    assert_eq!(track.picked, Some(Pick::Window(restarted.handle)));
}

#[test]
fn the_outline_only_moves_when_the_window_does() {
    // Re-aiming must not repaint a full-window frame on every move:
    // that work runs inside the hook callback, which the system skips
    // when it takes too long.
    let editor = window(4, 240, 126, 1440, 900);
    let mut track = Track::default();
    assert_eq!(
        step(
            &mut track,
            Input::Move {
                point: point(400, 400),
                under: Some(editor)
            }
        ),
        Action::Outline(editor.region)
    );
    assert_eq!(
        step(
            &mut track,
            Input::Move {
                point: point(500, 420),
                under: Some(editor)
            }
        ),
        Action::Keep
    );
    assert_eq!(
        step(
            &mut track,
            Input::Move {
                point: point(500, 420),
                under: None
            }
        ),
        Action::Clear
    );
    assert_eq!(track.handle, None);
}

#[test]
fn movement_without_a_press_never_drags() {
    // A press the picker never saw must not become a marquee.
    let screen = window(5, 0, 0, 1920, 1200);
    let mut track = Track::default();
    step(
        &mut track,
        Input::Move {
            point: point(10, 10),
            under: Some(screen),
        },
    );
    assert_eq!(
        step(
            &mut track,
            Input::Move {
                point: point(900, 700),
                under: Some(screen)
            }
        ),
        Action::Keep
    );
    assert!(!track.dragging);
    assert!(track.marquee.is_none());
}

#[test]
fn a_release_over_no_window_answers_nothing() {
    let mut track = Track {
        handle: Some(7),
        ..Track::default()
    };
    assert_eq!(
        step(
            &mut track,
            Input::Release {
                point: point(10, 10),
                under: None,
            }
        ),
        Action::Clear
    );
    assert_eq!(track.picked, None);
    assert!(track.done);
}

#[test]
fn cancelling_answers_nothing_and_drops_the_marquee() {
    let mut track = Track {
        dragging: true,
        origin: Some(point(10, 10)),
        marquee: Some(Region {
            x: 10,
            y: 10,
            width: 40,
            height: 40,
        }),
        handle: Some(9),
        ..Track::default()
    };
    assert_eq!(step(&mut track, Input::Cancel), Action::Clear);
    assert!(track.done);
    assert!(track.picked.is_none());
    assert!(track.origin.is_none());
    assert!(track.marquee.is_none());
    assert!(!track.dragging);
}

/// The idle guard clears the answer instead of leaving a stale target
/// behind, and posts no quit message (the pump is already gone).
#[test]
fn an_idle_session_abandons_instead_of_answering() {
    TRACK.with_borrow_mut(|track| {
        *track = Track::default();
        track.handle = Some(7);
        track.origin = Some(super::Point { x: 10, y: 10 });
        track.dragging = true;
    });
    abandon();
    TRACK.with_borrow(|track| {
        assert!(track.done, "the session ends");
        assert!(track.picked.is_none(), "nothing is answered");
        assert!(track.origin.is_none(), "the press is cleared");
    });
}

#[test]
fn a_real_window_answers_bounds() {
    // SAFETY: parameterless question about our own process.
    let handle = unsafe { GetConsoleWindow() };
    if handle.is_null() {
        return;
    }
    let bounds = window_bounds(handle).expect("console window bounds");
    assert!(bounds.right > bounds.left && bounds.bottom > bounds.top);
}

#[test]
fn fresh_tracks_answer_nothing() {
    let track = Track::default();
    assert!(
        track.handle.is_none()
            && track.picked.is_none()
            && track.highlight.is_none()
            && !track.dragging
            && track.origin.is_none()
    );
}

#[test]
fn click_and_drag_differ_by_the_threshold() {
    let origin = super::Point { x: 500, y: 500 };
    let inside = super::Point {
        x: 500 + DRAG_THRESHOLD_PX,
        y: 500,
    };
    let outside = super::Point {
        x: 500 + DRAG_THRESHOLD_PX + 1,
        y: 500,
    };
    assert!(!beyond_threshold(origin, inside));
    assert!(beyond_threshold(origin, outside));
}

#[test]
fn dragged_geometry_normalizes_and_never_zeroes() {
    // Drag up-left from bottom-right: rect still reads top-left.
    let (rect, region) = drag_geometry(
        super::Point { x: 900, y: 600 },
        super::Point { x: 300, y: 200 },
    );
    assert_eq!(rect.left, 300);
    assert_eq!(rect.top, 200);
    assert_eq!(region.x, 300);
    assert_eq!(region.y, 200);
    assert_eq!(region.width, 600);
    assert_eq!(region.height, 400);
    // One-pixel guarantee either way.
    let (_, tiny) = drag_geometry(super::Point { x: 7, y: 7 }, super::Point { x: 7, y: 7 });
    assert_eq!((tiny.width, tiny.height), (1, 1));
}

/// The release is the last word on where the drag ended. Reading only the
/// last processed move saved a rectangle that stopped short of the pointer:
/// under load the hook callback is skipped, and the moves it lost are
/// exactly the ones at the end of a fast drag.
#[test]
fn the_release_point_finishes_the_region() {
    let held = window(1, 0, 0, 1920, 1200);
    let mut track = Track::default();
    step(
        &mut track,
        Input::Press {
            point: point(600, 300),
            under: Some(held),
        },
    );
    // The last move the hook managed to deliver, well short of the release.
    step(
        &mut track,
        Input::Move {
            point: point(700, 400),
            under: None,
        },
    );
    assert!(track.dragging);
    assert_eq!(
        step(
            &mut track,
            Input::Release {
                point: point(900, 650),
                under: None,
            }
        ),
        Action::Clear
    );
    assert_eq!(
        track.picked,
        Some(Pick::Region(Region {
            x: 600,
            y: 300,
            width: 300,
            height: 350,
        })),
        "the region follows the release, not the stale move"
    );
}

/// The same rule in the other direction: a release inside the last move
/// shrinks the region instead of leaving it stretched past the pointer.
#[test]
fn a_release_pulled_back_shrinks_the_region() {
    let held = window(1, 0, 0, 1920, 1200);
    let mut track = Track::default();
    step(
        &mut track,
        Input::Press {
            point: point(200, 200),
            under: Some(held),
        },
    );
    step(
        &mut track,
        Input::Move {
            point: point(900, 800),
            under: None,
        },
    );
    step(
        &mut track,
        Input::Release {
            point: point(500, 450),
            under: None,
        },
    );
    assert_eq!(
        track.picked,
        Some(Pick::Region(Region {
            x: 200,
            y: 200,
            width: 300,
            height: 250,
        }))
    );
}

#[test]
fn drag_release_keeps_the_region_answer() {
    let region = super::Region {
        x: 10,
        y: 20,
        width: 300,
        height: 200,
    };
    let mut track = Track {
        marquee: Some(region),
        dragging: true,
        origin: Some(super::Point { x: 0, y: 0 }),
        ..Track::default()
    };
    confirm_drag(&mut track);
    assert_eq!(track.picked, Some(Pick::Region(region)));
    assert!(track.done);
    assert!(track.marquee.is_none());
}

#[test]
fn click_release_keeps_the_window_answer() {
    let mut track = Track {
        handle: Some(42),
        ..Track::default()
    };
    confirm_click(&mut track);
    assert_eq!(track.picked, Some(Pick::Window(42)));
    assert!(track.done);
}

#[test]
fn pick_variants_stay_exhaustive() {
    // Sanity on the enum shape the app layer matches.
    assert_eq!(Pick::Window(1), Pick::Window(1));
    assert_ne!(
        Pick::Window(1),
        Pick::Region(super::Region {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        })
    );
}

// ---------------------------------------------------------------------
// The pointer a pick shows: one crosshair, drawn by Windows itself.
//
// The regression here is the "crosshairs *and* the pointer" report. The
// session used to paint a crosshair marker of its own around the arrow, so
// two pointers were on screen — and the painted one only moved when a hook
// callback ran, so a slow callback left it standing where it last was, which
// read as a frozen crosshair. Both symptoms had the same root, a crosshair
// of ours, so both are pinned the same way: what the desktop draws while a
// pick is live must be the stock crosshair, and our process must own no
// window that could paint a second one.
//
// The shape is measured through `GetCursorInfo`, which answers the cursor of
// the window under the pointer — exactly the thing that chose the shape in
// the first place, and the reason a text field's I-beam used to win over
// anything the session installed system-wide. A pick is right when that
// window is ours and the shape it asks for is the crosshair.
// ---------------------------------------------------------------------

/// The stock pointers, by resource id (`MAKEINTRESOURCE(IDC_*)`).
const ARROW: usize = crate::winapi::IDC_ARROW;
const CROSSHAIR: usize = crate::winapi::IDC_CROSS;
/// `IDC_IBEAM` (winuser.h): the text-field pointer, listed only by the test
/// that needs the pick to win over it.
const IDC_IBEAM: usize = 32513;

/// `BLACKNESS` (wingdi.h): fill a region with the stock black brush.
const BLACKNESS: u32 = 0x0042;
/// `DIB_RGB_COLORS` (wingdi.h): the pixel data is literal colour.
const DIB_RGB_COLORS: u32 = 0;
/// `DI_NORMAL` (winuser.h): draw a cursor the way the desktop shows it.
const DI_NORMAL: u32 = 3;
/// The square a cursor is rendered onto, the box its body is drawn in, and
/// the margin that keeps the whole cursor inside whatever its hotspot is.
const CANVAS: i32 = 64;
const BODY: i32 = 32;
const MARGIN: i32 = 16;
/// The selection outline's window class: the frame a pick paints over its
/// pointer surface (`GRABBER_CLASS`).
const OUTLINE_CLASS: &str = "screencapture.selection.frame";
/// `MOUSEEVENTF_RIGHTDOWN` and `MOUSEEVENTF_RIGHTUP` (winuser.h): the
/// picker's cancel press and the release that leaves no button held.
const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;

/// `BITMAPINFOHEADER` (wingdi.h).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    image_size: u32,
    pixels_per_meter_x: i32,
    pixels_per_meter_y: i32,
    used: u32,
    important: u32,
}

/// `RGBQUAD` (wingdi.h).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RgbQuad {
    blue: u8,
    green: u8,
    red: u8,
    reserved: u8,
}

/// `BITMAPINFO` (wingdi.h): a header and a palette the 32-bit layout ignores.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfo {
    header: BitmapHeader,
    colors: [RgbQuad; 3],
}

#[link(name = "user32")]
unsafe extern "system" {
    /// A stock cursor resource: `MAKEINTRESOURCE(IDC_*)` is the id itself.
    fn LoadCursorW(instance: *mut c_void, cursor: *const u16) -> *mut c_void;
    /// Paints an icon or cursor into a device context.
    fn DrawIconEx(
        dc: Hdc,
        x: i32,
        y: i32,
        icon: *mut c_void,
        width: i32,
        height: i32,
        step: u32,
        brush: *mut c_void,
        flags: u32,
    ) -> i32;
    /// Visits every top-level window until the callback answers `0`.
    fn EnumWindows(visit: unsafe extern "system" fn(Hwnd, isize) -> i32, data: isize) -> i32;
    /// The window's process, and its answer is the thread that created it.
    fn GetWindowThreadProcessId(window: Hwnd, process: *mut u32) -> u32;
    fn GetClassNameW(window: Hwnd, name: *mut u16, size: i32) -> i32;
    fn IsWindowVisible(window: Hwnd) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: Hdc) -> Hdc;
    fn CreateCompatibleBitmap(dc: Hdc, width: i32, height: i32) -> *mut c_void;
    fn DeleteDC(dc: Hdc) -> i32;
    fn SelectObject(dc: Hdc, object: *mut c_void) -> *mut c_void;
    fn DeleteObject(object: *mut c_void) -> i32;
    fn PatBlt(dc: Hdc, x: i32, y: i32, width: i32, height: i32, operation: u32) -> i32;
    /// Copies a bitmap's pixels out as a DIB laid out as `info` asks. The
    /// bitmap must not be selected into a device context.
    fn GetDIBits(
        dc: Hdc,
        bitmap: *mut c_void,
        first: u32,
        lines: u32,
        bits: *mut c_void,
        info: *mut BitmapInfo,
        usage: u32,
    ) -> i32;
}

/// The pixels `cursor` paints, one bit per cell of a [`CANVAS`] square drawn
/// on black. Two cursors are the same pointer when every bit agrees, which is
/// what "the desktop draws the crosshair, not an arrow" has to mean: a handle
/// number proves nothing, because Windows hands out a fresh handle for every
/// cursor it installs.
fn shape(cursor: usize) -> Option<Vec<bool>> {
    if cursor == 0 {
        return None;
    }
    let mut info = BitmapInfo {
        header: BitmapHeader {
            size: u32::try_from(std::mem::size_of::<BitmapHeader>()).ok()?,
            width: CANVAS,
            height: CANVAS,
            planes: 1,
            bit_count: 32,
            ..BitmapHeader::default()
        },
        ..BitmapInfo::default()
    };
    let mut pixels = vec![0u32; usize::try_from(CANVAS * CANVAS).ok()?];
    // SAFETY: every handle created here is released before returning, and the
    // pixel buffer is a live local of exactly the size asked for.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return None;
        }
        let canvas = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, CANVAS, CANVAS);
        if canvas.is_null() || bitmap.is_null() {
            if !bitmap.is_null() {
                DeleteObject(bitmap);
            }
            if !canvas.is_null() {
                DeleteDC(canvas);
            }
            ReleaseDC(std::ptr::null_mut(), screen);
            return None;
        }
        let previous = SelectObject(canvas, bitmap);
        PatBlt(canvas, 0, 0, CANVAS, CANVAS, BLACKNESS);
        let drawn = DrawIconEx(
            canvas,
            MARGIN,
            MARGIN,
            cursor as *mut c_void,
            BODY,
            BODY,
            0,
            std::ptr::null_mut(),
            DI_NORMAL,
        ) != 0;
        // The bitmap leaves the device context before its pixels are read.
        SelectObject(canvas, previous);
        let read = drawn
            && GetDIBits(
                canvas,
                bitmap,
                0,
                u32::try_from(CANVAS).unwrap_or(0),
                pixels.as_mut_ptr().cast(),
                &raw mut info,
                DIB_RGB_COLORS,
            ) != 0;
        DeleteObject(bitmap);
        DeleteDC(canvas);
        ReleaseDC(std::ptr::null_mut(), screen);
        read.then(|| pixels.iter().map(|pixel| *pixel != 0).collect())
    }
}

/// The shape of one of the stock pointers, loaded the way the desktop loads
/// it for a window that shows the ordinary pointer.
fn stock_shape(id: usize) -> Option<Vec<bool>> {
    // SAFETY: a shared stock cursor resource; nothing is freed.
    let loaded = unsafe { LoadCursorW(std::ptr::null_mut(), id as *const u16) };
    shape(loaded as usize)
}

/// Every visible top-level window `thread` created, as handle, class name and
/// measured frame, minus the handles `ignore` names (the test's own targets).
/// The thread is the one that runs the session, so windows other tests own
/// from their own threads cannot wander into the answer, and the frame is
/// measured during the walk because the session's windows are gone by the time
/// a caller gets the answer back.
fn own_windows(ignore: &[usize], thread: u32) -> Vec<(usize, String, Option<Rect>)> {
    /// The windows found so far, and the thread to keep.
    type Walk = (Vec<(usize, String, Option<Rect>)>, u32);
    /// The `EnumWindows` callback; `data` is the caller's [`Walk`].
    unsafe extern "system" fn visit(window: Hwnd, data: isize) -> i32 {
        // SAFETY: `data` is the `Walk` lent for this walk and the callback
        // runs on this thread while it is alive.
        let (found, wanted) = unsafe { &mut *(data as *mut Walk) };
        let mut process = 0u32;
        // SAFETY: writes only the local `process`; the window is inspected.
        let creator = unsafe { GetWindowThreadProcessId(window, &raw mut process) };
        // SAFETY: a visibility query about the window asked about.
        if creator != *wanted
            || process != std::process::id()
            || unsafe { IsWindowVisible(window) } == 0
        {
            return 1;
        }
        let mut name = [0u16; 128];
        // SAFETY: the buffer is a live local of the size passed.
        let len = unsafe { GetClassNameW(window, name.as_mut_ptr(), 128) };
        let len = usize::try_from(len.max(0)).unwrap_or(0).min(name.len());
        let mut rect = Rect::default();
        // SAFETY: the handle is the window being visited and is inspected
        // only; `rect` is a live local.
        let measured = (unsafe { GetWindowRect(window, &raw mut rect) } != 0).then_some(rect);
        found.push((
            window as usize,
            String::from_utf16_lossy(&name[..len]),
            measured,
        ));
        1
    }
    let mut walk: Walk = (Vec::new(), thread);
    // SAFETY: the callback only touches the `walk` lent for this call.
    unsafe { EnumWindows(visit, (&raw mut walk) as isize) };
    walk.0
        .into_iter()
        .filter(|(handle, _, _)| !ignore.contains(handle))
        .collect()
}

/// The right click a pick cancels on, at `point` — the press and its release.
///
/// The release matters even though the pick is over by then: a button left
/// held sends the next window's input back to the window that received the
/// press, and that window is gone.
fn right_click_at(point: Point) -> bool {
    send(MOUSEEVENTF_RIGHTDOWN, point) & send(MOUSEEVENTF_RIGHTUP, point)
}

/// How long a pointer change may take to reach the screen.
///
/// The window under the pointer answers `WM_SETCURSOR` on the thread that owns
/// it, and only once that message is dispatched does the desktop draw the new
/// shape; an injected move reaches that point a moment after it was queued, and
/// on a busy desktop a moment is more than a single pump slice.
const CURSOR_SETTLE: Duration = Duration::from_millis(800);

/// Pump this thread's queue until the desktop draws `expected`, and answer
/// whatever is on screen when [`CURSOR_SETTLE`] runs out — a wrong shape is
/// reported as the shape it is, without a fixed sleep guessing at the delay.
fn wait_for_cursor(expected: &[bool]) -> Option<Vec<bool>> {
    let deadline = Instant::now() + CURSOR_SETTLE;
    loop {
        let drawn = drawn_cursor();
        if drawn.as_deref() == Some(expected) || Instant::now() >= deadline {
            return drawn;
        }
        let _ = pump_until(Duration::from_millis(20), || false);
    }
}

/// The pointer the desktop is drawing, as pixels; `None` when none is up.
///
/// This is the shape a person actually sees, not one this process installed:
/// it is the cursor of the window under the pointer, which is the whole point
/// of the test below.
fn drawn_cursor() -> Option<Vec<bool>> {
    let mut info = CursorInfo {
        size: u32::try_from(std::mem::size_of::<CursorInfo>()).ok()?,
        ..CursorInfo::default()
    };
    // SAFETY: `info` is a live local that carries its own size.
    let read = unsafe { GetCursorInfo(&raw mut info) } != 0;
    (read && info.flags & CURSOR_SHOWING != 0)
        .then(|| shape(info.cursor as usize))
        .flatten()
}

/// The pick's crosshair is the pointer over *every* window, not only over the
/// ones that never ask for a pointer of their own.
///
/// The shape on screen is chosen by the window under the pointer, so a pick
/// that swapped the system's ordinary pointer lost to a text field's I-beam.
/// The pick now covers the desktop with a window of its own, and that window's
/// class cursor is the crosshair: one pointer, moved by the window manager, on
/// any surface — and nothing left installed afterwards.
#[test]
fn a_pick_shows_the_crosshair_over_every_window() {
    let _desktop = DESKTOP.lock().unwrap_or_else(PoisonError::into_inner);
    let _pointer = PointerGuard::take();
    let (Some(arrow), Some(crosshair), Some(beam)) = (
        stock_shape(ARROW),
        stock_shape(CROSSHAIR),
        stock_shape(IDC_IBEAM),
    ) else {
        eprintln!("skipping: this desktop cannot render a cursor");
        return;
    };
    assert_ne!(
        arrow, crosshair,
        "the ordinary pointer and the crosshair are told apart"
    );
    assert_ne!(
        beam, crosshair,
        "a text field's pointer is told apart from the crosshair"
    );
    let Some(target) = Target::show("screencapture.test.crosshair", 120, 120, 460, 340) else {
        return;
    };
    let Some(field) =
        Target::with_cursor("screencapture.test.field", IDC_IBEAM, 700, 200, 380, 260)
    else {
        return;
    };
    let over_target = target.center();
    let over_field = field.center();
    if !quiet(&[(over_target, target.handle), (over_field, field.handle)]) {
        eprintln!("skipping: another window covers the test windows");
        return;
    }
    // Park on the plain window first, where the desktop's own pointer is the
    // arrow, so the samples below start from a shape that is not the pick's.
    move_to(over_target);
    // The plain window, the text field that wants its own pointer, and back:
    // the crosshair is the one drawn at all three, so the sample does not
    // depend on one spot and the pointer is asked to travel.
    let spots = [over_target, over_field, over_target];
    let samples = std::sync::Arc::new(Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&samples);
    let wanted = crosshair.clone();
    let injector = std::thread::spawn(move || {
        sleep(SETTLE);
        for spot in spots {
            move_to(spot);
            sink.lock().expect("clean samples").push((
                spot,
                wait_for_cursor(&wanted),
                cursor_at(spot),
            ));
        }
        right_click_at(over_target);
    });
    let answer = pick_within(INJECTED_IDLE).expect("the picker holds the pointer");
    let _ = injector.join();
    let drawn = samples.lock().expect("clean samples").clone();
    if !INJECTED.load(Ordering::SeqCst) {
        eprintln!("skipping: this desktop does not take synthetic input");
        return;
    }
    assert_eq!(answer, None, "the right click cancels the pick");
    assert_eq!(drawn.len(), spots.len(), "every position was sampled");
    for (spot, sampled, reached) in &drawn {
        assert!(*reached, "the pointer reached ({}, {})", spot.x, spot.y);
        assert!(
            sampled.as_deref() == Some(crosshair.as_slice()),
            "the pointer the desktop draws at ({}, {}) is the crosshair, with no second pointer beside it",
            spot.x,
            spot.y
        );
    }
    // Nothing of the pick is left installed: over the plain window the desktop
    // draws its arrow again, and over the text field the field's own I-beam.
    let back = wait_for_cursor(&arrow);
    assert!(
        back.as_deref() == Some(arrow.as_slice()),
        "the window's own pointer is back when the session ends"
    );
    move_to(over_field);
    let after = wait_for_cursor(&beam);
    assert!(
        after.as_deref() == Some(beam.as_slice()),
        "a text field's pointer comes back when the session ends"
    );
}

/// The other half of the same regression: the frozen crosshair was a window
/// of ours, placed from hook callbacks, so it only moved when one ran. A pick
/// may paint the selection outline and hold the pointer with one fullscreen
/// window at a fixed place — nothing of ours may follow the pointer, because
/// anything that has to be moved for every mouse event is a crosshair that
/// can fall behind one, and anything small that trails the pointer is a second
/// pointer however it is named.
#[test]
fn a_pick_paints_only_the_selection_outline() {
    let _desktop = DESKTOP.lock().unwrap_or_else(PoisonError::into_inner);
    let _pointer = PointerGuard::take();
    let Some(target) = Target::show("screencapture.test.outline", 200, 160, 520, 380) else {
        return;
    };
    let from = Point {
        x: target.rect.left + 60,
        y: target.rect.top + 60,
    };
    let to = target.center();
    if !quiet(&[(from, target.handle), (to, target.handle)]) {
        eprintln!("skipping: another window covers the test target");
        return;
    }
    let ignore = [target.handle];
    // SAFETY: parameterless question about this thread.
    let session = unsafe { GetCurrentThreadId() };
    assert!(
        own_windows(&ignore, session).is_empty(),
        "the session thread starts with nothing but its target"
    );
    let walks = std::sync::Arc::new(Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&walks);
    let injector = std::thread::spawn(move || {
        sleep(SETTLE);
        for spot in [from, to] {
            move_to(spot);
            sleep(Duration::from_millis(150));
            sink.lock()
                .expect("clean walks")
                .push(own_windows(&ignore, session));
        }
        right_click_at(to);
    });
    let answer = pick_within(INJECTED_IDLE).expect("the picker holds the pointer");
    let _ = injector.join();
    let walked = walks.lock().expect("clean walks").clone();
    assert_eq!(answer, None, "the right click cancels the pick");
    assert_eq!(walked.len(), 2, "both positions were sampled");
    let (left, top, width, height) = virtual_desktop();
    // The outline is the pick's own announcement of what a click would take,
    // so it follows the pointer; the pointer surface is the one window that
    // must not.
    for (index, windows) in walked.iter().enumerate() {
        let painted: Vec<&str> = windows.iter().map(|(_, name, _)| name.as_str()).collect();
        assert_eq!(
            painted.len(),
            2,
            "sample {index} painted {painted:?}, which is not one outline beside the pointer surface"
        );
        assert!(
            painted.contains(&OUTLINE_CLASS) && painted.contains(&GRABBER_CLASS),
            "sample {index} painted {painted:?}, which is not one outline beside the pointer surface"
        );
        let rect = windows
            .iter()
            .find(|(_, name, _)| name == GRABBER_CLASS)
            .and_then(|(_, _, rect)| *rect)
            .expect("sample {index} cannot measure the pointer surface");
        assert_eq!(
            (rect.left, rect.top, rect.right, rect.bottom),
            (left, top, left + width, top + height),
            "sample {index} found the pointer surface somewhere other than the desktop: a window of ours is following the pointer"
        );
    }
}

/// The whole desktop, as the pick's own window covers it.
fn virtual_desktop() -> (i32, i32, i32, i32) {
    // SAFETY: parameterless metric queries.
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// A window some of which another window covers is still the window a pick
/// answers, over the part of it a person can see.
///
/// The overlay must not flatten the stack: the pick resolves the window at the
/// pointer, at whichever depth it sits, and the frame it answers is the whole
/// window — the part behind the cover included — so a capture of it is the
/// window's own content (see the covered-window capture tests) rather than the
/// strip left visible.
#[test]
fn a_pick_answers_a_window_another_one_covers() {
    let _desktop = DESKTOP.lock().unwrap_or_else(PoisonError::into_inner);
    let _pointer = PointerGuard::take();
    let Some(target) = Target::show("screencapture.test.under", 260, 200, 560, 400) else {
        return;
    };
    let Some(cover) = Target::show("screencapture.test.cover", 600, 400, 400, 320) else {
        return;
    };
    let exposed = Point {
        x: target.rect.left + 80,
        y: target.rect.top + 80,
    };
    let hidden = Point {
        x: target.rect.right - 60,
        y: target.rect.bottom - 60,
    };
    if !quiet(&[(exposed, target.handle), (hidden, cover.handle)]) {
        eprintln!("skipping: the test windows are not stacked as needed");
        return;
    }
    INJECTED.store(false, Ordering::SeqCst);
    let mut answer = None;
    for _ in 0..ATTEMPTS {
        answer = pick_with(move || {
            move_to(exposed);
            sleep(Duration::from_millis(80));
            press_at(exposed);
            sleep(Duration::from_millis(60));
            release_at(exposed);
        });
        if matches!(answer, Some(Pick::Window(handle)) if handle == target.handle) {
            break;
        }
    }
    if answer.is_none() && !INJECTED.load(Ordering::SeqCst) {
        eprintln!("skipping: this desktop does not take synthetic input");
        return;
    }
    assert_eq!(
        answer,
        Some(Pick::Window(target.handle)),
        "the click answers the covered window, not the one covering it"
    );
    assert_eq!(
        target.recorded(),
        target.frame(),
        "the frame a capture saves is the whole window, the covered part included"
    );
    assert_ne!(
        target.recorded(),
        cover.frame(),
        "and it is not the covering window's frame"
    );
    assert_eq!(
        root_at(exposed),
        target.handle,
        "the pick's overlay is gone, so what is on screen there is the window it answered"
    );
}

/// Whether a `WM_QUIT` is pending on this thread: a pick that posts one and
/// returns leaves the loop behind it — the tray's — one message from
/// exiting, taking the icon and a recording the pick just started with it.
fn quit_pending() -> bool {
    /// `PM_NOREMOVE` (winuser.h): look at the message, leave it queued.
    const PM_NOREMOVE: u32 = 0;
    const WM_QUIT: u32 = 0x0012;
    let mut message = Msg {
        hwnd: std::ptr::null_mut(),
        message: 0,
        w_param: 0,
        l_param: 0,
        time: 0,
        point: Point::default(),
    };
    // SAFETY: writes only the local message; the queue is only peeked.
    unsafe {
        PeekMessageW(&raw mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) != 0
            && message.message == WM_QUIT
    }
}

/// A pick that resolves or cancels must leave no `WM_QUIT` behind on this
/// thread: the tray's own loop runs behind the pick's pump, and a stale
/// quit ends it one message after the pick returns.
#[test]
fn a_resolved_pick_leaves_no_quit_behind() {
    let _desktop = DESKTOP.lock().unwrap_or_else(PoisonError::into_inner);
    let _pointer = PointerGuard::take();
    let Some(target) = Target::show("screencapture.test.quit", 300, 200, 520, 380) else {
        return;
    };
    let at = target.center();
    if !quiet(&[(at, target.handle)]) {
        eprintln!("skipping: another window covers the test target");
        return;
    }
    INJECTED.store(false, Ordering::SeqCst);
    let mut answer = None;
    for _ in 0..ATTEMPTS {
        answer = pick_with(move || {
            move_to(at);
            sleep(Duration::from_millis(80));
            press_at(at);
            sleep(Duration::from_millis(60));
            release_at(at);
        });
        if matches!(answer, Some(Pick::Window(handle)) if handle == target.handle) {
            break;
        }
    }
    if answer.is_none() && !INJECTED.load(Ordering::SeqCst) {
        eprintln!("skipping: this desktop does not take synthetic input");
        return;
    }
    assert_eq!(
        answer,
        Some(Pick::Window(target.handle)),
        "the click resolves the pick"
    );
    assert!(!quit_pending(), "a resolved pick leaves no stale quit");
    // A cancelled pick answers the same rule.
    let cancelled = pick_with(move || {
        right_click_at(at);
    });
    assert_eq!(cancelled, None, "the right click cancels the pick");
    assert!(!quit_pending(), "a cancelled pick leaves no stale quit");
}
