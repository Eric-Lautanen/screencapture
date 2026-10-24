//! Windows clipboard handoff for still photos: one capture goes out as
//! both `CF_DIB` (the shape office suites, Paint, and most editors paste)
//! and a registered `PNG` format (the shape browsers and chat clients
//! prefer), so the photo can be pasted anywhere without opening a file.
//!
//! The bitmap is built from the frame the photo was encoded from, so the
//! clipboard and the PNG on disk are the same pixels; the photo pipeline
//! never fails because the clipboard did.

use std::ffi::c_void;
use std::io;
use std::time::Duration;

use crate::pin_layout;

/// `CF_DIB` (winuser.h): a `BITMAPINFOHEADER` followed by pixel rows.
const CF_DIB: u32 = 8;
/// `GMEM_MOVEABLE` (winbase.h): clipboard blocks are handed over as
/// moveable global memory the system takes ownership of.
const GMEM_MOVEABLE: u32 = 0x0002;
/// `BI_RGB` (wingdi.h): uncompressed, bottom-up rows.
const BI_RGB: u32 = 0;
/// `BITMAPINFOHEADER` bytes, `biSize` included.
const BITMAPINFOHEADER_BYTES: usize = 40;
/// Bytes per pixel in the frame the capture backends produce.
const PIXEL_BYTES: usize = 4;
/// How many times to ask for the clipboard: another application may hold
/// it open for a moment, and losing a paste over that would be needless.
const OPEN_ATTEMPTS: u32 = 10;
/// Pause between clipboard attempts (milliseconds).
const RETRY_MS: u64 = 20;
/// The registered clipboard format name for PNG, UTF-16 with a nul.
const PNG_FORMAT: [u16; 4] = [0x0050, 0x004E, 0x0047, 0];

#[link(name = "user32")]
unsafe extern "system" {
    fn OpenClipboard(owner: *mut c_void) -> i32;
    fn EmptyClipboard() -> i32;
    fn CloseClipboard() -> i32;
    fn SetClipboardData(format: u32, data: *mut c_void) -> *mut c_void;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
    fn GlobalLock(block: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(block: *mut c_void) -> i32;
    fn GlobalFree(block: *mut c_void) -> *mut c_void;
}

/// `BITMAPINFOHEADER` (wingdi.h): the DIB header `CF_DIB` starts with.
#[repr(C)]
#[derive(Clone, Copy)]
struct BitmapInfoHeader {
    /// Size of this header in bytes.
    size: u32,
    /// Width in pixels.
    width: i32,
    /// Height in pixels; positive means the rows run bottom-up.
    height: i32,
    /// Color planes; always 1.
    planes: u16,
    /// Bits per pixel; 32 keeps the alpha byte the frame already has.
    bit_count: u16,
    /// Compression: [`BI_RGB`] for plain rows.
    compression: u32,
    /// Pixel bytes; exact for the uncompressed form below.
    size_image: u32,
    /// Horizontal resolution (pixels per meter, informational).
    x_pels: i32,
    /// Vertical resolution (pixels per meter, informational).
    y_pels: i32,
    /// Palette entries used; 0 for a true-color bitmap.
    clr_used: u32,
    /// Palette entries required; 0 for a true-color bitmap.
    clr_important: u32,
}

// Bitmap headers are read by the OS and by the pasting application out of
// process, so the ABI layout is pinned exactly.
pin_layout!(BitmapInfoHeader, 40; size = 0, width = 4, height = 8,
    planes = 12, bit_count = 14, compression = 16, size_image = 20,
    x_pels = 24, y_pels = 28, clr_used = 32, clr_important = 36);

/// Put one captured photo on the clipboard: a 32-bit DIB built from the
/// captured frame, plus the finished PNG when its bytes are available.
///
/// The bitmap is the guarantee (every pasting application reads it); the
/// PNG flavour is the richer extra for browsers and chat clients, so its
/// refusal is not worth failing the handoff that already worked.
///
/// # Errors
/// Fails when the clipboard stays locked, a block cannot be allocated, or
/// Windows refuses the bitmap. The caller reports it and keeps the file.
pub(crate) fn copy_image(
    width: u32,
    height: u32,
    bgra: &[u8],
    png: Option<&[u8]>,
) -> io::Result<()> {
    let bitmap = dib(width, height, bgra)?;
    let _clipboard = Open::take()
        .ok_or_else(|| io::Error::other("another application is holding the clipboard open"))?;
    set_data(CF_DIB, &bitmap)?;
    if let Some(png) = png {
        // SAFETY: a static, nul-terminated UTF-16 name; the answer is a
        // format id (0 when the registration failed, where the PNG copy
        // is skipped and the DIB above still stands).
        let format = unsafe { RegisterClipboardFormatW(PNG_FORMAT.as_ptr()) };
        if format != 0 {
            // Deliberately not fatal: the bitmap above is already on the
            // clipboard, so a refused second flavour costs nothing a
            // paste would notice.
            let _ = set_data(format, png);
        }
    }
    Ok(())
}

/// An open, emptied clipboard; `CloseClipboard` runs on drop so no error
/// path can leave the system clipboard locked.
struct Open;

impl Open {
    /// Empty the clipboard into this process's hands; `None` when another
    /// application held it for the whole retry window.
    fn take() -> Option<Self> {
        for attempt in 0..OPEN_ATTEMPTS {
            // SAFETY: parameterless clipboard borrow, closed on drop.
            let opened = unsafe { OpenClipboard(std::ptr::null_mut()) } != 0;
            if opened {
                // SAFETY: the clipboard is ours; setting a format requires
                // clearing the previous contents first.
                if unsafe { EmptyClipboard() } != 0 {
                    return Some(Self);
                }
                // SAFETY: symmetric close of the borrow above.
                unsafe { CloseClipboard() };
                return None;
            }
            if attempt + 1 < OPEN_ATTEMPTS {
                std::thread::sleep(Duration::from_millis(RETRY_MS));
            }
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: the borrow this guard took in `take`.
        unsafe { CloseClipboard() };
    }
}

/// Hand a copy of `bytes` to the clipboard under `format`; the system
/// owns the block once `SetClipboardData` accepts it.
fn set_data(format: u32, bytes: &[u8]) -> io::Result<()> {
    // SAFETY: a plain global allocation of the requested size.
    let block = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
    if block.is_null() {
        return Err(io::Error::other("the clipboard block was refused"));
    }
    // SAFETY: `block` is ours until the handover below.
    let locked = unsafe { GlobalLock(block) };
    if locked.is_null() {
        // SAFETY: releasing the allocation this call made.
        unsafe { GlobalFree(block) };
        return Err(io::Error::other("the clipboard block could not be locked"));
    }
    // SAFETY: the block holds exactly `bytes.len()` writable bytes, and
    // the source slice is that long.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), locked.cast::<u8>(), bytes.len()) };
    // SAFETY: unlocks the lock taken above.
    unsafe { GlobalUnlock(block) };
    // SAFETY: the block is fully written and handed over here; on refusal
    // it is still ours and released, on success the system owns it.
    if unsafe { SetClipboardData(format, block) }.is_null() {
        // SAFETY: releasing the allocation the refusal left with us.
        unsafe { GlobalFree(block) };
        return Err(io::Error::other("the clipboard refused the image"));
    }
    Ok(())
}

/// A `BITMAPINFOHEADER` plus the frame's rows in DIB order (bottom-up,
/// 32 bits per pixel), with every alpha byte forced opaque.
///
/// Both capture routes leave the alpha byte at zero, and a fully
/// transparent DIB pastes as a blank image in applications that honor
/// it, so the copy writes `0xFF` over that column.
fn dib(width: u32, height: u32, bgra: &[u8]) -> io::Result<Vec<u8>> {
    let stride = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(PIXEL_BYTES))
        .ok_or_else(|| io::Error::other("the photo is too wide for a bitmap"))?;
    let rows = usize::try_from(height).map_err(|_| io::Error::other("the photo is too tall"))?;
    let pixels = stride
        .checked_mul(rows)
        .ok_or_else(|| io::Error::other("the photo is too large for a bitmap"))?;
    if stride == 0 || rows == 0 || bgra.len() < pixels {
        return Err(io::Error::other("the photo frame does not fill its bitmap"));
    }
    let header = BitmapInfoHeader {
        size: u32::try_from(BITMAPINFOHEADER_BYTES).expect("header size fits u32"),
        width: i32::try_from(width).map_err(|_| io::Error::other("the photo is too wide"))?,
        height: i32::try_from(height).map_err(|_| io::Error::other("the photo is too tall"))?,
        planes: 1,
        bit_count: 32,
        compression: BI_RGB,
        size_image: u32::try_from(pixels)
            .map_err(|_| io::Error::other("the photo is too large for a bitmap"))?,
        x_pels: 0,
        y_pels: 0,
        clr_used: 0,
        clr_important: 0,
    };
    let mut out = header_bytes(&header).to_vec();
    out.resize(BITMAPINFOHEADER_BYTES + pixels, 0);
    for row in 0..rows {
        let source = &bgra[row * stride..(row + 1) * stride];
        let target = rows - 1 - row;
        let start = BITMAPINFOHEADER_BYTES + target * stride;
        let line = &mut out[start..start + stride];
        line.copy_from_slice(source);
        for alpha in line.iter_mut().skip(3).step_by(PIXEL_BYTES) {
            *alpha = 0xFF;
        }
    }
    Ok(out)
}

/// The header's bytes as the OS will read them.
fn header_bytes(header: &BitmapInfoHeader) -> &[u8] {
    // SAFETY: the struct is a plain pinned C layout of
    // `BITMAPINFOHEADER_BYTES` bytes with no padding or pointers, so its
    // whole extent is readable as bytes.
    unsafe {
        std::slice::from_raw_parts(
            std::ptr::from_ref(header).cast::<u8>(),
            BITMAPINFOHEADER_BYTES,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{BITMAPINFOHEADER_BYTES, dib};

    /// One 2x2 frame, top row red then green, bottom row blue then white;
    /// every alpha byte left at zero, exactly as the capture backends
    /// hand the frame over.
    fn frame() -> Vec<u8> {
        vec![
            0, 0, 255, 0, // red
            0, 255, 0, 0, // green
            255, 0, 0, 0, // blue
            255, 255, 255, 0, // white
        ]
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
    }

    #[test]
    fn header_pins_the_bitmap_shape() {
        let bitmap = dib(2, 2, &frame()).expect("bitmap");
        assert_eq!(bitmap.len(), BITMAPINFOHEADER_BYTES + 16);
        assert_eq!(u32_at(&bitmap, 0), 40); // biSize
        assert_eq!(u32_at(&bitmap, 4), 2); // biWidth
        assert_eq!(u32_at(&bitmap, 8), 2); // biHeight (positive: bottom-up)
        assert_eq!(u16::from_le_bytes([bitmap[12], bitmap[13]]), 1); // biPlanes
        assert_eq!(u16::from_le_bytes([bitmap[14], bitmap[15]]), 32); // bits
        assert_eq!(u32_at(&bitmap, 16), 0); // BI_RGB
        assert_eq!(u32_at(&bitmap, 20), 16); // biSizeImage
    }

    #[test]
    fn rows_flip_to_bottom_up_and_alpha_turns_opaque() {
        let bitmap = dib(2, 2, &frame()).expect("bitmap");
        let pixels = &bitmap[BITMAPINFOHEADER_BYTES..];
        // The last frame row (blue, white) leads a bottom-up DIB.
        assert_eq!(&pixels[0..4], &[255, 0, 0, 0xFF]);
        assert_eq!(&pixels[4..8], &[255, 255, 255, 0xFF]);
        // Then the first frame row (red, green).
        assert_eq!(&pixels[8..12], &[0, 0, 255, 0xFF]);
        assert_eq!(&pixels[12..16], &[0, 255, 0, 0xFF]);
    }

    #[test]
    fn short_frames_and_empty_regions_are_refused() {
        assert!(dib(2, 2, &frame()[..15]).is_err());
        assert!(dib(0, 2, &[]).is_err());
        assert!(dib(2, 0, &[]).is_err());
    }
}
