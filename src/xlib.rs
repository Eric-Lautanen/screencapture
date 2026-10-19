//! X11 C surface (`libX11`, loaded by soname): signatures verified against
//! the libX11 `Xlib.h` and the xorgproto `X.h` headers (xorgproto moved
//! `X.h` out of libX11's tree, which is why the constants come from
//! `xorgproto/include/X11/X.h`).

use std::ffi::{c_char, c_int, c_ulong, c_void};
use std::io;
use std::mem;

use crate::dylib;

/// The X library soname every desktop distribution provides.
const LIBRARY: &str = "libX11.so.6";

/// `ZPixmap` (`X.h`): one packed pixel unit per addressable byte set with
/// no plane separation — the layout 32-bit desktops deliver.
pub const FORMAT_Z: i32 = 2;
/// `LSBFirst` (`X.h`): least-significant byte first, the standard little-
/// endian pixel unit order; true-color servers answer images in it.
pub const ORDER_LSB: i32 = 0;
/// Pixel bits per unit of a native-color `ZPixmap` (Xlib `bits_per_pixel`).
pub const BITS_PER_PIXEL: i32 = 32;

/// `X11` connection handle (`Display *`).
pub type Display = *mut c_void;
/// `Window` / `Drawable` (`X.h`, `XID`).
pub type Window = c_ulong;
/// `XImage *`.
pub type Image = *mut XImage;

/// The `XImage` record (`Xlib.h`), field-for-field; only `data`, `f`, and
/// the layout probes are read or written by this project.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct XImage {
    /// Image dimensions in pixels.
    pub width: c_int,
    /// Image height in pixels.
    pub height: c_int,
    /// Number of pixels skipped in the x direction.
    pub xoffset: c_int,
    /// `XYPixmap`, `XYBitmap`, or `ZPixmap`.
    pub format: c_int,
    /// The pixel data.
    pub data: *mut c_char,
    /// `LSBFirst` or `MSBFirst`.
    pub byte_order: c_int,
    /// Scanline quantum, 8, 16, or 32.
    pub bitmap_unit: c_int,
    /// `LSBFirst` or `MSBFirst` for bit order.
    pub bitmap_bit_order: c_int,
    /// Scanline pad, 8, 16, or 32.
    pub bitmap_pad: c_int,
    /// Depth of the image.
    pub depth: c_int,
    /// Accelerator value: bytes per scanline.
    pub bytes_per_line: c_int,
    /// Bits per pixel unit (`ZPixmap`).
    pub bits_per_pixel: c_int,
    /// Channel masks as the wire layout reports them.
    pub red_mask: c_ulong,
    /// Green channel mask.
    pub green_mask: c_ulong,
    /// Blue channel mask.
    pub blue_mask: c_ulong,
    /// Object hook (`XPointer`).
    pub obdata: *mut c_void,
    /// The `f` function table (`Xlib.h` `struct funcs`); entry 1 is
    /// `destroy_image`.
    pub funcs: [*mut c_void; 6],
}

type OpenDisplayProc = unsafe extern "C" fn(*const c_char) -> Display;
type CloseDisplayProc = unsafe extern "C" fn(Display) -> c_int;
type ScreenNumberProc = unsafe extern "C" fn(Display) -> c_int;
type SizeProc = unsafe extern "C" fn(Display, c_int) -> c_int;
type RootWindowProc = unsafe extern "C" fn(Display) -> Window;
type GetImageProc =
    unsafe extern "C" fn(Display, Window, i32, i32, u32, u32, c_ulong, i32) -> Image;

/// One `XGetImage` parameter bundle, collapsed so the api call stays
/// under the argument budget.
#[derive(Clone, Copy)]
pub struct ImageRequest {
    /// The drawable to read pixels from (the root window here).
    pub drawable: Window,
    /// Top-left corner, relative to the drawable.
    pub x: i32,
    pub y: i32,
    /// Extent in pixels.
    pub width: u32,
    pub height: u32,
    /// Requested plane set (`AllPlanes` = unsigned long high bits).
    pub planes: u64,
    /// Pixmap format (`ZPixmap` for packed pixels).
    pub format: i32,
}

/// `libX11` exports usable by both the capture backend and the screen
/// module. Loading resolves every symbol up front, so a running X server
/// with a crippled libX11 fails at construction instead of mid-frame.
pub struct Api {
    /// `XOpenDisplay`.
    pub open_display: OpenDisplayProc,
    /// `XCloseDisplay`.
    pub close_display: CloseDisplayProc,
    /// `XDefaultScreen`.
    pub default_screen: ScreenNumberProc,
    /// `XDisplayWidth`.
    pub display_width: SizeProc,
    /// `XDisplayHeight`.
    pub display_height: SizeProc,
    /// `XDefaultRootWindow`.
    pub root_window: RootWindowProc,
    /// `XGetImage`.
    pub get_image: GetImageProc,
}

impl Api {
    /// Load `libX11` and resolve every export. The library handle is
    /// parked loaded for the process lifetime (same policy as the
    /// Windows dxgi loader), so the resolved pointers stay valid.
    ///
    /// # Errors
    /// The library is absent (no X environment installed) or dropped an
    /// expected export.
    pub fn load() -> io::Result<Self> {
        let library = dylib::Library::open(LIBRARY)?;
        let api = Self {
            // SAFETY: each export exists in libX11 with exactly the
            // signature transcribed from Xlib.h above; every transmute
            // below re-types a valid function address.
            open_display: symbol(&library, b"XOpenDisplay\0")?,
            close_display: symbol(&library, b"XCloseDisplay\0")?,
            default_screen: symbol(&library, b"XDefaultScreen\0")?,
            display_width: symbol(&library, b"XDisplayWidth\0")?,
            display_height: symbol(&library, b"XDisplayHeight\0")?,
            root_window: symbol(&library, b"XDefaultRootWindow\0")?,
            get_image: symbol(&library, b"XGetImage\0")?,
        };
        // The handle stays loaded for the process lifetime, exactly like
        // the Windows dxgi loader; closing it would invalidate the
        // function pointers above.
        std::mem::forget(library);
        Ok(api)
    }

    /// Open the connection named by `DISPLAY` (or the default).
    ///
    /// # Errors
    /// No X server answers.
    pub fn open(&self) -> io::Result<Display> {
        // SAFETY: null selects the `DISPLAY` default; the connection is
        // closed through `close_display` in the caller's `Drop`.
        let display = unsafe { (self.open_display)(std::ptr::null()) };
        if display.is_null() {
            return Err(io::Error::other("no X server answers DISPLAY"));
        }
        Ok(display)
    }

    /// Close a connection opened by [`Self::open`]; the server sends
    /// any closing errors as X protocol errors, not a return value.
    pub fn close(&self, display: Display) {
        // SAFETY: the connection came from our own `XOpenDisplay`.
        unsafe { (self.close_display)(display) };
    }

    /// Default root window: the drawable spanning the default screen's
    /// monitors on screens configured as one.
    pub fn root(&self, display: Display) -> Window {
        // SAFETY: a read-only lookup on a live connection.
        unsafe { (self.root_window)(display) }
    }

    /// One image of a drawable rectangle; free it with `destroy_image`.
    pub fn image(&self, display: Display, request: ImageRequest) -> Image {
        let planes = c_ulong::try_from(request.planes).unwrap_or(c_ulong::MAX);
        // SAFETY: every argument is owned by this call, matching the
        // `XGetImage` parameter list transcribed from Xlib.h.
        unsafe {
            (self.get_image)(
                display,
                request.drawable,
                request.x,
                request.y,
                request.width,
                request.height,
                planes,
                request.format,
            )
        }
    }

    /// The default screen's extent in pixels.
    pub fn main_screen(&self, display: Display) -> (u32, u32) {
        // SAFETY: read-only metrics on a live connection.
        let screen = unsafe { (self.default_screen)(display) };
        let width = unsafe { (self.display_width)(display, screen) };
        let height = unsafe { (self.display_height)(display, screen) };
        (pixels(width), pixels(height))
    }
}

/// A non-negative screen metric as a pixel count.
fn pixels(quanta: i32) -> u32 {
    u32::try_from(i64::from(quanta)).unwrap_or(0)
}

/// Free one image through its own function table (the `XDestroyImage`
/// macro lowers to `f.destroy_image`, entry 1 of the table).
///
/// # Safety
/// `image` must be a live, table-intact image from `XGetImage`, released
/// before the X library unloads.
pub unsafe fn destroy_image(image: Image) {
    // SAFETY: entry 1 frees the image's own buffer and record; the
    // deref reads the function table of a live X-managed image.
    let destroy: DestroyProc = unsafe { mem::transmute((*image).funcs[1]) };
    unsafe { destroy(image) };
}

type DestroyProc = unsafe extern "C" fn(Image) -> c_int;

/// Resolve one export as a function pointer with the exact header
/// signature the caller's field declares.
///
/// # Errors
/// The library does not export the name.
fn symbol<F: Copy>(library: &dylib::Library, name: &[u8]) -> io::Result<F> {
    let raw = library.symbol(name)?;
    // SAFETY: `raw` is a live function address; the caller transcribed
    // the import's signature from the library's own headers.
    Ok(unsafe { mem::transmute_copy(&raw) })
}
