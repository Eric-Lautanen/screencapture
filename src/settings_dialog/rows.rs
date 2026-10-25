//! The settings dialog's control rows: a two-column sheet with Recording and
//! Hotkeys on the left and Still photo and Audio on the right, full-width
//! Application and Folders bands beneath them, the control builders, and the
//! getters Save reads back through.
//!
//! Every coordinate below is a designed pixel at 96 DPI; `create_child`
//! scales it with the display.

use crate::encoder::Video;
use crate::recording::Profile;
use crate::settings::{Settings, StillDepth, StillQuality};
use crate::winapi::{CreateWindowExW, Hwnd, Lparam, Wparam};

use super::ffi::{
    BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_GROUPBOX, BS_PUSHBUTTON, CB_ADDSTRING,
    CB_SETCURSEL, CBS_DROPDOWNLIST, ES_AUTOHSCROLL, HWND_TOP, SS_LEFT, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SendMessageW, SetWindowPos, TBM_SETPOS, TBM_SETRANGE, TBS_HORZ, WM_SETFONT,
    WS_CHILD, WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE, WS_TABSTOP, WS_VISIBLE,
};
use super::frame::scaled;
use super::{
    CHECK_AUTOSTART, CHECK_CHIMES, CHECK_CURSOR, CHECK_MIC, CHECK_SYSTEM_AUDIO,
    COMBO_AUDIO_QUALITY, COMBO_CODEC, COMBO_DEPTH, COMBO_MIC_DEVICE, COMBO_PROFILE, COMBO_QUALITY,
    COMBO_SYSTEM_DEVICE, DIALOG, EDIT_CHIMES, EDIT_IMAGES, EDIT_PHOTO, EDIT_RECORD, EDIT_VIDEOS,
    SLIDER_MIC_LEVEL, SLIDER_SYSTEM_LEVEL, module_handle, wide,
};

// ---------------------------------------------------------------------------
// The grid: two columns of grouped rows, then two bands spanning both.
// ---------------------------------------------------------------------------

/// Left column origin and the shared column width. The right column starts
/// [`COLUMN_GAP`] past the left one and the bands span both.
const COL_L: i32 = 16;
const COL_W: i32 = 340;
const COLUMN_GAP: i32 = 16;
/// Right column origin.
const COL_R: i32 = COL_L + COL_W + COLUMN_GAP;
/// Band width: both columns plus the gap between them.
const BAND_W: i32 = COL_W * 2 + COLUMN_GAP;
/// Border inset inside a group box.
const INNER: i32 = 16;
/// The checkbox column of a source row in the Audio group.
const CHECK_W: i32 = 118;
/// Where that row's device picker starts, past its checkbox.
const PICK_X: i32 = INNER + 128;
/// And how wide it is: the group's inner width less that start.
const PICK_W: i32 = COL_W - INNER - 24 - 128;
/// The dB text beside a fader.
const VALUE_W: i32 = 48;
/// Caption column: a control starts this far past [`INNER`], so captions
/// share one edge and every control shares another.
const LABEL_W: i32 = 88;
/// Control column width inside a group; the right inset mirrors [`INNER`]
/// plus the group frame's own eight pixels.
const CONTROL_W: i32 = COL_W - INNER - LABEL_W - 24;
/// Wide controls (checkboxes, hints) inside a group.
const WIDE_W: i32 = COL_W - INNER - 24;
/// Band row geometry: a wider caption column and the control after it.
const BAND_LABEL: i32 = COL_L + INNER;
const BAND_LABEL_W: i32 = 112;
const BAND_RIGHT: i32 = BAND_LABEL + BAND_LABEL_W;
/// Browse buttons close the bands' inner edge, so the rows read as one
/// full-width form instead of trailing off after the edits.
const BAND_BUTTON_X: i32 = COL_L + BAND_W - INNER - BUTTON_W;
/// Band text-entry width: everything between the caption column and the
/// button, one control gap short of it, so a long path is readable.
const BAND_EDIT_W: i32 = BAND_BUTTON_X - BAND_RIGHT - 8;

/// Left column, top to bottom: the video profile and the hotkeys that drive
/// it.
const RECORDING_Y: i32 = 12;
const RECORDING_H: i32 = 112;
const HOTKEYS_Y: i32 = 136;
const HOTKEYS_H: i32 = 138;
/// Right column, top to bottom: the still's form and the audio sheet. The two
/// columns end within one group gap of each other, so neither side trails off
/// into a bare stretch of background.
const STILL_Y: i32 = 12;
const STILL_H: i32 = 80;
const AUDIO_Y: i32 = 104;
const AUDIO_H: i32 = 178;
/// The two full-width bands beneath the columns, then the footer.
const APP_Y: i32 = 294;
const APP_H: i32 = 60;
const FOLDERS_Y: i32 = 362;
const FOLDERS_H: i32 = 92;
pub(super) const BUTTON_Y: i32 = 480;
const BUTTON_W: i32 = 76;
pub(super) const BUTTON_H: i32 = 28;
/// The clear space left below the footer buttons; the client height is
/// fitted from it, so the sheet cannot grow past its own last row.
pub(super) const FOOTER_MARGIN: i32 = 16;
/// Save and Cancel, right-aligned with the bands' own right edge.
const CANCEL_X: i32 = COL_L + BAND_W - BUTTON_W;
const SAVE_X: i32 = CANCEL_X - BUTTON_W - 8;

/// On-screen position and size of a child control, in designed pixels.
#[derive(Clone, Copy)]
struct Place {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

/// Every child control, seeded from the current settings.
pub(super) fn populate(settings: &Settings, window: Hwnd) {
    recording_group(settings, window);
    hotkeys_group(settings, window);
    still_group(settings, window);
    audio_group(settings, window);
    application_group(settings, window);
    folders_group(settings, window);

    // The button row, right-aligned with the bands' own right edge.
    push_button(window, super::BTN_SAVE, "Save", SAVE_X, BUTTON_Y);
    push_button(window, super::BTN_CANCEL, "Cancel", CANCEL_X, BUTTON_Y);
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

/// `Recording`: profile, codec, and the cursor switch.
fn recording_group(settings: &Settings, window: Hwnd) {
    group(window, COL_L, RECORDING_Y, COL_W, RECORDING_H, "Recording");
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
        COL_L,
        RECORDING_Y + 16,
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
        COL_L,
        RECORDING_Y + 44,
        &codec_entries,
        codec_index,
    );
    checkbox(
        window,
        CHECK_CURSOR,
        "Include mouse cursor in captures",
        COL_L + INNER,
        RECORDING_Y + 76,
        settings.cursor,
        WIDE_W,
    );
}

/// `Still photo`: what a still is encoded as, then the PNG's depth.
///
/// Quality comes first because it is the choice most users change; the depth
/// row is labelled for the only choice that has one, and the dialog greys the
/// whole row — caption included — for every other quality.
fn still_group(settings: &Settings, window: Hwnd) {
    group(window, COL_R, STILL_Y, COL_W, STILL_H, "Still photo");
    let quality_index = StillQuality::CHOICES
        .iter()
        .position(|quality| *quality == settings.photo_quality)
        .unwrap_or(0);
    let quality_entries: Vec<String> = StillQuality::CHOICES
        .iter()
        .map(|quality| quality.label().to_owned())
        .collect();
    combo_row(
        window,
        COMBO_QUALITY,
        "Photo quality",
        COL_R,
        STILL_Y + 16,
        &quality_entries,
        quality_index,
    );
    let depth_index = match settings.photo_depth {
        StillDepth::Eight => 0,
        StillDepth::Sixteen => 1,
    };
    let depth_caption = combo_row(
        window,
        COMBO_DEPTH,
        "PNG depth",
        COL_R,
        STILL_Y + 44,
        &["8-bit".to_owned(), "16-bit".to_owned()],
        depth_index,
    );
    DIALOG.with_borrow_mut(|dialog| dialog.depth_caption = depth_caption);
}

/// `Audio`: which sources recordings carry, which device each one records
/// from, and at what quality their track is encoded.
///
/// The device picker sits on its source's own row, so "System audio" and
/// the playback device it means are one line to read — and a source that is
/// off keeps its device remembered for the next take.
fn audio_group(settings: &Settings, window: Hwnd) {
    group(window, COL_R, AUDIO_Y, COL_W, AUDIO_H, "Audio");
    checkbox(
        window,
        CHECK_SYSTEM_AUDIO,
        "System audio",
        COL_R + INNER,
        AUDIO_Y + 14,
        settings.audio.system,
        CHECK_W,
    );
    device_row(
        window,
        COMBO_SYSTEM_DEVICE,
        settings.audio_devices.system.as_ref(),
        crate::audio::devices::PLAYBACK,
        COL_R + PICK_X,
        AUDIO_Y + 10,
        PICK_W,
    );
    checkbox(
        window,
        CHECK_MIC,
        "Microphone",
        COL_R + INNER,
        AUDIO_Y + 38,
        settings.audio.mic,
        CHECK_W,
    );
    device_row(
        window,
        COMBO_MIC_DEVICE,
        settings.audio_devices.mic.as_ref(),
        crate::audio::devices::RECORDING,
        COL_R + PICK_X,
        AUDIO_Y + 34,
        PICK_W,
    );
    let labels: Vec<String> = crate::audio::Quality::CHOICES
        .iter()
        .map(|quality| quality.label().to_owned())
        .collect();
    let quality_index = crate::audio::Quality::CHOICES
        .iter()
        .position(|quality| *quality == settings.audio.quality)
        .unwrap_or(0);
    combo_row(
        window,
        COMBO_AUDIO_QUALITY,
        "Quality",
        COL_R,
        AUDIO_Y + 64,
        &labels,
        quality_index,
    );
    level_row(
        window,
        SLIDER_MIC_LEVEL,
        "Mic level",
        COL_R,
        AUDIO_Y + 92,
        settings.audio_levels.mic_db,
    );
    level_row(
        window,
        SLIDER_SYSTEM_LEVEL,
        "System level",
        COL_R,
        AUDIO_Y + 118,
        settings.audio_levels.system_db,
    );
    hint(
        window,
        "A chosen device is kept even while it is unplugged.",
        COL_R + INNER,
        AUDIO_Y + 146,
        WIDE_W,
    );
}

/// One source's fader: its caption, a trackbar from the quietest to the
/// loudest position, and the decibels it sits at. Windows keeps the device
/// volume; this is the app's own balance, so a quiet microphone and a loud
/// game can share a track without either device being touched.
fn level_row(window: Hwnd, id: i32, caption: &str, column: i32, y: i32, db: i32) {
    label(window, caption, column + INNER, y + 5, LABEL_W);
    let Some(handle) = create_child(
        window,
        "msctls_trackbar32",
        "",
        TBS_HORZ | WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        Place {
            x: column + INNER + LABEL_W,
            y,
            width: CONTROL_W - VALUE_W - 8,
            height: 24,
        },
        id,
        0,
    ) else {
        return;
    };
    let span = crate::audio::Levels::MAX_DB - crate::audio::Levels::MIN_DB;
    // A trackbar's range is the high end in the upper half of one long.
    let range = isize::try_from(span).unwrap_or(0) << 16;
    let position = isize::try_from(db - crate::audio::Levels::MIN_DB).unwrap_or(0);
    // SAFETY: live trackbar and scalar range/position values.
    unsafe {
        SendMessageW(handle, TBM_SETRANGE, 1, range);
        SendMessageW(handle, TBM_SETPOS, 1, position);
    }
    let Some(value) = create_child(
        window,
        "STATIC",
        &level_text(db),
        SS_LEFT | WS_CHILD | WS_VISIBLE,
        Place {
            x: column + INNER + LABEL_W + CONTROL_W - VALUE_W,
            y: y + 5,
            width: VALUE_W,
            height: 16,
        },
        0,
        0,
    ) else {
        return;
    };
    DIALOG.with_borrow_mut(|dialog| {
        dialog.sliders.push((id, handle));
        dialog.level_labels.push((id, value));
    });
}

/// A fader position as the text beside it.
pub(super) fn level_text(db: i32) -> String {
    if db > 0 {
        format!("+{db} dB")
    } else {
        format!("{db} dB")
    }
}

/// One source's device picker: the Windows default, every endpoint the
/// machine remembers, and — when the chosen one is away — a first entry for
/// it, so a selection made with the headset switched off survives.
fn device_row(
    window: Hwnd,
    id: i32,
    chosen: Option<&crate::settings::Endpoint>,
    flow: i32,
    x: i32,
    y: i32,
    width: i32,
) {
    let mut entries = vec!["System default".to_owned()];
    let mut choices: Vec<Option<crate::settings::Endpoint>> = vec![None];
    for device in crate::audio::devices::list(flow).unwrap_or_default() {
        entries.push(device.label());
        choices.push(Some(crate::settings::Endpoint {
            id: device.id,
            name: device.name,
        }));
    }
    let mut index = 0;
    if let Some(chosen) = chosen {
        if let Some(found) = choices
            .iter()
            .position(|choice| choice.as_ref().is_some_and(|choice| choice.id == chosen.id))
        {
            index = found;
        } else {
            entries.insert(1, format!("{} (not connected)", chosen.name));
            choices.insert(1, Some(chosen.clone()));
            index = 1;
        }
    }
    let Some(handle) = create_child(
        window,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST | WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        Place {
            x,
            y,
            width,
            height: 240,
        },
        id,
        0,
    ) else {
        return;
    };
    for entry in &entries {
        let text = wide(entry);
        // SAFETY: the combobox owns its list; `text` lives through the call.
        unsafe {
            SendMessageW(handle, CB_ADDSTRING, 0, text.as_ptr() as Lparam);
        }
    }
    // SAFETY: live combobox and scalar selection.
    unsafe {
        SendMessageW(handle, CB_SETCURSEL, index, 0);
    }
    DIALOG.with_borrow_mut(|dialog| {
        dialog.combos.push((id, handle));
        dialog.device_choices.push((id, choices));
    });
}

/// `Hotkeys`: the three free combos, seeded with their canonical forms,
/// directly under the recording group they drive.
fn hotkeys_group(settings: &Settings, window: Hwnd) {
    group(window, COL_L, HOTKEYS_Y, COL_W, HOTKEYS_H, "Hotkeys");
    hotkey_row(
        window,
        EDIT_RECORD,
        "Record hotkey",
        &settings.hotkey.to_combo(),
        HOTKEYS_Y + 18,
    );
    hotkey_row(
        window,
        EDIT_PHOTO,
        "Photo hotkey",
        &settings.photo.to_combo(),
        HOTKEYS_Y + 50,
    );
    hotkey_row(
        window,
        EDIT_CHIMES,
        "Chime hotkey",
        &settings.chime_hotkey.to_combo(),
        HOTKEYS_Y + 82,
    );
    let Some(status) = create_child(
        window,
        "STATIC",
        "Click a field, then press the keys",
        SS_LEFT | WS_CHILD | WS_VISIBLE,
        Place {
            x: COL_L + INNER,
            y: HOTKEYS_Y + 114,
            width: WIDE_W,
            height: 16,
        },
        0,
        0,
    ) else {
        return;
    };
    DIALOG.with_borrow_mut(|dialog| dialog.hotkey_status = Some(status));
}

/// `Application`: startup and the capture chime, side by side.
fn application_group(settings: &Settings, window: Hwnd) {
    group(window, COL_L, APP_Y, BAND_W, APP_H, "Application");
    checkbox(
        window,
        CHECK_AUTOSTART,
        "Start screencapture when Windows starts",
        BAND_LABEL,
        APP_Y + 22,
        settings.autostart,
        300,
    );
    checkbox(
        window,
        CHECK_CHIMES,
        "Play capture chimes",
        360,
        APP_Y + 22,
        settings.chimes,
        250,
    );
}

/// `Folders`: both destinations, each with a native Browse button.
fn folders_group(settings: &Settings, window: Hwnd) {
    group(window, COL_L, FOLDERS_Y, BAND_W, FOLDERS_H, "Folders");
    label(
        window,
        "Videos folder",
        BAND_LABEL,
        FOLDERS_Y + 24,
        BAND_LABEL_W,
    );
    edit_box(
        window,
        EDIT_VIDEOS,
        &settings.video_dir.to_string_lossy(),
        BAND_RIGHT,
        FOLDERS_Y + 20,
        BAND_EDIT_W,
    );
    push_button(
        window,
        super::BTN_BROWSE_VIDEOS,
        "Browse…",
        BAND_BUTTON_X,
        FOLDERS_Y + 18,
    );
    label(
        window,
        "Images folder",
        BAND_LABEL,
        FOLDERS_Y + 56,
        BAND_LABEL_W,
    );
    edit_box(
        window,
        EDIT_IMAGES,
        &settings.image_dir.to_string_lossy(),
        BAND_RIGHT,
        FOLDERS_Y + 52,
        BAND_EDIT_W,
    );
    push_button(
        window,
        super::BTN_BROWSE_IMAGES,
        "Browse…",
        BAND_BUTTON_X,
        FOLDERS_Y + 50,
    );
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

/// One caption row + combo row pair inside a column, on the shared edges.
/// The caption's handle comes back for rows that are greyed as a unit.
fn combo_row(
    window: Hwnd,
    id: i32,
    caption: &str,
    column: i32,
    y: i32,
    entries: &[String],
    chosen: usize,
) -> Option<Hwnd> {
    let caption = label(window, caption, column + INNER, y + 4, LABEL_W);
    let Some(handle) = create_child(
        window,
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST | WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        Place {
            x: column + INNER + LABEL_W,
            y,
            // A dropped list needs room; the closed control still renders
            // one line, but the drop-down extent is this height.
            width: CONTROL_W,
            height: 240,
        },
        id,
        0,
    ) else {
        return caption;
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
    caption
}

/// One hotkey caption + key-capture field pair, in the left column under
/// the recording group.
fn hotkey_row(window: Hwnd, id: i32, caption: &str, value: &str, y: i32) {
    label(window, caption, COL_L + INNER, y + 4, LABEL_W);
    edit_box(window, id, value, COL_L + INNER + LABEL_W, y, CONTROL_W);
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

/// One caption line at `x`/`y`; the handle comes back when the caller may
/// need to grey it with its control.
fn label(window: Hwnd, caption: &str, x: i32, y: i32, width: i32) -> Option<Hwnd> {
    create_child(
        window,
        "STATIC",
        caption,
        SS_LEFT | WS_CHILD | WS_VISIBLE,
        Place {
            x,
            y,
            width,
            height: 16,
        },
        0,
        0,
    )
}

/// One secondary note under a group's controls, painted in grey so it reads
/// as a hint rather than another caption.
fn hint(window: Hwnd, caption: &str, x: i32, y: i32, width: i32) {
    let Some(handle) = label(window, caption, x, y, width) else {
        return;
    };
    DIALOG.with_borrow_mut(|dialog| dialog.hints.push(handle));
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
            width: BUTTON_W,
            height: BUTTON_H,
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
    use super::{
        APP_H, APP_Y, AUDIO_H, AUDIO_Y, BAND_BUTTON_X, BAND_EDIT_W, BAND_RIGHT, BAND_W, BUTTON_H,
        BUTTON_W, BUTTON_Y, CANCEL_X, COL_L, COL_R, COL_W, CONTROL_W, FOLDERS_H, FOLDERS_Y,
        FOOTER_MARGIN, HOTKEYS_H, HOTKEYS_Y, INNER, LABEL_W, RECORDING_H, RECORDING_Y, SAVE_X,
        STILL_H, STILL_Y, WIDE_W, codec_list,
    };

    #[test]
    fn codec_list_matches_every_variant() {
        let codecs = codec_list();
        assert_eq!(codecs.len(), 10);
        assert_eq!(codecs[0].id(), "x264");
        assert!(codecs.iter().any(|codec| codec.id() == "nvenc-hevc"));
    }

    /// The geometry invariants the design leans on: the columns sit inside
    /// the dialog with equal margins, each stacks its groups without gaps or
    /// overlaps, both end above the first band, and the footer closes the
    /// sheet inside the client edge.
    #[test]
    fn the_grid_stays_inside_its_window() {
        use super::super::frame::{CLIENT_H, CLIENT_W};

        const {
            // The right column mirrors the left one's margin.
            assert!(COL_R + COL_W == CLIENT_W - COL_L);
            // The bands span both columns plus the gap between them.
            assert!(BAND_W == COL_W * 2 + (COL_R - COL_W - COL_L));
            // A control inside a column ends before its group's right frame.
            assert!(COL_L + INNER + LABEL_W + CONTROL_W < COL_L + COL_W - 8);
            // A wide checkbox stays inside its group too.
            assert!(COL_L + INNER + WIDE_W < COL_L + COL_W - 8);
            // Each column stacks its groups with one gap between them, and
            // the two columns end close enough together that neither leaves
            // a bare stretch above the bands.
            assert!(RECORDING_Y + RECORDING_H + 12 == HOTKEYS_Y);
            assert!(STILL_Y + STILL_H + 12 == AUDIO_Y);
            assert!(HOTKEYS_Y + HOTKEYS_H < APP_Y);
            assert!(AUDIO_Y + AUDIO_H < APP_Y);
            assert!((HOTKEYS_Y + HOTKEYS_H) - (AUDIO_Y + AUDIO_H) <= 12);
            // The bands follow each other, and the folders end above the
            // footer.
            assert!(APP_Y + APP_H < FOLDERS_Y);
            assert!(FOLDERS_Y + FOLDERS_H < BUTTON_Y);
            // Save and Cancel are right-aligned with the band edge and leave
            // the same margin at the bottom as the columns do at the top.
            assert!(SAVE_X + BUTTON_W + 8 == CANCEL_X);
            assert!(CANCEL_X + BUTTON_W == COL_L + BAND_W);
            assert!(BUTTON_Y + BUTTON_H + FOOTER_MARGIN == CLIENT_H);
            // The band's Browse button closes the band's inner edge.
            assert!(BAND_BUTTON_X + BUTTON_W == COL_L + BAND_W - INNER);
            // The edits leave room for the button beside them.
            assert!(BAND_RIGHT + BAND_EDIT_W + 8 == BAND_BUTTON_X);
        }
    }
}
