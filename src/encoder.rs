//! Video encoder selection. x264 is the current engine; swapping to VP9,
//! AV1, or hardware encoders later should only require edits to this
//! module, not changes anywhere else.

use crate::recording::Profile;

/// The video codec used for motion recordings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Video {
    /// `libx264`, the current default; kept GPL-compatible in the engine.
    X264,
}

impl Video {
    /// Codec used when the settings file omits a choice.
    pub const DEFAULT: Self = Self::X264;

    /// Parse a settings value (case-insensitive).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "x264" | "libx264" | "h264" => Some(Self::X264),
            _ => None,
        }
    }

    /// Identifier written to settings files and diagnostics.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::X264 => "x264",
        }
    }

    /// ffmpeg `-c:v` value.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::X264 => "libx264",
        }
    }

    /// Container file extension implied by the codec.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::X264 => "mp4",
        }
    }

    /// ffmpeg arguments that select and tune this codec; the CRF value is
    /// interpolated from the capture profile.
    #[must_use]
    pub fn args(self, profile: &Profile) -> Vec<String> {
        match self {
            Self::X264 => [
                "-c:v".to_owned(),
                "libx264".to_owned(),
                "-preset".to_owned(),
                "veryfast".to_owned(),
                "-crf".to_owned(),
                profile.quality.to_string(),
                "-pix_fmt".to_owned(),
                "yuv420p".to_owned(),
                "-movflags".to_owned(),
                "+faststart".to_owned(),
            ]
            .to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Video;
    use crate::recording::DEFAULT as DEFAULT_PROFILE;

    #[test]
    fn parses_settings_values() {
        assert_eq!(Video::parse("X264"), Some(Video::DEFAULT));
        assert_eq!(Video::parse("libx264"), Some(Video::DEFAULT));
        assert_eq!(Video::parse("vp9"), None);
    }

    #[test]
    fn args_interpolate_profile_quality() {
        let args = Video::DEFAULT.args(&DEFAULT_PROFILE);
        assert!(args.contains(&"17".to_owned()));
        assert_eq!(Video::DEFAULT.name(), "libx264");
        assert_eq!(Video::DEFAULT.extension(), "mp4");
    }
}
