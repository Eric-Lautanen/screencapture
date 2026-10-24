//! Frame capture: one `Target` per session converts a screen region into
//! top-down BGRA bytes. Platform backends live in their own modules
//! (`win.rs` GDI blit, `mac.rs` CoreGraphics compositor draw, `linux.rs`
//! X11 root images) and only differ behind `Target`/`probe`.

use std::io;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(windows)]
pub mod win;

/// Region on screen: origin and extent in physical pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    /// Origin x.
    pub x: i32,
    /// Origin y.
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Region {
    /// Whether this region could ever contain pixels.
    #[must_use]
    pub fn is_sane(self) -> bool {
        self.width > 0 && self.height > 0
    }
}

#[cfg(target_os = "linux")]
use linux::Backend as Platform;
#[cfg(target_os = "macos")]
use mac::Backend as Platform;
#[cfg(windows)]
use win::Backend as Platform;

/// A capture target bound to one region; frames stay readable until the
/// next capture of the same target.
pub struct Target {
    backend: Platform,
}

impl Target {
    /// Capture pixels from `region` on the native composition backend.
    /// `include_cursor` asks the backend to composite the mouse pointer;
    /// backends without pointer support ignore it and note why.
    ///
    /// # Errors
    /// Fails when the region is empty or the platform backend cannot be
    /// set up.
    pub fn new(region: Region, include_cursor: bool) -> io::Result<Self> {
        if !region.is_sane() {
            return Err(io::Error::other("capture region is empty"));
        }
        Ok(Self {
            backend: Platform::open(region, include_cursor)?,
        })
    }

    /// Capture only `window`'s own content, laid out for `region`.
    ///
    /// A window that another application covers keeps drawing itself, so
    /// this saves the selected application rather than the rectangle of
    /// desktop it happens to sit behind. Windows only: every other platform
    /// captures the screen, and callers there never pick a window.
    ///
    /// # Errors
    /// Fails when the platform backend cannot capture that window (or does
    /// not exist on this platform at all).
    #[cfg(windows)]
    pub fn window(handle: usize, region: Region, include_cursor: bool) -> io::Result<Self> {
        if !region.is_sane() {
            return Err(io::Error::other("capture region is empty"));
        }
        Ok(Self {
            backend: Platform::window(handle, region, include_cursor)?,
        })
    }

    /// Lend the latest frame: top-down rows of `width * 4` BGRA bytes.
    /// The data lives inside the backend and is valid until the next
    /// capture, so callers must write it out before capturing again.
    ///
    /// # Errors
    /// Fails when the native backend refuses the frame.
    pub fn frame(&mut self) -> io::Result<&[u8]> {
        self.backend.frame()
    }
}

/// Opt into per-monitor DPI awareness on Windows so screen coordinates
/// are physical pixels; other systems already give physical quanta too.
pub fn prepare() {
    #[cfg(windows)]
    {
        win::prepare_dpi();
        // Any earlier process killed mid-pick left the system-wide arrow
        // slot as a crosshair; reload the user's scheme before doing
        // anything else with the screen.
        crate::guard::restore_system_cursors();
    }
}

/// Describe the native fast capture path for `--check`: backend label and
/// its capture resolution, or the reason it is unavailable.
#[cfg(not(windows))]
pub fn probe() -> Result<(String, u32, u32), String> {
    native_probe().map(|(label, width, height)| (label.to_owned(), width, height))
}

#[cfg(not(windows))]
fn native_probe() -> Result<(&'static str, u32, u32), String> {
    #[cfg(target_os = "macos")]
    return mac::probe();
    #[cfg(target_os = "linux")]
    return linux::probe();
}

/// What a user on this platform can do about a capture that will not open:
/// printed under the `--check` failure so the fix sits next to the symptom.
#[cfg(not(windows))]
pub fn hint() -> &'static str {
    #[cfg(target_os = "macos")]
    return mac::HINT;
    #[cfg(target_os = "linux")]
    return linux::HINT;
}

#[cfg(test)]
mod tests {
    use super::{Region, Target};

    /// The hint is the only text most users read after a FAIL, so it has to
    /// name the platform's actual switch, not just say capture failed.
    #[cfg(not(windows))]
    #[test]
    fn the_hint_names_this_platforms_fix() {
        let hint = super::hint();
        assert!(hint.len() > 40, "{hint}");
        #[cfg(target_os = "macos")]
        assert!(hint.contains("Screen Recording"), "{hint}");
        #[cfg(target_os = "linux")]
        assert!(hint.contains("X11"), "{hint}");
    }

    #[test]
    fn empty_regions_are_refused() {
        let region = Region {
            x: 0,
            y: 0,
            width: 0,
            height: 100,
        };
        assert!(!region.is_sane());
        assert!(Target::new(region, false).is_err());
    }
}
