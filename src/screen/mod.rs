//! Display, monitor, and window geometry. Each platform module reports
//! its displays in physical-pixel semantics (Windows enumerate monitors,
//! macOS answers the compositor's main display, Linux the default screen
//! root), and window picking answers per platform: Windows DWM bounds,
//! Linux the X client rectangle under the cursor, macOS the backend
//! window via the compositor's window list.

use std::io;

use crate::capture::Region;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
pub(crate) mod win;

/// One attached display: origin plus size in physical pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Display {
    /// Origin x (the primary display is at 0).
    pub x: i32,
    /// Origin y.
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
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
            .ok_or_else(|| io::Error::other("no display is attached")),
    }
}

/// One display by 1-based enumeration order (matches the tray's
/// "Display N" entries).
///
/// # Errors
/// Returns an error when the number is not currently attached.
pub fn display_at(number: u32) -> io::Result<Display> {
    let all = displays();
    let Some(index) = number.checked_sub(1) else {
        return Err(io::Error::other("display numbers start at 1"));
    };
    all.into_iter()
        .nth(usize::try_from(index).unwrap_or(usize::MAX))
        .ok_or_else(|| io::Error::other(format!("display {number} is not attached")))
}

/// The display the mouse pointer is on, else the primary one.
///
/// A capture started from the keyboard has no rectangle and no window to
/// learn a monitor from, so the pointer is what the user means by "here":
/// click the other screen, press the hotkey, and that screen is recorded
/// instead of whichever monitor happens to sit at the desktop origin.
#[must_use]
pub fn under_cursor() -> Display {
    if let Some(point) = cursor() {
        let all = displays();
        if let Some(display) = all.iter().find(|display| contains(**display, point)) {
            return *display;
        }
    }
    primary().unwrap_or(Display {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    })
}

/// Whether `display` covers `(x, y)`.
fn contains(display: Display, point: (i32, i32)) -> bool {
    let width = i32::try_from(display.width).unwrap_or(0);
    let height = i32::try_from(display.height).unwrap_or(0);
    point.0 >= display.x
        && point.0 < display.x.saturating_add(width)
        && point.1 >= display.y
        && point.1 < display.y.saturating_add(height)
}

/// The pointer's desktop position, where the platform can report it.
fn cursor() -> Option<(i32, i32)> {
    #[cfg(windows)]
    {
        win::cursor_position().map(|point| (point.x, point.y))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Every attached display in enumeration order.
#[must_use]
pub fn displays() -> Vec<Display> {
    #[cfg(windows)]
    return win::list();
    #[cfg(target_os = "macos")]
    return mac::list();
    #[cfg(target_os = "linux")]
    return linux::list();
}

/// A window's capture region: (Windows) DWM extended frame bounds, no
/// borders or shadows, clamped to the visible desktop; (Linux) the
/// client rectangle of the window under the cursor; (macOS) the bounds
/// of the frontmost application window under the cursor through the
/// compositor's window list. `preferred` — when given — names the
/// window handle the picker session chose (Windows pointer bits);
/// otherwise (and elsewhere) the window under the cursor answers.
///
/// # Errors
/// Returns an error when no window bounds can be resolved.
pub fn window_region(preferred: Option<usize>) -> Result<Region, io::Error> {
    #[cfg(windows)]
    return win::window_region(preferred);
    #[cfg(target_os = "linux")]
    return linux::window_region(preferred);
    #[cfg(target_os = "macos")]
    return mac::window_region(preferred);
}

#[cfg(test)]
mod tests {
    use super::{display_at, displays, primary};

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
    fn display_at_answers_or_errors_honestly() {
        let all = displays();
        let count = u32::try_from(all.len()).unwrap_or(16);
        if count > 0 {
            assert!(display_at(count).is_ok());
        }
        assert!(display_at(count + 1).is_err());
        assert!(display_at(999).is_err());
        assert!(display_at(0).is_err());
    }
}
