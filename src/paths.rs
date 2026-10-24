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

/// Default destination folder for recordings: the user's video folder
/// with a `Screencapture` subfolder, mirroring `Pictures/Screencapture`
/// for stills so recordings never mix with personal videos.
pub fn video_dir() -> Option<PathBuf> {
    video_root().map(|videos| videos.join(APP_DIR))
}

/// Where recordings used to land by default, before the subfolder: the
/// settings loader treats this exact folder as "never chosen" so an
/// existing config follows the new default instead of writing loose
/// files beside the user's own videos.
pub(crate) fn legacy_video_dir() -> Option<PathBuf> {
    video_root()
}

fn video_root() -> Option<PathBuf> {
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

/// Default destination folder for still photos.
pub fn image_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        home_dir().map(|home| home.join("Pictures").join(APP_DIR))
    } else if cfg!(target_os = "macos") {
        home_dir().map(|home| home.join("Pictures").join(APP_DIR))
    } else {
        match env::var_os("XDG_PICTURES_DIR") {
            Some(xdg) if Path::new(&xdg).is_absolute() => Some(PathBuf::from(xdg).join(APP_DIR)),
            _ => home_dir().map(|home| home.join("Pictures").join(APP_DIR)),
        }
    }
}

/// The user's home directory, when the platform exports one.
pub(crate) fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(key).map(PathBuf::from)
}
