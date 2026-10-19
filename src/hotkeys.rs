//! Global hotkeys: OS bindings registered on the tray window; modifier
//! data comes from the settings-derived hotkey choices handled here.

use crate::settings::Hotkey;

/// Role that a binding plays in the application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// Start/stop a recording.
    Record,
    /// Take a still photo.
    Photo,
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
pub const MOD_NOREPEAT: u32 = 0x4000;

pub const VK_1: u32 = 0x0031;
pub const VK_2: u32 = 0x0032;

/// Modifier flag of the parenthesized "ctrl" part of `ctrl+alt+1`.
#[must_use]
fn settings_modifiers(hotkey: Hotkey) -> u32 {
    match hotkey {
        // Every supported hotkey is ctrl+alt today.
        Hotkey::Record | Hotkey::Photo => MOD_CONTROL | MOD_ALT,
    }
}

/// Virtual-key code of the "1"/"2" part of the settings hotkey.
#[must_use]
fn settings_key(hotkey: Hotkey) -> u32 {
    match hotkey {
        Hotkey::Record => VK_1,
        Hotkey::Photo => VK_2,
    }
}

/// Build the current set of bindings from the settings view.
#[must_use]
pub fn bindings(hotkeys: (Hotkey, Hotkey)) -> Vec<Binding> {
    let (record, photo) = hotkeys;
    vec![
        Binding {
            id: 1,
            role: Role::Record,
            modifiers: settings_modifiers(record) | MOD_NOREPEAT,
            key: settings_key(record),
        },
        Binding {
            id: 2,
            role: Role::Photo,
            modifiers: settings_modifiers(photo) | MOD_NOREPEAT,
            key: settings_key(photo),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{Role, VK_1, bindings};
    use crate::settings::Hotkey;

    #[test]
    fn bindings_match_settings_roles() {
        let set = bindings((Hotkey::Record, Hotkey::Photo));
        assert_eq!(set.len(), 2);
        assert_eq!(set[0].role, Role::Record);
        assert_eq!(set[0].key, VK_1);
        assert_eq!(set[1].role, Role::Photo);
    }
}
