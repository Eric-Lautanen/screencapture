use super::{BUTTON, GAP, HEIGHT, Layout, PADDING, Role, scaled, slot};
use crate::capture::Region;

fn region(x: i32, y: i32, width: u32, height: u32) -> Region {
    Region {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn the_bar_centers_on_the_top_edge() {
    let layout = Layout::new(1920, 100);
    let rect = layout.rect(region(100, 100, 800, 600));
    assert_eq!(rect.right - rect.left, layout.width);
    assert_eq!(rect.bottom - rect.top, HEIGHT);
    assert_eq!(rect.left, 100 + (800 - layout.width) / 2);
    assert_eq!(rect.top, 100 - HEIGHT / 2);
}

#[test]
fn the_bar_never_leaves_the_screen() {
    let layout = Layout::new(1920, 100);
    let top = layout.rect(region(0, 0, 400, 300));
    assert_eq!(top.top, 0, "a bar at the top edge sits inside the region");
    let narrow = layout.rect(region(10, 50, 20, 20));
    assert_eq!(narrow.left, 10);
    assert_eq!(narrow.top, 50 - HEIGHT / 2);
}

#[test]
fn audio_buttons_sit_level_beside_stop() {
    let layout = Layout::new(1920, 100);
    let stop = layout.stop;
    let microphone = layout.microphone.expect("a microphone button");
    let system = layout.system.expect("a system button");
    assert_eq!(microphone.left, stop.right + GAP);
    assert_eq!(system.left, microphone.right + GAP);
    for button in [stop, microphone, system] {
        assert_eq!(button.right - button.left, BUTTON);
        assert_eq!(button.top, (HEIGHT - BUTTON) / 2);
    }
    assert_eq!(
        layout.width,
        PADDING * 2 + BUTTON * 3 + GAP * 2,
        "the bar wraps its three buttons"
    );
}

#[test]
fn both_audio_buttons_are_always_there() {
    let layout = Layout::new(1920, 100);
    assert!(
        layout.microphone.is_some(),
        "a silent take can ask for a mic"
    );
    assert!(layout.system.is_some());
    assert_eq!(layout.width, PADDING * 2 + BUTTON * 3 + GAP * 2);
}

#[test]
fn a_narrow_region_keeps_the_stop_button_alone() {
    let layout = Layout::new(60, 100);
    assert!(layout.microphone.is_none() && layout.system.is_none());
    assert_eq!(layout.width, PADDING * 2 + BUTTON);
}

#[test]
fn hit_testing_finds_each_button_and_their_gaps() {
    let layout = Layout::new(1920, 100);
    for role in [Role::Stop, Role::Microphone, Role::System] {
        let button = layout.button(role).expect("a button");
        let x = button.left + BUTTON / 2;
        let y = button.top + BUTTON / 2;
        assert_eq!(layout.role_at(x, y), Some(role));
        assert_eq!(layout.role_at(button.left, button.top), Some(role));
        assert_eq!(
            layout.role_at(button.right - 1, button.bottom - 1),
            Some(role)
        );
        assert_eq!(layout.role_at(button.right, y), None, "past the edge");
        assert_eq!(layout.role_at(x, button.bottom), None, "below the button");
    }
    assert_eq!(
        layout.role_at(layout.stop.right + GAP / 2, 10),
        None,
        "the gap between buttons is not a target"
    );
}

/// On a scaled display the bar and its buttons grow together, and a
/// click at the device position of a button still finds that button.
#[test]
fn the_bar_grows_with_the_display_scale() {
    let design = Layout::new(1920, 100);
    for percent in [125, 150, 200] {
        let grown = Layout::new(1920, percent);
        assert_eq!(grown.width, design.width, "the layout stays designed");
        assert_eq!(grown.device_width(), scaled(design.width, percent));
        assert_eq!(grown.device_height(), scaled(HEIGHT, percent));
        for role in [Role::Stop, Role::Microphone, Role::System] {
            let button = grown.device(grown.button(role).expect("a button"));
            assert!(
                button.right - button.left >= BUTTON,
                "{role:?} is at least its designed size at {percent}%"
            );
            let (x, y) = (
                button.left + (button.right - button.left) / 2,
                button.top + (button.bottom - button.top) / 2,
            );
            assert_eq!(grown.role_at(x, y), Some(role), "center at {percent}%");
            assert_eq!(grown.role_at(button.right, y), None, "past the edge");
        }
    }
}

/// A narrow capture keeps the stop button alone, at either scale: the
/// question is whether the whole row fits in device pixels.
#[test]
fn a_narrow_region_drops_the_audio_buttons_at_any_scale() {
    for percent in [100, 150, 200] {
        let narrow = Layout::new(80, percent);
        assert!(
            narrow.microphone.is_none(),
            "80 device pixels at {percent}%"
        );
        let roomy = Layout::new(200, percent);
        assert!(
            roomy.microphone.is_some(),
            "200 device pixels at {percent}%"
        );
    }
}

/// The bar sits on the region's top edge, and its own scale decides how
/// tall that strip is.
#[test]
fn the_bar_straddles_the_top_edge_at_its_own_size() {
    let region = Region {
        x: 100,
        y: 200,
        width: 800,
        height: 600,
    };
    for percent in [100, 150, 200] {
        let layout = Layout::new(800, percent);
        let rect = layout.rect(region);
        assert_eq!(rect.bottom - rect.top, layout.device_height());
        assert_eq!(rect.right - rect.left, layout.device_width());
        assert_eq!(rect.left, 100 + (800 - layout.device_width()) / 2);
        assert_eq!(rect.top, 200 - layout.device_height() / 2);
        // At the very top of the screen the bar sits inside instead.
        let top = layout.rect(Region { y: 0, ..region });
        assert_eq!(top.top, 0);
    }
}

#[test]
fn buttons_never_overlap_the_bar_edges() {
    let layout = Layout::new(1920, 100);
    for index in 0..3 {
        let button = slot(index);
        assert!(button.left >= PADDING);
        assert!(button.right <= layout.width - PADDING);
        assert!(button.top >= 0 && button.bottom <= HEIGHT);
    }
}
