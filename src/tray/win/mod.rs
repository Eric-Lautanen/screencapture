//! Windows tray implementation: notification icon, message loop, and
//! command dispatch (single construct per run, all on the pump thread).

mod ffi;
mod ico;
mod menu;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::thread_local;

use self::ffi::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GETMESSAGE_QUIT, GetMessageW, Guid, Hicon, Hwnd, KillTimer, Lparam, Lresult, Msg, NIF_ICON,
    NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NotifyIconDataW,
    Point, PostMessageW, PostQuitMessage, RegisterClassExW, RegisterHotKey, SetTimer,
    Shell_NotifyIconW, TranslateMessage, UnregisterClassW, UnregisterHotKey, WM_COMMAND, WM_CREATE,
    WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WM_USER, WndClassExW, Wparam,
};
use super::{Error, Event};
use crate::hotkeys::{Binding, MOD_NOREPEAT, combo_modifiers};
use crate::recording::Profile;
use crate::settings::Hotkey;
use crate::winapi::{kept_wide, module_handle, wide};

/// Tray notification callback message id (custom to this window).
const TRAY_CALLBACK: u32 = WM_USER + 1;
/// Tray icon identifier within this process.
const TRAY_ID: u32 = 1;
/// Class name for the hidden message window (kept for the OS).
const CLASS_NAME: &str = "ScreencaptureTray";
/// Title for the hidden message window.
const WINDOW_NAME: &str = "Screencapture";
/// Tooltip shown next to the tray icon.
const TOOLTIP: &str = "screencapture";

/// Menu command ids; all sit in the low word of `WM_COMMAND`.
const ID_RECORD: Wparam = 1;
const ID_PHOTO: Wparam = 2;
const ID_SETTINGS: Wparam = 3;
/// The quick capture-chime mute entry.
const ID_CHIMES: Wparam = 4;
/// Sent by the recording border's Stop button through [`post_stop`].
const ID_STOP: Wparam = 5;
const ID_PROFILE_BASE: Wparam = 100;
/// Per-display entries live in `ID_DISPLAY_BASE..ID_EXIT`.
const ID_DISPLAY_BASE: Wparam = 120;
const ID_EXIT: Wparam = 200;
/// Frame-timer id for `WM_TIMER`.
const TIMER_ID: usize = 1;

thread_local! {
    static HANDLER: RefCell<Handler> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<Hwnd>> = const { RefCell::new(None) };
    static BINDINGS: RefCell<Vec<Binding>> = const { RefCell::new(Vec::new()) };
    /// The bindings Windows actually accepted. A combo listed here belongs
    /// to this process, which is the one answer a probe cannot give.
    static LIVE: RefCell<Vec<Binding>> = const { RefCell::new(Vec::new()) };
    /// Events that arrived while a handler dispatch still owned the
    /// borrow; replayed in order when the outer dispatch returns.
    static PENDING: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    /// Chime state for the menu's check mark, kept in step with settings.
    static CHIMES: Cell<bool> = const { Cell::new(true) };
}

type Handler = Option<Box<dyn FnMut(Event)>>;

/// Enter the tray loop; returns when the icon is removed.
///
/// # Errors
/// Returns an error when Windows refuses the window class or window.
pub(super) fn run<F>(handler: F, bindings: &[Binding]) -> Result<(), Error>
where
    F: FnMut(Event) + 'static,
{
    HANDLER.with(|slot| *slot.borrow_mut() = Some(Box::new(handler)));
    BINDINGS.with(|slot| *slot.borrow_mut() = bindings.to_vec());

    let class_name = kept_wide(CLASS_NAME);
    let instance = module_handle();
    let class = WndClassExW {
        size: u32::try_from(std::mem::size_of::<WndClassExW>()).expect("class struct fits u32"),
        style: 0,
        wnd_proc: on_message,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: std::ptr::null_mut(),
        cursor: std::ptr::null_mut(),
        background: std::ptr::null_mut(),
        menu_name: std::ptr::null(),
        class_name,
        icon_small: std::ptr::null_mut(),
    };
    let atom = {
        // SAFETY: `class` points to a fully initialized local struct.
        unsafe { RegisterClassExW(&raw const class) }
    };
    if atom == 0 {
        return Err(Error::System("RegisterClassExW failed".into()));
    }

    let window_name = wide(WINDOW_NAME);
    let window = create_hidden_window(class_name, window_name.as_ptr(), instance);
    if window.is_null() {
        // SAFETY: `class_name` is the leaked static name registered above.
        let _ = unsafe { UnregisterClassW(class_name, instance) };
        return Err(Error::System("CreateWindowExW failed".into()));
    }
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window));
    register_hotkeys(window);
    pump();
    WINDOW.with(|slot| *slot.borrow_mut() = None);
    // SAFETY: `class_name` is the leaked static name registered above.
    let _ = unsafe { UnregisterClassW(class_name, instance) };
    Ok(())
}

/// Record the chime state the tray menu should render on its next open.
pub(super) fn set_chimes(enabled: bool) {
    CHIMES.with(|slot| slot.set(enabled));
}

/// Whether captures currently chime; the menu renders the inverse as a
/// check beside its mute entry.
pub(super) fn chimes_enabled() -> bool {
    CHIMES.with(Cell::get)
}

/// Swap the global hotkey registrations for a fresh set (the settings
/// window rebinds): the old registrations lift off first, so a combo
/// that moved registers cleanly, and failures print honestly.
pub(super) fn set_bindings(new: Vec<Binding>) {
    let window = WINDOW.with_borrow(|slot| *slot);
    if let Some(window) = window {
        unregister_hotkeys(window);
        BINDINGS.with_borrow_mut(|slot| *slot = new);
        register_hotkeys(window);
    } else {
        BINDINGS.with_borrow_mut(|slot| *slot = new);
    }
}

/// Turn `Option<u32>` into timer state: `None` kills the watchdog timer.
pub(super) fn set_watchdog_timer(interval_ms: Option<u32>) {
    let Some(window) = WINDOW.with_borrow(|slot| *slot) else {
        return;
    };
    match interval_ms {
        Some(ms) => {
            // SAFETY: the timer hangs off our own hidden window.
            unsafe { SetTimer(window, TIMER_ID, ms, std::ptr::null()) };
        }
        None => {
            // SAFETY: removing the timer registered on our own window.
            unsafe { KillTimer(window, TIMER_ID) };
        }
    }
}

/// Whether Windows would accept `combo` right now. The settings window
/// asks this while a hotkey field records the user's keys.
///
/// A combo this process already holds answers yes without asking Windows:
/// `RegisterHotKey` refuses a second registration of the same combo even for
/// the process that owns it, so a probe would call our own binding someone
/// else's. Everything else is a real probe on a spare id, and the live
/// bindings are never lifted for it — a question about a new combo must not
/// be able to cost the user a hotkey that works.
pub(super) fn hotkey_available(combo: Hotkey) -> bool {
    let window = WINDOW
        .with_borrow(|slot| *slot)
        .unwrap_or(std::ptr::null_mut());
    LIVE.with_borrow(|live| {
        availability(live, combo, |modifiers, key| probe(window, modifiers, key))
    })
}

/// The rule behind [`hotkey_available`], with the probe injected so the
/// owned-combo case can be tested without registering anything.
fn availability(live: &[Binding], combo: Hotkey, probe: impl FnOnce(u32, u32) -> bool) -> bool {
    let modifiers = combo_modifiers(combo) | MOD_NOREPEAT;
    let owned = live
        .iter()
        .any(|binding| binding.modifiers == modifiers && binding.key == combo.key);
    if owned {
        return true;
    }
    probe(modifiers, combo.key)
}

/// One `RegisterHotKey` probe on an id outside the three tray bindings,
/// removed immediately when it succeeds.
fn probe(window: Hwnd, modifiers: u32, key: u32) -> bool {
    /// `0x7FFE` sits above every binding id the tray hands out.
    const PROBE_ID: i32 = 0x7FFE;
    // SAFETY: a registration on our own window (or thread); removed at once.
    let registered = unsafe { RegisterHotKey(window, PROBE_ID, modifiers, key) } != 0;
    if registered {
        // SAFETY: symmetric removal of the probe above.
        unsafe { UnregisterHotKey(window, PROBE_ID) };
    }
    registered
}

fn register_hotkeys(window: Hwnd) {
    let mut live: Vec<Binding> = Vec::new();
    for binding in BINDINGS.with_borrow(Vec::clone) {
        // SAFETY: registrations live on our own window and are removed on
        // `WM_DESTROY`.
        let registered =
            unsafe { RegisterHotKey(window, binding.id, binding.modifiers, binding.key) };
        if registered == 0 {
            // The ring log is the only record that survives the tray: a
            // detached process has no console to print to.
            crate::log::note(
                "warn",
                &format!(
                    "the {} hotkey did not register; another app already owns it",
                    binding.role.label()
                ),
            );
            notify_hotkey_taken(binding.role);
        } else {
            live.push(binding);
        }
    }
    LIVE.with_borrow_mut(|slot| *slot = live);
}

/// Say something in the tray. The log alone is invisible to a tray-only
/// user, so a capture that refused to start — or a hotkey another app
/// took — has to reach the screen as well.
pub(super) fn warn(title: &str, message: &str) {
    let Some(window) = WINDOW.with_borrow(|slot| *slot) else {
        return;
    };
    let mut data = icon_data(window, std::ptr::null_mut());
    data.flags = NIF_INFO;
    data.info_flags = NIIF_WARNING;
    fill_text(&mut data.info_title, title);
    fill_text(&mut data.info, message);
    // SAFETY: `data` is a local initialized struct bound to the live icon.
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &raw mut data) };
}

/// A saved hotkey could not be registered: name the action that lost it.
fn notify_hotkey_taken(role: crate::hotkeys::Role) {
    warn(
        "Hotkey unavailable",
        &format!(
            "The {} hotkey is already used by another app. Open Settings to choose another.",
            role.label()
        ),
    );
}

fn unregister_hotkeys(window: Hwnd) {
    for binding in BINDINGS.with_borrow(Vec::clone) {
        // SAFETY: symmetric teardown of the registration made at startup.
        unsafe { UnregisterHotKey(window, binding.id) };
    }
    LIVE.with_borrow_mut(Vec::clear);
}

fn create_hidden_window(
    class_name: *const u16,
    window_name: *const u16,
    instance: *mut std::ffi::c_void,
) -> Hwnd {
    // SAFETY: class_name is a leaked static buffer; window_name outlives the
    // call; parent, menu, and param are intentionally unused.
    unsafe {
        CreateWindowExW(
            0,
            class_name,
            window_name,
            0,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        )
    }
}

fn pump() {
    let mut message = Msg {
        hwnd: std::ptr::null_mut(),
        message: 0,
        w_param: 0,
        l_param: 0,
        time: 0,
        point: Point { x: 0, y: 0 },
    };
    loop {
        // SAFETY: `message` is a local struct owned by this loop; the chain
        // of pump calls happens on one thread.
        let received = unsafe { GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) };
        if received == GETMESSAGE_QUIT || received < 0 {
            break;
        }
        let _ = unsafe { TranslateMessage(&raw const message) };
        unsafe { DispatchMessageW(&raw const message) };
    }
}

unsafe extern "system" fn on_message(
    window: Hwnd,
    message: u32,
    w_param: Wparam,
    l_param: Lparam,
) -> Lresult {
    if message == WM_CREATE {
        return add_icon(window);
    }
    if message == TRAY_CALLBACK {
        // Mouse-up notifications arrive in `l_param` as small positives.
        let clicks = u32::try_from(l_param).unwrap_or(0);
        if clicks == WM_LBUTTONUP || clicks == WM_RBUTTONUP {
            // SAFETY: `window` is the tray window owned by the pump thread.
            unsafe {
                menu::show(window);
            }
        }
        return 0;
    }
    if message == WM_COMMAND {
        return dispatch_command(w_param & 0xffff, window);
    }
    if message == WM_TIMER && w_param == TIMER_ID {
        emit(Event::Watchdog);
        return 0;
    }
    if message == WM_HOTKEY {
        dispatch_hotkey(w_param);
        return 0;
    }
    if message == WM_DESTROY {
        // SAFETY: standard teardown path on the pump thread.
        unsafe {
            unregister_hotkeys(window);
            remove_icon(window);
            PostQuitMessage(0);
        }
        return 0;
    }
    // SAFETY: pass-through to the default window procedure.
    unsafe { DefWindowProcW(window, message, w_param, l_param) }
}

fn dispatch_command(command: Wparam, window: Hwnd) -> Lresult {
    match command & 0xffff {
        ID_RECORD => emit(Event::Record),
        ID_PHOTO => emit(Event::Photo),
        ID_SETTINGS => emit(Event::OpenSettings),
        ID_CHIMES => emit(Event::ToggleChimes),
        ID_STOP => emit(Event::Stop),
        ID_EXIT => {
            emit(Event::Exit);
            // SAFETY: the tray window belongs to the pump thread; destroy it
            // to trigger `WM_DESTROY` cleanup and quit the pump.
            let _ = unsafe { DestroyWindow(window) };
            return 0;
        }
        offset if (ID_PROFILE_BASE..ID_DISPLAY_BASE).contains(&offset) => {
            let index = offset - ID_PROFILE_BASE;
            if let Some(profile) = Profile::PROFILES.get(index) {
                emit(Event::ProfileSelected(*profile));
            }
        }
        offset if (ID_DISPLAY_BASE..ID_EXIT).contains(&offset) => {
            // Menu ids are built from a fresh enumeration each open, so a
            // click implies the display was attached after the menu built.
            emit(Event::RecordDisplay(
                u32::try_from(offset - ID_DISPLAY_BASE + 1).unwrap_or(1),
            ));
        }
        _ => {}
    }
    0
}

/// Post the stop command to the tray window so the border is not
/// destroyed while its own window procedure is still on the stack.
pub(super) fn post_stop() {
    let Some(window) = WINDOW.with_borrow(|slot| *slot) else {
        return;
    };
    // SAFETY: posting to this thread's own window; no out-parameters.
    unsafe { PostMessageW(window, WM_COMMAND, ID_STOP, 0) };
}

fn dispatch_hotkey(id: Wparam) {
    let role = BINDINGS.with_borrow(|slot| {
        slot.iter()
            .find(|binding| binding.id == i32::try_from(id).unwrap_or(0))
            .map(|binding| binding.role)
    });
    if let Some(role) = role {
        let event = match role {
            crate::hotkeys::Role::Record => Event::Record,
            crate::hotkeys::Role::Photo => Event::Photo,
            crate::hotkeys::Role::Chimes => Event::ToggleChimes,
        };
        // Drop hotkeys that fire while a modal surface owns the handler.
        HANDLER.with(|slot| {
            if let Ok(mut borrower) = slot.try_borrow_mut()
                && let Some(handler) = borrower.as_mut()
            {
                handler(event);
            }
        });
    }
}

fn emit(event: Event) {
    if dispatch(event) {
        drain();
    }
}

/// One dispatch attempt: `true` when the handler answered the event,
/// `false` when it was queued behind a dispatch still on the stack.
fn dispatch(event: Event) -> bool {
    HANDLER.with(|slot| {
        if let Ok(mut borrower) = slot.try_borrow_mut() {
            if let Some(handler) = borrower.as_mut() {
                handler(event);
            }
            true
        } else {
            PENDING.with_borrow_mut(|queue| queue.push_back(event));
            false
        }
    })
}

/// Replay queued events while the handler answers.
fn drain() {
    while let Some(event) = PENDING.with_borrow_mut(VecDeque::pop_front) {
        if !dispatch(event) {
            return;
        }
    }
}

fn add_icon(window: Hwnd) -> Lresult {
    // SAFETY: builds the authored icon on the tray's own thread.
    let icon = unsafe { ico::app_or_fallback() };
    let mut data = icon_data(window, icon);
    fill_text(&mut data.tip, TOOLTIP);
    // SAFETY: `data` is a local, fully initialized struct bound to `window`.
    if unsafe { Shell_NotifyIconW(NIM_ADD, &raw mut data) } == 0 {
        -1
    } else {
        0
    }
}

fn icon_data(window: Hwnd, icon: Hicon) -> NotifyIconDataW {
    NotifyIconDataW {
        size: u32::try_from(std::mem::size_of::<NotifyIconDataW>())
            .expect("notify icon data struct fits u32"),
        window,
        id: TRAY_ID,
        flags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        callback_message: TRAY_CALLBACK,
        icon,
        tip: [0; 128],
        state: 0,
        state_mask: 0,
        info: [0; 256],
        version: 0,
        info_title: [0; 64],
        info_flags: 0,
        guid: Guid::default(),
    }
}

/// Copy `text` into a fixed UTF-16 buffer, zero-filling the remainder.
fn fill_text(buffer: &mut [u16], text: &str) {
    buffer.fill(0);
    for (slot, character) in buffer.iter_mut().zip(wide(text)) {
        *slot = character;
    }
}

fn remove_icon(window: Hwnd) {
    let mut data = icon_data(window, std::ptr::null_mut());
    // SAFETY: `data` is a local, fully initialized struct bound to `window`.
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &raw mut data) };
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::{BINDINGS, HANDLER, PENDING, availability, dispatch_hotkey, emit};
    use crate::hotkeys::{MOD_ALT, MOD_NOREPEAT, bindings};
    use crate::settings::Hotkey;
    use crate::tray::Event;

    /// A live set covering the three shipped combos.
    fn live() -> Vec<crate::hotkeys::Binding> {
        bindings(Hotkey::RECORD, Hotkey::PHOTO, Hotkey::CHIMES)
    }

    /// A combo this process already holds answers yes without a probe:
    /// registering it again is what Windows refuses, not a conflict.
    #[test]
    fn an_owned_combo_answers_free_without_probing() {
        let mut probed = false;
        let free = availability(&live(), Hotkey::RECORD, |_, _| {
            probed = true;
            false
        });
        assert!(free, "our own record combo stays usable");
        assert!(!probed, "the probe must not run for a combo we own");
    }

    /// The same key under different modifiers is a separate registration,
    /// so Windows has to decide.
    #[test]
    fn a_shared_key_under_other_modifiers_asks_windows() {
        let combo = Hotkey::parse("alt+shift+x").expect("parses");
        let mut probed = false;
        let free = availability(&live(), combo, |_, _| {
            probed = true;
            true
        });
        assert!(free);
        assert!(probed, "alt+shift+x is not ctrl+shift+x");
    }

    /// A foreign combo is answered by the probe, with the no-repeat flag
    /// the tray always registers with.
    #[test]
    fn a_foreign_combo_is_answered_by_windows() {
        let combo = Hotkey::parse("ctrl+alt+f9").expect("parses");
        let mut asked = None;
        let free = availability(&live(), combo, |modifiers, key| {
            asked = Some((modifiers, key));
            false
        });
        assert!(!free, "a refused probe means another app owns the combo");
        assert_eq!(
            asked,
            Some((
                crate::hotkeys::combo_modifiers(combo) | MOD_NOREPEAT,
                combo.key
            ))
        );
        assert_ne!(asked.map(|(modifiers, _)| modifiers & MOD_ALT), Some(0));
    }

    /// The regression behind the abort users could hit: dispatching an
    /// event *from inside a handler* (a modal picker pumping messages
    /// re-enters the tray window procedure) once panicked the `RefCell`
    /// borrow. Queued dispatch answers the event after the outer one.
    #[test]
    fn reentrant_dispatch_queues_and_replays() {
        let handled = Rc::new(RefCell::new(Vec::<String>::new()));
        HANDLER.with(|slot| {
            let seen = Rc::clone(&handled);
            *slot.borrow_mut() = Some(Box::new(move |event| {
                let tag = match event {
                    Event::Record => "record",
                    Event::Photo => "photo",
                    _ => "other",
                };
                seen.borrow_mut().push(tag.to_owned());
                if let Event::Record = event {
                    // Double dispatch while the borrow is still alive:
                    // queued by the fix, panicked before it. The replay
                    // happens when the outer dispatch returns, so the
                    // Photo event runs after this handler ends.
                    emit(Event::Photo);
                }
            }));
        });

        emit(Event::Record);

        let seen = handled.borrow().clone();
        assert_eq!(seen, vec!["record", "photo"], "both events answer");
        assert!(PENDING.with_borrow(std::collections::VecDeque::is_empty));
        HANDLER.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn the_chime_binding_emits_the_mute_toggle() {
        let fired = Rc::new(Cell::new(false));
        BINDINGS.with_borrow_mut(|bindings| {
            *bindings = crate::hotkeys::bindings(Hotkey::RECORD, Hotkey::PHOTO, Hotkey::CHIMES);
        });
        HANDLER.with(|slot| {
            let seen = Rc::clone(&fired);
            *slot.borrow_mut() = Some(Box::new(move |event| {
                if matches!(event, Event::ToggleChimes) {
                    seen.set(true);
                }
            }));
        });
        dispatch_hotkey(3);
        assert!(fired.get(), "binding 3 toggles the chimes");
        HANDLER.with(|slot| *slot.borrow_mut() = None);
        BINDINGS.with_borrow_mut(Vec::clear);
    }

    #[test]
    fn the_menu_entry_emits_the_mute_toggle() {
        let fired = Rc::new(Cell::new(false));
        HANDLER.with(|slot| {
            let seen = Rc::clone(&fired);
            *slot.borrow_mut() = Some(Box::new(move |event| {
                if matches!(event, Event::ToggleChimes) {
                    seen.set(true);
                }
            }));
        });
        let _ = super::dispatch_command(super::ID_CHIMES, std::ptr::null_mut());
        assert!(fired.get(), "the menu entry toggles the chimes");
        HANDLER.with(|slot| *slot.borrow_mut() = None);
    }
}
