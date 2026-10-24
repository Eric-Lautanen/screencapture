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
    use super::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, Role, bindings, combo_modifiers};
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
}
