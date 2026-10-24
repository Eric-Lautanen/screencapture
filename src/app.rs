//! Application runtime: one tray app per process, settings-synced, driving
//! the capture pipelines and the frame timer.

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use crate::autostart;
use crate::capture::Region;
use crate::config::Value;
use crate::encoder::Video;
use crate::ffmpeg;
#[cfg(windows)]
use crate::guard::EscapeWatch;
use crate::hotkeys;
use crate::instance::{Acquire, acquire};
use crate::log;
#[cfg(windows)]
use crate::overlay;
use crate::recorder::{Plan, Recorder};
use crate::recording::Profile;
use crate::screen;
use crate::settings::{self, CaptureMode, DisplayChoice, Loaded};
use crate::tray::{self, Event};

pub(super) mod demo;
mod photo;
mod settings_apply;

use photo::take_photo;

/// How often the pump checks that the capture worker is still alive (ms).
const WATCHDOG_MS: u32 = 500;

/// A recording that was asked to stop and whose worker is still closing
/// its file. It outlives the stop so the Stop chip, the hotkey, and the
/// tray all answer at once instead of waiting on the engine.
struct Finalizing {
    /// The stopped recorder, waiting for its worker to publish a result.
    recorder: Recorder,
    /// Engine kept for the post-capture ffprobe check.
    engine: Option<ffmpeg::Binary>,
    /// Frames grabbed when the stop was requested.
    grabbed: u64,
    /// Frames written when the stop was requested.
    written: u64,
    /// Wall-clock recording time when the stop was requested.
    elapsed: Duration,
}

/// Mutable application state the tray handler drives.
struct Runtime {
    /// Engine used by the active recording, kept for ffprobe verification.
    engine: Option<ffmpeg::Binary>,
    recorder: Option<Recorder>,
    /// Stops whose workers are still finalizing, oldest first.
    finalizing: Vec<Finalizing>,
    /// Media dir for the log flushes (from settings at startup).
    media_dir: PathBuf,
    /// The red dashed outline shown around the active recording.
    #[cfg(windows)]
    border: Option<overlay::Border>,
    /// Escape-as-stop watch for the live recording; dropping it hands the
    /// key back to the foreground application.
    #[cfg(windows)]
    escape: Option<EscapeWatch>,
}

impl Runtime {
    fn new(media_dir: PathBuf) -> Self {
        Self {
            engine: None,
            recorder: None,
            finalizing: Vec::new(),
            media_dir,
            #[cfg(windows)]
            border: None,
            #[cfg(windows)]
            escape: None,
        }
    }
}

/// Run the tray application until the user exits.
pub fn run(config: Option<&Path>) -> ExitCode {
    if !cfg!(windows) {
        return crate::cli::help_exit();
    }
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
    let bindings = hotkeys::bindings(
        loaded.settings.hotkey,
        loaded.settings.photo,
        loaded.settings.chime_hotkey,
    );
    tray::set_chimes(loaded.settings.chimes);
    warm_codec(&loaded);
    let log_dir = loaded.settings.video_dir.clone();
    let mut runtime = Runtime::new(log_dir.clone());
    let result = tray::run(
        move |event| handle(&mut loaded, &mut runtime, event),
        &bindings,
    );
    log::flush_to(&log_dir);
    if let Err(error) = result {
        log::note("error", &format!("tray loop ended: {error}"));
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Open the settings window outside the tray loop (`--settings`).
#[cfg(windows)]
pub fn open_settings_window(config: Option<&Path>) -> ExitCode {
    let mut loaded = settings::load(config);
    for problem in &loaded.problems {
        log::note("warn", &format!("settings: {problem}"));
    }
    let mut runtime = Runtime::new(loaded.settings.video_dir.clone());
    settings_apply::open_settings(&mut loaded, &mut runtime);
    log::flush_to(&loaded.settings.video_dir);
    ExitCode::SUCCESS
}

/// The settings window is a Windows surface; elsewhere edit the file.
#[cfg(not(windows))]
pub fn open_settings_window(_config: Option<&Path>) -> ExitCode {
    eprintln!("error: the settings window needs Windows; edit the settings file instead");
    ExitCode::FAILURE
}

/// Tray events: record and photo share one `select_region` answer, the
/// other sources feed the recorder, autostart/profile mutate settings.
fn handle(loaded: &mut Loaded, runtime: &mut Runtime, event: Event) {
    match event {
        Event::Record => record(loaded, runtime),
        Event::Photo => photograph(loaded, runtime),
        Event::OpenSettings => settings_apply::open_settings(loaded, runtime),
        Event::RecordDisplay(number) => begin(
            loaded,
            runtime,
            screen::display_at(number).map(display_region),
        ),
        Event::Watchdog => tick(runtime),
        Event::Stop => stop_from_user(loaded, runtime),
        Event::ToggleChimes => toggle_chimes(loaded),
        Event::ProfileSelected(profile) => update_profile(loaded, profile),
        Event::Exit => {
            // The tray is going away, so this is the one place that waits
            // for the engines: the files have to be closed before exit.
            stop_recording(runtime);
            drain_blocking(runtime);
            tray::set_watchdog_timer(None);
            log::flush_to(&loaded.settings.video_dir);
        }
    }
}

/// Shared selection path: `[capture] mode` names full display or interactive
/// pick/drag. Interactive sessions stop a running recording first.
fn select_region(loaded: &mut Loaded, runtime: &mut Runtime) -> Option<io::Result<Region>> {
    match loaded.settings.capture_mode {
        CaptureMode::Full => Some(full_region(loaded, None)),
        CaptureMode::Region | CaptureMode::Window => {
            stop_recording(runtime);
            pick_region()
        }
    }
}

/// The interactive session (Windows): one crosshair pass answers a
/// clicked window or a dragged rectangle; cancel answers None.
#[cfg(windows)]
fn pick_region() -> Option<io::Result<Region>> {
    match crate::window_picker::pick() {
        Ok(Some(crate::window_picker::Pick::Window(handle))) => {
            Some(screen::window_region(Some(handle)))
        }
        Ok(Some(crate::window_picker::Pick::Region(region))) => Some(Ok(region)),
        Ok(None) => {
            log::note("record", "selection cancelled");
            None
        }
        Err(error) => Some(Err(io::Error::other(format!(
            "cannot pick a capture target: {error}"
        )))),
    }
}

/// Non-Windows fallback: the capture backend's cursor answer drives the region.
#[cfg(not(windows))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the Windows variant cancels; this one only answers, and they share a signature"
)]
fn pick_region() -> Option<io::Result<Region>> {
    Some(screen::window_region(None))
}

/// Record the shared selection; a running recording stops first, so
/// the hotkey doubles as the stop button.
fn record(loaded: &mut Loaded, runtime: &mut Runtime) {
    if runtime.recorder.is_some() {
        stop_from_user(loaded, runtime);
        return;
    }
    if let Some(result) = select_region(loaded, runtime) {
        begin(loaded, runtime, result);
    }
}

/// Photograph the shared selection into the images folder.
fn photograph(loaded: &mut Loaded, runtime: &mut Runtime) {
    let Some(result) = select_region(loaded, runtime) else {
        return;
    };
    match result {
        Ok(region) => match take_photo(loaded, region) {
            Ok(photo) => log::note("photo", &photo.describe()),
            Err(error) => log::note("error", &format!("cannot take a photo: {error}")),
        },
        Err(error) => log::note("error", &error.to_string()),
    }
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

/// The configured display as a capture region (Full mode).
fn full_region(loaded: &Loaded, override_display: Option<u32>) -> io::Result<Region> {
    let which = override_display.map_or(loaded.settings.display, DisplayChoice::Number);
    which
        .number()
        .map_or_else(screen::primary, screen::display_at)
        .map(display_region)
}

/// Begin encoding `region`, replacing any recording already running.
fn begin(loaded: &mut Loaded, runtime: &mut Runtime, region: Result<Region, io::Error>) {
    let Ok(region) = region.map_err(|error| log::note("error", &error.to_string())) else {
        return;
    };
    stop_recording(runtime);
    if let Err(error) = std::fs::create_dir_all(&loaded.settings.video_dir) {
        log::note("error", &format!("cannot create video directory: {error}"));
        return;
    }
    let region = sane_region(region);
    #[cfg(windows)]
    let border = show_border(region);
    match start_recording(loaded, region) {
        Ok((recorder, engine)) => {
            runtime.recorder = Some(recorder);
            runtime.engine = Some(engine);
            #[cfg(windows)]
            {
                runtime.border = border;
                runtime.escape = watch_escape();
            }
        }
        Err(error) => {
            #[cfg(windows)]
            drop(border);
            log::note("error", &format!("cannot start recording: {error}"));
        }
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

/// Probe the configured encoder and fall back in a fixed order: x264,
/// then each hardware H.264 unit, and only then CPU x265. H.264 is the
/// primary output on every machine, so a missing accelerator can never
/// silently turn a recording into HEVC. Probing happens before the worker
/// starts, so an unusable engine is an honest error, not a dead stream.
fn effective_codec(binary: &ffmpeg::Binary, loaded: &Loaded) -> io::Result<Video> {
    let requested = loaded.settings.codec;
    let profile = &loaded.settings.profile;
    let codec = requested
        .choose(|video| binary.supports_encoder_cached(video, profile))
        .ok_or_else(|| {
            io::Error::other(format!(
                "no usable video encoder (requested {}); run `screencapture --check`",
                requested.name()
            ))
        })?;
    if codec != requested {
        log::note(
            "warn",
            &format!(
                "encoder: {} is not usable on this machine; recording with {} instead",
                requested.name(),
                codec.name()
            ),
        );
    }
    Ok(codec)
}

fn start_recording(loaded: &Loaded, region: Region) -> io::Result<(Recorder, ffmpeg::Binary)> {
    let binary = engine(loaded)?;
    let codec = effective_codec(&binary, loaded)?;
    let profile = loaded.settings.profile;
    let plan = Plan {
        origin: (region.x, region.y),
        size: (region.width, region.height),
        profile,
        codec,
        cursor: loaded.settings.cursor,
        media_dir: loaded.settings.video_dir.clone(),
    };
    let recorder = Recorder::start(&binary, &plan)?;
    tray::set_watchdog_timer(Some(WATCHDOG_MS));
    log::note(
        "record",
        &format!(
            "{} ({} from {},{} @ {} fps, {} to stop)",
            recorder.output.display(),
            size_note(region, profile),
            region.x,
            region.y,
            profile.fps,
            loaded.settings.hotkey,
        ),
    );
    Ok((recorder, binary))
}

/// Capture size, plus output size when the profile rescales it.
fn size_note(region: Region, profile: Profile) -> String {
    let output = profile.fit((region.width, region.height));
    if output == (region.width, region.height) {
        format!("{}x{}", region.width, region.height)
    } else {
        format!(
            "{}x{} -> {}x{}",
            region.width, region.height, output.0, output.1
        )
    }
}

/// Show the recording outline, or `None` after an honest log line.
#[cfg(windows)]
fn show_border(region: Region) -> Option<overlay::Border> {
    match overlay::Border::show(region, overlay::Style::Recording(tray::stop_requested)) {
        Ok(border) => Some(border),
        Err(error) => {
            log::note("warn", &format!("recording border unavailable: {error}"));
            None
        }
    }
}

/// Recording watchdog: report a dead worker, then collect finished stops.
fn tick(runtime: &mut Runtime) {
    if let Some(message) = runtime.recorder.as_ref().and_then(Recorder::failure) {
        log::note("error", &format!("recording stopped: {message}"));
        stop_recording(runtime);
    }
    drain_finalizing(runtime);
}

/// A stop the user asked for — the Stop chip, Escape, or the record
/// hotkey: answer with the same soft chime as a still at once, then take
/// the capture down without waiting on the encoder. Automatic stops
/// (watchdog failure, settings, exit) stay silent.
fn stop_from_user(loaded: &Loaded, runtime: &mut Runtime) {
    if runtime.recorder.is_some() {
        drop(crate::sound::capture_chime(loaded.settings.chimes));
    }
    stop_recording(runtime);
}

/// Arm Escape as a stop key for as long as the recording runs.
#[cfg(windows)]
fn watch_escape() -> Option<EscapeWatch> {
    // SAFETY: null resolves this executable; the tray pump on this thread
    // delivers the hook until `stop_recording` drops the watch.
    let instance = unsafe { crate::winapi::GetModuleHandleW(std::ptr::null()) };
    // SAFETY: install contract above; the action only posts a tray command.
    let watch = unsafe { EscapeWatch::install(tray::stop_requested, instance) };
    if watch.is_none() {
        log::note(
            "warn",
            "Escape cannot stop this recording; use the Stop chip or the hotkey",
        );
    }
    watch
}

/// Ask the active recording to stop: the outline and the capture go away
/// at once, while the worker closes its file in the background. The
/// watchdog collects the finished stop (or [`drain_blocking`] on exit).
fn stop_recording(runtime: &mut Runtime) {
    // The Escape watch belongs to the live recording, so it goes first: a
    // press during teardown must not queue a second stop.
    #[cfg(windows)]
    {
        runtime.escape = None;
    }
    let Some(recorder) = runtime.recorder.take() else {
        return;
    };
    #[cfg(windows)]
    {
        runtime.border = None;
    }
    let (written, elapsed) = recorder.stats();
    let grabbed = recorder.grabs();
    recorder.request_stop();
    runtime.finalizing.push(Finalizing {
        recorder,
        engine: runtime.engine.take(),
        grabbed,
        written,
        elapsed,
    });
    // The stop itself must not wait on the engine, so the timer keeps the
    // pump asking until the file is closed.
    tray::set_watchdog_timer(Some(WATCHDOG_MS));
}

/// Report every stop whose worker has left its loop.
fn drain_finalizing(runtime: &mut Runtime) {
    let mut remaining = Vec::new();
    for job in std::mem::take(&mut runtime.finalizing) {
        if job.recorder.finished() {
            finish_job(&runtime.media_dir, job);
        } else {
            remaining.push(job);
        }
    }
    runtime.finalizing = remaining;
    if runtime.finalizing.is_empty() && runtime.recorder.is_none() {
        tray::set_watchdog_timer(None);
    }
}

/// Close every pending file before the process leaves; only the exit path
/// waits on an engine.
fn drain_blocking(runtime: &mut Runtime) {
    for job in std::mem::take(&mut runtime.finalizing) {
        finish_job(&runtime.media_dir, job);
    }
}

/// Join one worker, report what it wrote, and flush the diagnostics.
fn finish_job(media_dir: &Path, job: Finalizing) {
    let Finalizing {
        recorder,
        engine,
        grabbed,
        written,
        elapsed,
    } = job;
    let secs = elapsed.as_secs_f64();
    match recorder.stop() {
        Ok(path) => match engine {
            Some(engine) => report_frames(&engine, &path, grabbed, written, elapsed),
            None => log::note(
                "record",
                &format!("saved {} ({written} frames in {secs:.2} s)", path.display()),
            ),
        },
        Err(error) => log::note(
            "error",
            &format!("cannot finalize recording after {written} frames in {secs:.2} s: {error}"),
        ),
    }
    log::flush_to(media_dir);
}

/// Report the file beside what the capture actually managed.
fn report_frames(
    engine: &ffmpeg::Binary,
    path: &Path,
    grabbed: u64,
    written: u64,
    elapsed: Duration,
) {
    let seconds = elapsed.as_secs_f64();
    let rate = |frames: u64| {
        let counted = u32::try_from(frames).unwrap_or(u32::MAX);
        if seconds > 0.0 {
            f64::from(counted) / seconds
        } else {
            0.0
        }
    };
    let detail = if grabbed == written {
        format!(
            "{written} frames in {seconds:.2} s, {:.1} fps measured",
            rate(written)
        )
    } else {
        format!(
            "{written} frames from {grabbed} grabs in {seconds:.2} s, {:.1} fps output, {:.1} fps captured",
            rate(written),
            rate(grabbed)
        )
    };
    match engine.probe().frames(path) {
        Ok(encoded) => log::note(
            "record",
            &format!("saved {} ({detail}; {encoded} encoded)", path.display()),
        ),
        Err(error) => log::note(
            "warn",
            &format!(
                "record: saved {} ({detail}; probe check failed: {error})",
                path.display()
            ),
        ),
    }
}

fn engine(loaded: &Loaded) -> io::Result<ffmpeg::Binary> {
    ffmpeg::discover(loaded.settings.engine.as_deref())
        .ok_or_else(|| io::Error::other("FFmpeg engine not found; run `screencapture --check`"))
}

/// Probe the configured encoder while the tray idles. When it is not
/// x264, probe x264 too: it is the first fallback, so a later demotion
/// does not pay for a cold synthetic encode between click and recording.
fn warm_codec(loaded: &Loaded) {
    let requested = loaded.settings.codec;
    if let Ok(binary) = engine(loaded) {
        let profile = loaded.settings.profile;
        std::thread::spawn(move || {
            let _ = binary.supports_encoder_cached(requested, &profile);
            if requested != Video::X264 {
                let _ = binary.supports_encoder_cached(Video::X264, &profile);
            }
        });
    }
}

/// Flip the capture chime from the tray menu, save it, and keep the menu's
/// check mark in step. A failed save leaves this session with the new
/// state and logs that the file still has the old one.
fn toggle_chimes(loaded: &mut Loaded) {
    let enabled = !loaded.settings.chimes;
    loaded.settings.chimes = enabled;
    loaded
        .document
        .set("capture", "chimes", Value::Bool(enabled));
    if let Some(path) = loaded.path.as_deref() {
        if let Err(error) = loaded.document.persist(path) {
            log::note("error", &format!("cannot save settings: {error}"));
        }
    } else {
        log::note(
            "warn",
            "capture chimes changed for this session only (no settings file)",
        );
    }
    tray::set_chimes(enabled);
    log::note(
        "settings",
        if enabled {
            "capture chimes on"
        } else {
            "capture chimes muted"
        },
    );
}

fn update_profile(loaded: &mut Loaded, profile: Profile) {
    loaded.settings.profile = profile;
    loaded
        .document
        .set("recording", "profile", Value::Str(profile.id.to_owned()));
    if let Some(path) = loaded.path.as_deref()
        && let Err(error) = loaded.document.persist(path)
    {
        log::note("error", &format!("cannot save settings: {error}"));
    }
}

fn sync_autostart(loaded: &mut Loaded) -> Result<(), std::io::Error> {
    let enable = loaded.settings.autostart;
    // Enabling is a no-op only when the entry already launches *this*
    // executable; disabling always runs, so a registration left behind by
    // another install is removed rather than kept alive by the flag alone.
    if enable && autostart::registered() {
        return Ok(());
    }
    autostart::set(enable)
}

#[cfg(test)]
mod tests {
    use super::{Finalizing, Runtime, drain_finalizing, size_note, toggle_chimes};
    use crate::capture::Region;
    use crate::recorder::Recorder;
    use crate::recording::Profile;
    use std::time::Duration;

    /// A stop whose worker already finished is collected on the next
    /// watchdog tick, without the stop path having waited on it.
    #[test]
    fn a_finished_stop_is_collected_from_the_queue() {
        let worker = std::thread::spawn(|| -> std::io::Result<()> { Ok(()) });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !worker.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(worker.is_finished(), "the test worker must finish");
        let directory =
            std::env::temp_dir().join(format!("screencapture-app-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let mut runtime = Runtime::new(directory.clone());
        runtime.finalizing.push(Finalizing {
            recorder: Recorder::from_worker(directory.join("clip.mp4"), worker),
            engine: None,
            grabbed: 3,
            written: 3,
            elapsed: Duration::from_millis(1_500),
        });
        drain_finalizing(&mut runtime);
        assert!(runtime.finalizing.is_empty(), "the stop is collected");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_tray_mute_toggle_flips_and_persists() {
        let directory =
            std::env::temp_dir().join(format!("screencapture-chime-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("temp directory");
        let path = directory.join("settings.cfg");
        std::fs::write(&path, "[capture]\nchimes = true\n").expect("settings seed");
        let mut loaded = crate::settings::load(Some(&path));
        toggle_chimes(&mut loaded);
        assert!(!loaded.settings.chimes, "the session mutes at once");
        assert!(
            !crate::settings::load(Some(&path)).settings.chimes,
            "the saved file mutes too"
        );
        toggle_chimes(&mut loaded);
        assert!(loaded.settings.chimes, "a second toggle restores audio");
        assert!(
            crate::settings::load(Some(&path)).settings.chimes,
            "the saved file turns audio back on"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn recording_logs_the_rescaled_size() {
        let region = Region {
            x: 0,
            y: 0,
            width: 1920,
            height: 1200,
        };
        let hd = Profile::by_id("hd-720p30").expect("720p profile");
        assert_eq!(size_note(region, hd), "1920x1200 -> 1152x720");
        let mut native = hd;
        native.width = 1920;
        native.height = 1200;
        assert_eq!(size_note(region, native), "1920x1200");
    }
}
