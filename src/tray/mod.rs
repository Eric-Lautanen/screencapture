//! Tray lifecycle: platform-specific presenter behind one small API.

use crate::hotkeys::Binding;
use crate::recording::Profile;
use crate::settings::Hotkey;

/// Actions the tray asks the application to perform.
#[derive(Clone, Copy, Debug)]
pub enum Event {
    /// Start or stop a recording, honoring the configured capture mode.
    Record,
    /// Photograph the selection `[capture] mode` names — the same
    /// selection path "Record" uses, different output.
    Photo,
    /// Open the settings window.
    OpenSettings,
    /// Record one named display fullscreen (1-based, tray "Display N").
    RecordDisplay(u32),
    /// The recording watchdog fired; check the capture worker's health.
    Watchdog,
    /// Stop the active recording (the recording border's Stop button).
    Stop,
    /// Flip the capture chime from the tray's quick mute entry.
    ToggleChimes,
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

/// Whether Windows would accept `combo` as a global hotkey right now.
/// The app's own registrations are lifted for the probe, so a combo
/// already used by screencapture answers free; another process holding
/// it answers taken. Used by the settings window while a field records
/// the keys the user presses.
#[must_use]
pub fn hotkey_available(combo: Hotkey) -> bool {
    #[cfg(windows)]
    {
        win::hotkey_available(combo)
    }
    #[cfg(not(windows))]
    {
        let _ = combo;
        true
    }
}

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

/// Ask the tray to stop the active recording. The recording border's
/// Stop button and the recording's Escape watch call this: it posts the
/// command instead of stopping inline, so the border that owns the button
/// can be destroyed while the button's own window procedure is still on
/// the stack.
#[cfg_attr(
    not(windows),
    expect(
        dead_code,
        reason = "only the Windows recording border has a Stop chip"
    )
)]
pub fn stop_requested() {
    #[cfg(windows)]
    win::post_stop();
}

/// Show a warning balloon in the tray. A capture that fails before it
/// starts is otherwise silent: the log lives next to the videos, and a
/// border that never appears tells the user nothing.
pub fn warn(title: &str, message: &str) {
    #[cfg(windows)]
    win::warn(title, message);
    #[cfg(not(windows))]
    {
        let _ = (title, message);
    }
}

/// Keep the tray's quick mute entry in step with the settings: called at
/// startup, after a Settings save, and by the toggle itself.
pub fn set_chimes(enabled: bool) {
    #[cfg(windows)]
    win::set_chimes(enabled);
    #[cfg(not(windows))]
    {
        let _ = enabled;
    }
}

/// Enable/disable the recording watchdog that emits [`Event::Watchdog`].
pub fn set_watchdog_timer(interval_ms: Option<u32>) {
    #[cfg(windows)]
    win::set_watchdog_timer(interval_ms);
    #[cfg(not(windows))]
    {
        let _ = interval_ms;
    }
}

/// Swap the global hotkey bindings live (settings-window saves move the
/// old registrations off and register the new combos).
#[cfg_attr(
    not(windows),
    expect(
        dead_code,
        reason = "the tray lands later on this platform; the seam stays ready"
    )
)]
pub fn set_bindings(bindings: &[Binding]) {
    #[cfg(windows)]
    win::set_bindings(bindings.to_vec());
    #[cfg(not(windows))]
    {
        let _ = bindings;
    }
}

#[cfg(windows)]
mod win;

/// The full event set, assembled once at startup of the non-Windows
/// stub: nothing emits events there yet, and the catalog keeps the
/// enum constructing on every target so the seam stays complete.
#[cfg(not(windows))]
fn catalogued() -> [Event; 9] {
    [
        Event::Record,
        Event::Photo,
        Event::OpenSettings,
        Event::RecordDisplay(1),
        Event::Watchdog,
        Event::Stop,
        Event::ToggleChimes,
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
        assert_eq!(catalogued().len(), 9);
    }
}
