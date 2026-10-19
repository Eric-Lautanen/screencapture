//! GDI frame capture: one `Target` per recording keeps screen DC, memory
//! DC, and a top-down BGRA DIB alive; frames reuse the cached bitmap.

use std::ffi::c_void;
use std::io;

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

const SRCCOPY: u32 = 0x00CC_0020;
const CAPTUREBLT: u32 = 0x4000_0000;
const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;
const CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;

type Hdc = *mut c_void;
type Hbitmap = *mut c_void;
type Bool = i32;

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDC(window: *mut c_void) -> Hdc;
    fn ReleaseDC(window: *mut c_void, dc: Hdc) -> i32;
    fn SetProcessDpiAwarenessContext(context: *mut c_void) -> Bool;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: Hdc) -> Hdc;
    fn DeleteDC(dc: Hdc) -> Bool;
    fn CreateDIBSection(
        dc: Hdc,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut c_void,
        section: *mut c_void,
        offset: u32,
    ) -> Hbitmap;
    fn SelectObject(dc: Hdc, object: Hbitmap) -> Hbitmap;
    fn DeleteObject(object: Hbitmap) -> Bool;
    fn BitBlt(
        dest: Hdc,
        dest_x: i32,
        dest_y: i32,
        width: i32,
        height: i32,
        source: Hdc,
        source_x: i32,
        source_y: i32,
        raster: u32,
    ) -> Bool;
}

/// `BITMAPINFOHEADER` plus the one color-slot in `BITMAPINFO`.
#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 1],
}

/// `BITMAPINFOHEADER` to the Win32 ABI.
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

/// Opt into per-monitor DPI awareness so screen coordinates are physical
/// pixels; call once at startup, before any geometry work.
pub fn prepare() {
    // SAFETY: passing a well-known negative context constant; failure only
    // means legacy DPI behavior.
    unsafe { SetProcessDpiAwarenessContext(CONTEXT_PER_MONITOR_AWARE_V2 as *mut c_void) };
}

/// A capture target bound to one region.
pub struct Target {
    region: Region,
    screen_dc: Hdc,
    memory_dc: Hdc,
    bitmap: Hbitmap,
    previous_bitmap: Hbitmap,
    bits: *mut c_void,
    stride: usize,
    raster: u32,
}

impl Target {
    /// Capture a frame from `region` into a reusable BGRA bitmap.
    /// `include_cursor` draws the mouse pointer into every frame.
    ///
    /// # Errors
    /// Fails when any GDI handle for the setup is refused.
    pub fn new(region: Region, include_cursor: bool) -> io::Result<Self> {
        if !region.is_sane() {
            return Err(io::Error::other("capture region is empty"));
        }
        // SAFETY: null window DC is the screen DC; released in `Drop`.
        let screen_dc = unsafe { GetDC(std::ptr::null_mut()) };
        if screen_dc.is_null() {
            return Err(io::Error::other("GetDC for the screen failed"));
        }
        let Some(setup) = build_bitmap(screen_dc, region) else {
            let _ = unsafe { ReleaseDC(std::ptr::null_mut(), screen_dc) };
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

    /// Perform one blit and lend the BGRA bytes; layout is top-down rows of
    /// `W * 4` bytes. The data lives in our DIB and is valid until the next
    /// blit, so callers must write it out before capturing again.
    ///
    /// # Errors
    /// Fails when `BitBlt` refuses the blit.
    pub fn frame(&self) -> io::Result<&[u8]> {
        let width = i32::try_from(self.region.width).map_err(invalid_region)?;
        let height = i32::try_from(self.region.height).map_err(invalid_region)?;
        // SAFETY: handles belong to this target; coordinates describe the
        // region recorded at construction.
        let blit_ok = unsafe {
            BitBlt(
                self.memory_dc,
                0,
                0,
                width,
                height,
                self.screen_dc,
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
            &raw const info,
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

impl Drop for Target {
    fn drop(&mut self) {
        // SAFETY: release in reverse creation order; DCs and bitmap are
        // ours exclusively.
        unsafe {
            let _ = SelectObject(self.memory_dc, self.previous_bitmap);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.memory_dc);
            let _ = ReleaseDC(std::ptr::null_mut(), self.screen_dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Region, Target, stride_for};

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

    #[test]
    fn stride_is_bgra_row_bytes() {
        assert_eq!(stride_for(1920), 7680);
    }
}
