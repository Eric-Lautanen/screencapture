//! The settings dialog's window frame: a fixed-size, modal-styled window
//! centered on the primary display, the exe's icon in its title bar, and
//! the dialog-wide `Segoe UI` font (`ClearType` quality) that every
//! child receives via `WM_SETFONT`.

use std::ffi::c_void;

use crate::winapi::{GetSystemMetrics, Hicon, Hinstance, Hwnd};

use super::ffi::{
    AdjustWindowRectEx, CLEARTYPE_QUALITY, CreateFontIndirectW, DEFAULT_CHARSET, DEFAULT_DPI,
    DeleteObject, DestroyIcon, FW_NORMAL, GetDpiForSystem, ICON_RESOURCE_ID, IMAGE_ICON,
    LR_DEFAULTSIZE, LoadImageW, LogFontW, Rect, SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA,
    SendMessageW, SystemParametersInfoW, UI_FONT_HEIGHT, WM_SETICON, WS_CAPTION,
    WS_EX_DLGMODALFRAME, WS_SYSMENU,
};

/// The designed client area: the two grouped columns in `rows` side by
/// side, the full-width bands beneath them, and the button row at the
/// bottom. Wide and short on purpose: a settings sheet that fits a 1366x768
/// laptop at 125% instead of shrinking the text.
pub(super) const CLIENT_W: i32 = 728;
/// Fitted from the footer's own row in `rows`, so the window always ends one
/// margin below the buttons it holds.
pub(super) const CLIENT_H: i32 =
    super::rows::BUTTON_Y + super::rows::BUTTON_H + super::rows::FOOTER_MARGIN;

/// Smallest layout percent the dialog shrinks to: below this the labels
/// stop being readable, so a display that still cannot fit the dialog
/// gets a clipped window rather than unreadable controls.
pub(super) const MIN_SCALE: i32 = 55;
/// Breathing room left around the dialog inside the work area.
const EDGE_MARGIN: i32 = 12;

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
    let percent = percent.clamp(MIN_SCALE, 400);
    value * percent / 100
}

/// The layout percent the dialog actually uses: the display's DPI scale,
/// reduced until the whole window — caption included — fits the work area
/// (the desktop minus the taskbar). A 1366x768 laptop at 150% would
/// otherwise draw a 786-pixel client area and put Save below the screen.
pub(super) fn fit_scale(scale: i32) -> i32 {
    let (_, _, work_w, work_h) = work_area();
    let (chrome_w, chrome_h) = frame_extents();
    fit_percent(work_w, work_h, chrome_w, chrome_h, scale)
}

/// The layout percent for a design area, a work area, and the window's
/// non-client extents; `scale` is the display's DPI percent. Width or
/// height can limit the answer on very small displays.
fn fit_percent(work_w: i32, work_h: i32, chrome_w: i32, chrome_h: i32, scale: i32) -> i32 {
    let by_width = percent_that_fits(CLIENT_W, work_w - chrome_w - EDGE_MARGIN);
    let by_height = percent_that_fits(CLIENT_H, work_h - chrome_h - EDGE_MARGIN);
    scale.min(by_width).min(by_height).clamp(MIN_SCALE, 400)
}

/// The largest whole percent whose scaled design still fits `available`
/// pixels; values below 100 are the shrink a small display needs.
fn percent_that_fits(design: i32, available: i32) -> i32 {
    available * 100 / design
}

/// The non-client extents (caption and borders) in physical pixels.
fn frame_extents() -> (i32, i32) {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
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
    if adjusted {
        (rect.right - rect.left, rect.bottom - rect.top)
    } else {
        // Honest fallback: a caption's height at 96 DPI plus a border.
        (16, 39)
    }
}

/// The primary display's work area: origin plus size, taskbar excluded.
fn work_area() -> (i32, i32, i32, i32) {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `rect` is a live local filled by the call.
    let read = unsafe {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, std::ptr::from_mut(&mut rect).cast(), 0)
    } != 0;
    if read && rect.right > rect.left && rect.bottom > rect.top {
        (
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
        )
    } else {
        let (width, height) = primary_size();
        (0, 0, width, height)
    }
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
    let (work_x, work_y, work_w, work_h) = work_area();
    let (x, y) = centered_origin(width, height, work_w, work_h);
    (width, height, work_x + x, work_y + y)
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
///
/// Answers the handle it loaded, which the caller still owns: `WM_SETICON`
/// borrows the icon for as long as the window shows it and never takes
/// ownership, so every open of the dialog would otherwise leak one.
pub(super) fn install_title_icon(window: Hwnd, instance: Hinstance) -> Option<Hicon> {
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
        return None;
    }
    // SAFETY: a live dialog window and an owned icon handle.
    unsafe {
        SendMessageW(window, WM_SETICON, 0, icon as isize);
    }
    Some(icon)
}

/// Release the title-bar icon [`install_title_icon`] loaded.
pub(super) fn drop_icon(icon: Hicon) {
    // SAFETY: the handle came from `LoadImageW` above and was only lent to
    // the window; nothing still needs it once that window is gone.
    unsafe {
        DestroyIcon(icon);
    }
}

#[cfg(test)]
mod tests {
    use super::{CLIENT_H, CLIENT_W, centered_origin};

    #[test]
    fn designed_pixels_scale_with_the_display() {
        use super::{MIN_SCALE, dpi_scale, scaled};

        assert_eq!(scaled(100, 100), 100);
        assert_eq!(scaled(100, 150), 150);
        assert_eq!(scaled(100, 200), 200);
        assert_eq!(scaled(-12, 150), -18);
        // A bad scale must not collapse the layout, and a shrunk layout
        // stops at the readable floor.
        assert_eq!(scaled(100, 0), MIN_SCALE);
        assert_eq!(scaled(100, 30), 55);
        assert!(dpi_scale() >= 100);
    }

    /// The fit math: a full-size desktop is untouched, a short one is
    /// shrunk by exactly the ratio its height allows.
    #[test]
    fn the_layout_shrinks_to_the_available_height() {
        use super::{CLIENT_H, CLIENT_W, percent_that_fits};

        assert_eq!(percent_that_fits(CLIENT_H, 900), 171);
        // A 768-high display minus a taskbar and the frame leaves ~690:
        // still enough for the sheet at 100% and then some.
        assert_eq!(percent_that_fits(CLIENT_H, 690), 131);
        // A 600-high netbook cannot show the design at full scale.
        assert_eq!(percent_that_fits(CLIENT_H, 540), 103);
        assert_eq!(percent_that_fits(CLIENT_W, 780), 107);
    }

    /// Whatever the display reports, the dialog's own scale answer stays
    /// inside the readable range.
    #[test]
    fn fit_scale_stays_readable() {
        use super::{MIN_SCALE, fit_scale};

        for scale in [100, 125, 150, 200, 400] {
            let fitted = fit_scale(scale);
            assert!((MIN_SCALE..=scale).contains(&fitted), "{scale} -> {fitted}");
        }
    }

    /// Real displays: the short design keeps its DPI scale on an ordinary
    /// laptop and a high-DPI desktop; only a small netbook is shrunk.
    #[test]
    fn small_displays_shrink_the_layout_to_fit() {
        use super::fit_percent;

        // 1366x768 at 125%: 728-high work area, 39-pixel caption. The
        // sheet keeps its full scale now that the columns are balanced.
        assert_eq!(fit_percent(1366, 728, 16, 39, 125), 125);
        // 1024x600 at 125%: the height only fits shrunk.
        assert_eq!(fit_percent(1024, 560, 16, 39, 125), 97);
        // 1920x1080 at 150%: nothing to trim either way.
        assert_eq!(fit_percent(1920, 1032, 16, 39, 150), 150);
        // 2560x1440 at 100%: nothing to do.
        assert_eq!(fit_percent(2560, 1400, 16, 39, 100), 100);
        // A 720p television: still full scale.
        assert_eq!(fit_percent(1280, 680, 16, 39, 100), 100);
        // A tiny display shrinks by its height; an absurdly small one is
        // clipped at the readable floor rather than drawn unreadably.
        assert_eq!(fit_percent(800, 520, 16, 39, 100), 89);
        assert_eq!(fit_percent(640, 320, 16, 39, 100), 55);
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
