//! The `dxgi.dll`/`d3d11.dll` loads and the two entry points that open the
//! duplication chain: `CreateDXGIFactory1` and `D3D11CreateDevice`. Loaded
//! by name so a missing graphics stack degrades instead of failing the
//! whole binary at link time.

use std::ffi::c_void;

use crate::dxgi::abi::{AdapterVt, ContextVt, DeviceVt, FactoryVt};
use crate::dxgi::com::Com;
use crate::dxgi::structs::{Guid, IDXGI_FACTORY1, S_OK};

// ---------------------------------------------------------------------------
// Library plumbing
// ---------------------------------------------------------------------------

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(file_name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, proc_name: *const u8) -> *mut c_void;
}

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

/// `D3D_DRIVER_TYPE_UNKNOWN`: mandatory when an adapter is passed.
const DRIVER_UNKNOWN: u32 = 0;
/// `D3D11_SDK_VERSION` (d3d11.h).
const SDK_VERSION: u32 = 7;

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
    let module = load(DXGI_DLL, FACTORY_LABEL)?;
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
    Ok(Com::own(out))
}

/// `D3D11CreateDevice(adapter, UNKNOWN, null, 0, null, 0, SDK, ...)`.
///
/// # Errors
/// Fails when the library or the driver refuses the device.
pub fn spawn_device(
    adapter: &Com<AdapterVt>,
) -> Result<(Com<DeviceVt>, Com<ContextVt>), ProcError> {
    let module = load(D3D11_DLL, DEVICE_LABEL)?;
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
            0,
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
    Ok((Com::own(device), Com::own(context)))
}

fn load(library: &[u16], name: &'static str) -> Result<*mut c_void, ProcError> {
    // SAFETY: `library` is a NUL-terminated wide string literal.
    let module = unsafe { LoadLibraryW(library.as_ptr()) };
    if module.is_null() {
        return Err(ProcError::Missing(name));
    }
    Ok(module)
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
