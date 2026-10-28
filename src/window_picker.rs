//! Click-to-pick capture session behind the tray's interactive capture
//! (Record and Photo when `[capture] mode` requests a target): the
//! crosshair-highlighted top-level window under the cursor (DWM extended
//! frame bounds) follows the mouse anywhere on the desktop, and one
//! press answers two ways — release without meaningful movement records
//! the highlighted window; press-and-drag past the threshold instead
//! draws the same dotted outline around that desktop rectangle. Escape
//! or right click cancels either way.
//!
//! The pointer belongs to a fullscreen window of ours for the session (see
//! [`grabber`]). Because the window under the pointer is the window whose
//! cursor the desktop draws, that window's class cursor makes the crosshair
//! the single pointer on every surface — a text field's I-beam included —
//! with the window manager moving it. Its messages are also where the gesture
//! comes from, so no pointer event is skipped for running late; the outline is
//! a separate click-through window over it, and Escape still arrives through
//! its own low-level watch (see [`EscapeWatch`]).
//!
//! Because a message can still be lost (a modal loop elsewhere, or capture
//! taken by another process), the session never assumes it saw every event: a
//! release is answered from the point it arrives at (never the highlight it
//! left behind), and a press always starts a fresh gesture, so a swallowed
//! release cannot stop a later click from resolving. The gesture is tracked
//! from these events alone: the picker consumes the press, so the system's own
//! button state never sees it (which is why `GetAsyncKeyState` cannot answer
//! for it).

use std::cell::{Cell, RefCell};
use std::thread_local;
use std::time::Duration;

use crate::capture::Region;
use crate::guard::{EscapeWatch, MouseCapture};
use crate::overlay::{Border, Style};
use crate::screen::win::window_bounds;
use crate::winapi::{
    GA_ROOT, GetAncestor, GetAsyncKeyState, GetCursorPos, Point, PumpEnd, Rect,
    SELECTION_IDLE_LIMIT, VK_LBUTTON, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN,
    module_handle, pump_until,
};

use grabber::Grabber;

mod grabber;

/// Chebyshev distance (px) separating a click from a drag.
const DRAG_THRESHOLD_PX: i32 = 8;

/// What one session answers: the highlighted window's handle, or the
/// rectangle the user dragged out (physical pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The confirmed top-level window.
    Window(usize),
    /// The dragged desktop rectangle.
    Region(Region),
}

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
    /// The fullscreen window holding the pointer and feeding this session;
    /// `Drop` destroys it, returning the desktop.
    grab: Option<Grabber>,
    /// Capture held from a press to its release, so a drag still ends when
    /// the pointer crosses a window above the overlay.
    capture: Option<MouseCapture>,
    /// Set when the session took over a press that was already down: the
    /// system replays that press to the new capture window, and it is not a
    /// gesture this session began.
    inherited: bool,
    /// The Escape watch; `Drop` unhooks it.
    escape: Option<EscapeWatch>,
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
        track.grab = Some(Grabber::show()?);
        // SAFETY: a global watch installed for this thread's pump; the guard
        // uninstalls it in its `Drop` on teardown or the next pick.
        track.escape = unsafe { EscapeWatch::install(cancel, instance) };
        if track.escape.is_none() {
            teardown(track);
            return Err(std::io::Error::other("the Escape watch did not install"));
        }
        let mut cursor = Point::default();
        // SAFETY: writes only the local `cursor` point.
        unsafe { GetCursorPos(&raw mut cursor) };
        // A pick can start with the button already down: a hotkey pressed in
        // the middle of a drag. Windows keeps that gesture with the window
        // that received the press, so without capture the release would go
        // there as well and the session would hear nothing until the idle
        // limit. The state machine already answers a release with no press.
        if button_down() {
            track.inherited = capture(track);
        }
        let under = track.grab.as_ref().and_then(|grab| hover_at(grab, cursor));
        let aimed_at = aimed(track, under.as_ref());
        apply(track, aimed_at);
        Ok(())
    })?;
    pump(budget);
    let answer = TRACK.with_borrow_mut(|track| {
        teardown(track);
        track.picked
    });
    Ok(answer)
}

/// Clear the guards, the pointer surface and the outline before the caller
/// captures: nothing of the pick may be on screen when the shutter opens.
fn teardown(track: &mut Track) {
    track.capture = None;
    track.escape = None;
    track.grab = None;
    track.highlight = None;
}

/// The message pump that keeps the hooks fed until the pick resolves.
///
/// A session that receives nothing — input aimed at a higher-integrity
/// window never reaches a low-level hook — ends at the idle limit instead
/// of hanging the tray with no way to cancel.
fn pump(budget: Duration) {
    let end = pump_until(budget, || {
        // Something can land over the surface while the session runs, and the
        // session then hears nothing at all: no move, no click, and only the
        // idle limit ends it. Claiming the top of the band back is what keeps
        // the pointer ours for as long as the pick is live.
        raise_surface();
        TRACK.with_borrow(|track| track.done)
    });
    if end == PumpEnd::Idle {
        abandon();
        crate::log::note("warn", "selection cancelled after sitting idle");
    }
}

/// Put the pick's surface back over whatever has landed on top of it.
fn raise_surface() {
    TRACK.with_borrow(|track| {
        if let Some(grab) = &track.grab {
            grab.raise_if_covered();
        }
    });
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

/// Resolve `point` against the desktop, past the pick's own overlay.
fn hover_at(grab: &Grabber, point: Point) -> Option<Hover> {
    let hovered = grab.beneath(point);
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

/// One pointer message from the grabber: resolve it against the desktop, let
/// the gesture state machine answer it, and draw what it asked for.
///
/// `point` is where the event happened, taken from the message, so a queued
/// release answers where the button came up rather than where the pointer has
/// moved on to since.
fn pointer(message: u32, point: Point) {
    TRACK.with_borrow_mut(|track| {
        if track.done {
            return;
        }
        let input = match message {
            // A press replayed from a gesture that was already running when
            // the session started is not this session's: it only aims the
            // outline, and the release still answers the window it came up
            // over.
            WM_LBUTTONDOWN => {
                let inherited = std::mem::take(&mut track.inherited);
                if inherited {
                    Input::Move {
                        point,
                        under: resolve(track, point),
                    }
                } else {
                    capture(track);
                    Input::Press {
                        point,
                        under: resolve(track, point),
                    }
                }
            }
            WM_LBUTTONUP => {
                track.capture = None;
                Input::Release {
                    point,
                    under: resolve(track, point),
                }
            }
            WM_MOUSEMOVE => Input::Move {
                point,
                under: resolve(track, point),
            },
            WM_RBUTTONDOWN => Input::Cancel,
            _ => return,
        };
        let action = step(track, input);
        apply(track, action);
        if track.done {
            track.capture = None;
        }
    });
}

/// Whether the left button is down right now: `GetAsyncKeyState`'s high bit
/// is set while it is, and the pick has no earlier press to rely on.
fn button_down() -> bool {
    // SAFETY: parameterless query about the desktop's input state.
    unsafe { GetAsyncKeyState(VK_LBUTTON) < 0 }
}

/// The window under `point` as this pick sees it: a live marquee answers the
/// pointer alone, so the lookup is skipped wherever it cannot change the
/// outcome.
fn resolve(track: &Track, point: Point) -> Option<Hover> {
    if track.dragging {
        return None;
    }
    track.grab.as_ref().and_then(|grab| hover_at(grab, point))
}

/// Hold capture on the grabber from the press to its release: without it a
/// topmost window above ours could take the release and leave the marquee
/// live until the idle limit.
fn capture(track: &mut Track) -> bool {
    let Some(grab) = track.grab.as_ref() else {
        return false;
    };
    // SAFETY: the grabber is this thread's own live window.
    let held = unsafe { MouseCapture::capture(grab.window()) };
    let taken = held.is_some();
    track.capture = held;
    taken
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

/// Cancel the pick and drop the outline. The pump's own predicate sees the
/// finished session and ends within one wake, so no quit message is needed:
/// a `WM_QUIT` posted from inside the pick's dispatch outlives the pick's
/// pump and would end the tray loop behind it — the tray, its icon, and a
/// recording the pick just started.
fn cancel() {
    abandon();
}

/// Abandon a session that received no input: the same state as a cancel.
/// The pump has already returned here, so nothing is posted; the next pick
/// resets the track anyway.
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
