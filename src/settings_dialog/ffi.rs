//! Win32 surface local to the settings dialog (kept out of `winapi.rs`,
//! which stays for the shared window plumbing): control styles and
//! messages, control messaging, and the shell folder picker.

use crate::pin_layout;
use std::ffi::c_void;

use crate::winapi::{Hicon, Hinstance, Hwnd, Lparam, Lresult, Msg, Wparam};

// Window and control styles (winuser.h).
pub(super) const WS_CAPTION: u32 = 0x00C0_0000;
pub(super) const WS_SYSMENU: u32 = 0x0008_0000;
/// Parent paints only its own uncovered area, never over child controls.
pub(super) const WS_CLIPCHILDREN: u32 = 0x0200_0000;
/// Enables the dialog manager's nested-control tab walk.
pub(super) const WS_EX_CONTROLPARENT: u32 = 0x0001_0000;
pub(super) const WS_CHILD: u32 = 0x4000_0000;
pub(super) const WS_VISIBLE: u32 = 0x1000_0000;
pub(super) const WS_TABSTOP: u32 = 0x0001_0000;

pub(super) const ES_AUTOHSCROLL: u32 = 0x0080;
pub(super) const CBS_DROPDOWNLIST: u32 = 0x0003;
pub(super) const BS_PUSHBUTTON: u32 = 0x0000_0000;
pub(super) const BS_DEFPUSHBUTTON: u32 = 0x0000_0001;
/// `BS_AUTOCHECKBOX` toggles itself on click; plain `BS_CHECKBOX`
/// (`0x0002`) leaves the check state to the application and made every
/// box look unresponsive.
pub(super) const BS_AUTOCHECKBOX: u32 = 0x0000_0003;
pub(super) const SS_LEFT: u32 = 0x0000_0000;

// Control and button messages. `BM_GETCHECK` is `0x00F0` and
// `BM_SETCHECK` is `0x00F1`; the neighbouring ids are `BM_GETSTATE`
// (`0x00F2`) and `BM_CLICK` (`0x00F5`), which are *not* interchangeable:
// sending `BM_CLICK` while seeding a checkbox toggles it and fires a
// phantom command.
pub(super) const BM_GETCHECK: u32 = 0x00F0;
pub(super) const BM_SETCHECK: u32 = 0x00F1;
pub(super) const CB_ADDSTRING: u32 = 0x0143;
pub(super) const CB_GETCURSEL: u32 = 0x0147;
pub(super) const CB_SETCURSEL: u32 = 0x014E;
pub(super) const WM_CLOSE: u32 = 0x0010;
pub(super) const WM_DESTROY: u32 = 0x0002;
pub(super) const WM_COMMAND: u32 = 0x0111;
pub(super) const WM_SETFOCUS: u32 = 0x0007;
pub(super) const WM_KILLFOCUS: u32 = 0x0008;
pub(super) const WM_KEYDOWN: u32 = 0x0100;
pub(super) const WM_KEYUP: u32 = 0x0101;
pub(super) const WM_CHAR: u32 = 0x0102;
pub(super) const WM_SYSKEYDOWN: u32 = 0x0104;
pub(super) const WM_SYSKEYUP: u32 = 0x0105;
pub(super) const WM_SYSCHAR: u32 = 0x0106;
pub(super) const WM_PASTE: u32 = 0x0302;
pub(super) const WM_LBUTTONDOWN: u32 = 0x0201;
pub(super) const WM_CTLCOLORSTATIC: u32 = 0x0138;
/// `GWLP_WNDPROC` (winuser.h): the subclass slot for an edit control.
pub(super) const GWLP_WNDPROC: i32 = -4;
/// `TRANSPARENT` background mode (wingdi.h).
pub(super) const TRANSPARENT: i32 = 1;
/// The button notification this window answers.
pub(super) const BN_CLICKED: u16 = 0;
/// Sets the control's font (`wparam` = font, `lparam` nonzero redraws).
pub(super) const WM_SETFONT: u32 = 0x0030;
/// Sets a window icon (`wparam` `0` = small/title, `1` = big).
pub(super) const WM_SETICON: u32 = 0x0080;
/// `BS_GROUPBOX` (winuser.h): a titled rectangle around related rows.
/// (`0x0009` looks tempting but is `BS_AUTORADIOBUTTON`.)
pub(super) const BS_GROUPBOX: u32 = 0x0000_0007;
/// `WS_EX_DLGMODALFRAME` (winuser.h): the classic thick modal border.
pub(super) const WS_EX_DLGMODALFRAME: u32 = 0x0000_0001;
/// `WS_EX_CLIENTEDGE` (winuser.h): the sunken text-field border.
pub(super) const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;
/// `SM_CXSCREEN` (winuser.h): primary display width in pixels.
pub(super) const SM_CXSCREEN: i32 = 0;
/// `SM_CYSCREEN` (winuser.h): primary display height in pixels.
pub(super) const SM_CYSCREEN: i32 = 1;
/// `SPI_GETWORKAREA` (winuser.h): the display area left after the taskbar,
/// which is what a dialog has to fit inside.
pub(super) const SPI_GETWORKAREA: u32 = 0x0030;
/// `WS_CLIPSIBLINGS` (winuser.h): children never paint over each other.
pub(super) const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
/// `DEFAULT_GUI_FONT` is avoided; this asks for the DPI-aware system DPI.
pub(super) const DEFAULT_DPI: u32 = 96;
/// `IMAGE_ICON` (winuser.h): the icon shape `LoadImageW` loads.
pub(super) const IMAGE_ICON: u32 = 1;
/// `LR_DEFAULTSIZE` (winuser.h): system metric icon sizing.
pub(super) const LR_DEFAULTSIZE: u32 = 0x0000_0040;
/// `MAKEINTRESOURCEW` index (winuser.h) of the exe's icon (`icon.rc`).
pub(super) const ICON_RESOURCE_ID: usize = 1;
/// `FW_NORMAL` (wingdi.h): the UI text weight.
pub(super) const FW_NORMAL: i32 = 400;
/// `CLEARTYPE_QUALITY` (wingdi.h): subpixel text rendering where present.
pub(super) const CLEARTYPE_QUALITY: u8 = 5;
/// `DEFAULT_CHARSET` (wingdi.h): follow the system locale.
pub(super) const DEFAULT_CHARSET: u8 = 1;
/// 9 pt at 96 dpi, expressed as the negative character height GDI wants.
pub(super) const UI_FONT_HEIGHT: i32 = -12;

/// `MessageBoxW` shape: stop-icon box with one OK button.
pub(super) const MB_ICONERROR_OK: u32 = 0x0000_0010;
/// `HWND_TOP` (winuser.h): raise the control above its siblings.
pub(super) const HWND_TOP: Hwnd = 0isize as Hwnd;
/// `SWP_NOSIZE` (winuser.h): keep current size.
pub(super) const SWP_NOSIZE: u32 = 0x0001;
/// `SWP_NOMOVE` (winuser.h): keep current position.
pub(super) const SWP_NOMOVE: u32 = 0x0002;
/// `SWP_NOACTIVATE` (winuser.h): do not activate the window.
pub(super) const SWP_NOACTIVATE: u32 = 0x0010;
/// `BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE` (`shlobj_core.h`).
pub(super) const FOLDER_FLAGS: u32 = 0x0001 | 0x0040;
/// `MAX_PATH` (minwindef.h): folder answers live inside this bound.
pub(super) const MAX_PATH: usize = 260;

pub(super) type Bool = i32;

#[link(name = "user32")]
unsafe extern "system" {
    /// Dialog keyboard routing: Tab moves focus and Enter presses the
    /// `BS_DEFPUSHBUTTON`. Nonzero means the message was consumed.
    pub(super) fn IsDialogMessageW(dialog: Hwnd, message: *mut Msg) -> Bool;
    /// Generic control messaging: combos, checks, and edit reads below.
    pub(super) fn SendMessageW(
        window: Hwnd,
        message: u32,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult;
    pub(super) fn GetWindowTextLengthW(window: Hwnd) -> i32;
    pub(super) fn GetWindowTextW(window: Hwnd, buffer: *mut u16, count_max: i32) -> i32;
    pub(super) fn SetWindowTextW(window: Hwnd, text: *const u16) -> Bool;
    pub(super) fn GetKeyState(key: i32) -> i16;
    /// The focused window of the calling thread; `null` when none.
    pub(super) fn GetFocus() -> Hwnd;
    pub(super) fn SetWindowLongPtrW(window: Hwnd, index: i32, value: isize) -> isize;
    pub(super) fn CallWindowProcW(
        previous: isize,
        window: Hwnd,
        message: u32,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult;
    pub(super) fn InvalidateRect(window: Hwnd, rect: *const Rect, erase: Bool) -> Bool;
    pub(super) fn SetTextColor(dc: crate::winapi::Hdc, color: u32) -> u32;
    pub(super) fn SetBkMode(dc: crate::winapi::Hdc, mode: i32) -> i32;
    pub(super) fn MessageBoxW(owner: Hwnd, text: *const u16, caption: *const u16, kind: u32)
    -> i32;
    /// Loads the exe's baked-in icon for the title bar (`name` is the
    /// resource ordinal from `assets/icon.rc`); `width`/`height` 0 with
    /// [`LR_DEFAULTSIZE`] asks for the system sizes.
    pub(super) fn LoadImageW(
        instance: Hinstance,
        name: *const u16,
        kind: u32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> *mut c_void;
    /// Releases an icon handle; `WM_SETICON` borrows the handle and never
    /// takes ownership, so the loader still owes this call.
    pub(super) fn DestroyIcon(icon: Hicon) -> Bool;
    /// Grows a wanted client rect into the to-be window rect (adds the
    /// caption/borders for `style`/`style_ex`); `menu` nonzero adds a
    /// menu bar this dialog does not have.
    pub(super) fn AdjustWindowRectEx(
        rect: *mut Rect,
        style: u32,
        menu: Bool,
        style_ex: u32,
    ) -> Bool;
    /// Per-monitor DPI in effect when the dialog opens (Windows 10+).
    pub(super) fn GetDpiForSystem() -> u32;
    /// Reads a system parameter; [`SPI_GETWORKAREA`] writes a [`Rect`]
    /// through `data` describing the usable desktop of the primary display.
    pub(super) fn SystemParametersInfoW(
        action: u32,
        parameter: u32,
        data: *mut c_void,
        update: u32,
    ) -> Bool;
    pub(super) fn SetWindowPos(
        window: Hwnd,
        insert_after: Hwnd,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    /// Creates an `HFONT` from a filled [`LogFontW`]; the caller deletes
    /// it with `DeleteObject` when the dialog closes.
    pub(super) fn CreateFontIndirectW(font: *const LogFontW) -> *mut c_void;
    pub(super) fn DeleteObject(object: *mut c_void) -> Bool;
}

/// `LOGFONTW` (wingdi.h), fields narrowed to their natural types.
#[repr(C)]
pub(super) struct LogFontW {
    /// Requested height in pixels (negative = character height).
    pub height: i32,
    /// Average character width (0 lets the mapper pick).
    pub width: i32,
    /// Rotation in tenths of degrees (unused for UI text).
    pub escapement: i32,
    /// Baseline rotation (unused for UI text).
    pub orientation: i32,
    /// Weight class (`400` normal, `700` bold).
    pub weight: i32,
    /// Italic flag (off for UI text).
    pub italic: u8,
    /// Underline flag (off for UI text).
    pub underline: u8,
    /// Strikeout flag (off for UI text).
    pub strike_out: u8,
    /// Character set (`DEFAULT_CHARSET` follows the system locale).
    pub char_set: u8,
    /// Output precision (`OUT_DEFAULT_PRECIS` = 0 lets GDI map it).
    pub out_precision: u8,
    /// Clipping precision (`OUT_DEFAULT_PRECIS`).
    pub clip_precision: u8,
    /// Rendering quality (`CLEARTYPE_QUALITY` where available).
    pub quality: u8,
    /// Pitch/family (`0` = default pitch, neutral family).
    pub pitch_and_family: u8,
    /// Face name, NUL-padded (`LF_FACESIZE` = 32 wide chars).
    pub face_name: [u16; 32],
}

// GDI's font descriptor: 11 scalar fields then the 64-byte face buffer.
pin_layout!(LogFontW, 92; height = 0, width = 4, escapement = 8,
    orientation = 12, weight = 16, italic = 20, underline = 21,
    strike_out = 22, char_set = 23, out_precision = 24,
    clip_precision = 25, quality = 26, pitch_and_family = 27,
    face_name = 28);

/// `RECT` (`windef.h`): the shared pinned shape from `winapi.rs`; the
/// dialog needs no second copy.
pub(super) use crate::winapi::Rect;

/// `BROWSEINFOW` (`shlobj_core.h`): the classic folder-pick descriptor.
#[repr(C)]
pub(super) struct BrowseInfo {
    /// Owning window (the settings dialog).
    pub owner: Hwnd,
    /// Null roots the dialog at the desktop namespace.
    pub root: *mut ItemList,
    /// Optional buffer for the picked item's display name.
    pub display_name: *mut u16,
    /// Prompt shown above the tree.
    pub title: *const u16,
    /// The [`FOLDER_FLAGS`] combination.
    pub flags: u32,
    /// Optional hook callback; unused here.
    pub callback: Option<unsafe extern "system" fn(Hwnd, u32, Lparam, Lparam) -> i32>,
    /// Opaque callback parameter; unused here.
    pub callback_data: Lparam,
    /// Receives the picked item's shell image index.
    pub image: i32,
}

// The folder-pick descriptor: payload ends with the image slot at 60.
pin_layout!(BrowseInfo, 64; owner = 0, root = 8, display_name = 16,
    title = 24, flags = 32, callback = 40, callback_data = 48, image = 56);

/// `LPITEMIDLIST`: shell namespace pointer, freed with `CoTaskMemFree`.
pub(super) type ItemList = c_void;

#[link(name = "shell32")]
unsafe extern "system" {
    pub(super) fn SHBrowseForFolderW(info: *mut BrowseInfo) -> *mut ItemList;
    pub(super) fn SHGetPathFromIDListW(list: *const ItemList, path: *mut u16) -> Bool;
}

#[link(name = "ole32")]
unsafe extern "system" {
    pub(super) fn CoTaskMemFree(block: *mut c_void);
}
