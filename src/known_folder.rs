//! The shell's own answers for the user's pictures and videos folders.
//!
//! `%USERPROFILE%\Pictures` is only the default location. Once `OneDrive`'s
//! Known Folder Move has run, the profile still holds a local `Pictures`
//! folder — often a stale one — while Explorer, Paint, and every file
//! picker open the redirected folder under `OneDrive`. Writing screenshots
//! to the profile copy therefore hides them from the place the user looks.
//!
//! `SHGetKnownFolderPath` asks the shell where a known folder actually
//! lives for this user, which follows Known Folder Move, the older Folder
//! Redirection policy, and a folder the user moved by hand. Videos are not
//! part of Known Folder Move (their size keeps them local), but the same
//! call answers whichever policy did move them, and answers the profile
//! folder on a machine where nothing has been redirected.

use std::ffi::c_void;
use std::path::PathBuf;

use crate::winapi::Guid;

/// `FOLDERID_Pictures` (knownfolders.h).
const PICTURES: Guid = Guid {
    a: 0x33E2_8130,
    b: 0x4E1E,
    c: 0x4676,
    d: [0x83, 0x5A, 0x98, 0x39, 0x5C, 0x3B, 0xC3, 0xBB],
};

/// `FOLDERID_Videos` (knownfolders.h).
const VIDEOS: Guid = Guid {
    a: 0x1898_9B1D,
    b: 0x99B5,
    c: 0x455B,
    d: [0x84, 0x1C, 0xAB, 0x7C, 0x74, 0xE4, 0xDD, 0xFC],
};

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetKnownFolderPath(
        id: *const Guid,
        flags: u32,
        token: *mut c_void,
        path: *mut *mut u16,
    ) -> i32;
    fn CoTaskMemFree(block: *mut c_void);
}

/// The shell's Pictures folder for this user; `None` when it has no
/// answer (a bare service context, or an ancient Windows).
pub fn pictures() -> Option<PathBuf> {
    folder(&PICTURES)
}

/// The shell's Videos folder for this user; `None` when it has no answer.
pub fn videos() -> Option<PathBuf> {
    folder(&VIDEOS)
}

/// One known folder, resolved for the calling user.
///
/// `KF_FLAG_DEFAULT` (zero) asks for the plain path rather than the
/// create-only or redirected variants, and a null token resolves the
/// current user: the same folder Explorer shows in its navigation pane.
fn folder(id: &Guid) -> Option<PathBuf> {
    let mut answer: *mut u16 = std::ptr::null_mut();
    // SAFETY: `id` is a live `KNOWNFOLDERID` local, the flags and token
    // ask for the current user's plain path, and `answer` is a local the
    // call writes.
    let status = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &raw mut answer) };
    if answer.is_null() {
        return None;
    }
    let resolved = if status < 0 {
        None
    } else {
        Some(read_path(answer))
    };
    // The shell hands the path over in task-allocated memory and expects
    // the caller to free it whether or not the call succeeded.
    // SAFETY: `SHGetKnownFolderPath` allocated this block with the task
    // allocator, and it is ours alone.
    unsafe { CoTaskMemFree(answer.cast::<c_void>()) };
    resolved.filter(|path| !path.as_os_str().is_empty())
}

/// Copy the NUL-terminated answer out of the shell's buffer.
fn read_path(raw: *mut u16) -> PathBuf {
    let mut length = 0usize;
    // SAFETY: the call promises a NUL-terminated string; the loop stops at
    // the terminator, so it never reads past the allocation.
    while unsafe { *raw.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` counts the units before the terminator found above,
    // all inside the same allocation.
    let units = unsafe { std::slice::from_raw_parts(raw, length) };
    PathBuf::from(String::from_utf16_lossy(units))
}

#[cfg(test)]
mod tests {
    use super::{folder, pictures, videos};
    use crate::winapi::Guid;

    #[link(name = "shell32")]
    unsafe extern "system" {
        /// The pre-Vista spelling of the same question, addressed by CSIDL
        /// instead of a GUID; it resolves through the shell's own table, so
        /// it names the folder Windows itself hands out.
        fn SHGetFolderPathW(
            owner: *mut std::ffi::c_void,
            folder: i32,
            token: *mut std::ffi::c_void,
            flags: u32,
            path: *mut u16,
        ) -> i32;
    }

    /// `CSIDL_MYPICTURES` and `CSIDL_MYVIDEO` (shlobj.h).
    const MY_PICTURES: i32 = 39;
    const MY_VIDEO: i32 = 14;

    /// The shell's answer through the other spelling of the question: if a
    /// digit in either GUID above were wrong, the two answers could not
    /// agree.
    fn legacy_answer(csidl: i32) -> Option<std::path::PathBuf> {
        let mut buffer = [0u16; 260];
        // SAFETY: the buffer is a full `MAX_PATH` wide string this call
        // fills; no token or owner handle is needed for a per-user folder.
        let status = unsafe {
            SHGetFolderPathW(
                std::ptr::null_mut(),
                csidl,
                std::ptr::null_mut(),
                0,
                buffer.as_mut_ptr(),
            )
        };
        (status >= 0).then(|| std::path::PathBuf::from(crate::winapi::wide_to_string(&buffer)))
    }

    #[test]
    fn both_folders_agree_with_the_csidl_spelling() {
        if let (Some(mine), Some(theirs)) = (pictures(), legacy_answer(MY_PICTURES)) {
            assert_eq!(mine, theirs, "pictures folder");
        }
        if let (Some(mine), Some(theirs)) = (videos(), legacy_answer(MY_VIDEO)) {
            assert_eq!(mine, theirs, "videos folder");
        }
    }

    #[test]
    fn the_shell_answers_both_media_folders() {
        let Some(pictures) = pictures() else {
            return; // no shell context in this environment
        };
        let Some(videos) = videos() else {
            return;
        };
        // A known-folder answer is an absolute path, and the shell strips
        // its trailing separator; anything else means the copy above read
        // the wrong bytes.
        assert!(pictures.is_absolute(), "pictures: {}", pictures.display());
        assert!(videos.is_absolute(), "videos: {}", videos.display());
        assert_ne!(
            pictures.to_string_lossy().trim_end_matches(['\\', '/']),
            "C:",
            "a drive root is never the pictures folder"
        );
    }

    #[test]
    fn an_unknown_folder_id_answers_nothing() {
        // The all-zero GUID is not a KNOWNFOLDERID: the shell reports a
        // failure instead of a path, and the buffer stays null.
        assert!(folder(&Guid::default()).is_none());
    }
}
