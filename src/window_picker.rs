//! Click-to-pick capture session behind the tray's interactive capture
//! (Record and Photo when `[capture] mode` requests a target): the
//! crosshair-highlighted top-level window under the cursor (DWM extended
//! frame bounds) follows the mouse anywhere on the desktop, and one
//! press answers two ways — release without meaningful movement records
//! the highlighted window; press-and-drag past the threshold instead
//! draws the same dotted outline around that desktop rectangle. Escape
//! or right click cancels either way. Input arrives through low-level
//! mouse and keyboard hooks (see [`EscapeWatch`]) and the outline is a
//! click-through topmost window painted with the picker's dotted style.

use crate::pin_layout;
use std::cell::RefCell;
use std::ffi::c_void;
use std::thread_local;

use crate::capture::Region;
use crate::guard::{EscapeWatch, Hook};
use crate::overlay::{Border, Crosshair, Style};
use crate::screen::win::window_bounds;
use crate::winapi::{
    GA_ROOT, GetAncestor, GetCursorPos, GetModuleHandleW, Hinstance, Point, PostQuitMessage,
    PumpEnd, Rect, SELECTION_IDLE_LIMIT, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_RBUTTONDOWN, WindowFromPoint, pump_until,
};

/// `WH_MOUSE_LL` (winuser.h): the low-level mouse hook.
const MOUSE_HOOK: i32 = 14;
/// Chebyshev distance (px) separating a click from a drag.
const DRAG_THRESHOLD_PX: i32 = 8;

/// Hook handle (`HHOOK`, winuser.h).
type Hhook = *mut c_void;

#[link(name = "user32")]
unsafe extern "system" {
    fn CallNextHookEx(hook: Hhook, code: i32, wparam: usize, lparam: isize) -> isize;
}

/// What one session answers: the highlighted window's handle, or the
/// rectangle the user dragged out (physical pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The confirmed top-level window.
    Window(usize),
    /// The dragged desktop rectangle.
    Region(Region),
}

/// `MSLLHOOKSTRUCT` (winuser.h): the low-level mouse event payload.
#[repr(C)]
struct MsllHook {
    pt: Point,
    mouse_data: u32,
    flags: u32,
    time: u32,
    extra: usize,
}

pin_layout!(MsllHook, 32; pt = 0, mouse_data = 8, flags = 12, time = 16, extra = 24);

/// Live picker state on the pump thread.
#[derive(Default)]
struct Track {
    /// The root window under the cursor, as plain pointer bits.
    handle: Option<usize>,
    /// The live outline over the hovered window or drag marquee.
    highlight: Option<Border>,
    /// The capture rectangle of the marquee currently outlined.
    marquee: Option<Region>,
    /// Set once the press moves beyond [`DRAG_THRESHOLD_PX`].
    dragging: bool,
    /// Where the left button went down; `None` between presses.
    origin: Option<Point>,
    /// Set when the pick resolves (confirm or cancel) and the pump ends.
    done: bool,
    /// The confirmed answer: window or dragged region, `None` on cancel.
    picked: Option<Pick>,
    /// Live hook guard; `Drop` uninstalls on teardown.
    mouse: Option<Hook>,
    /// The Escape watch; `Drop` unhooks it with the mouse hook above.
    escape: Option<EscapeWatch>,
    /// The drawn crosshair marker shown for this session.
    cursor: Option<Crosshair>,
}

thread_local! {
    static TRACK: RefCell<Track> = RefCell::new(Track::default());
}

/// Highlight the hovered window and answer the picked target.
///
/// # Errors
/// Windows refused the overlay's hooks; the caller answers its
/// cursor-under fallback instead.
pub fn pick() -> std::io::Result<Option<Pick>> {
    TRACK.with_borrow_mut(|track| *track = Track::default());
    let instance = module_handle();
    TRACK.with_borrow_mut(|track| {
        // SAFETY: global hooks installed for this thread's pump; the
        // guards uninstall in their `Drop` on teardown or the next pick.
        track.mouse = unsafe { Hook::install(MOUSE_HOOK, on_mouse, instance) };
        track.escape = unsafe { EscapeWatch::install(cancel, instance) };
        if track.mouse.is_none() || track.escape.is_none() {
            teardown(track);
            return Err(std::io::Error::other("the input hooks did not install"));
        }
        let mut cursor = Point::default();
        // SAFETY: writes only the local `cursor` point.
        unsafe { GetCursorPos(&raw mut cursor) };
        follow(track, cursor);
        track.cursor = Crosshair::show(cursor);
        Ok(())
    })?;
    pump();
    let answer = TRACK.with_borrow_mut(|track| {
        teardown(track);
        track.picked
    });
    Ok(answer)
}

/// Clear the hook guards and drop the outline before the caller captures.
fn teardown(track: &mut Track) {
    track.mouse = None;
    track.escape = None;
    track.cursor = None;
    track.highlight = None;
}

/// The message pump that keeps the hooks fed until the pick resolves.
///
/// A session that receives nothing — input aimed at a higher-integrity
/// window never reaches a low-level hook — ends at the idle limit instead
/// of hanging the tray with no way to cancel.
fn pump() {
    let end = pump_until(SELECTION_IDLE_LIMIT, || {
        TRACK.with_borrow(|track| track.done)
    });
    if end == PumpEnd::Idle {
        abandon();
        crate::log::note("warn", "selection cancelled after sitting idle");
    }
}

/// Low-level mouse hook: motion refreshes the window highlight or marquee,
/// a release confirms the target, right click cancels. Presses and releases
/// are consumed; motion passes through so the cursor keeps moving.
unsafe extern "system" fn on_mouse(code: i32, wparam: usize, lparam: isize) -> isize {
    let mut consume = false;
    if code >= 0 {
        // SAFETY: the payload pointer is valid per the hook contract.
        let event = unsafe { &*(lparam as *const MsllHook) };
        TRACK.with_borrow_mut(|track| {
            consume = consumed(wparam);
            if track.done {
                return;
            }
            if wparam == WM_LBUTTONDOWN as usize && !track.dragging {
                follow(track, event.pt);
                track.origin = Some(event.pt);
            } else if wparam == WM_LBUTTONUP as usize && !track.dragging {
                confirm_click(track);
                finish();
            } else if wparam == WM_LBUTTONUP as usize && track.dragging {
                confirm_drag(track);
                finish();
            } else if wparam == WM_MOUSEMOVE as usize {
                if let Some(origin) = track.origin {
                    if track.dragging {
                        draw_marquee(track, origin, event.pt);
                    } else if beyond_threshold(origin, event.pt) {
                        undraw(track);
                        track.dragging = true;
                        draw_marquee(track, origin, event.pt);
                    } else {
                        follow(track, event.pt);
                    }
                } else {
                    follow(track, event.pt);
                }
            } else if wparam == WM_RBUTTONDOWN as usize {
                track.done = true;
                undraw(track);
                track.origin = None;
                track.picked = None;
                finish();
            }
            if let Some(cursor) = &track.cursor {
                cursor.place(event.pt);
            }
        });
    }
    if consume {
        return 1;
    }
    // SAFETY: default pass-through chain.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Presses and releases are swallowed; motion passes through so the OS
/// cursor moves naturally.
fn consumed(message: usize) -> bool {
    message == WM_LBUTTONDOWN as usize
        || message == WM_LBUTTONUP as usize
        || message == WM_RBUTTONDOWN as usize
}

/// Confirm a plain click: the highlighted window is the answer.
fn confirm_click(track: &mut Track) {
    let window = track.handle;
    track.done = true;
    undraw(track);
    track.origin = None;
    track.picked = window.map(Pick::Window);
}

/// Confirm a drag: read the marquee *before* `undraw` clears it.
fn confirm_drag(track: &mut Track) {
    let dragged = track.marquee;
    track.done = true;
    undraw(track);
    track.origin = None;
    track.picked = dragged.map(Pick::Region);
}

/// End the session and quit the pump.
fn finish() {
    // SAFETY: the pump loop owns the quit sentinel.
    unsafe { PostQuitMessage(0) }
}

/// Cancel the pick, drop the outline, and end the pump.
fn cancel() {
    abandon();
    finish();
}

/// Abandon a session that received no input: the same state as a cancel,
/// but without the quit message. The pump has already returned here, and
/// a stray `WM_QUIT` would end the tray loop behind it.
fn abandon() {
    TRACK.with_borrow_mut(|track| {
        if track.done {
            return;
        }
        track.done = true;
        undraw(track);
        track.origin = None;
        track.picked = None;
    });
}

/// Chebyshev distance between two points in pixels.
fn chebyshev(a: Point, b: Point) -> i32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx.abs().max(dy.abs())
}

/// True when `cursor` has left [`DRAG_THRESHOLD_PX`] of `origin`.
fn beyond_threshold(origin: Point, cursor: Point) -> bool {
    chebyshev(origin, cursor) > DRAG_THRESHOLD_PX
}

/// The XOR-marquee rect and matching capture region for a drag from `a`
/// to `b` (normalized; at least one pixel on each axis).
fn drag_geometry(a: Point, b: Point) -> (Rect, Region) {
    let (left, right) = (a.x.min(b.x), a.x.max(b.x));
    let (top, bottom) = (a.y.min(b.y), a.y.max(b.y));
    let rect = Rect {
        left,
        top,
        right,
        bottom,
    };
    let region = Region {
        x: left,
        y: top,
        width: u32::try_from((right - left).max(1)).unwrap_or(1),
        height: u32::try_from((bottom - top).max(1)).unwrap_or(1),
    };
    (rect, region)
}

/// Move the outline to the marquee for origin/current and remember the
/// capture region it answers.
fn draw_marquee(track: &mut Track, origin: Point, current: Point) {
    let (_, region) = drag_geometry(origin, current);
    place_highlight(track, region);
    track.marquee = Some(region);
}

/// Move the outline to `region`, creating it on first use. A refused
/// outline is cosmetic only: the pick itself still answers.
fn place_highlight(track: &mut Track, region: Region) {
    if let Some(border) = &track.highlight {
        border.place(region);
    } else if let Ok(border) = Border::show(region, Style::Selection) {
        track.highlight = Some(border);
    }
}

/// The hovered root window at `cursor`: its handle and frame bounds (DWM
/// bordered, `GetWindowRect` when decoration introspection refuses).
fn follow(track: &mut Track, cursor: Point) {
    // SAFETY: point lookups and handle walks; the returned windows are
    // inspected only.
    let hovered = unsafe { WindowFromPoint(cursor) };
    let handle = if hovered.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { GetAncestor(hovered, GA_ROOT) }
    };
    if handle.is_null() {
        undraw(track);
        track.handle = None;
        return;
    }
    if track.handle == Some(handle as usize) {
        return;
    }
    let bounds = window_bounds(handle);
    undraw(track);
    track.handle = Some(handle as usize);
    if let Some(bounds) = bounds {
        place_highlight(track, region_from_rect(bounds));
    }
}

/// Drop the live outline (window highlight or drag marquee) and clear
/// both markers; the window is destroyed before the pick answers, so no
/// outline can leak into the capture.
fn undraw(track: &mut Track) {
    track.highlight = None;
    track.marquee = None;
}

/// A window's frame rect as a capture region.
fn region_from_rect(rect: Rect) -> Region {
    Region {
        x: rect.left,
        y: rect.top,
        width: u32::try_from((rect.right - rect.left).max(1)).unwrap_or(1),
        height: u32::try_from((rect.bottom - rect.top).max(1)).unwrap_or(1),
    }
}

/// This module's instance handle for the hook registrations.
fn module_handle() -> Hinstance {
    // SAFETY: null resolves the executable module.
    unsafe { GetModuleHandleW(std::ptr::null()) }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;

    use super::{
        DRAG_THRESHOLD_PX, Pick, TRACK, Track, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WM_RBUTTONDOWN, abandon, beyond_threshold, confirm_click, confirm_drag, consumed,
        drag_geometry,
    };
    use crate::screen::win::window_bounds;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        /// The console host window this test process usually owns; a
        /// headless runner answers null and the test steps aside.
        fn GetConsoleWindow() -> *mut c_void;
    }

    #[test]
    fn motion_passes_through_to_the_cursor() {
        // The pointer must keep moving while a pick is live; a consumed
        // move is a move the system never applies to the cursor.
        assert!(!consumed(WM_MOUSEMOVE as usize));
        assert!(consumed(WM_LBUTTONDOWN as usize));
        assert!(consumed(WM_LBUTTONUP as usize));
        assert!(consumed(WM_RBUTTONDOWN as usize));
    }

    /// The idle guard clears the answer instead of leaving a stale target
    /// behind, and posts no quit message (the pump is already gone).
    #[test]
    fn an_idle_session_abandons_instead_of_answering() {
        TRACK.with_borrow_mut(|track| {
            *track = Track::default();
            track.handle = Some(7);
            track.origin = Some(super::Point { x: 10, y: 10 });
            track.dragging = true;
        });
        abandon();
        TRACK.with_borrow(|track| {
            assert!(track.done, "the session ends");
            assert!(track.picked.is_none(), "nothing is answered");
            assert!(track.origin.is_none(), "the press is cleared");
        });
    }

    #[test]
    fn a_real_window_answers_bounds() {
        // SAFETY: parameterless question about our own process.
        let handle = unsafe { GetConsoleWindow() };
        if handle.is_null() {
            return;
        }
        let bounds = window_bounds(handle).expect("console window bounds");
        assert!(bounds.right > bounds.left && bounds.bottom > bounds.top);
    }

    #[test]
    fn fresh_tracks_answer_nothing() {
        let track = Track::default();
        assert!(
            track.handle.is_none()
                && track.picked.is_none()
                && track.highlight.is_none()
                && !track.dragging
                && track.origin.is_none()
        );
    }

    #[test]
    fn click_and_drag_differ_by_the_threshold() {
        let origin = super::Point { x: 500, y: 500 };
        let inside = super::Point {
            x: 500 + DRAG_THRESHOLD_PX,
            y: 500,
        };
        let outside = super::Point {
            x: 500 + DRAG_THRESHOLD_PX + 1,
            y: 500,
        };
        assert!(!beyond_threshold(origin, inside));
        assert!(beyond_threshold(origin, outside));
    }

    #[test]
    fn dragged_geometry_normalizes_and_never_zeroes() {
        // Drag up-left from bottom-right: rect still reads top-left.
        let (rect, region) = drag_geometry(
            super::Point { x: 900, y: 600 },
            super::Point { x: 300, y: 200 },
        );
        assert_eq!(rect.left, 300);
        assert_eq!(rect.top, 200);
        assert_eq!(region.x, 300);
        assert_eq!(region.y, 200);
        assert_eq!(region.width, 600);
        assert_eq!(region.height, 400);
        // One-pixel guarantee either way.
        let (_, tiny) = drag_geometry(super::Point { x: 7, y: 7 }, super::Point { x: 7, y: 7 });
        assert_eq!((tiny.width, tiny.height), (1, 1));
    }

    #[test]
    fn drag_release_keeps_the_region_answer() {
        let region = super::Region {
            x: 10,
            y: 20,
            width: 300,
            height: 200,
        };
        let mut track = Track {
            marquee: Some(region),
            dragging: true,
            origin: Some(super::Point { x: 0, y: 0 }),
            ..Track::default()
        };
        confirm_drag(&mut track);
        assert_eq!(track.picked, Some(Pick::Region(region)));
        assert!(track.done);
        assert!(track.marquee.is_none());
    }

    #[test]
    fn click_release_keeps_the_window_answer() {
        let mut track = Track {
            handle: Some(42),
            ..Track::default()
        };
        confirm_click(&mut track);
        assert_eq!(track.picked, Some(Pick::Window(42)));
        assert!(track.done);
    }

    #[test]
    fn pick_variants_stay_exhaustive() {
        // Sanity on the enum shape the app layer matches.
        assert_eq!(Pick::Window(1), Pick::Window(1));
        assert_ne!(
            Pick::Window(1),
            Pick::Region(super::Region {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            })
        );
    }
}
