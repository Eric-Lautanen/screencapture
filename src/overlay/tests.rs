//! Unit tests for the overlay geometry helpers (selection dot outline and
//! recording frame). The control bar's own layout is pinned in
//! `bar::tests`, beside the code that draws it; the pointer a pick shows is
//! not drawn here at all, and is pinned by the live-session tests in
//! `window_picker::tests`.

use super::{SELECTION_DOT, Tone, frame_rects, percent_for, selection_dots};

/// The pill is aimed at, so its physical size matters more than the logical
/// one Windows' scale keeps constant. These are real panels: the reference
/// 14-inch 1920x1200 (302 mm), a 27-inch 4K (598 mm), and a 15.6-inch
/// 1366x768 (345 mm), where 96 pixels is already the 24 mm the bar was drawn
/// for.
#[test]
fn a_dense_panel_gets_a_pill_big_enough_to_hit() {
    assert_eq!(percent_for(302, 1920), Some(159));
    assert_eq!(percent_for(598, 3840), Some(161));
    assert_eq!(percent_for(345, 1366), Some(100));
    // A 13-inch 2560x1600 panel: 15 mm at 100% would be too small.
    assert!(percent_for(286, 2560).is_some_and(|percent| percent > 200));
}

#[test]
fn a_driver_that_reports_nonsense_is_ignored() {
    assert_eq!(percent_for(0, 1920), None);
    assert_eq!(percent_for(302, 0), None);
    // A desktop-sized millimetre answer is 10 DPI; a made-up micro panel is
    // thousands. Either way the DPI scale is the only evidence left.
    assert_eq!(percent_for(5000, 1920), None);
    assert_eq!(percent_for(20, 3840), None);
    assert_eq!(percent_for(-302, 1920), None);
}

#[test]
fn selection_outline_is_a_bounded_two_tone_dot_line() {
    for (width, height) in [(320, 240), (1920, 1080), (1, 1), (5, 3)] {
        let dots = selection_dots(width, height);
        assert!(!dots.is_empty(), "{width}x{height} draws dots");
        // A degenerate frame is narrower than the second dot's offset,
        // so only the white half fits; bigger frames carry both tones.
        if width > SELECTION_DOT + 1 && height > SELECTION_DOT + 1 {
            assert!(
                dots.iter().any(|(_, white)| *white) && dots.iter().any(|(_, white)| !*white),
                "{width}x{height} draws both tones"
            );
        }
        for (dot, _) in &dots {
            assert!(dot.left >= 0 && dot.top >= 0);
            assert!(dot.right <= width && dot.bottom <= height);
            assert!(dot.right > dot.left && dot.bottom > dot.top);
        }
    }
    // Every edge is covered, so the line closes all the way round.
    let dots = selection_dots(320, 240);
    assert!(dots.iter().any(|(dot, _)| dot.top == 0));
    assert!(dots.iter().any(|(dot, _)| dot.bottom == 240));
    assert!(dots.iter().any(|(dot, _)| dot.left == 0));
    assert!(dots.iter().any(|(dot, _)| dot.right == 320));
}

#[test]
fn frame_rects_stay_inside_the_client_area() {
    for (width, height) in [(320, 240), (1920, 1080), (1, 1), (5, 3), (400, 12)] {
        for (rect, _) in frame_rects(width, height, 100) {
            assert!(
                rect.left >= 0 && rect.top >= 0,
                "origin inside {width}x{height}"
            );
            assert!(
                rect.right <= width && rect.bottom <= height,
                "edge inside {width}x{height}"
            );
            assert!(
                rect.right > rect.left && rect.bottom > rect.top,
                "non-empty"
            );
        }
    }
}

/// A 200% display draws the same frame twice as thick instead of thinning
/// to a hairline.
#[test]
fn the_frame_keeps_its_weight_on_a_scaled_display() {
    let plain = frame_rects(400, 300, 100);
    let scaled = frame_rects(400, 300, 200);
    let thickness = |rects: &[(super::Rect, Tone)]| {
        rects
            .iter()
            .filter(|(_, tone)| *tone == Tone::Light)
            .map(|(rect, _)| rect.bottom - rect.top)
            .max()
            .unwrap_or(0)
    };
    assert!(
        thickness(&scaled) >= thickness(&plain),
        "the light line never thins on a bigger display"
    );
    for (rect, _) in &scaled {
        assert!(rect.left >= 0 && rect.top >= 0);
        assert!(rect.right <= 400 && rect.bottom <= 300);
        assert!(rect.right > rect.left && rect.bottom > rect.top);
    }
    // A hairline at 200% is two device pixels, so the frame reads the same
    // size on screen as it does at 100%.
    let hairline = scaled
        .iter()
        .find(|(rect, tone)| *tone == Tone::Light && rect.top == 0 && rect.left == 0)
        .map(|(rect, _)| rect.bottom);
    assert_eq!(hairline, Some(2));
}

#[test]
fn every_edge_is_outlined() {
    let rects = frame_rects(400, 300, 100);
    assert!(rects.iter().any(|(rect, _)| rect.top == 0));
    assert!(rects.iter().any(|(rect, _)| rect.bottom == 300));
    assert!(rects.iter().any(|(rect, _)| rect.left == 0));
    assert!(rects.iter().any(|(rect, _)| rect.right == 400));
    // The hairline is two-tone all the way round, so it reads on pale and
    // dark content alike.
    assert!(rects.iter().any(|(_, tone)| *tone == Tone::Shade));
    assert!(rects.iter().any(|(_, tone)| *tone == Tone::Light));
}

/// The frame is one line, not a line plus corner marks: four light rects on
/// the edge and four shade ones inside it, and nothing else.
#[test]
fn the_frame_is_a_single_closed_line() {
    let rects = frame_rects(400, 300, 100);
    assert_eq!(rects.len(), 8, "one closed two-tone ring");
    assert_eq!(
        rects
            .iter()
            .filter(|(_, tone)| *tone == Tone::Shade)
            .count(),
        4
    );
    assert_eq!(
        rects
            .iter()
            .filter(|(_, tone)| *tone == Tone::Light)
            .count(),
        4
    );
}

/// The line the eye reads has to land on the app's own boundary: a dark
/// strip between the edge and the light line reads as a gutter, and the
/// frame then looks inset from the window instead of drawn on it.
#[test]
fn the_light_line_sits_on_the_app_edge() {
    let (width, height) = (400, 300);
    let rects = frame_rects(width, height, 100);
    let light_at = |x: i32, y: i32| {
        rects.iter().any(|(rect, tone)| {
            *tone == Tone::Light
                && rect.left <= x
                && x < rect.right
                && rect.top <= y
                && y < rect.bottom
        })
    };
    for x in [0, 1, width / 2, width - 2, width - 1] {
        assert!(light_at(x, 0), "the top edge at {x} is light");
        assert!(light_at(x, height - 1), "the bottom edge at {x} is light");
    }
    for y in [0, 1, height / 2, height - 2, height - 1] {
        assert!(light_at(0, y), "the left edge at {y} is light");
        assert!(light_at(width - 1, y), "the right edge at {y} is light");
    }
    // The shade line is the inner one, so it never covers the boundary.
    for (rect, tone) in &rects {
        if *tone == Tone::Shade {
            assert!(rect.left > 0 && rect.top > 0, "inset on the leading edges");
            assert!(
                rect.right < width && rect.bottom < height,
                "inset on the trailing edges"
            );
        }
    }
}

/// The corners are what the frame used to get wrong: each inner arm ran out
/// to the outer edge on its own axis, so the four of them overshot the
/// corner and the line read as four separate strokes. Both arms of a corner
/// now stop at the same inset, so they cross in one square.
#[test]
fn the_inner_line_meets_flush_at_each_corner() {
    let (width, height) = (400, 300);
    let rects = frame_rects(width, height, 100);
    let arms_at_corner = |x: i32, y: i32| {
        rects
            .iter()
            .filter(|(rect, tone)| {
                *tone == Tone::Shade
                    && rect.left <= x
                    && x < rect.right
                    && rect.top <= y
                    && y < rect.bottom
            })
            .count()
    };
    for (x, y) in [
        (1, 1),
        (width - 2, 1),
        (1, height - 2),
        (width - 2, height - 2),
    ] {
        assert_eq!(arms_at_corner(x, y), 2, "both arms meet at {x},{y}");
    }
}

#[test]
fn the_breath_eases_into_its_turns() {
    use super::{BREATH_MS, breath};
    use std::time::Duration;
    let at = |ms: u64| i32::from(breath(Duration::from_millis(ms)));
    // The opacity barely moves either side of a turn and moves fastest
    // halfway through each ramp, which is what makes it settle rather than
    // tick.
    let turn = (at(200) - at(0)).abs();
    let middle = (at(BREATH_MS / 4 + 100) - at(BREATH_MS / 4 - 100)).abs();
    assert!(
        turn < middle,
        "the turn settles ({turn}) faster than the middle moves ({middle})"
    );
}

#[test]
fn the_breath_stays_quiet_and_comes_back() {
    use super::{BREATH_HIGH, BREATH_LOW, BREATH_MS, breath};
    use std::time::Duration;
    assert_eq!(breath(Duration::ZERO), BREATH_LOW, "a take starts calm");
    assert_eq!(
        breath(Duration::from_millis(BREATH_MS / 2)),
        BREATH_HIGH,
        "and brightens as it settles in"
    );
    for step in 0..40 {
        let alpha = breath(Duration::from_millis(
            BREATH_MS * u64::try_from(step).unwrap_or(0) / 20,
        ));
        assert!(
            (BREATH_LOW..=BREATH_HIGH).contains(&alpha),
            "{alpha} stays inside the readable band"
        );
    }
}
