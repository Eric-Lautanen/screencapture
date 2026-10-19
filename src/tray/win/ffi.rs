//! Win32 declarations for the tray window, notification area, and menus
//! (edition 2024 `unsafe extern`, std types only).

use std::ffi::c_void;

pub(super) type Hwnd = *mut c_void;
pub(super) type Hmodule = *mut c_void;
pub(super) type Hicon = *mut c_void;
pub(super) type Hcursor = *mut c_void;
pub(super) type Hbrush = *mut c_void;
pub(super) type Hmenu = *mut c_void;
pub(super) type Wparam = usize;
pub(super) type Lparam = isize;
pub(super) type Lresult = isize;
pub(super) type Atom = u16;
pub(super) type Bool = i32;

pub(super) type WndProc = unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult;

// Message constants (winuser.h).
pub(super) const WM_CREATE: u32 = 0x0001;
pub(super) const WM_DESTROY: u32 = 0x0002;
pub(super) const WM_COMMAND: u32 = 0x0111;
pub(super) const WM_NULL: u32 = 0x0000;
pub(super) const WM_USER: u32 = 0x0400;
pub(super) const WM_LBUTTONUP: Lparam = 0x0202;
pub(super) const WM_RBUTTONUP: Lparam = 0x0205;

// Notification-area constants (shellapi.h).
pub(super) const NIM_ADD: u32 = 0x0000;
pub(super) const NIM_DELETE: u32 = 0x0002;
pub(super) const NIF_MESSAGE: u32 = 0x0001;
pub(super) const NIF_ICON: u32 = 0x0002;
pub(super) const NIF_TIP: u32 = 0x0004;

// Menu and window constants.
pub(super) const IDI_APPLICATION: usize = 32512;
pub(super) const CW_USEDEFAULT: i32 = i32::MIN;
pub(super) const TPM_RIGHTBUTTON: u32 = 0x0002;
pub(super) const TPM_BOTTOMALIGN: u32 = 0x0020;
pub(super) const MF_STRING: u32 = 0x0000;
pub(super) const MF_SEPARATOR: u32 = 0x0800;
pub(super) const MF_POPUP: u32 = 0x0010;
pub(super) const GETMESSAGE_QUIT: i32 = 0;

#[link(name = "user32")]
unsafe extern "system" {
    pub(super) fn RegisterClassExW(class: *const WndClassExW) -> Atom;
    pub(super) fn CreateWindowExW(
        style_ex: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: Hmenu,
        instance: Hmodule,
        param: *mut c_void,
    ) -> Hwnd;
    pub(super) fn DefWindowProcW(
        window: Hwnd,
        message: u32,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult;
    pub(super) fn GetMessageW(message: *mut Msg, window: Hwnd, first: u32, last: u32) -> i32;
    pub(super) fn TranslateMessage(message: *const Msg) -> Bool;
    pub(super) fn DispatchMessageW(message: *const Msg) -> Lresult;
    pub(super) fn PostQuitMessage(exit_code: i32);
    pub(super) fn DestroyWindow(window: Hwnd) -> Bool;
    pub(super) fn PostMessageW(window: Hwnd, message: u32, wparam: Wparam, lparam: Lparam) -> Bool;
}

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
    pub(super) fn SetForegroundWindow(window: Hwnd) -> Bool;
    pub(super) fn GetCursorPos(point: *mut Point) -> Bool;
    pub(super) fn LoadIconW(instance: Hmodule, icon: *const u16) -> Hicon;
    pub(super) fn UnregisterClassW(class_name: *const u16, instance: Hmodule) -> Atom;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub(super) fn GetModuleHandleW(name: *const u16) -> Hmodule;
}

/// `POINT`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Point {
    pub(super) x: i32,
    pub(super) y: i32,
}

/// `RECT` (unused fields are kept for completeness).
#[repr(C)]
pub(super) struct Rect {
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) right: i32,
    pub(super) bottom: i32,
}

/// `MSG`; position/format matches the Win32 ABI.
#[repr(C)]
pub(super) struct Msg {
    pub(super) hwnd: Hwnd,
    pub(super) message: u32,
    pub(super) w_param: Wparam,
    pub(super) l_param: Lparam,
    pub(super) time: u32,
    pub(super) point: Point,
}

/// `WNDCLASSEXW`; position/format matches the Win32 ABI.
#[repr(C)]
pub(super) struct WndClassExW {
    pub(super) size: u32,
    pub(super) style: u32,
    pub(super) wnd_proc: WndProc,
    pub(super) cls_extra: i32,
    pub(super) wnd_extra: i32,
    pub(super) instance: Hmodule,
    pub(super) icon: Hicon,
    pub(super) cursor: Hcursor,
    pub(super) background: Hbrush,
    pub(super) menu_name: *const u16,
    pub(super) class_name: *const u16,
    pub(super) icon_small: Hicon,
}

/// `GUID` laid out to the Win32 ABI.
#[repr(C)]
pub(super) struct Guid {
    pub(super) data1: u32,
    pub(super) data2: u16,
    pub(super) data3: u16,
    pub(super) data4: [u8; 8],
}

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
