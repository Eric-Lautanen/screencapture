//! Settings: a typed view over the on-disk document, with safe defaults.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::{Document, Value};
use crate::encoder::Video;
use crate::paths;
use crate::recording::{self, Profile};

const SETTINGS_FILE: &str = "settings.cfg";
const DEFAULT_DIRECTORY: &str = ".";

/// Global hotkeys (tray-level shortcuts for quick actions).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hotkey {
    /// Start/stop a recording.
    Record,
    /// Take a still photo of the screen.
    Photo,
}

/// Where a recording's frame comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureMode {
    /// The whole selected display (`display`; the primary by default).
    Full,
    /// Draw a rectangle on screen first (interactive).
    Region,
    /// A window's bounds, auto-detected at record time.
    Window,
}

/// Which display a Full capture records: the primary or a numbered one
/// (1-based, matching the tray's "Display N" menu entries).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum DisplayChoice {
    /// The display at the desktop origin.
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

impl Hotkey {
    /// Parse a normalized hotkey string such as `ctrl+alt+1`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "ctrl+alt+1" => Some(Self::Record),
            "ctrl+alt+2" => Some(Self::Photo),
            _ => None,
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

impl std::fmt::Display for Hotkey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Record => write!(formatter, "ctrl+alt+1"),
            Self::Photo => write!(formatter, "ctrl+alt+2"),
        }
    }
}

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
    /// Launch at system startup.
    pub autostart: bool,
    /// Include the mouse pointer in captures.
    pub cursor: bool,
    /// Where completed files land.
    pub media_dir: PathBuf,
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
    let media = paths::media_dir().unwrap_or_else(|| PathBuf::from(DEFAULT_DIRECTORY));
    Document::from_text(&template(&media)).expect("built-in default template must parse")
}

fn template(media: &Path) -> String {
    format!(
        concat!(
            "# screencapture settings\n",
            "# Edit by hand; unknown keys are preserved and ignored.\n",
            "\n",
            "[hotkeys]\n",
            "record = ctrl+alt+1\n",
            "photo = ctrl+alt+2\n",
            "\n",
            "[capture]\n",
            "mode = full\n",
            "display = primary\n",
            "\n",
            "[recording]\n",
            "profile = uhd-4k60\n",
            "codec = x264\n",
            "cursor = false\n",
            "autostart = true\n",
            "\n",
            "[engine]\n",
            "# Empty uses the bundled engine beside the application.\n",
            "path = \n",
            "\n",
            "[media]\n",
            "directory = \"{media}\"\n",
        ),
        media = media.display()
    )
}

fn read_document(document: &Document) -> (Settings, Vec<String>) {
    let hotkey_raw = document.value("hotkeys", "record").and_then(Value::as_str);
    let hotkey = hotkey_raw.and_then(Hotkey::parse);
    let photo_raw = document.value("hotkeys", "photo").and_then(Value::as_str);
    let photo = photo_raw.and_then(Hotkey::parse);
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
    let mut problems = Vec::new();
    if hotkey_raw.is_some() && hotkey.is_none() {
        problems.push(format!("invalid record hotkey; using {}", Hotkey::Record));
    }
    if photo_raw.is_some() && photo.is_none() {
        problems.push(format!("invalid photo hotkey; using {}", Hotkey::Photo));
    }
    if mode_raw.is_some() && mode.is_none() {
        problems.push(format!("invalid capture mode; using {}", CaptureMode::Full));
    }
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
    let settings = Settings {
        profile: profile.unwrap_or(recording::DEFAULT),
        codec: codec.unwrap_or(Video::DEFAULT),
        capture_mode: mode.unwrap_or(CaptureMode::Full),
        display: display.unwrap_or(DisplayChoice::Primary),
        hotkey: hotkey.unwrap_or(Hotkey::Record),
        photo: photo.unwrap_or(Hotkey::Photo),
        autostart: document
            .value("recording", "autostart")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        cursor: document
            .value("recording", "cursor")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        media_dir: media_directory(document),
        engine: engine_path(document),
    };
    (settings, problems)
}

fn media_directory(document: &Document) -> PathBuf {
    match document.value("media", "directory").and_then(Value::as_str) {
        Some(directory) if !directory.trim().is_empty() => PathBuf::from(directory),
        _ => paths::media_dir().unwrap_or_else(|| PathBuf::from(DEFAULT_DIRECTORY)),
    }
}

fn engine_path(document: &Document) -> Option<PathBuf> {
    match document.value("engine", "path").and_then(Value::as_str) {
        Some(path) if !path.trim().is_empty() => Some(PathBuf::from(path)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureMode, DisplayChoice, Document, Hotkey, default_document, read_document};

    #[test]
    fn builtin_template_round_trips_through_defaults() {
        let document = default_document();
        let (settings, problems) = read_document(&document);
        assert_eq!(settings.profile.id, "uhd-4k60");
        assert_eq!(settings.hotkey, Hotkey::Record);
        assert_eq!(settings.photo, Hotkey::Photo);
        assert_eq!(settings.capture_mode, CaptureMode::Full);
        assert_eq!(settings.display, DisplayChoice::Primary);
        assert!(settings.autostart);
        assert!(!settings.cursor);
        assert!(settings.engine.is_none());
        assert_eq!(problems.len(), 0);
        let reparsed =
            Document::from_text(&document.to_text()).expect("rendered text must reparse");
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
    fn invalid_values_fall_back_with_problems() {
        let document = Document::from_text(
            "[hotkeys]\nrecord = super+space\n[recording]\nprofile = big\nautostart = yes-please\n",
        )
        .expect("sample must parse");
        let (settings, problems) = read_document(&document);
        assert_eq!(settings.hotkey, Hotkey::Record);
        assert_eq!(settings.profile.id, "uhd-4k60");
        assert!(settings.autostart);
        assert_eq!(problems.len(), 2);
    }
}
