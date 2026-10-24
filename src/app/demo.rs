//! Headless capture paths behind `--record` and `--photo`: the same
//! pipelines the tray drives, used by the command line and doubles as
//! the smoke test for the whole engine stack.

use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use crate::capture::Region;
use crate::recorder::{Plan, Recorder};
use crate::screen;
#[cfg(windows)]
use crate::selector;
use crate::settings::{self, CaptureMode};

/// How often the command-line recorder checks that its capture worker is
/// still healthy while it waits out the recording's duration. Frames
/// themselves are paced by the worker.
const STEP: Duration = Duration::from_millis(10);

/// The drawn rectangle, or `None` when the user cancelled.
///
/// # Errors
/// Fails when the platform cannot present the selection overlay.
#[cfg(windows)]
fn draw_region() -> io::Result<Option<Region>> {
    match selector::draw() {
        Ok(selector::Outcome::Region(region)) => Ok(Some(region)),
        Ok(selector::Outcome::Cancelled) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(not(windows))]
fn draw_region() -> io::Result<Option<Region>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "region drawing needs the Windows overlay for now; record the full screen instead",
    ))
}

/// Record for `seconds` from the command line; doubles as the headless
/// smoke test for the whole capture pipeline (no tray, no single-instance).
/// `mode` overrides the configured capture mode when given; `display`
/// (1-based) overrides which display a Full capture records; `rect`
/// captures that exact rectangle instead of any interactive picking.
pub fn record_demo(
    config: Option<&Path>,
    seconds: u32,
    mode: Option<CaptureMode>,
    display: Option<u32>,
    rect: Option<Region>,
) -> ExitCode {
    let loaded = settings::load(config);
    for problem in &loaded.problems {
        eprintln!("warn  settings: {problem}");
    }
    let mode = mode.unwrap_or(loaded.settings.capture_mode);
    match demo_recording(&loaded, mode, seconds, display, rect) {
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
    rect: Option<Region>,
) -> io::Result<Option<std::path::PathBuf>> {
    let region = if let Some(rect) = rect {
        Ok(rect)
    } else {
        match mode {
            CaptureMode::Full => super::full_region(loaded, display),
            CaptureMode::Region => match draw_region()? {
                Some(region) => Ok(region),
                None => return Ok(None),
            },
            CaptureMode::Window => screen::window_region(None),
        }
    }?;
    let binary = super::engine(loaded)?;
    let codec = super::effective_codec(&binary, loaded);
    let plan = Plan {
        origin: (region.x, region.y),
        size: (region.width, region.height),
        profile: loaded.settings.profile,
        codec,
        cursor: loaded.settings.cursor,
        media_dir: loaded.settings.video_dir.clone(),
    };
    let recorder = Recorder::start(&binary, &plan)?;
    crate::log::note(
        "record",
        &format!(
            "{} ({} @ {} fps)",
            recorder.output.display(),
            super::size_note(region, loaded.settings.profile),
            loaded.settings.profile.fps,
        ),
    );
    // The recorder paces its own frames on its worker thread; the command
    // line only watches the clock and the worker's health.
    let budget = Duration::from_secs(u64::from(seconds));
    let started = std::time::Instant::now();
    while started.elapsed() < budget {
        if let Some(message) = recorder.failure() {
            crate::log::note("error", &format!("recording stopped: {message}"));
            let _ = recorder.stop();
            crate::log::flush_to(&loaded.settings.video_dir);
            return Err(io::Error::other(message));
        }
        std::thread::sleep(STEP);
    }
    let (written, elapsed) = recorder.stats();
    let grabbed = recorder.grabs();
    match recorder.stop() {
        Ok(path) => {
            // Same report as the tray stop path: measured rate plus the
            // probe's encoded-frame count.
            super::report_frames(&binary, &path, grabbed, written, elapsed);
            crate::log::flush_to(&loaded.settings.video_dir);
            Ok(Some(path))
        }
        Err(error) => {
            crate::log::note("error", &format!("cannot finalize: {error}"));
            crate::log::flush_to(&loaded.settings.video_dir);
            Err(error)
        }
    }
}

/// Take one still photo from the command line; smoke-tests the photo path.
/// `mode` overrides the configured capture mode when given; `display`
/// (1-based) overrides which display a Full capture photographs; `rect`
/// captures that exact rectangle instead of any picking.
pub fn photo_demo(
    config: Option<&Path>,
    mode: Option<CaptureMode>,
    display: Option<u32>,
    rect: Option<Region>,
) -> ExitCode {
    let loaded = settings::load(config);
    for problem in &loaded.problems {
        eprintln!("warn  settings: {problem}");
    }
    let mode = mode.unwrap_or(loaded.settings.capture_mode);
    // The still honors the same `[capture] mode` as a recording: Full uses
    // the display pick, Region opens the selector, Window asks the platform
    // for the window under the cursor. A rect always wins over the mode.
    let region = if let Some(rect) = rect {
        Ok(rect)
    } else {
        match mode {
            CaptureMode::Full => super::full_region(&loaded, display),
            CaptureMode::Region => match draw_region() {
                Ok(Some(region)) => Ok(region),
                Ok(None) => {
                    eprintln!("photo: selection cancelled");
                    return ExitCode::SUCCESS;
                }
                Err(error) => Err(error),
            },
            CaptureMode::Window => screen::window_region(None),
        }
    };
    let region = match region {
        Ok(region) => region,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    match super::take_photo(&loaded, region) {
        Ok(mut photo) => {
            println!("photo: {}", photo.describe());
            // This process exits right after the message, so let the
            // confirmation chime finish instead of cutting it off.
            photo.wait_for_sound();
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
