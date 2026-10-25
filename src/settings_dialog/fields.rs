//! The settings dialog's control read-back and native folder browser:
//! the getters `save()` uses to collect the current combo/edit/checkbox
//! state, plus the shell folder-pick path and its RAII `Pidl` owner.

use std::fs;
use std::path::Path;

use crate::winapi::Hwnd;

use super::DIALOG;
use super::ffi::{
    BM_GETCHECK, BrowseInfo, CB_GETCURSEL, CoTaskMemFree, FOLDER_FLAGS, ItemList, MAX_PATH,
    SHBrowseForFolderW, SHGetPathFromIDListW, SendMessageW, SetWindowTextW,
};

/// One shell item-list pointer (`LPITEMIDLIST`) the shell allocated for
/// `browse_folder`; `CoTaskMemFree` runs in `Drop`, so the conversion to
/// a path string and every edit-write after it can no longer leak the
/// allocation on panic or early return.
struct Pidl {
    /// The live pointer; `None` after `Drop` released it (or when the
    /// wrap rejected a null answer for a cancelled pick).
    list: Option<*mut ItemList>,
}

impl Pidl {
    /// Own a pidl the shell handed out; `None` for a null (cancelled).
    ///
    /// # Safety
    /// `list` must be a shell allocation that `CoTaskMemFree` may free.
    unsafe fn own(list: *mut ItemList) -> Option<Self> {
        (!list.is_null()).then_some(Self { list: Some(list) })
    }

    /// The live pointer for a shell call, or the null substitute `SHGet...`
    /// answers truthfully about.
    fn as_ptr(&self) -> *const ItemList {
        self.list
            .expect("a constructed Pidl always holds its allocation")
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        if let Some(list) = self.list.take() {
            // SAFETY: the shell allocation adopted in `own`.
            unsafe {
                CoTaskMemFree(list.cast::<std::ffi::c_void>());
            }
        }
    }
}

/// Open the native folder browser and, when the user picks one, write
/// the absolute path into the target edit field.
pub(super) fn browse_folder(window: Hwnd, target: i32, title: &str) {
    let title = super::wide(title);
    let mut buffer = [0u16; MAX_PATH];
    let mut info = BrowseInfo {
        owner: window,
        root: std::ptr::null_mut(),
        display_name: std::ptr::null_mut(),
        title: title.as_ptr(),
        flags: FOLDER_FLAGS,
        callback: None,
        callback_data: 0,
        image: 0,
    };
    // SAFETY: the info struct and strings live through the call.
    let list = unsafe { SHBrowseForFolderW(&raw mut info) };
    let Some(owned) = (unsafe { Pidl::own(list) }) else {
        return; // the user cancelled: nothing allocated to free
    };
    // SAFETY: a live shell pidl; the buffer is a local the call fills.
    let got = unsafe { SHGetPathFromIDListW(owned.as_ptr(), buffer.as_mut_ptr()) };
    // `owned` drops here (or on any path below after this point), freeing
    // the pidl exactly once even if a panic interrupts the conversion.
    drop(owned);
    let picked = (got != 0).then(|| super::wide_to_string(&buffer));
    if let Some(path) = picked.filter(|path| Path::new(path).is_absolute()) {
        write_edit(target, &path);
    }
}

/// The live handle of a combo, or `None` when it was not created.
pub(super) fn combo_handle(id: i32) -> Option<Hwnd> {
    DIALOG.with_borrow(|dialog| {
        dialog
            .combos
            .iter()
            .find(|(slot, _)| *slot == id)
            .map(|(_, handle)| *handle)
    })
}

/// The chosen index of a combo, `0` when the handle refused.
pub(super) fn combo_choice(id: i32) -> usize {
    let Some(handle) = combo_handle(id) else {
        return 0;
    };
    // SAFETY: live combobox handle from the dialog itself.
    let picked = unsafe { SendMessageW(handle, CB_GETCURSEL, 0, 0) };
    if picked < 0 {
        0
    } else {
        usize::try_from(picked).unwrap_or(0)
    }
}

/// The text of an edit field, trimmed.
pub(super) fn edit_text(id: i32) -> String {
    use super::ffi::{GetWindowTextLengthW, GetWindowTextW};
    let Some(handle) = DIALOG.with_borrow(|dialog| {
        dialog
            .edits
            .iter()
            .find(|(slot, _)| *slot == id)
            .map(|(_, handle)| *handle)
    }) else {
        return String::new();
    };
    // SAFETY: live edit handle; the length reads the control's text.
    let length = unsafe { GetWindowTextLengthW(handle) };
    if length <= 0 {
        return String::new();
    }
    let Ok(count) = usize::try_from(length).map(|length| length + 1) else {
        return String::new();
    };
    let mut buffer = vec![0u16; count];
    // SAFETY: `buffer` is sized at `length + 1` for the reads below.
    unsafe {
        GetWindowTextW(handle, buffer.as_mut_ptr(), length + 1);
    }
    super::wide_to_string(&buffer).trim().to_owned()
}

/// The checked state of a checkbox.
pub(super) fn check_state(id: i32) -> bool {
    DIALOG
        .with_borrow(|dialog| {
            dialog
                .checks
                .iter()
                .find(|(slot, _)| *slot == id)
                .map(|(_, handle)| *handle)
        })
        .is_some_and(|handle| {
            // SAFETY: live checkbox handle.
            unsafe { SendMessageW(handle, BM_GETCHECK, 0, 0) != 0 }
        })
}

/// Name of the probe file [`ensure_folder`] writes and removes again.
const WRITE_PROBE: &str = ".screencapture-write-test";

/// Create `folder` if it is missing, then prove a file can be written there.
///
/// A folder can exist and still refuse new files — a read-only drive, an
/// ACL that grants only reading, a path that is really a file — and a take
/// that cannot write fails after the user has already recorded it. Asking
/// for the folder here answers the question while the dialog is still open
/// and the field is still in front of them.
///
/// # Errors
/// The reason Save should be refused: the folder could not be created, or
/// it exists but refuses writes.
pub(super) fn ensure_folder(folder: &str) -> Result<(), String> {
    let path = Path::new(folder);
    // A file where the folder should be answers "already exists" from
    // `create_dir_all`, which says nothing useful about what is wrong.
    if path.exists() && !path.is_dir() {
        return Err("is a file, not a folder".to_owned());
    }
    if let Err(error) = fs::create_dir_all(path) {
        return Err(format!("cannot be created: {error}"));
    }
    let probe = path.join(WRITE_PROBE);
    if let Err(error) = fs::write(&probe, []) {
        return Err(format!("cannot be written to: {error}"));
    }
    // The probe is evidence, not output: failing to remove it is no reason
    // to refuse a folder that just accepted the file.
    let _ = fs::remove_file(&probe);
    Ok(())
}

/// Replace the text of the edit identified by `id`.
pub(super) fn write_edit(id: i32, text: &str) {
    DIALOG
        .with_borrow(|dialog| {
            dialog
                .edits
                .iter()
                .find(|(slot, _)| *slot == id)
                .map(|(_, handle)| *handle)
        })
        .inspect(|handle| {
            let encoded = super::wide(text);
            // SAFETY: live edit handle; the text outlives the call.
            unsafe {
                SetWindowTextW(*handle, encoded.as_ptr());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::ensure_folder;
    use std::fs;
    use std::path::PathBuf;

    /// A scratch path under the temp directory, unique to this run.
    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "screencapture-folder-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn a_missing_folder_is_created_and_accepted() {
        let root = scratch("missing");
        let folder = root.join("captures").join("2026");
        assert_eq!(ensure_folder(&folder.to_string_lossy()), Ok(()));
        assert!(folder.is_dir(), "the folder exists for the next take");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_in_the_way_is_refused() {
        let root = scratch("file");
        fs::create_dir_all(&root).expect("scratch folder");
        let file = root.join("not-a-folder");
        fs::write(&file, b"x").expect("scratch file");
        let refused = ensure_folder(&file.to_string_lossy()).unwrap_err();
        assert_eq!(refused, "is a file, not a folder");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_write_probe_does_not_stay_behind() {
        let folder = scratch("probe");
        assert_eq!(ensure_folder(&folder.to_string_lossy()), Ok(()));
        let left: Vec<_> = fs::read_dir(&folder)
            .expect("created folder")
            .map(|entry| entry.expect("readable entry").file_name())
            .collect();
        assert!(left.is_empty(), "the probe is gone: {left:?}");
        let _ = fs::remove_dir_all(&folder);
    }
}
