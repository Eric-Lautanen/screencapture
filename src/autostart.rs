//! Login autostart. Windows adds a `HKCU\...\Run` value; other platforms
//! land with their native mechanism in later sessions.

use std::io;

/// Whether launch-at-login is currently registered.
#[must_use]
pub fn registered() -> bool {
    native::registered()
}

/// Enable or disable launch-at-login.
///
/// # Errors
/// Propagates platform errors (registry writes on Windows).
pub fn set(enable: bool) -> io::Result<()> {
    native::set(enable)
}

#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::PathBuf;

    type Hkey = *mut c_void;
    type Status = i32;

    const ERROR_SUCCESS: Status = 0;
    const ERROR_FILE_NOT_FOUND: Status = 2;
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const KEY_SET_VALUE: u32 = 0x0002;
    const REG_OPTION_NON_VOLATILE: u32 = 0;
    const REG_SZ: u32 = 1;
    const VALUE_NAME: &str = "Screencapture";
    const SUB_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegCreateKeyExW(
            key: Hkey,
            sub_key: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            sam_desired: u32,
            security_attributes: *mut c_void,
            result: *mut Hkey,
            disposition: *mut u32,
        ) -> Status;
        fn RegQueryValueExW(
            key: Hkey,
            value_name: *const u16,
            reserved: *mut u32,
            kind: *mut u32,
            data: *mut u8,
            data_size: *mut u32,
        ) -> Status;
        fn RegSetValueExW(
            key: Hkey,
            value_name: *const u16,
            reserved: u32,
            kind: u32,
            data: *const u8,
            data_size: u32,
        ) -> Status;
        fn RegDeleteValueW(key: Hkey, value_name: *const u16) -> Status;
        fn RegCloseKey(key: Hkey) -> Status;
    }

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn run_key() -> Hkey {
        0x8000_0001usize as Hkey // HKEY_CURRENT_USER
    }

    fn open_for_access(sam: u32) -> io::Result<Hkey> {
        let name = wide(SUB_KEY);
        let mut key = std::ptr::null_mut();
        let status = {
            // SAFETY: sub_key is a null-terminated local buffer; `result`
            // receives a registry key handle owned by this call.
            unsafe {
                RegCreateKeyExW(
                    run_key(),
                    name.as_ptr(),
                    0,
                    std::ptr::null(),
                    REG_OPTION_NON_VOLATILE,
                    sam,
                    std::ptr::null_mut(),
                    &raw mut key,
                    std::ptr::null_mut(),
                )
            }
        };
        if status == ERROR_SUCCESS {
            Ok(key)
        } else {
            Err(io::Error::from_raw_os_error(status))
        }
    }

    fn exe_command() -> Option<String> {
        let path: PathBuf = std::env::current_exe().ok()?;
        let display = path.display().to_string();
        if display.is_empty() {
            return None;
        }
        Some(format!("\"{display}\""))
    }

    pub(super) fn registered() -> bool {
        if let Ok(key) = open_for_access(KEY_QUERY_VALUE) {
            let name = wide(VALUE_NAME);
            let mut size = 0u32;
            let status = {
                // SAFETY: `name` is null-terminated; only the optional
                // out-parameter `data_size` is written.
                unsafe {
                    RegQueryValueExW(
                        key,
                        name.as_ptr(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        &raw mut size,
                    )
                }
            };
            let _ = {
                // SAFETY: closing the key opened by `open_for_access`.
                unsafe { RegCloseKey(key) }
            };
            return status == ERROR_SUCCESS;
        }
        false
    }

    pub(super) fn set(enable: bool) -> io::Result<()> {
        if !enable {
            return remove();
        }
        let Some(command) = exe_command() else {
            return Err(io::Error::other("executable path unavailable"));
        };
        let key = open_for_access(KEY_SET_VALUE)?;
        let name = wide(VALUE_NAME);
        let mut data = wide(&command);
        data.pop(); // `data_size` below accounts for the terminating null.
        let data_size = u32::try_from(data.len()).unwrap_or_default() * 2 + 2;
        let status = {
            // SAFETY: `data` is a `u16` buffer passed with its byte length;
            // the value name is null-terminated.
            unsafe {
                RegSetValueExW(
                    key,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr().cast(),
                    data_size,
                )
            }
        };
        let _ = {
            // SAFETY: closing the key opened by `open_for_access`.
            unsafe { RegCloseKey(key) }
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status))
        }
    }

    fn remove() -> io::Result<()> {
        let Ok(key) = open_for_access(KEY_SET_VALUE) else {
            return Ok(()); // no key means not registered; nothing to remove
        };
        let name = wide(VALUE_NAME);
        let status = {
            // SAFETY: `name` is a null-terminated local buffer.
            unsafe { RegDeleteValueW(key, name.as_ptr()) }
        };
        let _ = {
            // SAFETY: closing the key opened by `open_for_access`.
            unsafe { RegCloseKey(key) }
        };
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status))
        }
    }
}

#[cfg(not(windows))]
mod native {
    use std::io;

    pub(super) fn registered() -> bool {
        false
    }

    pub(super) fn set(_enable: bool) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "autostart lands with the tray on this platform",
        ))
    }
}
