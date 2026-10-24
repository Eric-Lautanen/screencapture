//! Drop-based RAII guards around the Win32 acquire/release pairs the
//! interactive frontends use, so a panic landing between the two calls
//! can no longer leak the resource: screen DCs, low-level input hooks,
//! mouse capture, and the Escape watch every modal selector needs. Each
//! guard owns exactly one acquired handle, releases it in `Drop`, and
//! makes double-release impossible by construction. COM interfaces
//! (`dxgi::com::Com`) and the capture `Target` already live behind their
//! own `Drop` impls and stay there.

use std::cell::Cell;
use std::ffi::c_void;

use crate::pin_layout;
use crate::winapi::{GetDC, Hdc, Hinstance, Hwnd, ReleaseCapture, ReleaseDC, SetCapture};

/// Hook procedure signature (`HOOKPROC`, winuser.h).
pub(crate) type HookProc =
    unsafe extern "system" fn(code: i32, wparam: usize, lparam: isize) -> isize;

/// A screen DC borrowed via `GetDC`, released again on drop.
#[derive(Debug)]
pub(crate) struct ScreenDc {
    /// The window the DC belongs to (null = the desktop).
    owner: Hwnd,
    /// The live DC handle; `None` only after a failed construction.
    dc: Option<Hdc>,
}

impl ScreenDc {
    /// Borrow a DC for `owner` (or the whole desktop with null); `None`
    /// when Windows refused the borrow.
    #[must_use]
    pub(crate) fn borrow(owner: Hwnd) -> Option<Self> {
        // SAFETY: a plain DC borrow; released in `Drop`.
        let dc = unsafe { GetDC(owner) };
        (!dc.is_null()).then(|| Self {
            owner,
            dc: Some(dc),
        })
    }

    /// The desktop's screen DC; `None` when the desktop DC refused.
    #[must_use]
    pub(crate) fn desktop() -> Option<Self> {
        Self::borrow(std::ptr::null_mut())
    }

    /// The live DC for one draw call; the guard still owns the release.
    #[must_use]
    pub(crate) fn dc(&self) -> Hdc {
        // `Some(borrow)` construction guarantees `Some` here; a refused
        // borrow never becomes a guard at all.
        self.dc.expect("a constructed ScreenDc always holds its DC")
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        if let Some(dc) = self.dc.take() {
            // SAFETY: the DC this guard borrowed in `borrow`.
            unsafe { ReleaseDC(self.owner, dc) };
        }
    }
}

/// One low-level input hook, removed again on drop.
pub(crate) struct Hook {
    /// The live hook handle.
    handle: Hhook,
}

/// Hook handle (`HHOOK`, winuser.h).
type Hhook = *mut c_void;

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowsHookExW(id: i32, procedure: HookProc, instance: Hinstance, thread: u32) -> Hhook;
    fn UnhookWindowsHookEx(hook: Hhook) -> i32;
    fn CallNextHookEx(hook: Hhook, code: i32, wparam: usize, lparam: isize) -> isize;
}

/// Hand an event from a hook procedure to the next hook in the chain.
///
/// # Safety
/// Only valid inside a hook procedure; the parameters are the ones the
/// procedure received.
pub(crate) unsafe fn call_next_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: upheld by the caller contract above.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

impl Hook {
    /// Install a low-level hook of hook type `id`; `None` when Windows
    /// refuses (the caller answers its fallback).
    ///
    /// # Safety
    /// `procedure` must stay valid for the pump that keeps the hook fed,
    /// and `instance` must be this module's own handle.
    #[must_use]
    pub(crate) unsafe fn install(
        id: i32,
        procedure: HookProc,
        instance: Hinstance,
    ) -> Option<Self> {
        // SAFETY: upheld by the caller contract above.
        let handle = unsafe { SetWindowsHookExW(id, procedure, instance, 0) };
        (!handle.is_null()).then_some(Self { handle })
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        // SAFETY: the handle this guard installed in `install`.
        unsafe { UnhookWindowsHookEx(self.handle) };
    }
}

/// Mouse capture held for a window, released again on drop (the manual
/// `SetCapture`/`ReleaseCapture` pair, structured). `ReleaseCapture`
/// takes no argument, so the guard carries none.
pub(crate) struct MouseCapture;

impl MouseCapture {
    /// Capture mouse motion for `window` while the button is down;
    /// `None` when Windows refused (the drag then simply does not run).
    ///
    /// # Safety
    /// `window` must be owned by the calling thread.
    #[must_use]
    pub(crate) unsafe fn capture(window: Hwnd) -> Option<Self> {
        // SAFETY: upheld by the caller contract above.
        let previous = unsafe { SetCapture(window) };
        // Windows answers the previous capture window on success; a null
        // answer with `GetLastError` set means the capture failed. The
        // caller treats `None` as "no drag tracking this press".
        if previous.is_null() && std::io::Error::last_os_error().raw_os_error() != Some(0) {
            return None;
        }
        Some(Self)
    }
}

impl Drop for MouseCapture {
    fn drop(&mut self) {
        // SAFETY: the capture this guard took.
        unsafe { ReleaseCapture() };
    }
}

/// `WH_KEYBOARD_LL` (winuser.h): the low-level keyboard hook.
const KEYBOARD_HOOK: i32 = 13;
/// `LLKHF_UP` (winuser.h): set when a keyboard event reports key *up*.
const KEY_UP: u32 = 0x80;
/// `VK_ESCAPE` (winuser.h) as the `u32` the hook payload carries; the
/// shared [`crate::winapi::VK_ESCAPE`] is `usize`-typed for `WM_KEYDOWN`.
const ESCAPE_KEY: u32 = 0x1B;

/// `KBDLLHOOKSTRUCT` (winuser.h): the low-level keyboard event payload.
#[repr(C)]
#[derive(Clone, Copy)]
struct KbllHook {
    /// Virtual-key code.
    vk_code: u32,
    /// Hardware scan code.
    scan_code: u32,
    /// `LLKHF_*` flags; [`KEY_UP`] is the one this module reads.
    flags: u32,
    /// Event time (ms since boot).
    time: u32,
    /// Extra information attached by the hooking application.
    extra: usize,
}

// Hook payloads are read-only views of OS-owned memory; offsets exact.
pin_layout!(KbllHook, 24; vk_code = 0, scan_code = 4, flags = 8,
    time = 12, extra = 16);

thread_local! {
    /// What the live Escape watch answers a press with. At most one watch
    /// is armed at a time (a picker, or a live recording), so one action
    /// slot is exactly right.
    static ESCAPE_ACTION: Cell<Option<fn()>> = const { Cell::new(None) };
}

/// A global low-level keyboard hook that turns an Escape press into one
/// callback and swallows it, so a modal selector cancels — or a live
/// recording stops — even when its window never won the foreground.
///
/// A picker cannot rely on `WM_KEYDOWN`: Windows refuses
/// `SetForegroundWindow` for a process that is not the foreground one
/// (capturing from a terminal, an IDE, or a just-opened menu is enough),
/// and a window without the foreground never receives the key at all.
/// A low-level hook sits in the input path ahead of the focus rules, so
/// the press always arrives, is always swallowed (the application below
/// should not also close something), and never depends on a window.
pub(crate) struct EscapeWatch {
    /// The installed hook; the field exists only so `Drop` uninstalls it.
    #[expect(dead_code, reason = "the guard's whole job is to unhook on drop")]
    hook: Hook,
}

impl EscapeWatch {
    /// Install the watch; `None` when Windows refuses the hook, and the
    /// caller keeps its own fallback.
    ///
    /// # Safety
    /// `instance` must be this module's own handle, and the caller must
    /// pump messages on this thread for the session it guards.
    #[must_use]
    pub(crate) unsafe fn install(action: fn(), instance: Hinstance) -> Option<Self> {
        ESCAPE_ACTION.with(|slot| slot.set(Some(action)));
        // SAFETY: upheld by the caller contract above.
        let hook = unsafe { Hook::install(KEYBOARD_HOOK, on_escape, instance) };
        if hook.is_none() {
            // No hook means no delivery; an action left armed here would
            // let a later session run a stale cancel.
            ESCAPE_ACTION.with(|slot| slot.set(None));
        }
        hook.map(|hook| Self { hook })
    }
}

impl Drop for EscapeWatch {
    fn drop(&mut self) {
        ESCAPE_ACTION.with(|slot| slot.set(None));
    }
}

/// Whether one hook payload is Escape going *down*.
///
/// The virtual key rides in the payload, never in the hook's `wparam`
/// (that is `WM_KEYDOWN`/`WM_KEYUP`): comparing `wparam` to `VK_ESCAPE`
/// once left Escape silently unable to cancel anything.
fn escape_down(event: &KbllHook) -> bool {
    event.vk_code == ESCAPE_KEY && event.flags & KEY_UP == 0
}

/// Keyboard hook procedure: Escape cancels once (on key down) and both
/// halves of the press are swallowed; every other key passes through.
unsafe extern "system" fn on_escape(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        // SAFETY: the payload pointer is valid per the hook contract.
        let event = unsafe { &*(lparam as *const KbllHook) };
        if event.vk_code == ESCAPE_KEY {
            if escape_down(event) {
                ESCAPE_ACTION.with(|slot| {
                    if let Some(action) = slot.get() {
                        action();
                    }
                });
            }
            return 1;
        }
    }
    // SAFETY: default pass-through chain.
    unsafe { call_next_hook(code, wparam, lparam) }
}

/// `SPI_SETCURSORS` (winuser.h): reload the current cursor scheme.
const SPI_SET_CURSORS: u32 = 0x0057;

#[link(name = "user32")]
unsafe extern "system" {
    fn SystemParametersInfoW(action: u32, parameter: u32, data: *mut c_void, update: u32) -> i32;
}

/// Reload the user's cursor scheme. Called once at startup so a system
/// arrow left as a crosshair by an *older* build (which used
/// `SetSystemCursor`) is repaired even though the process was killed
/// before its drop could run.
pub(crate) fn restore_system_cursors() {
    // SAFETY: parameterless scheme reload.
    unsafe {
        let _ = SystemParametersInfoW(SPI_SET_CURSORS, 0, std::ptr::null_mut(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::{ESCAPE_KEY, KEY_UP, KbllHook, MouseCapture, ScreenDc, escape_down};
    use crate::winapi::GetModuleHandleW;

    /// A payload with every field set explicitly, so the test describes
    /// the hook event it claims to.
    fn payload(vk_code: u32, flags: u32) -> KbllHook {
        KbllHook {
            vk_code,
            scan_code: 1,
            flags,
            time: 0,
            extra: 0,
        }
    }

    #[test]
    fn escape_is_read_from_the_payload_and_only_on_key_down() {
        assert!(escape_down(&payload(ESCAPE_KEY, 0)));
        assert!(!escape_down(&payload(ESCAPE_KEY, KEY_UP)));
        // Any other key leaves the session alone, release or not.
        assert!(!escape_down(&payload(0x41, 0)));
        assert!(!escape_down(&payload(0x41, KEY_UP)));
    }

    #[test]
    fn screen_dc_rounds_trip_its_handle() {
        // The console host's window usually exists in a test run; a
        // headless runner answers null and the test steps aside.
        // SAFETY: parameterless question about this own process.
        let window = console_window();
        let Some(guard) = ScreenDc::borrow(window).or_else(ScreenDc::desktop) else {
            return; // no DC source in this environment
        };
        let handle = guard.dc();
        assert!(!handle.is_null());
        // A second borrow again answers a valid DC for the same window.
        if let Some(second) = ScreenDc::borrow(window) {
            assert!(!second.dc().is_null());
        }
        // `guard` now drops: released exactly once, by construction.
    }

    #[test]
    fn desktop_dc_guard_releases_on_drop() {
        let Some(guard) = ScreenDc::desktop() else {
            return;
        };
        assert!(!guard.dc().is_null());
        // The release happens in `Drop`; nothing else to assert here.
    }

    #[test]
    fn console_capture_takes_or_honestly_refuses() {
        // SAFETY: the test thread owns the console window (or there is
        // none, where the guard simply stays unbuilt).
        let guard = unsafe { MouseCapture::capture(console_window()) };
        // A scoped drop (never a manual call) releases the capture; the
        // invariant is no leaked capture on either answer.
        if guard.is_some() {
            drop(guard);
        }
    }

    #[test]
    fn module_handle_resolves() {
        // SAFETY: null resolves this executable.
        assert!(!unsafe { GetModuleHandleW(std::ptr::null()) }.is_null());
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        /// The console host window; null in a headless runner.
        fn GetConsoleWindow() -> *mut std::ffi::c_void;
    }

    /// SAFETY: parameterless question about this own process.
    fn console_window() -> *mut std::ffi::c_void {
        // SAFETY: parameterless.
        unsafe { GetConsoleWindow() }
    }
}
