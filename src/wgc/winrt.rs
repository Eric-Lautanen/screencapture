//! `WinRT` activation for the capture stack: class ids as `HSTRING`s, the
//! activation factory call, and one method per interface the window source
//! drives. Every raw pointer stays inside this module, so `mod.rs` only
//! ever hands over a window handle and reads pixels.

use std::ffi::c_void;
use std::sync::OnceLock;

use crate::dxgi::abi::Complete;
use crate::dxgi::com::Com;
use crate::dxgi::ffi::{self, ProcError};
use crate::dxgi::structs::{Guid, ID3D_TEXTURE2D};

use super::abi::{
    AccessVt, CAPTURE_ITEM, DIRECT_DEVICE, DirectDeviceVt, DxgiDeviceVt, FrameVt, InspectableVt,
    InteropVt, ItemVt, PIXEL_FORMAT_BGRA8, POOL_BUFFERS, PoolStaticsVt, PoolVt, SESSION_BORDER,
    SESSION_CURSOR, SessionBorderVt, SessionCursorVt, SessionVt, SizeInt32, SurfaceVt,
};

/// `Windows.Graphics.Capture.GraphicsCaptureItem`, the item factory class.
pub(super) const ITEM_CLASS: &str = "Windows.Graphics.Capture.GraphicsCaptureItem";
/// `Windows.Graphics.Capture.Direct3D11CaptureFramePool`, the pool class.
pub(super) const POOL_CLASS: &str = "Windows.Graphics.Capture.Direct3D11CaptureFramePool";

/// `RO_INIT_MULTITHREADED`: the capture objects are used from whichever
/// thread runs the grab loop, never from a UI apartment.
const RO_INIT_MULTITHREADED: u32 = 1;

/// Error code used when interface memory stopped presenting a v-table; no
/// call goes out and nothing can pretend to have succeeded.
pub(super) const ADRIFT: i32 = -1;

/// "combase.dll" as UTF-16 with its NUL, the module that hosts the `WinRT`
/// activation entry points.
const COMBASE_DLL: &[u16] = &[
    0x63, 0x6F, 0x6D, 0x62, 0x61, 0x73, 0x65, 0x2E, 0x64, 0x6C, 0x6C, 0x0,
];

/// `RoInitialize`.
type Initialize = unsafe extern "system" fn(u32) -> i32;
/// `RoGetActivationFactory`.
type Factory = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32;
/// `WindowsCreateString`.
type CreateString = unsafe extern "system" fn(*const u16, u32, *mut *mut c_void) -> i32;
/// `WindowsDeleteString`.
type DeleteString = unsafe extern "system" fn(*mut c_void) -> i32;

/// Every `combase.dll` entry point this module calls. They are looked up
/// by name rather than imported at link time: the mingw toolchain this
/// crate builds with ships no `combase` import library, and a Windows too
/// old to have the module should lose window capture, not the process.
struct Api {
    initialize: Initialize,
    factory: Factory,
    create_string: CreateString,
    delete_string: DeleteString,
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(resolve).as_ref()
}

fn resolve() -> Option<Api> {
    static MODULE: OnceLock<usize> = OnceLock::new();
    let module = ffi::load(&MODULE, COMBASE_DLL, "combase").ok()?;
    // SAFETY: each name is a NUL-terminated export of `combase.dll` and
    // every signature below is the documented one.
    Some(Api {
        initialize: unsafe {
            std::mem::transmute::<*mut c_void, Initialize>(export(module, b"RoInitialize\0")?)
        },
        factory: unsafe {
            std::mem::transmute::<*mut c_void, Factory>(export(
                module,
                b"RoGetActivationFactory\0",
            )?)
        },
        create_string: unsafe {
            std::mem::transmute::<*mut c_void, CreateString>(export(
                module,
                b"WindowsCreateString\0",
            )?)
        },
        delete_string: unsafe {
            std::mem::transmute::<*mut c_void, DeleteString>(export(
                module,
                b"WindowsDeleteString\0",
            )?)
        },
    })
}

/// One export, with the `unsafe` the lookup already requires spelled out.
///
/// # Safety
/// `module` must be live and `name` a NUL-terminated export whose signature
/// the caller's `transmute` target matches.
unsafe fn export(module: *mut c_void, name: &[u8]) -> Option<*mut c_void> {
    // SAFETY: upheld by the caller contract above.
    unsafe { ffi::export(module, name) }
}

/// Initialize the `WinRT` runtime for the calling thread. A second ask is
/// the documented way to repeat it (it answers `S_FALSE`), and an
/// apartment that is already initialized is not an error here: the session
/// only polls frames and copies textures, never marshals a callback.
pub fn initialize() {
    if let Some(api) = api() {
        // SAFETY: a per-thread request for the multithreaded model; no
        // per-thread teardown is required at this granularity.
        let _ = unsafe { (api.initialize)(RO_INIT_MULTITHREADED) };
    }
}

/// The activation factory for `class`, asked for one `iid` interface.
///
/// # Errors
/// Fails with the raw `HRESULT` when the class does not exist on this
/// Windows build or the factory refuses `iid`.
pub fn activate<T: Complete>(class: &str, iid: &Guid) -> Result<Com<T>, i32> {
    let api = api().ok_or(ADRIFT)?;
    let units: Vec<u16> = class.encode_utf16().collect();
    let length = u32::try_from(units.len()).map_err(|_| ADRIFT)?;
    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: `units` outlives the call, `length` counts them exactly, and
    // the handle is a local the call writes.
    let created = unsafe { (api.create_string)(units.as_ptr(), length, &raw mut handle) };
    if created < 0 || handle.is_null() {
        return Err(created);
    }
    let mut out: *mut c_void = std::ptr::null_mut();
    // SAFETY: a live HSTRING, a local `GUID`, and a local out slot; the
    // factory adds the reference this handle then owns.
    let hr = unsafe { (api.factory)(handle, iid, &raw mut out) };
    // SAFETY: the string was created above and is ours alone.
    unsafe { (api.delete_string)(handle) };
    if hr < 0 || out.is_null() {
        return Err(hr);
    }
    // SAFETY: the factory answered a non-null interface carrying the
    // reference it handed us.
    Ok(unsafe { Com::own(out) })
}

/// `CreateDirect3D11DeviceFromDXGIDevice` from `d3d11.dll`, loaded by name
/// like every other graphics entry point so a missing stack degrades
/// instead of refusing to start the process.
///
/// # Errors
/// Fails when the library or the export is missing, or the call refuses.
pub fn direct_device(device: &Com<DxgiDeviceVt>) -> Result<Com<DirectDeviceVt>, ProcError> {
    let module = ffi::d3d11_module()?;
    // SAFETY: the module is a live `d3d11.dll` handle and the name is a
    // NUL-terminated export of it.
    let raw = unsafe { ffi::export(module, b"CreateDirect3D11DeviceFromDXGIDevice\0") }
        .ok_or(ProcError::Missing("CreateDirect3D11DeviceFromDXGIDevice"))?;
    // SAFETY: the export is documented as exactly this signature.
    let entry: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
        unsafe { std::mem::transmute(raw) };
    let mut out: *mut c_void = std::ptr::null_mut();
    // SAFETY: a live DXGI device and a local out slot.
    let hr = unsafe { entry(device.raw(), &raw mut out) };
    if hr < 0 || out.is_null() {
        return Err(ProcError::Call(hr));
    }
    // SAFETY: the call answered a non-null `IInspectable` carrying the
    // reference it handed us; the WinRT device wraps that object.
    let inspectable = unsafe { Com::<InspectableVt>::own(out) };
    inspectable
        .query_to(&DIRECT_DEVICE)
        .map_err(ProcError::Call)
}

impl Com<InteropVt> {
    /// `CreateForWindow`: the capture item for one window handle.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` when the window cannot be captured
    /// (closed, or on a desktop this process cannot see).
    pub fn item(&self, window: usize) -> Result<Com<ItemVt>, i32> {
        let mut out: *mut c_void = std::ptr::null_mut();
        let handle = window as *mut c_void;
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let create = table.create_for_window;
        // SAFETY: live interop interface, a window handle, the item
        // v-table's `GUID`, and a local out slot; slot 3 takes exactly
        // these four arguments.
        let hr = unsafe { create(self.raw(), handle, &CAPTURE_ITEM, &raw mut out) };
        if hr < 0 || out.is_null() {
            return Err(hr);
        }
        // SAFETY: the interop answered a non-null item whose reference is
        // now ours.
        Ok(unsafe { Com::own(out) })
    }
}

impl Com<ItemVt> {
    /// `get_Size`: the window's capture extent in pixels.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT`.
    pub fn size(&self) -> Result<(u32, u32), i32> {
        let mut measured = SizeInt32::default();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let measure = table.get_size;
        // SAFETY: live interface and a local `SizeInt32`.
        let hr = unsafe { measure(self.raw(), &raw mut measured) };
        if hr < 0 {
            return Err(hr);
        }
        Ok((
            u32::try_from(measured.width).unwrap_or(0),
            u32::try_from(measured.height).unwrap_or(0),
        ))
    }
}

impl Com<PoolStaticsVt> {
    /// `Create`: a frame pool fed by a window's composition surface.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` (a device without BGRA support, an
    /// impossible size).
    pub fn pool(&self, device: &Com<DirectDeviceVt>, size: (u32, u32)) -> Result<Com<PoolVt>, i32> {
        let width = i32::try_from(size.0).map_err(|_| ADRIFT)?;
        let height = i32::try_from(size.1).map_err(|_| ADRIFT)?;
        let mut out: *mut c_void = std::ptr::null_mut();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let create = table.create;
        // SAFETY: live statics interface, a live WinRT device, and a local
        // out slot; slot 6 takes exactly these six arguments.
        let hr = unsafe {
            create(
                self.raw(),
                device.raw(),
                PIXEL_FORMAT_BGRA8,
                POOL_BUFFERS,
                SizeInt32 { width, height },
                &raw mut out,
            )
        };
        if hr < 0 || out.is_null() {
            return Err(hr);
        }
        // SAFETY: `Create` answered a non-null pool whose reference is
        // now ours.
        Ok(unsafe { Com::own(out) })
    }
}

impl Com<PoolVt> {
    /// `TryGetNextFrame`: the newest frame, or `None` when nothing has been
    /// presented since the last poll.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` when the session is over.
    pub fn next_frame(&self) -> Result<Option<Com<FrameVt>>, i32> {
        let mut out: *mut c_void = std::ptr::null_mut();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let poll = table.try_get_next_frame;
        // SAFETY: live interface and a local out slot; slot 7 takes
        // exactly these two arguments.
        let hr = unsafe { poll(self.raw(), &raw mut out) };
        if hr < 0 {
            return Err(hr);
        }
        if out.is_null() {
            return Ok(None);
        }
        // SAFETY: a non-null frame carries the reference handed to us.
        Ok(Some(unsafe { Com::own(out) }))
    }

    /// `CreateCaptureSession`: bind this pool to one capture item.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT`.
    pub fn session(&self, item: &Com<ItemVt>) -> Result<Com<SessionVt>, i32> {
        let mut out: *mut c_void = std::ptr::null_mut();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let create = table.create_capture_session;
        // SAFETY: live interfaces and a local out slot; slot 10 takes
        // exactly these three arguments.
        let hr = unsafe { create(self.raw(), item.raw(), &raw mut out) };
        if hr < 0 || out.is_null() {
            return Err(hr);
        }
        // SAFETY: a non-null session carries the reference handed to us.
        Ok(unsafe { Com::own(out) })
    }
}

impl Com<SessionVt> {
    /// `StartCapture`: begin feeding the pool.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT`, which is the only diagnostic for a
    /// window this process may not capture.
    pub fn start(&self) -> Result<(), i32> {
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let start = table.start_capture;
        // SAFETY: live interface; slot 6 takes just the object.
        let hr = unsafe { start(self.raw()) };
        (hr >= 0).then_some(()).ok_or(hr)
    }

    /// `put_IsCursorCaptureEnabled`: choose whether frames carry the
    /// pointer. Absent before Windows 10 2004, where the compositor drew
    /// the pointer into every frame with no switch to turn it off; that
    /// keeps an "include the pointer" setting satisfied and leaves a mute
    /// unable to remove it, which is the one thing to know here.
    pub fn set_cursor(&self, enabled: bool) {
        let Ok(cursor) = self.query_to::<SessionCursorVt>(&SESSION_CURSOR) else {
            return;
        };
        let Some(table) = cursor.view() else {
            return;
        };
        let switch = table.put_is_cursor_capture_enabled;
        // SAFETY: live interface; slot 7 takes `this` and a WinRT boolean.
        unsafe { switch(cursor.raw(), i32::from(enabled)) };
    }

    /// `put_IsBorderRequired`: drop the yellow capture outline Windows 11
    /// draws around the captured window. Absent before Windows 11 22H2,
    /// where the outline simply stays.
    pub fn hide_border(&self) {
        let Ok(border) = self.query_to::<SessionBorderVt>(&SESSION_BORDER) else {
            return;
        };
        let Some(table) = border.view() else {
            return;
        };
        let switch = table.put_is_border_required;
        // SAFETY: live interface; slot 7 takes `this` and a WinRT boolean.
        unsafe { switch(border.raw(), 0) };
    }
}

impl Com<FrameVt> {
    /// `get_ContentSize`: the frame's pixel extent, which follows the
    /// window if it was resized.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT`.
    pub fn size(&self) -> Result<(u32, u32), i32> {
        let mut measured = SizeInt32::default();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let measure = table.get_content_size;
        // SAFETY: live interface and a local `SizeInt32`.
        let hr = unsafe { measure(self.raw(), &raw mut measured) };
        if hr < 0 {
            return Err(hr);
        }
        Ok((
            u32::try_from(measured.width).unwrap_or(0),
            u32::try_from(measured.height).unwrap_or(0),
        ))
    }

    /// The D3D11 texture behind the frame's `WinRT` surface.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` when either hop refuses.
    pub fn texture(&self) -> Result<Com<crate::dxgi::abi::TextureVt>, i32> {
        let mut surface: *mut c_void = std::ptr::null_mut();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let take = table.get_surface;
        // SAFETY: live interface and a local out slot; slot 6 takes
        // exactly these two arguments.
        let hr = unsafe { take(self.raw(), &raw mut surface) };
        if hr < 0 || surface.is_null() {
            return Err(hr);
        }
        // SAFETY: a non-null surface carries the reference handed to us.
        let surface = unsafe { Com::<SurfaceVt>::own(surface) };
        let access = surface.query_to::<AccessVt>(&super::abi::DXGI_ACCESS)?;
        let Some(table) = access.view() else {
            return Err(ADRIFT);
        };
        let fetch = table.get_interface;
        let mut texture: *mut c_void = std::ptr::null_mut();
        // SAFETY: live access interface, the texture `GUID`, and a local
        // out slot; slot 3 takes exactly these three arguments.
        let hr = unsafe { fetch(access.raw(), &ID3D_TEXTURE2D, &raw mut texture) };
        if hr < 0 || texture.is_null() {
            return Err(hr);
        }
        // SAFETY: `GetInterface` answered a non-null texture whose
        // reference is now ours.
        Ok(unsafe { Com::own(texture) })
    }
}
