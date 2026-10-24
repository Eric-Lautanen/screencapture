//! The audio worker: open the requested sources, mix them into one stream,
//! and write that stream into the encoder's pipe.
//!
//! Mixing follows the recording clock rather than arrival order. Both
//! clients hand over short packets, the worker queues them, and every 10 ms
//! of recording time is filled from those queues; a gap — nothing playing on
//! the speakers, a microphone that has not delivered yet — becomes silence
//! instead of a stall, which is what keeps the audio aligned with the video
//! that is being paced by the same clock.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::Wanted;
use super::abi::{CAPTURE, CHANNELS, RENDER, SAMPLE_RATE};
use super::pipe::Server;
use super::wasapi::Client;

/// How often packets are drained and the pipe topped up.
const POLL: Duration = Duration::from_millis(5);
/// Frames written per chunk: 10 ms, small enough to keep latency invisible
/// and large enough to keep the syscall count low.
const CHUNK_FRAMES: usize = 480;
/// Samples per chunk, both channels included.
const CHUNK_SAMPLES: usize = CHUNK_FRAMES * CHANNELS as usize;
/// Bytes per chunk.
const CHUNK_BYTES: usize = CHUNK_SAMPLES * 2;
/// Queued audio beyond this is dropped from the front: a source whose clock
/// runs fast must not add latency that never comes back.
const MAX_QUEUED: usize = SAMPLE_RATE as usize * CHANNELS as usize * 2;
/// How long a stop waits for the worker to close the pipe.
const STOP_BUDGET: Duration = Duration::from_secs(2);

/// The capture clients a recording asked for; a source that refuses is
/// dropped with a log line instead of failing the recording.
pub(super) struct Clients {
    /// Loopback of the default playback device.
    system: Option<Client>,
    /// The default recording device.
    mic: Option<Client>,
}

impl Clients {
    /// Open whichever sources the plan wants.
    pub(super) fn open(wanted: Wanted) -> Self {
        let mut clients = Self {
            system: None,
            mic: None,
        };
        if wanted.system {
            match Client::open(RENDER) {
                Ok(client) => clients.system = Some(client),
                Err(error) => crate::log::note(
                    "audio",
                    &format!("system audio unavailable ({error}); recording without it"),
                ),
            }
        }
        if wanted.mic {
            match Client::open(CAPTURE) {
                Ok(client) => clients.mic = Some(client),
                Err(error) => crate::log::note(
                    "audio",
                    &format!("microphone unavailable ({error}); recording without it"),
                ),
            }
        }
        clients
    }

    /// Whether any source opened.
    fn any(&self) -> bool {
        self.system.is_some() || self.mic.is_some()
    }

    /// Begin delivering packets from every open source. A source that
    /// refuses to start keeps its place in the mix and contributes
    /// silence, so the track's length still matches the recording; the
    /// log says which one never ran.
    fn start(&self) {
        if let Some(client) = &self.system {
            note_start("system audio", client.start());
        }
        if let Some(client) = &self.mic {
            note_start("microphone", client.start());
        }
    }

    /// Stop every open source; called once when the worker leaves its loop.
    fn stop(&self) {
        if let Some(client) = &self.system {
            client.stop();
        }
        if let Some(client) = &self.mic {
            client.stop();
        }
    }

    /// Drain each open source into its queue, retiring any source that
    /// reports an error so the worker never spins on a dead stream.
    fn drain(&mut self, system: &mut Vec<i16>, mic: &mut Vec<i16>) {
        drain_into(&mut self.system, system, "system audio");
        drain_into(&mut self.mic, mic, "microphone");
        trim(system);
        trim(mic);
    }
}

/// Report a source that opened but would not start.
fn note_start(name: &str, result: io::Result<()>) {
    if let Err(error) = result {
        crate::log::note(
            "audio",
            &format!("{name} did not start ({error}); the track carries silence for it"),
        );
    }
}

/// Drain one optional source, dropping it when the stream fails.
fn drain_into(client: &mut Option<Client>, queue: &mut Vec<i16>, name: &str) {
    let Some(source) = client.take() else {
        return;
    };
    match source.drain(queue) {
        Ok(()) => *client = Some(source),
        Err(error) => crate::log::note(
            "audio",
            &format!("{name} capture stopped ({error}); the recording keeps its video"),
        ),
    }
}

/// Drop the oldest queued samples past the cap, so a fast source cannot
/// build up latency that never drains.
fn trim(queue: &mut Vec<i16>) {
    if queue.len() > MAX_QUEUED {
        let excess = queue.len() - MAX_QUEUED;
        queue.drain(..excess);
    }
}

/// Sources opened and a pipe waiting for the engine: everything decided
/// before the encoder is spawned, because its arguments name the pipe.
pub(crate) struct Prepared {
    /// The clients, already opened on this thread.
    clients: Clients,
    /// The pipe the engine will read.
    server: Server,
}

impl Prepared {
    /// Open the sources the plan asked for.
    ///
    /// `None` when nothing was requested, or when every requested source
    /// refused — the recording then proceeds without an audio track.
    pub(crate) fn open(wanted: Wanted) -> Option<Self> {
        if !wanted.any() {
            return None;
        }
        let clients = Clients::open(wanted);
        if !clients.any() {
            crate::log::note(
                "audio",
                "no audio source could be opened; recording without sound",
            );
            return None;
        }
        match Server::create() {
            Ok(server) => Some(Self { clients, server }),
            Err(error) => {
                crate::log::note(
                    "audio",
                    &format!("cannot open the audio pipe ({error}); recording without sound"),
                );
                None
            }
        }
    }

    /// The pipe path to hand the engine as a raw-PCM input.
    pub(crate) fn path(&self) -> &str {
        self.server.path()
    }
}

/// A running audio worker, stopped when the recording ends.
pub(crate) struct Audio {
    /// Set by [`Self::shut`] to leave the loop.
    stop: Arc<AtomicBool>,
    /// `None` once the worker was joined or abandoned.
    worker: Option<JoinHandle<()>>,
}

/// Start the worker for a prepared set of sources. `origin` is the clock
/// the frames are paced by: the track counts from it too, so whatever time
/// the worker spends connecting becomes silence at the head rather than a
/// shift of every later sample against the picture.
///
/// # Errors
/// Fails when the thread cannot be spawned; a source that fails later only
/// ends the audio track early, and is logged where it happens.
pub(crate) fn start(mut prepared: Prepared, origin: Instant) -> io::Result<Audio> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let worker = thread::Builder::new().name("audio".into()).spawn(move || {
        // The clients were activated on the setup thread's apartment; this
        // one has to join it before it polls them.
        super::wasapi::apartment();
        if let Err(error) = run(&mut prepared, &flag, origin) {
            crate::log::note("audio", &format!("audio capture stopped ({error})"));
        }
    })?;
    Ok(Audio {
        stop,
        worker: Some(worker),
    })
}

impl Audio {
    /// Stop the worker and close the pipe, which is the engine's signal that
    /// the audio stream ended. Bounded: a worker stuck in a write is left to
    /// the recorder's own stop budget rather than freezing this one.
    pub(crate) fn stop(mut self) {
        self.shut();
    }

    fn shut(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let Some(worker) = self.worker.take() else {
            return;
        };
        let deadline = Instant::now() + STOP_BUDGET;
        while !worker.is_finished() {
            if Instant::now() >= deadline {
                return;
            }
            thread::sleep(POLL);
        }
        let _ = worker.join();
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.shut();
    }
}

/// Connect, start the sources, and pump mixed chunks until asked to stop.
fn run(prepared: &mut Prepared, stop: &AtomicBool, origin: Instant) -> io::Result<()> {
    prepared.server.connect()?;
    prepared.clients.start();
    let mut system: Vec<i16> = Vec::new();
    let mut mic: Vec<i16> = Vec::new();
    let mut chunk = vec![0_u8; CHUNK_BYTES];
    let mut written: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        prepared.clients.drain(&mut system, &mut mic);
        let due = frames_due(origin.elapsed());
        while written.saturating_add(CHUNK_FRAMES as u64) <= due {
            mix(&mut chunk, &mut mic, &mut system);
            prepared.server.write_all(&chunk)?;
            written = written.saturating_add(CHUNK_FRAMES as u64);
        }
        thread::sleep(POLL);
    }
    prepared.clients.stop();
    Ok(())
}

/// How many frames the file should hold after `elapsed`: the recording
/// clock, in samples.
fn frames_due(elapsed: Duration) -> u64 {
    let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
    micros.saturating_mul(u64::from(SAMPLE_RATE)) / 1_000_000
}

/// Sum one chunk of both queues into interleaved 16-bit bytes, saturating
/// rather than wrapping where the two sources overlap, then consume what
/// was used.
fn mix(out: &mut [u8], mic: &mut Vec<i16>, system: &mut Vec<i16>) {
    for index in 0..CHUNK_SAMPLES {
        let from_mic = mic.get(index).copied().unwrap_or(0);
        let from_system = system.get(index).copied().unwrap_or(0);
        let sum = i32::from(from_mic) + i32::from(from_system);
        let clamped = sum.clamp(i32::from(i16::MIN), i32::from(i16::MAX));
        let sample = i16::try_from(clamped).unwrap_or(0);
        let at = index * 2;
        out[at..at + 2].copy_from_slice(&sample.to_le_bytes());
    }
    let used_mic = mic.len().min(CHUNK_SAMPLES);
    mic.drain(..used_mic);
    let used_system = system.len().min(CHUNK_SAMPLES);
    system.drain(..used_system);
}

#[cfg(test)]
mod tests {
    use super::{CHUNK_SAMPLES, frames_due, mix};
    use std::time::Duration;

    #[test]
    fn the_clock_asks_for_ten_ms_per_chunk() {
        assert_eq!(frames_due(Duration::ZERO), 0);
        assert_eq!(frames_due(Duration::from_millis(10)), 480);
        assert_eq!(frames_due(Duration::from_secs(1)), 48_000);
    }

    #[test]
    fn both_sources_sum_and_saturate() {
        let mut mic = vec![100_i16, i16::MAX, -100];
        let mut system = vec![50_i16, i16::MAX, 100];
        let mut out = vec![0_u8; CHUNK_SAMPLES * 2];
        mix(&mut out, &mut mic, &mut system);
        let samples: Vec<i16> = out
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[low, high]| i16::from_le_bytes([*low, *high]))
            .collect();
        assert_eq!(samples[0], 150);
        assert_eq!(samples[1], i16::MAX);
        assert_eq!(samples[2], 0);
        // The chunk consumed exactly what it used, and nothing more.
        assert_eq!(mic, Vec::<i16>::new());
        assert_eq!(system, Vec::<i16>::new());
    }

    #[test]
    fn a_short_source_is_padded_with_silence() {
        let mut mic = vec![1000_i16];
        let mut system = Vec::new();
        let mut out = vec![0_u8; CHUNK_SAMPLES * 2];
        mix(&mut out, &mut mic, &mut system);
        let samples: Vec<i16> = out
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[low, high]| i16::from_le_bytes([*low, *high]))
            .collect();
        assert_eq!(samples[0], 1000);
        assert!(samples[1..].iter().all(|sample| *sample == 0));
    }

    /// Leftovers belong to the next chunk: a source that delivered more
    /// than one chunk keeps the rest.
    #[test]
    fn a_long_source_keeps_the_remainder() {
        let mut mic = vec![7_i16; CHUNK_SAMPLES + 10];
        let mut system = Vec::new();
        let mut out = vec![0_u8; CHUNK_SAMPLES * 2];
        mix(&mut out, &mut mic, &mut system);
        assert_eq!(mic.len(), 10);
    }
}
