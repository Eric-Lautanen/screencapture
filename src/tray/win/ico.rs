//! Authored tray icon: the committed `assets/icon.ico` is compiled into
//! the binary via `include_bytes!`; one entry is handed to
//! `CreateIconFromResourceEx` at tray-creation time. The system default
//! icon remains the fallback whenever parsing or creation fails.

use std::ffi::c_void;

use super::ffi::{Hicon, IDI_APPLICATION, LoadIconW};

/// The packed multi-size icon shipped in `assets/icon.ico`; its SVG
/// master is `assets/icon.svg`.
static ICON: &[u8] = include_bytes!("../../../assets/icon.ico");

#[link(name = "user32")]
unsafe extern "system" {
    fn CreateIconFromResourceEx(
        bits: *const c_void,
        size: u32,
        icon: i32,
        version: u32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> Hicon;
}

/// `LR_DEFAULTSIZE`: zero width/height means "use the system icon size".
const LR_DEFAULTSIZE: u32 = 0x0000_0040;

/// Resource format version for `RT_ICON` payloads.
const ICON_VERSION: u32 = 0x0003_0000;

/// Tray icon size to prefer (Windows scales gracefully from either side).
const PREFERRED_SIZE: u8 = 32;

/// The authored icon, or the system default when that cannot be built.
///
/// # Safety
/// Must run on a thread that owns the tray window (or none at all).
pub(super) unsafe fn app_or_fallback() -> Hicon {
    // SAFETY: only handle-bit inspection and load calls; both are safe
    // from the message thread this runs on.
    if let Some(icon) = app_icon() {
        return icon;
    }
    crate::log::note(
        "warn",
        "tray: icon resource unusable; using the system icon",
    );
    // SAFETY: null instance resolves a shared system resource.
    unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION as *const u16) }
}

/// Parse the embedded .ico and materialize its best entry.
fn app_icon() -> Option<Hicon> {
    let entry = select_entry(ICON)?;
    let size = u32::try_from(entry.len()).ok()?;
    // SAFETY: `entry` points at a well-formed, static icon resource; the
    // returned handle lives for the process lifetime by design.
    let icon = unsafe {
        CreateIconFromResourceEx(
            entry.as_ptr().cast::<c_void>(),
            size,
            1,
            ICON_VERSION,
            0,
            0,
            LR_DEFAULTSIZE,
        )
    };
    (!icon.is_null()).then_some(icon)
}

/// One `ICONDIRENTRY`: dimensions, image byte length, image offset.
struct Entry {
    width: u8,
    bytes: u32,
    offset: u32,
}

/// Directory entry count from the `ICONDIR` header.
fn count(data: &[u8]) -> Option<usize> {
    Some(usize::from(u16::from_le_bytes([
        *data.get(4)?,
        *data.get(5)?,
    ])))
}

/// A four-byte little-endian word at `start`.
fn word(data: &[u8], start: usize) -> Option<u32> {
    let slice = data.get(start..start.checked_add(4)?)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

/// Decode directory entry `index`, bounds-checked throughout.
fn entry(data: &[u8], index: usize) -> Option<Entry> {
    let base = 6 + index.checked_mul(16)?;
    Some(Entry {
        width: *data.get(base)?,
        bytes: word(data, base + 8)?,
        offset: word(data, base + 12)?,
    })
}

/// The entry closest to the preferred tray size; `None` when the file
/// has no usable entry or its directory points outside the data.
fn select_entry(data: &[u8]) -> Option<&[u8]> {
    let total = count(data)?;
    let chosen = (0..total)
        .filter_map(|index| entry(data, index))
        .filter(|entry| entry.width != 0)
        .min_by_key(|entry| u32::from(entry.width.abs_diff(PREFERRED_SIZE)))?;
    let start = usize::try_from(chosen.offset).ok()?;
    let end = start.checked_add(usize::try_from(chosen.bytes).ok()?)?;
    (data.len() >= end).then(|| &data[start..end])
}

#[cfg(test)]
mod tests {
    use super::{ICON, PREFERRED_SIZE, select_entry};

    /// A minimal two-entry icon (16px and 32px) with dummy payloads.
    fn sample() -> Vec<u8> {
        let image: u32 = 6 + 32;
        let mut data = vec![0u8, 0u8, 1u8, 0u8, 2u8, 0u8];
        for (width, offset) in [(16u8, image), (32u8, image + 64)] {
            data.push(width);
            data.push(width);
            data.push(0);
            data.push(0);
            data.extend_from_slice(&1u16.to_le_bytes());
            data.extend_from_slice(&32u16.to_le_bytes());
            data.extend_from_slice(&64u32.to_le_bytes());
            data.extend_from_slice(&offset.to_le_bytes());
        }
        data.extend(std::iter::repeat_n(0xAA, 64));
        data.extend(std::iter::repeat_n(0xBB, 64));
        data
    }

    #[test]
    fn preferred_size_is_selected() {
        let binding = sample();
        let selected = select_entry(&binding).expect("a valid entry");
        assert_eq!(selected.first(), Some(&0xBB));
        assert_eq!(selected.len(), 64);
    }

    #[test]
    fn malformed_files_are_refused() {
        assert!(select_entry(&[]).is_none());
        assert!(select_entry(&[0, 0, 2, 0, 0, 0]).is_none());
    }

    #[test]
    fn embedded_icon_has_a_usable_entry() {
        let selected = select_entry(ICON).expect("committed icon parses");
        assert!(selected.len() >= 40, "entry must at least hold a header");
    }

    #[test]
    fn preferred_matches_windows_tray_metrics() {
        assert_eq!(PREFERRED_SIZE, 32);
    }
}
