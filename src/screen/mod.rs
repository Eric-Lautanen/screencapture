//! Display, monitor, and window geometry. Each platform module reports
//! its displays in physical-pixel semantics (Windows enumerate monitors,
//! macOS answers the compositor's main display, Linux the default screen
//! root), and window picking stays a Windows capability for now.

use std::io;

use crate::capture::Region;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod win;

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
    let index = usize::try_from(number - 1).unwrap_or(usize::MAX);
    all.into_iter()
        .nth(index)
        .ok_or_else(|| io::Error::other(format!("display {number} is not attached")))
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

/// A window's capture region: DWM extended frame bounds (no borders or
/// shadows), clamped to the visible desktop. `preferred` names a window
/// the caller had focused just before a tray menu opened; otherwise the
/// window under the cursor is used.
///
/// # Errors
/// Returns an error when no window bounds can be resolved.
pub fn window_region(preferred: Option<usize>) -> Result<Region, io::Error> {
    #[cfg(windows)]
    return win::window_region(preferred);
    #[cfg(not(windows))]
    {
        let _ = preferred;
        Err(io::Error::other(
            "window picking lands with the tray on this platform",
        ))
    }
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
    }
}
