use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use super::{
    Action, DRAG_THRESHOLD_PX, Hover, Input, Pick, Rect, Region, TRACK, Track, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, abandon, beyond_threshold, confirm_click,
    confirm_drag, consumed, drag_geometry, pick_within, region_from_rect, step,
};
use crate::overlay::{WS_EX_NOACTIVATE, WS_VISIBLE};
use crate::screen::win::window_bounds;
use crate::screen::window_region;
use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GA_ROOT, GetAncestor, GetCursorPos,
    GetSystemMetrics, GetWindowRect, Hwnd, Point, RegisterClassExW, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP, WindowFromPoint, WndClassExW, kept_wide, module_handle, wide,
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
}

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
/// Time the hooks have to install before the pointer starts moving.
const HOOK_SETTLE: Duration = Duration::from_millis(250);
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

fn press_here() -> bool {
    send(MOUSEEVENTF_LEFTDOWN, Point::default())
}

fn release_here() -> bool {
    send(MOUSEEVENTF_LEFTUP, Point::default())
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
            cursor: std::ptr::null_mut(),
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
        // The hooks install before the pump starts; let that happen
        // instead of racing `SetWindowsHookEx`.
        sleep(HOOK_SETTLE);
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
fn cursor_at(point: Point) -> bool {
    let mut at = Point::default();
    // SAFETY: writes only the local point.
    let found = unsafe { GetCursorPos(&raw mut at) } != 0;
    found && (at.x, at.y) == (point.x, point.y)
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
}

/// A real drag through the same hooks answers the rectangle it drew, so
/// the press, motion and release path stays wired to the state machine.
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
fn motion_passes_through_to_the_cursor() {
    // The pointer must keep moving while a pick is live; a consumed
    // move is a move the system never applies to the cursor.
    assert!(!consumed(WM_MOUSEMOVE));
    assert!(consumed(WM_LBUTTONDOWN));
    assert!(consumed(WM_LBUTTONUP));
    assert!(consumed(WM_RBUTTONDOWN));
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
