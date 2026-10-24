//! Recording audio: the system's own output through `WASAPI` loopback, the
//! microphone through a capture stream, or both mixed into one continuous
//! 48 kHz stereo stream that the encoder takes alongside the video frames.
//!
//! A source that refuses (no device, microphone blocked by privacy, a
//! format the engine will not convert) never fails the recording: it is
//! logged and the recording continues without it. Only a platform without
//! this module records silently by design.

#[cfg(windows)]
mod abi;
#[cfg(windows)]
pub(crate) mod devices;
#[cfg(windows)]
mod live;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod session;
#[cfg(windows)]
mod wasapi;

/// How much space the mixed track gets: the AAC bitrate written alongside
/// the frames. `High` is the default and is meant to be transparent: at
/// 320 kbps for 48 kHz stereo, the encoder keeps the whole audible band and
/// a music mix comes back the way it went in, which is worth the megabytes
/// on a recording somebody else has to listen to.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Quality {
    /// 96 kbps: voice-over clarity in a small file.
    Speech,
    /// 160 kbps: the usual middle for talking over a little music.
    Standard,
    /// 320 kbps: music and layered sound survive.
    #[default]
    High,
}

impl Quality {
    /// Every choice, smallest file first.
    pub const CHOICES: [Self; 3] = [Self::Speech, Self::Standard, Self::High];

    /// Parse a settings value (case-insensitive).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "speech" | "voice" | "low" => Some(Self::Speech),
            "standard" | "medium" | "normal" => Some(Self::Standard),
            "high" | "music" => Some(Self::High),
            _ => None,
        }
    }

    /// Identifier written to settings files and diagnostics.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Speech => "speech",
            Self::Standard => "standard",
            Self::High => "high",
        }
    }

    /// Dialog label: the choice and the bitrate it means.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Speech => "Speech (96 kbps)",
            Self::Standard => "Standard (160 kbps)",
            Self::High => "High (320 kbps)",
        }
    }

    /// ffmpeg `-b:a` value.
    #[must_use]
    pub fn bitrate(self) -> &'static str {
        match self {
            Self::Speech => "96k",
            Self::Standard => "160k",
            Self::High => "320k",
        }
    }
}

/// Which audio sources a recording asked for, and at what quality.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Wanted {
    /// Record the default microphone.
    pub mic: bool,
    /// Record what the default playback device plays.
    pub system: bool,
    /// AAC bitrate for the mixed track.
    pub quality: Quality,
}

/// Each source's fader, in decibels: what the mix multiplies that source
/// by before the two are summed. Windows owns the device volumes; these
/// are the app's own, so a quiet microphone and a loud game can be balanced
/// in the track without touching either device.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Levels {
    /// The microphone's fader.
    pub mic_db: i32,
    /// The system mix's fader.
    pub system_db: i32,
}

impl Levels {
    /// The quietest fader position: silence.
    pub const MIN_DB: i32 = -60;
    /// The loudest fader position.
    pub const MAX_DB: i32 = 12;

    /// `level` kept inside the mixable range.
    #[must_use]
    pub fn clamp_level(level: i32) -> i32 {
        level.clamp(Self::MIN_DB, Self::MAX_DB)
    }

    /// The linear gain one fader position means: 0 dB is unity, and the
    /// quietest position is silence rather than a rounding error. Only the
    /// Windows mixer scales samples, so only it asks.
    #[cfg(windows)]
    #[must_use]
    pub fn gain(db: i32) -> f32 {
        let db = Self::clamp_level(db);
        if db == Self::MIN_DB {
            return 0.0;
        }
        10.0_f32.powf(f32::from(i16::try_from(db).unwrap_or(0)) / 20.0)
    }

    /// Both sources' gains, microphone first.
    #[cfg(windows)]
    #[must_use]
    pub fn gains(self) -> (f32, f32) {
        (Self::gain(self.mic_db), Self::gain(self.system_db))
    }
}

/// The endpoints a take prefers, by endpoint id. `None` follows whatever
/// Windows calls the default for that flow; an id names one device and is
/// honoured again whenever that device comes back.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Devices {
    /// The microphone to record.
    pub mic: Option<String>,
    /// The playback device to record.
    pub system: Option<String>,
}

impl Wanted {
    /// One phrase for logs and diagnostics: the sources that were asked
    /// for, then the quality their track gets.
    #[must_use]
    pub fn describe(self) -> String {
        let mut sources: Vec<&str> = Vec::new();
        if self.system {
            sources.push("system");
        }
        if self.mic {
            sources.push("mic");
        }
        if sources.is_empty() {
            // The track is written either way: its buttons can bring a
            // source in mid-take, and only an existing pipe can carry it.
            return "silent audio track".to_owned();
        }
        format!("{} audio at {}", sources.join("+"), self.quality.id())
    }

    /// Whether the settings asked for any source. Only the Windows worker
    /// opens sources, so only it asks.
    #[cfg(windows)]
    #[must_use]
    pub fn any(self) -> bool {
        self.mic || self.system
    }
}

#[cfg(windows)]
pub(crate) use live::Live;
#[cfg(windows)]
pub(crate) use session::{Audio, Prepared, start};

/// No capture exists outside Windows, so a recording has no live switches.
/// The type still exists so the recorder needs no platform branches, and a
/// take with no sources simply never grows a button. It holds a unit rather
/// than being a unit struct so a take passes it the way the Windows one is
/// passed, by reference.
#[cfg(not(windows))]
#[derive(Clone, Debug, Default)]
pub(crate) struct Live(());

#[cfg(not(windows))]
impl Live {
    /// There is never a source to switch here.
    #[must_use]
    pub(crate) const fn new(_wanted: Wanted) -> Self {
        Self(())
    }
}

/// Everything needed to name an audio input; never built off Windows,
/// where recordings stay silent.
#[cfg(not(windows))]
pub(crate) struct Prepared;

#[cfg(not(windows))]
impl Prepared {
    /// No platform audio capture exists here; the switches stay inert.
    pub(crate) fn open(
        _wanted: Wanted,
        _devices: &Devices,
        _levels: Levels,
        _live: &Live,
    ) -> Option<Self> {
        None
    }

    /// No pipe exists here, so no input is ever named.
    #[expect(
        clippy::unused_self,
        reason = "the stub keeps the Windows signature, which names a pipe from `self`"
    )]
    pub(crate) fn path(&self) -> &'static str {
        ""
    }
}

/// A running audio worker; on platforms without capture there is nothing
/// to run.
#[cfg(not(windows))]
pub(crate) struct Audio;

#[cfg(not(windows))]
impl Audio {
    /// Nothing to stop.
    #[expect(
        clippy::unused_self,
        reason = "the stub keeps the Windows signature; the caller stops a handle it owns"
    )]
    pub(crate) const fn stop(self) {}
}

/// Start the worker for a prepared set of sources; a platform without
/// capture answers an inert handle. `origin` is the clock the recording
/// paces its frames by, which the audio timeline follows too.
#[cfg(not(windows))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the stub keeps the Windows signature, which fails when the worker thread cannot start"
)]
pub(crate) fn start(_prepared: Prepared, _origin: std::time::Instant) -> std::io::Result<Audio> {
    Ok(Audio)
}

#[cfg(test)]
mod tests {
    use super::Quality;

    #[test]
    fn quality_ids_round_trip() {
        for quality in Quality::CHOICES {
            assert_eq!(Quality::parse(quality.id()), Some(quality));
            assert!(quality.bitrate().ends_with('k'), "{}", quality.id());
            assert!(quality.label().contains("kbps"), "{}", quality.label());
        }
        assert_eq!(Quality::CHOICES.len(), 3);
        assert_eq!(Quality::parse("  MUSIC "), Some(Quality::High));
        assert_eq!(Quality::parse("tinny"), None);
        assert_eq!(Quality::default(), Quality::High);
    }

    #[test]
    fn a_recording_is_high_quality_until_told_otherwise() {
        let wanted = super::Wanted::default();
        assert_eq!(wanted.quality, Quality::High);
        assert!(!wanted.mic && !wanted.system);
    }

    #[test]
    fn a_take_describes_its_sources_and_quality() {
        let both = super::Wanted {
            mic: true,
            system: true,
            quality: Quality::Speech,
        };
        assert_eq!(both.describe(), "system+mic audio at speech");
        let mic_only = super::Wanted {
            system: false,
            ..both
        };
        assert_eq!(mic_only.describe(), "mic audio at speech");
        assert_eq!(
            super::Wanted::default().describe(),
            "silent audio track",
            "a take with no source set still writes the track"
        );
    }
}
