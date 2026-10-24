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
//!
//! A hook callback that runs long is skipped rather than queued, so the
//! session never assumes it saw every event: a release is answered from
//! the point it arrives at (never the highlight it left behind), and a
//! press always starts a fresh gesture, so a swallowed release cannot stop
//! a later click from resolving. The gesture is tracked from these events
//! alone: the picker consumes the press, so the system's own button state
//! never sees it (which is why `GetAsyncKeyState` cannot answer for it).

use crate::pin_layout;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::thread_local;
use std::time::Duration;

use crate::capture::Region;
use crate::guard::{EscapeWatch, Hook};
use crate::overlay::{Border, Crosshair, Style};
use crate::screen::win::window_bounds;
use crate::winapi::{
    GA_ROOT, GetAncestor, GetCursorPos, Point, PostQuitMessage, PumpEnd, Rect,
    SELECTION_IDLE_LIMIT, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN,
    WindowFromPoint, module_handle, pump_until,
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
    /// Set while a pick owns the hooks and [`TRACK`].
    static LIVE: Cell<bool> = const { Cell::new(false) };
}

/// The live-session claim; released on drop, so every way out of [`pick`]
/// frees the session again.
struct Live;

impl Live {
    /// Claim the pick session; `None` when one is already live here.
    fn enter() -> Option<Self> {
        (!LIVE.replace(true)).then_some(Self)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        LIVE.set(false);
    }
}

/// Highlight the hovered window and answer the picked target.
///
/// # Errors
/// Windows refused the overlay's hooks; the caller answers its
/// cursor-under fallback instead.
pub fn pick() -> std::io::Result<Option<Pick>> {
    pick_within(SELECTION_IDLE_LIMIT)
}

/// [`pick`] with an explicit idle budget.
///
/// The suite drives real synthetic input through this session, and a short
/// budget is what turns a lost injection into a failing test instead of a
/// two-minute stall: two minutes is the pause a user may legitimately take.
fn pick_within(budget: Duration) -> std::io::Result<Option<Pick>> {
    // The tray pumps messages from inside its own handler, so a menu or a
    // dialog still on the stack can dispatch a second capture request
    // into a re-entrant pick. That one answers "no selection": letting it
    // run would reset `TRACK`, uninstall the live session's hooks and drop
    // its outline halfway through the gesture.
    let Some(_live) = Live::enter() else {
        return Ok(None);
    };
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
        let aimed_at = aimed(track, hover_at(cursor).as_ref());
        apply(track, aimed_at);
        track.cursor = Crosshair::show(cursor);
        Ok(())
    })?;
    pump(budget);
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
fn pump(budget: Duration) {
    let end = pump_until(budget, || TRACK.with_borrow(|track| track.done));
    if end == PumpEnd::Idle {
        abandon();
        crate::log::note("warn", "selection cancelled after sitting idle");
    }
}

/// The root window under the pointer and its frame as a capture region.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Hover {
    /// The root window, as plain pointer bits (the pick's answer).
    handle: usize,
    /// The visible frame: what the outline hugs and a capture saves.
    region: Region,
}

/// One pointer event, already resolved against the desktop.
///
/// Resolution is the only part of a gesture that needs Windows, so
/// everything below it is a plain state machine the tests drive directly.
#[derive(Clone, Copy)]
enum Input {
    /// The left button went down at `point`.
    Press { point: Point, under: Option<Hover> },
    /// The pointer moved to `point`.
    Move { point: Point, under: Option<Hover> },
    /// The left button came up at `point`, over `under`.
    Release { point: Point, under: Option<Hover> },
    /// Escape, right click, or the idle limit: answer nothing.
    Cancel,
}

/// What the window layer has to do after [`step`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    /// The outline is already right.
    Keep,
    /// Drop the outline: nothing is pointed at.
    Clear,
    /// Outline `region` with the picker's dotted frame.
    Outline(Region),
}

/// Advance the gesture by one event.
///
/// The whole pick lives here rather than in the hook procedure because
/// event order cannot be trusted: Windows skips a hook callback that runs
/// past its patience instead of queueing it, so the session must have a
/// defined answer for every subsequence of press, move and release. Keeping
/// that answer testable without hooks is what stops the rules from drifting.
fn step(track: &mut Track, input: Input) -> Action {
    match input {
        Input::Press { point, under } => press(track, point, under.as_ref()),
        Input::Move { point, under } => moved(track, point, under.as_ref()),
        Input::Release { point, under } => released(track, point, under.as_ref()),
        Input::Cancel => {
            track.done = true;
            track.dragging = false;
            track.origin = None;
            track.marquee = None;
            track.picked = None;
            Action::Clear
        }
    }
}

/// Aim the outline at the window under the pointer, or at nothing.
///
/// The outline only moves when the pointed-at window changes: repainting a
/// full-window frame on every mouse move is work the hook callback cannot
/// afford, and a slow callback is a skipped event.
fn aimed(track: &mut Track, under: Option<&Hover>) -> Action {
    if let Some(hover) = under {
        if track.handle == Some(hover.handle) {
            return Action::Keep;
        }
        track.handle = Some(hover.handle);
        return Action::Outline(hover.region);
    }
    if track.handle.is_none() {
        return Action::Keep;
    }
    track.handle = None;
    Action::Clear
}

/// Start a press at `point`, whatever the previous gesture was.
///
/// A press that arrives while a marquee is still live means the release
/// that should have ended it never arrived: taking the press over is what
/// keeps a skipped event from wedging the session, where the old marquee
/// followed the pointer and no click could resolve it.
fn press(track: &mut Track, point: Point, under: Option<&Hover>) -> Action {
    if track.dragging {
        track.dragging = false;
        track.marquee = None;
        // The dropped marquee replaced the hovered highlight, so the next
        // aim has to place the outline again.
        track.handle = None;
    }
    track.origin = Some(point);
    aimed(track, under)
}

/// Track a move: a live marquee follows the pointer, a press past the
/// threshold starts one, and a press that stays put only re-aims the
/// outline at the window under the pointer.
fn moved(track: &mut Track, point: Point, under: Option<&Hover>) -> Action {
    let Some(origin) = track.origin else {
        return aimed(track, under);
    };
    if track.dragging || beyond_threshold(origin, point) {
        track.dragging = true;
        let (_, region) = drag_geometry(origin, point);
        track.marquee = Some(region);
        return Action::Outline(region);
    }
    aimed(track, under)
}

/// The button came up at `point` over `under`.
///
/// A click answers the window under the release, never the highlight the
/// session happened to be showing: when the system skips the press, that
/// stale highlight is what a pick begun over the tray used to record. A
/// marquee answers its own rectangle instead, whatever window the pointer
/// finished over.
///
/// The release carries the final pointer position because the moves on
/// the way there are the events a busy hook callback loses first: reading
/// only the last move seen ended a fast drag well short of where the
/// pointer stopped, and saved a rectangle smaller than the one dragged.
fn released(track: &mut Track, point: Point, under: Option<&Hover>) -> Action {
    if track.dragging {
        if let Some(origin) = track.origin {
            let (_, region) = drag_geometry(origin, point);
            track.marquee = Some(region);
        }
        confirm_drag(track);
    } else {
        track.handle = under.map(|hover| hover.handle);
        confirm_click(track);
    }
    Action::Clear
}

/// Carry out what [`step`] asked for: the only part that touches windows.
fn apply(track: &mut Track, action: Action) {
    match action {
        Action::Keep => {}
        Action::Clear => undraw(track),
        Action::Outline(region) => place_highlight(track, region),
    }
}

/// Resolve `point` against the desktop.
fn hover_at(point: Point) -> Option<Hover> {
    // SAFETY: point lookup and handle walk; the returned windows are
    // inspected only.
    let hovered = unsafe { WindowFromPoint(point) };
    if hovered.is_null() {
        return None;
    }
    // SAFETY: `hovered` came from the lookup above.
    let root = unsafe { GetAncestor(hovered, GA_ROOT) };
    if root.is_null() {
        return None;
    }
    Some(Hover {
        handle: root as usize,
        region: region_from_rect(window_bounds(root)?),
    })
}

/// Low-level mouse hook: resolve each event against the desktop, let the
/// gesture state machine answer it, and draw what it asked for. Presses and
/// releases are consumed; motion passes through so the cursor keeps moving.
unsafe extern "system" fn on_mouse(code: i32, wparam: usize, lparam: isize) -> isize {
    let mut consume = false;
    if code >= 0 {
        // SAFETY: the payload pointer is valid per the hook contract.
        let event = unsafe { &*(lparam as *const MsllHook) };
        let message = u32::try_from(wparam).unwrap_or(0);
        TRACK.with_borrow_mut(|track| {
            consume = consumed(message);
            // The marker stays glued to the pointer for the whole session,
            // including the event that ends it.
            if let Some(cursor) = &track.cursor {
                cursor.place(event.pt);
            }
            if track.done {
                return;
            }
            // A live marquee answers the pointer alone, so the window
            // lookup is skipped wherever it cannot change the outcome.
            let input = match message {
                WM_LBUTTONDOWN => Input::Press {
                    point: event.pt,
                    under: hover_at(event.pt),
                },
                WM_LBUTTONUP => Input::Release {
                    point: event.pt,
                    under: if track.dragging {
                        None
                    } else {
                        hover_at(event.pt)
                    },
                },
                WM_MOUSEMOVE => Input::Move {
                    point: event.pt,
                    under: if track.dragging {
                        None
                    } else {
                        hover_at(event.pt)
                    },
                },
                WM_RBUTTONDOWN => Input::Cancel,
                _ => return,
            };
            let action = step(track, input);
            apply(track, action);
            if track.done {
                finish();
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
fn consumed(message: u32) -> bool {
    matches!(message, WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN)
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
        let action = step(track, Input::Cancel);
        apply(track, action);
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

/// Move the outline to `region`, creating it on first use. A refused
/// outline is cosmetic only: the pick itself still answers.
fn place_highlight(track: &mut Track, region: Region) {
    if let Some(border) = &track.highlight {
        border.place(region);
    } else if let Ok(border) = Border::show(region, Style::Selection) {
        track.highlight = Some(border);
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

#[cfg(test)]
mod tests;
