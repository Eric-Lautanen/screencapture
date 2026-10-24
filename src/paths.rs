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

/// The folders earlier builds wrote as their own defaults, in full.
///
/// A settings file that still names one of these never chose anything: it
/// is following a default that has since moved, so the loader lets today's
/// default win instead. This matters most on Windows, where `OneDrive`'s
/// Known Folder Move redirects `Pictures` — and every screenshot written
/// to the old profile path is a file the user cannot find.
pub(crate) fn legacy_media_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // The pre-subfolder roots, wherever each platform still puts them.
    if let Some(root) = legacy_video_root() {
        dirs.push(root.join(APP_DIR));
        dirs.push(root);
    }
    if cfg!(windows)
        && let Some(pictures) = home_dir().map(|home| home.join("Pictures").join(APP_DIR))
    {
        dirs.push(pictures);
    }
    dirs
}

/// The videos root as this build's predecessors resolved it, before the
/// shell lookup: the profile folder, never the redirected one.
fn legacy_video_root() -> Option<PathBuf> {
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

/// The user's videos folder, as the shell resolves it: that answer follows
/// a redirect (Folder Redirection, or a folder the user moved) instead of
/// assuming the profile copy is the one Explorer shows.
#[cfg(windows)]
fn video_root() -> Option<PathBuf> {
    crate::known_folder::videos().or_else(legacy_video_root)
}

#[cfg(not(windows))]
fn video_root() -> Option<PathBuf> {
    legacy_video_root()
}

/// The user's pictures folder, by the same rules as [`video_root`].
#[cfg(windows)]
fn image_root() -> Option<PathBuf> {
    crate::known_folder::pictures().or_else(profile_pictures)
}

#[cfg(not(windows))]
fn image_root() -> Option<PathBuf> {
    profile_pictures()
}

fn profile_pictures() -> Option<PathBuf> {
    home_dir().map(|home| home.join("Pictures"))
}

/// Default destination folder for still photos.
pub fn image_dir() -> Option<PathBuf> {
    if cfg!(windows) || cfg!(target_os = "macos") {
        image_root().map(|pictures| pictures.join(APP_DIR))
    } else {
        match env::var_os("XDG_PICTURES_DIR") {
            Some(xdg) if Path::new(&xdg).is_absolute() => Some(PathBuf::from(xdg).join(APP_DIR)),
            _ => image_root().map(|pictures| pictures.join(APP_DIR)),
        }
    }
}

/// The user's home directory, when the platform exports one.
pub(crate) fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(key).map(PathBuf::from)
}
