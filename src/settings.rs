//! Settings: a typed view over the on-disk document, with safe defaults.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::{Document, Value};
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
            "[recording]\n",
            "profile = uhd-4k60\n",
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
    let profile_raw = document
        .value("recording", "profile")
        .and_then(Value::as_str);
    let profile = profile_raw.and_then(Profile::by_id);
    let mut problems = Vec::new();
    if hotkey_raw.is_some() && hotkey.is_none() {
        problems.push(format!("invalid record hotkey; using {}", Hotkey::Record));
    }
    if photo_raw.is_some() && photo.is_none() {
        problems.push(format!("invalid photo hotkey; using {}", Hotkey::Photo));
    }
    if profile_raw.is_some() && profile.is_none() {
        problems.push(format!("invalid profile; using {}", recording::DEFAULT.id));
    }
    let settings = Settings {
        profile: profile.unwrap_or(recording::DEFAULT),
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
    use super::{Document, Hotkey, default_document, read_document};

    #[test]
    fn builtin_template_round_trips_through_defaults() {
        let document = default_document();
        let (settings, problems) = read_document(&document);
        assert_eq!(settings.profile.id, "uhd-4k60");
        assert_eq!(settings.hotkey, Hotkey::Record);
        assert_eq!(settings.photo, Hotkey::Photo);
        assert!(settings.autostart);
        assert!(!settings.cursor);
        assert!(settings.engine.is_none());
        assert_eq!(problems.len(), 0);
        let reparsed =
            Document::from_text(&document.to_text()).expect("rendered text must reparse");
        assert_eq!(document, reparsed);
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
