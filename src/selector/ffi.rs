//! Selector-only Win32: cursor loading. The rest of the overlay window
//! plumbing is declared once in `crate::winapi`; DC and capture handles
//! live behind the guard types in `crate::guard`.

use crate::winapi::Hcursor;

#[link(name = "user32")]
unsafe extern "system" {
    fn LoadCursorW(instance: *mut std::ffi::c_void, cursor: *const u16) -> Hcursor;
}

/// Load the crosshair cursor for the overlay window class.
///
/// # Safety
/// A null instance resolves a shared stock resource; failure only means
/// the system's default arrow is used.
pub(super) unsafe fn cross_cursor() -> Hcursor {
    unsafe { LoadCursorW(std::ptr::null_mut(), crate::winapi::IDC_CROSS as *const u16) }
}
