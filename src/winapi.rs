//! Shared Win32 declarations: types, message constants, and window plumbing
//! used by more than one frontend (tray, selector). Declare once, document
//! once; specialties stay in their own modules.

use std::ffi::c_void;

pub type Hwnd = *mut c_void;
pub type Hinstance = *mut c_void;
pub type Hicon = *mut c_void;
pub type Hcursor = *mut c_void;
pub type Hbrush = *mut c_void;
pub type Hdc = *mut c_void;
pub type Wparam = usize;
pub type Lparam = isize;
pub type Lresult = isize;
pub type Atom = u16;
pub type Bool = i32;

pub type WndProc = unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult;

// Message constants (winuser.h).
pub const WM_CREATE: u32 = 0x0001;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_NULL: u32 = 0x0000;
pub const WM_USER: u32 = 0x0400;
pub const WM_HOTKEY: u32 = 0x0312;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONUP: u32 = 0x0205;

/// `GetMessageW` reports this when the pump should stop.
pub const GETMESSAGE_QUIT: i32 = 0;

// Window creation (selector overlay).
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const SW_SHOW: i32 = 5;
/// `CW_USEDEFAULT` position sentinel.
pub const CW_USEDEFAULT: i32 = i32::MIN;

// System metrics ids for the whole virtual desktop (all monitors).
pub const SM_XVIRTUALSCREEN: i32 = 76;
pub const SM_YVIRTUALSCREEN: i32 = 77;
pub const SM_CXVIRTUALSCREEN: i32 = 78;
pub const SM_CYVIRTUALSCREEN: i32 = 79;

/// Cursor resource id for the crosshair (`MAKEINTRESOURCE(IDC_CROSS)`).
pub const IDC_CROSS: usize = 32515;

/// Virtual key for the Escape key.
pub const VK_ESCAPE: usize = 0x1B;

#[link(name = "user32")]
unsafe extern "system" {
    pub fn RegisterClassExW(class: *const WndClassExW) -> Atom;
    pub fn CreateWindowExW(
        style_ex: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: *mut c_void,
        instance: Hinstance,
        param: *mut c_void,
    ) -> Hwnd;
    pub fn DefWindowProcW(window: Hwnd, message: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
    pub fn GetMessageW(message: *mut Msg, window: Hwnd, first: u32, last: u32) -> i32;
    pub fn TranslateMessage(message: *const Msg) -> Bool;
    pub fn DispatchMessageW(message: *const Msg) -> Lresult;
    pub fn PostQuitMessage(exit_code: i32);
    pub fn PostMessageW(window: Hwnd, message: u32, wparam: Wparam, lparam: Lparam) -> Bool;
    pub fn DestroyWindow(window: Hwnd) -> Bool;
    pub fn UnregisterClassW(class_name: *const u16, instance: Hinstance) -> Atom;
    pub fn GetModuleHandleW(name: *const u16) -> Hinstance;
    pub fn GetCursorPos(point: *mut Point) -> Bool;
    pub fn GetForegroundWindow() -> Hwnd;
    pub fn SetForegroundWindow(window: Hwnd) -> Bool;
    pub fn ShowWindow(window: Hwnd, command: i32) -> Bool;
    pub fn DrawFocusRect(dc: Hdc, rect: *const Rect) -> Bool;
    pub fn SetCapture(window: Hwnd) -> Hwnd;
    pub fn ReleaseCapture() -> Bool;
    pub fn GetSystemMetrics(index: i32) -> i32;
}

/// `POINT`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// `RECT`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    /// A zeroed rect stands for "no selection yet".
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.left == 0 && self.top == 0 && self.right == 0 && self.bottom == 0
    }
}

/// `MSG`; position/format matches the Win32 ABI.
#[repr(C)]
pub struct Msg {
    pub hwnd: Hwnd,
    pub message: u32,
    pub w_param: Wparam,
    pub l_param: Lparam,
    pub time: u32,
    pub point: Point,
}

/// `WNDCLASSEXW`; position/format matches the Win32 ABI.
#[repr(C)]
pub struct WndClassExW {
    pub size: u32,
    pub style: u32,
    pub wnd_proc: WndProc,
    pub cls_extra: i32,
    pub wnd_extra: i32,
    pub instance: Hinstance,
    pub icon: Hicon,
    pub cursor: Hcursor,
    pub background: Hbrush,
    pub menu_name: *const u16,
    pub class_name: *const u16,
    pub icon_small: Hicon,
}
