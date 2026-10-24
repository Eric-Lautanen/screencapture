//! Still-photo capture: grab one frame from the target region, encode it to a
//! lossless PNG in the images directory, and hand it to the system clipboard
//! when the platform provides a bridge.

use std::io;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crate::capture::{Region, Target};
use crate::clock;
use crate::pipe::{self, Stream};
use crate::settings::Loaded;

use super::engine;

/// Capture `region` as a lossless PNG in `loaded.settings.image_dir` and
/// offer it to the clipboard.
///
/// # Errors
/// Fails when the directory cannot be created, the target refuses the frame,
/// or the `FFmpeg` pipe cannot finish the PNG.
pub(super) fn take_photo(loaded: &Loaded, region: Region) -> io::Result<Photo> {
    // The chime is the audible "got it", so it starts the instant the
    // selection is committed — before the frame grab and the encoder spawn
    // can add their latency. Its own thread keeps the capture path walking.
    let sound = crate::sound::capture_chime(loaded.settings.chimes);
    let binary = engine(loaded)?;
    let folder = &loaded.settings.image_dir;
    // Each step names itself: a bare OS code (a refused handle, a folder
    // the shell keeps in the cloud) otherwise reads as "the photo failed"
    // with no way to tell where.
    std::fs::create_dir_all(folder)
        .map_err(|error| io::Error::other(format!("cannot use {}: {error}", folder.display())))?;
    let target = Target::new(region, loaded.settings.cursor)?;
    let frame = target
        .frame()
        .map_err(|error| io::Error::other(format!("cannot grab the screen: {error}")))?;
    let path = clock::stamped_path(folder, "png").map_err(|error| {
        io::Error::other(format!(
            "cannot create a file in {}: {error}",
            folder.display()
        ))
    })?;
    let stream = Stream {
        width: region.width,
        height: region.height,
        fps: 1,
    };
    let args = pipe::still_args(stream, loaded.settings.photo_depth, &path);
    let encoded = pipe::Feed::spawn(&binary, &args, &path)
        .map_err(|error| io::Error::other(format!("cannot start the encoder: {error}")))
        .and_then(|mut feed| {
            feed.frame(frame)?;
            feed.finish()
        });
    if let Err(error) = encoded {
        // The name was reserved with an empty file; a failed encode must
        // not leave that placeholder behind.
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    let clipboard = attach_clipboard(&path, region, frame);
    Ok(Photo {
        path,
        clipboard,
        sound,
    })
}

/// A finished photo: where the PNG landed and how the clipboard took it.
pub(super) struct Photo {
    /// The written PNG file.
    pub(super) path: PathBuf,
    /// The clipboard answer, or `None` on a platform without a clipboard
    /// bridge (where the file is the whole result).
    pub(super) clipboard: Option<io::Result<()>>,
    /// The confirmation chime started at capture time, still playing on
    /// its worker thread.
    sound: Option<JoinHandle<()>>,
}

impl Photo {
    /// Wait for the capture chime when the process is about to exit (the
    /// command line); the tray lets it finish on its own thread.
    pub(super) fn wait_for_sound(&mut self) {
        if let Some(handle) = self.sound.take() {
            let _ = handle.join();
        }
    }

    /// One line naming the file and the clipboard outcome, so the tray log
    /// and the command line both say whether a paste is ready.
    pub(super) fn describe(&self) -> String {
        match &self.clipboard {
            None => self.path.display().to_string(),
            Some(Ok(())) => format!("{} (copied to clipboard)", self.path.display()),
            Some(Err(error)) => {
                format!("{} (clipboard refused: {error})", self.path.display())
            }
        }
    }
}

/// Offer the finished photo to the system clipboard: the captured frame
/// as a bitmap plus the PNG already on disk, so both office suites and
/// browsers paste it. A refusal is reported, never fatal — the file is
/// the durable answer.
#[cfg(windows)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the platform without a clipboard bridge answers None; the photo type is shared"
)]
fn attach_clipboard(path: &Path, region: Region, frame: &[u8]) -> Option<io::Result<()>> {
    let png = std::fs::read(path).ok();
    Some(crate::clipboard::copy_image(
        region.width,
        region.height,
        frame,
        png.as_deref(),
    ))
}

#[cfg(not(windows))]
fn attach_clipboard(_path: &Path, _region: Region, _frame: &[u8]) -> Option<io::Result<()>> {
    None
}
