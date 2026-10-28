//! The fullscreen window a pick puts under the pointer.
//!
//! Two things a pick needs only happen while the window under the pointer is
//! ours. The first is the cursor: the shape on screen is the one chosen by the
//! window the pointer is over, so a text field's I-beam or a resize border's
//! double arrow wins over anything set system-wide. With this window on top,
//! the window under the pointer is ours, and the desktop draws the crosshair
//! its class asks for — one cursor, moved by the window manager, on every
//! surface. The second is the input itself: a low-level hook is skipped, not
//! queued, when its callback runs long, and a skipped message is a gesture the
//! session never saw. A window's messages are queued instead.
//!
//! The window covers the virtual desktop, never activates, is excluded from
//! captures, and is painted at one part in 255 — invisible to the eye, but
//! not zero, because a fully transparent layered window is click-through and
//! would hand the pointer back to the window underneath.
//!
//! The click is not forwarded into the window below: picking a window that
//! runs its own button would pick *and* press it. What the click answers is
//! the window a person aimed at — resolved by [`Grabber::beneath`] — and the
//! grabber is destroyed before the pick returns, so the capture that follows
//! photographs the desktop rather than our overlay.

use std::ffi::c_void;
use std::io;

use crate::overlay::{
    HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    WDA_EXCLUDEFROMCAPTURE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP, WS_VISIBLE,
};
use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetSystemMetrics, Hcursor, Hdc,
    Hinstance, Hwnd, IDC_ARROW, IDC_CROSS, Lparam, Lresult, Point, Rect, RegisterClassExW,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WindowFromPoint,
    WndClassExW, Wparam, kept_wide, module_handle, request_cursor, wide,
};

/// Window class for the pick's pointer surface.
pub(super) const CLASS_NAME: &str = "screencapture.selection.grabber";

/// The session's pointer messages, which this window hands to the picker.
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
/// `WM_PAINT`, `WM_ERASEBKGND`, `WM_MOUSEACTIVATE` (winuser.h).
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_MOUSEACTIVATE: u32 = 0x0021;
/// `MA_NOACTIVATE` (winuser.h): answer the click without taking activation,
/// so the window that had focus keeps it.
const MA_NOACTIVATE: Lresult = 3;
/// `GWL_EXSTYLE` and `WS_EX_TRANSPARENT` (winuser.h): the style that makes a
/// window invisible to hit testing, and the index it lives at.
const GWL_EXSTYLE: i32 = -20;
const WS_EX_TRANSPARENT: i32 = 0x0000_0020;
/// `GCLP_HCURSOR` (winuser.h): index of the class cursor.
const GCLP_HCURSOR: i32 = -12;
/// The surface's opacity: one part in 255 of the black it is filled with,
/// which is nothing the eye can find. Zero would make the window click-through
/// — and so would an unpainted surface, which stays fully transparent however
/// opaque the window's own opacity is set.
const OPACITY: u8 = 1;
/// The colour the surface is filled with: black, which at [`OPACITY`] leaves
/// the desktop underneath as good as untouched.
const SURFACE: u32 = 0x0000_0000;

#[link(name = "user32")]
unsafe extern "system" {
    fn LoadCursorW(instance: *mut c_void, cursor: *const u16) -> Hcursor;
    fn SetLayeredWindowAttributes(window: Hwnd, key: u32, alpha: u8, flags: u32) -> i32;
    fn SetWindowDisplayAffinity(window: Hwnd, affinity: u32) -> i32;
    fn GetWindowLongW(window: Hwnd, index: i32) -> i32;
    fn SetWindowLongW(window: Hwnd, index: i32, value: i32) -> i32;
    fn GetClassLongPtrW(window: Hwnd, index: i32) -> usize;
    fn SetCursor(cursor: Hcursor) -> Hcursor;
    fn BeginPaint(window: Hwnd, paint: *mut c_void) -> Hdc;
    fn EndPaint(window: Hwnd, paint: *const c_void) -> i32;
    fn CreateSolidBrush(color: u32) -> *mut c_void;
    fn DeleteObject(object: *mut c_void) -> i32;
    fn GetClientRect(window: Hwnd, rect: *mut Rect) -> i32;
    fn ClientToScreen(window: Hwnd, point: *mut Point) -> i32;
    fn FillRect(dc: Hdc, rect: *const Rect, brush: *mut c_void) -> i32;
}

/// The pointer surface of one pick session; destroying it returns the desktop
/// to the window beneath.
pub(super) struct Grabber {
    window: Hwnd,
}

impl Grabber {
    /// Put a crosshair-bearing window over the whole virtual desktop.
    ///
    /// # Errors
    /// Windows refused the window; a pick without one would hear nothing, so
    /// the session fails instead of sitting idle.
    pub(super) fn show() -> io::Result<Self> {
        let instance = module_handle();
        register(instance);
        let class = wide(CLASS_NAME);
        // SAFETY: metrics are plain queries, and the strings and handles
        // outlive the call; the window is owned by this thread.
        let (x, y, width, height) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        if width <= 0 || height <= 0 {
            return Err(io::Error::other("the desktop has no extent"));
        }
        // SAFETY: as above; the returned window is destroyed in `Drop`.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
                class.as_ptr(),
                std::ptr::null(),
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
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the window this function created.
        unsafe {
            SetLayeredWindowAttributes(window, 0, OPACITY, LWA_ALPHA);
            let _ = SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE);
        }
        Ok(Self { window })
    }

    /// The live window, for taking capture on a press.
    pub(super) fn window(&self) -> Hwnd {
        self.window
    }

    /// Claim the top of the always-on-top band again when something has landed
    /// over the surface at the pointer.
    ///
    /// The window under the pointer is the one the session hears from, and a
    /// window created or raised after this one takes that place: an
    /// always-on-top application — Task Manager keeps itself there — that
    /// raises itself mid-session leaves the pick with no move, no click and no
    /// shape of its own until the idle limit ends it, which reads as a hang.
    /// Asking the desktop who is at the pointer, and raising only when it is
    /// not this window, keeps the ordinary case free of z-order churn.
    pub(super) fn raise_if_covered(&self) {
        let mut point = Point::default();
        // SAFETY: writes only the local point.
        if unsafe { GetCursorPos(&raw mut point) } == 0 {
            return;
        }
        // SAFETY: the lookup only reads the desktop's window stack, and the
        // style change is this process's own window state.
        unsafe {
            if WindowFromPoint(point) != self.window {
                SetWindowPos(
                    self.window,
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

    /// The window at `point`, with this overlay taken out of the hit test.
    ///
    /// `WindowFromPoint` answers the topmost window at the point, and while a
    /// pick is live that is this overlay — it has to be, or the pointer would
    /// not be ours to shape. Asking through the click-through style answers
    /// what the pick is actually aimed at: the window a person can see, at
    /// whichever depth of the stack it happens to sit.
    pub(super) fn beneath(&self, point: Point) -> Hwnd {
        // SAFETY: the style is this process's own window state, and the
        // lookup only reads the desktop; the style is put back before
        // returning either way.
        unsafe {
            let style = GetWindowLongW(self.window, GWL_EXSTYLE);
            SetWindowLongW(self.window, GWL_EXSTYLE, style | WS_EX_TRANSPARENT);
            let found = WindowFromPoint(point);
            SetWindowLongW(self.window, GWL_EXSTYLE, style);
            found
        }
    }
}

impl Drop for Grabber {
    fn drop(&mut self) {
        // SAFETY: the window this guard created on this thread.
        unsafe { DestroyWindow(self.window) };
        hand_back_cursor();
    }
}

/// Give the pointer back to the window under it.
///
/// Destroying this window does not undo the crosshair it had the desktop
/// draw: the shape on screen is the last `WM_SETCURSOR` answer, and it stays
/// put until the pointer moves onto a window Windows asks again. Asking the
/// window underneath once, here, makes the pointer the desktop's own the
/// moment the session ends rather than on the user's next twitch.
fn hand_back_cursor() {
    let mut point = Point::default();
    // SAFETY: writes only the local point.
    if unsafe { GetCursorPos(&raw mut point) } == 0 {
        return;
    }
    // SAFETY: a plain desktop query; the answer is only asked for its cursor.
    let under = unsafe { WindowFromPoint(point) };
    if under.is_null() {
        return;
    }
    restore_cursor(under);
}

/// Put the shape `under` would draw back on screen.
///
/// The window answers when it can: a `WM_SETCURSOR` request makes it install
/// its own cursor, which is the I-beam of a text field as much as the arrow of
/// a plain window. Two cases have no answer to wait for, and the shape goes on
/// from here: a window with no cursor of its own is drawn with the system
/// arrow by the window manager, which a request cannot produce (that answers
/// only for the window's own cursor), and a window of a higher integrity —
/// Task Manager is the one people pick — is never sent the request at all,
/// since Windows drops messages aimed at it from this process. Either way the
/// crosshair the session was drawn with would stay on screen until the pointer
/// happened to reach a window that asks again.
pub(super) fn restore_cursor(under: Hwnd) {
    // SAFETY: the class cursor is read-only state of a live window, and the
    // shapes installed here are shared resources that `SetCursor` does not
    // take over.
    unsafe {
        let own = GetClassLongPtrW(under, GCLP_HCURSOR) as Hcursor;
        let shape = if own.is_null() {
            LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16)
        } else if request_cursor(under) {
            return;
        } else {
            own
        };
        if !shape.is_null() {
            SetCursor(shape);
        }
    }
}

/// Fill the window's surface so that it has one.
///
/// A layered window with nothing painted is fully transparent to the mouse as
/// well as to the eye: the desktop under it keeps taking the clicks, and a pick
/// that never hears a press is a pick that cannot answer. Painting the client
/// at the window's own one-part-in-255 opacity gives the window a surface to be
/// hit-tested against while staying invisible.
unsafe fn paint_surface(dc: Hdc, window: Hwnd) {
    let mut client = Rect::default();
    // SAFETY: the DC is live for this paint, and the rect and brush are locals
    // released before returning.
    unsafe {
        if GetClientRect(window, &raw mut client) == 0 {
            return;
        }
        let brush = CreateSolidBrush(SURFACE);
        if !brush.is_null() {
            FillRect(dc, &raw const client, brush);
            DeleteObject(brush);
        }
    }
}

/// The client point packed into a pointer message's `lParam`.
///
/// The halves are sign-extended from 16 bits, so a point on a monitor left of
/// or above the primary one keeps its sign. The message carries the point the
/// event happened at, where the cursor may already have moved on: a queued
/// release is answered where it was released, not where the pointer is by the
/// time the session gets to it.
fn client_point(lparam: Lparam) -> Point {
    let packed = u32::try_from(lparam).unwrap_or(0);
    let low = u16::try_from(packed & 0xFFFF).unwrap_or(0).cast_signed();
    let high = u16::try_from(packed >> 16).unwrap_or(0).cast_signed();
    Point {
        x: i32::from(low),
        y: i32::from(high),
    }
}

/// Whether `message` is the pointer's: the ones the picker answers, and the
/// only ones that are not left to the default handler.
pub(super) fn handles(message: u32) -> bool {
    matches!(
        message,
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN
    )
}

/// Register the grabber's class; repeats answer "already registered".
///
/// The class cursor is what the desktop draws while the pointer is over the
/// window, which is how the pick gets one crosshair instead of the arrow and
/// something drawn beside it. The name buffer is kept for the process
/// lifetime: `RegisterClassExW` stores the pointer rather than a copy.
fn register(instance: Hinstance) {
    let name = kept_wide(CLASS_NAME);
    let class = WndClassExW {
        size: u32::try_from(std::mem::size_of::<WndClassExW>()).expect("class size fits u32"),
        style: 0,
        wnd_proc: procedure,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: std::ptr::null_mut(),
        // SAFETY: a shared stock cursor resource; nothing is freed here and
        // the handle outlives the class.
        cursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_CROSS as *const u16) },
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

/// Hand the pointer's messages to the picker, keep the crosshair, and paint
/// nothing.
unsafe extern "system" fn procedure(
    window: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    match message {
        message if handles(message) => {
            let mut point = client_point(lparam);
            // SAFETY: the point is mapped for this window, which is live and
            // owned by this thread.
            unsafe { ClientToScreen(window, &raw mut point) };
            super::pointer(message, point);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            // SAFETY: the paint state buffer is large enough for the
            // platform `PAINTSTRUCT` (72 bytes on 64-bit, 64 on 32-bit).
            let mut state = [0u8; 128];
            unsafe {
                let dc = BeginPaint(window, state.as_mut_ptr().cast());
                if !dc.is_null() {
                    paint_surface(dc, window);
                    EndPaint(window, state.as_ptr().cast());
                }
            }
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE,
        _ => {
            // SAFETY: the default handler for a window of this class; its
            // class cursor is what paints the crosshair.
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
    }
}
