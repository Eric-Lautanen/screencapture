//! Linux display geometry: the default screen of the `DISPLAY` root, in
//! pixels, one display per X screen (the server composites a root that
//! spans every monitor on Xinerama-configured stacks).

use std::io;

use crate::capture::Region;
use crate::screen::Display;
use crate::xlib::{Api, Window};

/// The default root of the default screen.
///
/// # Errors
/// `libX11` is missing or no X server answers.
pub fn list() -> Vec<Display> {
    let Some(session) = Session::open() else {
        return Vec::new();
    };
    vec![Display {
        x: 0,
        y: 0,
        width: session.extent.0,
        height: session.extent.1,
    }]
}

/// One live X connection: the api surface, its connection, root, and
/// the root's pixel extent. The connection closes on drop.
struct Session {
    api: Api,
    display: crate::xlib::Display,
    root: Window,
    extent: (u32, u32),
}

impl Session {
    /// Connect; `None` without a usable libX11/server pair.
    fn open() -> Option<Self> {
        let api = Api::load().ok()?;
        let display = api.open().ok()?;
        let root = api.root(display);
        let extent = api.main_screen(display);
        Some(Self {
            api,
            display,
            root,
            extent,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.api.close(self.display);
    }
}

/// A window's capture region: the client rectangle of the window under
/// the cursor, translated into root coordinates and clamped to the
/// root's extent. The window under the desktop itself names the whole
/// root, matching the Windows desktop-handle answer. `preferred` is a
/// Windows notion (the tray's focus stash); Linux answers from the
/// cursor either way.
///
/// # Errors
/// Fails when no X server/pointer answer, or when the window has no
/// usable client rectangle.
pub fn window_region(preferred: Option<usize>) -> Result<Region, io::Error> {
    let _ = preferred;
    let missing = || io::Error::other("no window under the cursor");
    let Some(session) = Session::open() else {
        return Err(io::Error::other("libX11 is unavailable"));
    };
    let pointer = session
        .api
        .pointer(session.display, session.root)
        .ok_or_else(missing)?;
    // child == 0 names the root/desktop itself; capture the whole root.
    let drawable = if pointer.child == 0 {
        session.root
    } else {
        pointer.child
    };
    let geometry = session
        .api
        .geometry(session.display, drawable)
        .ok_or_else(|| io::Error::other("the target window has no usable bounds"))?;
    let (left, top) = if drawable == session.root {
        (0, 0)
    } else {
        session
            .api
            .translate(session.display, drawable, session.root, 0, 0)
            .ok_or_else(|| io::Error::other("the target window is not on this screen"))?
    };
    // Trim decoration/edges hanging off the root's pixel extent.
    let (root_width, root_height) = session.extent;
    let left_clamped = i64::from(left).max(0);
    let top_clamped = i64::from(top).max(0);
    let right = (i64::from(left) + i64::from(geometry.width)).min(i64::from(root_width));
    let bottom = (i64::from(top) + i64::from(geometry.height)).min(i64::from(root_height));
    if right <= left_clamped || bottom <= top_clamped {
        return Err(io::Error::other("the target window is not on this screen"));
    }
    Ok(Region {
        x: i32::try_from(left_clamped).map_err(|_| missing())?,
        y: i32::try_from(top_clamped).map_err(|_| missing())?,
        width: u32::try_from(right - left_clamped).map_err(|_| missing())?,
        height: u32::try_from(bottom - top_clamped).map_err(|_| missing())?,
    })
}
