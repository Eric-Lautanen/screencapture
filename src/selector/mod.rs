//! Drawn-rectangle capture selector: a full-virtual-screen topmost popup
//! on which the user drags a rubber-band rectangle; Escape or a barely
//! moved click cancels. Runs on the tray's message thread, replacing the
//! pump until the drag completes, so registered hotkeys queue for later.

use std::cell::RefCell;
use std::io;
use std::thread_local;

use crate::capture::Region;
use crate::guard::{EscapeWatch, MouseCapture, ScreenDc};
use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DrawFocusRect, GetSystemMetrics, Hwnd, Lparam,
    Lresult, Point, PostQuitMessage, PumpEnd, Rect, RegisterClassExW, SELECTION_IDLE_LIMIT,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW,
    SetForegroundWindow, ShowWindow, UnregisterClassW, VK_ESCAPE, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WndClassExW, Wparam,
    kept_wide, module_handle, pump_until, wide,
};

mod ffi;
use ffi::cross_cursor;

/// Window title for the overlay surface.
const WINDOW_NAME: &str = "Screencapture region select";

/// Class name for the overlay window (kept for the OS).
const CLASS_NAME: &str = "ScreencaptureSelect";

/// Drags narrower than this on either axis count as accidental clicks.
const MIN_REGION_PIXELS: u32 = 8;

/// What a completed drag handed back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// A usable rectangle in screen coordinates.
    Region(Region),
    /// Escape, or a click that never became a drag.
    Cancelled,
}

/// In-memory drag state on the pump thread.
struct Session {
    /// Drag origin in client coordinates (set on button-down).
    from: Point,
    /// Drag head in client coordinates; `None` until a drag begins.
    head: Option<Point>,
    /// The rectangle currently drawn on screen (client space).
    drawn: Rect,
    /// Set when the user aborts with Escape.
    cancelled: bool,
    /// Live mouse capture for the drag; dropped with the session.
    capture: Option<MouseCapture>,
}

impl Session {
    fn fresh() -> Self {
        Self {
            from: Point { x: 0, y: 0 },
            head: None,
            drawn: Rect::default(),
            cancelled: false,
            capture: None,
        }
    }

    /// Client-space marquee; a zero rect when no drag is under way.
    fn marquee(&self) -> Rect {
        match self.head {
            None => Rect::default(),
            Some(head) => Rect {
                left: self.from.x.min(head.x),
                top: self.from.y.min(head.y),
                right: self.from.x.max(head.x),
                bottom: self.from.y.max(head.y),
            },
        }
    }

    /// Translate the client-space marquee into a screen `Region`.
    fn region(&self) -> Region {
        let mark = self.marquee();
        let origin = virtual_screen_origin();
        Region {
            x: origin.x + mark.left,
            y: origin.y + mark.top,
            width: u32::try_from(mark.right - mark.left).unwrap_or(0),
            height: u32::try_from(mark.bottom - mark.top).unwrap_or(0),
        }
    }

    /// A usable rectangle, or a cancel for tiny/aborted selections.
    fn outcome(&self) -> Outcome {
        let region = self.region();
        if self.cancelled || region.width < MIN_REGION_PIXELS || region.height < MIN_REGION_PIXELS {
            return Outcome::Cancelled;
        }
        Outcome::Region(region)
    }
}

thread_local! {
    static SESSION: RefCell<Session> = RefCell::new(Session::fresh());
}

/// The virtual desktop origin (covers monitors left of and above primary).
fn virtual_screen_origin() -> Point {
    // SAFETY: read-only system metrics with no side effects.
    unsafe {
        Point {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
        }
    }
}

/// Show the overlay and await one drag; blocks the caller (the tray pump)
/// until the drag completes.
///
/// # Errors
/// Fails when Windows refuses the window class or the window itself.
pub fn draw() -> io::Result<Outcome> {
    SESSION.with_borrow_mut(|state| *state = Session::fresh());

    let class_name = kept_wide(CLASS_NAME);
    let instance = module_handle();
    let class = WndClassExW {
        size: u32::try_from(std::mem::size_of::<WndClassExW>()).expect("class struct fits u32"),
        style: 0,
        wnd_proc: on_message,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: std::ptr::null_mut(),
        cursor: load_cursor(),
        background: std::ptr::null_mut(),
        menu_name: std::ptr::null(),
        class_name,
        icon_small: std::ptr::null_mut(),
    };
    let atom = {
        // SAFETY: `class` points to a fully initialized local struct.
        unsafe { RegisterClassExW(&raw const class) }
    };
    if atom == 0 {
        return Err(io::Error::other("RegisterClassExW failed"));
    }

    let window_name = wide(WINDOW_NAME);
    let origin = virtual_screen_origin();
    let window = {
        // SAFETY: `class_name` is the leaked static buffer registered
        // above; `window_name` outlives the call; the window covers the
        // entire virtual desktop; parent, menu, and param are unused.
        unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class_name,
                window_name.as_ptr(),
                WS_POPUP,
                origin.x,
                origin.y,
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null_mut(),
            )
        }
    };
    if window.is_null() {
        // SAFETY: `class_name` is the leaked static name registered above.
        unsafe { UnregisterClassW(class_name, instance) };
        return Err(io::Error::other("CreateWindowExW failed"));
    }
    // SAFETY: showing and foregrounding our own window.
    unsafe {
        ShowWindow(window, SW_SHOW);
        // Taking the foreground is the polite case, not the guaranteed
        // one: Windows refuses it for a process that is not the current
        // foreground process, and the window then never sees WM_KEYDOWN
        // (see `EscapeWatch`). The attempt stays because a focused
        // overlay also swallows stray keys from the app below.
        SetForegroundWindow(window);
    }

    // Watch Escape through a low-level hook for the whole drag, so the
    // cancel works whether or not the foreground call above succeeded.
    // SAFETY: the pump below runs on this thread with our own instance.
    let _escape = unsafe { EscapeWatch::install(escape, instance) };

    pump_messages();

    let outcome = SESSION.with_borrow(Session::outcome);
    // SAFETY: the window belongs to this thread and the pump above ended.
    unsafe { DestroyWindow(window) };
    // SAFETY: `class_name` is the leaked static name registered above.
    unsafe { UnregisterClassW(class_name, instance) };
    Ok(outcome)
}

/// The Escape watch's answer: remember the cancel and end the pump.
fn escape() {
    SESSION.with_borrow_mut(|state| {
        state.capture = None;
        state.cancelled = true;
    });
    // SAFETY: the pump this watch was installed for runs on this thread.
    unsafe { PostQuitMessage(0) }
}

/// Crosshair cursor or, if that resource is refused, the default arrow.
fn load_cursor() -> *mut std::ffi::c_void {
    // SAFETY: null instance resolves a shared stock cursor resource.
    unsafe { cross_cursor() }
}

/// Local message loop until the drag's final mouse-up or Escape quits.
/// A session that sees no input at all cancels at the idle limit, so an
/// overlay that never wins the foreground cannot hang the tray.
fn pump_messages() {
    let end = pump_until(SELECTION_IDLE_LIMIT, || {
        SESSION.with_borrow(|state| state.cancelled)
    });
    if end == PumpEnd::Idle {
        SESSION.with_borrow_mut(|state| state.cancelled = true);
        crate::log::note("warn", "selection cancelled after sitting idle");
    }
}

/// Convert a packed `l_param` mouse position into client coordinates.
fn client_point(l_param: Lparam) -> Point {
    // Packed halves are each valid `i16` bit patterns; decode via bytes to
    // avoid sign-losing casts (multi-monitor negatives stay intact).
    let bytes = l_param.to_ne_bytes();
    let x = i32::from(i16::from_ne_bytes([bytes[0], bytes[1]]));
    let y = i32::from(i16::from_ne_bytes([bytes[2], bytes[3]]));
    Point { x, y }
}

/// Erase the previous rectangle and draw the new one; every `DrawFocusRect`
/// call toggles its rectangle, so exactly two calls make a clean repaint.
fn repaint(window: Hwnd, state: &mut Session) {
    let next = state.marquee();
    let Some(dc) = ScreenDc::borrow(window) else {
        return; // best-effort: the marquee repaint simply skips a turn
    };
    if !state.drawn.is_empty() {
        // SAFETY: the guard releases the borrowed DC on drop.
        unsafe { DrawFocusRect(dc.dc(), &raw const state.drawn) };
    }
    if !next.is_empty() {
        // SAFETY: the guard releases the borrowed DC on drop.
        unsafe { DrawFocusRect(dc.dc(), &raw const next) };
    }
    state.drawn = next;
}

/// Window procedure for the overlay; all state lives in `SESSION`.
unsafe extern "system" fn on_message(
    window: Hwnd,
    message: u32,
    w_param: Wparam,
    l_param: Lparam,
) -> Lresult {
    match message {
        WM_LBUTTONDOWN => {
            let point = client_point(l_param);
            SESSION.with_borrow_mut(|state| {
                state.from = point;
                state.head = Some(point);
                state.drawn = Rect::default();
                state.cancelled = false;
            });
            // SAFETY: capturing motion for our own popup window; the
            // guard releases on drop when the drag ends un-captured.
            let capture = unsafe { MouseCapture::capture(window) };
            SESSION.with_borrow_mut(|state| state.capture = capture);
            0
        }
        WM_MOUSEMOVE => {
            SESSION.with_borrow_mut(|state| {
                if state.head.is_some() {
                    state.head = Some(client_point(l_param));
                    repaint(window, state);
                }
            });
            0
        }
        WM_LBUTTONUP => {
            SESSION.with_borrow_mut(|state| state.capture = None);
            unsafe { PostQuitMessage(0) };
            0
        }
        // The fallback for a session where Windows refused the watch
        // above: a focused overlay still cancels on its own key.
        WM_KEYDOWN if w_param == VK_ESCAPE => {
            escape();
            0
        }
        // SAFETY: pass-through to the default window procedure.
        _ => unsafe { DefWindowProcW(window, message, w_param, l_param) },
    }
}

#[cfg(test)]
mod tests {
    use super::{Outcome, SESSION, Session, escape};
    use crate::winapi::Point;

    /// Start a fresh session mid-drag and answer its outcome.
    fn dragging() -> Outcome {
        SESSION.with_borrow_mut(|state| {
            *state = Session::fresh();
            state.from = Point { x: 0, y: 0 };
            state.head = Some(Point { x: 320, y: 200 });
        });
        SESSION.with_borrow(Session::outcome)
    }

    #[test]
    fn escape_turns_a_live_drag_into_a_cancel() {
        // The marquee alone is a usable rectangle; the keypress, not the
        // geometry, decides. Losing that made Escape look ignored on a
        // drag the user had already started.
        assert!(matches!(dragging(), Outcome::Region(_)));
        escape();
        SESSION.with_borrow(|state| {
            assert!(state.cancelled);
            assert_eq!(state.outcome(), Outcome::Cancelled);
        });
    }
}
