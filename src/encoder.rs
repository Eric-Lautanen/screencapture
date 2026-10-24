//! Video encoder selection. x264 is the universal default; the hardware
//! H.264 and HEVC units (AMD AMF, Intel QSV, NVIDIA NVENC, and Windows
//! Media Foundation) plus the CPU x265 belt are opt-in. Every choice is
//! probed with its exact argument set before first use. A failed choice
//! falls back through H.264 (x264, then hardware) and only reaches x265
//! when no H.264 encoder can run, so a missing accelerator never turns a
//! recording into HEVC by accident. Every anchored switch and the
//! per-encoder pixel format were verified against this engine's
//! `ffmpeg -h encoder` pages.

use crate::recording::Profile;

/// The video codec used for motion recordings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Video {
    /// `libx264`, software; runs everywhere and stays the default.
    X264,
    /// `libx265`, CPU HEVC; slower than x264 but anywhere it compiles.
    X265,
    /// `h264_amf`, AMD hardware.
    AmfH264,
    /// `h264_qsv`, Intel Quick Sync hardware.
    QsvH264,
    /// `h264_nvenc`, NVIDIA hardware.
    NvencH264,
    /// `h264_mf`, Windows Media Foundation (works without a discrete GPU).
    MfH264,
    /// `hevc_amf`, AMD hardware.
    AmfHevc,
    /// `hevc_qsv`, Intel Quick Sync hardware.
    QsvHevc,
    /// `hevc_nvenc`, NVIDIA hardware.
    NvencHevc,
    /// `hevc_mf`, Windows Media Foundation.
    MfHevc,
}

impl Video {
    /// Codec used when the settings file omits a choice.
    pub const DEFAULT: Self = Self::X264;

    /// Hardware variants in probe order; empty when software-only.
    #[must_use]
    pub fn hardware() -> [Self; 8] {
        [
            Self::AmfH264,
            Self::QsvH264,
            Self::NvencH264,
            Self::MfH264,
            Self::AmfHevc,
            Self::QsvHevc,
            Self::NvencHevc,
            Self::MfHevc,
        ]
    }

    /// True when this encoder needs an accelerator present.
    #[must_use]
    pub fn is_hardware(self) -> bool {
        matches!(
            self,
            Self::AmfH264
                | Self::QsvH264
                | Self::NvencH264
                | Self::MfH264
                | Self::AmfHevc
                | Self::QsvHevc
                | Self::NvencHevc
                | Self::MfHevc
        )
    }

    /// Alternatives tried when `self` cannot encode, in priority order:
    /// software H.264 first, then every hardware H.264 unit, and finally
    /// CPU x265. H.264 therefore wins over HEVC at every step; x265 is only
    /// a last resort for an engine built without libx264 and without a
    /// usable H.264 accelerator.
    #[must_use]
    pub fn fallback_order(self) -> Vec<Self> {
        let mut order = vec![
            Self::X264,
            Self::AmfH264,
            Self::QsvH264,
            Self::NvencH264,
            Self::MfH264,
            Self::X265,
        ];
        order.retain(|codec| *codec != self);
        order
    }

    /// The requested encoder when it works, otherwise the first usable
    /// entry in [`Self::fallback_order`]. `None` means no encoder on this
    /// engine can complete a synthetic encode with its real arguments.
    #[must_use]
    pub fn choose<F>(self, mut works: F) -> Option<Self>
    where
        F: FnMut(Self) -> bool,
    {
        if works(self) {
            return Some(self);
        }
        self.fallback_order()
            .into_iter()
            .find(|codec| works(*codec))
    }

    /// Parse a settings value (case-insensitive).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "x264" | "libx264" | "h264" => Some(Self::X264),
            "x265" | "libx265" | "hevc" => Some(Self::X265),
            "amf" | "h264_amf" => Some(Self::AmfH264),
            "qsv" | "h264_qsv" => Some(Self::QsvH264),
            "nvenc" | "h264_nvenc" => Some(Self::NvencH264),
            "mf" | "h264_mf" | "media-foundation" => Some(Self::MfH264),
            "amf-hevc" | "hevc_amf" => Some(Self::AmfHevc),
            "qsv-hevc" | "hevc_qsv" => Some(Self::QsvHevc),
            "nvenc-hevc" | "hevc_nvenc" => Some(Self::NvencHevc),
            "mf-hevc" | "hevc_mf" => Some(Self::MfHevc),
            _ => None,
        }
    }

    /// Identifier written to settings files and diagnostics.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::X264 => "x264",
            Self::X265 => "x265",
            Self::AmfH264 => "amf",
            Self::QsvH264 => "qsv",
            Self::NvencH264 => "nvenc",
            Self::MfH264 => "mf",
            Self::AmfHevc => "amf-hevc",
            Self::QsvHevc => "qsv-hevc",
            Self::NvencHevc => "nvenc-hevc",
            Self::MfHevc => "mf-hevc",
        }
    }

    /// ffmpeg `-c:v` value.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::X264 => "libx264",
            Self::X265 => "libx265",
            Self::AmfH264 => "h264_amf",
            Self::QsvH264 => "h264_qsv",
            Self::NvencH264 => "h264_nvenc",
            Self::MfH264 => "h264_mf",
            Self::AmfHevc => "hevc_amf",
            Self::QsvHevc => "hevc_qsv",
            Self::NvencHevc => "hevc_nvenc",
            Self::MfHevc => "hevc_mf",
        }
    }

    /// Container file extension implied by the codec.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::X264
            | Self::X265
            | Self::AmfH264
            | Self::QsvH264
            | Self::NvencH264
            | Self::MfH264
            | Self::AmfHevc
            | Self::QsvHevc
            | Self::NvencHevc
            | Self::MfHevc => "mp4",
        }
    }

    /// The piped frame format this encoder consumes. QSV's `h264_qsv` and
    /// `hevc_qsv` pages list only `nv12`/`qsv` inputs; everything else
    /// takes the `4:2:0` software planar form. `FFmpeg` converts the BGRA
    /// capture feed into whatever this names.
    #[must_use]
    fn pixel(self) -> &'static str {
        match self {
            Self::QsvH264 | Self::QsvHevc => "nv12",
            _ => "yuv420p",
        }
    }

    /// ffmpeg arguments that select and tune this codec. Hardware gets
    /// only options verified against this engine's `ffmpeg -h encoder`
    /// pages (`libx264`/`libx265` quality tuning from the capture
    /// profile). QSV's HEVC page carries no `-look_ahead` (its H.264
    /// page does), so `hevc_qsv` stays at vendor defaults.
    #[must_use]
    pub fn args(self, profile: &Profile) -> Vec<String> {
        let tuned: &[_] = match self {
            Self::X264 | Self::X265 => {
                &["-preset", "veryfast", "-crf", &profile.quality.to_string()]
            }
            // Vendor-documented "prefer quality" switches; rate control
            // stays at each unit's defaults, which are conservative.
            Self::AmfH264 | Self::AmfHevc => &["-quality", "high_quality"],
            Self::QsvH264 => &["-look_ahead", "true"],
            Self::NvencH264 | Self::NvencHevc => &["-preset", "p4", "-tune", "hq"],
            Self::QsvHevc | Self::MfH264 | Self::MfHevc => &[],
        };
        let mut args = vec!["-c:v".to_owned(), self.name().to_owned()];
        args.append(&mut tuned.iter().map(|part| (*part).to_owned()).collect());
        args.extend(["-pix_fmt".to_owned(), self.pixel().to_owned()]);
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
        assert_eq!(Video::parse("HEVC"), Some(Video::X265));
        assert_eq!(Video::parse("x265"), Some(Video::X265));
        assert_eq!(Video::parse("AMF"), Some(Video::AmfH264));
        assert_eq!(Video::parse("h264_nvenc"), Some(Video::NvencH264));
        assert_eq!(Video::parse("media-foundation"), Some(Video::MfH264));
        assert_eq!(Video::parse("amf-hevc"), Some(Video::AmfHevc));
        assert_eq!(Video::parse("hevc_qsv"), Some(Video::QsvHevc));
        assert_eq!(Video::parse("nvenc-hevc"), Some(Video::NvencHevc));
        assert_eq!(Video::parse("mf-hevc"), Some(Video::MfHevc));
        assert_eq!(Video::parse("vp9"), None);
    }

    #[test]
    fn hardware_flags_tell_the_variants_apart() {
        assert!(!Video::X264.is_hardware());
        assert!(!Video::X265.is_hardware());
        assert_eq!(Video::hardware().len(), 8);
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
    fn cpu_args_interpolate_profile_quality() {
        for codec in [Video::X264, Video::X265] {
            let args = codec.args(&DEFAULT_PROFILE);
            assert!(args.contains(&"17".to_owned()), "{codec:?} {args:?}");
            assert_eq!(args.contains(&"nv12".to_owned()), codec == Video::QsvH264);
        }
        assert_eq!(Video::DEFAULT.name(), "libx264");
    }

    #[test]
    fn fallbacks_keep_h264_ahead_of_hevc() {
        assert_eq!(
            Video::X264.fallback_order(),
            vec![
                Video::AmfH264,
                Video::QsvH264,
                Video::NvencH264,
                Video::MfH264,
                Video::X265,
            ],
            "hardware H.264 comes next, and HEVC is the last resort"
        );
        assert_eq!(
            Video::X265.choose(|codec| matches!(codec, Video::X264 | Video::MfH264)),
            Some(Video::X264),
            "an HEVC request still prefers H.264 when HEVC is unavailable"
        );
        assert_eq!(
            Video::X264.choose(|codec| matches!(codec, Video::MfH264 | Video::X265)),
            Some(Video::MfH264),
            "a hardware H.264 unit beats CPU HEVC"
        );
        assert_eq!(
            Video::AmfH264.choose(|codec| matches!(codec, Video::X264 | Video::QsvH264)),
            Some(Video::X264),
            "a failed hardware choice demotes to software H.264 first"
        );
        assert_eq!(
            Video::X264.choose(|codec| codec == Video::X265),
            Some(Video::X265),
            "x265 remains available when no H.264 encoder works"
        );
        assert_eq!(Video::X264.choose(|_| false), None);
    }

    #[test]
    fn qsv_inputs_match_the_engine_pages() {
        // The encoder pages list only nv12/qsv for the Quick Sync units;
        // the yuv420p default belongs to every other encoder.
        assert_eq!(Video::QsvH264.pixel(), "nv12");
        assert_eq!(Video::QsvHevc.pixel(), "nv12");
        for codec in [Video::X264, Video::X265, Video::AmfHevc, Video::MfHevc] {
            assert_eq!(codec.pixel(), "yuv420p");
        }
    }
}
