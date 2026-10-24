//! Recording sessions: capture worker plus encode feed, with the output
//! path settled at start so callers can surface it immediately.
//!
//! Frames are grabbed and piped on a worker thread. The tray pump only
//! starts and stops the thread, so a 9 MB blit or a full encoder pipe can
//! never freeze the menus or the Stop chip, and pacing is no longer tied
//! to the coarse `WM_TIMER` quantum (a 17 ms timer really fires every
//! ~31 ms, which used to make every 60 fps recording play back at double
//! speed).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio;
use crate::capture::{Region, Target};
use crate::clock;
#[cfg(windows)]
use crate::dxgi::Duplicator;
use crate::encoder::Video;
use crate::ffmpeg::Binary;
use crate::pipe::{self, Feed, Stream};
use crate::recording::Profile;

/// Multimedia timer period requested per recording (millisec).
#[cfg(windows)]
const FAST_PERIOD_MS: u32 = 1;

/// Extra time the stop path allows beyond the engine's own finishing
/// budget, so a healthy finalize completes and reports instead of racing
/// the join deadline.
const STOP_MARGIN: Duration = Duration::from_secs(5);
/// Poll interval while waiting for the worker to finish.
const STOP_POLL: Duration = Duration::from_millis(10);

/// How long this recording's stop may wait: the engine's finalize budget
/// for the bytes on disk plus a margin for the worker to publish its
/// result. Bounded, so a wedged engine cannot freeze the caller forever.
fn stop_budget(output: &Path) -> Duration {
    let bytes = std::fs::metadata(output).map_or(0, |meta| meta.len());
    pipe::finalize_budget(bytes) + STOP_MARGIN
}

#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

/// Coarse-timer boost: Windows clocks sleeps and `SetTimer` at ~15.6 ms
/// unless the multimedia period is lowered; held by the capture worker so
/// its pacing sleeps land within a millisecond or two of the deadline.
/// Other systems pace in finer increments natively.
#[cfg(windows)]
struct FastTimer;

#[cfg(windows)]
impl FastTimer {
    fn begin() -> Self {
        // SAFETY: parameterless period request; symmetric end in `Drop`.
        unsafe { timeBeginPeriod(FAST_PERIOD_MS) };
        Self
    }
}

#[cfg(windows)]
impl Drop for FastTimer {
    fn drop(&mut self) {
        // SAFETY: symmetric release of the period requested above.
        unsafe { timeEndPeriod(FAST_PERIOD_MS) };
    }
}

#[cfg(not(windows))]
struct FastTimer;

#[cfg(not(windows))]
impl FastTimer {
    fn begin() -> Self {
        Self
    }
}

/// Everything needed to start one recording.
#[derive(Clone, Debug)]
pub struct Plan {
    /// Capture origin (screen coordinates).
    pub origin: (i32, i32),
    /// Capture size in physical pixels.
    pub size: (u32, u32),
    /// Output profile (fps, quality, requested resolution).
    pub profile: Profile,
    /// Codec settings (id + container extension).
    pub codec: Video,
    /// Whether the mouse pointer is drawn into frames.
    pub cursor: bool,
    /// Destination folder for the finished file.
    pub media_dir: PathBuf,
    /// The clicked window whose own content is recorded, instead of the
    /// desktop rectangle that frame sits in. `None` records the screen.
    pub window: Option<usize>,
    /// Which audio sources to record alongside the frames.
    pub audio: audio::Wanted,
    /// Which device each source records from.
    pub audio_devices: audio::Devices,
    /// Each source's fader, in dB.
    pub audio_levels: audio::Levels,
}

/// What one recording's capture loop owns: the frame source, the engine
/// pipe, the pace, the clock the audio track shares, and the audio worker.
struct Session {
    /// Frames to grab from.
    source: Source,
    /// The engine reading them.
    feed: Feed,
    /// Frame-rate ceiling from the profile.
    fps: u32,
    /// The clock the frames — and the mixed audio — are paced by.
    started: Instant,
    /// The audio worker, `None` for a silent recording.
    sound: Option<audio::Audio>,
}

/// Frame counts the worker publishes for the reporting path.
#[derive(Default)]
struct Counters {
    /// Frames grabbed from the screen (one blit each).
    grabbed: AtomicU64,
    /// Frames written to the encoder, duplicates included.
    written: AtomicU64,
}

/// An active recording: a worker owns the frame source and the engine
/// pipe, this handle stops it and reports what it did.
pub struct Recorder {
    /// Destination file for this recording.
    pub output: PathBuf,
    /// Wall-clock start of the session, for measured-rate reporting.
    started: Instant,
    /// Set by the caller to end the worker's loop.
    stop: Arc<AtomicBool>,
    /// Progress counters the worker keeps fresh.
    counters: Arc<Counters>,
    /// The worker's fatal error, kept for the watchdog to report without
    /// waiting for a stop.
    failure: Arc<Mutex<Option<String>>>,
    /// Live audio switches for the frame's mute buttons; only Windows has
    /// those buttons, and the rest of the mixer is platform-neutral.
    #[cfg(windows)]
    live: audio::Live,
    /// `None` once the worker was joined.
    worker: Option<JoinHandle<io::Result<()>>>,
}

/// The frame source behind one recording. Desktop duplication serves
/// whole-screen grabs; every other plan (partial regions, a picked window,
/// cursor drawing) goes through the platform capture backend.
enum Source {
    /// Compositor frames, lent to the encoder cheaply.
    #[cfg(windows)]
    Duplication(Duplicator),
    /// One region — or one window's own content — through the platform
    /// backend: GDI blits on Windows, the native compositor elsewhere.
    Capture(Target),
}

impl Source {
    /// One frame, valid until the next call.
    ///
    /// # Errors
    /// Fails when blit or duplication refuses; GDI is the recovery path.
    fn frame(&mut self) -> io::Result<&[u8]> {
        match self {
            #[cfg(windows)]
            Self::Duplication(duplication) => duplication.frame(),
            Self::Capture(target) => target.frame(),
        }
    }
}

impl Recorder {
    /// Start encoding; the output file name is stamped from UTC time.
    /// The capture worker is running when this returns, and setup
    /// failures (no GDI handles, no engine) surface as errors instead of
    /// a silently dead recording.
    ///
    /// # Errors
    /// Fails when GDI setup or the engine spawn fails.
    pub fn start(binary: &Binary, plan: &Plan) -> io::Result<Self> {
        let region = Region {
            x: plan.origin.0,
            y: plan.origin.1,
            width: plan.size.0,
            height: plan.size.1,
        };
        if !region.is_sane() {
            return Err(io::Error::other("capture region is empty"));
        }
        // The recorder owns its output path, so it also owns the directory
        // that path needs; the CLI and tray paths must behave alike.
        std::fs::create_dir_all(&plan.media_dir)?;
        let output = clock::stamped_path(&plan.media_dir, plan.codec.extension())?;

        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Counters::default());
        let failure = Arc::new(Mutex::new(None));
        // One set of audio switches for the take: the worker marks what it
        // opened, the mixer reads them, and the frame's buttons flip them.
        let live = audio::Live::new(plan.audio);
        let (ready, started) = mpsc::channel();
        let setup = (binary.clone(), plan.clone(), region, output.clone());
        let worker = {
            let stop = Arc::clone(&stop);
            let counters = Arc::clone(&counters);
            let failure = Arc::clone(&failure);
            let live = live.clone();
            thread::Builder::new()
                .name("capture".into())
                .spawn(move || capture(setup, &live, &stop, &counters, &failure, &ready))
        };
        let worker = match worker {
            Ok(worker) => worker,
            Err(error) => {
                let _ = std::fs::remove_file(&output);
                return Err(io::Error::other(format!(
                    "cannot start the capture worker: {error}"
                )));
            }
        };
        match started.recv() {
            Ok(Ok(())) => Ok(Self {
                output,
                started: Instant::now(),
                stop,
                counters,
                failure,
                #[cfg(windows)]
                live,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                let _ = std::fs::remove_file(&output);
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                let _ = std::fs::remove_file(&output);
                Err(io::Error::other(
                    "the capture worker exited before the recording started",
                ))
            }
        }
    }

    /// Frames written to the engine, duplicates included, and the
    /// wall-clock time since the recording started.
    #[must_use]
    pub fn stats(&self) -> (u64, Duration) {
        (
            self.counters.written.load(Ordering::Relaxed),
            self.started.elapsed(),
        )
    }

    /// Frames actually grabbed from the screen; fewer than [`Self::stats`]
    /// when the capture could not keep up and frames were repeated.
    #[must_use]
    pub fn grabs(&self) -> u64 {
        self.counters.grabbed.load(Ordering::Relaxed)
    }

    /// The take's live audio switches: which sources are capturing and
    /// which are muted. The recording frame's buttons drive them, and the
    /// mixer obeys them mid-take.
    #[cfg(windows)]
    #[must_use]
    pub fn audio(&self) -> audio::Live {
        self.live.clone()
    }

    /// The worker's fatal error, if it stopped on its own; the recording
    /// keeps running until the caller finalizes it.
    ///
    /// A panic unwinds out of the capture loop before it can publish its
    /// error, so a finished worker with nothing recorded is reported here
    /// too: otherwise the border, the frame timer, and the hotkey would
    /// keep promising a recording that stopped capturing.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        if let Some(message) = self.failure.lock().ok().and_then(|slot| slot.clone()) {
            return Some(message);
        }
        self.worker
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
            .then(|| "the capture worker exited without reporting an error".to_owned())
    }

    /// A recorder wrapped around a caller-supplied worker, so a test can
    /// have a stop that finishes (or refuses to) on demand.
    #[cfg(test)]
    pub(crate) fn from_worker(output: PathBuf, worker: JoinHandle<io::Result<()>>) -> Self {
        Self {
            output,
            started: Instant::now(),
            stop: Arc::new(AtomicBool::new(false)),
            counters: Arc::new(Counters::default()),
            failure: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            live: crate::audio::Live::default(),
            worker: Some(worker),
        }
    }

    /// Stop the worker, let the engine finalize the container, and return
    /// the finished file.
    ///
    /// # Errors
    /// Fails when the capture worker or the engine reports an error.
    pub fn stop(mut self) -> io::Result<PathBuf> {
        self.request_stop();
        let output = self.output.clone();
        match self.join_within(stop_budget(&output)) {
            Some(Ok(Ok(()))) => Ok(output),
            Some(Ok(Err(error))) => Err(error),
            Some(Err(_)) => Err(io::Error::other("the capture worker stopped unexpectedly")),
            None => Err(io::Error::other(
                "the capture worker did not stop; the engine is wedged",
            )),
        }
    }

    /// Ask the worker to leave its loop; safe to call more than once.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Whether the worker has left its loop, so closing the recorder now
    /// will not wait on the engine.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Wait up to `budget` for the worker; `None` means it missed the
    /// deadline and was abandoned. A worker blocked on a wedged engine can
    /// never be joined, and waiting on it would freeze the calling thread.
    /// The abandoned thread ends with the process; the engine process
    /// itself can outlive it, which is the price of not holding a
    /// termination handle here.
    fn join_within(&mut self, budget: Duration) -> Option<thread::Result<io::Result<()>>> {
        if self.worker.is_none() {
            return Some(Ok(Ok(())));
        }
        let deadline = Instant::now() + budget;
        while !self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if Instant::now() >= deadline {
                self.worker = None;
                return None;
            }
            thread::sleep(STOP_POLL);
        }
        Some(self.worker.take()?.join())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // A recorder that was never explicitly stopped must still take its
        // worker and its ffmpeg child down with it — but never at the cost
        // of hanging the thread that drops it.
        self.request_stop();
        let _ = self.join_within(stop_budget(&self.output));
    }
}

/// The capture worker's body: open the audio sources, spawn the engine,
/// pick a capture source, then run the frame loop until the stop flag
/// flips. A failure before the first frame is reported to `ready` so the
/// caller learns about it instead of waiting forever.
fn capture(
    setup: (Binary, Plan, Region, PathBuf),
    live: &audio::Live,
    stop: &AtomicBool,
    counters: &Counters,
    failure: &Mutex<Option<String>>,
    ready: &mpsc::Sender<Result<(), io::Error>>,
) -> io::Result<()> {
    let (binary, plan, region, output) = setup;
    // Everything thread-touching is built here: no capture handle ever
    // crosses a thread boundary, and the audio sources open on the thread
    // that will poll them.
    let prepared = audio::Prepared::open(plan.audio, &plan.audio_devices, plan.audio_levels, live);
    let args = engine_args(&plan, &output, prepared.as_ref());
    let opened = pick_source(&region, &plan)
        .and_then(|source| Feed::spawn(&binary, &args, &output).map(|feed| (source, feed)));
    let (source, feed) = match opened {
        Ok(parts) => parts,
        Err(error) => {
            let _ = ready.send(Err(io::Error::other(error.to_string())));
            return Err(error);
        }
    };
    // One clock for the whole file: the audio worker counts
    // from the instant the first frame is due, so the time
    // it spends connecting becomes silence at the head of
    // the track instead of a shift against the picture.
    let started = Instant::now();
    let sound = prepared.and_then(|prepared| match audio::start(prepared, started) {
        Ok(sound) => Some(sound),
        Err(error) => {
            crate::log::note("audio", &format!("audio capture could not start ({error})"));
            None
        }
    });
    if ready.send(Ok(())).is_err() {
        return Ok(());
    }
    capture_loop(
        Session {
            source,
            feed,
            fps: plan.profile.fps,
            started,
            sound,
        },
        stop,
        counters,
        failure,
    )
}

/// The worker loop: grab a frame, write it, then sleep until the next
/// frame slot the wall clock says is due.
///
/// The output is a constant-frame-rate file, so its length is decided by
/// the number of frames, not by how fast the screen could be grabbed.
/// Frames are therefore paced against the clock: when the capture keeps
/// up each slot gets a fresh grab, and when a blit takes longer than a
/// frame period the same grab fills the missing slots, so a large window
/// recorded at 60 fps still plays back in real time (it just repeats
/// frames instead of running fast).
fn capture_loop(
    session: Session,
    stop: &AtomicBool,
    counters: &Counters,
    failure: &Mutex<Option<String>>,
) -> io::Result<()> {
    let Session {
        mut source,
        mut feed,
        fps,
        started,
        sound,
    } = session;
    let _timer = FastTimer::begin();
    let fps = fps.max(1);
    let mut written: u64 = 0;
    let mut grabbed: u64 = 0;
    let result = loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        let frame = match source.frame() {
            Ok(frame) => frame,
            Err(error) => break Err(error),
        };
        grabbed += 1;
        counters.grabbed.store(grabbed, Ordering::Relaxed);

        let due = frames_due(started.elapsed(), fps);
        let fill = due.saturating_sub(written).clamp(1, catch_up(fps));
        let mut blocked = None;
        for _ in 0..fill {
            if let Err(error) = feed.frame(frame) {
                blocked = Some(error);
                break;
            }
        }
        if let Some(error) = blocked {
            break Err(error);
        }
        written += fill;
        counters.written.store(written, Ordering::Relaxed);

        // Integer math keeps the deadline exact; it is just the wall-clock
        // time at which the frame just written was due.
        let deadline =
            started + Duration::from_micros(written.saturating_mul(1_000_000) / u64::from(fps));
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
    };
    if let Err(error) = &result
        && let Ok(mut slot) = failure.lock()
    {
        *slot = Some(error.to_string());
    }
    // Audio first: stopping the worker closes its pipe, which is how the
    // engine learns the sound ended while it still has video stdin open.
    if let Some(sound) = sound {
        sound.stop();
    }
    // Closing stdin lets a healthy engine flush the container; the
    // finished file is what the caller reports on either way.
    let finished = feed.finish();
    match result {
        Ok(()) => finished,
        Err(error) => Err(error),
    }
}

/// How many frames the file should hold after `elapsed`: one per frame
/// period already served, starting with the first frame at time zero.
fn frames_due(elapsed: Duration, fps: u32) -> u64 {
    let fps = u64::from(fps.max(1));
    let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
    (micros.saturating_mul(fps) / 1_000_000).saturating_add(1)
}

/// Frames one grab may fill at most: half a second of output. A longer
/// stall keeps the file within half a second of real time instead of
/// blasting a minute of duplicates into the encoder at once.
fn catch_up(fps: u32) -> u64 {
    u64::from(fps.max(2)) / 2
}

/// What a plan asks for, decided before any source is opened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Wanted {
    /// The picked window, through Windows Graphics Capture.
    Window,
    /// A whole display, straight from desktop duplication.
    #[cfg(windows)]
    Duplication,
    /// A desktop rectangle through the platform backend.
    Region,
}

/// Which source a plan is asking for.
///
/// A picked window wins even when its frame is exactly one display: a
/// maximized window covers the screen, and duplication would hand back the
/// desktop with whatever sits on top of it — the very bug window capture
/// exists to fix.
fn wanted(region: Region, plan: &Plan) -> Wanted {
    if plan.window.is_some() {
        return Wanted::Window;
    }
    #[cfg(windows)]
    if crate::dxgi::eligible(region) {
        return Wanted::Duplication;
    }
    #[cfg(not(windows))]
    let _ = region;
    Wanted::Region
}

/// Choose and open the frame source, demoting to the plain region backend
/// with a log line whenever a richer source refuses.
///
/// # Errors
/// Propagates the region backend's setup failure; a refused window or
/// duplication source only demotes the plan.
fn pick_source(region: &Region, plan: &Plan) -> io::Result<Source> {
    match wanted(*region, plan) {
        #[cfg(windows)]
        Wanted::Window => {
            if let Some(handle) = plan.window {
                match Target::window(handle, *region, plan.cursor) {
                    Ok(target) => return Ok(Source::Capture(target)),
                    Err(error) => crate::log::note(
                        "capture",
                        &format!(
                            "window capture unavailable ({error}); recording the screen instead"
                        ),
                    ),
                }
            }
            Ok(Source::Capture(Target::new(*region, plan.cursor)?))
        }
        #[cfg(windows)]
        Wanted::Duplication => match Duplicator::open(*region, plan.cursor) {
            Ok(duplication) => Ok(Source::Duplication(duplication)),
            Err(reason) => {
                crate::log::note(
                    "capture",
                    &format!("desktop duplication unavailable ({reason}); using GDI"),
                );
                Ok(Source::Capture(Target::new(*region, plan.cursor)?))
            }
        },
        _ => Ok(Source::Capture(Target::new(*region, plan.cursor)?)),
    }
}

/// The engine's argument vector for one plan: raw frames on stdin, plus the
/// mixed audio pipe as a second input when the recording has sound.
fn engine_args(plan: &Plan, output: &Path, sound: Option<&audio::Prepared>) -> Vec<String> {
    pipe::record_args(
        Stream {
            width: plan.size.0,
            height: plan.size.1,
            fps: plan.profile.fps,
        },
        &plan.codec.args(&plan.profile),
        scale_for(plan),
        output,
        sound.map(|prepared| pipe::Sound {
            path: prepared.path(),
            bitrate: plan.audio.quality.bitrate(),
        }),
    )
}

/// The output frame whenever it differs from the capture.
///
/// The engine input stream stays at capture resolution for fidelity; ffmpeg
/// then fits it into the profile's resolution while keeping the capture's
/// aspect ratio (a 16:10 window through 720p becomes 1152x720, not 1280x720
/// with side bars).
fn scale_for(plan: &Plan) -> Option<(u32, u32)> {
    let output = plan.profile.fit(plan.size);
    (output != plan.size).then_some(output)
}

#[cfg(test)]
mod tests {
    use super::{Counters, Plan, Recorder, Wanted, catch_up, frames_due, scale_for, wanted};
    use crate::encoder::Video;
    use crate::recording::Profile;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[test]
    fn scales_only_to_fit_the_profile() {
        let equal = Plan {
            origin: (0, 0),
            size: (3840, 2160),
            profile: Profile::by_id("uhd-4k60").expect("profile exists"),
            codec: Video::DEFAULT,
            cursor: false,
            media_dir: PathBuf::new(),
            window: None,
            audio: crate::audio::Wanted::default(),
            audio_devices: crate::audio::Devices::default(),
            audio_levels: crate::audio::Levels::default(),
        };
        assert_eq!(scale_for(&equal), None);
        // A capture inside the profile keeps its own pixels: the profile
        // caps the output, it never invents detail by upscaling.
        let smaller = Plan {
            size: (1920, 1080),
            ..equal
        };
        assert_eq!(scale_for(&smaller), None);
        let larger = Plan {
            size: (5120, 2880),
            ..smaller
        };
        assert_eq!(scale_for(&larger), Some((3840, 2160)));
    }

    #[test]
    fn scaling_keeps_the_capture_aspect_ratio() {
        let plan = Plan {
            origin: (0, 0),
            size: (1440, 900),
            profile: Profile::by_id("hd-720p60").expect("profile exists"),
            codec: Video::DEFAULT,
            cursor: false,
            media_dir: PathBuf::new(),
            window: None,
            audio: crate::audio::Wanted::default(),
            audio_devices: crate::audio::Devices::default(),
            audio_levels: crate::audio::Levels::default(),
        };
        // 16:10 through a 16:9 profile: full height, no letterbox bars.
        assert_eq!(scale_for(&plan), Some((1152, 720)));
    }

    #[test]
    fn frame_slots_follow_the_wall_clock() {
        // One frame immediately, then one per period.
        assert_eq!(frames_due(Duration::ZERO, 60), 1);
        assert_eq!(frames_due(Duration::from_millis(16), 60), 1);
        assert_eq!(frames_due(Duration::from_millis(17), 60), 2);
        assert_eq!(frames_due(Duration::from_secs(1), 60), 61);
        assert_eq!(frames_due(Duration::from_secs(1), 30), 31);
        // A stalled capture never asks for less than one slot.
        assert_eq!(frames_due(Duration::ZERO, 0), 1);
    }

    /// A worker that returns without publishing an error is dead, not
    /// busy: the watchdog must see it, or the recording UI keeps promising
    /// capture that already stopped.
    #[test]
    fn a_worker_that_exits_without_an_error_is_reported() {
        let worker = std::thread::spawn(|| -> std::io::Result<()> { Ok(()) });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !worker.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(worker.is_finished(), "the test worker must finish");
        let recorder = Recorder {
            output: PathBuf::new(),
            started: std::time::Instant::now(),
            stop: Arc::new(AtomicBool::new(false)),
            counters: Arc::new(Counters::default()),
            failure: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            live: crate::audio::Live::default(),
            worker: Some(worker),
        };
        assert_eq!(
            recorder.failure().as_deref(),
            Some("the capture worker exited without reporting an error")
        );
    }

    /// A worker that cannot finish is abandoned at the budget instead of
    /// holding the stop path forever, and dropping the handle afterwards
    /// does not wait on it either.
    #[test]
    fn a_wedged_worker_is_abandoned_instead_of_waited_on() {
        let worker = std::thread::spawn(|| -> std::io::Result<()> {
            std::thread::sleep(Duration::from_secs(30));
            Ok(())
        });
        let mut recorder = Recorder {
            output: PathBuf::new(),
            started: Instant::now(),
            stop: Arc::new(AtomicBool::new(false)),
            counters: Arc::new(Counters::default()),
            failure: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            live: crate::audio::Live::default(),
            worker: Some(worker),
        };
        assert!(recorder.join_within(Duration::from_millis(50)).is_none());
        assert!(recorder.worker.is_none(), "the handle is detached");
        let dropped = Instant::now();
        drop(recorder);
        assert!(dropped.elapsed() < Duration::from_secs(1));
    }

    /// A picked window must not fall into duplication just because its
    /// frame happens to cover a whole display (a maximized window): that
    /// path records the desktop, occluders included, which is exactly what
    /// selecting the application avoids.
    #[test]
    fn a_picked_window_outranks_a_whole_display() {
        let plan = Plan {
            origin: (0, 0),
            size: (1920, 1080),
            profile: Profile::by_id("hd-1080p60").expect("profile exists"),
            codec: Video::DEFAULT,
            cursor: false,
            media_dir: PathBuf::new(),
            window: Some(0x1234),
            audio: crate::audio::Wanted::default(),
            audio_devices: crate::audio::Devices::default(),
            audio_levels: crate::audio::Levels::default(),
        };
        let display = crate::capture::Region {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(wanted(display, &plan), Wanted::Window);
        // The same rectangle without a window is a whole-display grab, so
        // the assertion above is not vacuous on this platform.
        #[cfg(windows)]
        assert_eq!(
            wanted(
                display,
                &Plan {
                    window: None,
                    ..plan
                }
            ),
            Wanted::Duplication
        );
    }

    #[test]
    fn a_region_plan_without_a_window_asks_for_its_own_backend() {
        let plan = Plan {
            origin: (0, 0),
            size: (640, 480),
            profile: Profile::by_id("sd-480p30").expect("profile exists"),
            codec: Video::DEFAULT,
            cursor: false,
            media_dir: PathBuf::new(),
            window: None,
            audio: crate::audio::Wanted::default(),
            audio_devices: crate::audio::Devices::default(),
            audio_levels: crate::audio::Levels::default(),
        };
        let region = crate::capture::Region {
            x: 10,
            y: 10,
            width: 640,
            height: 480,
        };
        assert_ne!(wanted(region, &plan), Wanted::Window);
    }

    #[test]
    fn catch_up_stays_inside_half_a_second() {
        assert_eq!(catch_up(60), 30);
        assert_eq!(catch_up(30), 15);
        assert_eq!(catch_up(120), 60);
        // Degenerate profiles still allow a frame through.
        assert_eq!(catch_up(1), 1);
        assert_eq!(catch_up(0), 1);
    }
}
