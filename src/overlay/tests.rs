//! Unit tests for the overlay geometry helpers (selection dot outline,
//! recording dashed frame, Stop chip placement, and crosshair arms).

use super::{
    SELECTION_DOT,
    crosshair::crosshair_segments,
    frame_segments, selection_dots,
    stop::{STOP_HEIGHT, STOP_WIDTH, stop_rect},
};
use crate::capture::Region;

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
fn stop_button_centers_on_the_top_edge() {
    let rect = stop_rect(Region {
        x: 100,
        y: 100,
        width: 800,
        height: 600,
    });
    assert_eq!(rect.right - rect.left, STOP_WIDTH);
    assert_eq!(rect.bottom - rect.top, STOP_HEIGHT);
    assert_eq!(rect.left, 100 + (800 - STOP_WIDTH) / 2);
    // Half above the dashed line, half below: centered on it.
    assert_eq!(rect.top, 100 - STOP_HEIGHT / 2);
    assert_eq!(rect.bottom, 100 + STOP_HEIGHT / 2);
}

#[test]
fn stop_button_never_leaves_the_screen() {
    let top = stop_rect(Region {
        x: 0,
        y: 0,
        width: 400,
        height: 300,
    });
    assert_eq!(top.top, 0);
    assert_eq!(top.left, (400 - STOP_WIDTH) / 2);
    // A region narrower than the button keeps its left edge.
    let narrow = stop_rect(Region {
        x: 10,
        y: 50,
        width: 40,
        height: 40,
    });
    assert_eq!(narrow.left, 10);
    assert_eq!(narrow.top, 50 - STOP_HEIGHT / 2);
}

#[test]
fn segments_stay_inside_the_client_area() {
    for (width, height) in [(320, 240), (1920, 1080), (1, 1), (5, 3)] {
        for (left, top, right, bottom) in frame_segments(width, height) {
            assert!(left >= 0 && top >= 0, "origin inside {width}x{height}");
            assert!(
                right <= width && bottom <= height,
                "edge inside {width}x{height}"
            );
            assert!(right > left && bottom > top, "non-empty segment");
        }
    }
}

#[test]
fn every_edge_is_outlined() {
    let segments = frame_segments(400, 300);
    assert!(segments.iter().any(|segment| segment.1 == 0));
    assert!(segments.iter().any(|segment| segment.3 == 300));
    assert!(segments.iter().any(|segment| segment.0 == 0));
    assert!(segments.iter().any(|segment| segment.2 == 400));
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
