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

use std::ffi::c_void;
use std::io;
use std::time::Duration;

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
            busy_spins: 0,
        };
        duplicator.reconnect()?;
        let staging = duplicator
            .device
            .staged(region.width, region.height)
            .map_err(|hr| report("staging texture", hr))?;
        surface(&staging)?;
        duplicator.staging = staging;
        duplicator.scratch = vec![0; region.width as usize * region.height as usize * 4];
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
            // A quiet screen reports nothing, yet the pointer still
            // moves: re-anchor it from the live desktop cursor.
            if fresh.is_none() {
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
                let texture = resource
                    .query_to::<TextureVt>(&ID3D_TEXTURE2D)
                    .map_err(|hr| report("texture", hr))?;
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
                self.absorb(mapped);
                self.context.unlock(&self.staging);
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
        let mut state = CursorState {
            size: u32::try_from(std::mem::size_of::<CursorState>()).unwrap_or(0),
            flags: 0,
            cursor: std::ptr::null_mut(),
            position: crate::winapi::Point::default(),
        };
        let shown = unsafe { GetCursorInfo(&raw mut state) } != 0
            && state.flags & CURSOR_SHOWING != 0
            && state.flags & CURSOR_SUPPRESSED == 0;
        if shown {
            self.pose = Pose {
                left: state.position.x - self.origin.0,
                top: state.position.y - self.origin.1,
                visible: true,
            };
        } else {
            self.pose.visible = false;
        }
    }

    /// Composite the tracked pointer onto the current scratch frame.
    fn paint_pointer(&mut self) {
        let Some(shape) = self.cursor_shape.as_ref() else {
            return;
        };
        cursor::apply(&mut self.scratch, self.width, self.height, self.pose, shape);
    }

    /// Flatten the locked staging rows into the scratch frame, so every
    /// frame lent out stays a plain `width * 4` BGRA layout.
    fn absorb(&mut self, mapped: crate::dxgi::structs::Mapped) {
        let width_row = self.width as usize * 4;
        flatten(
            self.height as usize,
            width_row,
            mapped.row_pitch as usize,
            mapped.data,
            &mut self.scratch,
        );
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
fn flatten(rows: usize, width_row: usize, pitch: usize, data: *mut c_void, output: &mut [u8]) {
    output.fill(0);
    if data.is_null() {
        return;
    }
    // SAFETY: `data` points at `pitch * rows` bytes the driver mapped for
    // us until `unlock`; the bounds here follow the API contract.
    let source =
        unsafe { std::slice::from_raw_parts(data.cast::<u8>(), pitch.saturating_mul(rows)) };
    if pitch == width_row {
        output.copy_from_slice(&source[..width_row * rows]);
        return;
    }
    for row in 0..rows {
        let begin = row * pitch;
        let end = begin + width_row;
        if end > source.len() {
            break;
        }
        output[row * width_row..row * width_row + width_row].copy_from_slice(&source[begin..end]);
    }
}

/// Whether duplication should ever carry this plan (audio of the policy):
/// one whole unrotated output. Pointer drawing is not a disqualifier; the
/// pointer is composited in software from the duplication pointer stream.
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

// ---------------------------------------------------------------------------
// Live desktop cursor look-up (`user32`): duplication only reports pointer
// moves alongside acquired frames, so quiet screens re-anchor the pointer
// from the shell's own cursor state.
// ---------------------------------------------------------------------------

/// `CURSORINFO`.
#[repr(C)]
struct CursorState {
    size: u32,
    flags: u32,
    cursor: *mut c_void,
    position: crate::winapi::Point,
}

/// `CURSOR_SHOWING`: the cursor is being drawn.
const CURSOR_SHOWING: u32 = 1;
/// `CURSOR_SUPPRESSED`: the shell hides it (tablet mode, touch input).
const CURSOR_SUPPRESSED: u32 = 2;

#[link(name = "user32")]
unsafe extern "system" {
    fn GetCursorInfo(state: *mut CursorState) -> i32;
}

#[cfg(test)]
mod tests {
    use super::{eligible, flatten};
    use crate::capture::Region;

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
        let data = source.as_mut_ptr().cast::<std::ffi::c_void>();
        flatten(rows, width_row, 16, data, &mut output);
        assert_eq!(
            output,
            [10, 20, 30, 255, 0, 0, 0, 0, 70, 80, 90, 255, 0, 0, 0, 0]
        );
    }
}
