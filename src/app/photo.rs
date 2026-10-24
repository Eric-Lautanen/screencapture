//! Still-photo capture: grab one frame from the target region, encode it in
//! the images directory at the configured quality (a lossless PNG by
//! default, or a JPEG step), and hand it to the system clipboard when the
//! platform provides a bridge.

use std::io;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crate::capture::{Region, Target};
use crate::clock;
use crate::pipe::{self, Stream};
use crate::settings::{Loaded, StillQuality};

use super::engine;

/// Capture `selection` into `loaded.settings.image_dir` at the configured
/// still quality, and offer it to the clipboard.
///
/// # Errors
/// Fails when the directory cannot be created, the target refuses the frame,
/// or the `FFmpeg` pipe cannot finish the still.
pub(super) fn take_photo(loaded: &Loaded, selection: super::Selection) -> io::Result<Photo> {
    let region = selection.region;
    let quality = loaded.settings.photo_quality;
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
    let mut target = open_target(selection, loaded.settings.cursor)?;
    let frame = target
        .frame()
        .map_err(|error| io::Error::other(format!("cannot grab the screen: {error}")))?;
    let path = clock::stamped_path(folder, quality.extension()).map_err(|error| {
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
    let args = pipe::still_args(stream, loaded.settings.photo_depth, quality, &path);
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
    let clipboard = attach_clipboard(&path, region, frame, quality);
    Ok(Photo {
        path,
        clipboard,
        sound,
    })
}

/// The frame source for a selection: the clicked window's own content
/// where Windows allows it, else the desktop rectangle.
///
/// A window that refuses to be captured alone (protected content) is not a
/// failed photo: the region blit still saves what is on screen, and the log
/// says why the result may show the window on top.
#[cfg(windows)]
fn open_target(selection: super::Selection, cursor: bool) -> io::Result<Target> {
    let Some(handle) = selection.window else {
        return Target::new(selection.region, cursor);
    };
    Target::window(handle, selection.region, cursor).or_else(|error| {
        crate::log::note(
            "capture",
            &format!("window capture unavailable ({error}); saving the screen instead"),
        );
        Target::new(selection.region, cursor)
    })
}

#[cfg(not(windows))]
fn open_target(selection: super::Selection, cursor: bool) -> io::Result<Target> {
    Target::new(selection.region, cursor)
}

/// A finished photo: where the still landed and how the clipboard took it.
pub(super) struct Photo {
    /// The written still (PNG or JPEG, per the quality setting).
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
/// browsers paste it. Only a PNG file is added as the richer flavour; a
/// JPEG still pastes as the bitmap, which every application reads. A
/// refusal is reported, never fatal — the file is the durable answer.
#[cfg(windows)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the platform without a clipboard bridge answers None; the photo type is shared"
)]
fn attach_clipboard(
    path: &Path,
    region: Region,
    frame: &[u8],
    quality: StillQuality,
) -> Option<io::Result<()>> {
    let png = if quality == StillQuality::Png {
        std::fs::read(path).ok()
    } else {
        None
    };
    Some(crate::clipboard::copy_image(
        region.width,
        region.height,
        frame,
        png.as_deref(),
    ))
}

#[cfg(not(windows))]
fn attach_clipboard(
    _path: &Path,
    _region: Region,
    _frame: &[u8],
    _quality: StillQuality,
) -> Option<io::Result<()>> {
    None
}
