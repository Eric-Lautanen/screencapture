//! Locating and driving the `FFmpeg` engine shipped with the application.

use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
        let output = Command::new(&self.path)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?
            .wait_with_output()?;
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text.lines().next().unwrap_or("").trim().to_owned())
    }
}

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
        let output = Command::new(&self.path)
            .arg("-v")
            .arg("error")
            .arg("-show_entries")
            .arg("stream=nb_frames")
            .arg("-of")
            .arg("csv=p=0")
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?
            .wait_with_output()?;
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
