//! Application runtime: one tray app per process, settings-synced, driving
//! the capture pipelines and the frame timer.

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::autostart;
use crate::capture::{Region, Target};
use crate::clock;
use crate::config::Value;
use crate::encoder::Video;
use crate::ffmpeg;
use crate::hotkeys;
use crate::instance::{Acquire, acquire};
use crate::log;
use crate::pipe::{self, Stream};
use crate::recorder::{Plan, Recorder};
use crate::recording::Profile;
use crate::screen;
#[cfg(windows)]
use crate::selector;
use crate::settings::{self, CaptureMode, DisplayChoice, Loaded};
use crate::tray::{self, Event};

pub(super) mod demo;

/// Mutable application state the tray handler drives.
struct Runtime {
    /// Engine used by the active recording, kept for ffprobe verification.
    engine: Option<ffmpeg::Binary>,
    recorder: Option<Recorder>,
    /// Media dir for the log flushes (from settings at startup).
    media_dir: PathBuf,
}

impl Runtime {
    fn new(media_dir: PathBuf) -> Self {
        Self {
            engine: None,
            recorder: None,
            media_dir,
        }
    }
}

/// Run the tray application until the user exits.
pub fn run(config: Option<&Path>) -> ExitCode {
    let mut loaded = settings::load(config);
    for problem in &loaded.problems {
        log::note("warn", &format!("settings: {problem}"));
    }
    match acquire() {
        Ok(Acquire::Owner) => {}
        Ok(Acquire::AlreadyRunning) => {
            log::note("error", "screencapture is already running");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            log::note(
                "error",
                &format!("cannot check for a running instance: {error}"),
            );
            return ExitCode::FAILURE;
        }
    }
    if let Err(error) = sync_autostart(&mut loaded) {
        log::note("warn", &format!("autostart: {error}"));
    }
    let bindings = hotkeys::bindings((loaded.settings.hotkey, loaded.settings.photo));
    let log_dir = loaded.settings.media_dir.clone();
    let mut runtime = Runtime::new(loaded.settings.media_dir.clone());
    let result = tray::run(
        move |event| handle(&mut loaded, &mut runtime, event),
        &bindings,
    );
    log::flush_to(&log_dir);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::note("error", &format!("tray loop ended: {error}"));
            ExitCode::FAILURE
        }
    }
}

fn handle(loaded: &mut Loaded, runtime: &mut Runtime, event: Event) {
    match event {
        Event::Record => record(loaded, runtime),
        Event::RecordRegion => select_and_record(loaded, runtime),
        Event::RecordWindow => begin(
            loaded,
            runtime,
            screen::window_region(tray::previous_foreground()),
        ),
        Event::RecordDisplay(number) => begin(
            loaded,
            runtime,
            screen::display_at(number).map(display_region),
        ),
        Event::Photo => photo(loaded),
        Event::Tick => tick(runtime),
        Event::ToggleAutostart => toggle_autostart(loaded),
        Event::ProfileSelected(profile) => update_profile(loaded, profile),
        Event::Exit => {
            stop_recording(runtime);
            log::flush_to(&loaded.settings.media_dir);
        }
    }
}

fn record(loaded: &mut Loaded, runtime: &mut Runtime) {
    if runtime.recorder.is_some() {
        stop_recording(runtime);
        return;
    }
    match loaded.settings.capture_mode {
        CaptureMode::Full => begin(loaded, runtime, full_region(loaded, None)),
        CaptureMode::Region => select_and_record(loaded, runtime),
        CaptureMode::Window => begin(
            loaded,
            runtime,
            screen::window_region(tray::previous_foreground()),
        ),
    }
}

/// Interactive path: draw a rectangle, then record inside it. Windows
/// runs the on-screen rubber-band overlay; elsewhere the capability is
/// reported honestly so the user can record the full screen instead.
fn select_and_record(loaded: &mut Loaded, runtime: &mut Runtime) {
    match draw_region() {
        Ok(Some(region)) => begin(loaded, runtime, Ok(region)),
        Ok(None) => log::note("record", "selection cancelled"),
        Err(error) => log::note(
            "error",
            &format!("cannot open the region selector: {error}"),
        ),
    }
}

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

/// A display's full rect as a desktop-coordinate capture region.
fn display_region(display: screen::Display) -> Region {
    Region {
        x: display.x,
        y: display.y,
        width: display.width,
        height: display.height,
    }
}
/// The configured display as a capture region (Full mode), with the
/// command line able to override the choice.
fn full_region(loaded: &Loaded, override_display: Option<u32>) -> io::Result<Region> {
    let which = override_display.map_or(loaded.settings.display, DisplayChoice::Number);
    match which.number() {
        Some(number) => screen::display_at(number),
        None => screen::primary(),
    }
    .map(display_region)
}

/// Begin encoding `region`, replacing any recording already running.
/// Unreachable regions simply report and keep the previous state.
fn begin(loaded: &mut Loaded, runtime: &mut Runtime, region: Result<Region, io::Error>) {
    let region = match region {
        Ok(region) => region,
        Err(error) => {
            log::note("error", &error.to_string());
            return;
        }
    };
    stop_recording(runtime);
    if let Err(error) = std::fs::create_dir_all(&loaded.settings.media_dir) {
        log::note("error", &format!("cannot create media directory: {error}"));
        return;
    }
    match start_recording(loaded, sane_region(region)) {
        Ok((recorder, engine)) => {
            runtime.recorder = Some(recorder);
            runtime.engine = Some(engine);
        }
        Err(error) => log::note("error", &format!("cannot start recording: {error}")),
    }
}

/// Clamp insane region extents so the engine never sees absurd frame sizes.
fn sane_region(region: Region) -> Region {
    const MAX_EDGE: u32 = 7680;
    Region {
        x: region.x,
        y: region.y,
        width: region.width.clamp(1, MAX_EDGE),
        height: region.height.clamp(1, MAX_EDGE),
    }
}

/// The effective codec: hardware choices are probed up front (here, once
/// per recording) so an unavailable accelerator demotes to x264 with a
/// notice instead of failing frame by frame mid-flight.
fn effective_codec(binary: &ffmpeg::Binary, loaded: &Loaded) -> Video {
    let requested = loaded.settings.codec;
    if requested == Video::X264 {
        return requested;
    }
    if binary.supports_encoder(requested, &loaded.settings.profile) {
        return requested;
    }
    log::note(
        "warn",
        &format!(
            "encoder: {} is not usable on this machine; recording with x264 instead",
            requested.name()
        ),
    );
    Video::X264
}

fn start_recording(loaded: &Loaded, region: Region) -> io::Result<(Recorder, ffmpeg::Binary)> {
    let binary = engine(loaded)?;
    let codec = effective_codec(&binary, loaded);
    let profile = loaded.settings.profile;
    let plan = Plan {
        origin: (region.x, region.y),
        size: (region.width, region.height),
        profile,
        codec,
        cursor: loaded.settings.cursor,
        media_dir: loaded.settings.media_dir.clone(),
    };
    let recorder = Recorder::start(&binary, &plan)?;
    tray::set_frame_timer(Some(frame_interval_ms(profile.fps)));
    log::note(
        "record",
        &format!(
            "{} ({}x{} from {},{} @ {} fps, {} to stop)",
            recorder.output.display(),
            region.width,
            region.height,
            region.x,
            region.y,
            profile.fps,
            loaded.settings.hotkey,
        ),
    );
    Ok((recorder, binary))
}

fn tick(runtime: &mut Runtime) {
    let Some(mut recorder) = runtime.recorder.take() else {
        return;
    };
    match recorder.tick() {
        Ok(()) => runtime.recorder = Some(recorder),
        Err(error) => {
            tray::set_frame_timer(None);
            log::note("error", &format!("recording stopped: {error}"));
            log::flush_to(&runtime.media_dir);
        }
    }
}

/// Stop an active recording, then verify the finished file with ffprobe.
fn stop_recording(runtime: &mut Runtime) {
    if runtime.recorder.is_none() {
        return;
    }
    let recorder = runtime.recorder.take();
    let Some(recorder) = recorder else {
        return;
    };
    tray::set_frame_timer(None);
    match recorder.stop() {
        Ok(path) => {
            if let Some(engine) = runtime.engine.take() {
                report_frames(&engine, &path);
            } else {
                log::note("record", &format!("saved {}", path.display()));
            }
        }
        Err(error) => log::note("error", &format!("cannot finalize recording: {error}")),
    }
    log::flush_to(&runtime.media_dir);
}

fn report_frames(engine: &ffmpeg::Binary, path: &Path) {
    let probe = engine.probe();
    match probe.frames(path) {
        Ok(frames) => log::note(
            "record",
            &format!("saved {} ({frames} frames)", path.display()),
        ),
        Err(error) => log::note(
            "warn",
            &format!(
                "record: saved {} (probe check failed: {error})",
                path.display()
            ),
        ),
    }
}

fn photo(loaded: &Loaded) {
    match take_photo(loaded, None) {
        Ok(path) => log::note("photo", &path.display().to_string()),
        Err(error) => log::note("error", &error.to_string()),
    }
}

fn take_photo(loaded: &Loaded, override_display: Option<u32>) -> io::Result<std::path::PathBuf> {
    let binary = engine(loaded)?;
    std::fs::create_dir_all(&loaded.settings.media_dir)?;
    let display = match override_display
        .map_or(loaded.settings.display, DisplayChoice::Number)
        .number()
    {
        Some(number) => screen::display_at(number)?,
        None => screen::primary()?,
    };
    let region = Region {
        x: display.x,
        y: display.y,
        width: display.width,
        height: display.height,
    };
    let target = Target::new(region, loaded.settings.cursor)?;
    let frame = target.frame()?;
    let path = photo_path(&loaded.settings.media_dir);
    let stream = Stream {
        width: display.width,
        height: display.height,
        fps: 1,
    };
    let mut feed = pipe::Feed::spawn(&binary, &pipe::still_args(stream, &path))?;
    feed.frame(frame)?;
    feed.finish()?;
    Ok(path)
}

/// Fresh stamped photo path; appends `-2`, `-3`, ... on same-second clashes.
fn photo_path(media_dir: &Path) -> std::path::PathBuf {
    let stem = format!("screencapture-{}", clock::stamp());
    let mut path = media_dir.join(format!("{stem}.png"));
    let mut counter = 1;
    while path.exists() {
        counter += 1;
        path = media_dir.join(format!("{stem}-{counter}.png"));
    }
    path
}

fn engine(loaded: &Loaded) -> io::Result<ffmpeg::Binary> {
    ffmpeg::discover(loaded.settings.engine.as_deref())
        .ok_or_else(|| io::Error::other("FFmpeg engine not found; run `screencapture --check`"))
}

fn frame_interval_ms(fps: u32) -> u32 {
    (1_000 / fps.max(1)).max(1)
}

fn toggle_autostart(loaded: &mut Loaded) {
    let enable = !autostart::registered();
    if let Err(error) = autostart::set(enable) {
        log::note("error", &format!("cannot change autostart: {error}"));
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
        log::note("error", &format!("cannot save settings: {error}"));
    }
}

fn sync_autostart(loaded: &mut Loaded) -> Result<(), std::io::Error> {
    let enable = loaded.settings.autostart;
    if autostart::registered() == enable {
        return Ok(());
    }
    autostart::set(enable)
}
