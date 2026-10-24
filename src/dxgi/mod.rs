//! Windows desktop duplication capture: a `Duplicator` lends full-screen
//! BGRA frames straight from the compositor, far cheaper than a GDI blit
//! and the reason a 4K recording keeps 60 fps wall-clock honest.
//!
//! Policy: duplication carries a plan only when the region equals one
//! unrotated output in the plain BGRA format; everything else (partial
//! regions, HDR panels) falls back to the GDI `capture::Target`. A
//! requested cursor no longer disqualifies duplication: the pointer is
//! composited in software from the duplication pointer stream instead,
//! which keeps the fast path for the common full-screen case.
//!
//! A lost session (`DXGI_ERROR_ACCESS_LOST`, raised by mode switches,
//! desktop transitions, or a competing duplicator) is rebuilt in place;
//! the recording pauses on its last frame meanwhile and only stops when
//! the service never comes back.

use std::io;
use std::time::Duration;

use crate::winapi::Rect;

use crate::capture::Region;
use crate::dxgi::abi::{ContextVt, DeviceVt, DuplicationVt, Output1Vt, TextureVt};
use crate::dxgi::com::Com;
use crate::dxgi::cursor::{Pose, Shape};
use crate::dxgi::policy::{pick_output, report, report_spawn, shape_of, spawn, surface};
use crate::dxgi::structs::{
    ACCESS_LOST, FrameInfo, ID3D_TEXTURE2D, IDXGI_OUTPUT1, NOT_CURRENTLY_AVAILABLE,
};

pub mod abi;
pub mod com;
pub mod cursor;
pub mod ffi;
pub mod policy;
pub mod structs;

/// One recording's frame source: the compositor's desktop copy.
pub struct Duplicator {
    /// The active desktop duplication session, vacant only between a lost
    /// session and its successful rebuild.
    duplication: Option<Com<DuplicationVt>>,
    /// CPU-readable texture holding the latest frame.
    staging: Com<TextureVt>,
    /// The immediate context that copies and locks.
    context: Com<ContextVt>,
    /// The device that owns staging and context; duplication and staging
    /// hold its references, so nothing here dereferences it directly.
    device: Com<DeviceVt>,
    /// Latest frame, compacted to `width * 4` rows of BGRA.
    scratch: Vec<u8>,
    /// Width in pixels.
    width: u32,
    /// Height in pixels.
    height: u32,
    /// The region duplication must keep matching across rebuilds.
    region: Region,
    /// The captured output's desktop-coordinate origin; converts global
    /// cursor positions into frame coordinates.
    origin: (i32, i32),
    /// Whether frames get the pointer painted in (and poses tracked).
    draw_cursor: bool,
    /// Where the pointer is this tick, in frame coordinates.
    pose: Pose,
    /// Latest pointer bitmap the driver handed over, if any.
    cursor_shape: Option<Shape>,
    /// Frame rectangle the last composite painted the pointer into, so a
    /// quiet tick can put the desktop pixels back under it; `None` when
    /// nothing was drawn last tick.
    painted: Option<Rect>,
    /// Consecutive busy rebuild attempts; a runaway duplication service
    /// stops the recording with a message instead of spinning forever.
    busy_spins: u32,
}

impl Duplicator {
    /// Open duplication for a region that is exactly one display output.
    /// The session starts with one best-effort frame grab; a static screen
    /// may leave the first frames blank rather than fail.
    ///
    /// # Errors
    /// Fails when the region is not one whole unrotated output, the pixel
    /// format is not plain BGRA, or the graphics stack refuses anything.
    pub fn open(region: Region, draw_cursor: bool) -> io::Result<Self> {
        let factory = spawn(ffi::spawn_factory)?;
        let adapter = factory.adapter1(0).map_err(|hr| report("adapter", hr))?;
        surface(&adapter)?;
        let (_, origin) = pick_output(&adapter, region)?;
        let (device, context) =
            ffi::spawn_device(&adapter).map_err(|error| report_spawn(&error))?;
        surface(&device)?;
        let mut duplicator = Self {
            duplication: None,
            staging: Com::vacant(),
            context,
            device,
            scratch: Vec::new(),
            width: region.width,
            height: region.height,
            region,
            origin,
            draw_cursor,
            pose: Pose {
                left: 0,
                top: 0,
                visible: false,
            },
            cursor_shape: None,
            painted: None,
            busy_spins: 0,
        };
        duplicator.reconnect()?;
        let staging = duplicator
            .device
            .staged(region.width, region.height)
            .map_err(|hr| report("staging texture", hr))?;
        surface(&staging)?;
        duplicator.staging = staging;
        let bytes = (region.width as usize)
            .checked_mul(region.height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| io::Error::other("capture region is oversized"))?;
        duplicator.scratch = vec![0; bytes];
        // First paint best effort; blank beats stalling the opener when the
        // desktop has been quiet since duplication began.
        let _ = duplicator.refresh(FIRST_PAINT_WAIT);
        Ok(duplicator)
    }

    /// Grab one BGRA frame: fresh when the screen changed since the last
    /// tick, otherwise the previous update again. The bytes are valid until
    /// the next call, exactly like the GDI target's contract.
    ///
    /// # Errors
    /// Fails when the duplication session cannot be rebuilt after a loss,
    /// or the staging lock is refused.
    pub fn frame(&mut self) -> io::Result<&[u8]> {
        self.refresh(IDLE_WAIT)?;
        Ok(&self.scratch)
    }

    /// The lent frame's shape, for stream plumbing.
    #[must_use]
    pub fn shape(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Pull whatever the compositor updated within `wait` milliseconds and
    /// keep the old frame when it stayed quiet. A lost session is dropped
    /// and rebuilt on the spot; the caller keeps the last frame meanwhile.
    ///
    /// # Errors
    /// Fails when the session is unusable or the copy pipeline refuses.
    fn refresh(&mut self, wait: u32) -> io::Result<()> {
        let fresh = self.pull(wait)?;
        if self.draw_cursor {
            if fresh.is_none() {
                // The compositor had nothing new, so `scratch` still holds
                // the previous frame *with* the pointer composited in:
                // drawing again on top of that would darken semi-alpha
                // shapes and XOR-invert monochrome ones tick after tick.
                // Put the desktop back under the old pose first, then
                // re-anchor the pointer from the live cursor (it may have
                // moved on a screen that reports nothing).
                self.restore_pointer();
                self.follow_desktop();
            }
            self.paint_pointer();
        }
        Ok(())
    }

    /// Acquire, copy, and flatten one desktop update.
    ///
    /// # Errors
    /// Fails when the session is unusable or the copy pipeline refuses.
    fn pull(&mut self, wait: u32) -> io::Result<Option<FrameInfo>> {
        if self.duplication.is_none() {
            self.reconnect()?;
        }
        let outcome = match self.duplication.as_ref() {
            Some(session) => session.acquire(wait),
            None => return Ok(None),
        };
        match outcome {
            Ok((Some(resource), info)) => {
                let texture = match resource.query_to::<TextureVt>(&ID3D_TEXTURE2D) {
                    Ok(texture) => texture,
                    Err(hr) => {
                        // The acquired frame transaction is still open; giving
                        // it back keeps the next `AcquireNextFrame` usable
                        // instead of forcing a reconnect for one cast error.
                        if let Some(session) = self.duplication.as_ref() {
                            let _ = session.release_frame();
                        }
                        return Err(report("texture", hr));
                    }
                };
                self.context.clone_resource(&self.staging, &texture);
                if let Some(session) = self.duplication.as_ref() {
                    if self.draw_cursor {
                        // Pointer bookkeeping happens while the desktop
                        // frame is held, exactly like Microsoft's sample:
                        // the shape call pairs with an acquired frame.
                        self.pose = Pose {
                            left: info.pointer.position.x,
                            top: info.pointer.position.y,
                            visible: info.pointer.visible != 0,
                        };
                        fetch_shape(&mut self.cursor_shape, session, info.shape_budget);
                    }
                    // Release the desktop frame before the slow staging
                    // read, so the compositor is free well before `Map`
                    // stalls on the GPU queue.
                    session
                        .release_frame()
                        .map_err(|hr| report("release frame", hr))?;
                }
                let mapped = self
                    .context
                    .lock(&self.staging)
                    .map_err(|hr| report("staging lock", hr))?;
                let rows = self.height as usize;
                let width_row = self.width as usize * 4;
                // SAFETY: `staging` was created with this duplicator's
                // height, so exactly `rows` rows are mapped.
                let source = unsafe { mapped.bytes(rows) };
                flatten(
                    rows,
                    width_row,
                    mapped.row_pitch(),
                    source,
                    &mut self.scratch,
                );
                Ok(Some(info))
            }
            Ok((None, _)) => Ok(None),
            Err(hr) if hr == ACCESS_LOST => {
                // Mode switch, desktop transition, or a competing
                // duplicator invalidated the session. Drop it (the driver
                // refuses a second duplication while the stale one lives)
                // and rebuild; while that is pending the recording keeps
                // serving its last frame.
                self.duplication = None;
                self.reconnect()?;
                Ok(None)
            }
            Err(hr) => Err(report("acquire", hr)),
        }
    }

    /// Rebuild the duplication session, retrying while the service reports
    /// transient unavailability, up to a fixed budget of attempts.
    ///
    /// # Errors
    /// Fatal disconnects propagate; a stubborn busy service fails the
    /// caller once the budget runs out.
    fn reconnect(&mut self) -> io::Result<()> {
        loop {
            match self.connect() {
                Ok(()) => {
                    self.busy_spins = 0;
                    return Ok(());
                }
                Err(Retake::Busy) => {
                    self.busy_spins += 1;
                    if self.busy_spins > RETAKE_SPINS {
                        return Err(io::Error::other(
                            "the duplication service stayed unavailable; the session could not be rebuilt",
                        ));
                    }
                    std::thread::sleep(RETAKE_PAUSE);
                }
                Err(Retake::Dead(error)) => return Err(error),
            }
        }
    }

    /// One re-duplication attempt against the current display arrangement,
    /// reusing the device (mode switches do not remove it; device removal
    /// has its own error code and stops the recording outright).
    fn connect(&mut self) -> Result<(), Retake> {
        let factory = spawn(ffi::spawn_factory).map_err(Retake::Dead)?;
        let adapter = factory
            .adapter1(0)
            .map_err(|hr| Retake::Dead(report("adapter", hr)))?;
        surface(&adapter).map_err(Retake::Dead)?;
        let (output, origin) = pick_output(&adapter, self.region).map_err(Retake::Dead)?;
        let output1 = output
            .query_to::<Output1Vt>(&IDXGI_OUTPUT1)
            .map_err(|hr| Retake::Dead(report("output", hr)))?;
        surface(&output1).map_err(Retake::Dead)?;
        let duplication = output1.duplication(&self.device).map_err(|hr| match hr {
            NOT_CURRENTLY_AVAILABLE => Retake::Busy,
            _ => Retake::Dead(report("duplication", hr)),
        })?;
        surface(&duplication).map_err(Retake::Dead)?;
        shape_of(&duplication, self.region).map_err(Retake::Dead)?;
        self.origin = origin;
        self.duplication = Some(duplication);
        Ok(())
    }

    /// Re-anchor the pointer from the live desktop cursor; used on quiet
    /// screens, where duplication reports nothing but the pointer moves.
    fn follow_desktop(&mut self) {
        if let Some((x, y)) = cursor::desktop_cursor_position() {
            self.pose = Pose {
                left: x - self.origin.0,
                top: y - self.origin.1,
                visible: true,
            };
        } else {
            self.pose.visible = false;
        }
    }

    /// Composite the tracked pointer onto the current scratch frame and
    /// remember the frame rectangle it touched.
    fn paint_pointer(&mut self) {
        self.painted = match self.cursor_shape.as_ref() {
            Some(shape) => {
                cursor::apply(&mut self.scratch, self.width, self.height, self.pose, shape)
            }
            None => None,
        };
    }

    /// Put the pristine desktop pixels back where the last composite
    /// painted the pointer. The staging texture still holds the desktop
    /// image the frame was flattened from, so the repair is a row copy of
    /// the pointer's own rectangle.
    fn restore_pointer(&mut self) {
        let Some(area) = self.painted else {
            return;
        };
        let Ok(mapped) = self.context.lock(&self.staging) else {
            // Keep the rectangle: a later quiet tick retries the repair
            // instead of painting over its own previous composite.
            return;
        };
        let rows = self.height as usize;
        let width_row = self.width as usize * 4;
        // SAFETY: `staging` was created with this duplicator's height, so
        // exactly `rows` rows are mapped.
        let source = unsafe { mapped.bytes(rows) };
        restore(
            rows,
            width_row,
            mapped.row_pitch(),
            source,
            area,
            &mut self.scratch,
        );
        self.painted = None;
    }
}

/// Pull the pointer bitmap the driver owes while the desktop frame is
/// still acquired: either the announced pending shape (`budget` bytes) or,
/// when nothing was announced and nothing is cached, the current one via
/// the documented size-probe protocol (an empty buffer answers with the
/// required byte count). Failures hide the cursor rather than touch the
/// recording.
fn fetch_shape(cache: &mut Option<Shape>, session: &Com<DuplicationVt>, budget: u32) {
    let budget = if budget > 0 && budget as usize <= SHAPE_CAP {
        budget
    } else if budget == 0 && cache.is_some() {
        return; // no new shape event; keep the cached bitmap
    } else if cache.is_none() {
        match session.pointer_shape_size() {
            Ok(size) if size as usize <= SHAPE_CAP => size,
            _ => {
                *cache = None;
                return;
            }
        }
    } else {
        // An announced budget beyond the cap counts as garbage.
        *cache = None;
        return;
    };
    let mut bytes = vec![0_u8; budget as usize];
    match session.pointer_shape(&mut bytes) {
        Ok(fetched) => {
            *cache = Some(Shape {
                bytes,
                kind: fetched.kind,
                width: fetched.width,
                height: fetched.height,
                pitch: fetched.pitch,
            });
        }
        Err(_) => *cache = None,
    }
}

/// Copy `pitch`-spaced BGRA rows onto tight `width_row` rows, zero-padding
/// whatever the driver left beyond the row so lent frames stay stable.
///
/// Pure byte work over slices: the one raw-pointer step (mapped memory to
/// a slice) lives in [`crate::dxgi::com::Locked::bytes`], so the arithmetic
/// here is safe and testable on plain buffers.
fn flatten(rows: usize, width_row: usize, pitch: usize, source: &[u8], output: &mut [u8]) {
    output.fill(0);
    for row in 0..rows {
        let (Some(begin), Some(target)) = (row.checked_mul(pitch), row.checked_mul(width_row))
        else {
            break;
        };
        let (Some(end), Some(target_end)) =
            (begin.checked_add(width_row), target.checked_add(width_row))
        else {
            break;
        };
        if end > source.len() || target_end > output.len() {
            break;
        }
        output[target..target_end].copy_from_slice(&source[begin..end]);
    }
}

/// Put the desktop pixels inside `area` back into `output` from the locked
/// staging rows: the undo of one pointer composite. The rectangle comes
/// from [`cursor::apply`], so it is already clipped to the frame; the
/// bounds here only keep a nonsensical rectangle from touching anything.
fn restore(
    rows: usize,
    width_row: usize,
    pitch: usize,
    source: &[u8],
    area: Rect,
    output: &mut [u8],
) {
    let pixels = width_row / 4;
    let left = usize::try_from(area.left.max(0)).unwrap_or(0).min(pixels);
    let right = usize::try_from(area.right.max(0)).unwrap_or(0).min(pixels);
    let top = usize::try_from(area.top.max(0)).unwrap_or(0).min(rows);
    let bottom = usize::try_from(area.bottom.max(0)).unwrap_or(0).min(rows);
    if left >= right || top >= bottom {
        return;
    }
    let column = left * 4;
    let span = (right - left) * 4;
    for row in top..bottom {
        let (Some(begin), Some(target)) = (
            row.checked_mul(pitch)
                .and_then(|base| base.checked_add(column)),
            row.checked_mul(width_row)
                .and_then(|base| base.checked_add(column)),
        ) else {
            break;
        };
        let (Some(end), Some(target_end)) = (begin.checked_add(span), target.checked_add(span))
        else {
            break;
        };
        if end > source.len() || target_end > output.len() {
            break;
        }
        output[target..target_end].copy_from_slice(&source[begin..end]);
    }
}

/// Whether the region is a candidate for a duplication attempt (any non-empty
/// region; `open` and `pick_output` then check whether it matches one whole
/// unrotated BGRA output and fall back to GDI when it does not). Pointer
/// drawing is not a disqualifier; the pointer is composited in software from
/// the duplication pointer stream.
#[must_use]
pub fn eligible(region: Region) -> bool {
    region.is_sane()
}

/// Probe duplication once for diagnostics: the first output's shape.
///
/// # Errors
/// Returns whatever `open` would refuse.
pub fn probe() -> io::Result<(u32, u32)> {
    let display = crate::screen::primary()?;
    let region = Region {
        x: display.x,
        y: display.y,
        width: display.width,
        height: display.height,
    };
    let duplicator = Duplicator::open(region, false)?;
    Ok(duplicator.shape())
}

const FIRST_PAINT_WAIT: u32 = 50;
/// Non-blocking grab: the tick loop owns the pacing, duplication never
/// sleeps the pump.
const IDLE_WAIT: u32 = 0;
/// Rebuild attempts before a busy duplication service stops the recording
/// (~1.2 s of tolerance at 20 ms pauses).
const RETAKE_SPINS: u32 = 60;
/// Pause between busy rebuild attempts.
const RETAKE_PAUSE: Duration = Duration::from_millis(20);
/// Upper bound for a pointer-shape buffer (drivers ship cursors far below
/// a megabyte; the cap just refuses garbage budgets).
const SHAPE_CAP: usize = 4 << 20;

/// What one re-duplication attempt means for the session.
enum Retake {
    /// The service is mid-transition; retry on a later attempt.
    Busy,
    /// Recording cannot continue; surface the reason.
    Dead(io::Error),
}

#[cfg(test)]
mod tests {
    use super::{eligible, flatten, restore};
    use crate::capture::Region;
    use crate::winapi::Rect;

    #[test]
    fn whole_outputs_stay_eligible() {
        let screen = Region {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        assert!(eligible(screen));
        assert!(!eligible(Region { width: 0, ..screen }));
    }

    #[test]
    fn flatten_compacts_driver_padding() {
        let width_row = 8;
        let rows = 2;
        let mut source = vec![0u8; 32];
        source[0..4].copy_from_slice(&[10, 20, 30, 255]);
        source[16..20].copy_from_slice(&[70, 80, 90, 255]);
        let mut output = vec![1u8; 16];
        flatten(rows, width_row, 16, &source, &mut output);
        assert_eq!(
            output,
            [10, 20, 30, 255, 0, 0, 0, 0, 70, 80, 90, 255, 0, 0, 0, 0]
        );
    }

    /// The pointer composite is undone by copying its rectangle back from
    /// the staging image; pixels outside the rectangle keep whatever the
    /// painted frame held.
    #[test]
    fn restore_puts_the_desktop_back_under_the_pointer() {
        let width_row = 8;
        let rows = 2;
        // Padded staging rows: two 2-pixel rows of desktop pixels.
        let mut source = vec![0u8; 32];
        source[0..4].copy_from_slice(&[10, 20, 30, 255]);
        source[4..8].copy_from_slice(&[11, 21, 31, 255]);
        source[16..20].copy_from_slice(&[70, 80, 90, 255]);
        source[20..24].copy_from_slice(&[71, 81, 91, 255]);
        // The frame as the pointer composite left it.
        let mut output = vec![0xAAu8; 16];
        let area = Rect {
            left: 1,
            top: 0,
            right: 2,
            bottom: 2,
        };
        restore(rows, width_row, 16, &source, area, &mut output);
        assert_eq!(
            output,
            [
                0xAA, 0xAA, 0xAA, 0xAA, 11, 21, 31, 255, // first row, column 1
                0xAA, 0xAA, 0xAA, 0xAA, 71, 81, 91, 255, // second row, column 1
            ]
        );
    }

    /// A rectangle entirely outside the frame, or an empty one, touches
    /// nothing.
    #[test]
    fn restore_ignores_nonsense_rectangles() {
        let source = vec![9u8; 32];
        let mut output = vec![0xAAu8; 16];
        for area in [
            Rect {
                left: 5,
                top: 0,
                right: 2,
                bottom: 2,
            },
            Rect {
                left: -4,
                top: -4,
                right: -1,
                bottom: -1,
            },
            Rect::default(),
        ] {
            restore(2, 8, 16, &source, area, &mut output);
        }
        assert_eq!(output, vec![0xAA; 16]);
        restore(
            2,
            8,
            16,
            &[],
            Rect {
                left: 0,
                top: 0,
                right: 2,
                bottom: 2,
            },
            &mut output,
        );
        assert_eq!(output, vec![0xAA; 16]);
    }

    /// A mapping shorter than the declared frame (a driver that trimmed
    /// its staging rows) copies what exists and leaves the rest black
    /// instead of reading past the buffer.
    #[test]
    fn flatten_stops_at_a_short_mapping() {
        let source = vec![0x11u8; 8];
        let mut output = vec![0xAAu8; 16];
        flatten(2, 8, 8, &source, &mut output);
        assert_eq!(
            output,
            [
                0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
    }
}
