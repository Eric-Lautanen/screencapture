//! The settings dialog's control rows: a two-column sheet with Capture,
//! Recording, and Still photo on the left and Audio and Hotkeys on the
//! right, full-width Application and Folders bands beneath them, the
//! control builders, and the getters Save reads back through.
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
/// Band text-entry width, leaving the Browse button its own column.
const BAND_EDIT_W: i32 = 300;
/// Browse buttons sit one control gap past the edits.
const BAND_BUTTON_X: i32 = BAND_RIGHT + BAND_EDIT_W + 8;

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
    still_group(settings, window);
    audio_group(settings, window);
    hotkeys_group(settings, window);
    application_group(settings, window);
    folders_group(settings, window);

    // The button row, right-aligned with the bands' own right edge.
    push_button(window, super::BTN_SAVE, "Save", 552, 560);
    push_button(window, super::BTN_CANCEL, "Cancel", 636, 560);
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
    group(window, COL_L, 12, COL_W, 112, "Recording");
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
        28,
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
        56,
        &codec_entries,
        codec_index,
    );
    checkbox(
        window,
        CHECK_CURSOR,
        "Include mouse cursor in captures",
        COL_L + INNER,
        88,
        settings.cursor,
        WIDE_W,
    );
}

/// `Still photo`: what a still is encoded as, then the PNG's depth.
///
/// Quality comes first because it is the choice most users change; the depth
/// row is labelled for the only choice that has one, and the dialog greys it
/// out for every other quality.
fn still_group(settings: &Settings, window: Hwnd) {
    group(window, COL_L, 132, COL_W, 80, "Still photo");
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
        COL_L,
        148,
        &quality_entries,
        quality_index,
    );
    let depth_index = match settings.photo_depth {
        StillDepth::Eight => 0,
        StillDepth::Sixteen => 1,
    };
    combo_row(
        window,
        COMBO_DEPTH,
        "PNG depth",
        COL_L,
        176,
        &["8-bit".to_owned(), "16-bit".to_owned()],
        depth_index,
    );
}

/// `Audio`: which sources recordings carry, which device each one records
/// from, and at what quality their track is encoded.
///
/// The device picker sits on its source's own row, so "System audio" and
/// the playback device it means are one line to read — and a source that is
/// off keeps its device remembered for the next take.
fn audio_group(settings: &Settings, window: Hwnd) {
    group(window, COL_R, 12, COL_W, 178, "Audio");
    checkbox(
        window,
        CHECK_SYSTEM_AUDIO,
        "System audio",
        COL_R + INNER,
        26,
        settings.audio.system,
        CHECK_W,
    );
    device_row(
        window,
        COMBO_SYSTEM_DEVICE,
        settings.audio_devices.system.as_ref(),
        crate::audio::devices::PLAYBACK,
        COL_R + PICK_X,
        22,
        PICK_W,
    );
    checkbox(
        window,
        CHECK_MIC,
        "Microphone",
        COL_R + INNER,
        50,
        settings.audio.mic,
        CHECK_W,
    );
    device_row(
        window,
        COMBO_MIC_DEVICE,
        settings.audio_devices.mic.as_ref(),
        crate::audio::devices::RECORDING,
        COL_R + PICK_X,
        46,
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
        76,
        &labels,
        quality_index,
    );
    level_row(
        window,
        SLIDER_MIC_LEVEL,
        "Mic level",
        COL_R,
        104,
        settings.audio_levels.mic_db,
    );
    level_row(
        window,
        SLIDER_SYSTEM_LEVEL,
        "System level",
        COL_R,
        130,
        settings.audio_levels.system_db,
    );
    label(
        window,
        "A chosen device is kept even while it is unplugged.",
        COL_R + INNER,
        158,
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
/// under Still photo so the right column holds the audio sheet alone.
fn hotkeys_group(settings: &Settings, window: Hwnd) {
    group(window, COL_L, 224, COL_W, 138, "Hotkeys");
    hotkey_row(
        window,
        EDIT_RECORD,
        "Record hotkey",
        &settings.hotkey.to_combo(),
        242,
    );
    hotkey_row(
        window,
        EDIT_PHOTO,
        "Photo hotkey",
        &settings.photo.to_combo(),
        274,
    );
    hotkey_row(
        window,
        EDIT_CHIMES,
        "Chime hotkey",
        &settings.chime_hotkey.to_combo(),
        306,
    );
    let Some(status) = create_child(
        window,
        "STATIC",
        "Click a field, then press the keys",
        SS_LEFT | WS_CHILD | WS_VISIBLE,
        Place {
            x: COL_L + INNER,
            y: 338,
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
    group(window, COL_L, 374, BAND_W, 60, "Application");
    checkbox(
        window,
        CHECK_AUTOSTART,
        "Start screencapture when Windows starts",
        BAND_LABEL,
        396,
        settings.autostart,
        300,
    );
    checkbox(
        window,
        CHECK_CHIMES,
        "Play capture chimes",
        360,
        396,
        settings.chimes,
        250,
    );
}

/// `Folders`: both destinations, each with a native Browse button.
fn folders_group(settings: &Settings, window: Hwnd) {
    group(window, COL_L, 442, BAND_W, 92, "Folders");
    label(window, "Videos folder", BAND_LABEL, 466, BAND_LABEL_W);
    edit_box(
        window,
        EDIT_VIDEOS,
        &settings.video_dir.to_string_lossy(),
        BAND_RIGHT,
        462,
        BAND_EDIT_W,
    );
    push_button(
        window,
        super::BTN_BROWSE_VIDEOS,
        "Browse…",
        BAND_BUTTON_X,
        460,
    );
    label(window, "Images folder", BAND_LABEL, 498, BAND_LABEL_W);
    edit_box(
        window,
        EDIT_IMAGES,
        &settings.image_dir.to_string_lossy(),
        BAND_RIGHT,
        494,
        BAND_EDIT_W,
    );
    push_button(
        window,
        super::BTN_BROWSE_IMAGES,
        "Browse…",
        BAND_BUTTON_X,
        492,
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
fn combo_row(
    window: Hwnd,
    id: i32,
    caption: &str,
    column: i32,
    y: i32,
    entries: &[String],
    chosen: usize,
) {
    label(window, caption, column + INNER, y + 4, LABEL_W);
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

/// One hotkey caption + key-capture field pair, in the left column under
/// the still-photo group.
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

/// One caption or hint line at `x`/`y`.
fn label(window: Hwnd, caption: &str, x: i32, y: i32, width: i32) {
    let _ = create_child(
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
    use super::{
        BAND_BUTTON_X, BAND_EDIT_W, BAND_RIGHT, BAND_W, COL_L, COL_R, COL_W, CONTROL_W, LABEL_W,
        codec_list,
    };

    #[test]
    fn codec_list_matches_every_variant() {
        let codecs = codec_list();
        assert_eq!(codecs.len(), 10);
        assert_eq!(codecs[0].id(), "x264");
        assert!(codecs.iter().any(|codec| codec.id() == "nvenc-hevc"));
    }

    /// The geometry invariants the design leans on: the columns sit inside
    /// the dialog with equal margins, the controls stay inside their group,
    /// and the bands span both columns.
    #[test]
    fn the_grid_stays_inside_its_window() {
        use super::super::frame::{CLIENT_H, CLIENT_W};
        use super::{INNER, WIDE_W};

        const {
            // The right column mirrors the left one's margin.
            assert!(COL_R + COL_W == CLIENT_W - COL_L);
            // The bands span both columns plus the gap between them.
            assert!(BAND_W == COL_W * 2 + (COL_R - COL_W - COL_L));
            // A control inside a column ends before its group's right frame.
            assert!(COL_L + INNER + LABEL_W + CONTROL_W < COL_L + COL_W - 8);
            // A wide checkbox stays inside its group too.
            assert!(COL_L + INNER + WIDE_W < COL_L + COL_W - 8);
            // The band's Browse button stays inside the band.
            assert!(BAND_BUTTON_X + 76 < COL_L + BAND_W);
            // The edits leave room for the button beside them.
            assert!(BAND_RIGHT + BAND_EDIT_W + 8 == BAND_BUTTON_X);
            // The button row still fits above the client edge.
            assert!(470 + 28 < CLIENT_H);
        }
    }
}
