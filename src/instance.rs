//! Single-instance guard so hotkey-spawned or accidental relaunches never
//! stack duplicate tray icons or recorders.

use std::io;

/// Outcome of trying to become the single running instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Acquire {
    /// This process owns the tray.
    Owner,
    /// Another instance already owns the tray.
    AlreadyRunning,
}

/// Acquire the single-instance guard.
///
/// # Errors
/// Returns an error when the OS refuses to create the guard mutex.
pub fn acquire() -> io::Result<Acquire> {
    if native::already_running()? {
        return Ok(Acquire::AlreadyRunning);
    }
    Ok(Acquire::Owner)
}

#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const MUTEX_NAME: &str = "Local\\ScreencaptureInstance";

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(
            attributes: *mut c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn GetLastError() -> u32;
    }

    /// Returns true when a previous instance still holds the mutex. The
    /// created handle is intentionally leaked; the OS releases it when the
    /// process ends.
    pub(super) fn already_running() -> io::Result<bool> {
        let name: Vec<u16> = std::ffi::OsStr::new(MUTEX_NAME)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            // SAFETY: `name` outlives the call; attributes and initial
            // ownership are intentionally unused.
            CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr())
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { GetLastError() } == ERROR_ALREADY_EXISTS)
    }
}

#[cfg(not(windows))]
mod native {
    use std::io;

    pub(super) fn already_running() -> io::Result<bool> {
        Ok(false)
    }
}
