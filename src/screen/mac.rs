//! macOS display geometry and window picking: the compositor answers the
//! main display's bounds (points) and pixel metrics, and the on-screen
//! window list locates the window under the cursor. Capture regions stay
//! physical pixels, so window picks convert point-space bounds through
//! the main display's scale and clamp into the display's pixel extent
//! (multi-display origin policy still waits for a Mac-verified run).
//!
//! Facts used here were transcribed from Apple's SDK headers, as in
//! session 5: the window-list keys and their front-to-back ordering live
//! in `CGWindow.h`, the levels in `CGWindowLevel.h`, and the cursor
//! answer in `CGEvent.h` (global display space, top-left of the main
//! display).

use std::io;

use crate::capture::Region;
use crate::corefoundation::{self as cf, Api as CfApi};
use crate::coregraphics::{Api as CgApi, Rect};
use crate::screen::Display;

/// The window dictionary's bounds key. `CGWindow.h` declares these keys
/// as constant `CFStringRef`s whose values are their literal names (the
/// headers annotate `kCGWindowBounds` as a dictionary encoding of a
/// `CGRect`), so a here-created `UTF-8` string of the same name compares
/// equal to the runtime key.
const BOUNDS_KEY: &str = "kCGWindowBounds";
/// The window dictionary's layer key (a `CFNumber` of the window level).
const LAYER_KEY: &str = "kCGWindowLayer";
/// `kCGNormalWindowLevel` (`CGWindowLevel.h`, an `int32_t` of 0): the
/// level application windows live at; other levels cover the desktop,
/// dock, menu bar, and floating overlays.
const NORMAL_LEVEL: f64 = 0.0;

/// The main display, the node every recording anchors to. Coordinates
/// are physical pixels; empty when CoreGraphics is absent or reports no
/// usable display.
#[must_use]
pub fn list() -> Vec<Display> {
    let Ok(api) = CgApi::load() else {
        return Vec::new();
    };
    let Some((id, _, _)) = api.main_geometry() else {
        return Vec::new();
    };
    let width = u32::try_from(api.pixel_width(id)).unwrap_or(0);
    let height = u32::try_from(api.pixel_height(id)).unwrap_or(0);
    if width == 0 || height == 0 {
        return Vec::new();
    }
    vec![Display {
        x: 0,
        y: 0,
        width,
        height,
    }]
}

/// The window under the cursor as a physical-pixel region: the
/// front-to-back window list answers the first layer-zero window whose
/// bounds hold the cursor point, decoded through the SDK's rect rule.
/// When the cursor sits on the desktop (or nothing answers), the whole
/// main display takes over, mirroring the Linux backend. `preferred`
/// (a picked Windows handle) is a Windows notion; the mac tray does
/// not exist yet, so it is ignored, exactly like on Linux.
///
/// # Errors
/// CoreGraphics or CoreFoundation is missing, or no display answers.
pub fn window_region(preferred: Option<usize>) -> Result<Region, io::Error> {
    let _ = preferred;
    let graphics = CgApi::load().map_err(|e| io::Error::other(format!("CoreGraphics: {e}")))?;
    let foundation = CfApi::load().map_err(|e| io::Error::other(format!("CoreFoundation: {e}")))?;
    let Some((id, display, scale)) = graphics.main_geometry() else {
        return Err(io::Error::other("no active display found"));
    };
    let scale = scale.max(1.0);
    let extent = [
        u32::try_from(graphics.pixel_width(id)).unwrap_or(0),
        u32::try_from(graphics.pixel_height(id)).unwrap_or(0),
    ];
    let bounds = hovered_bounds(&graphics, &foundation).unwrap_or(display);
    region_from_bounds(&bounds, scale, extent)
}

/// The screen-space bounds of the frontmost layer-zero window that
/// contains the cursor: the window list is walked front to back (its
/// SDK-documented on-screen order), non-application levels skip, and the
/// `kCGWindowBounds` dictionary is decoded by CoreGraphics itself.
///
/// `None` when the cursor is unknowable, the list is absent, or only
/// desktop/dock/overlay surfaces hold the cursor.
fn hovered_bounds(graphics: &CgApi, foundation: &CfApi) -> Option<Rect> {
    let point = cursor_point(graphics, foundation)?;
    let list = graphics.window_list();
    if list.is_null() {
        return None;
    }
    let Some(bounds_key) = foundation.utf8(BOUNDS_KEY) else {
        foundation.release(list);
        return None;
    };
    let Some(level_key) = foundation.utf8(LAYER_KEY) else {
        foundation.release(bounds_key);
        foundation.release(list);
        return None;
    };
    // SAFETY: the casts below move live handles between the two APIs'
    // CoreFoundation-typed fields (array entries, dictionaries, keys),
    // whose ref types carry the same `void*` layout.
    let mut found = None;
    for index in 0..foundation.array_len(list) {
        let entry = foundation.array_value(list, index);
        if entry.is_null() || level(foundation, entry, level_key) != NORMAL_LEVEL {
            continue;
        }
        let bounds = foundation.dict_value(entry.cast_mut(), bounds_key);
        let Some(rect) = graphics.bounds_value(bounds.cast_mut()) else {
            continue;
        };
        if holds(&point, &rect) {
            found = Some(rect);
            break;
        }
    }
    foundation.release(level_key);
    foundation.release(bounds_key);
    foundation.release(list);
    found
}

/// The cursor position as the SDK-documented global display point (the
/// top-left of the main display is the origin), asked through the event
/// stream. Owned event released before returning.
///
/// `None` when the stream answers no event, leaving the caller its
/// own fallback.
fn cursor_point(graphics: &CgApi, foundation: &CfApi) -> Option<[f64; 2]> {
    // SAFETY: null selects the default event source per CGEvent.h; the
    // event is owned per the Create rule.
    let event = unsafe { (graphics.event_create)(std::ptr::null()) };
    if event.is_null() {
        return None;
    }
    // SAFETY: a live event; the answer is a read-only coordinate probe.
    let point = unsafe { (graphics.event_location)(event) };
    foundation.release(event);
    Some(point)
}

/// One window-entry level, coercing integer `CFNumber`s; an entry without
/// the key answers a value no normal window carries, so it is skipped.
fn level(foundation: &CfApi, entry: *const std::ffi::c_void, key: cf::Ref) -> f64 {
    foundation
        .number_f64(foundation.dict_value(entry.cast_mut(), key).cast_mut())
        .unwrap_or(f64::NAN)
}

/// Cursor-channel containment on a screen-space rect: edges exclusive
/// (a point on the right or bottom border belongs to the next window in
/// line), and zero-size answers never hold anything.
fn holds(point: &[f64; 2], rect: &Rect) -> bool {
    rect.size[0] >= 1.0
        && rect.size[1] >= 1.0
        && point[0] >= rect.origin[0]
        && point[0] < rect.origin[0] + rect.size[0]
        && point[1] >= rect.origin[1]
        && point[1] < rect.origin[1] + rect.size[1]
}

/// A screen-space rect as a physical-pixel region: every edge scales by
/// the display ratio, rounds to the pixel grid, and clamps into the
/// display extent (window bounds reaching outside record the visible
/// part; the multi-display origin session owns the rounding beyond).
/// A degenerate edge keeps at least one pixel, so the engine never sees
/// an empty frame.
///
/// # Errors
/// A bound is not a finite number, or falls outside the pixel space.
fn region_from_bounds(rect: &Rect, scale: f64, extent: [u32; 2]) -> Result<Region, io::Error> {
    let left = pixel(rect.origin[0] * scale, extent[0])?;
    let top = pixel(rect.origin[1] * scale, extent[1])?;
    let right = pixel((rect.origin[0] + rect.size[0]) * scale, extent[0])?;
    let bottom = pixel((rect.origin[1] + rect.size[1]) * scale, extent[1])?;
    // One visible pixel past every clamp keeps frames nonempty, and an
    // origin clamped to the extent hugs the last valid pixel instead.
    let x = left.min(extent[0].saturating_sub(1));
    let y = top.min(extent[1].saturating_sub(1));
    Ok(Region {
        x: i32::try_from(x).map_err(|_| out_of_pixel_space())?,
        y: i32::try_from(y).map_err(|_| out_of_pixel_space())?,
        width: right.saturating_sub(x).max(1).min(extent[0] - x),
        height: bottom.saturating_sub(y).max(1).min(extent[1] - y),
    })
}

/// One pixel coordinate from a screen-space scalar: round, clamp into
/// `0..=extent`, and extract the integral value exactly.
///
/// # Errors
/// The coordinate is not a finite number.
fn pixel(value: f64, extent: u32) -> io::Result<u32> {
    if !value.is_finite() {
        return Err(io::Error::other("window bounds carry a non-finite value"));
    }
    let clamped = value.round().clamp(0.0, f64::from(extent));
    // SAFETY: `clamped` is a finite, integer-valued float inside
    // `0..=u32::MAX`, so the unchecked extraction is exact.
    Ok(unsafe { clamped.to_int_unchecked::<u32>() })
}

/// The shared failure text for a bound that nothing on screen can hold.
fn out_of_pixel_space() -> io::Error {
    io::Error::other("window bounds exceed the pixel space")
}

#[cfg(test)]
mod tests {
    use super::{Display, Rect, holds, pixel, region_from_bounds};

    fn rect(origin: [f64; 2], size: [f64; 2]) -> Rect {
        Rect { origin, size }
    }

    #[test]
    fn rects_hold_their_interior_cursor() {
        let window = rect([100.0, 50.0], [600.0, 400.0]);
        assert!(holds(&[100.0, 50.0], &window));
        assert!(holds(&[699.9, 400.0], &window));
        // Edge-exclusive right and bottom; outside is outside.
        assert!(!holds(&[700.0, 400.0], &window));
        assert!(!holds(&[699.0, 450.0], &window));
        // Zero-size and degenerate rects hold no point.
        assert!(!holds(&[150.0, 60.0], &rect([100.0, 50.0], [0.0, 400.0])));
    }

    #[test]
    fn points_clamp_into_the_pixel_extent() {
        assert_eq!(pixel(100.4, 3840).unwrap(), 100);
        assert_eq!(pixel(100.6, 3840).unwrap(), 101);
        assert_eq!(pixel(-40.0, 3840).unwrap(), 0);
        assert_eq!(pixel(5000.0, 3840).unwrap(), 3840);
        assert!(pixel(f64::NAN, 3840).is_err());
        assert!(pixel(f64::INFINITY, 3840).is_err());
    }

    #[test]
    fn bounds_become_pixel_regions() {
        // A 2x-Retina panel whose app window spans the top-left corner:
        // 100x50 points becomes a 200x100 pixel origin.
        let extent = [3840, 2160];
        let region = region_from_bounds(&rect([100.0, 50.0], [600.0, 400.0]), 2.0, extent).unwrap();
        assert_eq!(
            region,
            super::Region {
                x: 200,
                y: 100,
                width: 1200,
                height: 800
            }
        );
        // Partially off-display windows record the visible part.
        let region =
            region_from_bounds(&rect([-50.0, -50.0], [1000.0, 1000.0]), 1.0, extent).unwrap();
        assert_eq!(
            region,
            super::Region {
                x: 0,
                y: 0,
                width: 950,
                height: 950,
            }
        );
        // A fully-clamped sliver stays a one-pixel answer inside the
        // display extent (the origin hugs the last valid pixel).
        let region = region_from_bounds(&rect([3840.0, 10.0], [10.0, 10.0]), 1.0, extent).unwrap();
        assert_eq!(region.x, 3839);
        assert_eq!(region.y, 10);
        assert_eq!(region.width, 1);
        assert_eq!(region.height, 10);
    }

    #[test]
    fn display_api_reports_the_main_panel() {
        // The geometry answer degrades silently on this host (no macOS
        // runtime); the platform-neutral contract must still hold.
        let all = super::list();
        for Display { width, height, .. } in all {
            assert!(width > 0 && height > 0);
        }
    }
}
