//! Output selection and error surfaces for the duplication pipeline: the
//! region-to-display match, the mode/shape gate, and the v-table
//! preflight every protocol handle must pass before its first call.

use std::io;

use crate::capture::Region;
use crate::dxgi::abi::{AdapterVt, Complete, DuplicationVt, OutputVt};
use crate::dxgi::com::Com;
use crate::dxgi::ffi::ProcError;
use crate::dxgi::structs::{B8G8R8A8, NOT_FOUND, ROTATION_PLAIN};
use crate::winapi::Rect;

/// Pick the output whose desktop coordinates equal `region`, plus that
/// output's desktop-coordinate origin.
///
/// # Errors
/// Fails when the exact output is missing or when it is rotated.
pub(crate) fn pick_output(
    adapter: &Com<AdapterVt>,
    region: Region,
) -> io::Result<(Com<OutputVt>, (i32, i32))> {
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
        let origin = (description.coordinates.left, description.coordinates.top);
        return Ok((output, origin));
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
pub(crate) fn shape_of(duplication: &Com<DuplicationVt>, region: Region) -> io::Result<(u32, u32)> {
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
pub(crate) fn surface<T: Complete>(connection: &Com<T>) -> io::Result<()> {
    if connection.intact() {
        return Ok(());
    }
    Err(io::Error::other("COM v-table layout is not intact"))
}

/// One failed COM call in message form: the step and its raw `HRESULT`.
pub(crate) fn report(step: &str, hr: i32) -> io::Error {
    io::Error::other(format!("{step} answered 0x{hr:08X}"))
}

/// One failed entry-point load in message form.
pub(crate) fn report_spawn(error: &ProcError) -> io::Error {
    io::Error::other(error.message())
}

/// Turn a fallible spawn into the uniform io error surface.
pub(crate) fn spawn<T, F>(boot: F) -> io::Result<T>
where
    F: FnOnce() -> Result<T, ProcError>,
{
    boot().map_err(|error| io::Error::other(error.message()))
}

#[cfg(test)]
mod tests {
    use super::matches_area;
    use crate::capture::Region;
    use crate::winapi::Rect;

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
}
