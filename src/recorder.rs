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
    /// `None` once the worker was joined.
    worker: Option<JoinHandle<io::Result<()>>>,
}

/// The frame source behind one recording. Desktop duplication serves
/// whole-screen grabs; every other plan (partial regions, window bounds,
/// cursor drawing) stays GDI by design.
enum Source {
    /// Compositor frames, lent to the encoder cheaply.
    #[cfg(windows)]
    Duplication(Duplicator),
    /// GDI blits on Windows; each platform's native backend elsewhere.
    Blit(Target),
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
            Self::Blit(target) => target.frame(),
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
        let output = clock::stamped_path(&plan.media_dir, plan.codec.extension());
        let args = pipe::record_args(
            Stream {
                width: plan.size.0,
                height: plan.size.1,
                fps: plan.profile.fps,
            },
            &plan.codec.args(&plan.profile),
            scale_for(plan),
            &output,
        );

        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Counters::default());
        let failure = Arc::new(Mutex::new(None));
        let (ready, started) = mpsc::channel();
        let setup = (binary.clone(), plan.clone(), region, args, output.clone());
        let worker = {
            let stop = Arc::clone(&stop);
            let counters = Arc::clone(&counters);
            let failure = Arc::clone(&failure);
            thread::Builder::new()
                .name("capture".into())
                .spawn(move || {
                    let (binary, plan, region, args, output) = setup;
                    // Everything thread-touching is built here, so no
                    // capture handle ever crosses a thread boundary.
                    let opened = pick_source(&region, &plan).and_then(|source| {
                        Feed::spawn(&binary, &args, &output).map(|feed| (source, feed))
                    });
                    let (source, feed) = match opened {
                        Ok(parts) => parts,
                        Err(error) => {
                            let _ = ready.send(Err(io::Error::other(error.to_string())));
                            return Err(error);
                        }
                    };
                    if ready.send(Ok(())).is_err() {
                        return Ok(());
                    }
                    capture_loop(source, feed, plan.profile.fps, &stop, &counters, &failure)
                })
        };
        let worker = match worker {
            Ok(worker) => worker,
            Err(error) => {
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
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
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
    mut source: Source,
    mut feed: Feed,
    fps: u32,
    stop: &AtomicBool,
    counters: &Counters,
    failure: &Mutex<Option<String>>,
) -> io::Result<()> {
    let _timer = FastTimer::begin();
    let fps = fps.max(1);
    let started = Instant::now();
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

/// Choose the frame source: desktop duplication when the plan fits it
/// and the compositor cooperates, the GDI blit otherwise; other systems
/// always use their native backend.
///
/// # Errors
/// Propagates the backend setup failure; duplication failures only
/// demote the plan to GDI after a printed notice.
fn pick_source(region: &Region, plan: &Plan) -> io::Result<Source> {
    #[cfg(windows)]
    {
        if crate::dxgi::eligible(*region) {
            match Duplicator::open(*region, plan.cursor) {
                Ok(duplication) => return Ok(Source::Duplication(duplication)),
                Err(reason) => {
                    eprintln!("capture: desktop duplication unavailable ({reason}); using GDI");
                }
            }
        }
    }
    Ok(Source::Blit(Target::new(*region, plan.cursor)?))
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
    use super::{Counters, Plan, Recorder, catch_up, frames_due, scale_for};
    use crate::encoder::Video;
    use crate::recording::Profile;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[test]
    fn scales_only_when_sizes_differ() {
        let equal = Plan {
            origin: (0, 0),
            size: (3840, 2160),
            profile: Profile::by_id("uhd-4k60").expect("profile exists"),
            codec: Video::DEFAULT,
            cursor: false,
            media_dir: PathBuf::new(),
        };
        assert_eq!(scale_for(&equal), None);
        let smaller = Plan {
            size: (1920, 1080),
            ..equal
        };
        assert_eq!(scale_for(&smaller), Some((3840, 2160)));
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
            worker: Some(worker),
        };
        assert!(recorder.join_within(Duration::from_millis(50)).is_none());
        assert!(recorder.worker.is_none(), "the handle is detached");
        let dropped = Instant::now();
        drop(recorder);
        assert!(dropped.elapsed() < Duration::from_secs(1));
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
