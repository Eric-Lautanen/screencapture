//! Tray lifecycle: platform-specific presenter behind one small API.

use crate::recording::Profile;

/// Actions the tray asks the application to perform.
#[derive(Clone, Copy, Debug)]
pub enum Event {
    /// Start or stop a recording, honoring the configured capture mode.
    Record,
    /// Start a recording from a rectangle the user draws on screen.
    RecordRegion,
    /// Start a recording of the application the user was just working in.
    RecordWindow,
    /// Capture a still photo.
    Photo,
    /// The frame timer fired; capture and encode one frame.
    Tick,
    /// Flip login autostart on or off in OS and settings.
    ToggleAutostart,
    /// Switch the active capture profile.
    ProfileSelected(Profile),
    /// Quit the process cleanly.
    Exit,
}

/// Launch failure modes for the tray loop.
#[derive(Debug)]
pub enum Error {
    /// A platform API refused a call.
    System(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System(message) => write!(formatter, "tray system error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// Run the tray loop; returns when the user picks exit or the loop fails.
/// `bindings` are registered as global hotkeys for the loop lifetime.
///
/// # Errors
/// Returns an error when the platform cannot present the tray.
pub fn run<F>(handler: F, bindings: &[crate::hotkeys::Binding]) -> Result<(), Error>
where
    F: FnMut(Event) + 'static,
{
    #[cfg(windows)]
    return win::run(handler, bindings);
    #[cfg(not(windows))]
    {
        let _ = catalogued();
        let _ = (handler, bindings);
        Err(Error::System(
            "the tray lands later on this platform; the CLI works today (screencapture --record, --photo)".into(),
        ))
    }
}

/// Enable/disable the frame timer that emits [`Event::Tick`].
pub fn set_frame_timer(interval_ms: Option<u32>) {
    #[cfg(windows)]
    win::set_frame_timer(interval_ms);
    #[cfg(not(windows))]
    {
        let _ = interval_ms;
    }
}

/// The window handle the user had focused just before the tray menu
/// opened, as a plain integer (Windows handle bits). "Record window"
/// targets that application.
#[must_use]
pub fn previous_foreground() -> Option<usize> {
    #[cfg(windows)]
    return win::previous_foreground();
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
mod win;

/// The full event set, assembled once at startup of the non-Windows
/// stub: nothing emits events there yet, and the catalog keeps the
/// enum constructing on every target so the seam stays complete.
#[cfg(not(windows))]
fn catalogued() -> [Event; 8] {
    [
        Event::Record,
        Event::RecordRegion,
        Event::RecordWindow,
        Event::Photo,
        Event::Tick,
        Event::ToggleAutostart,
        Event::ProfileSelected(Profile::PROFILES[0]),
        Event::Exit,
    ]
}

#[cfg(all(target_os = "linux", test))]
mod parity {
    use super::catalogued;

    /// The catalog stays assembled on the Linux target (compiles on the
    /// mac target too; both keep the enum alive).
    #[test]
    fn events_construct_on_every_platform() {
        assert_eq!(catalogued().len(), 8);
    }
}
