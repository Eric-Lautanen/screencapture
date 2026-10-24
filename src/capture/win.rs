//! GDI frame capture: one backend per recording keeps the screen DC,
//! memory DC, and a top-down BGRA DIB alive; frames reuse the cached
//! bitmap. This is the slow-but-always-available Windows path behind
//! desktop duplication.

use crate::guard::ScreenDc;
use crate::pin_layout;
use crate::winapi::{Bool, Hdc};
use std::ffi::c_void;
use std::io;

use super::Region;

const SRCCOPY: u32 = 0x00CC_0020;
const CAPTUREBLT: u32 = 0x4000_0000;
const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;
const CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;

type Hbitmap = *mut c_void;

#[link(name = "user32")]
unsafe extern "system" {
    fn SetProcessDpiAwarenessContext(value: *mut c_void) -> Bool;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: Hdc) -> Hdc;
    fn CreateDIBSection(
        dc: Hdc,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut c_void,
        section: *mut c_void,
        offset: u32,
    ) -> Hbitmap;
    fn DeleteDC(dc: Hdc) -> i32;
    fn DeleteObject(object: Hbitmap) -> i32;
    fn SelectObject(dc: Hdc, object: Hbitmap) -> Hbitmap;
    fn BitBlt(
        destination: Hdc,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source: Hdc,
        source_x: i32,
        source_y: i32,
        operation: u32,
    ) -> i32;
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    pixels_per_meter_x: i32,
    pixels_per_meter_y: i32,
    colors_used: u32,
    colors_important: u32,
}

// `BITMAPINFOHEADER` pins at its wingdi.h size of 40 tail-to-toe.
pin_layout!(BitmapInfoHeader, 40; size = 0, width = 4, height = 8,
    planes = 12, bit_count = 14, compression = 16, size_image = 20,
    pixels_per_meter_x = 24, pixels_per_meter_y = 28, colors_used = 32,
    colors_important = 36);

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 1],
}

// `BITMAPINFO` = header + the one-entry palette the struct type promises.
pin_layout!(BitmapInfo, 44; header = 0, colors = 40);

/// Opt into per-monitor DPI awareness so screen coordinates are physical
/// pixels; call once at startup, before any geometry work.
pub fn prepare_dpi() {
    // SAFETY: passing a well-known negative context constant; failure only
    // means legacy DPI behavior.
    unsafe { SetProcessDpiAwarenessContext(CONTEXT_PER_MONITOR_AWARE_V2 as *mut c_void) };
}

/// One GDI capture backend bound to one region.
pub struct Backend {
    region: Region,
    /// Desktop DC borrowed for the backend's lifetime and released by its
    /// own guard, so no manual `ReleaseDC` can be skipped.
    screen_dc: ScreenDc,
    memory_dc: Hdc,
    bitmap: Hbitmap,
    previous_bitmap: Hbitmap,
    bits: *mut c_void,
    stride: usize,
    raster: u32,
}

impl Backend {
    /// Set up the blit chain for `region`; a refused handle fails here
    /// rather than mid-recording.
    ///
    /// # Errors
    /// Fails when any GDI handle for the setup is refused.
    pub fn open(region: Region, include_cursor: bool) -> io::Result<Self> {
        let Some(screen_dc) = ScreenDc::desktop() else {
            return Err(io::Error::other("GetDC for the screen failed"));
        };
        let Some(setup) = build_bitmap(screen_dc.dc(), region) else {
            return Err(io::Error::other("GDI bitmap setup failed"));
        };
        Ok(Self {
            region,
            screen_dc,
            memory_dc: setup.memory_dc,
            bitmap: setup.bitmap,
            previous_bitmap: setup.previous_bitmap,
            bits: setup.bits,
            stride: stride_for(region.width),
            raster: raster_op(include_cursor),
        })
    }

    /// Perform one blit and lend the BGRA bytes; layout is top-down rows
    /// of `width * 4` bytes. The data lives in our DIB and is valid until
    /// the next blit.
    ///
    /// # Errors
    /// Fails when `BitBlt` refuses the blit.
    pub fn frame(&self) -> io::Result<&[u8]> {
        let width = i32::try_from(self.region.width).map_err(invalid_region)?;
        let height = i32::try_from(self.region.height).map_err(invalid_region)?;
        // SAFETY: handles belong to this backend; coordinates describe
        // the region recorded at construction.
        let blit_ok = unsafe {
            BitBlt(
                self.memory_dc,
                0,
                0,
                width,
                height,
                self.screen_dc.dc(),
                self.region.x,
                self.region.y,
                self.raster,
            )
        };
        if blit_ok == 0 {
            return Err(io::Error::other("BitBlt refused the capture"));
        }
        let len = self.stride * usize::try_from(self.region.height).unwrap_or(0);
        if self.bits.is_null() {
            return Err(io::Error::other("frame buffer is unavailable"));
        }
        Ok(unsafe { std::slice::from_raw_parts(self.bits.cast::<u8>(), len) })
    }
}

fn invalid_region(_error: std::num::TryFromIntError) -> io::Error {
    io::Error::other("capture region does not fit Win32 blit limits")
}

/// `CAPTUREBLT` pulls the mouse pointer and layered windows into blits.
fn raster_op(include_cursor: bool) -> u32 {
    if include_cursor {
        SRCCOPY | CAPTUREBLT
    } else {
        SRCCOPY
    }
}

struct Setup {
    memory_dc: Hdc,
    bitmap: Hbitmap,
    previous_bitmap: Hbitmap,
    bits: *mut c_void,
}

fn build_bitmap(screen_dc: Hdc, region: Region) -> Option<Setup> {
    let width = i32::try_from(region.width).ok()?;
    let height = i32::try_from(region.height).ok()?;
    let info = BitmapInfo {
        header: BitmapInfoHeader {
            size: u32::try_from(std::mem::size_of::<BitmapInfoHeader>()).unwrap_or(0),
            width,
            height: -height, // negative: top-down rows
            planes: 1,
            bit_count: 32,
            compression: BI_RGB,
            size_image: 0,
            pixels_per_meter_x: 0,
            pixels_per_meter_y: 0,
            colors_used: 0,
            colors_important: 0,
        },
        colors: [0],
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: `info` is a complete, zero-except-uninitialized-color-table
    // descriptor; `bits` receives the shared-memory pointer.
    let memory_dc = unsafe { CreateCompatibleDC(screen_dc) };
    if memory_dc.is_null() {
        return None;
    }
    let bitmap = unsafe {
        CreateDIBSection(
            memory_dc,
            std::ptr::from_ref(&info),
            DIB_RGB_COLORS,
            &raw mut bits,
            std::ptr::null_mut(),
            0,
        )
    };
    if bitmap.is_null() {
        // SAFETY: the memory DC is ours exclusively.
        let _ = unsafe { DeleteDC(memory_dc) };
        return None;
    }
    if bits.is_null() {
        // SAFETY: exclusive handles.
        let _ = unsafe { DeleteObject(bitmap) };
        let _ = unsafe { DeleteDC(memory_dc) };
        return None;
    }
    // SAFETY: selecting our bitmap into our memory DC.
    let previous_bitmap = unsafe { SelectObject(memory_dc, bitmap) };
    Some(Setup {
        memory_dc,
        bitmap,
        previous_bitmap,
        bits,
    })
}

fn stride_for(width: u32) -> usize {
    usize::try_from(width).unwrap_or(0) * 4
}

impl Drop for Backend {
    fn drop(&mut self) {
        // SAFETY: release in reverse creation order; DCs and bitmap are
        // ours exclusively.
        unsafe {
            let _ = SelectObject(self.memory_dc, self.previous_bitmap);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.memory_dc);
        }
        // The screen DC guard releases itself after this `Drop` body.
    }
}

#[cfg(test)]
mod tests {
    use super::stride_for;

    #[test]
    fn stride_is_bgra_row_bytes() {
        assert_eq!(stride_for(1920), 7680);
    }
}
