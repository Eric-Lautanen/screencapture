//! The red dashed frame drawn around the area a recording is capturing,
//! and the two-tone dotted selection frame used during interactive picking.
//! Both use click-through, non-activating, topmost layered windows with
//! `WDA_EXCLUDEFROMCAPTURE` so the chrome never appears in captured frames.

use std::ffi::c_void;
use std::io;

use crate::capture::Region;
use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetModuleHandleW, Hcursor, Hdc, Hinstance,
    Hwnd, Lparam, Lresult, Rect, RegisterClassExW, WndClassExW, Wparam,
};

mod crosshair;
mod stop;
#[cfg(test)]
mod tests;

pub(crate) use crosshair::Crosshair;

use crosshair::{CROSSHAIR_BLACK, CROSSHAIR_WHITE};
use stop::{destroy_stop, show_stop_button};

/// Window class registered for the frame.
const CLASS_NAME: &str = "screencapture.recording.frame";
/// Window class for the picker's selection outline.
const SELECTION_CLASS: &str = "screencapture.selection.frame";

pub(super) const WS_POPUP: u32 = 0x8000_0000;
pub(super) const WS_VISIBLE: u32 = 0x1000_0000;
pub(super) const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub(super) const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
pub(super) const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub(super) const WS_EX_LAYERED: u32 = 0x0008_0000;
pub(super) const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

/// `LWA_COLORKEY` (winuser.h): one colour becomes fully transparent.
pub(super) const LWA_COLORKEY: u32 = 1;
/// `WDA_EXCLUDEFROMCAPTURE` (winuser.h): hide the window from captures.
pub(super) const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
/// `COLORREF` magenta used as the transparent interior.
pub(super) const COLOR_KEY: u32 = 0x00FF_00FF;
/// `COLORREF` red for the frame.
pub(super) const FRAME_RED: u32 = 0x0000_00FF;

pub(super) const WM_PAINT: u32 = 0x000F;
pub(super) const WM_ERASEBKGND: u32 = 0x0014;
pub(super) const WM_LBUTTONDOWN: u32 = 0x0201;
pub(super) const WM_LBUTTONUP: u32 = 0x0202;

/// `DT_*` (winuser.h) for the Stop caption.
pub(super) const DT_CENTER: u32 = 0x0001;
pub(super) const DT_VCENTER: u32 = 0x0004;
pub(super) const DT_SINGLELINE: u32 = 0x0020;
/// `TRANSPARENT` background mode and `CLEARTYPE_QUALITY`.
pub(super) const TRANSPARENT: i32 = 1;
pub(super) const CLEARTYPE_QUALITY: u32 = 5;

/// Selection outline dot pattern, in physical pixels.
pub(super) const SELECTION_DOT: i32 = 2;
const SELECTION_PITCH: i32 = 6;

pub(super) type StopAction = fn();

pub(super) const SWP_NOACTIVATE: u32 = 0x0010;
pub(super) const SWP_SHOWWINDOW: u32 = 0x0040;
pub(super) const HWND_TOPMOST: Hwnd = -1isize as Hwnd;

/// Frame thickness and dash pattern, in physical pixels.
const STROKE: i32 = 2;
const DASH: i32 = 8;
const GAP: i32 = 5;

#[link(name = "user32")]
unsafe extern "system" {
    pub(super) fn BeginPaint(window: Hwnd, paint: *mut c_void) -> Hdc;
    pub(super) fn LoadCursorW(instance: *mut c_void, cursor: *const u16) -> Hcursor;
    pub(super) fn EndPaint(window: Hwnd, paint: *const c_void) -> i32;
    pub(super) fn GetClientRect(window: Hwnd, rect: *mut Rect) -> i32;
    pub(super) fn SetLayeredWindowAttributes(window: Hwnd, key: u32, alpha: u8, flags: u32) -> i32;
    pub(super) fn SetWindowDisplayAffinity(window: Hwnd, affinity: u32) -> i32;
    pub(super) fn SetWindowPos(
        window: Hwnd,
        insert_after: Hwnd,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    pub(super) fn InvalidateRect(window: Hwnd, rect: *const Rect, erase: i32) -> i32;
    pub(super) fn UpdateWindow(window: Hwnd) -> i32;
    pub(super) fn DrawTextW(
        dc: Hdc,
        text: *const u16,
        count: i32,
        rect: *mut Rect,
        format: u32,
    ) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    pub(super) fn CreateSolidBrush(color: u32) -> *mut c_void;
    pub(super) fn DeleteObject(object: *mut c_void) -> i32;
    pub(super) fn FillRect(dc: Hdc, rect: *const Rect, brush: *mut c_void) -> i32;
    pub(super) fn FrameRect(dc: Hdc, rect: *const Rect, brush: *mut c_void) -> i32;
    pub(super) fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike_out: u32,
        char_set: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face: *const u16,
    ) -> *mut c_void;
    pub(super) fn SelectObject(dc: Hdc, object: *mut c_void) -> *mut c_void;
    pub(super) fn SetBkMode(dc: Hdc, mode: i32) -> i32;
    pub(super) fn SetTextColor(dc: Hdc, color: u32) -> u32;
}

/// What a border means, which also decides how it paints.
#[derive(Clone, Copy)]
pub(crate) enum Style {
    /// The picker's selection box: a neutral dotted line around the
    /// hovered window or dragged rectangle. Screen-only — the frame is
    /// capture-excluded so it never lands in the image being picked.
    Selection,
    /// A live recording: the red dashed frame, with a Stop chip whose
    /// click runs the carried action.
    Recording(StopAction),
}

/// A live frame; destroying it removes the outline (and the Stop button,
/// when this is a recording).
pub(crate) struct Border {
    window: Hwnd,
    /// The Stop button window; `None` for the picker's highlight, which
    /// must not offer a way to stop anything.
    stop: Option<Hwnd>,
}

impl Border {
    /// Draw the outline for `style` around `region` for as long as the
    /// guard lives.
    ///
    /// # Errors
    /// Windows refused the window; recording continues without a frame.
    pub(crate) fn show(region: Region, style: Style) -> io::Result<Self> {
        let width = i32::try_from(region.width).unwrap_or(0);
        let height = i32::try_from(region.height).unwrap_or(0);
        if width <= 0 || height <= 0 {
            return Err(io::Error::other("the capture region is empty"));
        }
        let instance = module_handle();
        let (class_name, proc_fn): (
            &str,
            unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult,
        ) = match style {
            Style::Recording(_) => (CLASS_NAME, procedure),
            Style::Selection => (SELECTION_CLASS, selection_procedure),
        };
        register_border_class(instance, class_name, proc_fn);
        let class = wide(class_name);
        let style_ex =
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE;
        // SAFETY: strings and handles outlive the call; owned by this guard.
        let window = unsafe {
            CreateWindowExW(
                style_ex,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP | WS_VISIBLE,
                region.x,
                region.y,
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
        // SAFETY: `window` was created above and belongs to this thread.
        unsafe {
            SetLayeredWindowAttributes(window, COLOR_KEY, 0, LWA_COLORKEY);
            let _ = SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE);
        }
        let border = Self {
            window,
            stop: match style {
                Style::Recording(action) => show_stop_button(region, action),
                Style::Selection => None,
            },
        };
        border.place(region);
        Ok(border)
    }

    /// Move and resize the live outline; no-op if Windows refuses.
    pub(crate) fn place(&self, region: Region) {
        let (Ok(width), Ok(height)) = (i32::try_from(region.width), i32::try_from(region.height))
        else {
            return;
        };
        // SAFETY: the window is this guard's live topmost frame.
        unsafe {
            SetWindowPos(
                self.window,
                HWND_TOPMOST,
                region.x,
                region.y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            InvalidateRect(self.window, std::ptr::null(), 1);
            UpdateWindow(self.window);
        }
    }
}

impl Drop for Border {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            // SAFETY: stop button created by this guard on this thread.
            unsafe { destroy_stop(stop) };
        }
        // SAFETY: the frame this guard created.
        unsafe {
            DestroyWindow(self.window);
        }
    }
}

/// Register a border window class; repeats answer "already registered".
fn register_border_class(
    instance: Hinstance,
    name: &str,
    wnd_proc: unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult,
) {
    let name = wide(name);
    let class = WndClassExW {
        size: u32::try_from(std::mem::size_of::<WndClassExW>()).expect("class size fits u32"),
        style: 0,
        wnd_proc,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: std::ptr::null_mut(),
        cursor: std::ptr::null_mut(),
        background: std::ptr::null_mut(),
        menu_name: std::ptr::null(),
        class_name: name.as_ptr(),
        icon_small: std::ptr::null_mut(),
    };
    // SAFETY: the class struct and its wide string live through the call.
    unsafe {
        RegisterClassExW(&raw const class);
    }
}

/// Paint the colour-key background and the dashed frame.
unsafe extern "system" fn procedure(
    window: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    match message {
        WM_PAINT => {
            // SAFETY: the paint state buffer is large enough for the
            // platform `PAINTSTRUCT` (72 bytes on 64-bit, 64 on 32-bit).
            unsafe { paint(window) };
            0
        }
        // The whole surface is repainted in `WM_PAINT`.
        WM_ERASEBKGND => 1,
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// # Safety
/// `window` must be a live window owned by the calling thread.
unsafe fn paint(window: Hwnd) {
    let mut state = [0u8; 128];
    // SAFETY: `state` is at least `sizeof(PAINTSTRUCT)` for this ABI.
    let dc = unsafe { BeginPaint(window, state.as_mut_ptr().cast()) };
    if dc.is_null() {
        return;
    }
    let mut client = Rect::default();
    // SAFETY: writes only the local `client`.
    unsafe { GetClientRect(window, &raw mut client) };
    // SAFETY: fresh brushes are released after use; `FillRect` reads only
    // the local rect.
    unsafe {
        let key = CreateSolidBrush(COLOR_KEY);
        if !key.is_null() {
            FillRect(dc, &raw const client, key);
            DeleteObject(key);
        }
        let frame = CreateSolidBrush(FRAME_RED);
        if !frame.is_null() {
            for segment in frame_segments(client.right, client.bottom) {
                let rect = Rect {
                    left: segment.0,
                    top: segment.1,
                    right: segment.2,
                    bottom: segment.3,
                };
                FillRect(dc, &raw const rect, frame);
            }
            DeleteObject(frame);
        }
        EndPaint(window, state.as_ptr().cast());
    }
}

/// Dashed segments outlining a `width` by `height` client area.
pub(super) fn frame_segments(width: i32, height: i32) -> Vec<(i32, i32, i32, i32)> {
    let mut segments = Vec::new();
    if width <= 0 || height <= 0 {
        return segments;
    }
    let stroke = STROKE.min(width).min(height);
    let mut x = 0;
    while x < width {
        let end = (x + DASH).min(width);
        segments.push((x, 0, end, stroke));
        segments.push((x, height - stroke, end, height));
        x += DASH + GAP;
    }
    let mut y = 0;
    while y < height {
        let end = (y + DASH).min(height);
        segments.push((0, y, stroke, end));
        segments.push((width - stroke, y, width, end));
        y += DASH + GAP;
    }
    segments
}

/// Paint the colour-key background and the dotted selection outline.
unsafe extern "system" fn selection_procedure(
    window: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    match message {
        WM_PAINT => {
            // SAFETY: the paint state buffer is large enough for the
            // platform `PAINTSTRUCT`.
            unsafe { paint_selection(window) };
            0
        }
        // The whole surface is repainted in `WM_PAINT`.
        WM_ERASEBKGND => 1,
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// # Safety
/// `window` must be a live window owned by the calling thread.
unsafe fn paint_selection(window: Hwnd) {
    let mut state = [0u8; 128];
    // SAFETY: `state` is at least `sizeof(PAINTSTRUCT)` for this ABI.
    let dc = unsafe { BeginPaint(window, state.as_mut_ptr().cast()) };
    if dc.is_null() {
        return;
    }
    let mut client = Rect::default();
    // SAFETY: writes only the local `client`.
    unsafe { GetClientRect(window, &raw mut client) };
    // SAFETY: fresh brushes are released after use; `FillRect` reads only
    // the local rects.
    unsafe {
        let key = CreateSolidBrush(COLOR_KEY);
        if !key.is_null() {
            FillRect(dc, &raw const client, key);
            DeleteObject(key);
        }
        let black = CreateSolidBrush(CROSSHAIR_BLACK);
        let white = CreateSolidBrush(CROSSHAIR_WHITE);
        if !black.is_null() || !white.is_null() {
            for (dot, is_white) in selection_dots(client.right, client.bottom) {
                let brush = if is_white { white } else { black };
                if !brush.is_null() {
                    FillRect(dc, &raw const dot, brush);
                }
            }
        }
        if !black.is_null() {
            DeleteObject(black);
        }
        if !white.is_null() {
            DeleteObject(white);
        }
        EndPaint(window, state.as_ptr().cast());
    }
}

/// Two-tone dots along a `width` by `height` client area.
pub(super) fn selection_dots(width: i32, height: i32) -> Vec<(Rect, bool)> {
    let mut dots = Vec::new();
    if width <= 0 || height <= 0 {
        return dots;
    }
    let size = SELECTION_DOT.min(width).min(height);
    let mut x = 0;
    while x < width {
        for (offset, white) in [(0, true), (SELECTION_DOT + 1, false)] {
            let left = x + offset;
            if left >= width {
                break;
            }
            let right = (left + size).min(width);
            dots.push((
                Rect {
                    left,
                    top: 0,
                    right,
                    bottom: size,
                },
                white,
            ));
            dots.push((
                Rect {
                    left,
                    top: height - size,
                    right,
                    bottom: height,
                },
                white,
            ));
        }
        x += SELECTION_PITCH;
    }
    let mut y = 0;
    while y < height {
        for (offset, white) in [(0, true), (SELECTION_DOT + 1, false)] {
            let top = y + offset;
            if top >= height {
                break;
            }
            let bottom = (top + size).min(height);
            dots.push((
                Rect {
                    left: 0,
                    top,
                    right: size,
                    bottom,
                },
                white,
            ));
            dots.push((
                Rect {
                    left: width - size,
                    top,
                    right: width,
                    bottom,
                },
                white,
            ));
        }
        y += SELECTION_PITCH;
    }
    dots
}

/// UTF-16 string with a trailing nul for Win32 calls.
pub(super) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// This module's instance handle for window plumbing.
pub(super) fn module_handle() -> Hinstance {
    // SAFETY: null resolves this executable.
    unsafe { GetModuleHandleW(std::ptr::null()) }
}
