//! Unit tests for the overlay geometry helpers (selection dot outline,
//! recording frame, and crosshair arms). The control bar's own layout is
//! pinned in `bar::tests`, beside the code that draws it.

use super::{SELECTION_DOT, Tone, crosshair::crosshair_segments, frame_rects, selection_dots};

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
        for (rect, _) in frame_rects(width, height) {
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

#[test]
fn every_edge_is_outlined() {
    let rects = frame_rects(400, 300);
    assert!(rects.iter().any(|(rect, _)| rect.top == 0));
    assert!(rects.iter().any(|(rect, _)| rect.bottom == 300));
    assert!(rects.iter().any(|(rect, _)| rect.left == 0));
    assert!(rects.iter().any(|(rect, _)| rect.right == 400));
    // The hairline is two-tone all the way round, so it reads on pale and
    // dark content alike.
    assert!(rects.iter().any(|(_, tone)| *tone == Tone::Shade));
    assert!(rects.iter().any(|(_, tone)| *tone == Tone::Light));
}

#[test]
fn every_corner_carries_an_inset_bracket() {
    let (width, height) = (400, 300);
    let rects = frame_rects(width, height);
    // A probe box just inside each corner lands on the bracket's elbow. The
    // frame's own corner pixel stays hairline: that inset is what keeps the
    // mark reading as framing rather than a thicker border.
    let probe = 10;
    for (x, y, across, down) in [
        (0, 0, 1, 1),
        (width, 0, -1, 1),
        (0, height, 1, -1),
        (width, height, -1, -1),
    ] {
        let (left, right) = if across > 0 {
            (x, x + probe)
        } else {
            (x - probe, x)
        };
        let (top, bottom) = if down > 0 {
            (y, y + probe)
        } else {
            (y - probe, y)
        };
        assert!(
            rects.iter().any(|(rect, tone)| {
                *tone == Tone::Accent
                    && rect.left < right
                    && rect.right > left
                    && rect.top < bottom
                    && rect.bottom > top
            }),
            "a bracket inside {x},{y}"
        );
    }
    assert!(
        !rects.iter().any(|(rect, tone)| *tone == Tone::Accent
            && rect.left <= 0
            && rect.right > 0
            && rect.top <= 0
            && rect.bottom > 0),
        "the accent stands clear of the corner"
    );
}

#[test]
fn brackets_scale_with_the_frame_but_stay_bounded() {
    use super::{BRACKET_MAX, BRACKET_MIN};
    // The top-left bracket's horizontal leg is the widest accent rect in the
    // corner, so its width is the leg length.
    let leg = |width: i32, height: i32| {
        frame_rects(width, height)
            .into_iter()
            .filter(|(rect, tone)| *tone == Tone::Accent && rect.left < 10 && rect.top < 10)
            .map(|(rect, _)| rect.right - rect.left)
            .max()
            .unwrap_or(0)
    };
    assert_eq!(leg(3840, 2160), BRACKET_MAX, "a 4K take gets the full leg");
    assert!(
        (BRACKET_MIN..=BRACKET_MAX).contains(&leg(200, 150)),
        "a small window keeps a readable leg"
    );
    assert!(
        leg(40, 30) <= 15,
        "a tiny frame keeps its bracket inside the frame"
    );
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

#[test]
fn crosshair_arms_leave_the_pointer_clear() {
    let size = 33;
    let gap = 5;
    let arms = crosshair_segments(size, gap, 3);
    assert_eq!(arms.len(), 4);
    let mid = size / 2;
    for arm in &arms {
        assert!(arm.left >= 0 && arm.top >= 0);
        assert!(arm.right <= size && arm.bottom <= size);
        assert!(arm.right > arm.left && arm.bottom > arm.top);
        assert!(
            arm.right <= mid - gap + 1
                || arm.left >= mid + gap
                || arm.bottom <= mid - gap + 1
                || arm.top >= mid + gap,
            "crosshair arm covers the pointer gap"
        );
    }
}
