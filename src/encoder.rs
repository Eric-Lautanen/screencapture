//! Video encoder selection. x264 is the universal default; the hardware
//! H.264 units (AMD AMF, Intel QSV, NVIDIA NVENC, and Windows Media
//! Foundation) are opt-in and always probed with their exact argument set
//! before first use, so a machine without the accelerator demotes to x264
//! with a notice instead of recording a dead stream.

use crate::recording::Profile;

/// The video codec used for motion recordings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Video {
    /// `libx264`, software; runs everywhere and stays the default.
    X264,
    /// `h264_amf`, AMD hardware.
    AmfH264,
    /// `h264_qsv`, Intel Quick Sync hardware.
    QsvH264,
    /// `h264_nvenc`, NVIDIA hardware.
    NvencH264,
    /// `h264_mf`, Windows Media Foundation (works without a discrete GPU).
    MfH264,
}

impl Video {
    /// Codec used when the settings file omits a choice.
    pub const DEFAULT: Self = Self::X264;

    /// Hardware variants in probe order; empty when software-only.
    #[must_use]
    pub fn hardware() -> [Self; 4] {
        [Self::AmfH264, Self::QsvH264, Self::NvencH264, Self::MfH264]
    }

    /// True when this encoder needs an accelerator present.
    #[must_use]
    pub fn is_hardware(self) -> bool {
        self != Self::X264
    }

    /// Parse a settings value (case-insensitive).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "x264" | "libx264" | "h264" => Some(Self::X264),
            "amf" | "h264_amf" => Some(Self::AmfH264),
            "qsv" | "h264_qsv" => Some(Self::QsvH264),
            "nvenc" | "h264_nvenc" => Some(Self::NvencH264),
            "mf" | "h264_mf" | "media-foundation" => Some(Self::MfH264),
            _ => None,
        }
    }

    /// Identifier written to settings files and diagnostics.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::X264 => "x264",
            Self::AmfH264 => "amf",
            Self::QsvH264 => "qsv",
            Self::NvencH264 => "nvenc",
            Self::MfH264 => "mf",
        }
    }

    /// ffmpeg `-c:v` value.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::X264 => "libx264",
            Self::AmfH264 => "h264_amf",
            Self::QsvH264 => "h264_qsv",
            Self::NvencH264 => "h264_nvenc",
            Self::MfH264 => "h264_mf",
        }
    }

    /// Container file extension implied by the codec.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::X264 | Self::AmfH264 | Self::QsvH264 | Self::NvencH264 | Self::MfH264 => "mp4",
        }
    }

    /// ffmpeg arguments that select and tune this codec. Hardware gets
    /// only options verified against this engine's `ffmpeg -h encoder`
    /// pages (`libx264` quality tuning from the capture profile).
    #[must_use]
    pub fn args(self, profile: &Profile) -> Vec<String> {
        let tuned: &[_] = match self {
            Self::X264 => &["-preset", "veryfast", "-crf", &profile.quality.to_string()],
            // Vendor-documented "prefer quality" switches; rate control
            // stays at each unit's defaults, which are conservative.
            Self::AmfH264 => &["-quality", "high_quality"],
            Self::QsvH264 => &["-look_ahead", "true"],
            Self::NvencH264 => &["-preset", "p4", "-tune", "hq"],
            Self::MfH264 => &[],
        };
        let mut args = vec!["-c:v".to_owned(), self.name().to_owned()];
        args.append(&mut tuned.iter().map(|part| (*part).to_owned()).collect());
        args.extend(["-pix_fmt".to_owned(), "yuv420p".to_owned()]);
        args.extend(["-movflags".to_owned(), "+faststart".to_owned()]);
        args
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
        assert_eq!(Video::parse("AMF"), Some(Video::AmfH264));
        assert_eq!(Video::parse("h264_nvenc"), Some(Video::NvencH264));
        assert_eq!(Video::parse("media-foundation"), Some(Video::MfH264));
        assert_eq!(Video::parse("vp9"), None);
    }

    #[test]
    fn hardware_flags_tell_the_variants_apart() {
        assert!(!Video::X264.is_hardware());
        for variant in Video::hardware() {
            assert!(variant.is_hardware());
            assert_eq!(variant.extension(), "mp4");
            assert_eq!(Video::parse(variant.id()), Some(variant));
            let args = variant.args(&DEFAULT_PROFILE);
            assert!(
                args.contains(&variant.name().to_owned()),
                "{} must select its encoder: {args:?}",
                variant.id()
            );
        }
    }

    #[test]
    fn x264_args_interpolate_profile_quality() {
        let args = Video::DEFAULT.args(&DEFAULT_PROFILE);
        assert!(args.contains(&"17".to_owned()));
        assert_eq!(Video::DEFAULT.name(), "libx264");
    }
}
