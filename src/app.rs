//! Application runtime: one tray app per process, settings-synced.

use std::path::Path;
use std::process::ExitCode;

use crate::autostart;
use crate::config::Value;
use crate::instance::{Acquire, acquire};
use crate::recording::Profile;
use crate::settings::{self, Loaded};
use crate::tray::{self, Event};

/// Run the tray application until the user exits.
pub fn run(config: Option<&Path>) -> ExitCode {
    let mut loaded = settings::load(config);
    for problem in &loaded.problems {
        eprintln!("warn  settings: {problem}");
    }
    match acquire() {
        Ok(Acquire::Owner) => {}
        Ok(Acquire::AlreadyRunning) => {
            eprintln!("error: screencapture is already running");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("error: cannot check for a running instance: {error}");
            return ExitCode::FAILURE;
        }
    }
    if let Err(error) = sync_autostart(&mut loaded) {
        eprintln!("warn  autostart: {error}");
    }
    match tray::run(move |event| handle(&mut loaded, event)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn handle(loaded: &mut Loaded, event: Event) {
    match event {
        Event::Record => record(loaded),
        Event::Photo => photo(loaded),
        Event::ToggleAutostart => toggle_autostart(loaded),
        Event::ProfileSelected(profile) => update_profile(loaded, profile),
        Event::Exit => {}
    }
}

/// The capture pipeline lands in the next session; surface the plan today.
fn record(loaded: &Loaded) {
    let args = loaded.settings.codec.args(&loaded.settings.profile);
    eprintln!(
        "note: recording engine is next; profile {} -> .{} via {}",
        loaded.settings.profile.id,
        loaded.settings.codec.extension(),
        args.join(" ")
    );
}

/// The photo pipeline lands in the next session.
fn photo(_loaded: &Loaded) {
    eprintln!("note: photo engine is next");
}

fn toggle_autostart(loaded: &mut Loaded) {
    let enable = !autostart::registered();
    if let Err(error) = autostart::set(enable) {
        eprintln!("error: cannot change autostart: {error}");
        return;
    }
    loaded
        .document
        .set("recording", "autostart", Value::Bool(enable));
    save(loaded);
}

fn update_profile(loaded: &mut Loaded, profile: Profile) {
    loaded.settings.profile = profile;
    loaded
        .document
        .set("recording", "profile", Value::Str(profile.id.to_owned()));
    save(loaded);
}

fn save(loaded: &Loaded) {
    let Some(path) = loaded.path.as_deref() else {
        return;
    };
    if let Err(error) = loaded.document.persist(path) {
        eprintln!("error: cannot save settings: {error}");
    }
}

fn sync_autostart(loaded: &mut Loaded) -> Result<(), std::io::Error> {
    let enable = loaded.settings.autostart;
    if autostart::registered() == enable {
        return Ok(());
    }
    autostart::set(enable)
}
