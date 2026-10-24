//! Win32 ABI shapes of the DXGI/D3D11 capture contracts, hand-copied from
//! the system headers (WIDL's `dxgiformat`/`dxgitype`/`dxgi1_2`): GUIDs,
//! value tables, and the record layouts the interfaces accept.
//!
//! Everything here is plain data; the v-table shapes live in `abi.rs` and
//! the call protocol in `com.rs`.

use crate::pin_layout;
use std::ffi::c_void;

use crate::winapi::{Point, Rect};

/// HRESULT bits live in an unsigned potential; `HRESULT` keeps them
/// sign-extended like every API in the family stores them.
const fn hresult(raw: u32) -> i32 {
    i32::from_ne_bytes(raw.to_ne_bytes())
}

// ---------------------------------------------------------------------------
// Value tables
// ---------------------------------------------------------------------------

/// `HRESULT` for "it worked".
pub const S_OK: i32 = 0;
/// `DXGI_ERROR_NOT_FOUND`: no more outputs on the adapter.
pub const NOT_FOUND: i32 = hresult(0x887A_0002);
/// `DXGI_ERROR_ACCESS_LOST`: the duplication session is invalid.
pub const ACCESS_LOST: i32 = hresult(0x887A_0026);
/// `DXGI_ERROR_WAIT_TIMEOUT`: no new desktop frame during the wait.
pub const WAIT_TIMEOUT: i32 = hresult(0x887A_0027);
/// `DXGI_ERROR_NOT_CURRENTLY_AVAILABLE`: another stream owns duplication.
pub const NOT_CURRENTLY_AVAILABLE: i32 = hresult(0x887A_0028);

/// `DXGI_FORMAT_B8G8R8A8_UNORM`: the capture layout, byte per BGRA.
pub const B8G8R8A8: u32 = 0x57;
/// `DXGI_MODE_ROTATION_UNSPECIFIED` with `..._IDENTITY`: plain layouts.
pub const ROTATION_PLAIN: [u32; 2] = [0, 1];

/// `D3D11_USAGE_STAGING`: CPU-readable intermediate pooling.
pub const STAGING: u32 = 3;
/// `D3D11_CPU_ACCESS_READ`.
pub const CPU_READ: u32 = 0x0002_0000;
/// `D3D11_MAP_READ`.
pub const MAP_READ: u32 = 1;

// ---------------------------------------------------------------------------
// GUIDs
// ---------------------------------------------------------------------------

/// `DEFINE_GUID` layout for `REFGUID`, pinned once in [`crate::winapi`].
pub use crate::winapi::Guid;

impl Guid {
    /// Build a GUID from the familiar hex groups.
    const fn new(a: u32, b: u16, c: u16, d: [u8; 8]) -> Self {
        Self { a, b, c, d }
    }
}

/// `IID_IDXGIFactory1` (dxgi.h).
pub const IDXGI_FACTORY1: Guid = Guid::new(
    0x770a_ae78,
    0xf26f,
    0x4dba,
    [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
);
/// `IID_IDXGIOutput1` (`dxgi1_2.h`).
pub const IDXGI_OUTPUT1: Guid = Guid::new(
    0x00cd_dea8,
    0x939b,
    0x4b83,
    [0xa3, 0x40, 0xa6, 0x85, 0x22, 0x66, 0x66, 0xcc],
);
/// `IID_ID3D11Texture2D` (d3d11.h).
pub const ID3D_TEXTURE2D: Guid = Guid::new(
    0x6f15_aaf2,
    0xd208,
    0x4e89,
    [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
);

// ---------------------------------------------------------------------------
// Struct layouts (match the Win32 ABI exactly)
// ---------------------------------------------------------------------------

/// `DXGI_SAMPLE_DESC`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SampleDesc {
    /// Sample count.
    pub count: u32,
    /// Quality hint.
    pub quality: u32,
}

/// `DXGI_RATIONAL`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Rational {
    /// Numerator.
    pub numerator: u32,
    /// Denominator.
    pub denominator: u32,
}

/// `DXGI_MODE_DESC`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ModeDesc {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Refresh cadence.
    pub refresh: Rational,
    /// Pixel format (`DXGI_FORMAT_*`).
    pub format: u32,
    /// `DXGI_MODE_SCANLINE_ORDER_*`.
    pub scanline: u32,
    /// `DXGI_MODE_SCALING_*`.
    pub scaling: u32,
}

/// `DXGI_OUTPUT_DESC`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OutputDesc {
    /// Adapter display name (`WCHAR[32]`, NUL padded).
    pub device_name: [u16; 32],
    /// Display area inside the virtual desktop.
    pub coordinates: Rect,
    /// Whether the output takes part in the desktop.
    pub attached: i32,
    /// `DXGI_MODE_ROTATION_*`.
    pub rotation: u32,
    /// Monitor handle backing this output.
    pub monitor: *mut c_void,
}

/// `DXGI_OUTDUPL_DESC`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DuplDesc {
    /// Output mode at duplication time.
    pub mode: ModeDesc,
    /// `DXGI_MODE_ROTATION_*`.
    pub rotation: u32,
    /// Whether the desktop image already lives in system memory.
    pub system_memory: i32,
}

/// `DXGI_OUTDUPL_POINTER_POSITION`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PointerPosition {
    /// Screen point, relative to the duplicated output's top-left.
    pub position: Point,
    /// Whether the pointer sits on this output.
    pub visible: i32,
}

/// `DXGI_OUTDUPL_POINTER_SHAPE_INFO` (`dxgi1_2.h`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PointerShapeInfo {
    /// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_*`.
    pub kind: u32,
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels (double for the monochrome pair).
    pub height: u32,
    /// Bytes per row of the shape bitmap.
    pub pitch: u32,
    /// Offset of the click point inside the bitmap.
    pub hotspot: Point,
}

/// `DXGI_OUTDUPL_FRAME_INFO` (the two timestamps are `LARGE_INTEGER`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FrameInfo {
    /// `QueryPerformanceCounter` of the last update, else zero.
    pub last_present: i64,
    /// `QueryPerformanceCounter` of the last mouse-only update.
    pub last_mouse: i64,
    /// Updates folded into one frame.
    pub accumulated: u32,
    /// Updates were squashed into one.
    pub coalesced: i32,
    /// DRM content was masked out.
    pub masked: i32,
    /// Pointer state for this frame.
    pub pointer: PointerPosition,
    /// Bytes of metadata available.
    pub metadata_budget: u32,
    /// Bytes of pointer shape data available.
    pub shape_budget: u32,
}

/// `D3D11_TEXTURE2D_DESC`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Tex2dDesc {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Mip levels (1: no chain).
    pub mips: u32,
    /// Array size (1).
    pub array: u32,
    /// Pixel format (`DXGI_FORMAT_*`).
    pub format: u32,
    /// MSAA setting.
    pub sample: SampleDesc,
    /// Pool (`D3D11_USAGE_*`).
    pub usage: u32,
    /// Pipeline bindings (none for staged reads).
    pub bind: u32,
    /// CPU access (`D3D11_CPU_ACCESS_*`).
    pub cpu: u32,
    /// Miscellaneous resource flags.
    pub misc: u32,
}

/// `D3D11_MAPPED_SUBRESOURCE`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Mapped {
    /// Locked bytes.
    pub data: *mut c_void,
    /// Row length in bytes.
    pub row_pitch: u32,
    /// Depth length in bytes.
    pub depth_pitch: u32,
}

// ---------------------------------------------------------------------------
// Layout pins: these records are addressed by the driver through the raw
// pointers the COM protocol passes, so every byte position is load-bearing.
// ---------------------------------------------------------------------------

pin_layout!(SampleDesc, 8; count = 0, quality = 4);
pin_layout!(Rational, 8; numerator = 0, denominator = 4);
pin_layout!(ModeDesc, 28; width = 0, height = 4, refresh = 8, format = 16,
    scanline = 20, scaling = 24);

// Output: 32-char device name, then a rect, then scalars, then the
// monitor handle whose pointer alignment forces the 88 offset.
pin_layout!(OutputDesc, 96; device_name = 0, coordinates = 64,
    attached = 80, rotation = 84, monitor = 88);

pin_layout!(DuplDesc, 36; mode = 0, rotation = 28, system_memory = 32);
pin_layout!(PointerPosition, 12; position = 0, visible = 8);
pin_layout!(PointerShapeInfo, 24; kind = 0, width = 4, height = 8,
    pitch = 12, hotspot = 16);

// Frame info: two 64-bit timestamps, five scalars, the 12-byte pointer
// record it packs at 28, then the two byte budgets.
pin_layout!(FrameInfo, 48; last_present = 0, last_mouse = 8,
    accumulated = 16, coalesced = 20, masked = 24, pointer = 28,
    metadata_budget = 40, shape_budget = 44);

pin_layout!(Tex2dDesc, 44; width = 0, height = 4, mips = 8, array = 12,
    format = 16, sample = 20, usage = 28, bind = 32, cpu = 36, misc = 40);

// Map call's out-record: the pointer takes the 8-byte slot first, the
// pitches trail it without padding.
pin_layout!(Mapped, 16; data = 0, row_pitch = 8, depth_pitch = 12);
