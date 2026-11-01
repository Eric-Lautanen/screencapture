//! Windows geometry: `EnumDisplayMonitors` listing plus a DWM-aware
//! window picker. Coordinates are physical pixels because the process
//! opts into per-monitor DPI awareness (see `capture::prepare`).

use std::ffi::c_void;
use std::io;

use crate::capture::Region;
use crate::screen::Display;
use crate::winapi::{
    DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_VISIBLE_FRAME_BORDER_THICKNESS, DwmGetWindowAttribute,
    EnumDisplayMonitors, GA_ROOT, GetAncestor, GetClassNameW, GetCursorPos, GetSystemMetrics,
    GetWindowRect, Hwnd, Point, Rect, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, WindowFromPoint,
};

/// All attached displays in enumeration order.
#[must_use]
pub fn list() -> Vec<Display> {
    let mut found = Vec::new();
    let parameter = std::ptr::from_mut(&mut found);
    // SAFETY: `parameter` points to a live local `Vec`; the callback runs
    // synchronously inside this call on this thread.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            push_display,
            parameter as isize,
        );
    }
    found
}

/// Append one monitor rectangle; nonzero keeps enumeration going.
unsafe extern "system" fn push_display(
    _dc: *mut c_void,
    _monitor: *mut c_void,
    rect: *const Rect,
    data: *mut c_void,
) -> i32 {
    if rect.is_null() {
        return 0;
    }
    let boundary = unsafe { rect.read_unaligned() };
    let displays = unsafe { &mut *data.cast::<Vec<Display>>() };
    let width = u32::try_from(i64::from(boundary.right) - i64::from(boundary.left)).unwrap_or(0);
    let height = u32::try_from(i64::from(boundary.bottom) - i64::from(boundary.top)).unwrap_or(0);
    displays.push(Display {
        x: boundary.left,
        y: boundary.top,
        width,
        height,
    });
    1
}

/// A window's capture region: DWM extended frame bounds (no borders or
/// shadows), clamped to the visible desktop. `preferred` — when given —
/// names the window the picker session chose (pointer bits); otherwise
/// the window under the cursor answers.
///
/// A pick on the desktop answers the display under the pointer instead:
/// the desktop window is the wallpaper layer, so its own bounds are not
/// the screen the user is pointing at.
///
/// # Errors
/// Returns an error when no window bounds can be resolved.
pub fn window_region(preferred: Option<usize>) -> Result<Region, io::Error> {
    let handle = match preferred {
        Some(value) => window_from_value(value),
        None => window_at_cursor().ok_or_else(|| io::Error::other("no window under the cursor"))?,
    };
    if is_desktop(handle) {
        let display = super::under_cursor();
        return Ok(Region {
            x: display.x,
            y: display.y,
            width: display.width,
            height: display.height,
        });
    }
    region_for_window(handle)
        .ok_or_else(|| io::Error::other("the target window has no usable bounds"))
}

/// Window classes of the shell's desktop: `Progman` normally, `WorkerW`
/// when a slideshow moves the icon host up one level, and `#32769` for
/// the desktop window those two live in. All three are the wallpaper
/// layer every application sits on.
const DESKTOP_CLASSES: [&str; 3] = ["Progman", "WorkerW", "#32769"];

/// Whether the picker's `handle` names the shell's desktop window rather
/// than an application's own.
pub(crate) fn is_desktop_window(handle: usize) -> bool {
    let window = window_from_value(handle);
    // The picker hands over the root of whatever was under the pointer; a
    // child that reached here (the icon list) is still the desktop.
    // SAFETY: a root walk from a handle this process already published.
    let root = unsafe { GetAncestor(window, GA_ROOT) };
    is_desktop(if root.is_null() { window } else { root })
}

/// Whether `window` is one of the desktop classes above.
fn is_desktop(window: Hwnd) -> bool {
    if window.is_null() {
        return false;
    }
    let mut name = [0u16; 64];
    let size = i32::try_from(name.len()).unwrap_or(0);
    // SAFETY: the buffer is a live local sized by the count passed, and
    // the call only writes into it.
    let len = unsafe { GetClassNameW(window, name.as_mut_ptr(), size) };
    if len <= 0 {
        return false;
    }
    let len = usize::try_from(len).unwrap_or(0).min(name.len());
    desktop_class(&crate::winapi::wide_to_string(&name[..len]))
}

/// Whether a window's class name is the shell's desktop.
fn desktop_class(name: &str) -> bool {
    DESKTOP_CLASSES.contains(&name)
}

fn window_from_value(value: usize) -> *mut c_void {
    // Callers stash Windows handles as plain numbers; rebuild the pointer
    // identity for the user32 calls below. The process published these
    // values itself moments ago, so exposure is fresh.
    std::ptr::with_exposed_provenance_mut::<c_void>(value)
}

/// Where the mouse pointer is, in desktop coordinates.
pub(crate) fn cursor_position() -> Option<Point> {
    let mut point = Point { x: 0, y: 0 };
    // SAFETY: writes only the local `point`.
    if unsafe { GetCursorPos(&raw mut point) } == 0 {
        return None;
    }
    Some(point)
}

/// The root owner of the window under the mouse pointer.
fn window_at_cursor() -> Option<*mut c_void> {
    let point = cursor_position()?;
    // SAFETY: a point lookup and its root walk; the handles returned are
    // shared windows used only for inspection below.
    unsafe {
        let window = WindowFromPoint(point);
        if window.is_null() {
            return None;
        }
        let root = GetAncestor(window, GA_ROOT);
        (!root.is_null()).then_some(root)
    }
}

/// DWM frame bounds when decoration-aware bounds work, else raw bounds.
fn region_for_window(handle: *mut c_void) -> Option<Region> {
    visible_region(window_bounds(handle)?)
}

/// One window's opaque frame: DWM extended bounds without borders and
/// shadows, with the visible frame border trimmed. Windows 11 draws that
/// 1 px border translucently over the desktop, so keeping it in the rect
/// shows whatever sits behind the window along its edges (white text or
/// an image slice when a bright page is behind). Both the capture region
/// and the picker's highlight use this, so the dotted box hugs exactly
/// the pixels a capture saves.
///
/// Falls back to `GetWindowRect` when decoration introspection refuses.
pub(crate) fn window_bounds(handle: Hwnd) -> Option<Rect> {
    if handle.is_null() {
        return None;
    }
    let mut bounds = Rect::default();
    let size = u32::try_from(std::mem::size_of::<Rect>()).unwrap_or(0);
    // SAFETY: `bounds` is a live local rect shared with the call.
    let dwm_ok = unsafe {
        DwmGetWindowAttribute(
            handle,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut bounds).cast::<c_void>(),
            size,
        )
    } == 0;
    if !dwm_ok {
        // SAFETY: writes only the local `bounds`.
        if unsafe { GetWindowRect(handle, &raw mut bounds) } == 0 {
            return None;
        }
    }
    trim_frame_border(handle, bounds)
}

/// Inset `bounds` past the visible frame border. The attribute exists on
/// Windows 11; older builds fail and keep the bounds untouched. The
/// thickness is tiny by contract, and the guard keeps a degenerate
/// answer from emptying a legitimate window.
fn trim_frame_border(handle: Hwnd, bounds: Rect) -> Option<Rect> {
    let mut thickness = 0u32;
    let size = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(0);
    // SAFETY: writes only the local `thickness`.
    let found = unsafe {
        DwmGetWindowAttribute(
            handle,
            DWMWA_VISIBLE_FRAME_BORDER_THICKNESS,
            (&raw mut thickness).cast::<c_void>(),
            size,
        )
    } == 0;
    let border = i32::try_from(thickness).unwrap_or(0).clamp(0, 4);
    if found {
        inset(bounds, border).or_else(|| nonempty(bounds))
    } else {
        nonempty(bounds)
    }
}

/// `bounds` moved in by `border` on every side; `None` when that empties
/// it (a nonsense thickness must not erase a legitimate window).
fn inset(bounds: Rect, border: i32) -> Option<Rect> {
    if border <= 0 {
        return nonempty(bounds);
    }
    let inset = Rect {
        left: bounds.left.saturating_add(border),
        top: bounds.top.saturating_add(border),
        right: bounds.right.saturating_sub(border),
        bottom: bounds.bottom.saturating_sub(border),
    };
    nonempty(inset)
}

fn nonempty(bounds: Rect) -> Option<Rect> {
    (bounds.right > bounds.left && bounds.bottom > bounds.top).then_some(bounds)
}

/// A rect's on-screen capture region, limited to the visible desktop so
/// frame grabs never ask for pixels a monitor does not own.
fn visible_region(bounds: Rect) -> Option<Region> {
    let (x0, y0, x1, y1) = virtual_desktop();
    let left = bounds.left.max(x0);
    let top = bounds.top.max(y0);
    let right = bounds.right.min(x1);
    let bottom = bounds.bottom.min(y1);
    if right <= left || bottom <= top {
        return None;
    }
    Some(Region {
        x: left,
        y: top,
        width: u32::try_from(i64::from(right) - i64::from(left)).ok()?,
        height: u32::try_from(i64::from(bottom) - i64::from(top)).ok()?,
    })
}

/// Virtual desktop edges: origin, plus the bottom-right corner of the
/// bounding rect that contains every monitor.
fn virtual_desktop() -> (i32, i32, i32, i32) {
    // SAFETY: read-only system metrics with no side effects.
    let (left, top) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        )
    };
    // SAFETY: read-only system metrics with no side effects.
    let (width, height) = unsafe {
        (
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    (left, top, left + width, top + height)
}

#[cfg(test)]
mod tests {
    use super::{desktop_class, inset, nonempty, window_region};
    use crate::winapi::{Hwnd, Rect};

    #[link(name = "user32")]
    unsafe extern "system" {
        /// The shell's desktop window; null when no shell is running.
        fn GetShellWindow() -> Hwnd;
    }

    /// The live desktop window, or `None` on a desktop with no shell.
    fn shell_desktop() -> Option<Hwnd> {
        // SAFETY: a parameterless query about the current shell.
        let window = unsafe { GetShellWindow() };
        (!window.is_null()).then_some(window)
    }

    const FRAME: Rect = Rect {
        left: 240,
        top: 126,
        right: 1680,
        bottom: 1026,
    };

    #[test]
    fn zero_border_keeps_the_frame() {
        assert!(inset(FRAME, 0) == Some(FRAME));
    }

    #[test]
    fn one_pixel_border_trims_every_side() {
        let trimmed = inset(FRAME, 1).expect("a trimmed frame stays nonempty");
        assert_eq!(
            (trimmed.left, trimmed.top, trimmed.right, trimmed.bottom),
            (241, 127, 1679, 1025)
        );
    }

    #[test]
    fn nonsense_border_never_erases_the_frame() {
        assert!(inset(FRAME, 5_000).is_none());
        assert!(inset(Rect::default(), 1).is_none());
        assert!(nonempty(Rect::default()).is_none());
    }

    /// The wallpaper layer answers the screen, never its own paint: a
    /// capture of the desktop window saved icons on an empty background
    /// with no taskbar and no application windows in the frame.
    #[test]
    fn only_the_shells_desktop_classes_are_the_desktop() {
        assert!(desktop_class("Progman"));
        assert!(desktop_class("WorkerW"));
        assert!(desktop_class("#32769"));
        for other in ["Shell_TrayWnd", "Notepad", "Chrome_WidgetWin_1", ""] {
            assert!(!desktop_class(other), "{other}");
        }
    }

    /// Against the live desktop: the picker's own answer for this window
    /// is the display it sits on, not the 1 px-inset desktop frame.
    #[test]
    fn the_desktop_window_answers_the_display() {
        let Some(handle) = shell_desktop() else {
            return; // no shell desktop in this environment
        };
        assert!(super::is_desktop_window(handle as usize));
        let region = window_region(Some(handle as usize)).expect("the desktop is recordable");
        let display = crate::screen::under_cursor();
        assert_eq!(
            (region.x, region.y, region.width, region.height),
            (display.x, display.y, display.width, display.height)
        );
    }
}
