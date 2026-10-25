//! The tray Settings window: Win32 controls over the typed settings view,
//! so every capture option lives in one place — recording profile and
//! codec, cursor, the audio devices, sources and levels, the still's
//! quality and its PNG depth (greyed when the quality does not use one),
//! autostart, capture chimes, the record/photo/chime hotkeys (free
//! modifier+key text, validated before accept), and the destination
//! folders, which each get their own field plus a native folder browse and
//! are created and write-tested on Save, so a folder that cannot hold a take
//! is reported while the field is still open.
//! A modal loop runs on the calling thread; Save answers
//! [`Answers`] (validated), Cancel or close answers `None` and leaves
//! all state untouched. Control rows live in [`rows`], raw Win32 in
//! [`ffi`].

use std::cell::RefCell;
use std::io;
use std::path::Path;

use crate::settings::{AudioDevices, Endpoint, Hotkey, Settings, StillQuality};
use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, Hbrush,
    Hinstance, Hwnd, Msg, Point, PostQuitMessage, RegisterClassExW, SW_SHOW, SetForegroundWindow,
    ShowWindow, TranslateMessage, VK_ESCAPE, WM_KEYDOWN, WndClassExW, Wparam,
};

pub(super) use crate::winapi::{kept_wide, module_handle, wide, wide_to_string};

use std::ffi::c_void;

mod ffi;
mod fields;
mod frame;
mod keycapture;
mod rows;

use self::ffi::{
    BN_CLICKED, CBN_SELCHANGE, EnableWindow, IsDialogMessageW, MB_ICONERROR_OK, MessageBoxW,
    SendMessageW, SetBkMode, SetTextColor, SetWindowTextW, TBM_GETPOS, TRANSPARENT, WS_CAPTION,
    WS_CLIPCHILDREN, WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_SYSMENU, WS_VISIBLE,
};
use self::fields::{
    browse_folder, check_state, combo_choice, combo_handle, edit_text, ensure_folder,
};
use self::frame::{
    dpi_scale, drop_font, drop_icon, fit_scale, install_title_icon, ui_font, window_frame,
};
use self::rows::{codec_list, populate};

// ---------------------------------------------------------------------------
// Control ids routed through WM_COMMAND.
// ---------------------------------------------------------------------------

const COMBO_PROFILE: i32 = 103;
const COMBO_CODEC: i32 = 104;
const COMBO_DEPTH: i32 = 105;
const CHECK_CURSOR: i32 = 106;
const CHECK_AUTOSTART: i32 = 107;
const EDIT_RECORD: i32 = 108;
const EDIT_PHOTO: i32 = 109;
const EDIT_CHIMES: i32 = 115;
const EDIT_VIDEOS: i32 = 110;
const EDIT_IMAGES: i32 = 111;
const BTN_BROWSE_VIDEOS: i32 = 112;
const BTN_BROWSE_IMAGES: i32 = 113;
const CHECK_CHIMES: i32 = 114;
const CHECK_SYSTEM_AUDIO: i32 = 116;
const CHECK_MIC: i32 = 117;
const COMBO_AUDIO_QUALITY: i32 = 118;
const COMBO_MIC_DEVICE: i32 = 119;
const COMBO_SYSTEM_DEVICE: i32 = 120;
const SLIDER_MIC_LEVEL: i32 = 121;
const SLIDER_SYSTEM_LEVEL: i32 = 122;
const COMBO_QUALITY: i32 = 123;
/// Standard dialog ids: `IsDialogMessageW` sends `IDOK` for Enter and
/// `IDCANCEL` for Escape, so the buttons must carry those identities.
const BTN_SAVE: i32 = 1;
const BTN_CANCEL: i32 = 2;

/// The window class this module registers.
const CLASS_NAME: &str = "screencapture.settings.window";
/// The dialog caption.
const TITLE: &str = "Settings — screencapture";

/// The values a Save submits, keyed to the settings schema.
pub struct Answers {
    /// New recording profile id.
    pub profile: String,
    /// New codec id.
    pub codec: String,
    /// Whether recordings redraw the mouse pointer.
    pub cursor: bool,
    /// Which audio sources recordings carry.
    pub audio: crate::audio::Wanted,
    /// Which device each source records from.
    pub devices: AudioDevices,
    /// Each source's fader, in dB.
    pub levels: crate::audio::Levels,
    /// Whether the app launches at login.
    pub autostart: bool,
    /// New photo depth (`8` or `16`; the PNG's pixel depth, ignored by
    /// every other still quality).
    pub depth: String,
    /// New still quality (`lossless`, `high`, `standard`, or `compact`).
    pub quality: String,
    /// Whether captures play the confirmation chime.
    pub chimes: bool,
    /// Videos folder (absolute path).
    pub videos: String,
    /// Images folder (absolute path).
    pub images: String,
    /// Record hotkey combo in canonical form.
    pub record: String,
    /// Photo hotkey combo in canonical form.
    pub photo: String,
    /// Chime-mute hotkey combo in canonical form.
    pub chime_hotkey: String,
}

/// Alive controls while the dialog runs; the answer of a completed save.
#[derive(Default)]
struct Dialog {
    file: Option<Hwnd>,
    combos: Vec<(i32, Hwnd)>,
    checks: Vec<(i32, Hwnd)>,
    edits: Vec<(i32, Hwnd)>,
    /// Every child in creation order. Windows inserts new children at
    /// the *bottom* of the sibling z-order here, so the final pass in
    /// `rows` re-stacks them top-ward in this order.
    children: Vec<Hwnd>,
    /// Every device picker's choices, by control id and combo index, so a
    /// save answers an endpoint instead of the label it showed.
    device_choices: Vec<(i32, Vec<Option<Endpoint>>)>,
    /// The two level faders and the labels that report their position.
    sliders: Vec<(i32, Hwnd)>,
    level_labels: Vec<(i32, Hwnd)>,
    /// The one-line hotkey status label under the two combo fields.
    hotkey_status: Option<Hwnd>,
    /// Whether that label currently reports a conflict (painted red).
    hotkey_warning: bool,
    /// The dialog-wide `Segoe UI` font, created in `run` and deleted with
    /// it (children get it via `WM_SETFONT` in `rows`).
    font: Option<*mut c_void>,
    /// The title-bar icon, loaded for this open and released with it: a
    /// window borrows a `WM_SETICON` handle rather than owning it, so a
    /// dialog reopened often would leak one icon per open otherwise.
    icon: Option<crate::winapi::Hicon>,
    /// Display scale percentage used by every designed coordinate
    /// (`100` at 96 DPI).
    scale: i32,
    answer: Option<Answers>,
}

thread_local! {
    static DIALOG: RefCell<Dialog> = RefCell::new(Dialog::default());
}

/// Open the dialog against the current settings; Save answers the new
/// values, Cancel or close answers `None` (the caller leaves state alone).
///
/// # Errors
/// Windows refused window creation.
pub fn run(settings: &Settings) -> io::Result<Option<Answers>> {
    keycapture::reset();
    DIALOG.with_borrow_mut(|dialog| {
        *dialog = Dialog::default();
    });
    let instance = module_handle();
    // The display's DPI scale, reduced when the window would not fit the
    // work area: a small screen shrinks the whole layout instead of
    // hiding Save and Cancel below the desktop.
    let scale = fit_scale(dpi_scale());
    DIALOG.with_borrow_mut(|dialog| dialog.scale = scale);
    register_class(instance);
    let window = create_window(instance, scale);
    if window.is_null() {
        return Err(io::Error::other("the settings window could not be created"));
    }
    DIALOG.with_borrow_mut(|dialog| dialog.file = Some(window));
    DIALOG.with_borrow_mut(|dialog| dialog.font = ui_font(scale));
    DIALOG.with_borrow_mut(|dialog| dialog.icon = install_title_icon(window, instance));
    populate(settings, window);
    // The depth row starts greyed when the saved quality is not the PNG.
    sync_depth_state();
    // SAFETY: `window` is the dialog created above.
    unsafe {
        ShowWindow(window, SW_SHOW);
        SetForegroundWindow(window);
    }
    pump(window);
    let answer = DIALOG.with_borrow_mut(|dialog| {
        // SAFETY: `file` is the dialog created in this call, still alive.
        if let Some(window) = dialog.file.take() {
            unsafe {
                DestroyWindow(window);
            }
        }
        if let Some(icon) = dialog.icon.take() {
            drop_icon(icon);
        }
        if let Some(font) = dialog.font.take() {
            drop_font(font);
        }
        dialog.answer.take()
    });
    Ok(answer)
}

/// The modal message loop; ends when Save, Cancel, or close posts quit.
/// `IsDialogMessageW` supplies the dialog keyboard contract the control
/// styles promise: Tab focus, arrows in combos, Enter on the default button.
fn pump(window: Hwnd) {
    let mut message = Msg {
        hwnd: std::ptr::null_mut(),
        message: 0,
        w_param: 0,
        l_param: 0,
        time: 0,
        point: Point::default(),
    };
    loop {
        // SAFETY: `message` is a local struct owned by this loop.
        let received = unsafe { GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) };
        if received <= 0 {
            return;
        }
        // SAFETY: both handles are live and owned by this modal loop.
        if unsafe { IsDialogMessageW(window, &raw mut message) } != 0 {
            continue;
        }
        let _ = unsafe { TranslateMessage(&raw const message) };
        unsafe { DispatchMessageW(&raw const message) };
    }
}

/// Register the window class; Windows answers repeats harmlessly.
///
/// The name buffer is kept for the process lifetime: `RegisterClassExW`
/// stores the pointer it is handed, so a local buffer would leave the class
/// table pointing at freed memory.
fn register_class(instance: Hinstance) {
    let name = kept_wide(CLASS_NAME);
    let class = WndClassExW {
        size: u32::try_from(std::mem::size_of::<WndClassExW>()).expect("class size fits u32"),
        style: 0,
        wnd_proc: procedure,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: std::ptr::null_mut(),
        cursor: std::ptr::null_mut(),
        // `COLOR_BTNFACE` (`15`) with the brush double offset: the standard
        // dialog surface, so group boxes and labels blend instead of sitting
        // on the desktop's background colour.
        background: 16usize as Hbrush,
        menu_name: std::ptr::null(),
        class_name: name,
        icon_small: std::ptr::null_mut(),
    };
    // SAFETY: the class struct is a fully initialized local and the name
    // buffer outlives the registration.
    unsafe {
        RegisterClassExW(&raw const class);
    }
}

/// Create the dialog-sized, fixed, centered settings window.
fn create_window(instance: Hinstance, scale: i32) -> Hwnd {
    let class = wide(CLASS_NAME);
    let title = wide(TITLE);
    // SAFETY: strings outlive the call; the class is registered above.
    let (width, height, x, y) = window_frame(scale);
    unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            class.as_ptr(),
            title.as_ptr(),
            WS_CAPTION | WS_SYSMENU | WS_VISIBLE | WS_CLIPCHILDREN,
            x,
            y,
            width,
            height,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        )
    }
}

/// The dialog's window procedure.
unsafe extern "system" fn procedure(
    window: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: isize,
) -> isize {
    match message {
        ffi::WM_CTLCOLORSTATIC => {
            let status = DIALOG.with_borrow(|dialog| dialog.hotkey_status);
            if status == Some(lparam as Hwnd) {
                let warning = DIALOG.with_borrow(|dialog| dialog.hotkey_warning);
                // SAFETY: `wparam` is the control DC; colours are scalars.
                unsafe {
                    SetTextColor(
                        wparam as crate::winapi::Hdc,
                        if warning { 0x0000_00C0 } else { 0x0000_0000 },
                    );
                    SetBkMode(wparam as crate::winapi::Hdc, TRANSPARENT);
                }
                return 16isize; // COLOR_BTNFACE brush: no white box
            }
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
        ffi::WM_COMMAND => {
            let id = i32::try_from(wparam & 0xffff).unwrap_or(0);
            let code = u16::try_from((wparam >> 16) & 0xffff).unwrap_or(0);
            if code == BN_CLICKED {
                on_click(window, id);
            } else if code == CBN_SELCHANGE && id == COMBO_QUALITY {
                sync_depth_state();
            }
            0
        }
        ffi::WM_HSCROLL => {
            refresh_levels();
            0
        }
        ffi::WM_CLOSE => {
            quit_without_answer();
            0
        }
        ffi::WM_DESTROY => {
            // A field can be destroyed without a blur message; the hook
            // must not outlive the control it serves. Do *not* post a
            // quit here: `run` destroys the window after the pump has
            // already consumed the one quit Save/Cancel/Escape posted,
            // and a second WM_QUIT would escape into the tray pump and
            // terminate the whole application.
            keycapture::reset();
            0
        }
        WM_KEYDOWN if wparam == VK_ESCAPE => {
            // Escape cancels exactly like the Cancel button.
            quit_without_answer();
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Button feedback: browse folders, save, or cancel.
fn on_click(window: Hwnd, id: i32) {
    match id {
        BTN_BROWSE_VIDEOS => browse_folder(window, EDIT_VIDEOS, "Where recordings are saved"),
        BTN_BROWSE_IMAGES => browse_folder(window, EDIT_IMAGES, "Where photos are saved"),
        BTN_SAVE => save(window),
        BTN_CANCEL => quit_without_answer(),
        _ => {}
    }
}

/// Finish with no answer (Cancel; window close records None itself).
fn quit_without_answer() {
    // SAFETY: the pump loop owns the quit sentinel.
    unsafe {
        PostQuitMessage(0);
    }
}

/// Read the Save fields, validate, and either complain or finish.
fn save(window: Hwnd) {
    let profile = crate::recording::Profile::PROFILES[combo_choice(COMBO_PROFILE)]
        .id
        .to_owned();
    let codec = codec_list()[combo_choice(COMBO_CODEC)].id().to_owned();
    let depths = ["8", "16"];
    let depth = depths[combo_choice(COMBO_DEPTH)].to_owned();
    let quality = crate::settings::StillQuality::CHOICES[combo_choice(COMBO_QUALITY)].to_string();
    let record = edit_text(EDIT_RECORD);
    let photo = edit_text(EDIT_PHOTO);
    let chime = edit_text(EDIT_CHIMES);
    let videos = edit_text(EDIT_VIDEOS);
    let images = edit_text(EDIT_IMAGES);
    let cursor = check_state(CHECK_CURSOR);
    let audio = crate::audio::Wanted {
        mic: check_state(CHECK_MIC),
        system: check_state(CHECK_SYSTEM_AUDIO),
        quality: crate::audio::Quality::CHOICES[combo_choice(COMBO_AUDIO_QUALITY)],
    };
    let devices = AudioDevices {
        mic: device_choice(COMBO_MIC_DEVICE),
        system: device_choice(COMBO_SYSTEM_DEVICE),
    };
    let levels = crate::audio::Levels {
        mic_db: level_of(SLIDER_MIC_LEVEL),
        system_db: level_of(SLIDER_SYSTEM_LEVEL),
    };
    let autostart = check_state(CHECK_AUTOSTART);
    let chimes = check_state(CHECK_CHIMES);

    let Some((parsed_record, parsed_photo, parsed_chime)) =
        accepted_hotkeys(window, &record, &photo, &chime)
    else {
        return;
    };
    for (name, value) in [("videos", videos.as_str()), ("images", images.as_str())] {
        let folder = value.trim();
        if folder.is_empty() || !Path::new(folder).is_absolute() {
            complain(
                window,
                &format!("The {name} folder needs an absolute path (e.g. C:\\Users\\me\\Videos)."),
            );
            return;
        }
        // The folder is created here, while the field is still open: a
        // capture that cannot write would otherwise fail only after the
        // take the user just recorded.
        if let Err(reason) = ensure_folder(folder) {
            complain(window, &format!("The {name} folder {reason}."));
            return;
        }
    }
    DIALOG.with_borrow_mut(|dialog| {
        dialog.answer = Some(Answers {
            profile,
            codec,
            cursor,
            audio,
            devices,
            levels,
            autostart,
            depth,
            quality,
            chimes,
            videos,
            images,
            record: parsed_record.to_combo(),
            photo: parsed_photo.to_combo(),
            chime_hotkey: parsed_chime.to_combo(),
        });
    });
    // SAFETY: the pump loop below owns the quit sentinel.
    unsafe {
        PostQuitMessage(0);
    }
}

/// Whether the still's depth means anything for `quality`: only the PNG has
/// a pixel depth to choose. WebP, BMP, and the JPEG steps are always 8-bit,
/// so their depth row is disabled rather than left looking active.
fn depth_applies(quality: StillQuality) -> bool {
    quality == StillQuality::Png
}

/// Enable or grey the depth row for the quality now selected, and keep it
/// in step when the user changes quality without saving.
fn sync_depth_state() {
    let quality = StillQuality::CHOICES
        .get(combo_choice(COMBO_QUALITY))
        .copied()
        .unwrap_or_default();
    let Some(handle) = combo_handle(COMBO_DEPTH) else {
        return;
    };
    // SAFETY: a live child control of this dialog; enabling only changes
    // its input state.
    unsafe {
        EnableWindow(handle, i32::from(depth_applies(quality)));
    }
}

/// The three hotkey fields, parsed and vetted: each a real combo, all
/// different, and each free for this process to register. Anything else
/// complains at the window and answers `None`.
fn accepted_hotkeys(
    window: Hwnd,
    record: &str,
    photo: &str,
    chime: &str,
) -> Option<(Hotkey, Hotkey, Hotkey)> {
    let Some(parsed_record) = Hotkey::parse(record) else {
        complain(
            window,
            "The record hotkey is not a valid combo (e.g. ctrl+shift+x).",
        );
        return None;
    };
    let Some(parsed_photo) = Hotkey::parse(photo) else {
        complain(
            window,
            "The photo hotkey is not a valid combo (e.g. ctrl+shift+z).",
        );
        return None;
    };
    let Some(parsed_chime) = Hotkey::parse(chime) else {
        complain(
            window,
            "The chime hotkey is not a valid combo (e.g. ctrl+shift+m).",
        );
        return None;
    };
    if parsed_record == parsed_photo
        || parsed_record == parsed_chime
        || parsed_photo == parsed_chime
    {
        complain(
            window,
            "The record, photo, and chime hotkeys must all differ.",
        );
        return None;
    }
    // A combo this process already holds answers free without a probe,
    // and the probe never lifts the live bindings; another process holding
    // one is a real conflict that would make `RegisterHotKey` fail after a
    // save.
    for (name, combo) in [
        ("record", parsed_record),
        ("photo", parsed_photo),
        ("chime", parsed_chime),
    ] {
        if !crate::tray::hotkey_available(combo) {
            complain(
                window,
                &format!(
                    "The {name} hotkey is already used by another application. Windows does not report which app owns a global hotkey; close that app or pick a different combo."
                ),
            );
            return None;
        }
    }
    Some((parsed_record, parsed_photo, parsed_chime))
}

/// The fader position a slider's thumb sits at, in dB.
fn level_of(id: i32) -> i32 {
    let position = DIALOG.with_borrow(|dialog| {
        dialog
            .sliders
            .iter()
            .find(|(slider, _)| *slider == id)
            .map(|(_, handle)| {
                // SAFETY: live trackbar this dialog owns; the message reads
                // a scalar.
                let position = unsafe { SendMessageW(*handle, TBM_GETPOS, 0, 0) };
                i32::try_from(position).unwrap_or(0)
            })
    });
    crate::audio::Levels::clamp_level(position.unwrap_or(0) + crate::audio::Levels::MIN_DB)
}

/// Repaint the dB text beside both faders after one moved.
fn refresh_levels() {
    DIALOG.with_borrow(|dialog| {
        for (slider, label) in &dialog.level_labels {
            let db = level_of(*slider);
            let text = wide(&rows::level_text(db));
            // SAFETY: live label created by this dialog; the text outlives
            // the call.
            unsafe {
                SetWindowTextW(*label, text.as_ptr());
            }
        }
    });
}

/// The endpoint a device picker has selected; `None` is "System default"
/// and follows whatever Windows uses.
fn device_choice(id: i32) -> Option<Endpoint> {
    let index = combo_choice(id);
    DIALOG.with_borrow(|dialog| {
        dialog
            .device_choices
            .iter()
            .find(|(combo, _)| *combo == id)
            .and_then(|(_, choices)| choices.get(index).cloned().flatten())
    })
}

/// Show a validation complaint with the window as its parent.
fn complain(window: Hwnd, text: &str) {
    let text = wide(text);
    let caption = wide(TITLE);
    // SAFETY: strings outlive the modal box call.
    unsafe {
        MessageBoxW(window, text.as_ptr(), caption.as_ptr(), MB_ICONERROR_OK);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        BTN_BROWSE_IMAGES, BTN_BROWSE_VIDEOS, BTN_CANCEL, BTN_SAVE, CHECK_AUTOSTART, CHECK_CHIMES,
        CHECK_CURSOR, CHECK_MIC, CHECK_SYSTEM_AUDIO, CLASS_NAME, COMBO_AUDIO_QUALITY, COMBO_CODEC,
        COMBO_DEPTH, COMBO_MIC_DEVICE, COMBO_PROFILE, COMBO_QUALITY, COMBO_SYSTEM_DEVICE,
        EDIT_CHIMES, EDIT_IMAGES, EDIT_PHOTO, EDIT_RECORD, EDIT_VIDEOS, SLIDER_MIC_LEVEL,
        SLIDER_SYSTEM_LEVEL, TITLE, depth_applies,
    };
    use crate::settings::StillQuality;

    /// Every control needs its own id: `GetDlgItem` answers the first window
    /// that claims one, so a duplicate would quietly read or write a
    /// neighbour's state instead of the control the caller named.
    #[test]
    fn every_control_has_its_own_id() {
        let controls = [
            ("save", BTN_SAVE),
            ("cancel", BTN_CANCEL),
            ("profile", COMBO_PROFILE),
            ("codec", COMBO_CODEC),
            ("quality", COMBO_QUALITY),
            ("depth", COMBO_DEPTH),
            ("cursor", CHECK_CURSOR),
            ("autostart", CHECK_AUTOSTART),
            ("record hotkey", EDIT_RECORD),
            ("photo hotkey", EDIT_PHOTO),
            ("chime hotkey", EDIT_CHIMES),
            ("videos folder", EDIT_VIDEOS),
            ("images folder", EDIT_IMAGES),
            ("browse videos", BTN_BROWSE_VIDEOS),
            ("browse images", BTN_BROWSE_IMAGES),
            ("chimes", CHECK_CHIMES),
            ("system audio", CHECK_SYSTEM_AUDIO),
            ("microphone", CHECK_MIC),
            ("audio quality", COMBO_AUDIO_QUALITY),
            ("mic device", COMBO_MIC_DEVICE),
            ("system device", COMBO_SYSTEM_DEVICE),
            ("mic level", SLIDER_MIC_LEVEL),
            ("system level", SLIDER_SYSTEM_LEVEL),
        ];
        let mut seen = BTreeSet::new();
        for (name, id) in controls {
            assert!(seen.insert(id), "{name} reuses control id {id}");
            // Ids 1 and 2 belong to Save and Cancel; any other control on
            // one of them would take the keyboard route those two need.
            assert!(
                id > 2 || name == "save" || name == "cancel",
                "{name} would shadow a dialog-standard id"
            );
        }
    }

    /// Enter and Escape reach Save and Cancel through `IsDialogMessage`,
    /// which routes the dialog-standard ids and nothing else: renumbering
    /// either button silently takes the keyboard away from it.
    #[test]
    fn enter_and_escape_keep_their_dialog_ids() {
        assert_eq!(BTN_SAVE, 1, "IDOK");
        assert_eq!(BTN_CANCEL, 2, "IDCANCEL");
    }

    /// Only the PNG has a depth to choose: WebP, BMP, and the JPEG steps are
    /// always 8-bit, so the row is greyed for them. Walking the real choices
    /// keeps this true if a quality is added or reordered.
    #[test]
    fn only_the_png_quality_has_a_depth_to_choose() {
        for quality in StillQuality::CHOICES {
            assert_eq!(
                depth_applies(quality),
                quality == StillQuality::Png,
                "{quality:?}"
            );
        }
    }

    /// The class name is how the tray, the capture probe, and the asset
    /// scripts find this window.
    #[test]
    fn the_window_is_findable_by_its_class() {
        assert!(CLASS_NAME.starts_with("screencapture."));
        assert!(TITLE.contains("screencapture"));
    }
}
