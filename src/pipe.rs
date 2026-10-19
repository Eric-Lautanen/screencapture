//! `FFmpeg` frame-pipe jobs: raw BGRA frames streamed into stdin for both
//! recordings and still photos; the process outlives one call.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

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

/// Full argument vector for a recording; `scale` resizes when the capture
/// resolution differs from the profile.
#[must_use]
pub fn record_args(
    stream: Stream,
    codec_args: &[String],
    scale: Option<(u32, u32)>,
    output: &Path,
) -> Vec<String> {
    let mut args = base_args();
    args.extend(input_args(stream));
    args.extend(["-i".to_owned(), "pipe:0".to_owned()]);
    if let Some((width, height)) = scale {
        args.extend(["-vf".to_owned(), format!("scale={width}:{height}")]);
    }
    args.extend(codec_args.iter().cloned());
    args.push(output.display().to_string());
    args
}

/// Full argument vector for one still photo (lossless PNG).
#[must_use]
pub fn still_args(stream: Stream, output: &Path) -> Vec<String> {
    let mut args = base_args();
    args.extend(input_args(stream));
    args.extend([
        "-i".to_owned(),
        "pipe:0".to_owned(),
        "-frames:v".to_owned(),
        "1".to_owned(),
        "-c:v".to_owned(),
        "png".to_owned(),
        "-compression_level".to_owned(),
        "9".to_owned(),
    ]);
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

/// A running ffmpeg reading BGRA frames from stdin.
pub struct Feed {
    child: Child,
    stdin: ChildStdin,
}

impl Feed {
    /// Spawn ffmpeg with `args`, stdin piped for frame writes.
    ///
    /// # Errors
    /// Fails when the engine cannot be spawned or its stdin is unavailable.
    pub fn spawn(binary: &crate::ffmpeg::Binary, args: &[String]) -> io::Result<Self> {
        let mut command = Command::new(&binary.path);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("ffmpeg stdin is unavailable"))?;
        Ok(Self { child, stdin })
    }

    /// Write one BGRA frame.
    ///
    /// # Errors
    /// Fails when the pipe is closed (engine died or exited early).
    pub fn frame(&mut self, data: &[u8]) -> io::Result<()> {
        if self.child.try_wait()?.is_some() {
            return Err(io::Error::other("engine exited before the stream ended"));
        }
        self.stdin.write_all(data)
    }

    /// Close stdin, wait for the engine, and check its exit status.
    ///
    /// # Errors
    /// Fails on wait errors or a nonzero engine exit code.
    pub fn finish(self) -> io::Result<()> {
        let Self { mut child, stdin } = self;
        drop(stdin);
        let status = child.wait()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("engine exited with an error"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Stream, still_args};
    use std::path::Path;

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
    fn still_pins_one_frame_with_png() {
        let args = still_args(
            Stream {
                width: 64,
                height: 32,
                fps: 1,
            },
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
    }
}
