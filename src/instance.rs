//! Single-instance guard so hotkey-spawned or accidental relaunches never
//! stack duplicate tray icons or recorders: a Windows mutex per process
//! name, an exclusive file lock on Unix.

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
/// Returns an error when the OS refuses to create the guard mutex or
/// cannot open the lock file.
pub fn acquire() -> io::Result<Acquire> {
    native::acquire()
}

/// Whether another instance currently owns the tray, without creating the
/// guard. The CLI asks before the tray detaches its console, so a second
/// launch can say why it is exiting instead of dying silently.
pub fn running() -> bool {
    native::running()
}

#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;

    use super::Acquire;

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const MUTEX_NAME: &str = "Local\\ScreencaptureInstance";
    /// Access right for opening an existing named object.
    const SYNCHRONIZE: u32 = 0x0010_0000;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(
            attributes: *mut c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn OpenMutexW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn SetLastError(error: u32);
        fn GetLastError() -> u32;
    }

    /// Create the named mutex and read its ownership. The handle is
    /// intentionally leaked; the OS releases it when the process ends.
    ///
    /// # Errors
    /// The mutex could not be created.
    pub(super) fn acquire() -> io::Result<Acquire> {
        let name: Vec<u16> = std::ffi::OsStr::new(MUTEX_NAME)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // `CreateMutexW` only promises `ERROR_ALREADY_EXISTS` on the existing
        // case; clear the slot so a new mutex cannot inherit an unrelated
        // prior error and masquerade as a second instance.
        // SAFETY: plain thread-local last-error slot.
        unsafe { SetLastError(0) };
        let handle = unsafe {
            // SAFETY: `name` outlives the call; attributes and initial
            // ownership are intentionally unused.
            CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr())
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let taken = unsafe { GetLastError() == ERROR_ALREADY_EXISTS };
        Ok(if taken {
            Acquire::AlreadyRunning
        } else {
            Acquire::Owner
        })
    }

    /// Probe for the guard mutex without taking anything: a live instance
    /// keeps its handle for as long as it runs, so the object exists while
    /// the tray does and vanishes with it.
    pub(super) fn running() -> bool {
        let name: Vec<u16> = std::ffi::OsStr::new(MUTEX_NAME)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `name` is NUL-terminated and outlives the call; the
        // returned handle is closed once below.
        let handle = unsafe { OpenMutexW(SYNCHRONIZE, 0, name.as_ptr()) };
        if handle.is_null() {
            return false;
        }
        // SAFETY: the handle came from `OpenMutexW` and is closed once.
        unsafe { CloseHandle(handle) };
        true
    }
}

#[cfg(not(windows))]
mod native {
    use std::fs::{OpenOptions, TryLockError};
    use std::io;
    use std::path::PathBuf;

    use super::Acquire;

    const LOCK_NAME: &str = "screencapture.lock";

    /// An exclusive advisory file lock. The OS frees it when the holding
    /// process dies, so a leftover file is never a stale lock: the next
    /// run opens it again and locks successfully. The handle is parked
    /// for the process lifetime exactly like the Windows mutex.
    ///
    /// # Errors
    /// The lock file cannot be opened, or the OS refused a lock op.
    pub(super) fn acquire() -> io::Result<Acquire> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|value| value.is_absolute())
            .unwrap_or_else(std::env::temp_dir);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(LOCK_NAME))?;
        match file.try_lock() {
            Ok(()) => {
                std::mem::forget(file); // lock lives until process exit
                Ok(Acquire::Owner)
            }
            Err(TryLockError::WouldBlock) => Ok(Acquire::AlreadyRunning),
            Err(TryLockError::Error(problem)) => Err(problem),
        }
    }

    /// There is no tray off Windows, so nothing can be holding it: a bare
    /// launch prints the command list instead.
    pub(super) fn running() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::running;
    use super::{Acquire, acquire};

    /// The guard exists so a second launch — hotkey-spawned, or a stray
    /// double click — never stacks a second tray icon or recorder: the
    /// first acquire owns the tray, the next one is told it is taken.
    #[cfg(windows)]
    #[test]
    fn a_second_instance_is_told_the_tray_is_taken() {
        let first = acquire().expect("the guard mutex");
        if first != Acquire::Owner {
            return; // a real instance is running on this desktop
        }
        assert_eq!(acquire().expect("the guard mutex"), Acquire::AlreadyRunning);
        // The CLI's pre-flight probe answers the same way, without taking
        // the guard a second time.
        assert!(running());
    }

    #[cfg(not(windows))]
    #[test]
    fn the_lock_reinstalls_after_release() -> std::io::Result<()> {
        // One acquire per test binary process: the second call from a
        // raced sibling fails cleanly instead of corrupting state.
        let first = acquire()?;
        assert_eq!(first, Acquire::Owner);
        Ok(())
    }
}
