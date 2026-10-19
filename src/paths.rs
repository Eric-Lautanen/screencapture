//! Filesystem locations, resolved with `std` only.

use std::env;
use std::path::{Path, PathBuf};

const APP_DIR: &str = "Screencapture";

/// Directory holding settings; `None` when the platform provides no home.
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        match env::var_os("APPDATA") {
            Some(base) => Some(PathBuf::from(base).join(APP_DIR)),
            None => home_dir().map(|home| home.join("AppData/Roaming").join(APP_DIR)),
        }
    } else if cfg!(target_os = "macos") {
        home_dir().map(|home| home.join("Library/Application Support").join(APP_DIR))
    } else {
        match env::var_os("XDG_CONFIG_HOME") {
            Some(xdg) if Path::new(&xdg).is_absolute() => Some(PathBuf::from(xdg).join(APP_DIR)),
            _ => home_dir().map(|home| home.join(".config").join(APP_DIR)),
        }
    }
}

/// Default destination folder for recordings and still captures.
pub fn media_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        home_dir().map(|home| home.join("Videos"))
    } else if cfg!(target_os = "macos") {
        home_dir().map(|home| home.join("Movies"))
    } else {
        match env::var_os("XDG_VIDEOS_DIR") {
            Some(xdg) if Path::new(&xdg).is_absolute() => Some(PathBuf::from(xdg)),
            _ => home_dir().map(|home| home.join("Videos")),
        }
    }
}

/// The user's home directory, when the platform exports one.
pub(crate) fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(key).map(PathBuf::from)
}
