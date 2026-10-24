//! Global hotkeys: OS bindings registered on the tray window, mapped
//! directly from the settings-derived hotkey combos.

use crate::settings::Hotkey;

/// Role that a binding plays in the application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// Start/stop a recording.
    Record,
    /// Take a still photo.
    Photo,
    /// Mute or unmute the capture chimes.
    Chimes,
}

impl Role {
    /// The word this role is called by in messages and settings keys.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::Photo => "photo",
            Self::Chimes => "chimes",
        }
    }

    /// The `[hotkeys]` key this role is stored under.
    #[must_use]
    pub fn key(self) -> &'static str {
        self.label()
    }
}

/// One action's hotkey ladder: the built-in default plus the alternates
/// tried when another application already owns that default.
///
/// `ctrl+shift+x` is claimed by common vendor software (graphics and
/// capture overlays), which left recordings dead on machines where the
/// default never registered. A default that Windows refuses is replaced
/// by the first free alternate and saved, so the hotkey works on first
/// run instead of only warning about the conflict.
pub struct Ladder {
    /// Which action this ladder belongs to.
    pub role: Role,
    /// The built-in default, as shipped in settings.
    pub default: Hotkey,
    /// Free combos tried in order when the default is taken.
    pub alternates: [&'static str; 2],
}

impl Ladder {
    /// The combo that should replace `current` — `None` when the setting
    /// needs no change (it was customized, its default is free, or every
    /// alternate is taken too). `free` answers whether Windows would hand
    /// out a combo right now.
    #[must_use]
    pub fn replacement(&self, current: Hotkey, free: impl Fn(Hotkey) -> bool) -> Option<Hotkey> {
        if current != self.default || free(self.default) {
            return None;
        }
        self.alternates
            .iter()
            .filter_map(|text| Hotkey::parse(text))
            .find(|combo| free(*combo))
    }
}

/// The ladders for the three shippable actions.
pub const LADDERS: [Ladder; 3] = [
    Ladder {
        role: Role::Record,
        default: Hotkey::RECORD,
        alternates: ["ctrl+shift+9", "ctrl+alt+9"],
    },
    Ladder {
        role: Role::Photo,
        default: Hotkey::PHOTO,
        alternates: ["ctrl+shift+8", "ctrl+alt+8"],
    },
    Ladder {
        role: Role::Chimes,
        default: Hotkey::CHIMES,
        alternates: ["ctrl+shift+7", "ctrl+alt+7"],
    },
];

/// One OS-level hotkey binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    /// Registrant id passed to `RegisterHotKey`.
    pub id: i32,
    /// What action the binding performs.
    pub role: Role,
    /// Modifier flags.
    pub modifiers: u32,
    /// Virtual-key code.
    pub key: u32,
}

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
pub const MOD_NOREPEAT: u32 = 0x4000;

/// The `MOD_*` flags of a combo's control/alt/shift/win parts.
#[must_use]
pub(crate) fn combo_modifiers(combo: Hotkey) -> u32 {
    let mut flags = 0;
    if combo.control {
        flags |= MOD_CONTROL;
    }
    if combo.alt {
        flags |= MOD_ALT;
    }
    if combo.shift {
        flags |= MOD_SHIFT;
    }
    if combo.windows {
        flags |= MOD_WIN;
    }
    flags
}

/// Build the current set of bindings from the settings view.
#[must_use]
pub fn bindings(record: Hotkey, photo: Hotkey, chimes: Hotkey) -> Vec<Binding> {
    vec![
        Binding {
            id: 1,
            role: Role::Record,
            modifiers: combo_modifiers(record) | MOD_NOREPEAT,
            key: record.key,
        },
        Binding {
            id: 2,
            role: Role::Photo,
            modifiers: combo_modifiers(photo) | MOD_NOREPEAT,
            key: photo.key,
        },
        Binding {
            id: 3,
            role: Role::Chimes,
            modifiers: combo_modifiers(chimes) | MOD_NOREPEAT,
            key: chimes.key,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        LADDERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, Role, bindings, combo_modifiers,
    };
    use crate::settings::Hotkey;

    #[test]
    fn defaults_map_to_rebindable_bindings() {
        let set = bindings(Hotkey::RECORD, Hotkey::PHOTO, Hotkey::CHIMES);
        assert_eq!(set.len(), 3);
        assert_eq!(set[0].role, Role::Record);
        assert_eq!(set[0].key, Hotkey::RECORD.key);
        assert_eq!(
            set[0].modifiers & (MOD_CONTROL | MOD_SHIFT),
            MOD_CONTROL | MOD_SHIFT
        );
        assert_eq!(set[1].role, Role::Photo);
        assert_eq!(set[1].key, Hotkey::PHOTO.key);
        assert_eq!(set[2].role, Role::Chimes);
        assert_eq!(set[2].key, Hotkey::CHIMES.key);
    }

    #[test]
    fn custom_combos_carry_their_flags() {
        let custom = Hotkey::parse("win+ctrl+f2").expect("parses");
        let set = bindings(custom, Hotkey::PHOTO, Hotkey::CHIMES);
        assert_eq!(set[0].key, custom.key);
        assert_eq!(
            set[0].modifiers & (MOD_WIN | MOD_CONTROL),
            MOD_WIN | MOD_CONTROL
        );
        assert_eq!(set[1].modifiers & MOD_WIN, 0);
    }

    #[test]
    fn bare_modifier_flags_stay_available() {
        assert_eq!(
            combo_modifiers(Hotkey::parse("alt+f1").expect("parses")),
            MOD_ALT
        );
    }

    /// The record ladder: a taken default is replaced by its first free
    /// alternate, and a free default is left exactly as shipped.
    #[test]
    fn a_taken_default_falls_back_to_the_next_free_combo() {
        let ladder = &LADDERS[0];
        assert_eq!(ladder.role, Role::Record);
        let replacement = ladder.replacement(Hotkey::RECORD, |combo| combo != Hotkey::RECORD);
        assert_eq!(replacement, Hotkey::parse("ctrl+shift+9"));
        assert_eq!(ladder.replacement(Hotkey::RECORD, |_| true), None);
    }

    /// Every alternate taken keeps the configured combo: a wrong hotkey
    /// that the user can see beats silently moving their binding again.
    #[test]
    fn a_saturated_ladder_keeps_the_configured_combo() {
        let ladder = &LADDERS[0];
        assert_eq!(ladder.replacement(Hotkey::RECORD, |_| false), None);
    }

    /// A combo the user chose is never rewritten on their behalf.
    #[test]
    fn customised_combos_are_left_alone() {
        let ladder = &LADDERS[0];
        let custom = Hotkey::parse("alt+f4").expect("parses");
        assert_eq!(ladder.replacement(custom, |_| false), None);
    }

    /// Every alternate in every ladder parses: a typo there would simply
    /// disable the fallback.
    #[test]
    fn alternates_are_valid_combos() {
        for ladder in &LADDERS {
            for text in ladder.alternates {
                assert!(Hotkey::parse(text).is_some(), "{text} must parse");
                assert_ne!(ladder.default, Hotkey::parse(text).expect("parses"));
            }
        }
    }
}
