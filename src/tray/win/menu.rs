//! Tray menu construction and presentation, fresh on each open.

use super::ffi::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, Hmenu, Hwnd, MF_CHECKED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, Point, PostMessageW, SetForegroundWindow, TPM_BOTTOMALIGN,
    TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL, Wparam,
};
use super::{
    ID_CHIMES, ID_DISPLAY_BASE, ID_EXIT, ID_PHOTO, ID_PROFILE_BASE, ID_RECORD, ID_SETTINGS,
};
use crate::recording::Profile;
use crate::screen;

/// Profiles and per-display entries share the `WM_COMMAND` id space; the
/// profile range must never reach the display range.
const _: () = assert!(
    Profile::PROFILES.len() <= ID_DISPLAY_BASE - ID_PROFILE_BASE,
    "too many video profiles for the tray menu id range"
);

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
    add_checkable(
        menu,
        ID_CHIMES,
        "Mute capture chimes",
        mute_checked(super::chimes_enabled()),
    );
    add_text(menu, ID_SETTINGS, "Settings...");
    add_displays(menu);
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

/// One text entry with a check mark; the caller supplies the state.
fn add_checkable(menu: Hmenu, id: usize, text: &str, checked: bool) {
    let label = super::wide(text);
    // SAFETY: menu is a live handle; `label` outlives the call.
    let _ = unsafe { AppendMenuW(menu, check_flags(checked), id, label.as_ptr()) };
}

/// The mute entry reads checked while chimes are off, so the check mark
/// always describes the muted state the entry toggles.
fn mute_checked(enabled: bool) -> bool {
    !enabled
}

/// `MF_STRING` plus the check bit when the entry is on.
fn check_flags(checked: bool) -> u32 {
    MF_STRING | if checked { MF_CHECKED } else { 0 }
}

fn add_separator(menu: Hmenu) {
    // SAFETY: menu is a live handle; no string data for a separator.
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null()) };
}

/// A "Displays" submenu when several are attached, so recording the
/// non-primary panel is one click; single-panel machines skip it (the
/// list is enumerated fresh at every menu open, reflecting plug events).
fn add_displays(menu: Hmenu) {
    let attached = screen::displays();
    if attached.len() < 2 {
        return;
    }
    let submenu = {
        // SAFETY: parameterless Win32 menu creation.
        unsafe { CreatePopupMenu() }
    };
    if submenu.is_null() {
        return;
    }
    for (offset, display) in attached.iter().enumerate() {
        let label = display_menu_label(display, offset);
        add_text(submenu, ID_DISPLAY_BASE + offset, &label);
    }
    let label = super::wide("Record display");
    // SAFETY: `submenu` stays alive until `menu` (its parent) is destroyed.
    let _ = unsafe { AppendMenuW(menu, MF_POPUP, submenu as Wparam, label.as_ptr()) };
}

/// "Display 2 — 2560x1440 (+3840,0)"; the origin marks non-primary panels.
fn display_menu_label(display: &screen::Display, offset: usize) -> String {
    let numbered = offset + 1;
    if display.x == 0 && display.y == 0 {
        format!("Display {numbered} — {}x{}", display.width, display.height)
    } else {
        format!(
            "Display {numbered} — {}x{} (+{},+{})",
            display.width, display.height, display.x, display.y
        )
    }
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

#[cfg(test)]
mod tests {
    use super::{MF_CHECKED, check_flags, mute_checked};

    #[test]
    fn the_mute_entry_checks_only_while_chimes_are_off() {
        assert!(mute_checked(false), "muted shows the check");
        assert!(!mute_checked(true), "audible stays unchecked");
        assert_eq!(check_flags(false) & MF_CHECKED, 0);
        assert_eq!(check_flags(true) & MF_CHECKED, MF_CHECKED);
    }
}
