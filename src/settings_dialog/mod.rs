//! The tray Settings window: Win32 controls over the typed settings view,
//! so every schema-visible choice is editable in one place — capture mode
//! and display, recording profile and codec, photo depth, cursor,
//! autostart, and capture chimes, the record/photo/chime hotkey combos
//! (free modifier+key text, validated before accept), and the destination
//! folders, where videos
//! and images each get their own field plus a native folder browse.
//! A modal loop runs on the calling thread; Save answers
//! [`Answers`] (validated), Cancel or close answers `None` and leaves
//! all state untouched. Control rows live in [`rows`], raw Win32 in
//! [`ffi`].

use std::cell::RefCell;
use std::io;
use std::path::Path;

use crate::settings::{Hotkey, Settings};
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
    BN_CLICKED, IsDialogMessageW, MB_ICONERROR_OK, MessageBoxW, SetBkMode, SetTextColor,
    TRANSPARENT, WS_CAPTION, WS_CLIPCHILDREN, WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_SYSMENU,
    WS_VISIBLE,
};
use self::fields::{browse_folder, check_state, combo_choice, edit_text};
use self::frame::{dpi_scale, drop_font, fit_scale, install_title_icon, ui_font, window_frame};
use self::rows::{attached_count, codec_list, display_entries, populate};

// ---------------------------------------------------------------------------
// Control ids routed through WM_COMMAND.
// ---------------------------------------------------------------------------

const COMBO_MODE: i32 = 101;
const COMBO_DISPLAY: i32 = 102;
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
    /// New capture mode id (`full`, `region`, `window`).
    pub mode: String,
    /// New Full-mode display choice (`primary` or `N`).
    pub display: String,
    /// New recording profile id.
    pub profile: String,
    /// New codec id.
    pub codec: String,
    /// Whether recordings redraw the mouse pointer.
    pub cursor: bool,
    /// Whether the app launches at login.
    pub autostart: bool,
    /// New photo depth (`8` or `16`).
    pub depth: String,
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
    /// The one-line hotkey status label under the two combo fields.
    hotkey_status: Option<Hwnd>,
    /// Whether that label currently reports a conflict (painted red).
    hotkey_warning: bool,
    /// The dialog-wide `Segoe UI` font, created in `run` and deleted with
    /// it (children get it via `WM_SETFONT` in `rows`).
    font: Option<*mut c_void>,
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
    install_title_icon(window, instance);
    populate(settings, window);
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
            }
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
    let modes = ["full", "region", "window"];
    let mode = modes[combo_choice(COMBO_MODE)].to_owned();
    let displays = display_entries(attached_count());
    let display = displays[combo_choice(COMBO_DISPLAY)].clone();
    let profile = crate::recording::Profile::PROFILES[combo_choice(COMBO_PROFILE)]
        .id
        .to_owned();
    let codec = codec_list()[combo_choice(COMBO_CODEC)].id().to_owned();
    let depths = ["8", "16"];
    let depth = depths[combo_choice(COMBO_DEPTH)].to_owned();
    let record = edit_text(EDIT_RECORD);
    let photo = edit_text(EDIT_PHOTO);
    let chime = edit_text(EDIT_CHIMES);
    let videos = edit_text(EDIT_VIDEOS);
    let images = edit_text(EDIT_IMAGES);
    let cursor = check_state(CHECK_CURSOR);
    let autostart = check_state(CHECK_AUTOSTART);
    let chimes = check_state(CHECK_CHIMES);

    let Some(parsed_record) = Hotkey::parse(&record) else {
        complain(
            window,
            "The record hotkey is not a valid combo (e.g. ctrl+shift+x).",
        );
        return;
    };
    let Some(parsed_photo) = Hotkey::parse(&photo) else {
        complain(
            window,
            "The photo hotkey is not a valid combo (e.g. ctrl+shift+z).",
        );
        return;
    };
    let Some(parsed_chime) = Hotkey::parse(&chime) else {
        complain(
            window,
            "The chime hotkey is not a valid combo (e.g. ctrl+shift+m).",
        );
        return;
    };
    if parsed_record == parsed_photo
        || parsed_record == parsed_chime
        || parsed_photo == parsed_chime
    {
        complain(
            window,
            "The record, photo, and chime hotkeys must all differ.",
        );
        return;
    }
    // The live probe lifts this process's own registrations, so the
    // current combos answer free; another process holding one is a real
    // conflict that would make `RegisterHotKey` fail after a save.
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
            return;
        }
    }
    for (name, value) in [("videos", videos.as_str()), ("images", images.as_str())] {
        if value.trim().is_empty() || !Path::new(value.trim()).is_absolute() {
            complain(
                window,
                &format!("The {name} folder needs an absolute path (e.g. C:\\Users\\me\\Videos)."),
            );
            return;
        }
    }
    DIALOG.with_borrow_mut(|dialog| {
        dialog.answer = Some(Answers {
            mode,
            display,
            profile,
            codec,
            cursor,
            autostart,
            depth,
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
    use super::{CLASS_NAME, TITLE};

    #[test]
    fn constants_stay_recognizable() {
        assert!(CLASS_NAME.starts_with("screencapture."));
        assert!(TITLE.contains("screencapture"));
    }
}
