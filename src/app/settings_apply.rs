//! Settings application: open the tray Settings window, then commit a
//! saved answer to the document and the live services (persist, reload
//! the typed view, resync autostart, re-register the hotkeys). A closed
//! window (Save never pressed) leaves every piece of state untouched.

#[cfg(windows)]
use std::path::PathBuf;

use crate::settings::Loaded;

use super::Runtime;

#[cfg(windows)]
use super::sync_autostart;
#[cfg(windows)]
use crate::config::Value;
#[cfg(windows)]
use crate::hotkeys;
#[cfg(windows)]
use crate::log;
#[cfg(windows)]
use crate::settings;
#[cfg(windows)]
use crate::settings_dialog::{self, Answers};
#[cfg(windows)]
use crate::tray;

/// Open the settings window; a saved answer mutates the document,
/// persists it, reloads the typed view, re-synchronizes autostart, and
/// re-registers hotkeys live. Closing without saving leaves all state.
#[cfg(windows)]
pub(super) fn open_settings(loaded: &mut Loaded, runtime: &mut Runtime) {
    // A modal surface stops the active recording first (the picker rule):
    // otherwise frame-timer messages queue behind this handler for the whole
    // time the dialog is open and the recording stalls.
    super::stop_recording(runtime);
    match settings_dialog::run(&loaded.settings) {
        Ok(Some(answers)) => apply_settings(loaded, runtime, &answers),
        Ok(None) => {}
        Err(error) => log::note(
            "error",
            &format!("cannot open the settings window: {error}"),
        ),
    }
    // The window, its controls, and its font are gone with the dialog; a
    // session left open for a while has also faulted in pages this tray —
    // which runs for days — has no use for again. Hand them back now.
    // Opening the dialog stopped any live recording, and one still
    // finalizing is not idle memory: its worker may still be reading
    // capture buffers, so the trim waits for the watchdog's own pass.
    if runtime.idle() {
        crate::trim::idle();
    }
}

/// The same seam where no tray runs yet; the settings file stays the
/// interface there (CLI --init template explains it).
#[cfg(not(windows))]
pub(super) fn open_settings(_loaded: &mut Loaded, _runtime: &mut Runtime) {}

/// Apply a saved settings answer: mutate the document, persist, reload
/// the typed view, resync autostart, swap the hotkeys live.
#[cfg(windows)]
fn apply_settings(loaded: &mut Loaded, runtime: &mut Runtime, answers: &Answers) {
    let document = &mut loaded.document;
    document.set("capture", "chimes", Value::Bool(answers.chimes));
    document.set("recording", "profile", Value::Str(answers.profile.clone()));
    document.set("recording", "codec", Value::Str(answers.codec.clone()));
    document.set("recording", "cursor", Value::Bool(answers.cursor));
    document.set("recording", "mic", Value::Bool(answers.audio.mic));
    document.set(
        "recording",
        "system_audio",
        Value::Bool(answers.audio.system),
    );
    document.set(
        "recording",
        "audio_quality",
        Value::Str(answers.audio.quality.id().to_owned()),
    );
    document.set(
        "recording",
        "mic_level",
        Value::Int(i64::from(answers.levels.mic_db)),
    );
    document.set(
        "recording",
        "system_level",
        Value::Int(i64::from(answers.levels.system_db)),
    );
    for (id_key, name_key, endpoint) in [
        (
            "mic_device",
            "mic_device_name",
            answers.devices.mic.as_ref(),
        ),
        (
            "system_device",
            "system_device_name",
            answers.devices.system.as_ref(),
        ),
    ] {
        if let Some(endpoint) = endpoint {
            document.set("recording", id_key, Value::Str(endpoint.id.clone()));
            document.set("recording", name_key, Value::Str(endpoint.name.clone()));
        } else {
            document.remove("recording", id_key);
            document.remove("recording", name_key);
        }
    }
    document.set("recording", "autostart", Value::Bool(answers.autostart));
    document.set("photo", "depth", Value::Str(answers.depth.clone()));
    // Choosing one folder for both outputs splits it into `video` and
    // `image` subfolders; persist the effective paths so the file shows
    // where files actually land.
    let (videos, images) = settings::split_shared_media(
        PathBuf::from(answers.videos.trim()),
        PathBuf::from(answers.images.trim()),
    );
    document.set("media", "videos", Value::Str(videos.display().to_string()));
    document.set("media", "images", Value::Str(images.display().to_string()));
    document.set("hotkeys", "record", Value::Str(answers.record.clone()));
    document.set("hotkeys", "photo", Value::Str(answers.photo.clone()));
    document.set(
        "hotkeys",
        "chimes",
        Value::Str(answers.chime_hotkey.clone()),
    );
    let target = loaded.path.clone();
    if let Some(path) = &target
        && let Err(error) = document.persist(path)
    {
        log::note("error", &format!("cannot save settings: {error}"));
        // The document now holds values the file does not, and the typed
        // view still holds the old ones: reload so every later save starts
        // from the disk copy instead of persisting a half-applied edit.
        *loaded = settings::load(loaded.path.as_deref());
        return;
    }
    let saved = settings::load(loaded.path.as_deref());
    for problem in &saved.problems {
        log::note("warn", &format!("settings: {problem}"));
    }
    *loaded = saved;
    runtime.media_dir.clone_from(&loaded.settings.video_dir);
    tray::set_bindings(&hotkeys::bindings(
        loaded.settings.hotkey,
        loaded.settings.photo,
        loaded.settings.chime_hotkey,
    ));
    tray::set_chimes(loaded.settings.chimes);
    // Sync the OS login registration with the new setting.
    if let Err(error) = sync_autostart(loaded) {
        log::note("warn", &format!("autostart: {error}"));
    }
    log::note("settings", "saved and applied");
}
