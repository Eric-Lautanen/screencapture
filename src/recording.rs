//! Capture profiles: named combinations of resolution, frame rate, and
//! encoder quality for both motion recordings and still frames.

/// One named capture profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Profile {
    /// Short machine id used by settings (e.g. `"uhd-4k60"`).
    pub id: &'static str,
    /// Human-readable name shown in menus.
    pub label: &'static str,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frames per second for motion capture.
    pub fps: u32,
    /// Raw quality level for the frame pipe (lower is higher quality).
    pub quality: u8,
}

/// The built-in default profile (4K @ 60 fps); indexes [`Profile::PROFILES`].
/// The test below pins that index to the `uhd-4k60` id, so adding profiles
/// cannot silently move the default.
pub const DEFAULT: Profile = Profile::PROFILES[7];

impl Profile {
    /// All built-in profiles, smallest first. The ladder trades pixel size
    /// for frame rate at each tier, and quality rises with resolution so a
    /// 480p clip does not cost more bits than it can show.
    pub const PROFILES: [Self; 9] = [
        Self {
            id: "sd-480p30",
            label: "480p @ 30 fps",
            width: 854,
            height: 480,
            fps: 30,
            quality: 23,
        },
        Self {
            id: "hd-720p30",
            label: "720p @ 30 fps",
            width: 1280,
            height: 720,
            fps: 30,
            quality: 22,
        },
        Self {
            id: "hd-720p60",
            label: "720p @ 60 fps",
            width: 1280,
            height: 720,
            fps: 60,
            quality: 21,
        },
        Self {
            id: "hd-1080p30",
            label: "1080p @ 30 fps",
            width: 1920,
            height: 1080,
            fps: 30,
            quality: 20,
        },
        Self {
            id: "hd-1080p60",
            label: "1080p @ 60 fps",
            width: 1920,
            height: 1080,
            fps: 60,
            quality: 19,
        },
        Self {
            id: "qhd-1440p60",
            label: "1440p @ 60 fps",
            width: 2560,
            height: 1440,
            fps: 60,
            quality: 18,
        },
        Self {
            id: "uhd-4k30",
            label: "4K @ 30 fps",
            width: 3840,
            height: 2160,
            fps: 30,
            quality: 18,
        },
        Self {
            id: "uhd-4k60",
            label: "4K @ 60 fps",
            width: 3840,
            height: 2160,
            fps: 60,
            quality: 17,
        },
        Self {
            id: "uhd-4k120",
            label: "4K @ 120 fps",
            width: 3840,
            height: 2160,
            fps: 120,
            quality: 17,
        },
    ];

    /// Find a profile by its settings id.
    #[must_use]
    pub fn by_id(id: &str) -> Option<Self> {
        Self::PROFILES.into_iter().find(|profile| profile.id == id)
    }

    /// The output frame for a capture of `size`: the largest frame that
    /// keeps the capture's aspect ratio inside this profile's resolution.
    ///
    /// A 1440x900 window recorded through a 720p profile therefore lands
    /// on 1152x720 instead of 1280x720 with black pillarbox bars: the
    /// profile is a size ceiling, not a new shape. Dimensions are rounded
    /// down to an even number (H.264's 4:2:0 chroma needs it) and never
    /// below two pixels.
    #[must_use]
    pub fn fit(self, size: (u32, u32)) -> (u32, u32) {
        let (width, height) = (u64::from(size.0), u64::from(size.1));
        if width == 0 || height == 0 {
            return (self.width, self.height);
        }
        let (max_width, max_height) = (u64::from(self.width), u64::from(self.height));
        // Decide which edge caps first by cross multiplication, so the
        // choice is exact instead of depending on rounded ratios.
        let (output_width, output_height) = if max_width * height <= max_height * width {
            (max_width, round_edge(height, max_width, width))
        } else {
            (round_edge(width, max_height, height), max_height)
        };
        (even(output_width), even(output_height))
    }
}

/// `pixels * limit / whole`, rounded to the nearest pixel.
fn round_edge(pixels: u64, limit: u64, whole: u64) -> u64 {
    (pixels * limit + whole / 2) / whole
}

/// Round an edge down to an even pixel count, never below two.
fn even(edge: u64) -> u32 {
    let edge = edge.max(2) & !1;
    u32::try_from(edge).unwrap_or(u32::MAX)
}

impl std::fmt::Display for Profile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} ({}x{} @{}fps, quality {})",
            self.label, self.width, self.height, self.fps, self.quality
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT, Profile};

    #[test]
    fn default_matches_an_indexed_profile() {
        assert_eq!(DEFAULT.id, "uhd-4k60");
        assert!(Profile::by_id(DEFAULT.id).is_some());
    }

    #[test]
    fn finds_profiles_and_rejects_unknown_ids() {
        for id in [
            "sd-480p30",
            "hd-720p30",
            "hd-720p60",
            "hd-1080p30",
            "hd-1080p60",
            "qhd-1440p60",
            "uhd-4k30",
            "uhd-4k60",
            "uhd-4k120",
        ] {
            assert!(Profile::by_id(id).is_some(), "missing profile {id}");
        }
        assert!(Profile::by_id("nope").is_none());
    }

    #[test]
    fn fitting_keeps_the_capture_aspect_ratio() {
        let hd = Profile::by_id("hd-720p60").expect("profile exists");
        // 16:10 window through a 16:9 profile: the full height, no bars.
        assert_eq!(hd.fit((1440, 900)), (1152, 720));
        // Same aspect as the profile: the profile box itself.
        assert_eq!(hd.fit((1280, 720)), (1280, 720));
        assert_eq!(hd.fit((2560, 1440)), (1280, 720));
        // Wider than 16:9: the width caps first and the height follows.
        assert_eq!(hd.fit((1920, 800)), (1280, 532));
        // Smaller captures are still raised to the profile box.
        assert_eq!(hd.fit((586, 493)), (856, 720));
        // Odd captures cannot produce odd frames.
        let (width, height) = hd.fit((1439, 899));
        assert_eq!((width % 2, height % 2), (0, 0));
        assert_eq!(height, 720);
        // A nonsense size falls back to the nominal frame.
        assert_eq!(hd.fit((0, 0)), (1280, 720));
    }

    #[test]
    fn fitting_never_exceeds_the_profile() {
        for profile in Profile::PROFILES {
            for size in [
                (1920, 1080),
                (1440, 900),
                (900, 1440),
                (3840, 1600),
                (586, 493),
                (1920, 1200),
            ] {
                let (width, height) = profile.fit(size);
                assert!(width <= profile.width, "{profile} widens {size:?}");
                assert!(height <= profile.height, "{profile} heightens {size:?}");
                assert!(width % 2 == 0 && height % 2 == 0);
                // The shape survives to within the even rounding of one edge.
                let captured = u64::from(size.0) * u64::from(height);
                let output = u64::from(width) * u64::from(size.1);
                let allowance = u64::from(size.0) + u64::from(size.1);
                assert!(captured.abs_diff(output) <= allowance);
            }
        }
    }

    #[test]
    fn profiles_are_unique_and_ordered_smallest_first() {
        let mut previous = (0, 0, 0);
        let mut ids = std::collections::BTreeSet::new();
        for profile in Profile::PROFILES {
            assert!(ids.insert(profile.id), "duplicate id {}", profile.id);
            let rank = (profile.width, profile.height, profile.fps);
            assert!(
                rank >= previous,
                "{} is listed before a smaller profile",
                profile.id
            );
            assert!(profile.width % 2 == 0 && profile.height % 2 == 0);
            previous = rank;
        }
        // The ladder really reaches down to 480p and stops at 4K.
        assert_eq!(Profile::PROFILES.first().map(|p| p.height), Some(480));
        assert_eq!(Profile::PROFILES.last().map(|p| p.height), Some(2160));
    }
}
