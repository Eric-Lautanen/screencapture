//! Selector-only Win32: cursor loading plus screen-dc borrowing. The rest
//! of the overlay window plumbing is declared once in `crate::winapi`.

use crate::winapi::{Bool, Hcursor, Hdc, Hwnd};

#[link(name = "user32")]
unsafe extern "system" {
    fn LoadCursorW(instance: *mut std::ffi::c_void, cursor: *const u16) -> Hcursor;
    fn GetDC(window: Hwnd) -> Hdc;
    fn ReleaseDC(window: Hwnd, dc: Hdc) -> Bool;
}

/// Load the crosshair cursor for the overlay window class.
///
/// # Safety
/// A null instance resolves a shared stock resource; failure only means
/// the system's default arrow is used.
pub(super) unsafe fn cross_cursor() -> Hcursor {
    unsafe { LoadCursorW(std::ptr::null_mut(), crate::winapi::IDC_CROSS as *const u16) }
}

/// Borrow the window's client DC for marquee drawing.
///
/// # Safety
/// `window` must be a live window owned by the calling thread; the DC is
/// released before the borrow ends.
pub(super) unsafe fn client_dc(window: Hwnd) -> Hdc {
    unsafe { GetDC(window) }
}

/// Return a client DC borrowed above.
///
/// # Safety
/// Pairs with `client_dc` on the same thread.
pub(super) unsafe fn release_client_dc(window: Hwnd, dc: Hdc) {
    unsafe { ReleaseDC(window, dc) };
}
