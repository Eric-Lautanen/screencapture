//! Display, monitor, and window geometry in physical pixels (DPI aware).

use std::ffi::c_void;
use std::io;

use crate::capture::Region;
use crate::winapi::{
    GetCursorPos, GetSystemMetrics, Point, Rect, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// One attached display: origin plus size in physical pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Display {
    /// Origin x (the primary display is at 0).
    pub x: i32,
    /// Origin y.
    pub y: i32,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

/// The display at the origin (0,0), else the first display found.
///
/// # Errors
/// Returns an error when no display is attached.
pub fn primary() -> io::Result<Display> {
    let mut list = displays();
    match list
        .iter()
        .position(|display| display.x == 0 && display.y == 0)
    {
        Some(index) => Ok(list.swap_remove(index)),
        None => list
            .pop()
            .ok_or_else(|| io::Error::other("no attached display found")),
    }
}

/// All attached displays. Coordinates are physical pixels because the
/// process opts into per-monitor DPI awareness (see `capture::prepare`).
#[must_use]
pub fn displays() -> Vec<Display> {
    let mut found = Vec::new();
    let parameter = &raw mut found;
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

type Lparam = isize;
type DisplayCallback =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *const Rect, *mut c_void) -> i32;

#[link(name = "user32")]
unsafe extern "system" {
    fn EnumDisplayMonitors(
        dc: *mut c_void,
        clip: *const Rect,
        callback: DisplayCallback,
        data: Lparam,
    ) -> i32;
    fn WindowFromPoint(point: Point) -> *mut c_void;
    fn GetAncestor(window: *mut c_void, flags: u32) -> *mut c_void;
    fn GetWindowRect(window: *mut c_void, rect: *mut Rect) -> i32;
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmGetWindowAttribute(
        window: *mut c_void,
        attribute: u32,
        value: *mut c_void,
        size: u32,
    ) -> i32;
}

/// `DWMWA_EXTENDED_FRAME_BOUNDS` (dwmapi.h): the visible window area
/// without borders and shadows.
const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;

/// `GA_ROOT` (winuser.h): the root owner of a window.
const GA_ROOT: u32 = 2;

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
/// shadows), clamped to the visible desktop. `preferred` names a window
/// the caller had focused just before a tray menu opened; otherwise the
/// window under the cursor is used.
///
/// # Errors
/// Returns an error when no window bounds can be resolved.
pub fn window_region(preferred: Option<usize>) -> Result<Region, io::Error> {
    let handle = match preferred {
        Some(value) => window_from_value(value),
        None => window_at_cursor().ok_or_else(|| io::Error::other("no window under the cursor"))?,
    };
    region_for_window(handle)
        .ok_or_else(|| io::Error::other("the target window has no usable bounds"))
}

fn window_from_value(value: usize) -> *mut c_void {
    // Callers stash Windows handles as plain numbers; rebuild the pointer
    // identity for the user32 calls below. The process published these
    // values itself moments ago, so exposure is fresh.
    std::ptr::with_exposed_provenance_mut::<c_void>(value)
}

/// The root owner of the window under the mouse pointer.
fn window_at_cursor() -> Option<*mut c_void> {
    let mut point = Point { x: 0, y: 0 };
    // SAFETY: writes only the local `point`.
    if unsafe { GetCursorPos(&raw mut point) } == 0 {
        return None;
    }
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
    visible_region(bounds)
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
    use super::{displays, primary};

    #[test]
    fn primary_works_or_fails_cleanly() {
        let all = displays();
        assert!((!all.is_empty() && primary().is_ok()) || (all.is_empty() && primary().is_err()));
        if let Ok(display) = primary() {
            assert!(display.width > 0 && display.height > 0);
            let is_origin = display.x == 0 && display.y == 0;
            let is_first = all.first() == Some(&display);
            assert!(is_origin || is_first);
        }
    }
}
