//! X11 root-window frames: one `XGetImage` per frame straight from the X
//! server. The server layout is asserted to what every true-color desktop
//! delivers — 32-bit little-endian `ZPixmap`, whose bytes read as BGRA
//! with an unused fourth byte that this backend forces opaque so still
//! captures stay fully visible.

use std::io;
use std::slice;

use crate::xlib::{self, Api, BITS_PER_PIXEL, FORMAT_Z, Image, ORDER_LSB, Window};

use super::Region;

/// What to tell a user whose capture will not open: this backend reads the
/// root window, which only an X11 session has.
pub const HINT: &str = "capture here reads the X11 root window: run the app in an \
     X11 session (log in with \"Xorg\" / \"X11\" chosen), because a Wayland session \
     shows XWayland clients but not the desktop itself";

/// `AllPlanes` (`Xlib.h`): every server plane included.
const PLANES_ALL: u64 = u64::MAX;

/// One capture backend bound to one region.
pub struct Backend {
    region: Region,
    api: Api,
    display: xlib::Display,
    root: Window,
    /// Frame dimensions in pixels, fixed at construction.
    width: usize,
    height: usize,
    /// Flattened BGRA frame: allocated up front, pinned, overwritten per
    /// frame. Kept raw for the same lend contract as the other backends.
    buffer: *mut u8,
}

impl Backend {
    /// Open the X connection and allocate the frame buffer.
    ///
    /// # Errors
    /// Fails when `libX11` is absent, no X server answers, or the region
    /// does not fit the buffer budget.
    pub fn open(region: Region, _include_cursor: bool) -> io::Result<Self> {
        let api = Api::load()?;
        let display = api.open()?;
        let root = api.root(display);
        let width = edge(region.width)?;
        let height = edge(region.height)?;
        let buffer = frame_buffer(width, height)?;
        Ok(Self {
            region,
            api,
            display,
            root,
            width,
            height,
            buffer,
        })
    }

    /// Fetch one image from the root drawable and lend its BGRA bytes:
    /// top-down rows of `width * 4` bytes, alpha forced opaque.
    ///
    /// # Errors
    /// Fails when the X server refuses the image or answers in a layout
    /// other than 32-bit little-endian true color.
    pub fn frame(&self) -> io::Result<&[u8]> {
        // SAFETY: every argument is owned by this call; the returned
        // image is X-managed and destroyed on all paths below.
        let image = self.api.image(
            self.display,
            xlib::ImageRequest {
                drawable: self.root,
                x: self.region.x,
                y: self.region.y,
                width: self.region.width,
                height: self.region.height,
                planes: PLANES_ALL,
                format: FORMAT_Z,
            },
        );
        if image.is_null() {
            return Err(io::Error::other("the X server refused the image"));
        }
        let out = self.absorb(image);
        // SAFETY: the image handle is live exactly through `absorb`.
        unsafe { xlib::destroy_image(image) };
        out
    }

    /// Copy the server image into the pinned buffer, repairing alpha.
    fn absorb(&self, image: Image) -> io::Result<&[u8]> {
        // SAFETY: the image is a live, X-managed record; the accesses
        // below are plain field reads, and the pointer offsets stay
        // inside the image buffer and the backend's pinned frame.
        let source_width = usize::try_from(unsafe { (*image).width }).unwrap_or(0);
        let source_height = usize::try_from(unsafe { (*image).height }).unwrap_or(0);
        let (bits, order, pitch, data) = unsafe {
            (
                (*image).bits_per_pixel,
                (*image).byte_order,
                i64::from((*image).bytes_per_line),
                (*image).data.cast::<u8>(),
            )
        };
        if source_width != self.width || source_height != self.height {
            return Err(io::Error::other(
                "the X server answered a differently sized image",
            ));
        }
        if bits != BITS_PER_PIXEL || order != ORDER_LSB {
            return Err(io::Error::other(
                "only 32-bit little-endian ZPixmap layouts are supported",
            ));
        }
        let pitch = usize::try_from(pitch).unwrap_or(0);
        let row = self.width * 4;
        if pitch < row || data.is_null() {
            return Err(io::Error::other(
                "the X image stride is narrower than the frame",
            ));
        }
        for line in 0..self.height {
            // SAFETY: the read spans one row inside the server image;
            // the write spans one row of the pinned backend buffer.
            let from = unsafe { slice::from_raw_parts(data.add(line * pitch), row) };
            // SAFETY: same span, destination side.
            let into = unsafe { slice::from_raw_parts_mut(self.buffer.add(line * row), row) };
            into.copy_from_slice(from);
            declare_opaque(into);
        }
        // SAFETY: the whole buffer span was authored by this backend.
        Ok(unsafe { slice::from_raw_parts(self.buffer, row * self.height) })
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        // SAFETY: the connection and buffer came from this backend; the
        // buffer is rebuilt with the same dimensions used to allocate it.
        self.api.close(self.display);
        let row = self.width * 4;
        if !self.buffer.is_null() {
            drop(unsafe {
                Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                    self.buffer,
                    row * self.height,
                ))
            });
        }
    }
}

/// Force the alpha byte of every BGRA pixel: the server leaves the
/// unused channel zeroed, which would make still captures transparent.
fn declare_opaque(row: &mut [u8]) {
    for pixel in row.as_chunks_mut::<4>().0 {
        pixel[3] = u8::MAX;
    }
}

/// The native fast path, reported by `--check`: a display connection and
/// the default screen size.
///
/// # Errors
/// `libX11` is missing or no server answers `DISPLAY`.
pub fn probe() -> Result<(&'static str, u32, u32), String> {
    let api = Api::load().map_err(|error| format!("libX11: {error}"))?;
    let display = api.open().map_err(|error| format!("X server: {error}"))?;
    let (width, height) = api.main_screen(display);
    api.close(display);
    Ok(("X11 root image", width, height))
}

/// A region edge as a bitmap dimension.
fn edge(pixels: u32) -> io::Result<usize> {
    usize::try_from(u64::from(pixels)).map_err(|_| io::Error::other("capture region is oversized"))
}

/// A pinned, zeroed buffer for `width x height` BGRA frames.
fn frame_buffer(width: usize, height: usize) -> io::Result<*mut u8> {
    let bytes = width
        .checked_mul(4)
        .and_then(|row| row.checked_mul(height))
        .ok_or_else(|| io::Error::other("capture region is oversized"))?;
    let mut buffer = std::vec![0u8; bytes];
    let raw = buffer.as_mut_ptr();
    std::mem::forget(buffer); // ownership moves to the pinned pointer
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::declare_opaque;

    #[test]
    fn the_alpha_byte_is_repaired() {
        // Three BGRA pixels with the server's zeroed alpha channel.
        let mut row = [7u8; 12];
        declare_opaque(&mut row);
        assert_eq!(row, [7, 7, 7, 255, 7, 7, 7, 255, 7, 7, 7, 255]);
    }
}
