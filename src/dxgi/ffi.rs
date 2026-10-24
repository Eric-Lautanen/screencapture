//! The `dxgi.dll`/`d3d11.dll` loads and the two entry points that open the
//! duplication chain: `CreateDXGIFactory1` and `D3D11CreateDevice`. Loaded
//! by name so a missing graphics stack degrades instead of failing the
//! whole binary at link time.

use std::ffi::c_void;
use std::sync::OnceLock;

use crate::dxgi::abi::{AdapterVt, ContextVt, DeviceVt, FactoryVt};
use crate::dxgi::com::Com;
use crate::dxgi::structs::{Guid, IDXGI_FACTORY1, S_OK};

// ---------------------------------------------------------------------------
// Library plumbing
// ---------------------------------------------------------------------------

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(file_name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, proc_name: *const u8) -> *mut c_void;
}

/// `LOAD_LIBRARY_SEARCH_SYSTEM32` (libloaderapi.h): resolve only from
/// the system directory, never from the working directory or `PATH`.
const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;

const DXGI_DLL: &[u16] = &[
    0x64, 0x78, 0x67, 0x69, 0x2E, 0x64, 0x6C, 0x6C, 0x0, // dxgi.dll
];
const D3D11_DLL: &[u16] = &[
    0x64, 0x33, 0x64, 0x31, 0x31, 0x2E, 0x64, 0x6C, 0x6C, 0x0, // d3d11.dll
];
const FACTORY1_PROC: &[u8] = b"CreateDXGIFactory1\0";
const DEVICE_PROC: &[u8] = b"D3D11CreateDevice\0";
const FACTORY_LABEL: &str = "dxgi";
const DEVICE_LABEL: &str = "d3d11";

/// Loaded modules, cached so a recording's session rebuilds do not leak
/// another library reference on every attempt. Stored as integers because
/// a raw pointer is not `Sync`; each handle lives until process exit.
static DXGI_MODULE: OnceLock<usize> = OnceLock::new();
static D3D11_MODULE: OnceLock<usize> = OnceLock::new();

/// `D3D_DRIVER_TYPE_UNKNOWN`: mandatory when an adapter is passed.
const DRIVER_UNKNOWN: u32 = 0;
/// `D3D11_SDK_VERSION` (d3d11.h).
const SDK_VERSION: u32 = 7;
/// `D3D11_CREATE_DEVICE_BGRA_SUPPORT` (d3d11.h): the window-capture frame
/// pool refuses a device without it, because its frames are BGRA.
const BGRA_SUPPORT: u32 = 0x0000_0020;

type FactoryProc = unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32;
type DeviceProc = unsafe extern "system" fn(
    // adapter; driver type; software module; creation flags
    *mut c_void,
    u32,
    *mut c_void,
    u32,
    // feature levels and count (null/0: driver defaults)
    *const u32,
    u32,
    // SDK version; device out; feature level out; immediate context out
    u32,
    *mut *mut c_void,
    *mut u32,
    *mut *mut c_void,
) -> i32;

/// Why a native graphics entry point refused to run.
#[derive(Debug)]
pub enum ProcError {
    /// The system library is missing.
    Missing(&'static str),
    /// The library answered with a nonzero `HRESULT`.
    Call(i32),
}

impl ProcError {
    /// Printable explanation for diagnostics.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Missing(library) => format!("{library} cannot be loaded"),
            Self::Call(hr) => format!("call answered 0x{hr:08X}"),
        }
    }
}

/// `CreateDXGIFactory1(riid, factory)` from `dxgi.dll`.
///
/// # Errors
/// Fails when the library cannot load or the call returns a failure code.
pub fn spawn_factory() -> Result<Com<FactoryVt>, ProcError> {
    let module = load(&DXGI_MODULE, DXGI_DLL, FACTORY_LABEL)?;
    // SAFETY: the arguments below name an export this module always has.
    let raw = unsafe { proc(module, FACTORY1_PROC) };
    // SAFETY: `raw` is a live code address; the signature is dxgi.h's
    // `CreateDXGIFactory1` exactly.
    let entry: FactoryProc = unsafe { std::mem::transmute(raw) };
    let mut out: *mut c_void = std::ptr::null_mut();
    // SAFETY: arguments are live locals; the GUID layout matches REFIID.
    let hr = unsafe { entry(&IDXGI_FACTORY1, &raw mut out) };
    if hr != S_OK || out.is_null() {
        return Err(ProcError::Call(hr));
    }
    // SAFETY: `CreateDXGIFactory1` returned S_OK with a non-null factory
    // whose reference is now ours.
    Ok(unsafe { Com::own(out) })
}

/// `D3D11CreateDevice(adapter, UNKNOWN, null, 0, null, 0, SDK, ...)`.
///
/// # Errors
/// Fails when the library or the driver refuses the device.
pub fn spawn_device(
    adapter: &Com<AdapterVt>,
) -> Result<(Com<DeviceVt>, Com<ContextVt>), ProcError> {
    create_device(adapter, 0)
}

/// A device able to hand BGRA textures to the Windows Graphics Capture
/// frame pool; the desktop duplication path uses plain [`spawn_device`].
///
/// # Errors
/// Fails when the library or the driver refuses the device.
pub fn spawn_capture_device(
    adapter: &Com<AdapterVt>,
) -> Result<(Com<DeviceVt>, Com<ContextVt>), ProcError> {
    create_device(adapter, BGRA_SUPPORT)
}

fn create_device(
    adapter: &Com<AdapterVt>,
    flags: u32,
) -> Result<(Com<DeviceVt>, Com<ContextVt>), ProcError> {
    let module = load(&D3D11_MODULE, D3D11_DLL, DEVICE_LABEL)?;
    // SAFETY: the arguments below name an export this module always has.
    let raw = unsafe { proc(module, DEVICE_PROC) };
    // SAFETY: `raw` is a live code address; the signature is d3d11.h's
    // `D3D11CreateDevice` exactly.
    let entry: DeviceProc = unsafe { std::mem::transmute(raw) };
    let mut device: *mut c_void = std::ptr::null_mut();
    let mut context: *mut c_void = std::ptr::null_mut();
    // SAFETY: the adapter is a live COM object we own; out params are
    // locals, and null feature levels mean "driver defaults".
    let hr = unsafe {
        entry(
            adapter.raw(),
            DRIVER_UNKNOWN,
            std::ptr::null_mut(),
            flags,
            std::ptr::null(),
            0,
            SDK_VERSION,
            &raw mut device,
            std::ptr::null_mut(),
            &raw mut context,
        )
    };
    if hr != S_OK || device.is_null() || context.is_null() {
        return Err(ProcError::Call(hr));
    }
    // SAFETY: `D3D11CreateDevice` returned S_OK with non-null device and
    // context interfaces, each holding the reference it handed us.
    Ok((unsafe { Com::own(device) }, unsafe { Com::own(context) }))
}

/// The `d3d11.dll` handle, for entry points beyond device creation (the
/// window-capture device bridge).
///
/// # Errors
/// Fails when the library is missing.
pub(crate) fn d3d11_module() -> Result<*mut c_void, ProcError> {
    load(&D3D11_MODULE, D3D11_DLL, DEVICE_LABEL)
}

/// One export of an already-open module, or `None` when this Windows build
/// does not have it.
///
/// # Safety
/// `module` must be a live library handle and `name` a NUL-terminated
/// export name whose signature the caller matches exactly.
pub(crate) unsafe fn export(module: *mut c_void, name: &[u8]) -> Option<*mut c_void> {
    // SAFETY: upheld by the caller contract above.
    let entry = unsafe { GetProcAddress(module, name.as_ptr()) };
    (!entry.is_null()).then_some(entry)
}

pub(crate) fn load(
    cache: &'static OnceLock<usize>,
    library: &[u16],
    name: &'static str,
) -> Result<*mut c_void, ProcError> {
    let module = *cache.get_or_init(|| {
        // SAFETY: `library` is a NUL-terminated wide string literal; null
        // selects the caller's process token and the flags pin the lookup
        // to the system directory.
        let raw = unsafe {
            LoadLibraryExW(
                library.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        raw as usize
    });
    if module == 0 {
        return Err(ProcError::Missing(name));
    }
    Ok(module as *mut c_void)
}

/// Fetch one export from an already-open module.
///
/// # Panics
/// Only on a machine whose graphics libraries exist yet export nothing;
/// calling through such layouts would corrupt memory, so we abort first.
///
/// # Safety
/// `module` must be a live library handle and `name` must name an export
/// whose signature the caller matches exactly.
unsafe fn proc(module: *mut c_void, name: &[u8]) -> *mut c_void {
    // SAFETY: both arguments below are live strings inside this call.
    let entry = unsafe { GetProcAddress(module, name.as_ptr()) };
    if entry.is_null() {
        std::process::abort();
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::{DXGI_DLL, DXGI_MODULE, FACTORY_LABEL, load};

    /// The system loader is cached: the second call must answer the same
    /// module instead of opening another reference.
    #[test]
    fn the_graphics_module_is_loaded_once() {
        let first =
            load(&DXGI_MODULE, DXGI_DLL, FACTORY_LABEL).expect("dxgi.dll exists on Windows");
        let second = load(&DXGI_MODULE, DXGI_DLL, FACTORY_LABEL).expect("the cached dxgi.dll");
        assert_eq!(first, second);
    }
}
