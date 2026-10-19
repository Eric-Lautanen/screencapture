//! Headless capture paths behind `--record` and `--photo`: the same
//! pipelines the tray drives, used by the command line and doubles as
//! the smoke test for the whole engine stack.

use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use crate::recorder::{Plan, Recorder};
use crate::screen;
use crate::settings::{self, CaptureMode};

/// Fine step for the deadline sleep, so `--record` timing is not ground
/// under the coarse OS sleep quantum.
const STEP: Duration = Duration::from_millis(1);

/// Record for `seconds` from the command line; doubles as the headless
/// smoke test for the whole capture pipeline (no tray, no single-instance).
/// `mode` overrides the configured capture mode when given; `display`
/// (1-based) overrides which display a Full capture records.
pub fn record_demo(
    config: Option<&Path>,
    seconds: u32,
    mode: Option<CaptureMode>,
    display: Option<u32>,
) -> ExitCode {
    let loaded = settings::load(config);
    for problem in &loaded.problems {
        eprintln!("warn  settings: {problem}");
    }
    let mode = mode.unwrap_or(loaded.settings.capture_mode);
    match demo_recording(&loaded, mode, seconds, display) {
        Ok(Some(path)) => {
            println!("recorded: {}", path.display());
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!("record: selection cancelled");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// One timed recording against the current (or overridden) capture mode.
/// `Ok(None)` means the interactive selection was cancelled.
///
/// # Errors
/// Propagates capture/encoding failures.
fn demo_recording(
    loaded: &settings::Loaded,
    mode: CaptureMode,
    seconds: u32,
    display: Option<u32>,
) -> io::Result<Option<std::path::PathBuf>> {
    let region = match mode {
        CaptureMode::Full => super::full_region(loaded, display),
        CaptureMode::Region => match super::draw_region()? {
            Some(region) => Ok(region),
            None => return Ok(None),
        },
        CaptureMode::Window => screen::window_region(None),
    }?;
    let binary = super::engine(loaded)?;
    let codec = super::effective_codec(&binary, loaded);
    let plan = Plan {
        origin: (region.x, region.y),
        size: (region.width, region.height),
        profile: loaded.settings.profile,
        codec,
        cursor: loaded.settings.cursor,
        media_dir: loaded.settings.media_dir.clone(),
    };
    let mut recorder = Recorder::start(&binary, &plan)?;
    crate::log::note(
        "record",
        &format!(
            "{} ({}x{} @ {} fps)",
            recorder.output.display(),
            region.width,
            region.height,
            loaded.settings.profile.fps,
        ),
    );
    let budget = Duration::from_secs(u64::from(seconds));
    let interval = Duration::from_millis(u64::from(super::frame_interval_ms(
        loaded.settings.profile.fps,
    )));
    let started = std::time::Instant::now();
    let mut deadline = started;
    while started.elapsed() < budget {
        recorder.tick()?;
        deadline += interval;
        while deadline > std::time::Instant::now() {
            std::thread::sleep(STEP);
        }
    }
    match recorder.stop() {
        Ok(path) => {
            crate::log::note("record", &format!("saved {}", path.display()));
            crate::log::flush_to(&loaded.settings.media_dir);
            Ok(Some(path))
        }
        Err(error) => {
            crate::log::note("error", &format!("cannot finalize: {error}"));
            crate::log::flush_to(&loaded.settings.media_dir);
            Err(error)
        }
    }
}

/// Take one still photo from the command line; smoke-tests the photo path.
/// `display` (1-based) overrides which display is photographed.
pub fn photo_demo(config: Option<&Path>, display: Option<u32>) -> ExitCode {
    let loaded = settings::load(config);
    for problem in &loaded.problems {
        eprintln!("warn  settings: {problem}");
    }
    match super::take_photo(&loaded, display) {
        Ok(path) => {
            println!("photo: {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
