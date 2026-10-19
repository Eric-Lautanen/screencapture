//! Recording sessions: capture target plus encode feed, with the output
//! path settled at start so callers can surface it immediately.

use std::io;
use std::path::PathBuf;

use crate::capture::{Region, Target};
use crate::clock;
use crate::dxgi::Duplicator;
use crate::encoder::Video;
use crate::ffmpeg::Binary;
use crate::pipe::{self, Feed, Stream};
use crate::recording::Profile;

/// Multimedia timer period requested per recording (millisec).
const FAST_PERIOD_MS: u32 = 1;

#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

/// Coarse-timer boost: Windows clocks sleeps and `SetTimer` at ~15.6 ms
/// unless the multimedia period is lowered; active for the recorder's
/// whole life so pacing deadlines stay honest.
struct FastTimer;

impl FastTimer {
    fn begin() -> Self {
        // SAFETY: parameterless period request; symmetric end in `Drop`.
        unsafe { timeBeginPeriod(FAST_PERIOD_MS) };
        Self
    }
}

impl Drop for FastTimer {
    fn drop(&mut self) {
        // SAFETY: symmetric release of the period requested above.
        unsafe { timeEndPeriod(FAST_PERIOD_MS) };
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

/// An active recording: grab frames and encode them.
pub struct Recorder {
    source: Source,
    feed: Feed,
    /// Held so the fast multimedia timer drops exactly with the recording.
    _timer: FastTimer,
    /// Destination file for this recording.
    pub output: PathBuf,
}

/// The frame source behind one recording. Desktop duplication serves
/// whole-screen grabs; every other plan (partial regions, window bounds,
/// cursor drawing) stays GDI by design.
enum Source {
    /// Compositor frames, lent to the encoder cheaply.
    Duplication(Duplicator),
    /// GDI blits, the universal fallback.
    Blit(Target),
}

impl Source {
    /// One frame, valid until the next call.
    ///
    /// # Errors
    /// Fails when blit or duplication refuses; GDI is the recovery path.
    fn frame(&mut self) -> io::Result<&[u8]> {
        match self {
            Self::Duplication(duplication) => duplication.frame(),
            Self::Blit(target) => target.frame(),
        }
    }
}

impl Recorder {
    /// Start encoding; the output file name is stamped from UTC time.
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
        let timer = FastTimer::begin();
        let source = pick_source(&region, plan)?;
        let output = plan.media_dir.join(format!(
            "screencapture-{}.{ext}",
            clock::stamp(),
            ext = plan.codec.extension()
        ));
        let scale = scale_for(plan);
        let args = pipe::record_args(
            Stream {
                width: plan.size.0,
                height: plan.size.1,
                fps: plan.profile.fps,
            },
            &plan.codec.args(&plan.profile),
            scale,
            &output,
        );
        let feed = Feed::spawn(binary, &args)?;
        Ok(Self {
            source,
            feed,
            _timer: timer,
            output,
        })
    }

    /// Capture and enqueue one frame; the frame is lent straight from the
    /// capture surface, so nothing is copied twice.
    ///
    /// # Errors
    /// Fails when the frame refuses, the frame is unavailable, or the engine
    /// stopped consuming frames.
    pub fn tick(&mut self) -> io::Result<()> {
        let frame = self.source.frame()?;
        self.feed.frame(frame)
    }

    /// Stop the pipeline and let ffmpeg finalize the container; returns the
    /// finished file.
    ///
    /// # Errors
    /// Fails when the engine exits with an error.
    pub fn stop(self) -> io::Result<std::path::PathBuf> {
        let output = self.output.clone();
        self.feed.finish()?;
        Ok(output)
    }
}

/// Choose the frame source: desktop duplication when the plan fits it and
/// the compositor cooperates, the GDI blit otherwise.
///
/// # Errors
/// Propagates the GDI setup failure; duplication failures only demote the
/// plan to GDI after a printed notice.
fn pick_source(region: &Region, plan: &Plan) -> io::Result<Source> {
    if crate::dxgi::eligible(*region, plan.cursor) {
        match Duplicator::open(*region) {
            Ok(duplication) => return Ok(Source::Duplication(duplication)),
            Err(reason) => {
                eprintln!("capture: desktop duplication unavailable ({reason}); using GDI");
            }
        }
    }
    Ok(Source::Blit(Target::new(*region, plan.cursor)?))
}

/// Scale the capture to the profile resolution whenever they differ.
///
/// The engine input stream stays at capture resolution for fidelity; ffmpeg
/// then downsizes (or upsizes) to exactly what the profile promises.
fn scale_for(plan: &Plan) -> Option<(u32, u32)> {
    let (capture_width, capture_height) = plan.size;
    (plan.profile.width != capture_width || plan.profile.height != capture_height)
        .then_some((plan.profile.width, plan.profile.height))
}

#[cfg(test)]
mod tests {
    use super::{Plan, scale_for};
    use crate::encoder::Video;
    use crate::recording::Profile;
    use std::path::PathBuf;

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
}
