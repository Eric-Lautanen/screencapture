//! The recording border's Stop chip: a small non-activating topmost window
//! centered on the top edge of the recording frame.

use std::cell::Cell;

use crate::capture::Region;
use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, Hcursor, Hdc, Hinstance, Hwnd, Lparam, Lresult,
    Rect, RegisterClassExW, WndClassExW, Wparam,
};

use super::{
    BeginPaint, CLEARTYPE_QUALITY, CreateFontW, CreateSolidBrush, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FRAME_RED, FillRect, FrameRect, GetClientRect,
    HWND_TOPMOST, InvalidateRect, LoadCursorW, SWP_NOACTIVATE, SWP_SHOWWINDOW, SelectObject,
    SetBkMode, SetTextColor, SetWindowDisplayAffinity, SetWindowPos, StopAction, TRANSPARENT,
    UpdateWindow, WDA_EXCLUDEFROMCAPTURE, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_PAINT,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE, module_handle, wide,
};

const STOP_CLASS: &str = "screencapture.recording.stop";
pub(super) const STOP_WIDTH: i32 = 88;
pub(super) const STOP_HEIGHT: i32 = 28;
const STOP_WHITE: u32 = 0x00FF_FFFF;

thread_local! {
    /// What the live Stop button asks for. A plain function pointer (not
    /// a closure) keeps the window procedure free of captured state; the
    /// action posts a tray command and returns, so the click never tears
    /// down the window it is dispatching through.
    static ON_STOP: Cell<Option<StopAction>> = const { Cell::new(None) };
}

/// Tear down the Stop button window and disarm its callback.
pub(super) unsafe fn destroy_stop(stop: Hwnd) {
    ON_STOP.with(|slot| slot.set(None));
    // SAFETY: upheld by caller (`Border::drop`).
    unsafe {
        DestroyWindow(stop);
    }
}

/// The Stop button's frame for a capture region: centered on the top
/// dashed edge, pushed just inside the region when that edge sits at the
/// top of the screen (where half the button would fall off it).
pub(super) fn stop_rect(region: Region) -> Rect {
    let width = i32::try_from(region.width).unwrap_or(0);
    let x = region.x + (width - STOP_WIDTH).max(0) / 2;
    let lifted = region.y - STOP_HEIGHT / 2;
    let y = if lifted < 0 { region.y } else { lifted };
    Rect {
        left: x,
        top: y,
        right: x + STOP_WIDTH,
        bottom: y + STOP_HEIGHT,
    }
}

/// Create the button window and arm the stop action; `None` when Windows
/// refuses the window (the outline still shows, Escape and the hotkey
/// still stop).
pub(super) fn show_stop_button(region: Region, action: StopAction) -> Option<Hwnd> {
    let instance = module_handle();
    register_stop_class(instance);
    let class = wide(STOP_CLASS);
    let rect = stop_rect(region);
    // SAFETY: strings and handles outlive the call; the window is owned by
    // this process and destroyed by the `Border` guard.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP | WS_VISIBLE,
            rect.left,
            rect.top,
            STOP_WIDTH,
            STOP_HEIGHT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        )
    };
    if window.is_null() {
        return None;
    }
    ON_STOP.with(|slot| slot.set(Some(action)));
    // SAFETY: `window` was created above on this thread.
    unsafe {
        // Keep the button out of the recording it stops; older Windows
        // builds refuse and the button then appears in the shot, which is
        // a cosmetic loss, not a failure.
        let _ = SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE);
        SetWindowPos(
            window,
            HWND_TOPMOST,
            rect.left,
            rect.top,
            STOP_WIDTH,
            STOP_HEIGHT,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        InvalidateRect(window, std::ptr::null(), 1);
        UpdateWindow(window);
    }
    Some(window)
}

/// Register the Stop button class; repeats answer "already registered".
fn register_stop_class(instance: Hinstance) {
    let name = wide(STOP_CLASS);
    let class = WndClassExW {
        size: u32::try_from(std::mem::size_of::<WndClassExW>()).expect("class size fits u32"),
        style: 0,
        wnd_proc: stop_procedure,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: std::ptr::null_mut(),
        cursor: stop_cursor(),
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

/// The chip's hover cursor: the hand pointer that marks a control as
/// clickable. A class cursor is also what keeps the cursor *stable*: the
/// button appears while the pump thread is still starting the recorder,
/// so Windows paints its "app starting" spinner over the fresh window,
/// and a class with no cursor would leave that spinner in place.
fn stop_cursor() -> Hcursor {
    // SAFETY: null instance resolves shared stock cursor resources.
    unsafe {
        let hand = LoadCursorW(std::ptr::null_mut(), crate::winapi::IDC_HAND as *const u16);
        if hand.is_null() {
            LoadCursorW(std::ptr::null_mut(), crate::winapi::IDC_ARROW as *const u16)
        } else {
            hand
        }
    }
}

/// The Stop button's window procedure: paint the red chip, and turn a
/// left click into the armed stop action. Presses are swallowed so the
/// application below never sees a click meant for the button.
unsafe extern "system" fn stop_procedure(
    window: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    match message {
        WM_PAINT => {
            // SAFETY: the paint state buffer fits the platform PAINTSTRUCT.
            unsafe { paint_stop(window) };
            0
        }
        WM_ERASEBKGND | WM_LBUTTONDOWN => 1,
        WM_LBUTTONUP => {
            if let Some(action) = ON_STOP.with(Cell::get) {
                action();
            }
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Paint the button: a red chip with a white rim and a centered `Stop`.
///
/// # Safety
/// `window` must be a live window owned by the calling thread.
unsafe fn paint_stop(window: Hwnd) {
    let mut state = [0u8; 128];
    // SAFETY: `state` is at least `sizeof(PAINTSTRUCT)` for this ABI.
    let dc = unsafe { BeginPaint(window, state.as_mut_ptr().cast()) };
    if dc.is_null() {
        return;
    }
    let mut client = Rect::default();
    // SAFETY: writes only the local `client`.
    unsafe { GetClientRect(window, &raw mut client) };
    // SAFETY: fresh brushes and fonts are released after use; all rects
    // passed are local.
    unsafe {
        let red = CreateSolidBrush(FRAME_RED);
        if !red.is_null() {
            FillRect(dc, &raw const client, red);
            DeleteObject(red);
        }
        let white = CreateSolidBrush(STOP_WHITE);
        if !white.is_null() {
            FrameRect(dc, &raw const client, white);
            DeleteObject(white);
        }
        draw_stop_label(dc, client);
        EndPaint(window, state.as_ptr().cast());
    }
}

/// Draw `Stop` centered in `client` with a small bold face.
///
/// # Safety
/// `dc` must be a live device context; the created font is released here.
unsafe fn draw_stop_label(dc: Hdc, client: Rect) {
    let face = wide("Segoe UI");
    // SAFETY: `face` outlives the call; the font is selected only below.
    let font = unsafe {
        CreateFontW(
            -15,
            0,
            0,
            0,
            700,
            0,
            0,
            0,
            1,
            0,
            0,
            CLEARTYPE_QUALITY,
            0,
            face.as_ptr(),
        )
    };
    let previous = if font.is_null() {
        std::ptr::null_mut()
    } else {
        // SAFETY: live DC and a font we just created.
        unsafe { SelectObject(dc, font) }
    };
    let text = wide("Stop");
    let mut area = client;
    // SAFETY: the text pointer is null-terminated; `area` is local.
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, STOP_WHITE);
        DrawTextW(
            dc,
            text.as_ptr(),
            -1,
            &raw mut area,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    if !font.is_null() {
        // SAFETY: restore the old font before deleting ours.
        unsafe {
            SelectObject(dc, previous);
            DeleteObject(font);
        }
    }
}
