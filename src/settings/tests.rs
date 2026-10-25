//! Settings parse/fallback tests, kept beside the module they exercise.

use std::path::PathBuf;

use super::{
    CaptureMode, DEFAULT_CAPTURE_MODE, DisplayChoice, Document, Hotkey, StillDepth, StillQuality,
    default_document, read_document,
};

#[test]
fn builtin_template_round_trips_through_defaults() {
    let document = default_document();
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.profile.id, "hd-1080p60");
    assert_eq!(settings.hotkey, Hotkey::RECORD);
    assert_eq!(settings.photo, Hotkey::PHOTO);
    assert_eq!(settings.chime_hotkey, Hotkey::CHIMES);
    assert_eq!(settings.capture_mode, DEFAULT_CAPTURE_MODE);
    assert_eq!(settings.display, DisplayChoice::Primary);
    assert!(settings.autostart);
    assert!(!settings.cursor);
    assert_eq!(settings.photo_depth, StillDepth::Eight);
    assert_eq!(settings.photo_quality, StillQuality::Png);
    assert!(settings.chimes, "captures are audible by default");
    assert!(!settings.audio.mic, "the microphone starts muted");
    assert!(settings.audio.system, "system sound is on by default");
    assert_eq!(
        settings.audio.quality,
        crate::audio::Quality::High,
        "the track starts at full quality"
    );
    assert!(settings.engine.is_none());
    assert_eq!(problems.len(), 0);
    let reparsed = Document::from_text(&document.to_text()).expect("rendered text must reparse");
    assert_eq!(document, reparsed);
}

#[test]
fn capture_mode_parses_and_falls_back() {
    assert_eq!(CaptureMode::parse("REGION"), Some(CaptureMode::Region));
    assert_eq!(CaptureMode::parse("window"), Some(CaptureMode::Window));
    assert_eq!(CaptureMode::parse("full"), Some(CaptureMode::Full));
    assert_eq!(CaptureMode::parse("whatever"), None);
}

#[test]
fn display_choice_parses_primary_and_numbers() {
    assert_eq!(
        DisplayChoice::parse("PRIMARY"),
        Some(DisplayChoice::Primary)
    );
    assert_eq!(DisplayChoice::parse("2"), Some(DisplayChoice::Number(2)));
    assert_eq!(DisplayChoice::parse("17"), None);
    assert_eq!(DisplayChoice::parse("one"), None);
    assert_eq!(DisplayChoice::Number(3).number(), Some(3));
    assert_eq!(DisplayChoice::Primary.number(), None);
}

#[test]
fn still_quality_parses_and_survives_the_file() {
    for (text, quality) in [
        ("lossless", StillQuality::Png),
        ("PNG", StillQuality::Png),
        ("webp", StillQuality::Webp),
        ("lossless-webp", StillQuality::Webp),
        ("BMP", StillQuality::Bmp),
        ("high", StillQuality::High),
        ("Standard", StillQuality::Standard),
        ("compact", StillQuality::Compact),
    ] {
        assert_eq!(StillQuality::parse(text), Some(quality), "{text}");
    }
    assert_eq!(StillQuality::parse("maximum"), None);

    // Each step writes its own file type, and Display/parse are the pair
    // the settings document round-trips through.
    for quality in StillQuality::CHOICES {
        let text = quality.to_string();
        assert_eq!(StillQuality::parse(&text), Some(quality), "{text}");
        let document =
            Document::from_text(&format!("[photo]\nquality = {text}\n")).expect("value parses");
        let (settings, problems) = read_document(&document);
        assert_eq!(settings.photo_quality, quality, "{text}");
        assert_eq!(problems.len(), 0, "{text}");
    }
    assert_eq!(StillQuality::Png.extension(), "png");
    assert_eq!(StillQuality::Webp.extension(), "webp");
    assert_eq!(StillQuality::Bmp.extension(), "bmp");
    assert_eq!(StillQuality::High.extension(), "jpg");
    assert_eq!(StillQuality::Standard.extension(), "jpg");
    assert_eq!(StillQuality::Compact.extension(), "jpg");
    assert_eq!(StillQuality::High.jpeg_qscale(), Some("2"));
    assert_eq!(StillQuality::Standard.jpeg_qscale(), Some("4"));
    assert_eq!(StillQuality::Compact.jpeg_qscale(), Some("7"));
    assert_eq!(StillQuality::Png.jpeg_qscale(), None);
    assert_eq!(StillQuality::Webp.jpeg_qscale(), None);
    assert_eq!(StillQuality::Bmp.jpeg_qscale(), None);
}

#[test]
fn an_unknown_still_quality_falls_back_to_lossless() {
    let document = Document::from_text("[photo]\nquality = enormous\n").expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.photo_quality, StillQuality::Png);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("photo quality"), "{problems:?}");
}

#[test]
fn still_depth_parses_and_falls_back() {
    assert_eq!(
        super::StillDepth::parse("16"),
        Some(super::StillDepth::Sixteen)
    );
    assert_eq!(
        super::StillDepth::parse("8-BIT"),
        Some(super::StillDepth::Eight)
    );
    assert_eq!(super::StillDepth::parse("ten"), None);
    assert_eq!(super::StillDepth::Eight.to_string(), "8");
    assert_eq!(super::StillDepth::Sixteen.to_string(), "16");
}

#[test]
fn deep_photo_setting_round_trips() {
    for value in ["16", "\"16\""] {
        let document =
            Document::from_text(&format!("[photo]\ndepth = {value}\n")).expect("value parses");
        let (settings, problems) = read_document(&document);
        assert_eq!(settings.photo_depth, StillDepth::Sixteen, "{value}");
        assert_eq!(problems.len(), 0);
    }

    let document = Document::from_text("[photo]\ndepth = 12\n").expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.photo_depth, StillDepth::Eight);
    assert_eq!(problems.len(), 1);
}

#[test]
fn media_folders_split_videos_and_images() {
    let document = Document::from_text(concat!(
        "[media]\n",
        "videos = \"C:\\captures\\clips\"\n",
        "images = \"D:\\pictures\\stills\"\n",
    ))
    .expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.video_dir, PathBuf::from("C:\\captures\\clips"));
    assert_eq!(settings.image_dir, PathBuf::from("D:\\pictures\\stills"));
    assert_eq!(problems.len(), 0);
}

#[test]
fn legacy_media_directory_keeps_recordings_moving() {
    let document =
        Document::from_text("[media]\ndirectory = \"C:\\media\"\n").expect("sample parses");
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.video_dir, PathBuf::from("C:\\media"));
    // Photos keep their own Pictures default; the legacy key pointed
    // old configs at the recordings folder by design.
    assert_eq!(settings.image_dir, crate::paths::image_dir().unwrap());
    assert_eq!(problems.len(), 0);
}

#[test]
fn recordings_get_their_own_folder_by_default() {
    let Some(expected) = crate::paths::video_dir() else {
        return; // no home directory in this environment
    };

    // A fresh default config points at the subfolder.
    let (settings, problems) = read_document(&default_document());
    assert_eq!(settings.video_dir, expected);
    assert_eq!(problems.len(), 0);

    // Every folder an earlier build wrote as its default follows today's
    // answer instead of pinning a path the user never chose.
    for legacy in crate::paths::legacy_media_dirs() {
        let document =
            Document::from_text(&format!("[media]\nvideos = \"{}\"\n", legacy.display()))
                .expect("sample must parse");
        let (settings, problems) = read_document(&document);
        assert_eq!(settings.video_dir, expected, "legacy {}", legacy.display());
        assert_eq!(problems.len(), 0);
    }

    // ...while a folder the user actually chose is kept untouched.
    let chosen =
        Document::from_text("[media]\nvideos = \"C:\\clips\\mine\"\n").expect("sample must parse");
    let (settings, _) = read_document(&chosen);
    assert_eq!(settings.video_dir, PathBuf::from("C:\\clips\\mine"));
}

/// The reason this migration exists: an account whose `Pictures` folder
/// was redirected (`OneDrive`'s Known Folder Move does it to every signed-in
/// profile) keeps a stale profile copy, and a config written against it
/// must follow the shell to the folder the user actually opens.
#[test]
fn stills_follow_the_pictures_folder_the_shell_names() {
    let Some(expected) = crate::paths::image_dir() else {
        return; // no home directory in this environment
    };
    let (settings, problems) = read_document(&default_document());
    assert_eq!(settings.image_dir, expected);
    assert_eq!(problems.len(), 0);

    for legacy in crate::paths::legacy_media_dirs() {
        let document =
            Document::from_text(&format!("[media]\nimages = \"{}\"\n", legacy.display()))
                .expect("sample must parse");
        let (settings, _) = read_document(&document);
        assert_eq!(settings.image_dir, expected, "legacy {}", legacy.display());
    }
    // A folder the user chose, even one that looks similar, is kept.
    let chosen =
        Document::from_text("[media]\nimages = \"D:\\shots\\mine\"\n").expect("sample must parse");
    let (settings, _) = read_document(&chosen);
    assert_eq!(settings.image_dir, PathBuf::from("D:\\shots\\mine"));
}

#[test]
fn one_folder_for_both_media_gets_video_and_image_subfolders() {
    let shared = PathBuf::from("C:\\captures");
    let document = Document::from_text(&format!(
        "[media]\nvideos = \"{}\"\nimages = \"{}\"\n",
        shared.display(),
        shared.display()
    ))
    .expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.video_dir, shared.join("video"));
    assert_eq!(settings.image_dir, shared.join("image"));
    assert_eq!(problems.len(), 0);

    // Different folders keep the plain `Screencapture` folders, with no
    // extra level added.
    let split = Document::from_text(
        "[media]\nvideos = \"C:\\clips\\Screencapture\"\nimages = \"C:\\stills\\Screencapture\"\n",
    )
    .expect("sample must parse");
    let (settings, _) = read_document(&split);
    assert_eq!(
        settings.video_dir,
        PathBuf::from("C:\\clips\\Screencapture")
    );
    assert_eq!(
        settings.image_dir,
        PathBuf::from("C:\\stills\\Screencapture")
    );
}

#[test]
fn shared_folder_matching_ignores_case_and_trailing_separator() {
    let document = Document::from_text(concat!(
        "[media]\n",
        "videos = \"C:\\Captures/\"\n",
        "images = \"c:\\captures\"\n",
    ))
    .expect("sample must parse");
    let (settings, _) = read_document(&document);
    assert_eq!(
        settings.video_dir,
        PathBuf::from("C:\\Captures").join("video")
    );
    assert_eq!(
        settings.image_dir,
        PathBuf::from("c:\\captures").join("image")
    );
}

#[test]
fn invalid_values_fall_back_with_problems() {
    let document = Document::from_text(
        "[hotkeys]\nrecord = super+space\n[recording]\nprofile = big\nautostart = yes-please\n",
    )
    .expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.hotkey, Hotkey::RECORD);
    assert_eq!(settings.profile.id, "hd-1080p60");
    assert!(settings.autostart);
    assert_eq!(problems.len(), 3);
}

#[test]
fn invalid_chime_hotkey_falls_back_with_a_problem() {
    let document =
        Document::from_text("[hotkeys]\nchimes = ctrl+nope\n").expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.chime_hotkey, Hotkey::CHIMES);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("chime"));
}

#[test]
fn identical_hotkeys_fall_back_to_the_defaults() {
    let document = Document::from_text("[hotkeys]\nrecord = ctrl+shift+x\nphoto = ctrl+shift+x\n")
        .expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.hotkey, Hotkey::RECORD);
    assert_eq!(settings.photo, Hotkey::PHOTO);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("identical"));
}

#[test]
fn a_duplicate_chime_hotkey_falls_back_to_the_defaults() {
    let document = Document::from_text(
        "[hotkeys]\nrecord = ctrl+shift+m\nphoto = ctrl+shift+z\nchimes = ctrl+shift+m\n",
    )
    .expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.hotkey, Hotkey::RECORD);
    assert_eq!(settings.photo, Hotkey::PHOTO);
    assert_eq!(settings.chime_hotkey, Hotkey::CHIMES);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("identical"));
}

#[test]
fn chimes_can_be_muted_and_fall_back_loud() {
    let muted = Document::from_text("[capture]\nchimes = false\n").expect("sample must parse");
    let (settings, problems) = read_document(&muted);
    assert!(!settings.chimes);
    assert_eq!(problems.len(), 0);

    let invalid = Document::from_text("[capture]\nchimes = maybe\n").expect("sample must parse");
    let (settings, problems) = read_document(&invalid);
    assert!(settings.chimes, "an invalid flag keeps the audible default");
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("chimes"));
}

#[test]
fn invalid_booleans_report_their_fallbacks() {
    let document = Document::from_text("[recording]\nautostart = yes-please\ncursor = sometimes\n")
        .expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert!(settings.autostart);
    assert!(!settings.cursor);
    assert_eq!(problems.len(), 2);
}

/// The two audio switches answer independently, and each has its own
/// fallback: an unreadable value never turns the microphone on by accident.
#[test]
fn audio_switches_default_and_fall_back_separately() {
    let document = Document::from_text("[recording]\nmic = true\nsystem_audio = false\n")
        .expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert!(settings.audio.mic);
    assert!(!settings.audio.system);
    assert_eq!(problems.len(), 0);

    let invalid = Document::from_text("[recording]\nmic = maybe\nsystem_audio = loud\n")
        .expect("sample must parse");
    let (settings, problems) = read_document(&invalid);
    assert!(!settings.audio.mic, "an invalid mic flag stays muted");
    assert!(
        settings.audio.system,
        "an invalid system flag keeps sound on"
    );
    assert_eq!(problems.len(), 2);
    assert!(problems.iter().any(|problem| problem.contains("mic")));
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("system_audio"))
    );
}

/// `--init` writes the commented template, not a bare rendered document:
/// the comments are the only place the quality values are explained, and
/// the file has to parse back to the same defaults the app runs with.
#[test]
fn the_init_template_carries_its_comments() {
    let directory =
        std::env::temp_dir().join(format!("screencapture-template-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let path = directory.join("settings.ini");
    super::write_template(&path).expect("write the template");
    let text = std::fs::read_to_string(&path).expect("read it back");
    for wanted in [
        "# screencapture settings",
        "# audio_quality: the track's AAC bitrate",
        "audio_quality = high",
        "profile = hd-1080p60",
    ] {
        assert!(text.contains(wanted), "missing {wanted:?}: {text}");
    }
    let document = Document::from_text(&text).expect("the written template must parse");
    let (settings, problems) = read_document(&document);
    assert_eq!(problems, Vec::<String>::new());
    assert_eq!(settings.profile.id, "hd-1080p60");
    assert_eq!(settings.audio.quality, crate::audio::Quality::High);
    assert!(!directory.join("settings.ini.tmp").exists());
    let _ = std::fs::remove_dir_all(&directory);
}

/// The track's quality follows its key and falls back to the full bitrate,
/// never to a thinner track the user did not ask for.
#[test]
fn audio_quality_parses_and_falls_back() {
    let document =
        Document::from_text("[recording]\naudio_quality = speech\n").expect("sample must parse");
    let (settings, problems) = read_document(&document);
    assert_eq!(settings.audio.quality, crate::audio::Quality::Speech);
    assert_eq!(problems.len(), 0);

    let invalid =
        Document::from_text("[recording]\naudio_quality = tinny\n").expect("sample must parse");
    let (settings, problems) = read_document(&invalid);
    assert_eq!(settings.audio.quality, crate::audio::Quality::High);
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("audio_quality")),
        "the unreadable value is reported: {problems:?}"
    );
}

/// A chosen endpoint is remembered by id and name; an empty id means the
/// Windows default, and a fader outside the mixable range is clamped with a
/// note instead of failing the whole file.
#[test]
fn audio_devices_and_levels_round_trip() {
    let text = "\
[recording]
mic = true
mic_device = {0.0.1.00000000}.{abc}
mic_device_name = Headset Microphone
system_device = {0.0.0.00000000}.{def}
system_device_name = Speakers
mic_level = 6
system_level = -60
";
    let document = Document::from_text(text).expect("valid settings");
    let (settings, problems) = read_document(&document);
    assert_eq!(problems.len(), 0);
    let mic = settings
        .audio_devices
        .mic
        .clone()
        .expect("a microphone is chosen");
    assert_eq!(mic.id, "{0.0.1.00000000}.{abc}");
    assert_eq!(mic.name, "Headset Microphone");
    assert_eq!(
        settings
            .audio_devices
            .system
            .clone()
            .expect("a playback device")
            .name,
        "Speakers"
    );
    assert_eq!(settings.audio_levels.mic_db, 6);
    assert_eq!(settings.audio_levels.system_db, -60);
    let ids = settings.audio_devices.ids();
    assert_eq!(ids.mic.as_deref(), Some("{0.0.1.00000000}.{abc}"));

    // A device without its name is kept under its id; an empty id follows
    // the default; a silly fader is clamped and reported.
    let text = "\
[recording]
mic_device = {0.0.1.00000000}.{abc}
mic_device_name =
mic_level = 120
";
    let document = Document::from_text(text).expect("valid settings");
    let (settings, problems) = read_document(&document);
    assert_eq!(
        settings
            .audio_devices
            .mic
            .clone()
            .expect("still chosen")
            .name,
        "{0.0.1.00000000}.{abc}"
    );
    assert!(settings.audio_devices.system.is_none());
    assert_eq!(settings.audio_levels.mic_db, crate::audio::Levels::MAX_DB);
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("mic_device")),
        "the missing name is reported: {problems:?}"
    );
    assert!(
        problems.iter().any(|problem| problem.contains("mic_level")),
        "the clamped fader is reported: {problems:?}"
    );
}

/// The mixer that scales samples is the Windows one; elsewhere a fader is
/// only a stored number.
#[cfg(windows)]
#[test]
fn the_faders_gain_is_unity_at_zero_and_silence_at_the_floor() {
    use crate::audio::Levels;
    assert!((Levels::gain(0) - 1.0).abs() < f32::EPSILON);
    assert_eq!(Levels::gain(Levels::MIN_DB), 0.0);
    let boost = Levels::gain(6);
    assert!(boost > 1.9 && boost < 2.0, "6 dB doubles: {boost}");
    assert_eq!(Levels::clamp_level(99), Levels::MAX_DB);
}
