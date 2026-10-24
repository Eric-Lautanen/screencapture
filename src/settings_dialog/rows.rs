//! The settings dialog's control rows: the grouped layout (Capture /
//! Recording / Still photo / Hotkeys / Folders group boxes over aligned
//! label-column rows), the control builders, and the getters Save reads
//! back through.

use std::iter;

use crate::encoder::Video;
use crate::recording::Profile;
use crate::screen;
use crate::settings::{CaptureMode, Settings, StillDepth};
use crate::winapi::{CreateWindowExW, Hwnd, Lparam, Wparam};

use super::ffi::{
    BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_GROUPBOX, BS_PUSHBUTTON, CB_ADDSTRING,
    CB_SETCURSEL, CBS_DROPDOWNLIST, ES_AUTOHSCROLL, HWND_TOP, SS_LEFT, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SendMessageW, SetWindowPos, WM_SETFONT, WS_CHILD, WS_CLIPSIBLINGS,
    WS_EX_CLIENTEDGE, WS_TABSTOP, WS_VISIBLE,
};
use super::frame::scaled;
use super::{
    CHECK_AUTOSTART, CHECK_CHIMES, CHECK_CURSOR, COMBO_CODEC, COMBO_DEPTH, COMBO_DISPLAY,
    COMBO_MODE, COMBO_PROFILE, DIALOG, EDIT_CHIMES, EDIT_IMAGES, EDIT_PHOTO, EDIT_RECORD,
    EDIT_VIDEOS, module_handle, wide,
};

/// Row margin inside a group box, and the two aligned columns.
const LEFT: i32 = 32;
/// Control column: every combo/edit/button answer lines up here.
const RIGHT: i32 = 170;
/// Label column width: stop short of [`RIGHT`] so a caption never runs
/// under the control beside it.
const LABEL_W: i32 = RIGHT - LEFT - 8;

/// On-screen position and size of a child control, in pixels.
#[derive(Clone, Copy)]
struct Place {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

/// Every child control, seeded from the current settings.
pub(super) fn populate(settings: &Settings, window: Hwnd) {
    capture_group(settings, window);
    recording_group(settings, window);

    group(window, 16, 244, 458, 56, "Still photo");
    let depth_index = match settings.photo_depth {
        StillDepth::Eight => 0usize,
        StillDepth::Sixteen => 1,
    };
    combo_row(
        window,
        COMBO_DEPTH,
        "Photo depth",
        266,
        &["8-bit".to_owned(), "16-bit".to_owned()],
        depth_index,
    );

    checkbox(
        window,
        CHECK_AUTOSTART,
        "Start screencapture when Windows starts",
        LEFT,
        312,
        settings.autostart,
        420,
    );
    checkbox(
        window,
        CHECK_CHIMES,
        "Play capture chimes",
        LEFT,
        344,
        settings.chimes,
        420,
    );

    hotkeys_group(settings, window);
    folders_group(settings, window);

    push_button(window, super::BTN_SAVE, "Save", 318, 634);
    push_button(window, super::BTN_CANCEL, "Cancel", 402, 634);
    settle_children();
}

/// Restore intended paint order: Windows places each newly created child at
/// the bottom of the sibling Z order, so raising them in creation order leaves
/// group frames at the bottom and interactive controls on top.
fn settle_children() {
    let children = DIALOG.with_borrow(|dialog| dialog.children.clone());
    for handle in children {
        // SAFETY: live child windows created by this dialog.
        unsafe {
            SetWindowPos(
                handle,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

/// `Capture`: what the hotkey session targets (mode, display choice).
fn capture_group(settings: &Settings, window: Hwnd) {
    group(window, 16, 12, 458, 92, "Capture");
    let mode_index = match settings.capture_mode {
        CaptureMode::Full => 0,
        CaptureMode::Region => 1,
        CaptureMode::Window => 2,
    };
    combo_row(
        window,
        COMBO_MODE,
        "Capture mode",
        34,
        &[
            "Full screen".to_owned(),
            "Drawn region".to_owned(),
            "Picked window".to_owned(),
        ],
        mode_index,
    );
    let count = attached_count();
    let display_index = settings
        .display
        .number()
        .map_or(0, |number| usize::min(number as usize, count));
    combo_row(
        window,
        COMBO_DISPLAY,
        "Display",
        66,
        &display_entries(count),
        display_index,
    );
}

/// `Recording`: profile, codec, and the capture-behavior checkboxes.
fn recording_group(settings: &Settings, window: Hwnd) {
    group(window, 16, 112, 458, 124, "Recording");
    let profile_entries: Vec<String> = Profile::PROFILES
        .iter()
        .map(|profile| format!("{} ({}×{})", profile.label, profile.width, profile.height))
        .collect();
    let profile_index = Profile::PROFILES
        .iter()
        .position(|profile| profile.id == settings.profile.id)
        .unwrap_or(0);
    combo_row(
        window,
        COMBO_PROFILE,
        "Video profile",
        134,
        &profile_entries,
        profile_index,
    );
    let codecs = codec_list();
    let codec_entries: Vec<String> = codecs.iter().map(|codec| codec.name().to_owned()).collect();
    let codec_index = codecs
        .iter()
        .position(|codec| *codec == settings.codec)
        .unwrap_or(0);
    combo_row(
        window,
        COMBO_CODEC,
        "Video codec",
        166,
        &codec_entries,
        codec_index,
    );
    checkbox(
        window,
        CHECK_CURSOR,
        "Include mouse cursor in captures",
        LEFT,
        198,
        settings.cursor,
        420,
    );
}

/// `Hotkeys`: the three free combos, seeded with their canonical forms.
fn hotkeys_group(settings: &Settings, window: Hwnd) {
    group(window, 16, 372, 458, 128, "Hotkeys");
    label(window, "Record hotkey", LEFT, 396);
    edit_box(
        window,
        EDIT_RECORD,
        &settings.hotkey.to_combo(),
        RIGHT,
        392,
        272,
    );
    label(window, "Photo hotkey", LEFT, 428);
    edit_box(
        window,
        EDIT_PHOTO,
        &settings.photo.to_combo(),
        RIGHT,
        424,
        272,
    );
    label(window, "Chime hotkey", LEFT, 460);
    edit_box(
        window,
        EDIT_CHIMES,
        &settings.chime_hotkey.to_combo(),
        RIGHT,
        456,
        272,
    );
    let Some(status) = create_child(
        window,
        "STATIC",
        "Click a field, then press the keys",
        SS_LEFT | WS_CHILD | WS_VISIBLE,
        Place {
            x: LEFT,
            y: 480,
            width: 420,
            height: 16,
        },
        0,
        0,
    ) else {
        return;
    };
    DIALOG.with_borrow_mut(|dialog| dialog.hotkey_status = Some(status));
}

/// `Folders`: both destinations, each with a native Browse button.
fn folders_group(settings: &Settings, window: Hwnd) {
    group(window, 16, 506, 458, 100, "Folders");
    label(window, "Videos folder", LEFT, 532);
    edit_box(
        window,
        EDIT_VIDEOS,
        &settings.video_dir.to_string_lossy(),
        RIGHT,
        528,
        200,
    );
    push_button(window, super::BTN_BROWSE_VIDEOS, "Browse…", 388, 526);
    label(window, "Images folder", LEFT, 564);
    edit_box(
        window,
        EDIT_IMAGES,
        &settings.image_dir.to_string_lossy(),
        RIGHT,
        560,
        200,
    );
    push_button(window, super::BTN_BROWSE_IMAGES, "Browse…", 388, 558);
}

/// How many displays are attached right now (at least one).
pub(super) fn attached_count() -> usize {
    screen::displays().len().max(1)
}

/// Display entries: `primary` then `1`..`N` attached.
pub(super) fn display_entries(count: usize) -> Vec<String> {
    iter::once(String::from("primary"))
        .chain((1..=count).map(|number| number.to_string()))
        .collect()
}

/// Every codec choice, in display order: software first, hardware after.
pub(super) fn codec_list() -> Vec<Video> {
    vec![
        Video::X264,
        Video::X265,
        Video::AmfH264,
        Video::QsvH264,
        Video::NvencH264,
        Video::MfH264,
        Video::AmfHevc,
        Video::QsvHevc,
        Video::NvencHevc,
        Video::MfHevc,
    ]
}

/// One label row + combo row pair, aligned to the shared columns.
fn combo_row(window: Hwnd, id: i32, caption: &str, y: i32, entries: &[String], chosen: usize) {
    label(window, caption, LEFT, y + 4);
    let Some(handle) = create_child(
        window,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST | WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        Place {
            x: RIGHT,
            y,
            // A dropped list needs room; the closed control still renders
            // one line, but the drop-down extent is this height.
            width: 272,
            height: 240,
        },
        id,
        0,
    ) else {
        return;
    };
    for entry in entries {
        let text = wide(entry);
        // SAFETY: the combobox owns its list; `text` lives through the call.
        unsafe {
            SendMessageW(handle, CB_ADDSTRING, 0, text.as_ptr() as Lparam);
        }
    }
    // SAFETY: live combobox and scalar selection.
    unsafe {
        SendMessageW(handle, CB_SETCURSEL, chosen, 0);
    }
    DIALOG.with_borrow_mut(|dialog| dialog.combos.push((id, handle)));
}

/// One labelled checkbox aligned to the label column.
fn checkbox(window: Hwnd, id: i32, caption: &str, x: i32, y: i32, on: bool, width: i32) {
    let Some(handle) = create_child(
        window,
        "BUTTON",
        caption,
        BS_AUTOCHECKBOX | WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        Place {
            x,
            y,
            width,
            height: 18,
        },
        id,
        0,
    ) else {
        return;
    };
    // SAFETY: live checkbox and scalar state.
    unsafe {
        SendMessageW(handle, BM_SETCHECK, usize::from(on), 0);
    }
    DIALOG.with_borrow_mut(|dialog| dialog.checks.push((id, handle)));
}

/// One caption line at `x`/`y`.
fn label(window: Hwnd, caption: &str, x: i32, y: i32) {
    let _ = create_child(
        window,
        "STATIC",
        caption,
        SS_LEFT | WS_CHILD | WS_VISIBLE,
        Place {
            x,
            y,
            width: LABEL_W,
            height: 16,
        },
        0,
        0,
    );
}

/// One sunken text entry seeded with `value`.
fn edit_box(window: Hwnd, id: i32, value: &str, x: i32, y: i32, width: i32) {
    let Some(handle) = create_child(
        window,
        "EDIT",
        value,
        ES_AUTOHSCROLL | WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        Place {
            x,
            y,
            width,
            height: 22,
        },
        id,
        WS_EX_CLIENTEDGE,
    ) else {
        return;
    };
    DIALOG.with_borrow_mut(|dialog| dialog.edits.push((id, handle)));
    if id == EDIT_RECORD || id == EDIT_PHOTO || id == EDIT_CHIMES {
        super::keycapture::install(handle, id, value);
    }
}

/// One push button (Save is the default).
fn push_button(window: Hwnd, id: i32, caption: &str, x: i32, y: i32) {
    let style = if id == super::BTN_SAVE {
        BS_DEFPUSHBUTTON
    } else {
        BS_PUSHBUTTON
    };
    let _ = create_child(
        window,
        "BUTTON",
        caption,
        style | WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        Place {
            x,
            y,
            width: 76,
            height: 28,
        },
        id,
        0,
    );
}

/// One group-box rectangle with its titled caption.
fn group(window: Hwnd, x: i32, y: i32, width: i32, height: i32, caption: &str) {
    let _ = create_child(
        window,
        "BUTTON",
        caption,
        BS_GROUPBOX | WS_CHILD | WS_VISIBLE,
        Place {
            x,
            y,
            width,
            height,
        },
        0,
        0,
    );
}

/// Common child-creation path; `None` when Windows refused. Applies the
/// dialog font (`WM_SETFONT`) so text renders in `Segoe UI`, not the
/// raw default.
fn create_child(
    window: Hwnd,
    class: &str,
    text: &str,
    style: u32,
    place: Place,
    id: i32,
    style_ex: u32,
) -> Option<Hwnd> {
    let class_name = class;
    let class = wide(class);
    let text = wide(text);
    let scale = DIALOG.with_borrow(|dialog| dialog.scale);
    // SAFETY: strings outlive the call; the parent owns the child after it.
    let handle = unsafe {
        CreateWindowExW(
            style_ex,
            class.as_ptr(),
            text.as_ptr(),
            style | WS_CLIPSIBLINGS,
            scaled(place.x, scale),
            scaled(place.y, scale),
            scaled(place.width, scale),
            scaled(place.height, scale),
            window,
            usize::try_from(id).unwrap_or(0) as *mut std::ffi::c_void,
            module_handle(),
            std::ptr::null_mut(),
        )
    };
    if handle.is_null() {
        crate::log::note(
            "settings",
            &format!(
                "cannot create {class_name} control {id}: {}",
                std::io::Error::last_os_error()
            ),
        );
        return None;
    }
    if let Some(font) = DIALOG.with_borrow(|dialog| dialog.font) {
        // SAFETY: a live child handle and an owned font handle.
        unsafe {
            SendMessageW(handle, WM_SETFONT, font as Wparam, 1);
        }
    }
    DIALOG.with_borrow_mut(|dialog| dialog.children.push(handle));
    Some(handle)
}

#[cfg(test)]
mod tests {
    use super::{attached_count, codec_list, display_entries};

    #[test]
    fn display_entries_start_at_primary() {
        let entries = display_entries(3);
        assert_eq!(entries.first().map(String::as_str), Some("primary"));
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn codec_list_matches_every_variant() {
        let codecs = codec_list();
        assert_eq!(codecs.len(), 10);
        assert_eq!(codecs[0].id(), "x264");
        assert!(codecs.iter().any(|codec| codec.id() == "nvenc-hevc"));
    }

    #[test]
    fn attached_count_never_zeroes() {
        assert!(attached_count() >= 1);
    }
}
