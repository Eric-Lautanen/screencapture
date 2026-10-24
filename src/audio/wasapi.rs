//! `WASAPI` capture clients: one shared-mode stream per source, polled from
//! the audio worker.
//!
//! Both sources are asked for the same 48 kHz stereo 16-bit format with the
//! engine's own converters enabled (`AUTOCONVERTPCM`), so a 44.1 kHz headset
//! and a 48 kHz speaker mix need no resampling here: mixing two streams is
//! then a saturating add. Nothing in this module blocks except the calls
//! themselves, and no event handles or COM callbacks are involved, so the
//! worker can poll at whatever pace the mixer wants.

use std::ffi::c_void;
use std::io;

use super::abi::{
    AUDIO_CLIENT, AUTOCONVERT, AudioClientVt, BUFFER_100MS, CAPTURE_CLIENT, CLSCTX_ALL,
    CaptureClientVt, DEVICE_INVALIDATED, DeviceVt, EnumeratorVt, LOOPBACK, RENDER, ROLE_CONSOLE,
    ROLE_MULTIMEDIA, SHARED, SILENT, SRC_DEFAULT_QUALITY, WaveFormatExtensible, stereo_pcm,
};
use crate::dxgi::abi::Complete;
use crate::dxgi::com::Com;
use crate::dxgi::structs::Guid;

/// `COINIT_MULTITHREADED`: the worker polls objects it created itself, so
/// there is no apartment state worth marshaling.
const MULTITHREADED: u32 = 0x0;

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, model: u32) -> i32;
    fn CoCreateInstance(
        class: *const Guid,
        outer: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> i32;
}

/// One live capture stream: the client that runs it and the port that hands
/// its packets over.
pub(super) struct Client {
    /// The stream: started once, stopped on drop of the owner.
    stream: Com<AudioClientVt>,
    /// Where the packets come from.
    packets: Com<CaptureClientVt>,
    /// Whether this is the Windows default because the endpoint the take
    /// preferred was not there. A source on its fallback moves over as soon
    /// as the chosen device comes back.
    fallback: bool,
}

// The client is opened on the recording's setup thread and then moved to
// the audio worker, which is the only thread that ever polls it. `COM`
// handles created in the multithreaded apartment may be used from another
// thread without marshaling, and no two threads ever hold this one.
unsafe impl Send for Client {}

/// Join the process-wide multithreaded apartment. Every thread that touches
/// a capture client has to make this call first: the clients are activated
/// on the recording's setup thread and polled from the audio worker, and
/// only an initialized apartment may call into them.
///
/// # Safety
/// Safe to call from any thread; `S_FALSE` (already in this apartment) and
/// `RPC_E_CHANGED_MODE` (already in another one) are both fine, because an
/// object created in the multithreaded apartment is free-threaded.
pub(super) fn apartment() {
    // SAFETY: a thread-local apartment request with no pointers or state.
    let _ = unsafe { CoInitializeEx(std::ptr::null_mut(), MULTITHREADED) };
}

impl Client {
    /// Open `flow` ([`RENDER`] for system audio, [`CAPTURE`] for the
    /// microphone) on the endpoint `preferred` names, or on the Windows
    /// default when it names none or that device is not connected.
    ///
    /// # Errors
    /// Fails when the device cannot be opened or refuses the shared-mode
    /// format; the caller logs it and records without that source.
    pub(super) fn open(flow: i32, preferred: Option<&str>) -> io::Result<Self> {
        apartment();
        let (device, fallback) = match preferred {
            Some(id) if super::devices::present(id) => (super::devices::find(id)?, false),
            Some(_) => {
                crate::log::note(
                    "audio",
                    "the chosen device is not connected; using the Windows default",
                );
                (super::devices::default_device(flow)?, true)
            }
            None => (super::devices::default_device(flow)?, false),
        };
        let stream = device.capture_client()?;
        stream.initialize(flow)?;
        let packets = stream.capture_service()?;
        Ok(Self {
            stream,
            packets,
            fallback,
        })
    }

    /// Whether this stream is the Windows default standing in for a chosen
    /// device that was away.
    pub(super) const fn on_fallback(&self) -> bool {
        self.fallback
    }

    /// Begin delivering packets.
    ///
    /// # Errors
    /// Fails when the engine refuses to run the stream (an endpoint that
    /// was removed between opening and starting, a session the device
    /// will not grant). The caller logs it: a source that never started
    /// would otherwise be a silent track with no explanation.
    pub(super) fn start(&self) -> io::Result<()> {
        let Some(table) = self.stream.view() else {
            return Err(io::Error::other("WASAPI: the audio client went away"));
        };
        // SAFETY: live stream interface; slot 10 takes just the object.
        let hr = unsafe { (table.start)(self.stream.raw()) };
        if hr < 0 {
            return Err(native("stream start", hr));
        }
        Ok(())
    }

    /// Stop delivering packets; the stream is released with the client.
    pub(super) fn stop(&self) {
        let Some(table) = self.stream.view() else {
            return;
        };
        // SAFETY: live stream interface; slot 11 takes just the object.
        let _ = unsafe { (table.stop)(self.stream.raw()) };
    }

    /// Append every packet that is ready as interleaved 16-bit samples.
    ///
    /// # Errors
    /// Fails when the stream reports an error, including the endpoint being
    /// removed mid-recording; the caller stops this source and keeps the
    /// audio it already wrote.
    pub(super) fn drain(&self, out: &mut Vec<i16>) -> io::Result<()> {
        let Some(table) = self.packets.view() else {
            return Err(io::Error::other("WASAPI: the capture port went away"));
        };
        loop {
            let mut frames = 0_u32;
            // SAFETY: live port interface and a local out slot.
            let hr = unsafe { (table.get_next_packet_size)(self.packets.raw(), &raw mut frames) };
            if hr < 0 {
                return Err(native("packet size", hr));
            }
            if frames == 0 {
                return Ok(());
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut flags = 0_u32;
            let mut position = 0_u64;
            let mut timestamp = 0_u64;
            // SAFETY: live port interface; every out slot is a local the
            // call writes, and `frames` is the count asked for above.
            let hr = unsafe {
                (table.get_buffer)(
                    self.packets.raw(),
                    &raw mut data,
                    &raw mut frames,
                    &raw mut flags,
                    &raw mut position,
                    &raw mut timestamp,
                )
            };
            if hr < 0 {
                return Err(native("packet", hr));
            }
            if frames > 0 {
                let samples = usize::try_from(frames)
                    .unwrap_or(0)
                    .saturating_mul(usize::from(super::abi::CHANNELS));
                // SAFETY: the port lent `frames` frames of 16-bit
                // interleaved samples for this buffer, valid until the
                // release below. A silent packet may carry no buffer at
                // all, which is not a slice to read.
                let bytes = if data.is_null() {
                    None
                } else {
                    Some(unsafe { std::slice::from_raw_parts(data, samples.saturating_mul(2)) })
                };
                append_packet(out, bytes, frames, flags);
                // SAFETY: hands back the same frame count the port lent.
                let _ = unsafe { (table.release_buffer)(self.packets.raw(), frames) };
            }
        }
    }
}

/// Append one packet to the queue as interleaved 16-bit samples.
///
/// A packet the engine marks silent still owns its duration: appending the
/// silence keeps the stream's timeline equal to the recording's clock,
/// whether or not the port lent a buffer for it. Dropping it instead would
/// pull every later sample earlier against the video.
pub(super) fn append_packet(out: &mut Vec<i16>, bytes: Option<&[u8]>, frames: u32, flags: u32) {
    let samples = usize::try_from(frames)
        .unwrap_or(0)
        .saturating_mul(usize::from(super::abi::CHANNELS));
    match bytes {
        Some(bytes) if flags & SILENT == 0 => out.extend(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[low, high]| i16::from_le_bytes([*low, *high])),
        ),
        // Silence: a flagged packet, or one whose duration the port
        // reported without lending samples for it.
        _ => out.resize(out.len().saturating_add(samples), 0),
    }
}

/// Run one class's activation factory for one interface.
pub(super) fn activate<T: Complete>(class: &Guid, iid: &Guid) -> io::Result<Com<T>> {
    let mut out: *mut c_void = std::ptr::null_mut();
    // SAFETY: both ids are live locals the call only reads, no outer object
    // is used, and the out slot is a local; the class answers with the one
    // reference this handle then owns.
    let hr =
        unsafe { CoCreateInstance(class, std::ptr::null_mut(), CLSCTX_ALL, iid, &raw mut out) };
    if hr < 0 || out.is_null() {
        return Err(native("device enumerator", hr));
    }
    // SAFETY: the call answered a non-null interface carrying its reference.
    Ok(unsafe { Com::own(out) })
}

impl Com<EnumeratorVt> {
    /// `GetDefaultAudioEndpoint` for the role that matters here.
    ///
    /// Applications play and record through the *multimedia* default — the
    /// device Windows' Sound panel sets as "Default" — so that is the role
    /// asked for first. The console and communications roles can each name
    /// a different endpoint, and capturing one of those gives a track of
    /// digital silence while the machine is plainly making noise. The
    /// console role stays as a fallback for machines that answer only it.
    pub(super) fn default_endpoint(&self, flow: i32) -> io::Result<Com<DeviceVt>> {
        let Some(table) = self.view() else {
            return Err(io::Error::other("WASAPI: the device enumerator went away"));
        };
        let mut failure = None;
        for role in [ROLE_MULTIMEDIA, ROLE_CONSOLE] {
            let mut out: *mut c_void = std::ptr::null_mut();
            // SAFETY: live enumerator and a local out slot; slot 4 takes
            // exactly these four arguments.
            let hr = unsafe { (table.get_default_endpoint)(self.raw(), flow, role, &raw mut out) };
            if hr >= 0 && !out.is_null() {
                // SAFETY: a non-null device carries the reference handed
                // to us.
                return Ok(unsafe { Com::own(out) });
            }
            failure.get_or_insert(native("default device", hr));
        }
        Err(failure.unwrap_or_else(|| io::Error::other("WASAPI: no default device")))
    }
}

impl Com<DeviceVt> {
    /// `Activate` an audio client on this device.
    fn capture_client(&self) -> io::Result<Com<AudioClientVt>> {
        let Some(table) = self.view() else {
            return Err(io::Error::other("WASAPI: the device went away"));
        };
        let mut out: *mut c_void = std::ptr::null_mut();
        // SAFETY: live device, the client `GUID`, no activation parameters,
        // and a local out slot; slot 3 takes exactly these five arguments.
        let hr = unsafe {
            (table.activate)(
                self.raw(),
                &AUDIO_CLIENT,
                CLSCTX_ALL,
                std::ptr::null_mut(),
                &raw mut out,
            )
        };
        if hr < 0 || out.is_null() {
            return Err(native("audio client", hr));
        }
        // SAFETY: a non-null client carries the reference handed to us.
        Ok(unsafe { Com::own(out) })
    }
}

impl Com<AudioClientVt> {
    /// `Initialize` the stream in shared mode for the one format this
    /// application records, with the engine's converters enabled.
    fn initialize(&self, flow: i32) -> io::Result<()> {
        let Some(table) = self.view() else {
            return Err(io::Error::other("WASAPI: the audio client went away"));
        };
        let format: WaveFormatExtensible = stereo_pcm();
        let mut flags = AUTOCONVERT | SRC_DEFAULT_QUALITY;
        if flow == RENDER {
            flags |= LOOPBACK;
        }
        // SAFETY: a live client, a format record that outlives the call,
        // and the documented shared-mode values (periodicity zero).
        let hr = unsafe {
            (table.initialize)(
                self.raw(),
                SHARED,
                flags,
                BUFFER_100MS,
                0,
                (&raw const format).cast(),
                std::ptr::null(),
            )
        };
        if hr < 0 {
            return Err(native("stream format", hr));
        }
        Ok(())
    }

    /// `GetService` for the port packets are read from.
    fn capture_service(&self) -> io::Result<Com<CaptureClientVt>> {
        let Some(table) = self.view() else {
            return Err(io::Error::other("WASAPI: the audio client went away"));
        };
        let mut out: *mut c_void = std::ptr::null_mut();
        // SAFETY: live client, the capture `GUID`, and a local out slot;
        // slot 14 takes exactly these three arguments.
        let hr = unsafe { (table.get_service)(self.raw(), &CAPTURE_CLIENT, &raw mut out) };
        if hr < 0 || out.is_null() {
            return Err(native("capture service", hr));
        }
        // SAFETY: a non-null port carries the reference handed to us.
        Ok(unsafe { Com::own(out) })
    }
}

/// One failed `WASAPI` step, named, with the raw code so the log can tell a
/// missing device from a refused format.
fn native(stage: &str, hr: i32) -> io::Error {
    if hr == DEVICE_INVALIDATED {
        return io::Error::other(format!("WASAPI: {stage}: the device was removed"));
    }
    io::Error::other(format!("WASAPI: {stage}: 0x{hr:08X}"))
}

#[cfg(test)]
mod tests {
    use super::{SILENT, append_packet};

    #[test]
    fn a_packet_decodes_little_endian_stereo() {
        let mut out = Vec::new();
        // Two frames of stereo: (1, -1), then (256, -256).
        let bytes = [1u8, 0, 0xFF, 0xFF, 0x00, 0x01, 0x00, 0xFF];
        append_packet(&mut out, Some(&bytes), 2, 0);
        assert_eq!(out, vec![1, -1, 256, -256]);
    }

    /// A silent packet keeps its duration, and the flag wins over any buffer
    /// the port handed along with it.
    #[test]
    fn a_silent_packet_keeps_its_duration() {
        let mut out = vec![7];
        append_packet(&mut out, None, 3, SILENT);
        assert_eq!(out, vec![7, 0, 0, 0, 0, 0, 0]);

        let mut buffered = Vec::new();
        append_packet(&mut buffered, Some(&[9u8; 4]), 1, SILENT);
        assert_eq!(buffered, vec![0, 0]);
    }

    /// The port can report frames without lending samples; that packet is
    /// silence too, not a dropped slice of the timeline.
    #[test]
    fn a_packet_without_samples_still_takes_its_time() {
        let mut out = Vec::new();
        append_packet(&mut out, None, 2, 0);
        assert_eq!(out, vec![0, 0, 0, 0]);
    }
}
