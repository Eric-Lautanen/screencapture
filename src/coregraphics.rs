//! CoreGraphics C surface: the framework load, callable signatures, and
//! display geometry shared by the macOS capture backend and screen module.
//! Symbols and constants were transcribed from Apple's SDK headers
//! (`CGBitmapContext.h`, `CGImage.h`, `CGWindow.h`, `CGGeometry.h`,
//! `CGDirectDisplay.h`).

use std::ffi::c_void;
use std::io;
use std::mem;

use crate::dylib;

/// The CoreGraphics framework binary inside the system bundle.
const FRAMEWORK: &str = "/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics";

/// `kCGWindowListOptionOnScreenOnly` (`CGWindow.h`): composed pixels of
/// everything currently on screen.
pub const LIST_ON_SCREEN: u32 = 1;
/// `kCGWindowImageShouldBeOpaque` (`CGWindow.h`): empty areas become white,
/// keeping the premultiplied alpha maximal and colors faithful.
pub const IMAGE_OPAQUE: u32 = 2;
/// `kCGWindowImageBestResolution` (`CGWindow.h`): compose at the display's
/// native scale so a point-space area answers device pixels.
pub const BEST_RESOLUTION: u32 = 8;
/// `kCGNullWindowID` (`CGWindow.h`): no specific window, the region rules.
pub const WINDOW_ALL: u32 = 0;
/// `kCGImageAlphaPremultipliedFirst` (`CGImage.h`).
const ALPHA_PREMULTIPLIED_FIRST: u32 = 2;
/// `kCGBitmapByteOrder32Little` (`CGImage.h`): little-endian 32-bit units,
/// which in alpha-first ordering yields BGRA bytes in memory.
const BYTE_ORDER_32_LITTLE: u32 = 0x2000;

/// Bitmap-context pixel layout for our BGRA frame bytes.
pub const CONTEXT_LAYOUT: u32 = ALPHA_PREMULTIPLIED_FIRST | BYTE_ORDER_32_LITTLE;

/// Opaque CoreGraphics object handle (`CGImageRef`, `CGContextRef`, ...).
pub type Ref = *mut c_void;
/// `CGDirectDisplayID` / `CGWindowID`.
pub type Id = u32;

/// Screen-space rectangle in points; `CGWindow.h` documents this space as
/// having its origin at the top-left of the main display.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Rect {
    /// Top-left origin.
    pub origin: [f64; 2],
    /// Width and height in points.
    pub size: [f64; 2],
}

type WindowListImageProc = unsafe extern "C" fn(Rect, u32, Id, u32) -> Ref;
type BitmapContextProc = unsafe extern "C" fn(
    // data (null lets CG allocate); dimensions and row pitch; color
    // space; pixel layout.
    *mut u8,
    usize,
    usize,
    usize,
    usize,
    Ref,
    u32,
) -> Ref;
type ContextDataProc = unsafe extern "C" fn(Ref) -> *mut u8;
type DrawImageProc = unsafe extern "C" fn(Ref, Rect, Ref);
type TransformProc = unsafe extern "C" fn(Ref, f64, f64);
type StateProc = unsafe extern "C" fn(Ref);
type ReleaseProc = unsafe extern "C" fn(Ref);
type ColorSpaceProc = unsafe extern "C" fn() -> Ref;
type MainDisplayProc = unsafe extern "C" fn() -> Id;
type DisplayBoundsProc = unsafe extern "C" fn(Id) -> Rect;
type PixelCountProc = unsafe extern "C" fn(Id) -> usize;

/// Every export used by the macOS backends; each signature matches the
/// SDK header exactly. One load per construction, closed on drop.
pub struct Api {
    /// `CGWindowListCreateImage`.
    pub window_list_image: WindowListImageProc,
    /// `CGBitmapContextCreate`.
    pub bitmap_context: BitmapContextProc,
    /// `CGBitmapContextGetData`.
    pub context_data: ContextDataProc,
    /// `CGContextRelease`.
    pub context_release: ReleaseProc,
    /// `CGContextDrawImage`.
    pub draw_image: DrawImageProc,
    /// `CGContextTranslateCTM`.
    pub translate: TransformProc,
    /// `CGContextScaleCTM`.
    pub scale: TransformProc,
    /// `CGContextSaveGState`.
    pub save_state: StateProc,
    /// `CGContextRestoreGState`.
    pub restore_state: StateProc,
    /// `CGImageRelease`.
    pub image_release: ReleaseProc,
    /// `CGColorSpaceCreateDeviceRGB`.
    pub color_space: ColorSpaceProc,
    /// `CGColorSpaceRelease`.
    pub color_space_release: ReleaseProc,
    /// `CGMainDisplayID`.
    pub main_display: MainDisplayProc,
    /// `CGDisplayBounds`.
    pub display_bounds: DisplayBoundsProc,
    /// `CGDisplayPixelsWide`.
    pub display_pixels_wide: PixelCountProc,
    /// `CGDisplayPixelsHigh`.
    pub display_pixels_high: PixelCountProc,
}

impl Api {
    /// Load CoreGraphics and resolve every export. The library handle is
    /// parked loaded for the process lifetime (same policy as the
    /// Windows dxgi loader), so the resolved pointers stay valid.
    ///
    /// # Errors
    /// The framework is absent or dropped an expected export.
    pub fn load() -> io::Result<Self> {
        let library = dylib::Library::open(FRAMEWORK)?;
        let api = Self {
            // SAFETY: each export exists in CoreGraphics with exactly the
            // signature transcribed from the SDK headers above; every
            // transmute below re-types a valid function address.
            window_list_image: symbol(&library, b"CGWindowListCreateImage\0")?,
            bitmap_context: symbol(&library, b"CGBitmapContextCreate\0")?,
            context_data: symbol(&library, b"CGBitmapContextGetData\0")?,
            context_release: symbol(&library, b"CGContextRelease\0")?,
            draw_image: symbol(&library, b"CGContextDrawImage\0")?,
            translate: symbol(&library, b"CGContextTranslateCTM\0")?,
            scale: symbol(&library, b"CGContextScaleCTM\0")?,
            save_state: symbol(&library, b"CGContextSaveGState\0")?,
            restore_state: symbol(&library, b"CGContextRestoreGState\0")?,
            image_release: symbol(&library, b"CGImageRelease\0")?,
            color_space: symbol(&library, b"CGColorSpaceCreateDeviceRGB\0")?,
            color_space_release: symbol(&library, b"CGColorSpaceRelease\0")?,
            main_display: symbol(&library, b"CGMainDisplayID\0")?,
            display_bounds: symbol(&library, b"CGDisplayBounds\0")?,
            display_pixels_wide: symbol(&library, b"CGDisplayPixelsWide\0")?,
            display_pixels_high: symbol(&library, b"CGDisplayPixelsHigh\0")?,
        };
        // The handle is parked loaded for the process lifetime, exactly
        // like the Windows dxgi loader; closing it would invalidate the
        // function pointers above.
        mem::forget(library);
        Ok(api)
    }
    /// An owned device-RGB color space; release it with
    /// [`Self::release_color_space`].
    ///
    /// # Errors
    /// The framework refused the allocation.
    pub fn device_color_space(&self) -> io::Result<Ref> {
        // SAFETY: parameterless allocation; released through
        // `release_color_space` or in the caller's `Drop`.
        let space = unsafe { (self.color_space)() };
        if space.is_null() {
            return Err(io::Error::other("the device RGB space is missing"));
        }
        Ok(space)
    }

    /// Release a color space from [`Self::device_color_space`].
    pub fn release_color_space(&self, space: Ref) {
        // SAFETY: a live object this API allocated itself.
        unsafe { (self.color_space_release)(space) };
    }

    /// A bitmap context of `width` x `height` premultiplied BGRA rows
    /// (`width * 4` bytes per row, top-down); release it with
    /// [`Self::release_context`].
    ///
    /// # Errors
    /// The framework refused the allocation.
    pub fn bg_bitmap_context(&self, width: usize, height: usize, space: Ref) -> io::Result<Ref> {
        // SAFETY: null data makes CG allocate the buffer; layout
        // constants are SDK-verified above; 8 bits per component and one
        // row per `width * 4` bytes.
        let context = unsafe {
            (self.bitmap_context)(
                std::ptr::null_mut(),
                width,
                height,
                8,
                width * 4,
                space,
                CONTEXT_LAYOUT,
            )
        };
        if context.is_null() {
            return Err(io::Error::other("the bitmap context was refused"));
        }
        Ok(context)
    }

    /// Release a context from [`Self::bg_bitmap_context`].
    pub fn release_context(&self, context: Ref) {
        // SAFETY: a live object this API allocated itself.
        unsafe { (self.context_release)(context) };
    }

    /// The data buffer of a bitmap context: width rows of top-down BGRA.
    pub fn bitmap_bits(&self, context: Ref) -> *mut u8 {
        // SAFETY: the context is a live bitmap allocation.
        unsafe { (self.context_data)(context) }
    }

    /// Compose the screen image for `area` into `context` (`width` x
    /// `height`, the context's own size), with rows mirrored so the
    /// buffer reads top-down, then release the image. The image is asked
    /// for best resolution and drawn to fit, so mixed-scale display
    /// stacks resample instead of tearing.
    ///
    /// # Errors
    /// The compositor refused the capture; the context is untouched.
    pub fn draw_screen(
        &self,
        area: Rect,
        context: Ref,
        width: usize,
        height: usize,
    ) -> io::Result<()> {
        // SAFETY: constant window-list options from the SDK headers; the
        // returned image is owned per Apple's Create/Copy rule.
        let options = IMAGE_OPAQUE | BEST_RESOLUTION;
        let image = unsafe { (self.window_list_image)(area, LIST_ON_SCREEN, WINDOW_ALL, options) };
        if image.is_null() {
            return Err(io::Error::other("the compositor refused the capture"));
        }
        // SAFETY: Quartz draws with a bottom-left origin, so mirror the
        // context vertically — translate to the bottom edge, scale y by
        // -1, draw the whole rect, restore the CTM — and release the
        // image per the Create rule above. `self_*` are context-sized
        // and ceil below a few thousand, so the `u32` fits exactly.
        unsafe {
            (self.save_state)(context);
            (self.translate)(context, 0.0, points(u32::try_from(height).unwrap_or(0)));
            (self.scale)(context, 1.0, -1.0);
            (self.draw_image)(
                context,
                Rect {
                    origin: [0.0, 0.0],
                    size: [
                        points(u32::try_from(width).unwrap_or(0)),
                        points(u32::try_from(height).unwrap_or(0)),
                    ],
                },
                image,
            );
            (self.restore_state)(context);
            (self.image_release)(image);
        }
        Ok(())
    }

    /// Main display: id, screen-space bounds in points, and the scale
    /// factor that turns points into physical pixels.
    pub fn main_geometry(&self) -> Option<(Id, Rect, f64)> {
        // SAFETY: read-only metrics for the shared main display id.
        let id = unsafe { (self.main_display)() };
        if id == 0 {
            return None;
        }
        // SAFETY: read-only geometry lookup.
        let bounds = unsafe { (self.display_bounds)(id) };
        let points = bounds.size[0];
        if points <= 0.0 {
            return None;
        }
        // SAFETY: read-only metric.
        let pixels = i64::try_from(unsafe { (self.display_pixels_wide)(id) }).ok()?;
        let count = u32::try_from(pixels).ok()?;
        if count == 0 {
            return None;
        }
        Some((id, bounds, ratio(count, points)))
    }

    /// Physical pixel height of a display id.
    pub fn pixel_height(&self, id: Id) -> usize {
        // SAFETY: a read-only metric for an id the caller owns.
        unsafe { (self.display_pixels_high)(id) }
    }

    /// Physical pixel width of a display id.
    pub fn pixel_width(&self, id: Id) -> usize {
        // SAFETY: a read-only metric for an id the caller owns.
        unsafe { (self.display_pixels_wide)(id) }
    }
}

/// An `f64` screen coordinate from a bounded pixel dimension.
fn points(pixels: u32) -> f64 {
    f64::from(pixels)
}

/// Pixels over points, as the Retina scale factor.
fn ratio(pixels: u32, points: f64) -> f64 {
    f64::from(pixels) / points.max(1.0)
}

/// Resolve one export as a function pointer with the exact header
/// signature the caller's field declares.
///
/// # Errors
/// The framework does not export the name.
fn symbol<F: Copy>(library: &dylib::Library, name: &[u8]) -> io::Result<F> {
    let raw = library.symbol(name)?;
    // SAFETY: `raw` is a live function address; the caller transcribed
    // the import's signature from the framework's own headers.
    Ok(unsafe { mem::transmute_copy(&raw) })
}
