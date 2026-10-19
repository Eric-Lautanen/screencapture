//! Tray menu construction and presentation, fresh on each open.

use super::ffi::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, Hmenu, Hwnd, MF_POPUP, MF_SEPARATOR,
    MF_STRING, Point, PostMessageW, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON,
    TrackPopupMenu, WM_NULL, Wparam,
};
use super::{ID_AUTOSTART, ID_EXIT, ID_PHOTO, ID_PROFILE_BASE, ID_RECORD};
use crate::recording::Profile;

/// Build the popup menu at the cursor and let the window collect the picks.
///
/// # Safety
/// `window` must be a valid window owned by the calling pump thread.
pub(super) unsafe fn show(window: Hwnd) {
    let menu = {
        // SAFETY: parameterless Win32 menu creation.
        unsafe { CreatePopupMenu() }
    };
    if menu.is_null() {
        return;
    }
    add_text(menu, ID_RECORD, "Record");
    add_text(menu, ID_PHOTO, "Photo");
    add_text(menu, ID_AUTOSTART, "Autostart");
    add_separator(menu);
    add_profiles(menu);
    add_separator(menu);
    add_text(menu, ID_EXIT, "Exit");

    let mut cursor = Point { x: 0, y: 0 };
    // SAFETY: writes only the local `cursor` point.
    unsafe { GetCursorPos(&raw mut cursor) };
    // SAFETY: `window` is the tray window; tracking focuses it, and the
    // trailing null message restores the input-focus handshake documented
    // for `TrackPopupMenu`.
    unsafe { SetForegroundWindow(window) };
    // SAFETY: menu is ours; rect is unused for cursor-anchored tracking.
    unsafe {
        TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            cursor.x,
            cursor.y,
            0,
            window,
            std::ptr::null(),
        );
    }
    // SAFETY: menu was created above and is no longer needed.
    unsafe { DestroyMenu(menu) };
    // SAFETY: posts to this thread's own window; no out-parameters.
    unsafe { PostMessageW(window, WM_NULL, 0, 0) };
}

fn add_text(menu: Hmenu, id: usize, text: &str) {
    let label = super::wide(text);
    // SAFETY: menu is a live handle; `label` outlives the call.
    let _ = unsafe { AppendMenuW(menu, MF_STRING, id, label.as_ptr()) };
}

fn add_separator(menu: Hmenu) {
    // SAFETY: menu is a live handle; no string data for a separator.
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null()) };
}

fn add_profiles(menu: Hmenu) {
    let submenu = {
        // SAFETY: parameterless Win32 menu creation.
        unsafe { CreatePopupMenu() }
    };
    if submenu.is_null() {
        return;
    }
    for (offset, profile) in Profile::PROFILES.iter().enumerate() {
        add_text(submenu, ID_PROFILE_BASE + offset, profile.label);
    }
    let label = super::wide("Profile");
    // SAFETY: `submenu` stays alive until `menu` (its parent) is destroyed.
    let _ = unsafe { AppendMenuW(menu, MF_POPUP, submenu as Wparam, label.as_ptr()) };
}
