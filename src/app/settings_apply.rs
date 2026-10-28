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

/// Write a saved answer into the settings document.
///
/// This is the one place where the dialog's fields become config keys, and
/// `settings::load` is what reads them back. It is kept apart from the live
/// services the rest of `apply_settings` touches so the mapping can be
/// round-tripped in a test: a key renamed here is a setting that silently
/// stops saving.
#[cfg(windows)]
fn store_answers(document: &mut crate::config::Document, answers: &Answers) {
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
    document.set("photo", "quality", Value::Str(answers.quality.clone()));
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
    // A folder picked here is a choice, not the default an older build
    // happened to write: without this, a folder that is one of those
    // defaults (`Videos`, or the `Pictures` copy a redirect left behind) is
    // migrated to today's answer on the next load, and the window shows a
    // folder nobody picked.
    document.set("media", "chosen", Value::Bool(true));
    document.set("hotkeys", "record", Value::Str(answers.record.clone()));
    document.set("hotkeys", "photo", Value::Str(answers.photo.clone()));
    document.set(
        "hotkeys",
        "chimes",
        Value::Str(answers.chime_hotkey.clone()),
    );
}

/// The same seam where no tray runs yet; the settings file stays the
/// interface there (CLI --init template explains it).
#[cfg(not(windows))]
pub(super) fn open_settings(_loaded: &mut Loaded, _runtime: &mut Runtime) {}

/// Apply a saved settings answer: mutate the document, persist, reload
/// the typed view, resync autostart, swap the hotkeys live.
#[cfg(windows)]
fn apply_settings(loaded: &mut Loaded, runtime: &mut Runtime, answers: &Answers) {
    store_answers(&mut loaded.document, answers);
    let target = loaded.path.clone();
    if let Some(path) = &target
        && let Err(error) = loaded.document.persist(path)
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

#[cfg(all(test, windows))]
mod tests {
    use std::path::Path;

    use super::store_answers;
    use crate::audio::{Levels, Quality, Wanted};
    use crate::encoder::Video;
    use crate::settings::{self, AudioDevices, Endpoint, Hotkey, StillDepth, StillQuality};

    use crate::settings_dialog::Answers;

    /// One answer per dialog field, each different from the machine default
    /// where the default is knowable, so a key that stops being written is
    /// visible as a value that came back unchanged.
    fn answers(media: &Path) -> Answers {
        Answers {
            profile: "hd-720p30".to_owned(),
            codec: "x265".to_owned(),
            cursor: false,
            audio: Wanted {
                mic: true,
                system: false,
                quality: Quality::CHOICES[1],
            },
            devices: AudioDevices {
                mic: Some(Endpoint {
                    id: "mic-endpoint-id".to_owned(),
                    name: "Blue Yeti".to_owned(),
                }),
                system: None,
            },
            levels: Levels {
                mic_db: -12,
                system_db: 6,
            },
            autostart: false,
            depth: "8".to_owned(),
            quality: StillQuality::High.to_string(),
            chimes: false,
            videos: media.display().to_string(),
            images: media.display().to_string(),
            record: "ctrl+alt+9".to_owned(),
            photo: "shift+f9".to_owned(),
            chime_hotkey: "win+ctrl+7".to_owned(),
        }
    }

    /// Saving from the dialog must survive the file. This walks the whole
    /// contract: the answer's keys into the document, the document through
    /// `persist`, and the file back through `settings::load`. A key renamed
    /// on either side of that seam fails here instead of silently losing a
    /// setting in the shipped build.
    #[test]
    fn a_saved_answer_comes_back_from_the_file_unchanged() {
        let media = std::env::temp_dir().join("screencapture-apply-media");
        let path =
            std::env::temp_dir().join(format!("screencapture-apply-{}.cfg", std::process::id()));
        let answers = answers(&media);

        let mut loaded = settings::load(None);
        store_answers(&mut loaded.document, &answers);
        loaded
            .document
            .persist(&path)
            .expect("the settings file is written");

        let reloaded = settings::load(Some(&path));
        let _ = std::fs::remove_file(&path);
        assert!(reloaded.problems.is_empty(), "{:?}", reloaded.problems);

        let saved = &reloaded.settings;
        assert_eq!(saved.profile.id, answers.profile);
        assert_eq!(saved.codec, Video::X265);
        assert!(!saved.cursor);
        assert!(saved.audio.mic);
        assert!(!saved.audio.system);
        assert_eq!(saved.audio.quality, answers.audio.quality);
        assert_eq!(saved.audio_levels, answers.levels);
        assert_eq!(saved.audio_devices.mic, answers.devices.mic);
        assert_eq!(saved.audio_devices.system, None);
        assert!(!saved.autostart);
        assert_eq!(saved.photo_depth, StillDepth::Eight);
        assert_eq!(saved.photo_quality, StillQuality::High);
        assert!(!saved.chimes);
        assert_eq!(
            saved.hotkey,
            Hotkey::parse(&answers.record).expect("record combo")
        );
        assert_eq!(
            saved.photo,
            Hotkey::parse(&answers.photo).expect("photo combo")
        );
        assert_eq!(
            saved.chime_hotkey,
            Hotkey::parse(&answers.chime_hotkey).expect("chime combo")
        );
        // One folder for both outputs is persisted as the two subfolders
        // the files actually land in.
        assert_eq!(saved.video_dir, media.join("video"));
        assert_eq!(saved.image_dir, media.join("image"));
    }

    /// A folder the window submitted is a choice, not the default an older
    /// build wrote: the save has to mark the pair, or the next load migrates
    /// the choice to another folder and the window shows one nobody picked.
    #[test]
    fn a_folder_picked_in_settings_survives_an_old_default() {
        let legacy = crate::paths::legacy_media_dirs();
        let (Some(videos), Some(images)) = (legacy.first(), legacy.get(1)) else {
            return; // no home directory in this environment
        };
        let path = std::env::temp_dir().join(format!(
            "screencapture-apply-legacy-{}.cfg",
            std::process::id()
        ));
        let mut answers = answers(videos);
        answers.videos = videos.display().to_string();
        answers.images = images.display().to_string();

        let mut loaded = settings::load(None);
        store_answers(&mut loaded.document, &answers);
        loaded
            .document
            .persist(&path)
            .expect("the settings file is written");
        let reloaded = settings::load(Some(&path));
        let _ = std::fs::remove_file(&path);
        assert!(reloaded.problems.is_empty(), "{:?}", reloaded.problems);
        assert_eq!(
            reloaded.settings.video_dir,
            videos.clone(),
            "the folder the window saved is where recordings land"
        );
        assert_eq!(
            reloaded.settings.image_dir,
            images.clone(),
            "and the stills folder too"
        );
    }

    /// An answer with no chosen device must clear the remembered one, not
    /// leave the previous endpoint behind to be recorded from.
    #[test]
    fn clearing_a_device_removes_its_remembered_endpoint() {
        let media = std::env::temp_dir().join("screencapture-apply-media");
        let path = std::env::temp_dir().join(format!(
            "screencapture-apply-clear-{}.cfg",
            std::process::id()
        ));
        let mut answers = answers(&media);
        answers.devices.system = Some(Endpoint {
            id: "system-endpoint-id".to_owned(),
            name: "Speakers".to_owned(),
        });

        let mut loaded = settings::load(None);
        store_answers(&mut loaded.document, &answers);
        assert_eq!(
            loaded.settings.audio_devices.system, None,
            "the typed view is only replaced after a reload"
        );
        loaded
            .document
            .persist(&path)
            .expect("the settings file is written");
        let with_system = settings::load(Some(&path));
        assert_eq!(
            with_system.settings.audio_devices.system,
            answers.devices.system
        );

        answers.devices.system = None;
        store_answers(&mut loaded.document, &answers);
        loaded
            .document
            .persist(&path)
            .expect("the settings file is rewritten");
        let cleared = settings::load(Some(&path));
        let _ = std::fs::remove_file(&path);
        assert_eq!(cleared.settings.audio_devices.system, None);
        assert!(cleared.problems.is_empty(), "{:?}", cleared.problems);
    }
}
