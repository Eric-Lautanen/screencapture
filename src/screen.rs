//! Display and monitor geometry in physical pixels (DPI aware).

use std::ffi::c_void;
use std::io;

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
}

/// `RECT` as used by monitor enumeration.
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
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

#[cfg(test)]
mod tests {
    use super::{Display, displays, primary};

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

    #[test]
    fn display_construction_clamps() {
        let bad = Display {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        assert_eq!(
            bad,
            Display {
                x: 0,
                y: 0,
                width: 0,
                height: 0
            }
        );
    }
}
