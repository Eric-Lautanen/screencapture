//! The picker's crosshair: a tiny topmost, click-through, layered window
//! that paints a crosshair around the pointer. No system-wide cursor
//! state is changed, so ending (or killing) the picker can never leave
//! another application's pointer stuck as a crosshair.

use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, Hinstance, Hwnd, Lparam, Lresult, Point, Rect,
    RegisterClassExW, WndClassExW, Wparam,
};

use super::{
    BeginPaint, COLOR_KEY, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetClientRect,
    HWND_TOPMOST, InvalidateRect, LWA_COLORKEY, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos, UpdateWindow,
    WDA_EXCLUDEFROMCAPTURE, WM_ERASEBKGND, WM_PAINT, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE, kept_wide,
    module_handle, wide,
};

const CROSSHAIR_CLASS: &str = "screencapture.picker.crosshair";
const CROSSHAIR_SIZE: i32 = 33;
/// Clear space around the pointer so the real cursor stays visible in the
/// middle of the drawn crosshair.
const CROSSHAIR_GAP: i32 = 5;
/// Black outline thickness; a white line runs through the middle so the
/// marker reads on both light and dark content.
const CROSSHAIR_THICK: i32 = 3;
pub(super) const CROSSHAIR_WHITE: u32 = 0x00FF_FFFF;
pub(super) const CROSSHAIR_BLACK: u32 = 0x0000_0000;

/// The picker's crosshair window.
pub(crate) struct Crosshair {
    window: Hwnd,
}

impl Crosshair {
    /// Create the crosshair centered on `point`; `None` when Windows
    /// refuses the window (the pick itself still works).
    pub(crate) fn show(point: Point) -> Option<Self> {
        let instance = module_handle();
        register_crosshair_class(instance);
        let class = wide(CROSSHAIR_CLASS);
        let style_ex =
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE;
        // SAFETY: strings and handles outlive the call; the window is
        // owned by this process and destroyed in `Drop`.
        let window = unsafe {
            CreateWindowExW(
                style_ex,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP | WS_VISIBLE,
                point.x - CROSSHAIR_SIZE / 2,
                point.y - CROSSHAIR_SIZE / 2,
                CROSSHAIR_SIZE,
                CROSSHAIR_SIZE,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null_mut(),
            )
        };
        if window.is_null() {
            return None;
        }
        // SAFETY: `window` was created above and belongs to this thread.
        unsafe {
            SetLayeredWindowAttributes(window, COLOR_KEY, 0, LWA_COLORKEY);
            // Keep the marker out of the eventual capture; older Windows
            // builds refuse and the marker is merely cosmetic there.
            let _ = SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE);
        }
        let crosshair = Self { window };
        crosshair.place(point);
        // `UpdateWindow` paints the marker before the picker's pump starts;
        // a layered window otherwise stays transparent until the first
        // queued `WM_PAINT` is dispatched.
        // SAFETY: the window this guard created.
        unsafe {
            InvalidateRect(window, std::ptr::null(), 1);
            UpdateWindow(window);
        }
        Some(crosshair)
    }

    /// Center the crosshair on `point`; no-op if Windows refuses.
    pub(crate) fn place(&self, point: Point) {
        // SAFETY: the window is this guard's live topmost marker.
        unsafe {
            SetWindowPos(
                self.window,
                HWND_TOPMOST,
                point.x - CROSSHAIR_SIZE / 2,
                point.y - CROSSHAIR_SIZE / 2,
                CROSSHAIR_SIZE,
                CROSSHAIR_SIZE,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }
}

impl Drop for Crosshair {
    fn drop(&mut self) {
        // SAFETY: the window this guard created.
        unsafe {
            DestroyWindow(self.window);
        }
    }
}

/// Paint the colour-key background and the crosshair arms.
unsafe extern "system" fn crosshair_procedure(
    window: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    match message {
        WM_PAINT => {
            // SAFETY: the paint state buffer is large enough for the
            // platform `PAINTSTRUCT`.
            unsafe { paint_crosshair(window) };
            0
        }
        WM_ERASEBKGND => 1,
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// # Safety
/// `window` must be a live window owned by the calling thread.
unsafe fn paint_crosshair(window: Hwnd) {
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
        if !black.is_null() {
            for rect in crosshair_segments(CROSSHAIR_SIZE, CROSSHAIR_GAP, CROSSHAIR_THICK) {
                FillRect(dc, &raw const rect, black);
            }
        }
        if !white.is_null() {
            for rect in crosshair_segments(CROSSHAIR_SIZE, CROSSHAIR_GAP, 1) {
                FillRect(dc, &raw const rect, white);
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

/// Four arms around a clear center: `(left, top, right, bottom)` in the
/// marker's client coordinates.
pub(super) fn crosshair_segments(size: i32, gap: i32, thickness: i32) -> Vec<Rect> {
    let mut arms = Vec::with_capacity(4);
    if size <= 0 || thickness <= 0 {
        return arms;
    }
    let mid = size / 2;
    let half = (thickness / 2).max(0);
    let inner = (mid - gap).max(1);
    let outer = (mid + gap).min(size - 1);
    let a = (mid - half).clamp(0, size);
    let b = (mid + half + 1).clamp(0, size);
    arms.push(Rect {
        left: 0,
        top: a,
        right: inner,
        bottom: b,
    });
    arms.push(Rect {
        left: outer,
        top: a,
        right: size,
        bottom: b,
    });
    arms.push(Rect {
        left: a,
        top: 0,
        right: b,
        bottom: inner,
    });
    arms.push(Rect {
        left: a,
        top: outer,
        right: b,
        bottom: size,
    });
    arms
}

/// Register the crosshair class; repeats answer "already registered".
///
/// The name buffer stays alive for the process lifetime: the class table
/// keeps the pointer, not a copy, so a local buffer would dangle.
fn register_crosshair_class(instance: Hinstance) {
    let name = kept_wide(CROSSHAIR_CLASS);
    let class = WndClassExW {
        size: u32::try_from(std::mem::size_of::<WndClassExW>()).expect("class size fits u32"),
        style: 0,
        wnd_proc: crosshair_procedure,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: std::ptr::null_mut(),
        cursor: std::ptr::null_mut(),
        background: std::ptr::null_mut(),
        menu_name: std::ptr::null(),
        class_name: name,
        icon_small: std::ptr::null_mut(),
    };
    // SAFETY: the class struct is a fully initialized local and the name
    // buffer outlives the registration.
    unsafe {
        RegisterClassExW(&raw const class);
    }
}
