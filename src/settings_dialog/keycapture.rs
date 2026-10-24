//! The hotkey fields as key recorders: focusing a field clears it,
//! the next combo pressed becomes its text, and the status line reports
//! conflicts from a real `RegisterHotKey` probe.
//!
//! A field is subclassed to notice focus/blur, and while it is focused a
//! low-level keyboard hook observes the keys. The hook matters: Windows
//! delivers a combo that another application already registered as
//! `WM_HOTKEY` instead of a key message, so a plain edit never sees it.
//! The hook sees every key first, records the combo, and consumes it, so
//! the field can report the conflict instead of silently ignoring the
//! key press.

use std::cell::RefCell;

use crate::guard::Hook;
use crate::settings::Hotkey;
use crate::winapi::{DefWindowProcW, Hwnd, Lparam, Lresult, Wparam};

use super::ffi::{
    CallWindowProcW, GWLP_WNDPROC, GetFocus, GetKeyState, GetWindowTextLengthW, GetWindowTextW,
    InvalidateRect, SetWindowLongPtrW, SetWindowTextW, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS,
    WM_LBUTTONDOWN, WM_PASTE, WM_SETFOCUS, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use super::fields::edit_text;
use super::{DIALOG, module_handle, wide, wide_to_string};

/// `WH_KEYBOARD_LL` (winuser.h): the low-level keyboard hook.
const KEYBOARD_HOOK: i32 = 13;
/// `LLKHF_UP` (winuser.h): set on a key-up event.
const KEY_UP: u32 = 0x80;
/// `VK_F4` (winuser.h): the key the window manager's close accelerator uses.
const VK_F4: u32 = 0x73;

/// Modifier bits tracked while the hook owns the keyboard.
const HELD_CTRL: u32 = 1;
const HELD_ALT: u32 = 2;
const HELD_SHIFT: u32 = 4;
const HELD_WIN: u32 = 8;

/// One captured field: its subclassed window and the last good combo.
struct Field {
    handle: Hwnd,
    id: i32,
    previous: isize,
    seed: String,
}

/// The focused field's key session; the hook lives only while it does.
struct Session {
    handle: Hwnd,
    /// The installed hook; held only so `Drop` removes it.
    _hook: Option<Hook>,
    /// The key whose press the hook consumed, so its release is consumed
    /// as a pair.
    captured: Option<u32>,
    /// Modifiers held while the hook consumes their key events; the hook
    /// cannot ask `GetKeyState` because it swallows the keys itself.
    held: u32,
}

thread_local! {
    static FIELDS: RefCell<Vec<Field>> = const { RefCell::new(Vec::new()) };
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

/// `KBDLLHOOKSTRUCT` (winuser.h): the low-level keyboard payload.
#[repr(C)]
struct KbdHook {
    vk_code: u32,
    scan_code: u32,
    flags: u32,
    time: u32,
    extra: usize,
}

crate::pin_layout!(KbdHook, 24; vk_code = 0, scan_code = 4, flags = 8,
    time = 12, extra = 16);

/// Turn `handle` into a key recorder; `seed` is the value restored when
/// the user leaves without pressing a valid combo.
pub(super) fn install(handle: Hwnd, id: i32, seed: &str) {
    // SAFETY: `handle` is a live edit control owned by the dialog; the
    // replacement procedure is a static function that lives forever.
    let previous = unsafe { SetWindowLongPtrW(handle, GWLP_WNDPROC, hook as *const () as isize) };
    if previous == 0 {
        // A refused subclass leaves the field as a plain edit; reading
        // it on Save still works, there is just no live capture.
        return;
    }
    FIELDS.with_borrow_mut(|fields| {
        fields.push(Field {
            handle,
            id,
            previous,
            seed: seed.to_owned(),
        });
    });
}

/// Update the status line under the fields; warnings paint red.
pub(super) fn set_status(text: &str, warning: bool) {
    DIALOG.with_borrow_mut(|dialog| dialog.hotkey_warning = warning);
    let status = DIALOG.with_borrow(|dialog| dialog.hotkey_status);
    let Some(status) = status else {
        return;
    };
    let encoded = wide(text);
    // SAFETY: live status control; the text outlives the call.
    unsafe {
        SetWindowTextW(status, encoded.as_ptr());
        InvalidateRect(status, std::ptr::null(), 1);
    }
}

/// The subclass procedure shared by every hotkey field.
unsafe extern "system" fn hook(
    window: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    let Some(previous) = FIELDS.with_borrow(|fields| {
        fields
            .iter()
            .find(|field| field.handle == window)
            .map(|field| field.previous)
    }) else {
        // Not one of ours: defer to the dialog default.
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    };
    match message {
        WM_SETFOCUS => {
            set_text(window, "");
            set_status(
                "Press the combo now: ctrl/alt/shift/win plus a letter, digit, or F1-F12",
                false,
            );
            start_session(window);
            // The edit's own focus handling maintains the caret; skipping
            // it made Windows hand focus straight back to the dialog.
            unsafe { CallWindowProcW(previous, window, message, wparam, lparam) }
        }
        WM_KILLFOCUS => {
            stop_session();
            let text = field_text(window);
            if Hotkey::parse(&text).is_some() {
                remember(window, &text);
            } else {
                set_text(window, &seed_for(window));
            }
            set_status("Click a field, then press the keys", false);
            unsafe { CallWindowProcW(previous, window, message, wparam, lparam) }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let key = u32::try_from(wparam).unwrap_or(0);
            // Leave dialog navigation alone: Escape, Tab, Enter, the caret
            // keys, and the system Alt+F4 close still reach the dialog.
            let system_close = message == WM_SYSKEYDOWN && key == VK_F4;
            if system_close || matches!(key, 0x1B | 0x09 | 0x0D | 0x21..=0x28) {
                return unsafe { CallWindowProcW(previous, window, message, wparam, lparam) };
            }
            // With the hook installed the key was already consumed; this
            // path only matters when Windows refused the hook.
            key_down(window, key);
            0
        }
        WM_KEYUP | WM_SYSKEYUP => {
            let key = u32::try_from(wparam).unwrap_or(0);
            // Only refresh the "modifiers held" preview; a finished combo
            // must survive the Ctrl/Shift release that follows it.
            if is_modifier(key) && Hotkey::parse(&field_text(window)).is_none() {
                preview_modifiers(window, held_flags());
            }
            0
        }
        WM_LBUTTONDOWN => {
            // Clicking a field that already has focus sends no SETFOCUS;
            // the click should still start a fresh capture.
            // SAFETY: a thread-local focus query.
            if unsafe { GetFocus() } == window {
                set_text(window, "");
                set_status(
                    "Press the combo now: ctrl/alt/shift/win plus a letter, digit, or F1-F12",
                    false,
                );
            }
            unsafe { CallWindowProcW(previous, window, message, wparam, lparam) }
        }
        // The recorder writes its own text; characters and paste stay out.
        WM_CHAR | WM_SYSCHAR | WM_PASTE => 0,
        _ => unsafe { CallWindowProcW(previous, window, message, wparam, lparam) },
    }
}

/// Install the keyboard hook for the field that just took focus.
fn start_session(window: Hwnd) {
    stop_session();
    // SAFETY: the procedure and instance stay valid for the process; the
    // guard removes the hook on blur.
    let hook = unsafe { Hook::install(KEYBOARD_HOOK, keyboard_proc, module_handle()) };
    SESSION.with_borrow_mut(|session| {
        *session = Some(Session {
            handle: window,
            _hook: hook,
            captured: None,
            held: held_flags(),
        });
    });
}

/// Remove the session and (through `Drop`) the hook.
fn stop_session() {
    SESSION.with_borrow_mut(Option::take);
}

/// Forget every field from a previous opening of the dialog; the trap
/// is a reopened dialog pushing a second entry for the same control ids,
/// so the stale window handle would win the lookup.
pub(super) fn reset() {
    stop_session();
    FIELDS.with_borrow_mut(Vec::clear);
}

/// The low-level keyboard hook: records combos even when another process
/// owns them, consuming the captured key so no app acts on it.
unsafe extern "system" fn keyboard_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0
        && let Some(handle) = SESSION.with_borrow(|session| session.as_ref().map(|s| s.handle))
    {
        let event = unsafe { &*(lparam as *const KbdHook) };
        let up = event.flags & KEY_UP != 0;
        let key = event.vk_code;
        if up {
            let released = SESSION.with_borrow_mut(|session| {
                session
                    .as_mut()
                    .and_then(|session| session.captured.take().filter(|key| *key == event.vk_code))
            });
            if released.is_some() {
                return 1;
            }
            if let Some(bit) = modifier_bit(key) {
                let held = SESSION.with_borrow_mut(|session| {
                    session.as_mut().map(|session| {
                        session.held &= !bit;
                        session.held
                    })
                });
                if let Some(held) = held
                    && Hotkey::parse(&field_text(handle)).is_none()
                {
                    preview_modifiers(handle, held);
                }
                // Swallow the modifier pair: the hook consumed its
                // press, so the system never saw the key go down.
                return 1;
            }
        } else if let Some(bit) = modifier_bit(key) {
            let held = SESSION.with_borrow_mut(|session| {
                session.as_mut().map(|session| {
                    session.held |= bit;
                    session.held
                })
            });
            if let Some(held) = held {
                preview_modifiers(handle, held);
            }
            // Keeping Win/Alt down away from the shell avoids the
            // Start menu and menu-bar beeps during a capture.
            return 1;
        } else if !is_navigation(key) {
            let held =
                SESSION.with_borrow(|session| session.as_ref().map_or(0, |session| session.held));
            // Alt+F4 belongs to the window manager: recording it as a combo
            // would leave the dialog without a keyboard way to close.
            if !closes_the_window(key, held) && key_down_from(handle, key, held) {
                SESSION.with_borrow_mut(|session| {
                    if let Some(session) = session.as_mut() {
                        session.captured = Some(key);
                    }
                });
                return 1;
            }
        }
    }
    // SAFETY: still inside the hook procedure.
    unsafe { crate::guard::call_next_hook(code, wparam, lparam) }
}

/// One non-modifier key: build the combo from the held modifiers, store
/// it, and report whether the system accepts it. `true` means the key
/// was handled and should be consumed.
fn key_down(window: Hwnd, key: u32) -> bool {
    if is_modifier(key) {
        preview_modifiers(window, held_flags());
        return false;
    }
    key_down_from(window, key, held_flags())
}

/// The fallback path's core: a non-modifier key with `held` bits.
fn key_down_from(window: Hwnd, key: u32, held: u32) -> bool {
    if key == 0x08 || key == 0x2E {
        // Backspace/Delete: clear the field so a fresh combo can be typed.
        set_text(window, "");
        set_status("Press the combo now", false);
        return true;
    }
    let (control, alt, shift, windows) = held_bools(held);
    let Some(combo) = Hotkey::from_vk(key, control, alt, shift, windows) else {
        set_text(window, "");
        set_status(
            "That key cannot be used; hold ctrl/alt/shift/win and press a letter, digit, or F1-F12",
            true,
        );
        return true;
    };
    let text = combo.to_combo();
    set_text(window, &text);
    remember(window, &text);
    if conflicts_with(window, combo) {
        set_status("Hotkeys must all differ", true);
    } else if crate::tray::hotkey_available(combo) {
        set_status("Available", false);
    } else {
        set_status("In use by another app (Windows does not say which)", true);
    }
    true
}

/// Show the modifiers currently held while the user is still reaching
/// for the key, e.g. `ctrl+shift`.
fn preview_modifiers(window: Hwnd, held: u32) {
    let mut parts = Vec::new();
    let (control, alt, shift, windows) = held_bools(held);
    for (set, name) in [
        (control, "ctrl"),
        (alt, "alt"),
        (shift, "shift"),
        (windows, "win"),
    ] {
        if set {
            parts.push(name);
        }
    }
    set_text(window, &parts.join("+"));
}

/// The four modifiers as Windows currently reports them.
fn held_flags() -> u32 {
    // SAFETY: key-state queries for this thread.
    let down = |key: i32| unsafe { GetKeyState(key) < 0 };
    let mut held = 0;
    if down(0x11) {
        held |= HELD_CTRL;
    }
    if down(0x12) {
        held |= HELD_ALT;
    }
    if down(0x10) {
        held |= HELD_SHIFT;
    }
    if down(0x5B) || down(0x5C) {
        held |= HELD_WIN;
    }
    held
}

/// Split held bits into the `(ctrl, alt, shift, win)` tuple `Hotkey` wants.
fn held_bools(held: u32) -> (bool, bool, bool, bool) {
    (
        held & HELD_CTRL != 0,
        held & HELD_ALT != 0,
        held & HELD_SHIFT != 0,
        held & HELD_WIN != 0,
    )
}

/// True for the modifier keys themselves (either side).
fn is_modifier(key: u32) -> bool {
    modifier_bit(key).is_some()
}

/// The held-bits flag for one modifier virtual key.
fn modifier_bit(key: u32) -> Option<u32> {
    match key {
        0x11 | 0xA2 | 0xA3 => Some(HELD_CTRL),  // VK_CONTROL, L/R
        0x12 | 0xA4 | 0xA5 => Some(HELD_ALT),   // VK_MENU, L/R
        0x10 | 0xA0 | 0xA1 => Some(HELD_SHIFT), // VK_SHIFT, L/R
        0x5B | 0x5C => Some(HELD_WIN),          // VK_LWIN / VK_RWIN
        _ => None,
    }
}

/// Keys the dialog manager owns; the recorder lets them through.
fn is_navigation(key: u32) -> bool {
    matches!(key, 0x09 | 0x0D | 0x1B | 0x21..=0x28)
}

/// Whether this press is the window manager's Alt+F4 close rather than a
/// combo to record.
fn closes_the_window(key: u32, held: u32) -> bool {
    key == VK_F4 && held & HELD_ALT != 0
}

/// Whether `combo` is already typed into another hotkey field; the
/// recorder warns while typing instead of waiting for Save.
fn conflicts_with(window: Hwnd, combo: Hotkey) -> bool {
    let id = FIELDS.with_borrow(|fields| {
        fields
            .iter()
            .find(|field| field.handle == window)
            .map(|field| field.id)
    });
    let Some(id) = id else {
        return false;
    };
    let others: Vec<i32> = FIELDS.with_borrow(|fields| {
        fields
            .iter()
            .filter(|field| field.id != id)
            .map(|field| field.id)
            .collect()
    });
    others
        .into_iter()
        .any(|other| Hotkey::parse(&edit_text(other)) == Some(combo))
}

fn field_text(handle: Hwnd) -> String {
    // SAFETY: live edit handle; the buffer is sized from its text length.
    let length = unsafe { GetWindowTextLengthW(handle) };
    if length <= 0 {
        return String::new();
    }
    let Ok(count) = usize::try_from(length).map(|length| length + 1) else {
        return String::new();
    };
    let mut buffer = vec![0u16; count];
    // SAFETY: `buffer` holds `length + 1` UTF-16 units.
    unsafe {
        GetWindowTextW(handle, buffer.as_mut_ptr(), length + 1);
    }
    wide_to_string(&buffer).trim().to_owned()
}

fn set_text(handle: Hwnd, text: &str) {
    let encoded = wide(text);
    // SAFETY: live edit handle; the encoded text outlives the call.
    unsafe {
        SetWindowTextW(handle, encoded.as_ptr());
    }
}

fn remember(handle: Hwnd, text: &str) {
    FIELDS.with_borrow_mut(|fields| {
        if let Some(field) = fields.iter_mut().find(|field| field.handle == handle) {
            text.clone_into(&mut field.seed);
        }
    });
}

fn seed_for(handle: Hwnd) -> String {
    FIELDS.with_borrow(|fields| {
        fields
            .iter()
            .find(|field| field.handle == handle)
            .map(|field| field.seed.clone())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::{HELD_ALT, HELD_CTRL, VK_F4, closes_the_window};

    #[test]
    fn alt_f4_stays_with_the_window_manager() {
        assert!(closes_the_window(VK_F4, HELD_ALT));
        assert!(closes_the_window(VK_F4, HELD_ALT | HELD_CTRL));
        // Plain F4 and Alt with any other key are ordinary combos.
        assert!(!closes_the_window(VK_F4, 0));
        assert!(!closes_the_window(0x58, HELD_ALT));
    }
}
