//! Runtime library loading on Unix: `dlopen`/`dlsym` by name so a missing
//! framework, X server, or X library degrades instead of failing the whole
//! binary at link time. The Windows equivalent lives beside its users in
//! `dxgi/ffi.rs`.

use std::ffi::c_void;
use std::io;

/// `RTLD_NOW` (dlfcn.h): resolve every relocation during the load.
const RTLD_NOW: i32 = 2;
/// `RTLD_LOCAL` (dlfcn.h): keep symbols out of the global namespace.
const RTLD_LOCAL: i32 = 0;

// The `dl` routines live in libSystem on macOS (its SDK also ships a libdl
// stub) and in libdl or libc on Linux; naming `dl` links on both.
#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(file: *const std::ffi::c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> i32;
    fn dlerror() -> *const std::ffi::c_char;
}

/// One open library; closes itself on drop.
pub struct Library {
    raw: *mut c_void,
}

impl Library {
    /// Open a library by absolute framework path or dynamic-loader soname.
    ///
    /// # Errors
    /// The name cannot be loaded (driver stack absent, no X server, ...).
    pub fn open(name: &str) -> io::Result<Self> {
        let file = std::ffi::CString::new(name)?;
        // SAFETY: `file` is an owned, NUL-terminated C string valid across
        // the call; the handle is closed in `Drop`.
        let raw = unsafe { dlopen(file.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        if raw.is_null() {
            return Err(io::Error::other(format!(
                "library {name} did not load: {}",
                last_error()
            )));
        }
        Ok(Self { raw })
    }

    /// Address of one export.
    ///
    /// # Errors
    /// The name is not exported by this library.
    pub fn symbol(&self, name: &[u8]) -> io::Result<*mut c_void> {
        let symbol = std::ffi::CStr::from_bytes_until_nul(name)
            .map_err(|_| io::Error::other("symbol name lacks the NUL terminator"))?;
        // SAFETY: the handle is an open module and `symbol` is a
        // NUL-terminated C string.
        let raw = unsafe { dlsym(self.raw, symbol.as_ptr()) };
        if raw.is_null() {
            return Err(io::Error::other(format!(
                "{:?} was not found: {}",
                symbol.to_string_lossy(),
                last_error()
            )));
        }
        Ok(raw)
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: `raw` is the handle from our own successful `dlopen`.
        unsafe { dlclose(self.raw) };
    }
}

/// The `dlerror` message, if any; reading it also clears the slot.
fn last_error() -> String {
    // SAFETY: the message, when present, is a live thread-local C string.
    let text = unsafe {
        let raw = dlerror();
        (!raw.is_null()).then(|| std::ffi::CStr::from_ptr(raw).to_string_lossy().into_owned())
    };
    text.unwrap_or_else(|| "reason unavailable".to_owned())
}

#[cfg(test)]
mod tests {
    use super::Library;

    #[test]
    fn unknown_library_fails_cleanly() {
        let result = Library::open("no-such-library-named-like-this.so");
        assert!(result.is_err(), "found a nonexistent library");
    }
}
