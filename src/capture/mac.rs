//! CoreGraphics screen frames: the compositor answers a window-list image
//! for the capture area and draws it into a cached premultiplied-BGRA
//! bitmap context. Screen-space coordinates are points (origin at the
//! top-left of the main display), so this backend translates the
//! physical-pixel regions the rest of the project uses into point space.

use std::io;

use crate::coregraphics::{self as cg, Api, Rect};

use super::Region;

/// What to tell a user whose capture will not open: the compositor answers
/// nothing without this permission, and the switch is not where a command
/// line user would look.
pub const HINT: &str = "macOS gates the whole desktop behind Screen Recording: \
     System Settings > Privacy & Security > Screen Recording, enable the app that \
     launched this binary (Terminal, iTerm, or screencapture itself), then restart it";

/// One capture backend bound to one region.
pub struct Backend {
    region: Region,
    /// Main display origin in screen-space points.
    origin: [f64; 2],
    /// Pixels per point (1.0 when unknown: covers non-Retina layouts).
    scale: f64,
    api: Api,
    space: cg::Ref,
    context: cg::Ref,
    /// Context dimensions in pixels.
    width: usize,
    /// Context height in pixels.
    height: usize,
}

impl Backend {
    /// Allocate the bitmap context every frame lands in. The pointer
    /// overlay is not composited along this route (window-list images
    /// carry no cursor), so a requested cursor is a documented no-op.
    ///
    /// # Errors
    /// Fails when CoreGraphics is missing or refuses the context.
    pub fn open(region: Region, _include_cursor: bool) -> io::Result<Self> {
        let api = Api::load()?;
        let space = api.device_color_space()?;
        let width = edge(region.width)?;
        let height = edge(region.height)?;
        let context = api.bg_bitmap_context(width, height, space);
        if context.is_err() {
            api.release_color_space(space);
        }
        let context = context?;
        let (origin, scale) = geometry(&api);
        Ok(Self {
            region,
            origin,
            scale,
            api,
            space,
            context,
            width,
            height,
        })
    }

    /// Draw the screen composition into the cached context, then lend
    /// its bytes: top-down rows of `width * 4` BGRA bytes (premultiplied
    /// alpha, opaque across the desktop thanks to the opaque flag).
    ///
    /// # Errors
    /// Fails when the compositor refuses the window-list capture.
    pub fn frame(&self) -> io::Result<&[u8]> {
        let area = area_in_points(self.origin, self.scale, &self.region);
        self.api
            .draw_screen(area, self.context, self.width, self.height)?;
        let bits = self.api.bitmap_bits(self.context);
        if bits.is_null() {
            return Err(io::Error::other("the bitmap context has no buffer"));
        }
        let len = self.width * 4 * self.height;
        // SAFETY: `bits` is the live context buffer of exactly `len`
        // bytes for the dimensions recorded at construction.
        Ok(unsafe { std::slice::from_raw_parts(bits, len) })
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        // SAFETY: the context and space were allocated by this backend's
        // own Api handles, released in reverse creation order.
        self.api.release_context(self.context);
        self.api.release_color_space(self.space);
    }
}

/// The native fast path, reported by `--check`: the compositor plus the
/// main display's physical-pixel geometry.
///
/// # Errors
/// CoreGraphics is missing or reports no display.
pub fn probe() -> Result<(&'static str, u32, u32), String> {
    let api = Api::load().map_err(|error| format!("CoreGraphics: {error}"))?;
    let Some((id, _, _)) = api.main_geometry() else {
        return Err("no active display found".to_owned());
    };
    let width = u32::try_from(api.pixel_width(id)).unwrap_or(0);
    let height = u32::try_from(api.pixel_height(id)).unwrap_or(0);
    Ok(("CoreGraphics compositor", width, height))
}

/// Screen-space area (points) for a physical-pixel region, given the main
/// display origin and scale.
fn area_in_points(origin: [f64; 2], scale: f64, region: &Region) -> Rect {
    let scale = scale.max(1.0);
    Rect {
        origin: [
            origin[0] + f64::from(region.x) / scale,
            origin[1] + f64::from(region.y) / scale,
        ],
        size: [
            f64::from(region.width) / scale,
            f64::from(region.height) / scale,
        ],
    }
}

/// Main display geometry, or a scale-1 fallback when nothing answers.
fn geometry(api: &Api) -> ([f64; 2], f64) {
    api.main_geometry()
        .map_or(([0.0, 0.0], 1.0), |(_, bounds, scale)| {
            (bounds.origin, scale)
        })
}

/// A region edge as a bitmap dimension.
fn edge(pixels: u32) -> io::Result<usize> {
    usize::try_from(u64::from(pixels)).map_err(|_| io::Error::other("capture region is oversized"))
}

#[cfg(test)]
mod tests {
    use super::{Region, area_in_points};

    #[test]
    fn regions_map_into_point_space() {
        // 2x scale: a 1600-pixel row spans 800 points, offset from a
        // display origin of 100 points.
        let region = Region {
            x: 100,
            y: 50,
            width: 1600,
            height: 1200,
        };
        let area = area_in_points([100.0, 0.0], 2.0, &region);
        assert_eq!(area.origin, [150.0, 25.0]);
        assert_eq!(area.size, [800.0, 600.0]);
    }

    #[test]
    fn unscaled_regions_pass_through() {
        let region = Region {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let area = area_in_points([0.0, 0.0], 1.0, &region);
        assert_eq!(area.origin, [0.0, 0.0]);
        assert_eq!(area.size, [1920.0, 1080.0]);
    }
}
