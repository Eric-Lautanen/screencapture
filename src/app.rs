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
use crate::recording::{self, Profile};
use crate::screen;
use crate::settings::{self, Loaded};
use crate::tray::{self, Event};

/// Mutable application state the tray handler drives.
struct Runtime {
    recorder: Option<Recorder>,
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
    let mut runtime = Runtime { recorder: None };
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
    if let Err(error) = std::fs::create_dir_all(&loaded.settings.media_dir) {
        eprintln!("error: cannot create media directory: {error}");
        return;
    }
    match start_recording(loaded) {
        Ok(recorder) => runtime.recorder = Some(recorder),
        Err(error) => eprintln!("error: cannot start recording: {error}"),
    }
}

fn start_recording(loaded: &Loaded) -> io::Result<Recorder> {
    let binary = engine(loaded)?;
    let display = screen::primary()?;
    let profile = loaded.settings.profile;
    let plan = Plan {
        origin: (display.x, display.y),
        size: (display.width, display.height),
        profile,
        codec: loaded.settings.codec,
        cursor: loaded.settings.cursor,
        media_dir: loaded.settings.media_dir.clone(),
    };
    let recorder = Recorder::start(&binary, &plan)?;
    tray::set_frame_timer(Some(frame_interval_ms(profile.fps)));
    eprintln!(
        "record: {} ({}x{} @ {} fps, {} to stop)",
        recorder.output.display(),
        display.width,
        display.height,
        profile.fps,
        loaded.settings.hotkey,
    );
    Ok(recorder)
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

/// Stop an active recording; called from the stop paths and on exit.
fn stop_recording(runtime: &mut Runtime) {
    if let Some(recorder) = runtime.recorder.take() {
        tray::set_frame_timer(None);
        match recorder.stop() {
            Ok(path) => eprintln!("record: saved {}", path.display()),
            Err(error) => eprintln!("error: cannot finalize recording: {error}"),
        }
    }
}

fn photo(loaded: &Loaded) {
    match take_photo(loaded) {
        Ok(path) => eprintln!("photo: {}", path.display()),
        Err(error) => eprintln!("error: {error}"),
    }
}

fn take_photo(loaded: &Loaded) -> io::Result<std::path::PathBuf> {
    let binary = engine(loaded)?;
    std::fs::create_dir_all(&loaded.settings.media_dir)?;
    let display = screen::primary()?;
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
pub fn record_demo(config: Option<&Path>, seconds: u32) -> ExitCode {
    let loaded = settings::load(config);
    for problem in &loaded.problems {
        eprintln!("warn  settings: {problem}");
    }
    match demo_recording(&loaded, loaded.settings.profile, seconds) {
        Ok(path) => {
            println!("recorded: {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn demo_recording(
    loaded: &Loaded,
    profile: recording::Profile,
    seconds: u32,
) -> io::Result<std::path::PathBuf> {
    let binary = engine(loaded)?;
    let display = screen::primary()?;
    let plan = Plan {
        origin: (display.x, display.y),
        size: (display.width, display.height),
        profile,
        codec: loaded.settings.codec,
        cursor: loaded.settings.cursor,
        media_dir: loaded.settings.media_dir.clone(),
    };
    let mut recorder = Recorder::start(&binary, &plan)?;
    let budget = Duration::from_secs(u64::from(seconds));
    let interval = Duration::from_millis(u64::from(frame_interval_ms(profile.fps)));
    let started = std::time::Instant::now();
    let mut deadline = started;
    while started.elapsed() < budget {
        recorder.tick()?;
        deadline += interval;
        let now = std::time::Instant::now();
        // Sleep only while ahead of the frame deadline; each tick may already
        // have consumed most of it.
        if deadline > now {
            std::thread::sleep(deadline - now);
        }
    }
    recorder.stop()
}

/// Take one still photo from the command line; smoke-tests the photo path.
pub fn photo_demo(config: Option<&Path>) -> ExitCode {
    let loaded = settings::load(config);
    for problem in &loaded.problems {
        eprintln!("warn  settings: {problem}");
    }
    match take_photo(&loaded) {
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
