//! Windows tray implementation: notification icon, message loop, and
//! command dispatch (single construct per run, all on the pump thread).

mod ffi;
mod ico;
mod menu;

use std::cell::RefCell;
use std::thread_local;

use self::ffi::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GETMESSAGE_QUIT, GetForegroundWindow, GetMessageW, Guid, Hicon, Hwnd, KillTimer, Lparam,
    Lresult, Msg, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NotifyIconDataW, Point,
    PostQuitMessage, RegisterClassExW, RegisterHotKey, SetTimer, Shell_NotifyIconW,
    TranslateMessage, UnregisterClassW, UnregisterHotKey, WM_COMMAND, WM_CREATE, WM_DESTROY,
    WM_HOTKEY, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WM_USER, WndClassExW, Wparam,
};
use super::{Error, Event};
use crate::hotkeys::Binding;
use crate::recording::Profile;

/// Tray notification callback message id (custom to this window).
const TRAY_CALLBACK: u32 = WM_USER + 1;

/// Tray icon identifier within this process.
const TRAY_ID: u32 = 1;

/// Class name for the hidden message window (leaked to the OS).
const CLASS_NAME: &str = "ScreencaptureTray";

/// Title for the hidden message window.
const WINDOW_NAME: &str = "Screencapture";

/// Tooltip shown next to the tray icon.
const TOOLTIP: &str = "screencapture";

/// Menu command ids; all sit in the low word of `WM_COMMAND`.
const ID_RECORD: Wparam = 1;
const ID_PHOTO: Wparam = 2;
const ID_AUTOSTART: Wparam = 3;
const ID_RECORD_REGION: Wparam = 4;
const ID_RECORD_WINDOW: Wparam = 5;
const ID_PROFILE_BASE: Wparam = 100;
/// Per-display entries live in the half-open range from
/// `ID_DISPLAY_BASE` up to `ID_EXIT`; the gap keeps future static ids out
/// of the dynamic range.
const ID_DISPLAY_BASE: Wparam = 120;
const ID_EXIT: Wparam = 200;

/// Frame-timer id for `WM_TIMER`.
const TIMER_ID: usize = 1;

thread_local! {
    static HANDLER: RefCell<Handler> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<Hwnd>> = const { RefCell::new(None) };
    static BINDINGS: RefCell<Vec<Binding>> = const { RefCell::new(Vec::new()) };
    /// The window focused just before the tray menu opened, so "record
    /// window" targets the application the user was working in.
    static PREVIOUS_FOREGROUND: RefCell<Option<usize>> = const { RefCell::new(None) };
}

type Handler = Option<Box<dyn FnMut(Event)>>;

/// UTF-16 string with a trailing nul for Win32 calls.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Leak a UTF-16 string so Win32 can hold it indefinitely (class name).
fn static_wide(text: &str) -> *const u16 {
    Box::leak(wide(text).into_boxed_slice()).as_ptr()
}

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

    let class_name = static_wide(CLASS_NAME);
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

/// Turn `Option<u32>` into timer state: `None` kills the frame timer.
pub(super) fn set_frame_timer(interval_ms: Option<u32>) {
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

/// The window handle stashed by [`stash_foreground`], as plain bits.
pub(super) fn previous_foreground() -> Option<usize> {
    PREVIOUS_FOREGROUND.with_borrow(|slot| *slot)
}

fn register_hotkeys(window: Hwnd) {
    for binding in BINDINGS.with_borrow(Vec::clone) {
        // SAFETY: registrations live on our own window and are removed on
        // `WM_DESTROY`.
        let registered =
            unsafe { RegisterHotKey(window, binding.id, binding.modifiers, binding.key) };
        if registered == 0 {
            eprintln!(
                "warn  hotkey id {} registration failed (already in use?)",
                binding.id
            );
        }
    }
}

fn unregister_hotkeys(window: Hwnd) {
    for binding in BINDINGS.with_borrow(Vec::clone) {
        // SAFETY: symmetric teardown of the registration made at startup.
        unsafe { UnregisterHotKey(window, binding.id) };
    }
}

fn module_handle() -> *mut std::ffi::c_void {
    // SAFETY: passing a null module name resolves the executable module.
    unsafe { self::ffi::GetModuleHandleW(std::ptr::null()) }
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
            // Remember what the user had focused before our menu took
            // focus; "record window" needs that application.
            stash_foreground();
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
        emit(Event::Tick);
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
        ID_RECORD_REGION => emit(Event::RecordRegion),
        ID_RECORD_WINDOW => emit(Event::RecordWindow),
        ID_PHOTO => emit(Event::Photo),
        ID_AUTOSTART => emit(Event::ToggleAutostart),
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

fn dispatch_hotkey(id: Wparam) {
    let role = BINDINGS.with_borrow(|slot| {
        slot.iter()
            .find(|binding| binding.id == i32::try_from(id).unwrap_or(0))
            .map(|binding| binding.role)
    });
    if let Some(role) = role {
        emit(match role {
            crate::hotkeys::Role::Record => Event::Record,
            crate::hotkeys::Role::Photo => Event::Photo,
        });
    }
}

fn emit(event: Event) {
    HANDLER.with(|slot| {
        if let Some(handler) = slot.borrow_mut().as_mut() {
            handler(event);
        }
    });
}

/// Note the currently focused window into `PREVIOUS_FOREGROUND`, exposed
/// through [`super::previous_foreground`] for window-mode captures.
fn stash_foreground() {
    // SAFETY: reads the foreground handle only; nothing is stored.
    let window = unsafe { GetForegroundWindow() };
    let value = if window.is_null() {
        None
    } else {
        Some(window as usize)
    };
    PREVIOUS_FOREGROUND.with_borrow_mut(|slot| *slot = value);
}

fn add_icon(window: Hwnd) -> Lresult {
    // SAFETY: builds the authored icon on the tray's own thread.
    let icon = unsafe { ico::app_or_fallback() };
    let mut data = icon_data(window, icon);
    set_tip(&mut data.tip, TOOLTIP);
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
        guid: Guid {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        },
    }
}

fn set_tip(tip: &mut [u16; 128], text: &str) {
    for (slot, character) in tip.iter_mut().zip(wide(text)) {
        *slot = character;
    }
}

fn remove_icon(window: Hwnd) {
    let mut data = icon_data(window, std::ptr::null_mut());
    // SAFETY: `data` is a local, fully initialized struct bound to `window`.
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &raw mut data) };
}
