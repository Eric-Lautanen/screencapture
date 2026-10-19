//! macOS display geometry: the compositor answers the main display's
//! bounds (points) and pixel metrics (physical). The capture region space
//! stays physical pixels, so the display reports its own pixel size with
//! the origin pinned at (0,0); multi-display enumeration and origins need
//! a Mac-verified session and are deferred (see ROADMAP).

use crate::coregraphics::Api;
use crate::screen::Display;

/// The main display, the node every recording anchors to. Coordinates
/// are physical pixels.
///
/// # Errors
/// CoreGraphics is missing or reports no display.
pub fn list() -> Vec<Display> {
    let Ok(api) = Api::load() else {
        return Vec::new();
    };
    let Some((id, _, _)) = api.main_geometry() else {
        return Vec::new();
    };
    vec![Display {
        x: 0,
        y: 0,
        width: u32::try_from(api.pixel_width(id)).unwrap_or(0),
        height: u32::try_from(api.pixel_height(id)).unwrap_or(0),
    }]
}
