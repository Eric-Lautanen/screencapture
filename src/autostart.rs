//! Login autostart: the Windows `HKCU\...\Run` value, the macOS
//! `LaunchAgent` plist, and the freedesktop XDG autostart entry.

use std::io;

/// Whether launch-at-login is currently registered.
#[must_use]
pub fn registered() -> bool {
    native::registered()
}

/// Enable or disable launch-at-login.
///
/// # Errors
/// Propagates platform errors (registry on Windows; file operations on
/// macOS and Linux).
pub fn set(enable: bool) -> io::Result<()> {
    native::set(enable)
}

#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::io;
    use std::path::PathBuf;

    use crate::winapi::wide;

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

    /// Whether the login registration exists *and* still launches this
    /// executable. A value left behind by a previous install answers
    /// `false`, so the startup sync rewrites it instead of silently
    /// starting the old path (or nothing, once that path is gone).
    pub(super) fn registered() -> bool {
        let Ok(key) = open_for_access(KEY_QUERY_VALUE) else {
            return false;
        };
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
        let stored = if status == ERROR_SUCCESS {
            read_command(key, name.as_ptr(), size)
        } else {
            None
        };
        let _ = {
            // SAFETY: closing the key opened by `open_for_access`.
            unsafe { RegCloseKey(key) }
        };
        stored.is_some_and(|command| launches_this_exe(&command))
    }

    /// The registered command string; `None` when the value is empty,
    /// absurdly large, or unreadable.
    fn read_command(key: Hkey, name: *const u16, size: u32) -> Option<String> {
        /// Generous ceiling for one `Run` command; real entries are a path.
        const MAX_BYTES: u32 = 4096;
        if !(2..=MAX_BYTES).contains(&size) {
            return None;
        }
        let units = usize::try_from(size).ok()? / 2 + 1;
        let mut buffer = vec![0u16; units];
        let mut copied = size;
        let status = {
            // SAFETY: `buffer` holds `units` UTF-16 slots (at least `size`
            // bytes); `copied` is the in/out byte count for the value.
            unsafe {
                RegQueryValueExW(
                    key,
                    name,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    buffer.as_mut_ptr().cast(),
                    &raw mut copied,
                )
            }
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let end = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    /// Whether `command` names this executable; quotes around the path are
    /// the form [`set`] writes, but a hand-edited unquoted entry counts too.
    fn launches_this_exe(command: &str) -> bool {
        let trimmed = command.trim().trim_matches('"');
        std::env::current_exe()
            .is_ok_and(|current| trimmed.eq_ignore_ascii_case(&current.display().to_string()))
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

    #[cfg(test)]
    mod tests {
        use super::launches_this_exe;

        #[test]
        fn only_this_executable_counts_as_registered() {
            let current = std::env::current_exe().expect("the test binary has a path");
            // Both forms the app writes and a hand edit might leave behind.
            assert!(launches_this_exe(&format!("\"{}\"", current.display())));
            assert!(launches_this_exe(&current.display().to_string()));
            // A value pointing at a previous install is stale, not registered.
            assert!(!launches_this_exe("C:\\old\\screencapture.exe"));
        }
    }
}

/// Registration for the two Unix mechanisms below: the macOS `LaunchAgent`
/// plist (`Library/LaunchAgents`) and the freedesktop XDG autostart entry
/// (`.config/autostart`). Both are plain files the desktop reads on
/// login, so registration is file presence and toggling is write/remove.
#[cfg(unix)]
mod native {
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    use crate::paths;

    /// Reverse-DNS label `launchd` also reads from the plist file name.
    const LABEL: &str = "dev.screencapture";

    /// XDG entry name before the `.desktop` suffix.
    const ENTRY: &str = "screencapture";

    /// macOS user agent directory (launchd's login scan point).
    const LAUNCH_AGENTS: &str = "Library/LaunchAgents";

    /// Whether launch-at-login is currently registered.
    #[must_use]
    pub fn registered() -> bool {
        match layout() {
            Some((home, xdg_config)) => document_path(&home, xdg_config.as_deref()).exists(),
            None => false,
        }
    }

    /// Enable or disable launch-at-login.
    ///
    /// # Errors
    /// Propagates filesystem errors; a missing home directory fails with
    /// a resolution notice.
    pub fn set(enable: bool) -> io::Result<()> {
        let Some((home, xdg_config)) = layout() else {
            return Err(io::Error::other("cannot resolve the user directory"));
        };
        let path = document_path(&home, xdg_config.as_deref());
        if enable {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let program = std::env::current_exe()?;
            fs::write(&path, document(&program))?;
        } else {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// The home directory plus the absolute `XDG_CONFIG_HOME` override.
    fn layout() -> Option<(PathBuf, Option<PathBuf>)> {
        let home = paths::home_dir()?;
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|value| value.is_absolute());
        Some((home, xdg))
    }

    /// The autostart document for this platform's mechanism.
    fn document_path(home: &Path, xdg_config: Option<&Path>) -> PathBuf {
        if cfg!(target_os = "macos") {
            home.join(LAUNCH_AGENTS).join(format!("{LABEL}.plist"))
        } else {
            let default = home.join(".config");
            let config = xdg_config.unwrap_or(&default);
            config.join("autostart").join(format!("{ENTRY}.desktop"))
        }
    }

    /// The registration document text for the current platform.
    fn document(program: &Path) -> String {
        if cfg!(target_os = "macos") {
            agent_plist(program)
        } else {
            desktop_entry(program)
        }
    }

    /// The `LaunchAgent` document registering `program` at login.
    fn agent_plist(program: &Path) -> String {
        format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
                "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
                "<plist version=\"1.0\">\n",
                "<dict>\n",
                "  <key>Label</key>\n  <string>{}</string>\n",
                "  <key>ProgramArguments</key>\n  <array>\n",
                "    <string>{}</string>\n  </array>\n",
                "  <key>RunAtLoad</key>\n  <true/>\n",
                "</dict>\n</plist>\n"
            ),
            LABEL,
            xml_text(&program.to_string_lossy())
        )
    }

    /// XML character data: escape the delimiters markup otherwise eats.
    fn xml_text(raw: &str) -> String {
        raw.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// The freedesktop autostart entry registering `program` at login.
    fn desktop_entry(program: &Path) -> String {
        format!(
            concat!(
                "[Desktop Entry]\n",
                "Type=Application\n",
                "Name=Screencapture\n",
                "Comment=Start the Screencapture tray app at login\n",
                "Exec={}\n",
                "Terminal=false\n",
                "Categories=Utility;Video\n",
                "X-GNOME-Autostart-enabled=true\n"
            ),
            exec_value(&program.to_string_lossy())
        )
    }

    /// An `Exec=` value per the desktop entry spec: shell-level quoting
    /// with backslash escapes around the whole program path.
    fn exec_value(raw: &str) -> String {
        let escaped = raw.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    }

    #[cfg(test)]
    mod tests {
        use super::{agent_plist, desktop_entry, document_path, exec_value, xml_text};
        use std::path::{Path, PathBuf};

        #[test]
        fn the_plist_registers_at_run_load() {
            let text = agent_plist(Path::new("/opt/app/screencapture"));
            assert!(text.starts_with("<?xml version=\"1.0\""));
            assert!(text.contains("<string>dev.screencapture</string>"));
            assert!(text.contains("<string>/opt/app/screencapture</string>"));
            assert!(text.contains("<key>RunAtLoad</key>"));
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn the_plist_path_lives_in_launch_agents() {
            let path = document_path(Path::new("/Users/ericl"), None);
            assert_eq!(
                path,
                PathBuf::from("/Users/ericl/Library/LaunchAgents/dev.screencapture.plist")
            );
        }

        #[test]
        fn xml_delimiters_are_escaped() {
            assert_eq!(xml_text("a&b<c>"), "a&amp;b&lt;c&gt;");
        }

        #[test]
        fn the_desktop_entry_quotes_exec() {
            let text = desktop_entry(Path::new("/opt/app/screencapture"));
            assert!(text.contains("Exec=\"/opt/app/screencapture\""));
            assert!(text.contains("X-GNOME-Autostart-enabled=true"));
        }

        #[test]
        fn exec_values_escape_quotes_and_backslashes() {
            assert_eq!(
                exec_value("/opt/with \"spaces\"/app"),
                "\"/opt/with \\\"spaces\\\"/app\""
            );
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn the_xdg_path_honors_the_config_home() {
            let home = Path::new("/home/ericl");
            assert_eq!(
                document_path(home, None),
                PathBuf::from("/home/ericl/.config/autostart/screencapture.desktop")
            );
            assert_eq!(
                document_path(home, Some(Path::new("/custom/xdg"))),
                PathBuf::from("/custom/xdg/autostart/screencapture.desktop")
            );
        }
    }
}
