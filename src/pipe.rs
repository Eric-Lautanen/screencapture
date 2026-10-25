//! `FFmpeg` frame-pipe jobs: raw BGRA frames streamed into stdin for both
//! recordings and still photos; the process outlives one call.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

/// Poll interval while waiting for the engine to exit.
const FINISH_POLL: Duration = Duration::from_millis(20);
/// Smallest finalize budget: covers engine start-up, the moov write, and
/// the still-photo path, which has no `+faststart` rewrite to do.
const FINALIZE_FLOOR: Duration = Duration::from_secs(10);
/// Ceiling for a pathological recording: a wedged engine must not hold the
/// stop path for longer than this, however large the file looked.
const FINALIZE_CEILING: Duration = Duration::from_secs(300);
/// Rewrite throughput assumed when sizing the budget. `+faststart` moves
/// the whole container, so finalization costs roughly the file size at
/// disk speed; this is deliberately conservative so a slow but healthy
/// disk is never killed mid-rewrite.
const FINALIZE_THROUGHPUT: u64 = 10_000_000;

/// How long the engine may take to close `bytes` of finished output. The
/// budget grows with the file because `+faststart` rewrites the container;
/// it still bounds a wedged engine so the stop path cannot hang forever.
#[must_use]
pub fn finalize_budget(bytes: u64) -> Duration {
    (FINALIZE_FLOOR + Duration::from_secs(bytes / FINALIZE_THROUGHPUT)).min(FINALIZE_CEILING)
}

/// Fixed input-stream description for a raw BGRA pipe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stream {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Frames per second (photos use 1).
    pub fps: u32,
}

/// Arguments that make ffmpeg accept BGRA frames from stdin.
#[must_use]
pub fn input_args(stream: Stream) -> Vec<String> {
    vec![
        "-f".to_owned(),
        "rawvideo".to_owned(),
        "-pix_fmt".to_owned(),
        "bgra".to_owned(),
        "-s".to_owned(),
        format!("{}x{}", stream.width, stream.height),
        "-framerate".to_owned(),
        stream.fps.to_string(),
    ]
}

/// The mixed-audio input for one recording: the pipe the audio worker
/// writes and the AAC bitrate the engine gives the track.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sound<'a> {
    /// Named pipe carrying 48 kHz stereo s16 samples.
    pub path: &'a str,
    /// `-b:a` value, from [`crate::audio::Quality::bitrate`].
    pub bitrate: &'a str,
}

/// Full argument vector for a recording; `frame` is the output size the
/// caller computed from the capture's aspect ratio, or `None` to keep the
/// capture size, and `audio` describes the pipe the mixed capture stream
/// arrives on, or `None` for a silent recording.
#[must_use]
pub fn record_args(
    stream: Stream,
    codec_args: &[String],
    frame: Option<(u32, u32)>,
    output: &Path,
    audio: Option<Sound<'_>>,
) -> Vec<String> {
    let mut args = base_args();
    args.extend(input_args(stream));
    args.extend(["-i".to_owned(), "pipe:0".to_owned()]);
    if let Some(sound) = audio {
        // Raw PCM at the exact shape the audio worker writes. Two inputs
        // mean stream selection is no longer automatic: the video is the
        // first input's one stream, the mix is the second's.
        args.extend([
            "-f".to_owned(),
            "s16le".to_owned(),
            "-ar".to_owned(),
            "48000".to_owned(),
            "-ac".to_owned(),
            "2".to_owned(),
            "-i".to_owned(),
            sound.path.to_owned(),
            "-map".to_owned(),
            "0:v:0".to_owned(),
            "-map".to_owned(),
            "1:a:0".to_owned(),
            "-c:a".to_owned(),
            "aac".to_owned(),
            "-b:a".to_owned(),
            sound.bitrate.to_owned(),
        ]);
    }
    if let Some((width, height)) = frame {
        // Resample to the frame the profile allows at the capture's own
        // aspect ratio. No padding: a 16:10 window through a 16:9 profile
        // becomes 1152x720, not 1280x720 with black side bars.
        args.extend(["-vf".to_owned(), format!("scale={width}:{height}")]);
    }
    args.extend(codec_args.iter().cloned());
    args.push(output.display().to_string());
    args
}

/// Full argument vector for one still photo. A lossless choice writes PNG,
/// where `depth` selects the pixel depth: 16 adds the 48-bit RGB big-endian
/// conversion the PNG encoder documents (`rgb48be`), under the same
/// lossless compression arguments. A JPEG choice swaps the codec for MJPEG
/// at that step's quality, in full-resolution 4:4:4 (`yuvj444p`) because
/// 4:2:0 makes coloured UI text fringe; `depth` does not apply to it.
/// Lossless WebP asks libwebp for its lossless mode at full effort; BMP is
/// the uncompressed 32-bit bitmap the encoder takes straight from BGRA.
#[must_use]
pub fn still_args(
    stream: Stream,
    depth: crate::settings::StillDepth,
    quality: crate::settings::StillQuality,
    output: &Path,
) -> Vec<String> {
    let mut args = base_args();
    args.extend(input_args(stream));
    args.extend([
        "-i".to_owned(),
        "pipe:0".to_owned(),
        "-frames:v".to_owned(),
        "1".to_owned(),
    ]);
    if let Some(qscale) = quality.jpeg_qscale() {
        args.extend([
            "-c:v".to_owned(),
            "mjpeg".to_owned(),
            "-q:v".to_owned(),
            qscale.to_owned(),
            "-pix_fmt".to_owned(),
            "yuvj444p".to_owned(),
        ]);
    } else if quality == crate::settings::StillQuality::Webp {
        args.extend([
            "-c:v".to_owned(),
            "libwebp".to_owned(),
            "-lossless".to_owned(),
            "1".to_owned(),
            // In lossless mode libwebp reads this as compression effort.
            "-quality".to_owned(),
            "100".to_owned(),
        ]);
    } else if quality == crate::settings::StillQuality::Bmp {
        // Uncompressed 32-bit BMP: BGRA goes in as the encoder's own bgra
        // input and nothing is compressed — the choice for a consumer that
        // wants the raw bitmap.
        args.extend(["-c:v".to_owned(), "bmp".to_owned()]);
    } else {
        if depth == crate::settings::StillDepth::Sixteen {
            args.extend(["-vf".to_owned(), "format=rgb48be".to_owned()]);
        }
        args.extend([
            "-c:v".to_owned(),
            "png".to_owned(),
            "-compression_level".to_owned(),
            "9".to_owned(),
        ]);
    }
    args.push(output.display().to_string());
    args
}

fn base_args() -> Vec<String> {
    vec![
        "-hide_banner".to_owned(),
        "-loglevel".to_owned(),
        "error".to_owned(),
        "-y".to_owned(),
    ]
}

/// A running ffmpeg reading BGRA frames from stdin. `Drop` owns the failure
/// path: an engine that was not explicitly finished is killed and reaped, so
/// every early return between `spawn` and `finish` leaves no child behind.
pub struct Feed {
    /// `None` once the child was reaped (finished or observed exiting).
    child: Option<Child>,
    /// `None` once stdin was closed (finished or observed exiting).
    stdin: Option<ChildStdin>,
    /// Where the engine is writing; sized at finish to budget the wait.
    output: std::path::PathBuf,
}

impl Feed {
    /// Spawn ffmpeg with `args`, stdin piped for frame writes.
    ///
    /// All three streams are named here, stderr included. A tray detaches
    /// its console with `FreeConsole`, which leaves the process's standard
    /// handles pointing at something that no longer exists; a child that
    /// inherited one of those failed to spawn at all (Windows reports
    /// `ERROR_NOT_SUPPORTED`, os error 50), so a photo wrote nothing and a
    /// recording flashed its border and vanished. Nothing reads the
    /// engine's stderr in a tray anyway.
    ///
    /// # Errors
    /// Fails when the engine cannot be spawned or its stdin is unavailable.
    pub fn spawn(
        binary: &crate::ffmpeg::Binary,
        args: &[String],
        output: &Path,
    ) -> io::Result<Self> {
        let mut command = Command::new(&binary.path);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::child::quiet(&mut command);
        let mut child = command.spawn().map_err(|error| {
            // The path belongs in the message: "the request is not
            // supported" alone names neither the engine nor the file.
            io::Error::new(
                error.kind(),
                format!("cannot start {}: {error}", binary.path.display()),
            )
        })?;
        let Some(stdin) = child.stdin.take() else {
            // A child spawned without its stdin still has to be reaped;
            // killing it here keeps the failure path from leaking one.
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("ffmpeg stdin is unavailable"));
        };
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            output: output.to_path_buf(),
        })
    }

    /// Write one BGRA frame.
    ///
    /// # Errors
    /// Fails when the pipe is closed (engine died or exited early).
    pub fn frame(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(child) = self.child.as_mut()
            && child.try_wait()?.is_some()
        {
            // `try_wait` reaped the child; forget both handles so `Drop`
            // does not wait on it a second time.
            self.child = None;
            self.stdin = None;
        }
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(io::Error::other("engine exited before the stream ended"));
        };
        stdin.write_all(data)
    }

    /// Close stdin, wait for the engine, and check its exit status.
    ///
    /// # Errors
    /// Fails on wait errors or a nonzero engine exit code.
    pub fn finish(mut self) -> io::Result<()> {
        drop(self.stdin.take());
        let Some(mut child) = self.child.take() else {
            return Err(io::Error::other("engine exited before the stream ended"));
        };
        // `+faststart` rewrites the whole container, so a large recording
        // legitimately takes a while to close; size the budget from what
        // the engine actually wrote rather than a flat timeout.
        let bytes = std::fs::metadata(&self.output).map_or(0, |meta| meta.len());
        let deadline = Instant::now() + finalize_budget(bytes);
        loop {
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other("engine exited with an error"))
                };
            }
            if Instant::now() >= deadline {
                // Killing the engine releases the pipe, so a capture worker
                // still blocked in a write can leave its loop instead of
                // hanging the stop path behind it.
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other(
                    "the engine did not finish within its time budget",
                ));
            }
            std::thread::sleep(FINISH_POLL);
        }
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        // Closing stdin first gives a healthy engine the chance to flush a
        // final container; an engine still running is then killed rather
        // than leaked as an unreaped child.
        drop(self.stdin.take());
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Stream, record_args, still_args};
    use crate::settings::{StillDepth, StillQuality};
    use std::path::Path;

    /// A named audio pipe adds a second input, the mapping that keeps the
    /// video first, and the AAC encoder for it.
    #[test]
    fn a_sound_recording_names_the_audio_pipe() {
        let stream = Stream {
            width: 1280,
            height: 720,
            fps: 30,
        };
        let args = record_args(
            stream,
            &["-c:v".to_owned(), "libx264".to_owned()],
            None,
            Path::new("out.mp4"),
            Some(super::Sound {
                path: r"\\.\pipe\screencapture-1-1",
                bitrate: "320k",
            }),
        );
        let joined = args.join(" ");
        assert!(joined.contains("-i \\\\.\\pipe\\screencapture-1-1"));
        assert!(joined.contains("-map 0:v:0 -map 1:a:0"));
        assert!(joined.contains("-c:a aac -b:a 320k"));
        assert_eq!(args.last().map(String::as_str), Some("out.mp4"));
    }

    /// A silent recording names no audio input at all: one input, no maps.
    #[test]
    fn a_silent_recording_keeps_one_input() {
        let stream = Stream {
            width: 1280,
            height: 720,
            fps: 30,
        };
        let args = record_args(
            stream,
            &["-c:v".to_owned(), "libx264".to_owned()],
            None,
            Path::new("out.mp4"),
            None,
        );
        let joined = args.join(" ");
        assert!(!joined.contains("-map"));
        assert!(!joined.contains("s16le"));
    }

    /// A spawn the system refuses has to say which engine it tried: the
    /// bare OS text ("the request is not supported") sent the last hunt
    /// through the installer, the settings file, and the capture backend
    /// before anyone looked at the path.
    #[test]
    fn a_refused_spawn_names_the_engine() {
        let binary = crate::ffmpeg::Binary {
            path: std::path::PathBuf::from("screencapture-no-such-engine.exe"),
        };
        let Err(error) = super::Feed::spawn(&binary, &[], Path::new("out.mp4")) else {
            panic!("a missing engine cannot start");
        };
        let text = error.to_string();
        assert!(
            text.contains("screencapture-no-such-engine.exe"),
            "the message names the engine: {text}"
        );
    }

    /// Manual check of the finish budget: an engine that never exits is
    /// killed instead of waited on. Ignored because it burns five seconds.
    #[cfg(windows)]
    #[test]
    #[ignore = "wedges an engine for the whole finish budget"]
    fn a_wedged_engine_is_killed_at_the_budget() {
        let binary = crate::ffmpeg::Binary {
            path: std::path::PathBuf::from("cmd.exe"),
        };
        let feed = super::Feed::spawn(
            &binary,
            &["/C".to_owned(), "ping -n 30 127.0.0.1 > nul".to_owned()],
            Path::new("screencapture-wedged-engine.mp4"),
        )
        .expect("spawn the stand-in engine");
        let started = std::time::Instant::now();
        let result = feed.finish();
        assert!(result.is_err(), "a wedged engine is an error");
        assert!(
            started.elapsed() >= std::time::Duration::from_secs(4),
            "the budget is respected"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(15),
            "the wait is bounded"
        );
    }

    /// The budget floors for small files, scales with the `+faststart`
    /// rewrite, and never grows without bound.
    #[test]
    fn the_finalize_budget_grows_with_the_recording() {
        use std::time::Duration;

        assert_eq!(super::finalize_budget(0), Duration::from_secs(10));
        // 100 MB at the conservative 10 MB/s rewrite rate.
        assert_eq!(super::finalize_budget(100_000_000), Duration::from_secs(20));
        assert_eq!(super::finalize_budget(u64::MAX), Duration::from_secs(300));
    }

    #[test]
    fn stream_describes_the_pipe() {
        let stream = Stream {
            width: 3840,
            height: 2160,
            fps: 60,
        };
        let args = super::input_args(stream);
        assert!(args.contains(&"3840x2160".to_owned()));
        assert!(args.contains(&"60".to_owned()));
    }

    #[test]
    fn rescaling_keeps_the_capture_aspect_ratio() {
        let args = super::record_args(
            Stream {
                width: 1920,
                height: 1200,
                fps: 30,
            },
            &[],
            Some((1152, 720)),
            Path::new("out.mp4"),
            None,
        );
        let index = args
            .iter()
            .position(|arg| arg == "-vf")
            .expect("scaled recordings carry a filter");
        // Exactly the frame the profile allowed at this aspect ratio, and
        // no pad stage filling the unused edges with black.
        assert_eq!(args[index + 1], "scale=1152:720");
        assert_eq!(args.last().map(String::as_str), Some("out.mp4"));
    }

    #[test]
    fn matching_resolutions_skip_the_filter() {
        let args = super::record_args(
            Stream {
                width: 1280,
                height: 720,
                fps: 30,
            },
            &[],
            None,
            Path::new("out.mp4"),
            None,
        );
        assert!(!args.contains(&"-vf".to_owned()));
    }

    #[test]
    fn the_audio_bitrate_follows_the_quality() {
        let quality = crate::audio::Quality::Speech;
        let args = record_args(
            Stream {
                width: 640,
                height: 360,
                fps: 30,
            },
            &[],
            None,
            Path::new("out.mp4"),
            Some(super::Sound {
                path: r"\\.\pipe\screencapture-test",
                bitrate: quality.bitrate(),
            }),
        );
        let index = args
            .iter()
            .position(|arg| arg == "-b:a")
            .expect("a sound recording names a bitrate");
        assert_eq!(args[index + 1], "96k", "speech writes a smaller track");
    }

    #[test]
    fn still_pins_one_frame_with_png() {
        let args = still_args(
            Stream {
                width: 64,
                height: 32,
                fps: 1,
            },
            StillDepth::Eight,
            StillQuality::Png,
            Path::new("out.png"),
        );
        for wanted in [
            "-frames:v",
            "1",
            "-c:v",
            "png",
            "-compression_level",
            "9",
            "out.png",
        ] {
            assert!(args.contains(&wanted.to_owned()));
        }
        assert!(!args.contains(&"-vf".to_owned()));
    }

    #[test]
    fn deep_stills_ask_for_the_png_16_bit_form() {
        let args = still_args(
            Stream {
                width: 64,
                height: 32,
                fps: 1,
            },
            StillDepth::Sixteen,
            StillQuality::Png,
            Path::new("out.png"),
        );
        assert!(args.contains(&"-vf".to_owned()));
        assert!(args.contains(&"format=rgb48be".to_owned()));
        // Compression arguments stay identical so both depths differ
        // only in their pixel form.
        for wanted in ["-c:v", "png", "-compression_level", "9"] {
            assert!(args.contains(&wanted.to_owned()));
        }
    }

    #[test]
    fn a_jpeg_still_asks_for_mjpeg_at_its_step() {
        for (quality, qscale) in [
            (StillQuality::High, "2"),
            (StillQuality::Standard, "4"),
            (StillQuality::Compact, "7"),
        ] {
            let args = still_args(
                Stream {
                    width: 64,
                    height: 32,
                    fps: 1,
                },
                // Depth belongs to the PNG: a JPEG is always 8-bit and
                // must not gain the 48-bit conversion filter.
                StillDepth::Sixteen,
                quality,
                Path::new("out.jpg"),
            );
            assert!(args.contains(&"mjpeg".to_owned()), "{quality:?}");
            let index = args
                .iter()
                .position(|arg| arg == "-q:v")
                .unwrap_or_else(|| panic!("{quality:?} names a qscale"));
            assert_eq!(args[index + 1], qscale, "{quality:?}");
            // 4:4:4 keeps coloured UI text from fringing, which is how a
            // screenshot JPEG is judged.
            assert!(args.contains(&"yuvj444p".to_owned()), "{quality:?}");
            assert!(!args.contains(&"png".to_owned()), "{quality:?}");
            assert!(
                !args.contains(&"-compression_level".to_owned()),
                "{quality:?}"
            );
            assert!(!args.contains(&"format=rgb48be".to_owned()), "{quality:?}");
        }
    }

    /// The lossless WebP arm keeps the pixels but not the PNG container: it
    /// must ask libwebp for lossless mode, or the encoder silently writes a
    /// lossy file and the choice stops meaning what it says.
    #[test]
    fn a_lossless_webp_still_asks_for_webp_lossless() {
        let args = still_args(
            Stream {
                width: 64,
                height: 32,
                fps: 1,
            },
            StillDepth::Sixteen,
            StillQuality::Webp,
            Path::new("out.webp"),
        );
        assert!(args.contains(&"libwebp".to_owned()));
        let index = args
            .iter()
            .position(|arg| arg == "-lossless")
            .expect("lossless mode is named");
        assert_eq!(args[index + 1], "1");
        assert!(args.contains(&"-quality".to_owned()));
        assert!(!args.contains(&"png".to_owned()));
        assert!(!args.contains(&"mjpeg".to_owned()));
        // Depth is the PNG's; libwebp has no 48-bit form.
        assert!(!args.contains(&"format=rgb48be".to_owned()));
    }

    /// BMP is the uncompressed choice: the encoder writes the frame as it
    /// arrives, so no compression or conversion argument belongs in it.
    #[test]
    fn a_bmp_still_asks_for_the_uncompressed_encoder() {
        let args = still_args(
            Stream {
                width: 64,
                height: 32,
                fps: 1,
            },
            StillDepth::Eight,
            StillQuality::Bmp,
            Path::new("out.bmp"),
        );
        assert!(args.contains(&"bmp".to_owned()));
        for absent in ["png", "mjpeg", "libwebp", "-compression_level", "-q:v"] {
            assert!(!args.contains(&absent.to_owned()), "{absent}");
        }
    }
}
