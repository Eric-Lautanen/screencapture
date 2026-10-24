//! Confirmation chime for a committed capture — a still is taken or a
//! recording is asked to stop: three soft notes rising a little, synthesized
//! in memory (no asset file, no crate) as a PCM WAV. It starts the moment
//! the capture is committed, before encoding, and plays through `PlaySound`
//! on a worker thread, so the capture path never waits on the audio device;
//! other platforms stay silent.
//!
//! Playback walks every route Windows offers instead of trusting one: the
//! in-memory image, then the same bytes as a file, then the stock asterisk.
//! A machine whose audio stack refuses one route usually takes the next, and
//! a machine that refuses all three gets a log line naming the cause rather
//! than a silently quiet capture.

#[cfg(windows)]
use std::path::{Path, PathBuf};

/// Start the capture chime the instant a capture is committed, unless the
/// settings mute is on. The returned handle lets a caller that is about to
/// exit (the command line) wait for the last note; the tray drops it and
/// lets the sound finish in the background.
#[cfg(windows)]
pub(crate) fn capture_chime(enabled: bool) -> Option<std::thread::JoinHandle<()>> {
    if !enabled {
        return None;
    }
    let wav = synth::chime();
    Some(std::thread::spawn(move || {
        if play(&wav) {
            return;
        }
        crate::log::note(
            "chime",
            "the audio system refused the confirmation chime; captures stay \
             silent (check the output device and this app's volume in the mixer)",
        );
    }))
}

/// Other platforms have no capture chime yet; the signature stays shared.
#[cfg(not(windows))]
pub(crate) fn capture_chime(_enabled: bool) -> Option<std::thread::JoinHandle<()>> {
    None
}

/// How many audio output devices Windows currently knows about; `None` on
/// platforms without the chime. Zero explains a silent capture outright.
#[cfg(windows)]
#[must_use]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the shared signature answers nothing on platforms without a chime"
)]
pub(crate) fn output_devices() -> Option<u32> {
    // SAFETY: the call takes no arguments and only reads the device count.
    Some(unsafe { wave_out_num_devs() })
}

/// Platforms without a chime have no devices to report.
#[cfg(not(windows))]
#[must_use]
pub(crate) fn output_devices() -> Option<u32> {
    None
}

/// Play the chime through every route Windows offers, cheapest first.
#[cfg(windows)]
fn play(wav: &[u8]) -> bool {
    play_image(wav) || play_file(wav) || play_alias()
}

/// The usual route: the WAV image straight from memory.
#[cfg(windows)]
fn play_image(wav: &[u8]) -> bool {
    // SAFETY: `wav` outlives this synchronous call, and an in-memory image
    // takes no module handle.
    unsafe {
        PlaySoundW(
            wav.as_ptr().cast(),
            std::ptr::null_mut(),
            SND_MEMORY | SND_NODEFAULT,
        ) != 0
    }
}

/// Second route: the same bytes as a file, for audio stacks that refuse
/// `SND_MEMORY` images but play the identical file. Only reached when the
/// memory route failed, so the write costs nothing on a healthy machine,
/// and the file is removed again before this returns.
#[cfg(windows)]
fn play_file(wav: &[u8]) -> bool {
    let Some(path) = write_chime_file(wav) else {
        return false;
    };
    let file = crate::winapi::wide(&path.to_string_lossy());
    // SAFETY: `file` outlives this synchronous call.
    let played = unsafe {
        PlaySoundW(
            file.as_ptr(),
            std::ptr::null_mut(),
            SND_FILENAME | SND_NODEFAULT,
        ) != 0
    };
    let _ = std::fs::remove_file(&path);
    played
}

/// Last route: the stock asterisk. Without `SND_NODEFAULT`, a machine that
/// has no asterisk mapped still answers with the default system sound, so
/// this succeeds wherever the machine can make any sound at all.
#[cfg(windows)]
fn play_alias() -> bool {
    let alias = crate::winapi::wide("SystemAsterisk");
    // SAFETY: `alias` outlives this synchronous call.
    unsafe { PlaySoundW(alias.as_ptr(), std::ptr::null_mut(), SND_ALIAS) != 0 }
}

/// Write the chime to a stable temporary file; `None` when the temp folder
/// is unusable (the caller still has the alias route).
#[cfg(windows)]
fn write_chime_file(wav: &[u8]) -> Option<PathBuf> {
    let path = chime_path(&std::env::temp_dir());
    std::fs::write(&path, wav).ok()?;
    Some(path)
}

/// Where the file route parks the chime. Fixed name so repeated captures
/// reuse (and overwrite) one file instead of littering the temp folder.
#[cfg(windows)]
fn chime_path(directory: &Path) -> PathBuf {
    directory.join("screencapture-chime.wav")
}

/// `SND_MEMORY` (mmsystem.h): the name argument points at a WAV image
/// rather than a file.
#[cfg(windows)]
const SND_MEMORY: u32 = 0x0004;
/// `SND_NODEFAULT` (mmsystem.h): stay silent instead of falling back to
/// the system sound when the image is refused.
#[cfg(windows)]
const SND_NODEFAULT: u32 = 0x0002;
/// `SND_FILENAME` (mmsystem.h): the name argument is a WAV file path.
#[cfg(windows)]
const SND_FILENAME: u32 = 0x0002_0000;
/// `SND_ALIAS` (mmsystem.h): the name argument is a registry sound alias.
#[cfg(windows)]
const SND_ALIAS: u32 = 0x0001_0000;

#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    /// Synchronous by default: the call returns when the sound ends, which
    /// is exactly what the dedicated playback thread wants.
    fn PlaySoundW(sound: *const u16, module: *mut std::ffi::c_void, flags: u32) -> i32;
    /// Output devices the wave mapper can reach; zero means silence is
    /// the only possible outcome, whatever route playback takes. The
    /// export is spelled with a lowercase `w` in mmsystem.h.
    #[link_name = "waveOutGetNumDevs"]
    fn wave_out_num_devs() -> u32;
}

/// The waveform itself: quiet, contained, and testable without an audio
/// device. Only compiled where it is played or measured.
#[cfg(any(windows, test))]
mod synth {
    /// Samples per second in the generated wave.
    pub(super) const SAMPLE_RATE: u32 = 44_100;
    /// The three notes, a D-major triad rising (D5, F#5, A5).
    pub(super) const TONES: [f64; 3] = [587.33, 739.99, 880.0];
    /// Length of one note in samples (180 ms).
    pub(super) const NOTE_SAMPLES: usize = 7_938;
    /// Distance between note starts in samples (110 ms). Shorter than a
    /// note, so each voice blooms before the previous one has faded.
    pub(super) const STEP_SAMPLES: usize = 4_851;
    /// Smooth attack in samples (8 ms); a hard start clicks.
    const ATTACK_SAMPLES: usize = 353;
    /// Peak amplitude of one voice: quiet enough to sit under everything
    /// else, and low enough that overlapping voices cannot clip.
    pub(super) const PEAK: f64 = 0.22;
    /// Second-harmonic level, for a soft bell color instead of a bare sine.
    const HARMONIC: f64 = 0.15;

    /// Bytes of the canonical PCM WAV header.
    const HEADER_BYTES: usize = 44;

    /// The whole chime as a 16-bit mono WAV image.
    pub(super) fn chime() -> Vec<u8> {
        wave(&samples())
    }

    /// The mixed chime before it is quantized to 16-bit samples.
    pub(super) fn samples() -> Vec<f64> {
        let total = (TONES.len() - 1) * STEP_SAMPLES + NOTE_SAMPLES;
        let mut samples = vec![0.0_f64; total];
        for (index, &frequency) in TONES.iter().enumerate() {
            add_note(&mut samples, index * STEP_SAMPLES, frequency);
        }
        samples
    }

    /// Mix one enveloped note into `samples` starting at `start`.
    pub(super) fn add_note(samples: &mut [f64], start: usize, frequency: f64) {
        let step = std::f64::consts::TAU * frequency / f64::from(SAMPLE_RATE);
        let mut phase = 0.0_f64;
        for (index, sample) in samples
            .iter_mut()
            .skip(start)
            .take(NOTE_SAMPLES)
            .enumerate()
        {
            let gain = envelope(index) * PEAK / (1.0 + HARMONIC);
            let tone = HARMONIC.mul_add((2.0 * phase).sin(), phase.sin());
            *sample += gain * tone;
            phase += step;
            if phase >= std::f64::consts::TAU {
                phase -= std::f64::consts::TAU;
            }
        }
    }

    /// Smooth in, slow out: a smoothstep attack and a quadratic release,
    /// so every note starts and ends at silence instead of clicking.
    fn envelope(index: usize) -> f64 {
        let index = f64::from(u32::try_from(index).unwrap_or(u32::MAX));
        let note = f64::from(u32::try_from(NOTE_SAMPLES).unwrap_or(u32::MAX));
        let attack = f64::from(u32::try_from(ATTACK_SAMPLES).unwrap_or(u32::MAX));
        if index < attack {
            let progress = index / attack;
            progress * progress * (3.0 - 2.0 * progress)
        } else {
            let remaining = (note - index) / (note - attack);
            remaining * remaining
        }
    }

    /// Wrap normalized samples in a canonical PCM WAV container.
    fn wave(samples: &[f64]) -> Vec<u8> {
        let data_bytes =
            u32::try_from(samples.len() * 2).expect("the chime comfortably fits a wave file");
        let mut bytes = Vec::with_capacity(HEADER_BYTES + samples.len() * 2);
        bytes.extend_from_slice(b"RIFF");
        push_u32(&mut bytes, 36 + data_bytes);
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        push_u32(&mut bytes, 16);
        push_u16(&mut bytes, 1); // PCM
        push_u16(&mut bytes, 1); // mono
        push_u32(&mut bytes, SAMPLE_RATE);
        push_u32(&mut bytes, SAMPLE_RATE * 2); // byte rate
        push_u16(&mut bytes, 2); // block align
        push_u16(&mut bytes, 16); // bits per sample
        bytes.extend_from_slice(b"data");
        push_u32(&mut bytes, data_bytes);
        for &sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * f64::from(i16::MAX)).round();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the value is clamped to i16's range before the cast"
            )]
            let value = value as i16;
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn push_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::synth::{
        NOTE_SAMPLES, PEAK, SAMPLE_RATE, STEP_SAMPLES, TONES, add_note, chime, samples,
    };

    fn u16_at(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("two bytes"))
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
    }

    /// Frequency from sign changes: every fundamental period crosses zero
    /// twice, and the quiet harmonic never flips the sign on its own.
    fn zero_crossing_hz(measured: &[f64]) -> f64 {
        let crossings = measured
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        let crossings = f64::from(u32::try_from(crossings).expect("crossings fit"));
        let frames = f64::from(u32::try_from(measured.len()).expect("sample count fits"));
        crossings / 2.0 / (frames / f64::from(SAMPLE_RATE))
    }

    #[test]
    fn the_settings_mute_skips_playback_entirely() {
        assert!(
            super::capture_chime(false).is_none(),
            "a muted capture must not start an audio thread"
        );
    }

    /// The file route is the fallback for audio stacks that refuse the
    /// in-memory image, so it must hand over exactly the same bytes.
    #[cfg(windows)]
    #[test]
    fn the_file_route_writes_the_chime_byte_for_byte() {
        let wav = chime();
        let path = super::write_chime_file(&wav).expect("the temp folder takes the chime");
        let written = std::fs::read(&path).expect("the written chime is readable");
        assert_eq!(written, wav);
        let _ = std::fs::remove_file(&path);
    }

    /// Repeated captures overwrite one file instead of filling the temp
    /// folder with a chime per shot.
    #[cfg(windows)]
    #[test]
    fn the_file_route_reuses_one_name() {
        let folder = std::env::temp_dir();
        let path = super::chime_path(&folder);
        assert_eq!(path, super::chime_path(&folder));
        assert!(
            path.to_string_lossy().ends_with("screencapture-chime.wav"),
            "{path:?} must name the chime file"
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_device_count_answers_without_a_device() {
        // Silence is diagnosable: a zero here is the whole answer.
        assert!(super::output_devices().is_some());
    }

    #[test]
    fn the_notes_rise_a_little() {
        assert!(
            TONES.windows(2).all(|pair| pair[0] < pair[1]),
            "the three tones must escalate"
        );
    }

    #[test]
    fn the_wave_header_describes_a_mono_pcm_chime() {
        let wav = chime();
        let frames = (TONES.len() - 1) * STEP_SAMPLES + NOTE_SAMPLES;
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u32_at(&wav, 16), 16);
        assert_eq!(u16_at(&wav, 20), 1, "PCM");
        assert_eq!(u16_at(&wav, 22), 1, "mono");
        assert_eq!(u32_at(&wav, 24), SAMPLE_RATE);
        assert_eq!(u32_at(&wav, 28), SAMPLE_RATE * 2, "byte rate");
        assert_eq!(u16_at(&wav, 32), 2, "block align");
        assert_eq!(u16_at(&wav, 34), 16, "bits per sample");
        assert_eq!(&wav[36..40], b"data");
        let data = usize::try_from(u32_at(&wav, 40)).expect("data size fits");
        assert_eq!(data, frames * 2);
        assert_eq!(data, wav.len() - 44);
        assert_eq!(
            usize::try_from(u32_at(&wav, 4)).expect("riff size fits"),
            wav.len() - 8
        );
    }

    #[test]
    fn a_note_starts_and_ends_in_silence_and_never_clips() {
        let mut samples = vec![0.0_f64; NOTE_SAMPLES];
        add_note(&mut samples, 0, TONES[0]);
        assert!(samples[0].abs() < 1e-12, "silent first sample");
        assert!(
            samples[NOTE_SAMPLES - 1].abs() < 1e-3,
            "the note fades out instead of clicking"
        );
        let peak = samples.iter().fold(0.0_f64, |max, s| max.max(s.abs()));
        assert!((0.1..=PEAK).contains(&peak), "a soft, audible peak");
    }

    #[test]
    fn the_oscillator_sounds_at_the_note_frequency() {
        let mut samples = vec![0.0_f64; NOTE_SAMPLES];
        add_note(&mut samples, 0, TONES[0]);
        let measured = zero_crossing_hz(&samples[1_000..NOTE_SAMPLES - 1_000]);
        let expected = TONES[0];
        assert!(
            (measured - expected).abs() < expected * 0.02,
            "{measured:.1} Hz should be {expected:.1} Hz"
        );
    }

    #[test]
    fn the_audible_chime_rises_from_note_to_note() {
        let mixed = samples();
        // Each window catches one note at its clearest: the earlier voices
        // have decayed away by the time the next one is centred there.
        let heard = [
            zero_crossing_hz(&mixed[1_000..4_800]),
            zero_crossing_hz(&mixed[6_000..9_600]),
            zero_crossing_hz(&mixed[11_000..12_700]),
        ];
        for (heard, intended) in heard.iter().zip(TONES) {
            assert!(
                (heard - intended).abs() < intended * 0.04,
                "{heard:.1} Hz should be near {intended:.1} Hz"
            );
        }
        assert!(
            heard.windows(2).all(|pair| pair[0] < pair[1]),
            "the chime must climb: {heard:?}"
        );
    }

    #[test]
    fn the_mixed_chime_has_no_clipped_samples() {
        let wav = chime();
        let peak = wav[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair).unsigned_abs())
            .max()
            .expect("the chime has samples");
        assert!(peak < i16::MAX.unsigned_abs(), "the mix stays inside i16");
    }
}
