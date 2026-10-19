//! Windows desktop duplication capture: a `Duplicator` lends full-screen
//! BGRA frames straight from the compositor, far cheaper than a GDI blit
//! and the reason a 4K recording keeps 60 fps wall-clock honest.
//!
//! Policy: duplication carries a plan only when the region equals one
//! unrotated output, no cursor is requested, and the pixel format is the
//! plain BGRA one; everything else (partial regions, HDR panels, pointer
//! drawing) falls back to the GDI `capture::Target`.

use std::ffi::c_void;
use std::io;

use crate::capture::Region;
use crate::dxgi::abi::{
    AdapterVt, Complete, ContextVt, DeviceVt, DuplicationVt, Output1Vt, OutputVt, TextureVt,
};
use crate::dxgi::com::Com;
use crate::dxgi::ffi::ProcError;
use crate::dxgi::structs::{B8G8R8A8, ID3D_TEXTURE2D, IDXGI_OUTPUT1, NOT_FOUND, ROTATION_PLAIN};
use crate::winapi::Rect;

pub mod abi;
pub mod com;
pub mod ffi;
pub mod structs;

/// One recording's frame source: the compositor's desktop copy.
pub struct Duplicator {
    /// The active desktop duplication session.
    duplication: Com<DuplicationVt>,
    /// CPU-readable texture holding the latest frame.
    staging: Com<TextureVt>,
    /// The immediate context that copies and locks.
    context: Com<ContextVt>,
    /// The device that owns staging and context; held for release order,
    /// which is why the name reads as unused.
    _device: Com<DeviceVt>,
    /// Latest frame, compacted to `width * 4` rows of BGRA.
    scratch: Vec<u8>,
    /// Width in pixels.
    width: u32,
    /// Height in pixels.
    height: u32,
}

impl Duplicator {
    /// Open duplication for a region that is exactly one display output.
    /// The session starts with one best-effort frame grab; a static screen
    /// may leave the first frames blank rather than fail.
    ///
    /// # Errors
    /// Fails when the region is not one whole unrotated output, the pixel
    /// format is not plain BGRA, or the graphics stack refuses anything.
    pub fn open(region: Region) -> io::Result<Self> {
        let factory = spawn(ffi::spawn_factory)?;
        let adapter = factory.adapter1(0).map_err(|hr| report("adapter", hr))?;
        surface(&adapter)?;
        let output = pick_output(&adapter, region)?;
        let output1 = output
            .query_to::<Output1Vt>(&IDXGI_OUTPUT1)
            .map_err(|hr| report("output", hr))?;
        surface(&output1)?;
        let (device, context) =
            ffi::spawn_device(&adapter).map_err(|error| report_spawn(&error))?;
        surface(&device)?;
        let duplication = output1
            .duplication(&device)
            .map_err(|hr| report("duplication", hr))?;
        surface(&duplication)?;
        let shape = shape_of(&duplication, region)?;
        let staging = device
            .staged(shape.0, shape.1)
            .map_err(|hr| report("staging texture", hr))?;
        surface(&staging)?;
        let mut duplicator = Self {
            duplication,
            staging,
            context,
            _device: device,
            scratch: vec![0; shape.0 as usize * shape.1 as usize * 4],
            width: shape.0,
            height: shape.1,
        };
        // First paint best effort; blank beats stalling the opener when the
        // desktop has been quiet since duplication began.
        let _ = duplicator.refresh(FIRST_PAINT_WAIT);
        Ok(duplicator)
    }

    /// Grab one BGRA frame: fresh when the screen changed since the last
    /// tick, otherwise the previous update again. The bytes are valid until
    /// the next call, exactly like the GDI target's contract.
    ///
    /// # Errors
    /// Fails when the duplication session was lost (mode switches and
    /// device resets wholesale) or the staging lock is refused.
    pub fn frame(&mut self) -> io::Result<&[u8]> {
        self.refresh(IDLE_WAIT)?;
        Ok(&self.scratch)
    }

    /// The lent frame's shape, for stream plumbing.
    #[must_use]
    pub fn shape(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Pull whatever the compositor updated within `wait` milliseconds,
    /// volunteering nothing (and keeping the old frame) when it stayed
    /// quiet.
    ///
    /// # Errors
    /// Fails when the session is unusable or the copy pipeline refuses.
    fn refresh(&mut self, wait: u32) -> io::Result<()> {
        let Some(resource) = self
            .duplication
            .acquire(wait)
            .map_err(|hr| report("acquire", hr))?
        else {
            return Ok(());
        };
        let texture = resource
            .query_to::<TextureVt>(&ID3D_TEXTURE2D)
            .map_err(|hr| report("texture", hr))?;
        self.context.clone_resource(&self.staging, &texture);
        // Release the desktop frame before the slow staging read, so the
        // compositor is free well before `Map` stalls on the GPU queue.
        self.duplication
            .release_frame()
            .map_err(|hr| report("release frame", hr))?;
        let mapped = self
            .context
            .lock(&self.staging)
            .map_err(|hr| report("staging lock", hr))?;
        self.absorb(mapped);
        self.context.unlock(&self.staging);
        Ok(())
    }

    /// Flatten the locked staging rows into the scratch frame, so every
    /// frame lent out stays a plain `width * 4` BGRA layout.
    fn absorb(&mut self, mapped: crate::dxgi::structs::Mapped) {
        let width_row = self.width as usize * 4;
        flatten(
            self.height as usize,
            width_row,
            mapped.row_pitch as usize,
            mapped.data,
            &mut self.scratch,
        );
    }
}

/// Copy `pitch`-spaced BGRA rows onto tight `width_row` rows, zero-padding
/// whatever the driver left beyond the row so lent frames stay stable.
fn flatten(rows: usize, width_row: usize, pitch: usize, data: *mut c_void, output: &mut [u8]) {
    output.fill(0);
    if data.is_null() {
        return;
    }
    // SAFETY: `data` points at `pitch * rows` bytes the driver mapped for
    // us until `unlock`; the bounds here follow the API contract.
    let source =
        unsafe { std::slice::from_raw_parts(data.cast::<u8>(), pitch.saturating_mul(rows)) };
    if pitch == width_row {
        output.copy_from_slice(&source[..width_row * rows]);
        return;
    }
    for row in 0..rows {
        let begin = row * pitch;
        let end = begin + width_row;
        if end > source.len() {
            break;
        }
        output[row * width_row..row * width_row + width_row].copy_from_slice(&source[begin..end]);
    }
}

/// Whether duplication should ever carry this plan (audio of the policy).
#[must_use]
pub fn eligible(region: Region, cursor: bool) -> bool {
    region.is_sane() && !cursor
}

/// Probe duplication once for diagnostics: the first output's shape.
///
/// # Errors
/// Returns whatever `open` would refuse.
pub fn probe() -> io::Result<(u32, u32)> {
    let display = crate::screen::primary()?;
    let region = Region {
        x: display.x,
        y: display.y,
        width: display.width,
        height: display.height,
    };
    let duplicator = Duplicator::open(region)?;
    Ok(duplicator.shape())
}

const FIRST_PAINT_WAIT: u32 = 50;
/// Non-blocking grab: the tick loop owns the pacing, duplication never
/// sleeps the pump.
const IDLE_WAIT: u32 = 0;

/// Pick the output whose desktop coordinates equal `region`.
///
/// # Errors
/// Fails when the exact output is missing or when it is rotated.
fn pick_output(adapter: &Com<AdapterVt>, region: Region) -> io::Result<Com<OutputVt>> {
    let mut index = 0;
    loop {
        let output = match adapter.output(index) {
            Ok(found) => found,
            Err(hr) if hr == NOT_FOUND => {
                return Err(io::Error::other(
                    "no display output matches the capture region",
                ));
            }
            Err(hr) => return Err(report("output enumeration", hr)),
        };
        surface(&output)?;
        let description = output.describe();
        if !matches_area(&description.coordinates, region) {
            index += 1;
            continue;
        }
        if !ROTATION_PLAIN.contains(&description.rotation) {
            return Err(io::Error::other(
                "the matching display is rotated; duplication support stops here",
            ));
        }
        return Ok(output);
    }
}

/// True when the rect describes exactly this region on screen.
fn matches_area(rect: &Rect, region: Region) -> bool {
    i64::from(rect.right) - i64::from(rect.left) == i64::from(region.width)
        && i64::from(rect.bottom) - i64::from(rect.top) == i64::from(region.height)
        && rect.left == region.x
        && rect.top == region.y
}

/// The duplication's mode must already be the capture's shape in BGRA.
///
/// # Errors
/// Fails when the shape differs or the panel is not plain BGRA (HDR).
fn shape_of(duplication: &Com<DuplicationVt>, region: Region) -> io::Result<(u32, u32)> {
    let state = duplication.descriptor();
    if state.mode.format != B8G8R8A8 {
        return Err(io::Error::other(format!(
            "desktop image is 0x{:X}; only 0x{B8G8R8A8:X} (BGRA) is supported",
            state.mode.format
        )));
    }
    if state.mode.width != region.width || state.mode.height != region.height {
        return Err(io::Error::other(format!(
            "desktop image is {}x{}; the plan wants {}x{}",
            state.mode.width, state.mode.height, region.width, region.height
        )));
    }
    Ok((state.mode.width, state.mode.height))
}

/// Prove an interface's declared v-table layout before its first call.
///
/// # Errors
/// Fails when the shape of the live object disagrees with the compiled
/// assumption; recordings then fall back to GDI instead of guessing.
fn surface<T: Complete>(connection: &Com<T>) -> io::Result<()> {
    if connection.intact() {
        return Ok(());
    }
    Err(io::Error::other("COM v-table layout is not intact"))
}

fn report(step: &str, hr: i32) -> io::Error {
    io::Error::other(format!("{step} answered 0x{hr:08X}"))
}

fn report_spawn(error: &ProcError) -> io::Error {
    io::Error::other(error.message())
}

/// Turn a fallible spawn into the uniform io error surface.
fn spawn<T, F>(boot: F) -> io::Result<T>
where
    F: FnOnce() -> Result<T, ProcError>,
{
    boot().map_err(|error| io::Error::other(error.message()))
}

#[cfg(test)]
mod tests {
    use super::{eligible, flatten, matches_area};
    use crate::capture::Region;
    use crate::winapi::Rect;

    #[test]
    fn partial_plans_stay_gdi() {
        let screen = Region {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        assert!(eligible(screen, false));
        assert!(!eligible(screen, true));
        assert!(!eligible(Region { width: 0, ..screen }, false));
    }

    #[test]
    fn only_exact_output_bounds_match() {
        let rect = Rect {
            left: 0,
            top: 0,
            right: 3840,
            bottom: 2160,
        };
        let exact = Region {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        assert!(matches_area(&rect, exact));
        let shifted = Region {
            x: 60,
            y: 0,
            width: 3840,
            height: 2160,
        };
        assert!(!matches_area(&rect, shifted));
        let partial = Region {
            x: 0,
            y: 0,
            width: 1920,
            height: 2160,
        };
        assert!(!matches_area(&rect, partial));
    }

    #[test]
    fn flatten_compacts_driver_padding() {
        let width_row = 8;
        let rows = 2;
        let mut source = vec![0u8; 32];
        source[0..4].copy_from_slice(&[10, 20, 30, 255]);
        source[16..20].copy_from_slice(&[70, 80, 90, 255]);
        let mut output = vec![1u8; 16];
        let data = source.as_mut_ptr().cast::<std::ffi::c_void>();
        flatten(rows, width_row, 16, data, &mut output);
        assert_eq!(
            output,
            [10, 20, 30, 255, 0, 0, 0, 0, 70, 80, 90, 255, 0, 0, 0, 0]
        );
    }
}
