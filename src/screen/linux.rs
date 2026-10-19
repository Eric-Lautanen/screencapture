//! Linux display geometry: the default screen of the `DISPLAY` root, in
//! pixels, one display per X screen (the server composites a root that
//! spans every monitor on Xinerama-configured stacks).

use crate::screen::Display;
use crate::xlib::Api;

/// The default root of the default screen.
///
/// # Errors
/// `libX11` is missing or no X server answers.
pub fn list() -> Vec<Display> {
    let Ok(api) = Api::load() else {
        return Vec::new();
    };
    let Ok(display) = api.open() else {
        return Vec::new();
    };
    let (width, height) = api.main_screen(display);
    api.close(display);
    vec![Display {
        x: 0,
        y: 0,
        width,
        height,
    }]
}
