//! Locating and driving the `FFmpeg` engine shipped with the application.

use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::recording::Profile;

#[cfg(windows)]
const ENGINE_FILE: &str = "ffmpeg.exe";
#[cfg(not(windows))]
const ENGINE_FILE: &str = "ffmpeg";
#[cfg(windows)]
const PROBE_FILE: &str = "ffprobe.exe";
#[cfg(not(windows))]
const PROBE_FILE: &str = "ffprobe";

/// A located `FFmpeg` executable.
#[derive(Clone, Debug)]
pub struct Binary {
    /// Absolute path, or a bare command name resolved via `PATH`.
    pub path: PathBuf,
}

impl Binary {
    /// Version banner (first line of `ffmpeg -version`).
    ///
    /// # Errors
    /// Returns an error when the engine cannot be spawned or its output
    /// cannot be collected.
    pub fn version(&self) -> io::Result<String> {
        let mut command = Command::new(&self.path);
        command
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::child::quiet(&mut command);
        let output = command.spawn()?.wait_with_output()?;
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text.lines().next().unwrap_or("").trim().to_owned())
    }

    /// [`Self::supports_encoder`], remembered for the rest of the process.
    /// The test is a full synthetic encode and costs hundreds of
    /// milliseconds on a cold engine, which is far too long to pay in the
    /// middle of starting a recording; an accelerator that passed once on
    /// this machine will pass again, and a settings change to a different
    /// codec asks a different question.
    #[must_use]
    pub fn supports_encoder_cached(&self, video: crate::encoder::Video, profile: &Profile) -> bool {
        if let Ok(probes) = ANSWERS.lock()
            && let Some(answer) = probes.iter().find_map(|(path, id, answer)| {
                (path == &self.path && *id == video.id()).then_some(*answer)
            })
        {
            return answer;
        }
        let answer = self.supports_encoder(video, profile);
        if let Ok(mut probes) = ANSWERS.lock() {
            probes.push((self.path.clone(), video.id(), answer));
        }
        answer
    }

    /// True when the engine can complete a short synthetic encode with
    /// `video`'s exact argument set: an accelerator is present, the driver
    /// loads, and every tuner option is accepted. The recorder runs this
    /// for the configured codec and each fallback candidate before the
    /// capture worker starts, so an unusable choice fails cleanly instead
    /// of writing a dead stream.
    #[must_use]
    pub fn supports_encoder(&self, video: crate::encoder::Video, profile: &Profile) -> bool {
        let mut command = Command::new(&self.path);
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=duration=0.2:size=320x240:rate=10",
            ])
            .args(video.args(profile))
            .args(["-f", "null", "-"]);
        crate::child::quiet(&mut command);
        let status = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        matches!(status, Ok(status) if status.success())
    }
}

/// Encoder probes already answered in this process: engine path, codec
/// id, and the answer.
static ANSWERS: Mutex<Vec<(PathBuf, &'static str, bool)>> = Mutex::new(Vec::new());

/// Search configured, bundled, and system locations and return the first
/// usable engine.
#[must_use]
pub fn discover(configured: Option<&Path>) -> Option<Binary> {
    let mut candidates: Vec<(PathBuf, bool)> = Vec::new();
    if let Some(path) = configured {
        candidates.push((path.to_path_buf(), true));
    }
    if let Ok(executable) = env::current_exe()
        && let Some(install_dir) = executable.parent()
    {
        candidates.push((install_dir.join("ffmpeg").join(ENGINE_FILE), true));
    }
    // Developer layout: running from `target/<profile>` inside the repository.
    if let Ok(workdir) = env::current_dir() {
        candidates.push((workdir.join("vendor/ffmpeg").join(ENGINE_FILE), true));
    }
    // Windows never falls back to a bare name: Command would search the
    // current directory and PATH, so a stray ffmpeg.exe beside the
    // working directory could be launched instead of the shipped engine.
    // The installer layout and an explicit [engine] path are the only
    // sources there.
    #[cfg(not(windows))]
    candidates.push((PathBuf::from(ENGINE_FILE), false));

    candidates
        .into_iter()
        .find_map(|(path, verify)| (!verify || path.is_file()).then_some(Binary { path }))
}

/// A located `ffprobe` beside the engine, for post-capture verification.
pub struct Probe {
    /// Absolute path or bare command name, as with the engine.
    pub path: PathBuf,
}

impl Binary {
    /// The probe executable expected beside this engine.
    #[must_use]
    pub fn probe(&self) -> Probe {
        Probe {
            path: self.path.with_file_name(PROBE_FILE),
        }
    }
}

impl Probe {
    /// Frames `ffprobe` reports for `target` (0 for uncountable streams).
    ///
    /// # Errors
    /// Fails when the probe cannot run or refuses the file.
    pub fn frames(&self, target: &Path) -> io::Result<u64> {
        let mut command = Command::new(&self.path);
        command
            .arg("-v")
            .arg("error")
            .arg("-show_entries")
            .arg("stream=nb_frames")
            .arg("-of")
            .arg("csv=p=0")
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::child::quiet(&mut command);
        let output = command.spawn()?.wait_with_output()?;
        if !output.status.success() {
            return Err(io::Error::other("ffprobe refused the file"));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text
            .lines()
            .find_map(|line| line.trim().parse::<u64>().ok())
            .unwrap_or(0))
    }
}
