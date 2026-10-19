//! Drawn-rectangle capture selector: a full-virtual-screen topmost popup
//! on which the user drags a rubber-band rectangle; Escape or a barely
//! moved click cancels. Runs on the tray's message thread, replacing the
//! pump until the drag completes, so registered hotkeys queue for later.

use std::cell::RefCell;
use std::io;
use std::thread_local;

use crate::capture::Region;
use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, DrawFocusRect,
    GETMESSAGE_QUIT, GetMessageW, GetModuleHandleW, GetSystemMetrics, Hdc, Hwnd, Lparam, Lresult,
    Msg, Point, PostQuitMessage, Rect, RegisterClassExW, ReleaseCapture, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW, SetCapture,
    SetForegroundWindow, ShowWindow, TranslateMessage, UnregisterClassW, VK_ESCAPE, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    WndClassExW, Wparam,
};

mod ffi;
use ffi::{client_dc, cross_cursor, release_client_dc};

/// Window title for the overlay surface.
const WINDOW_NAME: &str = "Screencapture region select";

/// Class name for the overlay window (leaked to the OS).
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
}

impl Session {
    fn fresh() -> Self {
        Self {
            from: Point { x: 0, y: 0 },
            head: None,
            drawn: Rect::default(),
            cancelled: false,
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

/// Text as UTF-16 with a trailing nul.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Leak a UTF-16 buffer so the OS can retain the class name indefinitely.
fn wide_static(text: &str) -> *const u16 {
    Box::leak(wide(text).into_boxed_slice()).as_ptr()
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

    let class_name = wide_static(CLASS_NAME);
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
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
        SetForegroundWindow(window);
    }

    pump_messages();

    let outcome = SESSION.with_borrow(Session::outcome);
    // SAFETY: the window belongs to this thread and the pump above ended.
    unsafe { DestroyWindow(window) };
    // SAFETY: `class_name` is the leaked static name registered above.
    unsafe { UnregisterClassW(class_name, instance) };
    Ok(outcome)
}

/// Crosshair cursor or, if that resource is refused, the default arrow.
fn load_cursor() -> *mut std::ffi::c_void {
    // SAFETY: null instance resolves a shared stock cursor resource.
    unsafe { cross_cursor() }
}

/// Local message loop until the drag's final mouse-up or Escape quits.
fn pump_messages() {
    let mut message = Msg {
        hwnd: std::ptr::null_mut(),
        message: 0,
        w_param: 0,
        l_param: 0,
        time: 0,
        point: Point { x: 0, y: 0 },
    };
    loop {
        // SAFETY: `message` is a local struct owned by this loop.
        let received = unsafe { GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) };
        if received == GETMESSAGE_QUIT || received < 0 {
            return;
        }
        let _ = unsafe { TranslateMessage(&raw const message) };
        unsafe { DispatchMessageW(&raw const message) };
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
    // SAFETY: the DC is borrowed, used, and released on this thread.
    let dc: Hdc = unsafe { client_dc(window) };
    if !state.drawn.is_empty() {
        // SAFETY: erase pass; `dc` is live for the borrow.
        unsafe { DrawFocusRect(dc, &raw const state.drawn) };
    }
    if !next.is_empty() {
        // SAFETY: redraw pass onto the same live DC.
        unsafe { DrawFocusRect(dc, &raw const next) };
    }
    // SAFETY: pairs with `client_dc` above.
    unsafe { release_client_dc(window, dc) };
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
            // SAFETY: capturing motion for our own popup window.
            unsafe { SetCapture(window) };
            SESSION.with_borrow_mut(|state| {
                state.from = point;
                state.head = Some(point);
                state.drawn = Rect::default();
                state.cancelled = false;
            });
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
            // SAFETY: releasing a capture our window owns.
            unsafe { ReleaseCapture() };
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_KEYDOWN if w_param == VK_ESCAPE => {
            SESSION.with_borrow_mut(|state| state.cancelled = true);
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: pass-through to the default window procedure.
        _ => unsafe { DefWindowProcW(window, message, w_param, l_param) },
    }
}
