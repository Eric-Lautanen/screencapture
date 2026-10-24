//! Window capture: Windows Graphics Capture hands over a window's *own*
//! composition frames, so a window that is covered, partly offscreen, or
//! behind another application is recorded as it draws itself instead of as
//! the desktop happens to show it. It is the API Snipping Tool and Game Bar
//! use, and the reason a window capture no longer depends on what is
//! stacked on top of it.
//!
//! The frames arrive through `IDirect3D11CaptureFramePool`, which is polled
//! rather than driven by a callback: no COM event handler, no apartment
//! marshaling, and the grab thread decides the pace. Each polled frame is
//! copied to a CPU-readable staging texture and read like a blit.
//!
//! A window that Windows will not let this process capture (a protected
//! one, or a build before Windows 10 1903) fails at [`Window::open`], which
//! is where the caller falls back to a region blit.

pub mod abi;
mod winrt;

use std::io;
use std::time::{Duration, Instant};

use crate::dxgi::abi::{ContextVt, DeviceVt, TextureVt};
use crate::dxgi::com::Com;
use crate::dxgi::ffi;

use abi::{
    DXGI_DEVICE, DxgiDeviceVt, FrameVt, ITEM_INTEROP, InteropVt, POOL_STATICS, PoolStaticsVt,
    PoolVt, SessionVt,
};

/// How long the first frame may take. The pool answers after the window's
/// next present: immediate for anything that animates, but a quiet window
/// waits for its next repaint, and a still photo must never come out black.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(2);
/// Poll interval while waiting for that first frame.
const FRAME_POLL: Duration = Duration::from_millis(10);

/// One window's live capture: the frames it draws, at the region the user
/// picked.
pub struct Window {
    /// The pool the compositor fills; polled once per grab.
    pool: Com<PoolVt>,
    /// The live session: held only so releasing it (on drop) stops the
    /// capture; everything else goes through the pool.
    _session: Com<SessionVt>,
    /// CPU-readable twin of the newest frame.
    staging: Com<TextureVt>,
    /// The immediate context that copies and locks.
    context: Com<ContextVt>,
    /// The device that owns `staging` and `context`.
    device: Com<DeviceVt>,
    /// Size of the frames the pool currently produces.
    texture: (u32, u32),
    /// Size of the region the caller asked for.
    size: (u32, u32),
    /// Latest frame, compacted to [`Self::size`].
    scratch: Vec<u8>,
    /// Whether any frame has landed yet.
    primed: bool,
}

impl Window {
    /// Start capturing `handle`, matching the `size` region the picker
    /// outlined. Frames closer to `size` than the window's own content are
    /// centered, so a window resized mid-recording keeps a stable frame.
    ///
    /// # Errors
    /// Fails when the graphics stack refuses a device or pool, when the
    /// window is not capturable (protected content, closed, minimized to
    /// nothing), or when the region cannot be allocated.
    pub fn open(handle: usize, size: (u32, u32), include_cursor: bool) -> io::Result<Self> {
        if handle == 0 {
            return Err(io::Error::other("window capture needs a window handle"));
        }
        let bytes = usize::try_from(size.0)
            .ok()
            .zip(usize::try_from(size.1).ok())
            .and_then(|(width, height)| width.checked_mul(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .filter(|bytes| *bytes > 0)
            .ok_or_else(|| io::Error::other("window capture: the region is empty"))?;
        winrt::initialize();
        let factory =
            ffi::spawn_factory().map_err(|error| native("DXGI factory", error.message()))?;
        let adapter = factory
            .adapter1(0)
            .map_err(|hr| native("adapter", code(hr)))?;
        let (device, context) = ffi::spawn_capture_device(&adapter)
            .map_err(|error| native("D3D11 device", error.message()))?;
        let dxgi_device = device
            .query_to::<DxgiDeviceVt>(&DXGI_DEVICE)
            .map_err(|hr| native("DXGI device", code(hr)))?;
        let direct = winrt::direct_device(&dxgi_device)
            .map_err(|error| native("WinRT device", error.message()))?;
        // The item factory is the interop interface itself: asking for it
        // by `GUID` is how a window handle becomes a capture item.
        let interop = winrt::activate::<InteropVt>(winrt::ITEM_CLASS, &ITEM_INTEROP)
            .map_err(|hr| native("capture item factory", code(hr)))?;
        let item = interop
            .item(handle)
            .map_err(|hr| native("capture item", code(hr)))?;
        let measured = item.size().map_err(|hr| native("window size", code(hr)))?;
        if measured.0 == 0 || measured.1 == 0 {
            return Err(io::Error::other(
                "window capture: the window has no drawable size (minimized?)",
            ));
        }
        let statics = winrt::activate::<PoolStaticsVt>(winrt::POOL_CLASS, &POOL_STATICS)
            .map_err(|hr| native("frame pool factory", code(hr)))?;
        let pool = statics
            .pool(&direct, measured)
            .map_err(|hr| native("frame pool", code(hr)))?;
        let session = pool
            .session(&item)
            .map_err(|hr| native("capture session", code(hr)))?;
        // Both switches are best effort, and both predate nothing useful:
        // before Windows 10 2004 the pointer is always drawn into frames
        // (so asking for it still works, muting it does not), and before
        // Windows 11 22H2 the capture outline stays on screen.
        session.set_cursor(include_cursor);
        session.hide_border();
        session
            .start()
            .map_err(|hr| native("start capture", code(hr)))?;
        let staging = device
            .staged(measured.0, measured.1)
            .map_err(|hr| native("staging texture", code(hr)))?;
        Ok(Self {
            pool,
            _session: session,
            staging,
            context,
            device,
            texture: measured,
            size,
            scratch: vec![0; bytes],
            primed: false,
        })
    }

    /// The newest frame, as top-down rows of `size.0 * 4` BGRA bytes. The
    /// bytes are valid until the next call, like every other capture
    /// backend here; a poll with nothing new repeats the previous frame.
    ///
    /// # Errors
    /// Fails when the session ends or the staging copy is refused.
    pub fn frame(&mut self) -> io::Result<&[u8]> {
        if !self.primed {
            self.await_first()?;
        } else if let Some(frame) = self.poll()? {
            self.absorb(frame)?;
        }
        Ok(&self.scratch)
    }

    /// Wait for the first frame, within the budget a still photo can afford.
    fn await_first(&mut self) -> io::Result<()> {
        let deadline = Instant::now() + FIRST_FRAME_WAIT;
        loop {
            if let Some(frame) = self.poll()? {
                self.absorb(frame)?;
                self.primed = true;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other(
                    "window capture: the window produced no frame",
                ));
            }
            std::thread::sleep(FRAME_POLL);
        }
    }

    fn poll(&self) -> io::Result<Option<Com<FrameVt>>> {
        self.pool
            .next_frame()
            .map_err(|hr| native("frame poll", code(hr)))
    }

    /// Copy one polled frame into [`Self::scratch`].
    fn absorb(&mut self, frame: Com<FrameVt>) -> io::Result<()> {
        let measured = frame.size().map_err(|hr| native("frame size", code(hr)))?;
        if measured.0 == 0 || measured.1 == 0 {
            return Err(io::Error::other("window capture: the frame is empty"));
        }
        let texture = frame
            .texture()
            .map_err(|hr| native("frame surface", code(hr)))?;
        // A resized window changes the frame's shape; the staging texture
        // has to follow it, or the copy is refused.
        if measured != self.texture {
            let staging = self
                .device
                .staged(measured.0, measured.1)
                .map_err(|hr| native("staging texture", code(hr)))?;
            self.staging = staging;
            self.texture = measured;
        }
        self.context.clone_resource(&self.staging, &texture);
        drop(texture);
        drop(frame);
        let rows = usize::try_from(self.texture.1).unwrap_or(0);
        let locked = self
            .context
            .lock(&self.staging)
            .map_err(|hr| native("staging lock", code(hr)))?;
        // SAFETY: `rows` is the height of the texture that was locked, the
        // contract `Locked::bytes` asks for.
        let source = unsafe { locked.bytes(rows) };
        copy_center(
            source,
            locked.row_pitch(),
            self.texture,
            self.size,
            &mut self.scratch,
        );
        Ok(())
    }
}

// The session and the pool stop feeding frames when they are released, and
// `Com` releases on drop; nothing else is owed here.

/// Copy `source` into `output`, centering the shapes: the window's frame
/// fills the region the picker outlined, a frame that grew is cropped from
/// its middle, and one that shrank is centered with black around it. The
/// output is always fully written, so no stale pixels survive a resize.
fn copy_center(
    source: &[u8],
    pitch: usize,
    frame: (u32, u32),
    output_size: (u32, u32),
    output: &mut [u8],
) {
    output.fill(0);
    let (frame_width, frame_height) = (usize::try_from(frame.0), usize::try_from(frame.1));
    let (width, height) = (
        usize::try_from(output_size.0),
        usize::try_from(output_size.1),
    );
    let (Ok(frame_width), Ok(frame_height), Ok(width), Ok(height)) =
        (frame_width, frame_height, width, height)
    else {
        return;
    };
    if frame_width == 0 || frame_height == 0 || width == 0 || height == 0 {
        return;
    }
    let columns = frame_width.min(width);
    let rows = frame_height.min(height);
    let left = frame_width.saturating_sub(width) / 2;
    let top = frame_height.saturating_sub(height) / 2;
    let output_left = width.saturating_sub(frame_width) / 2;
    let output_top = height.saturating_sub(frame_height) / 2;
    let span = columns.saturating_mul(4);
    let output_row = width.saturating_mul(4);
    for row in 0..rows {
        let Some(from) = (top + row)
            .checked_mul(pitch)
            .and_then(|base| base.checked_add(left.saturating_mul(4)))
        else {
            break;
        };
        let Some(to) = (output_top + row)
            .checked_mul(output_row)
            .and_then(|base| base.checked_add(output_left.saturating_mul(4)))
        else {
            break;
        };
        let (Some(from_end), Some(to_end)) = (from.checked_add(span), to.checked_add(span)) else {
            break;
        };
        let (Some(source), Some(target)) = (source.get(from..from_end), output.get_mut(to..to_end))
        else {
            break;
        };
        target.copy_from_slice(source);
    }
}

/// One failed native step, named, with the raw code so the log can tell a
/// protected window from a broken driver.
fn native(stage: &str, detail: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("window capture: {stage}: {detail}"))
}

fn code(hr: i32) -> String {
    format!("0x{hr:08X}")
}

#[cfg(test)]
mod tests;
