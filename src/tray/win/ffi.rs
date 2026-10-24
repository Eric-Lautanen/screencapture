//! Win32 declarations specific to the tray: notification area, menus,
//! hotkeys, and timers. Shared window plumbing lives in `crate::winapi`
//! and is re-exported here so tray callers keep a single import site.

use crate::pin_layout;
use std::ffi::c_void;

pub(super) use crate::winapi::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GETMESSAGE_QUIT, GetCursorPos, GetMessageW, Hicon, Hinstance, Hwnd, Lparam, Lresult, Msg,
    Point, PostMessageW, PostQuitMessage, RegisterClassExW, SetForegroundWindow, TranslateMessage,
    UnregisterClassW, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_NULL,
    WM_RBUTTONUP, WM_TIMER, WM_USER, WndClassExW, Wparam,
};

// Shell namespace (shellapi.h).
pub(super) const NIM_ADD: u32 = 0x0000;
pub(super) const NIM_MODIFY: u32 = 0x0001;
pub(super) const NIM_DELETE: u32 = 0x0002;
pub(super) const NIF_MESSAGE: u32 = 0x0001;
pub(super) const NIF_ICON: u32 = 0x0002;
pub(super) const NIF_TIP: u32 = 0x0004;
/// `NIF_INFO` (shellapi.h): the balloon fields below are valid.
pub(super) const NIF_INFO: u32 = 0x0010;
/// `NIIF_WARNING` (shellapi.h): warning icon in the balloon.
pub(super) const NIIF_WARNING: u32 = 0x0002;

// Menu constants.
pub(super) const TPM_RIGHTBUTTON: u32 = 0x0002;
pub(super) const TPM_BOTTOMALIGN: u32 = 0x0020;
pub(super) const MF_STRING: u32 = 0x0000;
pub(super) const MF_SEPARATOR: u32 = 0x0800;
pub(super) const MF_POPUP: u32 = 0x0010;
/// `MF_CHECKED` (winuser.h): the check mark beside a menu item.
pub(super) const MF_CHECKED: u32 = 0x0008;

// Shared icon resource for the system fallback icon.
pub(super) const IDI_APPLICATION: usize = 32512;

pub(super) type Hmenu = *mut c_void;

#[link(name = "shell32")]
unsafe extern "system" {
    pub(super) fn Shell_NotifyIconW(action: u32, data: *mut NotifyIconDataW) -> Bool;
}

#[link(name = "user32")]
unsafe extern "system" {
    pub(super) fn CreatePopupMenu() -> Hmenu;
    pub(super) fn AppendMenuW(menu: Hmenu, flags: u32, id: Wparam, text: *const u16) -> Bool;
    pub(super) fn DestroyMenu(menu: Hmenu) -> Bool;
    pub(super) fn TrackPopupMenu(
        menu: Hmenu,
        flags: u32,
        x: i32,
        y: i32,
        reserved: i32,
        window: Hwnd,
        rect: *const Rect,
    ) -> Bool;
    pub(super) fn LoadIconW(instance: Hinstance, icon: *const u16) -> Hicon;
    pub(super) fn SetTimer(window: Hwnd, id: usize, interval_ms: u32, proc: *const c_void)
    -> usize;
    pub(super) fn KillTimer(window: Hwnd, id: usize) -> Bool;
    pub(super) fn RegisterHotKey(window: Hwnd, id: i32, modifiers: u32, key: u32) -> Bool;
    pub(super) fn UnregisterHotKey(window: Hwnd, id: i32) -> Bool;
}

/// `GUID` laid out to the Win32 ABI (the shared pin in `winapi.rs`).
pub(super) use crate::winapi::Guid;

/// `NOTIFYICONDATAW` laid out to the Win32 ABI.
#[repr(C)]
pub(super) struct NotifyIconDataW {
    pub(super) size: u32,
    pub(super) window: Hwnd,
    pub(super) id: u32,
    pub(super) flags: u32,
    pub(super) callback_message: u32,
    pub(super) icon: Hicon,
    pub(super) tip: [u16; 128],
    pub(super) state: u32,
    pub(super) state_mask: u32,
    pub(super) info: [u16; 256],
    pub(super) version: u32,
    pub(super) info_title: [u16; 64],
    pub(super) info_flags: u32,
    pub(super) guid: Guid,
}

// The whole balloon-capable descriptor: x64 accounting matches shell32.
// Both handle slots demand 8-byte alignment, which absorbs two pads.
pin_layout!(NotifyIconDataW, 968; size = 0, window = 8, id = 16,
    flags = 20, callback_message = 24, icon = 32, tip = 40, state = 296,
    state_mask = 300, info = 304, version = 816, info_title = 820,
    info_flags = 948, guid = 952);

/// `BITMAPINFO`-family types (`Bool`, `Rect`) re-exported for callers.
pub(super) use crate::winapi::{Bool, Rect};
