//! Tray lifecycle: platform-specific presenter behind one small API.

use crate::recording::Profile;

/// Actions the tray asks the application to perform.
#[derive(Clone, Copy, Debug)]
pub enum Event {
    /// Start or stop a recording.
    Record,
    /// Capture a still photo.
    Photo,
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
///
/// # Errors
/// Returns an error when the platform cannot present the tray.
pub fn run<F>(handler: F) -> Result<(), Error>
where
    F: FnMut(Event) + 'static,
{
    #[cfg(windows)]
    return win::run(handler);
    #[cfg(not(windows))]
    {
        let _ = handler;
        Err(Error::System(
            "tray not supported on this platform yet".into(),
        ))
    }
}

#[cfg(windows)]
mod win;
