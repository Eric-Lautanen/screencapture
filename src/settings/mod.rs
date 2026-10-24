//! Settings: a typed view over the on-disk document, with safe defaults.

mod hotkey;
pub use hotkey::Hotkey;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::audio;
use crate::config::{Document, Value};
use crate::encoder::Video;
use crate::paths;
use crate::recording::{self, Profile};

const SETTINGS_FILE: &str = "settings.cfg";
const DEFAULT_DIRECTORY: &str = ".";

/// The still-photo pixel depth (`8` and `16` are real bit depths; 16
/// keeps every captured BGRA gradient lossless end-to-end, the way the
/// GDI blit and the PNG codec agree on).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum StillDepth {
    /// 8 bits per channel (the long-standing default).
    #[default]
    Eight,
    /// 16 bits per channel (48-bit RGB PNG; the 8-bit capture route
    /// benefits zero, so prefer it only for future 16-bit captures).
    Sixteen,
}

impl StillDepth {
    /// Parse a settings value (case-insensitive, bare depth).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "8" | "8-bit" | "eight" => Some(Self::Eight),
            "16" | "16-bit" | "sixteen" => Some(Self::Sixteen),
            _ => None,
        }
    }
}

impl std::fmt::Display for StillDepth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eight => write!(formatter, "8"),
            Self::Sixteen => write!(formatter, "16"),
        }
    }
}

/// Where a recording's frame comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureMode {
    /// The whole selected display (`display`; the primary by default).
    Full,
    /// Draw a rectangle on screen first (interactive).
    Region,
    /// One window: clicked in the interactive pass (which saves that
    /// window's own content), or auto-detected under the cursor by the
    /// command line.
    Window,
}

/// Which display a Full capture records: the one the capture starts on,
/// or a numbered one (1-based).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum DisplayChoice {
    /// Wherever the pointer is when the capture starts.
    #[default]
    Primary,
    /// A named part of the enumeration order, 1-based.
    Number(u32),
}

impl DisplayChoice {
    /// Parse a settings value: `primary` or a 1-based display number.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "primary" | "main" => Some(Self::Primary),
            _ => text
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|number| (1..=16).contains(number))
                .map(Self::Number),
        }
    }

    /// The 1-based part of the enumeration order, or `None` for primary.
    #[must_use]
    pub fn number(self) -> Option<u32> {
        match self {
            Self::Primary => None,
            Self::Number(number) => Some(number),
        }
    }
}

impl std::fmt::Display for DisplayChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Primary => write!(formatter, "primary"),
            Self::Number(number) => write!(formatter, "{number}"),
        }
    }
}

impl CaptureMode {
    /// Parse a settings value (case-insensitive).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "full" | "fullscreen" => Some(Self::Full),
            "region" | "draw" => Some(Self::Region),
            "window" | "app" => Some(Self::Window),
            _ => None,
        }
    }
}

impl std::fmt::Display for CaptureMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(formatter, "full"),
            Self::Region => write!(formatter, "region"),
            Self::Window => write!(formatter, "window"),
        }
    }
}

/// The mode a fresh install starts in. The interactive picker is a
/// Windows surface, so other platforms start from the whole display
/// rather than from a mode their build cannot run.
pub const DEFAULT_CAPTURE_MODE: CaptureMode = if cfg!(windows) {
    CaptureMode::Region
} else {
    CaptureMode::Full
};

/// The parsed settings view used across the application.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Motion-capture profile.
    pub profile: Profile,
    /// Video codec used for recordings.
    pub codec: Video,
    /// Where the recording frame comes from.
    pub capture_mode: CaptureMode,
    /// Which display a Full capture records.
    pub display: DisplayChoice,
    /// Start/stop-recording hotkey.
    pub hotkey: Hotkey,
    /// Save-a-still hotkey.
    pub photo: Hotkey,
    /// Mute/unmute the capture chimes.
    pub chime_hotkey: Hotkey,
    /// Launch at system startup.
    pub autostart: bool,
    /// Include the mouse pointer in captures.
    pub cursor: bool,
    /// Which audio sources recordings include.
    pub audio: audio::Wanted,
    /// Still-photo channel depth.
    pub photo_depth: StillDepth,
    /// Play a soft chime when a still is taken or a recording is stopped.
    pub chimes: bool,
    /// Where completed recordings land.
    pub video_dir: PathBuf,
    /// Where completed stills land.
    pub image_dir: PathBuf,
    /// Custom engine location; `None` uses the bundled engine.
    pub engine: Option<PathBuf>,
}

/// Outcome of loading settings: the document plus the typed view.
#[derive(Debug)]
pub struct Loaded {
    /// Absolute settings path when the platform provides one.
    pub path: Option<PathBuf>,
    /// True when an existing file was parsed (false means defaults are in use).
    pub found: bool,
    /// The document as parsed, or the machine defaults.
    pub document: Document,
    /// The typed view used by the application.
    pub settings: Settings,
    /// Human-readable problems; affected values fell back to defaults.
    pub problems: Vec<String>,
}

/// Load settings; missing or corrupt files fall back to machine defaults.
pub fn load(config: Option<&Path>) -> Loaded {
    let configured = config.map(PathBuf::from);
    let path = configured
        .clone()
        .or_else(|| paths::config_dir().map(|dir| dir.join(SETTINGS_FILE)));
    let mut problems: Vec<String> = Vec::new();
    let (found, document) = if let Some(file) = &path {
        match fs::read_to_string(file) {
            Ok(text) => match Document::from_text(&text) {
                Ok(document) => (true, document),
                Err(error) => {
                    problems.push(format!("cannot parse settings file: {error}"));
                    (false, default_document())
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => (false, default_document()),
            Err(error) => {
                problems.push(format!("cannot read settings file: {error}"));
                (false, default_document())
            }
        }
    } else {
        problems.push("no user directory is available".into());
        (false, default_document())
    };
    let (settings, mut value_problems) = read_document(&document);
    problems.append(&mut value_problems);
    Loaded {
        path,
        found,
        document,
        settings,
        problems,
    }
}

fn default_document() -> Document {
    Document::from_text(&template_text()).expect("built-in default template must parse")
}

/// The commented settings template with this machine's default folders:
/// what a new file starts from, and what `--init` writes, so the file a
/// user opens explains every key instead of listing bare values.
#[must_use]
pub fn template_text() -> String {
    let videos = paths::video_dir().unwrap_or_else(|| PathBuf::from(DEFAULT_DIRECTORY));
    let images = paths::image_dir().unwrap_or_else(|| PathBuf::from(DEFAULT_DIRECTORY));
    template(&videos, &images)
}

/// Write the commented template to `path`, creating its directory, via a
/// temporary file and a rename so a half-written settings file is never
/// left where the next launch would read it.
///
/// # Errors
/// Propagates a directory, write, or rename failure.
pub fn write_template(path: &Path) -> io::Result<()> {
    // A bare file name has an empty parent, which `create_dir_all` must
    // not be handed.
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    if let Err(error) = fs::write(&temporary, template_text()) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn template(videos: &Path, images: &Path) -> String {
    format!(
        concat!(
            "# screencapture settings\n",
            "# Edit by hand; unknown keys are preserved and ignored.\n",
            "\n",
            "[hotkeys]\n",
            "# Combos: ctrl | alt | shift | win, any order, joined by + or\n",
            "# -, then a key (digit, letter, or F1-F12); at least one\n",
            "# modifier is required. Defaults: ctrl+shift+x record,\n",
            "# ctrl+shift+z photo, ctrl+shift+m chimes.\n",
            "record = ctrl+shift+x\n",
            "photo = ctrl+shift+z\n",
            "chimes = ctrl+shift+m\n",
            "\n",
            "[capture]\n",
            "# mode: region (crosshair picker: click a window, drag a\n",
            "# rectangle), window (auto-detect at the cursor), or full.\n",
            "# The picker is Windows-only; other platforms start on full.\n",
            "mode = {mode}\n",
            "display = primary\n",
            "# chimes: a soft three-note confirmation when a still is taken\n",
            "# or a recording is stopped. Set false to keep captures silent.\n",
            "chimes = true\n",
            "\n",
            "[photo]\n",
            "# depth: 8 (default) or 16 (48-bit RGB PNG; both capture\n",
            "# routes today are 8-bit, so 16 is future-proofing).\n",
            "depth = 8\n",
            "\n",
            "[recording]\n",
            "profile = {profile}\n",
            "# codec: x264 (universal default), x265 (HEVC on CPU), or a\n",
            "# hardware accelerator: amf | qsv | nvenc | mf (H.264) and\n",
            "# amf-hevc | qsv-hevc | nvenc-hevc | mf-hevc (HEVC). Hardware\n",
            "# choices are probed on record start, then fall back to x264,\n",
            "# another hardware H.264 unit, and only then CPU x265.\n",
            "codec = x264\n",
            "cursor = false\n",
            "# Audio: mic records the default microphone, off by default so a\n",
            "# recording never surprises you with your own voice;\n",
            "# system_audio records what the default playback device plays.\n",
            "mic = false\n",
            "system_audio = true\n",
            "# audio_quality: the track's AAC bitrate — speech (96 kbps),\n",
            "# standard (128 kbps), or high (192 kbps, the default).\n",
            "audio_quality = high\n",
            "autostart = true\n",
            "\n",
            "[engine]\n",
            "# Empty uses the bundled engine beside the application.\n",
            "path = \n",
            "\n",
            "[media]\n",
            "# Separate folders for each output.\n",
            "# A legacy `directory` key still answers for recordings.\n",
            "videos = \"{videos}\"\n",
            "images = \"{images}\"\n",
        ),
        videos = videos.display(),
        images = images.display(),
        mode = DEFAULT_CAPTURE_MODE,
        profile = crate::recording::DEFAULT.id
    )
}

fn read_document(document: &Document) -> (Settings, Vec<String>) {
    let hotkey_raw = document.value("hotkeys", "record").and_then(Value::as_str);
    let hotkey = hotkey_raw.and_then(Hotkey::parse);
    let photo_raw = document.value("hotkeys", "photo").and_then(Value::as_str);
    let photo = photo_raw.and_then(Hotkey::parse);
    let toggle_raw = document.value("hotkeys", "chimes").and_then(Value::as_str);
    let toggle = toggle_raw.and_then(Hotkey::parse);
    let mode_raw = document.value("capture", "mode").and_then(Value::as_str);
    let mode = mode_raw.and_then(CaptureMode::parse);
    let display_raw = document.value("capture", "display").and_then(Value::as_str);
    let display = display_raw.and_then(DisplayChoice::parse);
    let profile_raw = document
        .value("recording", "profile")
        .and_then(Value::as_str);
    let profile = profile_raw.and_then(Profile::by_id);
    let codec_raw = document.value("recording", "codec").and_then(Value::as_str);
    let codec = codec_raw.and_then(Video::parse);
    let autostart_raw = document.value("recording", "autostart");
    let autostart = autostart_raw.and_then(Value::as_bool);
    let cursor_raw = document.value("recording", "cursor");
    let cursor = cursor_raw.and_then(Value::as_bool);
    let chimes_raw = document.value("capture", "chimes");
    let chimes = chimes_raw.and_then(Value::as_bool);
    let depth_raw = document.value("photo", "depth");
    let depth = still_depth(depth_raw);
    let mut problems = Vec::new();
    let audio = audio_settings(document, &mut problems);
    if depth_raw.is_some() && depth.is_none() {
        problems.push(format!("invalid photo depth; using {}", StillDepth::Eight));
    }
    if hotkey_raw.is_some() && hotkey.is_none() {
        problems.push(format!("invalid record hotkey; using {}", Hotkey::RECORD));
    }
    if photo_raw.is_some() && photo.is_none() {
        problems.push(format!("invalid photo hotkey; using {}", Hotkey::PHOTO));
    }
    if toggle_raw.is_some() && toggle.is_none() {
        problems.push(format!("invalid chime hotkey; using {}", Hotkey::CHIMES));
    }
    if mode_raw.is_some() && mode.is_none() {
        problems.push(format!(
            "invalid capture mode; using {DEFAULT_CAPTURE_MODE}"
        ));
    }
    let mode = usable_mode(mode.unwrap_or(DEFAULT_CAPTURE_MODE), &mut problems);
    if display_raw.is_some() && display.is_none() {
        problems.push(format!(
            "invalid capture display; using {}",
            DisplayChoice::Primary
        ));
    }
    if profile_raw.is_some() && profile.is_none() {
        problems.push(format!("invalid profile; using {}", recording::DEFAULT.id));
    }
    if codec_raw.is_some() && codec.is_none() {
        problems.push(format!("invalid codec; using {}", Video::DEFAULT.id()));
    }
    if autostart_raw.is_some() && autostart.is_none() {
        problems.push("invalid autostart flag; using true".to_owned());
    }
    if cursor_raw.is_some() && cursor.is_none() {
        problems.push("invalid cursor flag; using false".to_owned());
    }
    if chimes_raw.is_some() && chimes.is_none() {
        problems.push("invalid chimes flag; using true".to_owned());
    }
    // A file can hand us the same combo for two actions; registering the
    // duplicate fails, which would silently leave a hotkey dead.
    let (hotkey, photo, chime_hotkey) = match (
        hotkey.unwrap_or(Hotkey::RECORD),
        photo.unwrap_or(Hotkey::PHOTO),
        toggle.unwrap_or(Hotkey::CHIMES),
    ) {
        (record, picture, chime) if record == picture || record == chime || picture == chime => {
            problems.push("hotkeys are identical; using the defaults".to_owned());
            (Hotkey::RECORD, Hotkey::PHOTO, Hotkey::CHIMES)
        }
        (record, picture, chime) => (record, picture, chime),
    };
    let (video_dir, image_dir) =
        split_shared_media(video_directory(document), image_directory(document));
    let settings = Settings {
        profile: profile.unwrap_or(recording::DEFAULT),
        codec: codec.unwrap_or(Video::DEFAULT),
        capture_mode: mode,
        display: display.unwrap_or(DisplayChoice::Primary),
        hotkey,
        photo,
        chime_hotkey,
        autostart: autostart.unwrap_or(true),
        cursor: cursor.unwrap_or(false),
        audio,
        photo_depth: depth.unwrap_or(StillDepth::Eight),
        chimes: chimes.unwrap_or(true),
        video_dir,
        image_dir,
        engine: engine_path(document),
    };
    (settings, problems)
}

/// The capture mode this platform can actually run: the picker is
/// Windows-only, so a `region` value elsewhere captures the whole display
/// instead of failing before the recording starts.
fn usable_mode(mode: CaptureMode, problems: &mut Vec<String>) -> CaptureMode {
    if mode == CaptureMode::Region && !cfg!(windows) {
        problems.push("the region picker needs Windows; using full capture".to_owned());
        CaptureMode::Full
    } else {
        mode
    }
}

/// A photo-depth value: bare text (`"8"`, quoted or not) and bare
/// integers both answer, since the document stores either shape.
fn still_depth(value: Option<&Value>) -> Option<StillDepth> {
    match value {
        Some(Value::Str(text)) => StillDepth::parse(text),
        Some(Value::Int(number)) => match number {
            8 => Some(StillDepth::Eight),
            16 => Some(StillDepth::Sixteen),
            _ => None,
        },
        _ => None,
    }
}

fn video_directory(document: &Document) -> PathBuf {
    let chosen = document
        .value("media", "videos")
        .and_then(Value::as_str)
        .or_else(|| document.value("media", "directory").and_then(Value::as_str))
        .filter(|directory| !directory.trim().is_empty())
        .map(PathBuf::from);
    match chosen {
        Some(directory) if !is_legacy_media_default(&directory) => directory,
        // Unset, or a folder recordings used to land in by default: adopt
        // the `Videos/Screencapture` default so recordings get their own
        // folder like stills already do.
        _ => paths::video_dir().unwrap_or_else(|| PathBuf::from(DEFAULT_DIRECTORY)),
    }
}

/// The effective media folders. When both choices name the same folder,
/// each medium gets its own `video`/`image` subfolder instead of mixing
/// clips and stills in one directory (selecting the same folder twice is
/// an edge case, but a destructive one: a photo and a recording can no
/// longer collide by name only if they never share a directory).
pub(crate) fn split_shared_media(video: PathBuf, image: PathBuf) -> (PathBuf, PathBuf) {
    if same_path(&video, &image) {
        (video.join("video"), image.join("image"))
    } else {
        (video, image)
    }
}

/// Whether `directory` is exactly one of the folders earlier builds wrote
/// as a default. Such a config never chose anything, so it follows today's
/// default instead of pinning a folder the user never picked — which is how
/// a config written before `OneDrive` redirected `Pictures` keeps writing
/// screenshots to the copy nobody opens.
fn is_legacy_media_default(directory: &Path) -> bool {
    paths::legacy_media_dirs()
        .iter()
        .any(|legacy| same_path(directory, legacy))
}

/// Two paths naming the same folder, ignoring case and trailing
/// separators (the folders may not exist yet, so no canonicalization).
fn same_path(a: &Path, b: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .trim_end_matches(&['\\', '/'][..])
            .to_ascii_lowercase()
    };
    normalize(a) == normalize(b)
}

/// The two audio switches and the track's quality, defaulted
/// independently: the microphone stays off so a recording never surprises
/// anyone with their own voice, the system mix stays on because it is what
/// a screen recording is for, and the track keeps its full bitrate.
fn audio_settings(document: &Document, problems: &mut Vec<String>) -> audio::Wanted {
    let mic_raw = document.value("recording", "mic");
    let mic = mic_raw.and_then(Value::as_bool);
    let system_raw = document.value("recording", "system_audio");
    let system = system_raw.and_then(Value::as_bool);
    let quality_raw = document
        .value("recording", "audio_quality")
        .and_then(Value::as_str);
    let quality = quality_raw.and_then(audio::Quality::parse);
    if mic_raw.is_some() && mic.is_none() {
        problems.push("invalid mic flag; using false".to_owned());
    }
    if system_raw.is_some() && system.is_none() {
        problems.push("invalid system_audio flag; using true".to_owned());
    }
    if quality_raw.is_some() && quality.is_none() {
        // Name the accepted values: a typo is otherwise only a note that
        // the value was ignored, with no way to find the right spelling.
        let choices: Vec<&str> = audio::Quality::CHOICES
            .iter()
            .map(|choice| choice.id())
            .collect();
        problems.push(format!(
            "invalid audio_quality; using {} (choices: {})",
            audio::Quality::default().id(),
            choices.join(", ")
        ));
    }
    audio::Wanted {
        mic: mic.unwrap_or(false),
        system: system.unwrap_or(true),
        quality: quality.unwrap_or_default(),
    }
}

fn image_directory(document: &Document) -> PathBuf {
    let chosen = document
        .value("media", "images")
        .and_then(Value::as_str)
        .filter(|directory| !directory.trim().is_empty())
        .map(PathBuf::from);
    // The legacy `directory` key deliberately does NOT answer here:
    // existing configs pointed it at the recordings folder, and the user
    // asked for photos to keep their own (Pictures) default.
    match chosen {
        Some(directory) if !is_legacy_media_default(&directory) => directory,
        _ => paths::image_dir().unwrap_or_else(|| PathBuf::from(DEFAULT_DIRECTORY)),
    }
}

fn engine_path(document: &Document) -> Option<PathBuf> {
    match document.value("engine", "path").and_then(Value::as_str) {
        Some(path) if !path.trim().is_empty() => Some(PathBuf::from(path)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
