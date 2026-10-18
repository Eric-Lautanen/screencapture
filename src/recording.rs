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
pub const DEFAULT: Profile = Profile::PROFILES[2];

impl Profile {
    /// All built-in profiles, smallest first.
    pub const PROFILES: [Self; 4] = [
        Self {
            id: "hd-1080p30",
            label: "1080p @ 30 fps",
            width: 1920,
            height: 1080,
            fps: 30,
            quality: 20,
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
        assert!(Profile::by_id("hd-1080p30").is_some());
        assert!(Profile::by_id("nope").is_none());
    }
}
