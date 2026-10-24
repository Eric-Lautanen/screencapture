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

use super::abi::{CAPTURE, CHANNELS, RENDER, SAMPLE_RATE};
use super::live::{Live, Running};
use super::pipe::Server;
use super::wasapi::Client;
use super::{Devices, Levels, Wanted};

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
/// How often a source standing in on the Windows default asks whether the
/// device the settings chose has come back. A Bluetooth headset takes a
/// moment to reconnect, and a poll is one cheap call.
const FOLLOW_EVERY: Duration = Duration::from_secs(2);

/// The capture clients a recording asked for; a source that refuses is
/// dropped with a log line instead of failing the recording.
pub(super) struct Clients {
    /// Loopback of the playback device.
    system: Source,
    /// The recording device.
    mic: Source,
}

impl Clients {
    /// Open whichever sources the plan wants, on the endpoints the settings
    /// chose.
    pub(super) fn open(wanted: Wanted, devices: &Devices) -> Self {
        let mut clients = Self {
            system: Source::new(devices.system.clone()),
            mic: Source::new(devices.mic.clone()),
        };
        if wanted.system
            && let Err(error) = clients.system.open(RENDER)
        {
            crate::log::note(
                "audio",
                &format!("system audio unavailable ({error}); recording without it"),
            );
        }
        if wanted.mic
            && let Err(error) = clients.mic.open(CAPTURE)
        {
            crate::log::note(
                "audio",
                &format!("microphone unavailable ({error}); recording without it"),
            );
        }
        clients
    }

    /// Which sources are capturing.
    fn running(&self) -> Running {
        Running {
            mic: !self.mic.idle(),
            system: !self.system.idle(),
        }
    }

    /// Whether any source opened.
    fn any(&self) -> bool {
        !self.system.idle() || !self.mic.idle()
    }

    /// Start whatever the take has asked for since the last pass. The
    /// bar's audio buttons come through here: a source the settings left
    /// off (or one that refused at the start) joins a take in progress,
    /// from the moment it is asked for on.
    fn join(&mut self, live: &Live) {
        let (mic, system) = live.awaiting();
        if system && self.system.idle() {
            join_source(&mut self.system, RENDER, "system audio", |running| {
                live.system_started(running);
            });
        }
        if mic && self.mic.idle() {
            join_source(&mut self.mic, CAPTURE, "microphone", |running| {
                live.microphone_started(running);
            });
        }
    }

    /// Move a source that is standing in on the Windows default onto the
    /// device the settings chose, as soon as that device comes back.
    fn follow(&mut self) {
        self.system.follow(RENDER, "system audio");
        self.mic.follow(CAPTURE, "microphone");
    }

    /// Begin delivering packets from every open source. A source that
    /// refuses to start contributes silence, so the track's length still
    /// matches the recording; the log says which one never ran.
    fn start(&mut self) {
        for (source, name) in [
            (&mut self.system, "system audio"),
            (&mut self.mic, "microphone"),
        ] {
            if !source.idle() {
                note_start(name, source.start());
            }
        }
    }

    /// Stop every open source; called once when the worker leaves its loop.
    fn stop(&mut self) {
        self.system.stop();
        self.mic.stop();
    }

    /// Drain each open source into its queue, reopening any stream that
    /// fails so a device that was unplugged is replaced instead of lost.
    fn drain(&mut self, system: &mut Vec<i16>, mic: &mut Vec<i16>) {
        self.system.drain(RENDER, system, "system audio");
        self.mic.drain(CAPTURE, mic, "microphone");
    }
}

/// One of the take's sources: the stream capturing it, the endpoint the
/// settings chose, and when the worker last looked for that endpoint.
struct Source {
    /// The running stream; `None` while nothing captures this source.
    client: Option<Client>,
    /// The endpoint the settings chose, by id. `None` follows whatever
    /// Windows calls the default.
    preferred: Option<String>,
    /// When the worker last asked whether a missing chosen endpoint had
    /// come back, so that question costs one call every couple of seconds
    /// rather than one per chunk.
    checked: Instant,
}

impl Source {
    /// A source that is not capturing yet.
    fn new(preferred: Option<String>) -> Self {
        Self {
            client: None,
            preferred,
            checked: Instant::now(),
        }
    }

    /// Whether nothing is capturing this source.
    const fn idle(&self) -> bool {
        self.client.is_none()
    }

    /// Open the endpoint this source prefers, or the Windows default when
    /// the chosen device is not connected.
    fn open(&mut self, flow: i32) -> io::Result<()> {
        self.client = Some(Client::open(flow, self.preferred.as_deref())?);
        self.checked = Instant::now();
        Ok(())
    }

    /// Begin delivering packets. A stream that will not run is no stream,
    /// so it is dropped rather than left pretending to capture.
    fn start(&mut self) -> io::Result<()> {
        let Some(client) = self.client.take() else {
            return Ok(());
        };
        match client.start() {
            Ok(()) => {
                self.client = Some(client);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Stop delivering packets and forget the stream.
    fn stop(&mut self) {
        if let Some(client) = self.client.take() {
            client.stop();
        }
    }

    /// Replace whatever is running with a fresh stream on the endpoint this
    /// source prefers, so a device that came back is used again.
    fn reopen(&mut self, flow: i32, name: &str) {
        self.stop();
        if let Err(error) = self.open(flow) {
            crate::log::note(
                "audio",
                &format!("{name} unavailable ({error}); the track carries silence for it"),
            );
            return;
        }
        let started = self.start();
        let running = started.is_ok();
        note_start(name, started);
        if !running {
            self.client = None;
        }
    }

    /// Move a stream that is only standing in on the Windows default onto
    /// the device the settings chose, once that device is connected again.
    fn follow(&mut self, flow: i32, name: &str) {
        let Some(preferred) = self.preferred.clone() else {
            return;
        };
        let Some(client) = self.client.as_ref() else {
            return;
        };
        if !client.on_fallback() || self.checked.elapsed() < FOLLOW_EVERY {
            return;
        }
        self.checked = Instant::now();
        if !super::devices::present(&preferred) {
            return;
        }
        self.reopen(flow, name);
        if self
            .client
            .as_ref()
            .is_some_and(|client| !client.on_fallback())
        {
            crate::log::note("audio", &format!("{name} moved to the chosen device"));
        }
    }

    /// Append whatever this source has ready, reopening it when the stream
    /// reports that its device went away.
    fn drain(&mut self, flow: i32, queue: &mut Vec<i16>, name: &str) {
        let Some(client) = self.client.as_ref() else {
            return;
        };
        if let Err(error) = client.drain(queue) {
            crate::log::note(
                "audio",
                &format!("{name} capture stopped ({error}); reopening it"),
            );
            self.reopen(flow, name);
        }
        trim(queue);
    }
}

/// Open one source the take has asked for, start it, and report to the
/// switches what the worker managed: a device that opened but would not run
/// leaves its button off rather than promising sound the take never gets.
fn join_source(source: &mut Source, flow: i32, name: &str, settle: impl FnOnce(bool)) {
    if let Err(error) = source.open(flow) {
        crate::log::note(
            "audio",
            &format!("{name} unavailable ({error}); the button stays off"),
        );
        settle(false);
        return;
    }
    let started = source.start();
    let running = started.is_ok();
    note_start(name, started);
    if !running {
        source.client = None;
    }
    settle(running);
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
    /// The switches the frame's buttons flip and the mixer reads.
    live: Live,
    /// Each source's fader, read once per take.
    gains: (f32, f32),
}

impl Prepared {
    /// Open the sources the plan asked for, and tell `live` which ones are
    /// capturing.
    ///
    /// `None` only when the pipe itself cannot be created — the track has
    /// to exist for the bar's buttons to bring a source in mid-take, so a
    /// take that starts silent still writes a silent track.
    pub(crate) fn open(
        wanted: Wanted,
        devices: &Devices,
        levels: Levels,
        live: &Live,
    ) -> Option<Self> {
        let clients = Clients::open(wanted, devices);
        if !clients.any() && wanted.any() {
            crate::log::note(
                "audio",
                "no audio source could be opened; the track stays silent",
            );
        }
        live.opened(clients.running());
        match Server::create() {
            Ok(server) => Some(Self {
                clients,
                server,
                live: live.clone(),
                gains: levels.gains(),
            }),
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
        // A button pressed mid-take asks for a source here, so the mix can
        // start carrying it without restarting the recording.
        prepared.clients.join(&prepared.live);
        // A source that is only standing in on the Windows default moves to
        // the chosen device as soon as that device is connected again.
        prepared.clients.follow();
        prepared.clients.drain(&mut system, &mut mic);
        // The buttons can flip between chunks; reading them here keeps a
        // mute within one 10 ms chunk of the click.
        let (mic_on, system_on) = prepared.live.audible();
        let due = frames_due(origin.elapsed());
        while written.saturating_add(CHUNK_FRAMES as u64) <= due {
            mix(
                &mut chunk,
                &mut mic,
                &mut system,
                (mic_on, system_on),
                prepared.gains,
            );
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

/// Where the limiter starts bending: 95% of full scale.
const LIMIT_KNEE: i32 = 31_130;
/// Room left between the knee and full scale, in sample units.
const LIMIT_HEADROOM: i32 = 32_768 - LIMIT_KNEE;

/// Sum one chunk of both queues into interleaved 16-bit bytes, then
/// consume what was used. A muted source still gives up its samples, so the
/// queues stay aligned with the clock and unmuting lands back in real time.
///
/// A take with both sources live can ask for twice full scale. A hard clamp
/// there is a square wave — a crackle exactly at the loud moment — so a sum
/// past the knee is bent hyperbolically into the last half decibel instead:
/// loud peaks compress a touch and never break up. One source alone never
/// reaches the knee, so those takes are sample-for-sample untouched.
fn mix(
    out: &mut [u8],
    mic: &mut Vec<i16>,
    system: &mut Vec<i16>,
    (mic_on, system_on): (bool, bool),
    (mic_gain, system_gain): (f32, f32),
) {
    for index in 0..CHUNK_SAMPLES {
        let from_mic = if mic_on {
            scale(mic.get(index).copied().unwrap_or(0), mic_gain)
        } else {
            0
        };
        let from_system = if system_on {
            scale(system.get(index).copied().unwrap_or(0), system_gain)
        } else {
            0
        };
        let sum = from_mic + from_system;
        let sample = if mic_on && system_on {
            limit(sum)
        } else {
            i16::try_from(sum.clamp(i32::from(i16::MIN), i32::from(i16::MAX))).unwrap_or(0)
        };
        let at = index * 2;
        out[at..at + 2].copy_from_slice(&sample.to_le_bytes());
    }
    let used_mic = mic.len().min(CHUNK_SAMPLES);
    mic.drain(..used_mic);
    let used_system = system.len().min(CHUNK_SAMPLES);
    system.drain(..used_system);
}

/// One source sample at its fader's gain. Unity passes the sample through
/// bit for bit, so a take that never touched a fader is unchanged.
fn scale(sample: i16, gain: f32) -> i32 {
    if (gain - 1.0).abs() < f32::EPSILON {
        return i32::from(sample);
    }
    let scaled = f32::from(sample) * gain;
    let rounded = scaled
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is clamped to the 16-bit range just above"
    )]
    let value = rounded as i32;
    value
}

/// One mixed sample, bent rather than clipped above the knee.
fn limit(sum: i32) -> i16 {
    let magnitude = sum.abs();
    if magnitude <= LIMIT_KNEE {
        return i16::try_from(sum).unwrap_or(0);
    }
    let excess = magnitude - LIMIT_KNEE;
    let bent = excess * LIMIT_HEADROOM / (LIMIT_HEADROOM + excess);
    let value = (LIMIT_KNEE + bent).min(i32::from(i16::MAX));
    i16::try_from(value * sum.signum()).unwrap_or(0)
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
    fn both_sources_sum_and_a_loud_one_bends() {
        let mut mic = vec![100_i16, i16::MAX, -100];
        let mut system = vec![50_i16, i16::MAX, 100];
        let mut out = vec![0_u8; CHUNK_SAMPLES * 2];
        mix(&mut out, &mut mic, &mut system, (true, true), (1.0, 1.0));
        let samples: Vec<i16> = out
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[low, high]| i16::from_le_bytes([*low, *high]))
            .collect();
        assert_eq!(samples[0], 150);
        // Twice full scale used to clamp to a square wave; it now bends into
        // the last half decibel and stays clear of the rail.
        assert!(
            samples[1] > 32_000 && samples[1] < i16::MAX,
            "a loud sum bends: {}",
            samples[1]
        );
        assert_eq!(samples[2], 0);
        // The chunk consumed exactly what it used, and nothing more.
        assert_eq!(mic, Vec::<i16>::new());
        assert_eq!(system, Vec::<i16>::new());
    }

    #[test]
    fn one_source_is_never_touched() {
        // With a single source the mix cannot reach the knee, so the only
        // source's samples pass through exactly as captured.
        let loud = vec![i16::MAX; CHUNK_SAMPLES];
        let mut mic = loud.clone();
        let mut system = Vec::new();
        let mut out = vec![0_u8; CHUNK_SAMPLES * 2];
        mix(&mut out, &mut mic, &mut system, (true, false), (1.0, 1.0));
        let samples: Vec<i16> = out
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[low, high]| i16::from_le_bytes([*low, *high]))
            .collect();
        assert!(samples.iter().all(|sample| *sample == i16::MAX));
    }

    #[test]
    fn the_limiter_is_monotonic_and_bounded() {
        use super::limit;
        let mut previous = i16::MIN;
        for sum in -70_000..=70_000 {
            let out = limit(sum);
            assert!(out >= previous, "monotonic at {sum}");
            previous = out;
        }
        assert_eq!(limit(0), 0);
        for sum in [1, 1000, 20_000, 31_130, 31_131, 32_767, 40_000, 65_534] {
            assert_eq!(limit(-sum), -limit(sum), "symmetric at {sum}");
        }
        // Twice full scale still lands below the rail rather than on it.
        assert!(limit(65_534) < i16::MAX);
        assert!(limit(65_534) > 32_000);
    }

    #[test]
    fn a_short_source_is_padded_with_silence() {
        let mut mic = vec![1000_i16];
        let mut system = Vec::new();
        let mut out = vec![0_u8; CHUNK_SAMPLES * 2];
        mix(&mut out, &mut mic, &mut system, (true, true), (1.0, 1.0));
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
        mix(&mut out, &mut mic, &mut system, (true, true), (1.0, 1.0));
        assert_eq!(mic.len(), 10);
    }

    /// A muted source contributes silence but still gives up the samples
    /// the chunk covered, so unmuting resumes at the right moment instead
    /// of replaying the muted stretch.
    #[test]
    fn a_muted_source_goes_quiet_without_slipping_the_clock() {
        let mut mic = vec![1000_i16; CHUNK_SAMPLES + 5];
        let mut system = vec![500_i16; CHUNK_SAMPLES];
        let mut out = vec![0_u8; CHUNK_SAMPLES * 2];
        mix(&mut out, &mut mic, &mut system, (false, true), (1.0, 1.0));
        let samples: Vec<i16> = out
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[low, high]| i16::from_le_bytes([*low, *high]))
            .collect();
        assert!(samples.iter().all(|sample| *sample == 500), "mic is silent");
        assert_eq!(mic.len(), 5, "the muted chunk is still consumed");
    }
}
