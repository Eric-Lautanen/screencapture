//! Application runtime: one tray app per process, settings-synced, driving
//! the capture pipelines and the frame timer.

use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use crate::autostart;
use crate::capture::{Region, Target};
use crate::clock;
use crate::config::Value;
use crate::ffmpeg;
use crate::hotkeys;
use crate::instance::{Acquire, acquire};
use crate::pipe::{self, Stream};
use crate::recorder::{Plan, Recorder};
use crate::recording::Profile;
use crate::screen;
#[cfg(windows)]
use crate::selector;
use crate::settings::{self, CaptureMode, DisplayChoice, Loaded};
use crate::tray::{self, Event};

/// Mutable application state the tray handler drives.
struct Runtime {
    /// Engine used by the active recording, kept for ffprobe verification.
    engine: Option<ffmpeg::Binary>,
    recorder: Option<Recorder>,
}

impl Runtime {
    fn new() -> Self {
        Self {
            engine: None,
            recorder: None,
        }
    }
}

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
    let bindings = hotkeys::bindings((loaded.settings.hotkey, loaded.settings.photo));
    let mut runtime = Runtime::new();
    let result = tray::run(
        move |event| handle(&mut loaded, &mut runtime, event),
        &bindings,
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
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
        Event::Exit => stop_recording(runtime),
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
        Ok(None) => eprintln!("record: selection cancelled"),
        Err(error) => eprintln!("error: cannot open the region selector: {error}"),
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
            eprintln!("error: {error}");
            return;
        }
    };
    stop_recording(runtime);
    if let Err(error) = std::fs::create_dir_all(&loaded.settings.media_dir) {
        eprintln!("error: cannot create media directory: {error}");
        return;
    }
    match start_recording(loaded, sane_region(region)) {
        Ok((recorder, engine)) => {
            runtime.recorder = Some(recorder);
            runtime.engine = Some(engine);
        }
        Err(error) => eprintln!("error: cannot start recording: {error}"),
    }
}

/// Clamp insane region extents so the engine never sees absurd frame sizes.
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

fn start_recording(loaded: &Loaded, region: Region) -> io::Result<(Recorder, ffmpeg::Binary)> {
    let binary = engine(loaded)?;
    let profile = loaded.settings.profile;
    let plan = Plan {
        origin: (region.x, region.y),
        size: (region.width, region.height),
        profile,
        codec: loaded.settings.codec,
        cursor: loaded.settings.cursor,
        media_dir: loaded.settings.media_dir.clone(),
    };
    let recorder = Recorder::start(&binary, &plan)?;
    tray::set_frame_timer(Some(frame_interval_ms(profile.fps)));
    eprintln!(
        "record: {} ({}x{} from {},{} @ {} fps, {} to stop)",
        recorder.output.display(),
        region.width,
        region.height,
        region.x,
        region.y,
        profile.fps,
        loaded.settings.hotkey,
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
            eprintln!("error: recording stopped: {error}");
        }
    }
}

/// Stop an active recording, then verify the finished file with ffprobe.
fn stop_recording(runtime: &mut Runtime) {
    let Some(recorder) = runtime.recorder.take() else {
        return;
    };
    tray::set_frame_timer(None);
    match recorder.stop() {
        Ok(path) => report_frames(runtime, &path),
        Err(error) => eprintln!("error: cannot finalize recording: {error}"),
    }
}

fn report_frames(runtime: &mut Runtime, path: &Path) {
    let probe = runtime.engine.as_ref().map(ffmpeg::Binary::probe);
    match probe {
        Some(probe) => match probe.frames(path) {
            Ok(frames) => eprintln!("record: saved {} ({frames} frames)", path.display()),
            Err(error) => {
                eprintln!(
                    "record: saved {} (probe check failed: {error})",
                    path.display()
                );
            }
        },
        None => eprintln!("record: saved {}", path.display()),
    }
}

fn photo(loaded: &Loaded) {
    match take_photo(loaded, None) {
        Ok(path) => eprintln!("photo: {}", path.display()),
        Err(error) => eprintln!("error: {error}"),
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
fn demo_recording(
    loaded: &Loaded,
    mode: CaptureMode,
    seconds: u32,
    display: Option<u32>,
) -> io::Result<Option<std::path::PathBuf>> {
    let region = match mode {
        CaptureMode::Full => full_region(loaded, display),
        CaptureMode::Region => match draw_region()? {
            Some(region) => Ok(region),
            None => return Ok(None),
        },
        CaptureMode::Window => screen::window_region(None),
    }?;
    let binary = engine(loaded)?;
    let plan = Plan {
        origin: (region.x, region.y),
        size: (region.width, region.height),
        profile: loaded.settings.profile,
        codec: loaded.settings.codec,
        cursor: loaded.settings.cursor,
        media_dir: loaded.settings.media_dir.clone(),
    };
    let mut recorder = Recorder::start(&binary, &plan)?;
    let budget = Duration::from_secs(u64::from(seconds));
    let interval = Duration::from_millis(u64::from(frame_interval_ms(loaded.settings.profile.fps)));
    let started = std::time::Instant::now();
    let mut deadline = started;
    while started.elapsed() < budget {
        recorder.tick()?;
        deadline += interval;
        // Sleep in fine steps so the deadline is not ground under the
        // coarse OS sleep quantum.
        while deadline > std::time::Instant::now() {
            std::thread::sleep(STEP);
        }
    }
    recorder.stop().map(Some)
}

const STEP: Duration = Duration::from_millis(1);

/// Take one still photo from the command line; smoke-tests the photo path.
/// `display` (1-based) overrides which display is photographed.
pub fn photo_demo(config: Option<&Path>, display: Option<u32>) -> ExitCode {
    let loaded = settings::load(config);
    for problem in &loaded.problems {
        eprintln!("warn  settings: {problem}");
    }
    match take_photo(&loaded, display) {
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
