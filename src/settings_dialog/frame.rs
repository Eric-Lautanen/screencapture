//! The settings dialog's window frame: a fixed-size, modal-styled window
//! centered on the primary display, the exe's icon in its title bar, and
//! the dialog-wide `Segoe UI` font (`ClearType` quality) that every
//! child receives via `WM_SETFONT`.

use std::ffi::c_void;

use crate::winapi::{GetSystemMetrics, Hinstance, Hwnd};

use super::ffi::{
    AdjustWindowRectEx, CLEARTYPE_QUALITY, CreateFontIndirectW, DEFAULT_CHARSET, DEFAULT_DPI,
    DeleteObject, FW_NORMAL, GetDpiForSystem, ICON_RESOURCE_ID, IMAGE_ICON, LR_DEFAULTSIZE,
    LoadImageW, LogFontW, Rect, SM_CXSCREEN, SM_CYSCREEN, SendMessageW, UI_FONT_HEIGHT, WM_SETICON,
    WS_CAPTION, WS_EX_DLGMODALFRAME, WS_SYSMENU,
};

/// The designed client area: fits the grouped layout in `rows` with a
/// consistent margin and the button row at the bottom.
pub(super) const CLIENT_W: i32 = 490;
/// See [`CLIENT_W`].
pub(super) const CLIENT_H: i32 = 676;

/// The display's scale as an integer percentage (100 at 96 DPI). The
/// process is DPI aware for capture geometry, so the system will not
/// scale this window for us: every designed coordinate goes through
/// [`scaled`] instead. Integer math keeps text metrics exact: `-12` at
/// 150% becomes `-18`, not `-17`.
pub(super) fn dpi_scale() -> i32 {
    // SAFETY: parameterless metric query.
    let dpi = unsafe { GetDpiForSystem() };
    let dpi = if dpi == 0 { DEFAULT_DPI } else { dpi };
    let dpi = i32::try_from(dpi).unwrap_or(96);
    (dpi * 100 / 96).clamp(100, 400)
}

/// One designed pixel value at `percent` scale, rounded down after exact
/// integer multiplication (values stay small: the largest is a drop-down
/// extent, so overflow is not reachable).
pub(super) fn scaled(value: i32, percent: i32) -> i32 {
    let percent = percent.clamp(100, 400);
    value * percent / 100
}

/// Outer window geometry for the designed client area: the frame sizes
/// come from `AdjustWindowRectEx` (honest fallback: the client size
/// itself when the call refuses), and the window lands centered on the
/// primary display.
pub(super) fn window_frame(scale: i32) -> (i32, i32, i32, i32) {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: scaled(CLIENT_W, scale),
        bottom: scaled(CLIENT_H, scale),
    };
    // SAFETY: `rect` is a live local updated in place by the call.
    let adjusted = unsafe {
        AdjustWindowRectEx(
            &raw mut rect,
            WS_CAPTION | WS_SYSMENU,
            0,
            WS_EX_DLGMODALFRAME,
        )
    } != 0;
    let (width, height) = if adjusted {
        (rect.right - rect.left, rect.bottom - rect.top)
    } else {
        (scaled(CLIENT_W, scale), scaled(CLIENT_H, scale))
    };
    let (screen_w, screen_h) = primary_size();
    let (x, y) = centered_origin(width, height, screen_w, screen_h);
    (width, height, x, y)
}

/// The primary display's size in pixels (0x0 only if Windows refuses).
fn primary_size() -> (i32, i32) {
    // SAFETY: metric queries only.
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// The top-left origin that centers an `outer_w`-by-`outer_h` window on
/// a `screen_w`-by-`screen_h` display, clamped to remain on it.
fn centered_origin(outer_w: i32, outer_h: i32, screen_w: i32, screen_h: i32) -> (i32, i32) {
    let x = i32::max((screen_w - outer_w) / 2, 0);
    let y = i32::max((screen_h - outer_h) / 2, 0);
    (x, y)
}

/// The dialog-wide UI font: `Segoe UI`, `ClearType` quality, scaled with
/// the display, or `None` when GDI refuses (controls then keep their
/// default font).
pub(super) fn ui_font(scale: i32) -> Option<*mut c_void> {
    let mut face = [0u16; 32];
    for (slot, unit) in face.iter_mut().zip("Segoe UI".encode_utf16()) {
        *slot = unit;
    }
    let descriptor = LogFontW {
        height: scaled(UI_FONT_HEIGHT, scale),
        width: 0,
        escapement: 0,
        orientation: 0,
        weight: FW_NORMAL,
        italic: 0,
        underline: 0,
        strike_out: 0,
        char_set: DEFAULT_CHARSET,
        out_precision: 0,
        clip_precision: 0,
        quality: CLEARTYPE_QUALITY,
        pitch_and_family: 0,
        face_name: face,
    };
    // SAFETY: the descriptor pointer lives through the call; the handle
    // is owned by the dialog and deleted in `run`.
    let font = unsafe { CreateFontIndirectW(&raw const descriptor) };
    (!font.is_null()).then_some(font)
}

/// Dispose a font this module created (paired with `ui_font`).
pub(super) fn drop_font(font: *mut c_void) {
    // SAFETY: the handle came from `CreateFontIndirectW` in this module.
    unsafe {
        DeleteObject(font);
    }
}

/// Put the exe's baked-in icon (`assets/icon.rc` resource 1) in the
/// title bar; best-effort - a refusal simply leaves the default icon.
pub(super) fn install_title_icon(window: Hwnd, instance: Hinstance) {
    // SAFETY: the resource id is a plain ordinal cast to the wide-name
    // pointer Win32's `MAKEINTRESOURCEW` produces.
    let icon = unsafe {
        LoadImageW(
            instance,
            ICON_RESOURCE_ID as *const u16,
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE,
        )
    };
    if icon.is_null() {
        return;
    }
    // SAFETY: a live dialog window and an owned icon handle.
    unsafe {
        SendMessageW(window, WM_SETICON, 0, icon as isize);
    }
}

#[cfg(test)]
mod tests {
    use super::{CLIENT_H, CLIENT_W, centered_origin};

    #[test]
    fn designed_pixels_scale_with_the_display() {
        use super::{dpi_scale, scaled};

        assert_eq!(scaled(100, 100), 100);
        assert_eq!(scaled(100, 150), 150);
        assert_eq!(scaled(100, 200), 200);
        assert_eq!(scaled(-12, 150), -18);
        // A bad scale must not collapse the layout.
        assert_eq!(scaled(100, 0), 100);
        assert!(dpi_scale() >= 100);
    }

    #[test]
    fn origin_centers_on_the_primary_display() {
        let (x, y) = centered_origin(100, 60, 1920, 1200);
        assert_eq!((x, y), (910, 570));
    }

    #[test]
    fn oversized_windows_clamp_to_the_display_origin() {
        let (x, y) = centered_origin(4000, 2000, 1920, 1200);
        assert_eq!((x, y), (0, 0));
    }

    #[test]
    fn client_area_fits_a_720p_display() {
        const {
            assert!(CLIENT_W < 1280 && CLIENT_H < 720 - 40);
        }
    }
}
