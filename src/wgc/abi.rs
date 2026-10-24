//! `WinRT` shapes for Windows Graphics Capture: the interfaces the window
//! frame source calls, pinned to the slots the system headers declare.
//!
//! Windows Graphics Capture is a `WinRT` surface, so every interface opens
//! with `IInspectable` — `IUnknown` plus `GetIids`, `GetRuntimeClassName`,
//! and `GetTrustLevel` — before its own methods. Those three slots are
//! declared as reserved placeholders purely to hold that offset, so the
//! ordinals below are the ones `windows.graphics.capture.h` publishes.

use crate::dxgi::abi::{Complete, Reserved, UnknownVt, interfaces};
use crate::dxgi::structs::Guid;
use crate::pin_layout;
use std::ffi::c_void;

interfaces! {
    (
        "`IGraphicsCaptureItem`: `GetSize` at 7.",
        ItemVt: {
            get_iids: Reserved,
            get_runtime_class_name: Reserved,
            get_trust_level: Reserved,
            get_display_name: Reserved,
            get_size: Measure,
        },
    ),
    (
        "`IGraphicsCaptureItemInterop`: `CreateForWindow` at 3.",
        InteropVt: {
            create_for_window: ForWindow,
        },
    ),
    (
        "`IDirect3D11CaptureFramePoolStatics`: `Create` at 6.",
        PoolStaticsVt: {
            get_iids: Reserved,
            get_runtime_class_name: Reserved,
            get_trust_level: Reserved,
            create: MakePool,
        },
    ),
    (
        "`IDirect3D11CaptureFramePool`: `TryGetNextFrame` 7, \
         `CreateCaptureSession` 10.",
        PoolVt: {
            get_iids: Reserved,
            get_runtime_class_name: Reserved,
            get_trust_level: Reserved,
            recreate: Reserved,
            try_get_next_frame: NextFrame,
            add_frame_arrived: Reserved,
            remove_frame_arrived: Reserved,
            create_capture_session: MakeSession,
        },
    ),
    (
        "`IGraphicsCaptureSession`: `StartCapture` at 6.",
        SessionVt: {
            get_iids: Reserved,
            get_runtime_class_name: Reserved,
            get_trust_level: Reserved,
            start_capture: Start,
        },
    ),
    (
        "`IGraphicsCaptureSession2`: the pointer switch, at 6 and 7.",
        SessionCursorVt: {
            get_iids: Reserved,
            get_runtime_class_name: Reserved,
            get_trust_level: Reserved,
            get_is_cursor_capture_enabled: Reserved,
            put_is_cursor_capture_enabled: Switch,
        },
    ),
    (
        "`IGraphicsCaptureSession3`: the border switch, at 6 and 7.",
        SessionBorderVt: {
            get_iids: Reserved,
            get_runtime_class_name: Reserved,
            get_trust_level: Reserved,
            get_is_border_required: Reserved,
            put_is_border_required: Switch,
        },
    ),
    (
        "`IDirect3D11CaptureFrame`: `get_Surface` 6, `get_ContentSize` 8.",
        FrameVt: {
            get_iids: Reserved,
            get_runtime_class_name: Reserved,
            get_trust_level: Reserved,
            get_surface: TakeSurface,
            get_system_relative_time: Reserved,
            get_content_size: Measure,
        },
    ),
    (
        "`IDirect3DDxgiInterfaceAccess`: `GetInterface` at 3.",
        AccessVt: {
            get_interface: TakeInterface,
        },
    ),
    (
        "`IInspectable`: the shape the device bridge hands back, held only
         long enough to ask it for the Direct3D device.",
        InspectableVt: {},
    ),
    (
        "`IDirect3DSurface` (the `WinRT` wrapper): held only for its DXGI cast.",
        SurfaceVt: {},
    ),
    (
        "`IDirect3DDevice` (the `WinRT` wrapper): held, never called.",
        DirectDeviceVt: {},
    ),
    (
        "`IDXGIDevice`: the bridge into the `WinRT` wrapper.",
        DxgiDeviceVt: {},
    ),
}

// Signature aliases: every entry follows the Win32 `system` convention.
type ForWindow =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> i32;
type Measure = unsafe extern "system" fn(*mut c_void, *mut SizeInt32) -> i32;
type MakePool = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    i32,
    i32,
    SizeInt32,
    *mut *mut c_void,
) -> i32;
type NextFrame = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
type MakeSession = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> i32;
type Start = unsafe extern "system" fn(*mut c_void) -> i32;
type Switch = unsafe extern "system" fn(*mut c_void, i32) -> i32;
type TakeSurface = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
type TakeInterface = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32;

// ---------------------------------------------------------------------------
// GUIDs
// ---------------------------------------------------------------------------

/// `IID_IGraphicsCaptureItem`.
pub const CAPTURE_ITEM: Guid = Guid::new(
    0x79c3_f95b,
    0x31f7,
    0x4ec2,
    [0xa4, 0x64, 0x63, 0x2e, 0xf5, 0xd3, 0x07, 0x60],
);
/// `IID_IGraphicsCaptureItemInterop` (Windows.Graphics.Capture.Interop.h).
pub const ITEM_INTEROP: Guid = Guid::new(
    0x3628_e81b,
    0x3cac,
    0x4c60,
    [0xb7, 0xf4, 0x23, 0xce, 0x0e, 0x0c, 0x33, 0x56],
);
/// `IID_IDirect3D11CaptureFramePoolStatics`.
pub const POOL_STATICS: Guid = Guid::new(
    0x7784_056a,
    0x67aa,
    0x4d53,
    [0xae, 0x54, 0x10, 0x88, 0xd5, 0xa8, 0xca, 0x21],
);
/// `IID_IGraphicsCaptureSession2` (cursor inclusion).
pub const SESSION_CURSOR: Guid = Guid::new(
    0x2c39_ae40,
    0x7d2e,
    0x5044,
    [0x80, 0x4e, 0x8b, 0x67, 0x99, 0xd4, 0xcf, 0x9e],
);
/// `IID_IGraphicsCaptureSession3` (the capture border).
pub const SESSION_BORDER: Guid = Guid::new(
    0xf2cd_d966,
    0x22ae,
    0x5ea1,
    [0x95, 0x96, 0x3a, 0x28, 0x93, 0x44, 0xc3, 0xbe],
);
/// `IID_IDirect3DDevice` (windows.graphics.directx.direct3d11.h).
pub const DIRECT_DEVICE: Guid = Guid::new(
    0xa376_24ab,
    0x8d5f,
    0x4650,
    [0x9d, 0x3e, 0x9e, 0xae, 0x3d, 0x9b, 0xc6, 0x70],
);
/// `IID_IDirect3DDxgiInterfaceAccess`
/// (windows.graphics.directx.direct3d11.interop.h).
pub const DXGI_ACCESS: Guid = Guid::new(
    0xa9b3_d012,
    0x3df2,
    0x4ee3,
    [0xb8, 0xd1, 0x86, 0x95, 0xf4, 0x57, 0xd3, 0xc1],
);
/// `IID_IDXGIDevice` (dxgi.h).
pub const DXGI_DEVICE: Guid = Guid::new(
    0x54ec_77fa,
    0x1377,
    0x44e6,
    [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c],
);

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// `Windows::Graphics::SizeInt32`: a frame's pixel extent.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SizeInt32 {
    /// Width in pixels.
    pub width: i32,
    /// Height in pixels.
    pub height: i32,
}

pin_layout!(SizeInt32, 8; width = 0, height = 4);

/// `DirectXPixelFormat::B8G8R8A8UIntNormalized`: the same number as
/// `DXGI_FORMAT_B8G8R8A8_UNORM`, the capture layout every encoder here
/// already eats.
pub const PIXEL_FORMAT_BGRA8: i32 = 0x57;

/// Frames the pool keeps in flight. Two is what the samples use: one being
/// read, one being filled.
pub const POOL_BUFFERS: i32 = 2;
