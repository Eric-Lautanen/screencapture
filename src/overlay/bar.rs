//! The recording frame's control bar.
//!
//! One small, non-activating, topmost window straddles the frame's top edge
//! and carries the stop button and the switches for the microphone and the
//! system mix. It is drawn as a single layer: the bar's body, its hairline
//! rim, and the line icons are rendered four times oversized into a working
//! bitmap and box-filtered down, which is what gives the rounded ends and
//! the icon strokes their antialiased edge without a drawing dependency.
//!
//! Every button acts on mouse-down: a switch has to change under the
//! pointer, not after it lifts. The press captures the pointer, so a hand
//! that drifts a pixel still gets its button back.

use std::cell::RefCell;
use std::ffi::c_void;

use crate::audio::Live;
use crate::capture::Region;
use crate::winapi::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, Hcursor, Hdc, Hinstance, Hwnd, Lparam, Lresult,
    Point, Rect, RegisterClassExW, WndClassExW, Wparam,
};

use super::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, HWND_TOPMOST, KillTimer, LoadCursorW,
    SWP_NOACTIVATE, SelectObject, SetTimer, SetWindowDisplayAffinity, SetWindowPos, StopAction,
    WDA_EXCLUDEFROMCAPTURE, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_TIMER, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    chrome_percent, kept_wide, module_handle, rgb, scaled, wide,
};

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: Hdc) -> Hdc;
    fn DeleteDC(dc: Hdc) -> i32;
    fn CreateDIBSection(
        dc: Hdc,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut c_void,
        section: *mut c_void,
        offset: u32,
    ) -> *mut c_void;
    fn CreatePen(style: i32, width: i32, color: u32) -> *mut c_void;
    fn RoundRect(
        dc: Hdc,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        width: i32,
        height: i32,
    ) -> i32;
    fn Polygon(dc: Hdc, points: *const Point, count: i32) -> i32;
    fn IntersectClipRect(dc: Hdc, left: i32, top: i32, right: i32, bottom: i32) -> i32;
    fn SaveDC(dc: Hdc) -> i32;
    fn RestoreDC(dc: Hdc, level: i32) -> i32;
    fn MoveToEx(dc: Hdc, x: i32, y: i32, previous: *mut Point) -> i32;
    fn LineTo(dc: Hdc, x: i32, y: i32) -> i32;
    fn GetStockObject(index: i32) -> *mut c_void;
    fn SetMapMode(dc: Hdc, mode: i32) -> i32;
    fn SetWindowExtEx(dc: Hdc, x: i32, y: i32, previous: *mut Size) -> i32;
    fn SetViewportExtEx(dc: Hdc, x: i32, y: i32, previous: *mut Size) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn ShowWindow(window: Hwnd, command: i32) -> i32;
    fn TrackMouseEvent(event: *mut TrackMouse) -> i32;
    fn UpdateLayeredWindow(
        window: Hwnd,
        destination: Hdc,
        at: *const Point,
        size: *const Size,
        source: Hdc,
        from: *const Point,
        key: u32,
        blend: *const Blend,
        flags: u32,
    ) -> i32;
}

/// `BITMAPINFOHEADER` (wingdi.h).
#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pixels: i32,
    y_pixels: i32,
    used: u32,
    important: u32,
}

/// `BITMAPINFO` (wingdi.h) with room for one palette entry, which a
/// 32-bit bitmap never fills.
#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 1],
}

/// `SIZE` (windef.h).
#[repr(C)]
struct Size {
    cx: i32,
    cy: i32,
}

/// `BLENDFUNCTION` (wingdi.h): straight alpha over the desktop.
#[repr(C)]
struct Blend {
    operation: u8,
    flags: u8,
    alpha: u8,
    format: u8,
}

/// `TRACKMOUSEEVENT` (winuser.h).
#[repr(C)]
struct TrackMouse {
    size: u32,
    flags: u32,
    window: Hwnd,
    hover_time: u32,
}

/// `PS_SOLID`, `NULL_BRUSH`, and `MM_ANISOTROPIC`.
const PS_SOLID: i32 = 0;
const NULL_BRUSH: i32 = 5;
const MM_ANISOTROPIC: i32 = 8;
/// `SW_SHOWNOACTIVATE` (winuser.h): show the bar without taking focus.
const SW_SHOWNOACTIVATE: i32 = 4;
/// `TME_LEAVE`, `WM_MOUSELEAVE`, `WM_CAPTURECHANGED`.
const TME_LEAVE: u32 = 0x0000_0002;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_CAPTURECHANGED: u32 = 0x0215;
/// The one-shot paint that follows a click which had to open a source: the
/// worker's answer lands a beat later, and that answer is worth a frame.
const SETTLE_TIMER: usize = 1;
const SETTLE_STEP: u32 = 200;
/// `ULW_ALPHA`, `AC_SRC_OVER`, and `AC_SRC_ALPHA`.
const ULW_ALPHA: u32 = 0x0000_0002;
const AC_SRC_OVER: u8 = 0;
const AC_SRC_ALPHA: u8 = 1;

/// Window class of the bar.
const CLASS_NAME: &str = "screencapture.recording.bar";

/// Supersampling factor: every logical pixel is drawn into a four-by-four
/// block and averaged back down.
const SCALE: i32 = 4;

/// The bar's geometry, in physical pixels: a 12-pixel pill with 28-pixel
/// buttons and a four-pixel inset.
const HEIGHT: i32 = 36;
const RADIUS: i32 = 12;
const PADDING: i32 = 4;
const BUTTON: i32 = 28;
const GAP: i32 = 2;
const BUTTON_RADIUS: i32 = 9;
/// The pill's designed size at 100%, and the measure the physical-size floor
/// in [`super::percent_for`] is taken against.
pub(super) const DESIGN_WIDTH: i32 = PADDING * 2 + BUTTON * 3 + GAP * 2;
pub(super) const DESIGN_HEIGHT: i32 = HEIGHT;
const ICON: i32 = 16;

// The bar borrows the frame's own edge, so the pill reads as part of the
// border it sits on rather than a chip resting against it. The frame draws
// its hairline dark outside and light inside; the pill draws the same two
// lines, and its body is the frame's shade lifted towards that light line —
// a near-black slab would break the frame it is supposed to continue.
const BAR_BG: u32 = rgb(0x1D, 0x1E, 0x22);
const BAR_EDGE: u32 = super::FRAME_SHADE;
/// The light hairline: the frame's own light tone at the middle of its
/// breath, which is how bright that line reads over the body.
const BAR_RIM: u32 = rgb(0xD3, 0xD4, 0xD8);
const BUTTON_HOVER: u32 = rgb(0x2A, 0x2B, 0x31);
const BUTTON_PRESS: u32 = rgb(0x35, 0x36, 0x3E);
const STOP_HOVER: u32 = rgb(0x33, 0x26, 0x2B);
const STOP_PRESS: u32 = rgb(0x3F, 0x2E, 0x34);
const INK: u32 = rgb(0xF1, 0xF2, 0xF4);
const INK_MUTED: u32 = rgb(0x94, 0x98, 0x9F);
const STOP_RED: u32 = rgb(0xFF, 0x5C, 0x5C);

/// The transparent key of the working bitmap; no chip colour may equal it.
const KEY: u32 = 0x00FF_00FF;

/// What a bar button does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Role {
    /// Ends the take.
    Stop,
    /// Mutes the microphone for the rest of the take.
    Microphone,
    /// Mutes the system mix for the rest of the take.
    System,
}

/// Where the buttons sit: the bar's width and one frame each. Both audio
/// buttons are always there — a source the settings left off can still be
/// brought in mid-take — but a region too narrow for the bar keeps only the
/// stop button.
#[derive(Clone, Copy)]
struct Layout {
    /// The designed width at 96 DPI; the device width follows the display.
    width: i32,
    /// The display's scale percentage this bar was laid out for.
    percent: i32,
    stop: Rect,
    microphone: Option<Rect>,
    system: Option<Rect>,
}

impl Layout {
    /// Lay out the bar for a region `available` device pixels wide: stop, the
    /// microphone, and the system mix, or the stop button alone when the row
    /// does not fit at this display's scale.
    fn new(available: i32, percent: i32) -> Self {
        let percent = percent.clamp(100, 400);
        let width = DESIGN_WIDTH;
        if scaled(width, percent) > available {
            return Self::stop_only(percent);
        }
        Self {
            width,
            percent,
            stop: slot(0),
            microphone: Some(slot(1)),
            system: Some(slot(2)),
        }
    }

    /// The bar with the stop button alone.
    const fn stop_only(percent: i32) -> Self {
        Self {
            width: PADDING * 2 + BUTTON,
            percent,
            stop: slot(0),
            microphone: None,
            system: None,
        }
    }

    /// The bar's width in the display's own pixels.
    fn device_width(self) -> i32 {
        scaled(self.width, self.percent)
    }

    /// The bar's height in the display's own pixels.
    fn device_height(self) -> i32 {
        scaled(HEIGHT, self.percent)
    }

    /// One designed rect in the display's own pixels: what a click at that
    /// button's position actually sends. Only the tests need it, since the
    /// bar itself paints in designed units.
    #[cfg(test)]
    fn device(self, rect: Rect) -> Rect {
        Rect {
            left: scaled(rect.left, self.percent),
            top: scaled(rect.top, self.percent),
            right: scaled(rect.right, self.percent),
            bottom: scaled(rect.bottom, self.percent),
        }
    }

    /// One button's frame, or `None` when the take has no such button.
    const fn button(self, role: Role) -> Option<Rect> {
        match role {
            Role::Stop => Some(self.stop),
            Role::Microphone => self.microphone,
            Role::System => self.system,
        }
    }

    /// The button under a point in the display's own pixels: the mouse
    /// reports device coordinates, the buttons are designed ones.
    fn role_at(self, x: i32, y: i32) -> Option<Role> {
        let (x, y) = (x * 100 / self.percent, y * 100 / self.percent);
        [Role::Stop, Role::Microphone, Role::System]
            .into_iter()
            .find(|role| self.button(*role).is_some_and(|rect| hits(rect, x, y)))
    }

    /// The bar's frame for a capture region: centered on the top edge, and
    /// pushed just inside the region when that edge sits at the top of the
    /// screen (where half the bar would fall off it).
    fn rect(self, region: Region) -> Rect {
        let width = i32::try_from(region.width).unwrap_or(0);
        let bar_width = self.device_width();
        let bar_height = self.device_height();
        let x = region.x + (width - bar_width).max(0) / 2;
        let lifted = region.y - bar_height / 2;
        let y = if lifted < 0 { region.y } else { lifted };
        Rect {
            left: x,
            top: y,
            right: x + bar_width,
            bottom: y + bar_height,
        }
    }
}

/// The `index`th button's frame, in the bar's own coordinates.
const fn slot(index: i32) -> Rect {
    let left = PADDING + index * (BUTTON + GAP);
    let top = (HEIGHT - BUTTON) / 2;
    Rect {
        left,
        top,
        right: left + BUTTON,
        bottom: top + BUTTON,
    }
}

/// Whether `(x, y)` falls inside `rect`.
const fn hits(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

/// The whole bar's state: where its buttons are, what they do, and which
/// of them the pointer is on.
struct Bar {
    layout: Layout,
    stop: Option<StopAction>,
    microphone: Option<Live>,
    system: Option<Live>,
    hover: Option<Role>,
    pressed: Option<Role>,
}

/// What one paint reads from [`Bar`], copied out so drawing never holds the
/// lock.
#[derive(Clone, Copy, Debug, Default)]
struct Look {
    hover: Option<Role>,
    pressed: Option<Role>,
    microphone_muted: bool,
    system_muted: bool,
}

thread_local! {
    /// One recording at a time, so one bar is enough.
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// Forget every armed button; called when the frame is torn down.
pub(super) fn disarm() {
    BAR.with_borrow_mut(|slot| *slot = None);
}

/// Show the bar over `region`, wired to the take's `live` audio switches.
/// `None` when Windows refuses the window or the layer (the outline,
/// Escape, and the hotkey still stop the take).
pub(super) fn show_bar(region: Region, action: StopAction, live: &Live) -> Option<Hwnd> {
    let instance = module_handle();
    register_class(instance);
    let class = wide(CLASS_NAME);
    // Created hidden at the region's own origin: the bar's size depends on
    // the display's scale, which only answers for the monitor the window is
    // already on.
    // SAFETY: strings and handles outlive the call; the window is owned by
    // this process and destroyed by the caller's guard.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            region.x,
            region.y,
            DESIGN_WIDTH,
            DESIGN_HEIGHT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        )
    };
    if window.is_null() {
        return None;
    }
    let available = i32::try_from(region.width).unwrap_or(0);
    let layout = Layout::new(available, chrome_percent(window));
    let rect = layout.rect(region);
    BAR.with_borrow_mut(|slot| {
        *slot = Some(Bar {
            layout,
            stop: Some(action),
            microphone: Some(live.clone()),
            system: Some(live.clone()),
            hover: None,
            pressed: None,
        });
    });
    // SAFETY: `window` was created above on this thread.
    unsafe {
        // Keep the bar out of the recording it controls; older Windows
        // builds refuse and the bar then appears in the shot, which is a
        // cosmetic loss, not a failure.
        let _ = SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE);
        SetWindowPos(
            window,
            HWND_TOPMOST,
            rect.left,
            rect.top,
            layout.device_width(),
            layout.device_height(),
            SWP_NOACTIVATE,
        );
    }
    if !redraw(window) {
        // A bar nobody can see would swallow clicks over a strip of the
        // recording, so a refused layer takes the window down with it.
        // SAFETY: the window created above, not yet shown to anyone.
        unsafe {
            DestroyWindow(window);
        }
        disarm();
        return None;
    }
    // SAFETY: a live window this process owns, created hidden above.
    unsafe {
        ShowWindow(window, SW_SHOWNOACTIVATE);
    }
    Some(window)
}

/// Tear down the bar window and disarm its buttons.
pub(super) fn destroy(window: Hwnd) {
    disarm();
    // SAFETY: upheld by the caller (`Border::drop`).
    unsafe {
        DestroyWindow(window);
    }
}

/// Register the bar's window class; repeats answer "already registered".
///
/// The name buffer is kept for the process lifetime: the class table stores
/// the pointer it was given, so a local buffer would dangle after return.
fn register_class(instance: Hinstance) {
    let name = kept_wide(CLASS_NAME);
    let class = WndClassExW {
        size: u32::try_from(std::mem::size_of::<WndClassExW>()).expect("class size fits u32"),
        style: 0,
        wnd_proc: procedure,
        cls_extra: 0,
        wnd_extra: 0,
        instance,
        icon: std::ptr::null_mut(),
        cursor: hand_cursor(),
        background: std::ptr::null_mut(),
        menu_name: std::ptr::null(),
        class_name: name,
        icon_small: std::ptr::null_mut(),
    };
    // SAFETY: the class struct is a fully initialized local and the name
    // buffer outlives the registration.
    unsafe {
        RegisterClassExW(&raw const class);
    }
}

/// The bar's hover cursor: the hand pointer that marks a control as
/// clickable. A class cursor is also what keeps the cursor *stable*: the
/// bar appears while the pump thread is still starting the recorder, so
/// Windows paints its "app starting" spinner over the fresh window, and a
/// class with no cursor would leave that spinner in place.
fn hand_cursor() -> Hcursor {
    // SAFETY: null instance resolves shared stock cursor resources.
    unsafe {
        let hand = LoadCursorW(std::ptr::null_mut(), crate::winapi::IDC_HAND as *const u16);
        if hand.is_null() {
            LoadCursorW(std::ptr::null_mut(), crate::winapi::IDC_ARROW as *const u16)
        } else {
            hand
        }
    }
}

/// The bar's window procedure: hover, press, and the layer's repaint.
///
/// # Safety
/// `window` must be a live bar window owned by the calling thread.
unsafe extern "system" fn procedure(
    window: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    match message {
        // The layer owns the bar's pixels; the paint only has to validate
        // the region Windows just marked dirty.
        WM_PAINT => {
            // SAFETY: the paint state buffer fits the platform PAINTSTRUCT.
            unsafe { validate(window) };
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            let (x, y) = point_of(lparam);
            hover(window, x, y);
            0
        }
        WM_MOUSELEAVE => {
            set_hover(window, None);
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point_of(lparam);
            press(window, x, y);
            1
        }
        WM_LBUTTONUP => {
            // SAFETY: the capture belongs to this window.
            unsafe {
                ReleaseCapture();
            }
            set_pressed(window, None);
            1
        }
        WM_CAPTURECHANGED => {
            set_pressed(window, None);
            0
        }
        WM_TIMER => {
            settle(window);
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Validate the paint region without drawing: the layer is the content.
///
/// # Safety
/// `window` must be a live window owned by the calling thread.
unsafe fn validate(window: Hwnd) {
    let mut state = [0u8; 128];
    // SAFETY: `state` is at least `sizeof(PAINTSTRUCT)` for this ABI.
    let dc = unsafe { BeginPaint(window, state.as_mut_ptr().cast()) };
    if !dc.is_null() {
        // SAFETY: the same paint session `BeginPaint` opened.
        unsafe {
            EndPaint(window, state.as_ptr().cast());
        }
    }
}

/// Split a mouse message's `lparam` into client coordinates.
fn point_of(lparam: Lparam) -> (i32, i32) {
    let low = u16::try_from(lparam & 0xFFFF).unwrap_or(0);
    let high = u16::try_from((lparam >> 16) & 0xFFFF).unwrap_or(0);
    (
        i32::from(i16::from_ne_bytes(low.to_ne_bytes())),
        i32::from(i16::from_ne_bytes(high.to_ne_bytes())),
    )
}

/// Track which button the pointer is on, and repaint when that changes.
fn hover(window: Hwnd, x: i32, y: i32) {
    let role = BAR.with_borrow(|slot| slot.as_ref().and_then(|bar| bar.layout.role_at(x, y)));
    set_hover(window, role);
}

/// Set the hovered button and repaint when it actually changed.
fn set_hover(window: Hwnd, role: Option<Role>) {
    let changed = BAR.with_borrow_mut(|slot| {
        let Some(bar) = slot.as_mut() else {
            return false;
        };
        if bar.hover == role {
            return false;
        }
        bar.hover = role;
        true
    });
    if !changed {
        return;
    }
    if role.is_some() {
        let mut event = TrackMouse {
            size: u32::try_from(std::mem::size_of::<TrackMouse>()).expect("track size fits u32"),
            flags: TME_LEAVE,
            window,
            hover_time: 0,
        };
        // SAFETY: the struct is a live local initialized for this window.
        unsafe {
            TrackMouseEvent(&raw mut event);
        }
    }
    redraw(window);
}

/// Repaint for the worker's answer to a click that had to open a source:
/// the button already shows what the click asked for, and this frame is
/// where a device that refused takes it back. Stops the timer once nothing
/// is left starting.
fn settle(window: Hwnd) {
    redraw(window);
    if starting() {
        return;
    }
    // SAFETY: a live bar window this process owns.
    unsafe {
        KillTimer(window, SETTLE_TIMER);
    }
}

/// Whether a source the bar asked for is still opening. A muted or settled
/// source is no longer the worker's business.
fn starting() -> bool {
    BAR.with_borrow(|slot| {
        slot.as_ref().is_some_and(|bar| {
            bar.microphone.as_ref().is_some_and(Live::starting)
                || bar.system.as_ref().is_some_and(Live::starting)
        })
    })
}

/// Ask for one more frame after a click that had to open a source.
fn arm_settle(window: Hwnd) {
    if !starting() {
        return;
    }
    // SAFETY: a live window this process owns; the id is this bar's own, so
    // a second click resets the one-shot timer instead of stacking another.
    unsafe {
        SetTimer(window, SETTLE_TIMER, SETTLE_STEP, std::ptr::null());
    }
}

/// Set the pressed button and repaint when it actually changed.
fn set_pressed(window: Hwnd, role: Option<Role>) {
    let changed = BAR.with_borrow_mut(|slot| {
        let Some(bar) = slot.as_mut() else {
            return false;
        };
        if bar.pressed == role {
            return false;
        }
        bar.pressed = role;
        true
    });
    if changed {
        redraw(window);
    }
}

/// Run the button under the pointer. The press is captured so the release
/// always comes back here, and the action happens *now*: a mute switch that
/// waited for the release would land a beat after the click.
fn press(window: Hwnd, x: i32, y: i32) {
    let role = BAR.with_borrow(|slot| slot.as_ref().and_then(|bar| bar.layout.role_at(x, y)));
    let Some(role) = role else {
        return;
    };
    // SAFETY: `window` is the bar being clicked.
    unsafe {
        SetCapture(window);
    }
    BAR.with_borrow_mut(|slot| {
        if let Some(bar) = slot.as_mut() {
            bar.pressed = Some(role);
        }
    });
    // Flip the switch, then paint once: the press and the new state have
    // to land in the same frame, or the button looks a beat behind the
    // click. The log line is written afterwards, off the click's path.
    let note = activate(role);
    redraw(window);
    arm_settle(window);
    if let Some(note) = note {
        crate::log::note("audio", note);
    }
}

/// Run the button's action: stop the take, or flip one audio switch. The
/// answer is the log line for a successful toggle, which the caller writes
/// after the button has already repainted.
fn activate(role: Role) -> Option<&'static str> {
    // The stop action leaves the bar before it runs. It posts a tray
    // command, and that command tears this bar down; calling it while the
    // bar's own state is borrowed would meet the teardown's `disarm` with a
    // second borrow of the same cell.
    let stop = BAR.with_borrow(|slot| match role {
        Role::Stop => slot.as_ref().and_then(|bar| bar.stop),
        Role::Microphone | Role::System => None,
    });
    if let Some(stop) = stop {
        stop();
        return None;
    }
    BAR.with_borrow(|slot| {
        let bar = slot.as_ref()?;
        match role {
            Role::Stop => None,
            Role::Microphone => bar.microphone.as_ref()?.toggle_mic().map(|muted| {
                if muted {
                    "microphone muted for this take"
                } else {
                    "microphone unmuted"
                }
            }),
            Role::System => bar.system.as_ref()?.toggle_system().map(|muted| {
                if muted {
                    "system audio muted for this take"
                } else {
                    "system audio unmuted"
                }
            }),
        }
    })
}

/// Render the bar's current state into its layer. Answers whether the
/// layer was accepted.
fn redraw(window: Hwnd) -> bool {
    let state = BAR.with_borrow(|slot| slot.as_ref().map(|bar| (bar.layout, look_of(bar))));
    let Some((layout, look)) = state else {
        return false;
    };
    let Some(surface) = Surface::new(layout) else {
        return false;
    };
    surface.clear();
    paint(&surface.work, layout, look);
    surface.flatten();
    let mut frame = Rect::default();
    // SAFETY: writes only the local frame.
    unsafe {
        GetWindowRect(window, &raw mut frame);
    }
    surface.present(window, frame.left, frame.top)
}

/// Copy what one paint needs out of the bar.
fn look_of(bar: &Bar) -> Look {
    Look {
        hover: bar.hover,
        pressed: bar.pressed,
        microphone_muted: bar.microphone.as_ref().and_then(Live::mic).unwrap_or(false),
        system_muted: bar.system.as_ref().and_then(Live::system).unwrap_or(false),
    }
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetWindowRect(window: Hwnd, rect: *mut Rect) -> i32;
    fn SetCapture(window: Hwnd) -> Hwnd;
    fn ReleaseCapture() -> i32;
}

/// Draw the whole bar: body, hover, buttons, then the rim over the lot.
fn paint(canvas: &Canvas, layout: Layout, look: Look) {
    let whole = Rect {
        left: 0,
        top: 0,
        right: layout.width,
        bottom: HEIGHT,
    };
    fill_round(canvas, whole, RADIUS, BAR_BG);
    for role in [Role::Stop, Role::Microphone, Role::System] {
        let Some(button) = layout.button(role) else {
            continue;
        };
        let resting = if role == Role::Stop {
            STOP_HOVER
        } else {
            BUTTON_HOVER
        };
        let held = if role == Role::Stop {
            STOP_PRESS
        } else {
            BUTTON_PRESS
        };
        let background = match (look.hover == Some(role), look.pressed == Some(role)) {
            (false, _) => BAR_BG,
            (true, false) => resting,
            (true, true) => held,
        };
        if background != BAR_BG {
            fill_round(canvas, button, BUTTON_RADIUS, background);
        }
        match role {
            Role::Stop => stop_icon(canvas, button),
            Role::Microphone => microphone_icon(canvas, button, look.microphone_muted, background),
            Role::System => system_icon(canvas, button, look.system_muted, background),
        }
    }
    // The frame's edge, wrapped around the buttons: a shade line at the
    // very outside, the light line one pixel in.
    let edge = Rect {
        left: 0,
        top: 0,
        right: layout.width,
        bottom: HEIGHT,
    };
    stroke_round(canvas, edge, RADIUS, BAR_EDGE);
    let rim = Rect {
        left: 1,
        top: 1,
        right: layout.width - 1,
        bottom: HEIGHT - 1,
    };
    stroke_round(canvas, rim, RADIUS - 1, BAR_RIM);
}

/// The stop square: one soft red rounded square, nothing louder.
fn stop_icon(canvas: &Canvas, button: Rect) {
    let (x, y) = icon_origin(button);
    fill_round(
        canvas,
        Rect {
            left: x + 4,
            top: y + 4,
            right: x + 12,
            bottom: y + 12,
        },
        2,
        STOP_RED,
    );
}

/// The microphone: a capsule inside its cradle, on a stem and a foot. A
/// muted microphone goes quiet grey and takes a slash.
fn microphone_icon(canvas: &Canvas, button: Rect, muted: bool, background: u32) {
    let (x, y) = icon_origin(button);
    let ink = if muted { INK_MUTED } else { INK };
    fill_round(
        canvas,
        Rect {
            left: x + 5,
            top: y + 1,
            right: x + 11,
            bottom: y + 9,
        },
        3,
        ink,
    );
    // The cradle is the bottom of a rounded ring: clipped at the capsule's
    // waist, what is left reads as the U the microphone sits in.
    // SAFETY: the canvas's DC is this function's to clip, and the saved
    // state puts the clip back exactly as it was.
    unsafe {
        SaveDC(canvas.dc);
        IntersectClipRect(canvas.dc, x, y + 7, x + ICON, y + ICON);
        stroke_round(
            canvas,
            Rect {
                left: x + 4,
                top: y + 3,
                right: x + 12,
                bottom: y + 13,
            },
            4,
            ink,
        );
        RestoreDC(canvas.dc, -1);
    }
    line(canvas, (x + 8, y + 13), (x + 8, y + 14), 1, ink);
    line(canvas, (x + 5, y + 15), (x + 11, y + 15), 1, ink);
    if muted {
        slash(canvas, x, y, background);
    }
}

/// The speaker: a cone with two waves. A muted mix goes quiet grey and
/// takes a slash.
fn system_icon(canvas: &Canvas, button: Rect, muted: bool, background: u32) {
    let (x, y) = icon_origin(button);
    let ink = if muted { INK_MUTED } else { INK };
    fill_polygon(
        canvas,
        &[
            (x + 2, y + 7),
            (x + 4, y + 7),
            (x + 8, y + 3),
            (x + 8, y + 13),
            (x + 4, y + 9),
            (x + 2, y + 9),
        ],
        ink,
    );
    line(canvas, (x + 10, y + 5), (x + 12, y + 8), 1, ink);
    line(canvas, (x + 12, y + 8), (x + 10, y + 11), 1, ink);
    line(canvas, (x + 13, y + 3), (x + 15, y + 8), 1, ink);
    line(canvas, (x + 15, y + 8), (x + 13, y + 13), 1, ink);
    if muted {
        slash(canvas, x, y, background);
    }
}

/// Cut a gap through the icon, then draw the slash through it: the gap is
/// what keeps the slash legible over a capsule or a cone.
fn slash(canvas: &Canvas, x: i32, y: i32, background: u32) {
    line(canvas, (x + 3, y + 13), (x + 13, y + 3), 4, background);
    line(canvas, (x + 3, y + 13), (x + 13, y + 3), 1, INK);
}

/// A button's icon origin: the 16-pixel grid centered in the 28-pixel
/// button.
const fn icon_origin(button: Rect) -> (i32, i32) {
    (
        button.left + (BUTTON - ICON) / 2,
        button.top + (BUTTON - ICON) / 2,
    )
}

// --- Drawing -------------------------------------------------------------
//
// Every call below takes logical bar pixels; the canvas maps them onto its
// supersampled bitmap, so a one-pixel pen lands as four and comes back as
// one crisp pixel after the filter.

/// Fill a rounded rectangle.
fn fill_round(canvas: &Canvas, area: Rect, radius: i32, colour: u32) {
    // SAFETY: the brush and pen are selected on a canvas this function owns
    // and released before it returns.
    unsafe {
        let brush = CreateSolidBrush(colour);
        if brush.is_null() {
            return;
        }
        let pen = CreatePen(PS_SOLID, 1, colour);
        let previous_brush = SelectObject(canvas.dc, brush);
        let previous_pen = if pen.is_null() {
            std::ptr::null_mut()
        } else {
            SelectObject(canvas.dc, pen)
        };
        RoundRect(
            canvas.dc,
            area.left,
            area.top,
            area.right,
            area.bottom,
            radius * 2,
            radius * 2,
        );
        SelectObject(canvas.dc, previous_brush);
        if !previous_pen.is_null() {
            SelectObject(canvas.dc, previous_pen);
        }
        if !pen.is_null() {
            DeleteObject(pen);
        }
        DeleteObject(brush);
    }
}

/// Outline a rounded rectangle with a hairline.
fn stroke_round(canvas: &Canvas, area: Rect, radius: i32, colour: u32) {
    // SAFETY: the pen is selected on a canvas this function owns and
    // released before it returns; `NULL_BRUSH` is a shared stock object
    // that must not be deleted.
    unsafe {
        let pen = CreatePen(PS_SOLID, 1, colour);
        if pen.is_null() {
            return;
        }
        let brush = GetStockObject(NULL_BRUSH);
        let previous_brush = SelectObject(canvas.dc, brush);
        let previous_pen = SelectObject(canvas.dc, pen);
        RoundRect(
            canvas.dc,
            area.left,
            area.top,
            area.right,
            area.bottom,
            radius * 2,
            radius * 2,
        );
        SelectObject(canvas.dc, previous_pen);
        SelectObject(canvas.dc, previous_brush);
        DeleteObject(pen);
    }
}

/// Fill a polygon.
fn fill_polygon(canvas: &Canvas, points: &[(i32, i32)], colour: u32) {
    let points: Vec<Point> = points.iter().map(|&(x, y)| Point { x, y }).collect();
    // SAFETY: the brush and pen are selected on a canvas this function owns
    // and released before it returns; the point buffer outlives the call.
    unsafe {
        let brush = CreateSolidBrush(colour);
        if brush.is_null() {
            return;
        }
        let pen = CreatePen(PS_SOLID, 1, colour);
        let previous_brush = SelectObject(canvas.dc, brush);
        let previous_pen = if pen.is_null() {
            std::ptr::null_mut()
        } else {
            SelectObject(canvas.dc, pen)
        };
        Polygon(
            canvas.dc,
            points.as_ptr(),
            i32::try_from(points.len()).unwrap_or(0),
        );
        SelectObject(canvas.dc, previous_brush);
        if !previous_pen.is_null() {
            SelectObject(canvas.dc, previous_pen);
        }
        if !pen.is_null() {
            DeleteObject(pen);
        }
        DeleteObject(brush);
    }
}

/// Draw a line of `width` logical pixels.
fn line(canvas: &Canvas, from: (i32, i32), to: (i32, i32), width: i32, colour: u32) {
    // SAFETY: the pen is selected on a canvas this function owns and
    // released before it returns.
    unsafe {
        let pen = CreatePen(PS_SOLID, width, colour);
        if pen.is_null() {
            return;
        }
        let previous_pen = SelectObject(canvas.dc, pen);
        MoveToEx(canvas.dc, from.0, from.1, std::ptr::null_mut());
        LineTo(canvas.dc, to.0, to.1);
        SelectObject(canvas.dc, previous_pen);
        DeleteObject(pen);
    }
}

// --- The layer -----------------------------------------------------------

/// One 32-bit top-down bitmap and the memory context that draws into it.
struct Canvas {
    dc: Hdc,
    bitmap: *mut c_void,
    previous: *mut c_void,
    bits: *mut u32,
    width: i32,
    height: i32,
}

impl Canvas {
    /// Create a bitmap of `width` by `height` pixels, select it, and hand
    /// back its pixels for the filter.
    fn new(width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let header = BitmapInfoHeader {
            size: u32::try_from(std::mem::size_of::<BitmapInfoHeader>()).ok()?,
            width,
            height: -height,
            planes: 1,
            bit_count: 32,
            compression: 0,
            size_image: 0,
            x_pixels: 0,
            y_pixels: 0,
            used: 0,
            important: 0,
        };
        let info = BitmapInfo {
            header,
            colors: [0],
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: the info struct is a live, fully initialized local; the
        // out slot is a local pointer.
        let (dc, bitmap) = unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return None;
            }
            let bitmap = CreateDIBSection(
                dc,
                &raw const info,
                0,
                &raw mut bits,
                std::ptr::null_mut(),
                0,
            );
            (dc, bitmap)
        };
        if bitmap.is_null() || bits.is_null() {
            // SAFETY: the DC was created above and holds no bitmap yet.
            unsafe {
                DeleteDC(dc);
            }
            return None;
        }
        // SAFETY: a live DC and a bitmap created for it.
        let previous = unsafe { SelectObject(dc, bitmap) };
        Some(Self {
            dc,
            bitmap,
            previous,
            bits: bits.cast(),
            width,
            height,
        })
    }

    /// Map the bar's designed pixels onto this canvas: one design pixel
    /// spans `width / design_width` device pixels, which is the display's
    /// scale times the four-fold oversize every drawn edge is filtered out
    /// of. GDI scales the pens with the mapping, so the icons grow with the
    /// bar instead of staying 16 pixels on a 200% screen.
    fn scale(&self, design: (i32, i32)) {
        // SAFETY: a live canvas this value owns.
        unsafe {
            SetMapMode(self.dc, MM_ANISOTROPIC);
            SetWindowExtEx(self.dc, design.0, design.1, std::ptr::null_mut());
            SetViewportExtEx(self.dc, self.width, self.height, std::ptr::null_mut());
        }
    }

    /// Paint every pixel with the transparent key.
    fn clear(&self) {
        for pixel in self.pixels() {
            // SAFETY: the loop walks exactly the bitmap's own pixels.
            unsafe {
                *pixel = KEY;
            }
        }
    }

    /// Every pixel of the bitmap, in memory order.
    fn pixels(&self) -> impl Iterator<Item = *mut u32> + '_ {
        let count = usize::try_from(self.width)
            .unwrap_or(0)
            .saturating_mul(usize::try_from(self.height).unwrap_or(0));
        (0..count).map(|index| {
            // SAFETY: the index is bounded by the bitmap's own size.
            unsafe { self.bits.add(index) }
        })
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: the bitmap is this value's; the previous one is put back
        // before it is deleted, and the DC is this value's.
        unsafe {
            if !self.previous.is_null() {
                SelectObject(self.dc, self.previous);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
        }
    }
}

/// The bar's layer: the oversized working bitmap it is drawn into and the
/// premultiplied bitmap Windows blits to the screen.
struct Surface {
    work: Canvas,
    out: Canvas,
    width: i32,
    height: i32,
}

impl Surface {
    /// A layer the size the bar's layout asks for on this display.
    fn new(layout: Layout) -> Option<Self> {
        let width = layout.device_width();
        let height = layout.device_height();
        let work = Canvas::new(width * SCALE, height * SCALE)?;
        work.scale((layout.width, HEIGHT));
        let out = Canvas::new(width, height)?;
        Some(Self {
            work,
            out,
            width,
            height,
        })
    }

    /// Paint the working bitmap with the transparent key.
    fn clear(&self) {
        self.work.clear();
    }

    /// Box-filter the working bitmap down into the layer, turning the key
    /// into transparency and premultiplying what is left.
    fn flatten(&self) {
        let steps = u32::try_from(SCALE * SCALE).unwrap_or(16);
        let step = usize::try_from(SCALE).unwrap_or(1);
        let work_width = usize::try_from(self.work.width).unwrap_or(0);
        let width = usize::try_from(self.width).unwrap_or(0);
        let height = usize::try_from(self.height).unwrap_or(0);
        for y in 0..height {
            for x in 0..width {
                let mut red = 0_u32;
                let mut green = 0_u32;
                let mut blue = 0_u32;
                let mut covered = 0_u32;
                for sy in 0..step {
                    for sx in 0..step {
                        let index = (y * step + sy) * work_width + x * step + sx;
                        // SAFETY: every subsample of every pixel is inside
                        // the working bitmap, which is `step` times bigger.
                        let pixel = unsafe { *self.work.bits.add(index) };
                        if pixel & 0x00FF_FFFF == KEY {
                            continue;
                        }
                        red += (pixel >> 16) & 0xFF;
                        green += (pixel >> 8) & 0xFF;
                        blue += pixel & 0xFF;
                        covered += 1;
                    }
                }
                let alpha = (covered * 255 + steps / 2) / steps;
                let pixel = (alpha << 24)
                    | (((red + steps / 2) / steps) << 16)
                    | (((green + steps / 2) / steps) << 8)
                    | ((blue + steps / 2) / steps);
                // SAFETY: the index is inside the layer's own bitmap.
                unsafe {
                    *self.out.bits.add(y * width + x) = pixel;
                }
            }
        }
    }

    /// Hand the layer to the window; answers whether Windows took it.
    fn present(&self, window: Hwnd, left: i32, top: i32) -> bool {
        let at = Point { x: left, y: top };
        let size = Size {
            cx: self.width,
            cy: self.height,
        };
        let from = Point { x: 0, y: 0 };
        let blend = Blend {
            operation: AC_SRC_OVER,
            flags: 0,
            alpha: 255,
            format: AC_SRC_ALPHA,
        };
        // SAFETY: the destination and source are live values; the source DC
        // holds a bitmap of exactly `size`.
        unsafe {
            UpdateLayeredWindow(
                window,
                std::ptr::null_mut(),
                &raw const at,
                &raw const size,
                self.out.dc,
                &raw const from,
                0,
                &raw const blend,
                ULW_ALPHA,
            ) != 0
        }
    }
}

#[cfg(test)]
mod tests;
