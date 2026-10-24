//! Global hotkey combos: parsing, canonical display, and the defaults.

/// A user-configurable global hotkey: any combination of `ctrl`,
/// `alt`, `shift` (and `win`) with a key — digits `0-9`, letters `a-z`,
/// or `F1`-`F12`. Written `ctrl+shift+x` (order-insensitive when
/// parsed, canonical on display).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a modifier set is genuinely four independent flags"
)]
pub struct Hotkey {
    /// Ctrl flag (the `control` alias parses too).
    pub control: bool,
    /// Alt flag.
    pub alt: bool,
    /// Shift flag.
    pub shift: bool,
    /// Win (logo) flag.
    pub windows: bool,
    /// Virtual-key code of the hotkey's key part.
    pub key: u32,
}

impl Hotkey {
    /// `ctrl+shift+x`: the record hotkey default.
    pub const RECORD: Self = Self {
        control: true,
        alt: false,
        shift: true,
        windows: false,
        key: VK_LETTER_BASE + (b'X' - b'A') as u32,
    };
    /// `ctrl+shift+z`: the photo hotkey default.
    pub const PHOTO: Self = Self {
        control: true,
        alt: false,
        shift: true,
        windows: false,
        key: VK_LETTER_BASE + (b'Z' - b'A') as u32,
    };
    /// `ctrl+shift+m`: the chime mute hotkey default.
    pub const CHIMES: Self = Self {
        control: true,
        alt: false,
        shift: true,
        windows: false,
        key: VK_LETTER_BASE + (b'M' - b'A') as u32,
    };

    /// Parse a combo like `ctrl+shift+x` (order-insensitive; separators
    /// `+` or `-`; the key part is a digit `0-9`, a letter `a-z`, or
    /// `F1`-`F12`). At least one modifier must be present — a bare key
    /// would claim global keystrokes.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut combo = Self {
            control: false,
            alt: false,
            shift: false,
            windows: false,
            key: 0,
        };
        let mut key = None;
        let lower = text.to_ascii_lowercase();
        for part in lower
            .split(['+', '-'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            match part {
                "ctrl" | "control" => combo.control = true,
                "alt" => combo.alt = true,
                "shift" => combo.shift = true,
                "win" | "windows" => combo.windows = true,
                other => {
                    if key.is_some() {
                        return None; // a second non-modifier part
                    }
                    key = Some(other);
                }
            }
        }
        if !combo.control && !combo.alt && !combo.shift && !combo.windows {
            return None;
        }
        let Some(other) = key else {
            return None; // modifiers alone carry no action
        };
        if let Ok(digit) = other.parse::<u32>() {
            if digit > 9 {
                return None;
            }
            combo.key = VK_DIGIT_BASE + digit;
            return Some(combo);
        }
        let bytes = other.as_bytes();
        if bytes.len() == 1 && bytes[0].is_ascii_lowercase() {
            combo.key = VK_LETTER_BASE + u32::from(bytes[0] - b'a');
            return Some(combo);
        }
        let function = other
            .strip_prefix('f')
            .and_then(|number| number.parse::<u32>().ok())
            .filter(|number| (1..=12).contains(number))
            .map(|number| VK_FUNCTION_BASE + number - 1);
        combo.key = function?;
        Some(combo)
    }

    /// Build a combo from a virtual-key code and the modifiers held
    /// while it was pressed; `None` for keys this syntax cannot name or
    /// for a bare key with no modifier.
    #[must_use]
    #[cfg_attr(
        all(not(windows), not(test)),
        expect(dead_code, reason = "only the Windows settings window reads combos")
    )]
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "the four modifier flags are the combo's coordinates"
    )]
    pub fn from_vk(key: u32, control: bool, alt: bool, shift: bool, windows: bool) -> Option<Self> {
        if !control && !alt && !shift && !windows {
            return None;
        }
        let named = (VK_DIGIT_BASE..=VK_DIGIT_BASE + 9).contains(&key)
            || (VK_LETTER_BASE..=VK_LETTER_BASE + 25).contains(&key)
            || (VK_FUNCTION_BASE..=VK_FUNCTION_BASE + 11).contains(&key);
        named.then_some(Self {
            control,
            alt,
            shift,
            windows,
            key,
        })
    }

    /// Canonical text (`ctrl+shift+x`), modifiers in fixed order, then
    /// the key as a digit, letter, or function key.
    #[must_use]
    pub fn to_combo(self) -> String {
        let mut text = String::new();
        for (set, name) in [
            (self.control, "ctrl"),
            (self.alt, "alt"),
            (self.shift, "shift"),
            (self.windows, "win"),
        ] {
            if set {
                if !text.is_empty() {
                    text.push('+');
                }
                text.push_str(name);
            }
        }
        text.push('+');
        text.push_str(&key_text(self.key));
        text
    }
}

/// The key part of a canonical combo (`1`, `x`, `F5`...).
#[must_use]
fn key_text(key: u32) -> String {
    if let Some(digit) = key.checked_sub(VK_DIGIT_BASE).filter(|d| *d <= 9) {
        return digit.to_string();
    }
    if let Some(offset) = key.checked_sub(VK_LETTER_BASE).filter(|o| *o <= 25) {
        return String::from_utf8_lossy(&[u8::try_from(u32::from(b'a') + offset).unwrap_or(b'?')])
            .into_owned();
    }
    if let Some(index) = key
        .checked_sub(VK_FUNCTION_BASE)
        .filter(|i| (0..=11).contains(i))
    {
        return format!("F{}", index + 1);
    }
    "?".to_owned()
}

/// `0x30` (winuser.h): `VK_0`; digits ascend from here.
const VK_DIGIT_BASE: u32 = 0x30;
/// `0x41` (winuser.h): `VK_A`; letters ascend from here.
const VK_LETTER_BASE: u32 = 0x41;
/// `0x70` (winuser.h): `VK_F1`; `F2`+ ascend from here.
const VK_FUNCTION_BASE: u32 = 0x70;

impl std::fmt::Display for Hotkey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.to_combo())
    }
}

#[cfg(test)]
mod tests {
    use super::Hotkey;

    #[test]
    fn virtual_keys_build_combos_only_with_a_modifier() {
        let combo = Hotkey::from_vk(0x58, true, false, true, false).expect("ctrl+shift+x");
        assert_eq!(combo, Hotkey::RECORD);
        assert_eq!(Hotkey::from_vk(0x58, false, false, false, false), None);
        // Letters, digits, and F1-F12 are nameable; punctuation is not.
        assert!(Hotkey::from_vk(0x30, true, false, false, false).is_some());
        assert!(Hotkey::from_vk(0x7B, true, false, false, false).is_some());
        assert!(Hotkey::from_vk(0xBA, true, false, false, false).is_none());
    }

    #[test]
    fn hotkey_combos_parse_the_known_shapes() {
        // The defaults, both orders, dash separator, and the letter key.
        assert_eq!(Hotkey::parse("ctrl+shift+x"), Some(Hotkey::RECORD));
        assert_eq!(Hotkey::parse("ctrl+shift+m"), Some(Hotkey::CHIMES));
        assert_eq!(Hotkey::parse("X-SHIFT-CTRL"), Some(Hotkey::RECORD));
        assert_eq!(
            Hotkey::parse("ctrl+alt+shift+f5".to_ascii_lowercase().as_str()),
            Hotkey::parse("ctrl+alt+shift+F5")
        );
        // Modifiers alone, or two keys, refuse.
        assert_eq!(Hotkey::parse("ctrl+shift"), None);
        assert_eq!(Hotkey::parse("x"), None); // a modifier is required
        assert_eq!(Hotkey::parse("ctrl+x+y"), None);
        assert_eq!(Hotkey::parse("ctrl+f13"), None);
        // Round-trip: canonical text reparses to the same combo.
        for text in ["ctrl+shift+x", "alt+f5", "win+ctrl+7", "shift+z"] {
            let combo = Hotkey::parse(text).expect("combo parses");
            assert_eq!(Hotkey::parse(&combo.to_combo()), Some(combo));
        }
    }
}
