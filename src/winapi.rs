//! Shared Win32 declarations: types, message constants, and window plumbing
//! used by more than one frontend (tray, selector). Declare once, document
//! once; specialties stay in their own modules.

use crate::pin_layout;
use std::ffi::c_void;
use std::time::{Duration, Instant};

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
pub const WM_QUIT: u32 = 0x0012;
pub const WM_USER: u32 = 0x0400;
pub const WM_HOTKEY: u32 = 0x0312;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
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

/// Cursor resource ids (`MAKEINTRESOURCE(IDC_*)`): the standard arrow,
/// the clickable-hand pointer, and the crosshair.
pub const IDC_ARROW: usize = 32512;
pub const IDC_HAND: usize = 32649;
pub const IDC_CROSS: usize = 32515;

/// Virtual key for the Escape key.
pub const VK_ESCAPE: usize = 0x1B;

/// `GA_ROOT` (winuser.h): the root owner of a window.
pub const GA_ROOT: u32 = 2;

/// Monitor-enumeration callback; enum procedures deliver one rect per
/// attached display in `l-param`-style data slots.
pub type MonitorProc = unsafe extern "system" fn(
    *mut std::ffi::c_void,
    *mut std::ffi::c_void,
    *const Rect,
    *mut std::ffi::c_void,
) -> i32;

/// `DWMWA_EXTENDED_FRAME_BOUNDS` (dwmapi.h): the visible window area
/// without borders and shadows.
pub const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;

/// `DWMWA_VISIBLE_FRAME_BORDER_THICKNESS` (dwmapi.h, Windows 11): the
/// width of the 1 px border DWM draws around a window. The border is
/// translucent over the desktop, so a capture that keeps it shows what
/// is behind the window along the edges.
pub const DWMWA_VISIBLE_FRAME_BORDER_THICKNESS: u32 = 37;

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
    pub fn PeekMessageW(
        message: *mut Msg,
        window: Hwnd,
        first: u32,
        last: u32,
        remove: u32,
    ) -> Bool;
    pub fn MsgWaitForMultipleObjectsEx(
        count: u32,
        handles: *const *mut c_void,
        wait: u32,
        mask: u32,
        flags: u32,
    ) -> u32;
    pub fn TranslateMessage(message: *const Msg) -> Bool;
    pub fn DispatchMessageW(message: *const Msg) -> Lresult;
    pub fn PostQuitMessage(exit_code: i32);
    pub fn PostMessageW(window: Hwnd, message: u32, wparam: Wparam, lparam: Lparam) -> Bool;
    pub fn DestroyWindow(window: Hwnd) -> Bool;
    pub fn UnregisterClassW(class_name: *const u16, instance: Hinstance) -> Atom;
    pub fn GetModuleHandleW(name: *const u16) -> Hinstance;
    pub fn GetCursorPos(point: *mut Point) -> Bool;
    pub fn SetForegroundWindow(window: Hwnd) -> Bool;
    pub fn ShowWindow(window: Hwnd, command: i32) -> Bool;
    pub fn DrawFocusRect(dc: Hdc, rect: *const Rect) -> Bool;
    pub fn SetCapture(window: Hwnd) -> Hwnd;
    pub fn ReleaseCapture() -> Bool;
    pub fn GetSystemMetrics(index: i32) -> i32;
    pub fn WindowFromPoint(point: Point) -> Hwnd;
    pub fn GetAncestor(window: Hwnd, flags: u32) -> Hwnd;
    pub fn GetWindowRect(window: Hwnd, rect: *mut Rect) -> Bool;
    pub fn GetDC(window: Hwnd) -> Hdc;
    pub fn ReleaseDC(window: Hwnd, dc: Hdc) -> i32;
    pub fn EnumDisplayMonitors(
        dc: Hdc,
        clip: *const Rect,
        callback: MonitorProc,
        data: isize,
    ) -> i32;
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    /// Decoration-aware bounds for highlights and captures; the display
    /// attribute id lives in [`DWMWA_EXTENDED_FRAME_BOUNDS`].
    pub fn DwmGetWindowAttribute(
        window: Hwnd,
        attribute: u32,
        value: *mut c_void,
        size: u32,
    ) -> i32;
}

/// `GUID` (`guiddef.h`): shared by the DXGI interfaces and the tray icon
/// descriptor, so both call sites pin the same shape once.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Guid {
    /// First 32 bits.
    pub a: u32,
    /// Next 16 bits.
    pub b: u16,
    /// Next 16 bits.
    pub c: u16,
    /// Remaining 8 bytes in declared order.
    pub d: [u8; 8],
}

pin_layout!(Guid, 16; a = 0, b = 4, c = 6, d = 8);

/// `POINT`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

// SAFETY-free proof: this layout contract is checked below.
pin_layout!(Point, 8; x = 0, y = 4);

/// `RECT`.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

pin_layout!(Rect, 16; left = 0, top = 4, right = 8, bottom = 12);

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

pin_layout!(Msg, 48; hwnd = 0, message = 8, w_param = 16, l_param = 24,
    time = 32, point = 36);

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

pin_layout!(WndClassExW, 80; size = 0, style = 4, wnd_proc = 8,
    cls_extra = 16, wnd_extra = 20, instance = 24, icon = 32, cursor = 40,
    background = 48, menu_name = 56, class_name = 64, icon_small = 72);

/// How a modal pick's message pump ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PumpEnd {
    /// The caller's predicate answered true.
    Done,
    /// The thread received `WM_QUIT`: the session ended itself.
    Quit,
    /// Nothing happened for [`SELECTION_IDLE_LIMIT`].
    Idle,
}

/// How long a modal pick may sit with a still cursor and no dispatched
/// messages before it cancels itself. Low-level hooks never see input
/// aimed at a higher-integrity window, so a pick whose overlay stopped
/// updating has no other way out and must not hang the tray.
///
/// Two minutes is long enough for a user to pause and decide what to
/// capture, and short enough that a dead session recovers on its own.
pub const SELECTION_IDLE_LIMIT: Duration = Duration::from_secs(120);

/// Pump this thread's messages until `done` answers true, the thread
/// receives a quit message, or `budget` passes with the cursor still and
/// nothing dispatched. Wakes at least every 100 ms, so a caller whose
/// activity arrives through hooks (not messages) still gets its predicate
/// polled while the queue is empty.
pub fn pump_until(budget: Duration, done: impl Fn() -> bool) -> PumpEnd {
    /// `PM_REMOVE` (winuser.h): take the message out of the queue.
    const PM_REMOVE: u32 = 0x0001;
    /// `QS_ALLINPUT` (winuser.h): wake for any input aimed at this thread.
    const QS_ALLINPUT: u32 = 0x04FF;
    /// Longest single wait, so cursor movement is noticed promptly.
    const WAIT_MS: u32 = 100;

    let mut message = Msg {
        hwnd: std::ptr::null_mut(),
        message: 0,
        w_param: 0,
        l_param: 0,
        time: 0,
        point: Point::default(),
    };
    let mut cursor = Point::default();
    // SAFETY: writes only the local point.
    unsafe { GetCursorPos(&raw mut cursor) };
    let mut deadline = Instant::now() + budget;
    loop {
        if done() {
            return PumpEnd::Done;
        }
        loop {
            // SAFETY: `message` is a local struct owned by this loop.
            let queued =
                unsafe { PeekMessageW(&raw mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) }
                    != 0;
            if !queued {
                break;
            }
            if message.message == WM_QUIT {
                return PumpEnd::Quit;
            }
            let _ = unsafe { TranslateMessage(&raw const message) };
            unsafe { DispatchMessageW(&raw const message) };
            deadline = Instant::now() + budget;
            if done() {
                return PumpEnd::Done;
            }
        }
        if Instant::now() >= deadline {
            return PumpEnd::Idle;
        }
        // SAFETY: no handles are waited on; the call only sleeps this
        // thread until input arrives or the poll interval passes.
        unsafe {
            MsgWaitForMultipleObjectsEx(0, std::ptr::null(), WAIT_MS, QS_ALLINPUT, 0);
        }
        let mut moved = Point::default();
        // SAFETY: writes only the local point.
        if unsafe { GetCursorPos(&raw mut moved) } != 0
            && (moved.x != cursor.x || moved.y != cursor.y)
        {
            cursor = moved;
            deadline = Instant::now() + budget;
        }
    }
}
