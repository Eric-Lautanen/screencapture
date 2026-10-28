//! Cursor compositing for duplication frames: the desktop image carries no
//! pointer, so when the plan wants one we draw the shape the tracker handed
//! us onto the BGRA frame bytes here, exactly following the semantics the
//! `DXGI_OUTDUPL_POINTER_SHAPE_TYPE` documentation states (cross-checked
//! against Microsoft's desktop duplication sample, `OutputManager.cpp`):
//!
//! - `MONOCHROME`: a 1 bpp `AND` mask occupies the top half of the bitmap
//!   and an equal-sized 1 bpp `XOR` mask sits below it, bits MSB-first.
//!   The pixel becomes `(screen & and) ^ xor` — unchanged, black, white,
//!   or inverted; alpha always ends opaque.
//! - `COLOR`: 32 bpp ARGB blended with straight alpha; the result reads
//!   opaque.
//! - `MASKED_COLOR`: the alpha byte is the mask (only `0` and `0xFF` are
//!   legal); mask `0` replaces the screen pixel with the RGB value, mask
//!   `0xFF` XORs them.
//!
//! The pointer image is drawn with its top-left at the pose the API
//! reports; parts outside the frame simply do not exist.

use crate::winapi::{CURSOR_SHOWING, CURSOR_SUPPRESSED, CursorInfo, GetCursorInfo, Point, Rect};

/// Live desktop cursor coordinates when the shell is currently showing the
/// pointer; `None` when hidden or suppressed.
#[must_use]
pub(super) fn desktop_cursor_position() -> Option<(i32, i32)> {
    let mut state = CursorInfo {
        size: u32::try_from(std::mem::size_of::<CursorInfo>()).unwrap_or(0),
        flags: 0,
        cursor: std::ptr::null_mut(),
        point: Point::default(),
    };
    // SAFETY: `state` is a local initialized struct with `size` populated.
    let shown = unsafe { GetCursorInfo(&raw mut state) } != 0
        && state.flags & CURSOR_SHOWING != 0
        && state.flags & CURSOR_SUPPRESSED == 0;
    shown.then_some((state.point.x, state.point.y))
}

/// Where the pointer is this tick, in the frame's own coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    /// Top-left corner of the bitmap inside the frame.
    pub left: i32,
    /// Top-left corner, vertical arm.
    pub top: i32,
    /// Whether any pointer should be drawn at all.
    pub visible: bool,
}

/// One fetched pointer bitmap plus its describing metadata.
pub struct Shape {
    /// Bitmap bytes, laid out by the driver's own row pitch.
    pub bytes: Vec<u8>,
    /// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_*`.
    pub kind: u32,
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels (AND+XOR rows for the monochrome pair).
    pub height: u32,
    /// Bytes per bitmap row.
    pub pitch: u32,
}

/// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME`.
pub const MONOCHROME: u32 = 1;
/// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR`.
pub const COLOR: u32 = 2;
/// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR`.
pub const MASKED_COLOR: u32 = 4;

impl Shape {
    /// Whether the stored bitmap plausibly matches its metadata: the kind
    /// is known, every declared row exists, and rows hold enough bytes.
    fn coherent(&self) -> bool {
        let rows = u64::from(self.height);
        let pitch = u64::from(self.pitch);
        let width = u64::from(self.width);
        let fits = pitch * rows == self.bytes.len() as u64 && pitch > 0;
        match self.kind {
            MONOCHROME => fits && self.height.is_multiple_of(2) && pitch >= width.div_ceil(8),
            COLOR | MASKED_COLOR => fits && pitch >= width * 4,
            _ => false,
        }
    }
}

/// Draw the pointer onto the frame when it intersects it.
///
/// Answers the frame rectangle that was composited, or `None` when the pose
/// hides the pointer, the shape is malformed, or the shape sits entirely
/// outside the frame — every case keeps the frame a valid image, just
/// possibly without the pointer drawn. The rectangle lets the caller undo
/// its own composite when the compositor has nothing new to offer.
pub fn apply(frame: &mut [u8], width: u32, height: u32, pose: Pose, shape: &Shape) -> Option<Rect> {
    if !pose.visible || width == 0 || height == 0 || !shape.coherent() {
        return None;
    }
    // Clipping window: the frame pixels this tick actually draws, and where
    // inside the bitmap they come from.
    let first_x = pose.left.max(0);
    let first_y = pose.top.max(0);
    let last_x = (i64::from(pose.left) + i64::from(shape.width)).min(i64::from(width));
    let last_y = (i64::from(pose.top) + i64::from(shape.height)).min(i64::from(height));
    if i64::from(first_x) >= last_x || i64::from(first_y) >= last_y {
        return None;
    }
    let skip_x = u32::try_from(first_x - pose.left).unwrap_or(0);
    let skip_y = u32::try_from(first_y - pose.top).unwrap_or(0);
    let span_x = u32::try_from(last_x - i64::from(first_x)).unwrap_or(0);
    let span_y = u32::try_from(last_y - i64::from(first_y)).unwrap_or(0);
    let stride = width as usize * 4;
    // Non-negative by construction; widening needs no sign fix-up.
    let origin_x = usize::try_from(first_x).unwrap_or(0);
    let origin_y = usize::try_from(first_y).unwrap_or(0);

    for line in 0..span_y {
        for column in 0..span_x {
            let index = (origin_y + line as usize) * stride + (origin_x + column as usize) * 4;
            let pixel = &mut frame[index..index + 4];
            let screen = [pixel[0], pixel[1], pixel[2], pixel[3]];
            if let Some(painted) = paint(shape, skip_x + column, skip_y + line, screen) {
                pixel.copy_from_slice(&painted);
            }
        }
    }
    Some(Rect {
        left: first_x,
        top: first_y,
        right: i32::try_from(last_x).unwrap_or(i32::MAX),
        bottom: i32::try_from(last_y).unwrap_or(i32::MAX),
    })
}

/// One pixel's verdict: the BGRA quad to paint, or `None` to leave the
/// screen pixel alone (`AND`-transparent monochrome bits).
fn paint(shape: &Shape, shape_x: u32, shape_y: u32, screen: [u8; 4]) -> Option<[u8; 4]> {
    match shape.kind {
        MONOCHROME => {
            let and = mono_bit(shape, shape_x, shape_y);
            let xor = mono_bit(shape, shape_x, shape_y + shape.height / 2);
            Some(opaque(screen, and, xor))
        }
        COLOR => Some(blend(shape_pixel(shape, shape_x, shape_y), screen)),
        MASKED_COLOR => {
            let source = shape_pixel(shape, shape_x, shape_y);
            if source[3] == 0xFF {
                // XOR the RGB trio over the screen, alpha stays opaque.
                Some([
                    screen[0] ^ source[0],
                    screen[1] ^ source[1],
                    screen[2] ^ source[2],
                    0xFF,
                ])
            } else {
                let painted = source;
                Some([painted[0], painted[1], painted[2], 0xFF])
            }
        }
        _ => None,
    }
}

/// One 1 bpp mask bit, MSB-first across the driver's row pitch.
fn mono_bit(shape: &Shape, shape_x: u32, shape_row: u32) -> bool {
    let index = shape_row as usize * shape.pitch as usize + shape_x as usize / 8;
    let mask = 0x80_u8 >> (shape_x % 8);
    shape.bytes.get(index).is_some_and(|byte| byte & mask != 0)
}

/// The BGRA quad at one bitmap pixel (zeros past the bitmap edge).
fn shape_pixel(shape: &Shape, shape_x: u32, shape_y: u32) -> [u8; 4] {
    let mut pixel = [0, 0, 0, 0];
    let base = shape_y as usize * shape.pitch as usize + shape_x as usize * 4;
    if let Some(slice) = shape.bytes.get(base..base + 4) {
        pixel.copy_from_slice(slice);
    }
    pixel
}

/// `(screen & and) ^ xor`, byte-wise, with the alpha forced opaque: `and`
/// keeps the screen pixel, `and == false` is a black base, and `xor`
/// inverts the RGB trio.
fn opaque(screen: [u8; 4], and: bool, xor: bool) -> [u8; 4] {
    let base = [
        if and { screen[0] } else { 0 },
        if and { screen[1] } else { 0 },
        if and { screen[2] } else { 0 },
        0xFF,
    ];
    if xor {
        [base[0] ^ 0xFF, base[1] ^ 0xFF, base[2] ^ 0xFF, 0xFF]
    } else {
        base
    }
}

/// Straight-alpha blend of the shape pixel over the screen (the semantics
/// the sample's `SRC_ALPHA` / `INV_SRC_ALPHA` blend state expresses).
fn blend(source: [u8; 4], screen: [u8; 4]) -> [u8; 4] {
    let weight = u32::from(source[3]);
    let mix = |intent: u8, screen: u8| {
        // The weighted average stays inside 0..=255 by construction.
        u8::try_from((u32::from(intent) * weight + u32::from(screen) * (255 - weight) + 127) / 255)
            .unwrap_or(u8::MAX)
    };
    [
        mix(source[0], screen[0]),
        mix(source[1], screen[1]),
        mix(source[2], screen[2]),
        0xFF,
    ]
}

#[cfg(test)]
mod tests {
    use super::{COLOR, MASKED_COLOR, MONOCHROME, Pose, Shape, apply};

    fn flat_screen() -> Vec<u8> {
        // 4x4 BGRA frame, mid gray, opaque.
        let mut frame = Vec::new();
        for _ in 0..16 {
            frame.extend_from_slice(&[0x40, 0x40, 0x40, 0xFF]);
        }
        frame
    }

    /// Opaque color replaces the pixel; half-alpha color blends over it.
    #[test]
    fn color_with_alpha_blends() {
        let mut frame = flat_screen();
        // Two-pixel pointer row: opaque red, then 50%-alpha red.
        let shape = Shape {
            kind: COLOR,
            bytes: vec![
                0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0x80, 0, 0, 0xFF, 0, 0, 0, 0, 0,
            ],
            width: 2,
            height: 2,
            pitch: 8,
        };
        let pose = Pose {
            left: 1,
            top: 1,
            visible: true,
        };
        assert!(apply(&mut frame, 4, 4, pose, &shape).is_some());
        let pixel = |x: usize, y: usize| &frame[y * 16 + x * 4..y * 16 + x * 4 + 4];
        assert_eq!(pixel(1, 1), &[0, 0, 0xFF, 0xFF]);
        // r = (255*128 + 64*127 + 127) / 255 = 160.
        assert_eq!(pixel(2, 1)[2], 160);
        // Transparent second row leaves the screen pixel untouched.
        assert_eq!(pixel(1, 2), &[0x40, 0x40, 0x40, 0xFF]);
        assert_eq!(pixel(3, 3), &[0x40, 0x40, 0x40, 0xFF]);
    }

    /// Monochrome AND/XOR: keep, invert, black, white.
    #[test]
    fn monochrome_masks_paint_classically() {
        // 3x4 pointer: two AND rows on top, two XOR rows below. The AND
        // mask lights column 1, the XOR mask lights columns 1 and 2, so
        // the truth table is exercised: column 1 is AND=1 XOR=1 (screen
        // kept, then inverted), column 2 is AND=0 XOR=1 (screen discarded,
        // black base inverted to white), column 0 is AND=0 XOR=0 (opaque
        // black).
        let shape = Shape {
            kind: MONOCHROME,
            bytes: vec![0b0100_0000, 0b0100_0000, 0b0110_0000, 0b0110_0000],
            width: 3,
            height: 4,
            pitch: 1,
        };
        let mut dark = vec![0_u8; 64];
        let pose = Pose {
            left: 0,
            top: 0,
            visible: true,
        };
        assert!(apply(&mut dark, 4, 4, pose, &shape).is_some());
        let pixel = |x: usize, y: usize| &dark[y * 16 + x * 4..y * 16 + x * 4 + 4];
        // Black screen: (1,0) AND=1 XOR=1 keeps then inverts — white.
        assert_eq!(pixel(1, 0), &[0xFF, 0xFF, 0xFF, 0xFF]);
        // (2,0) AND=0 XOR=1 discards the screen (black base) and inverts
        // — white, on any screen.
        assert_eq!(pixel(2, 0), &[0xFF, 0xFF, 0xFF, 0xFF]);
        // (0,0) AND=0 XOR=0 paints opaque black regardless of the screen.
        assert_eq!(pixel(0, 0), &[0, 0, 0, 0xFF]);
        // On a white screen: column 1 inverts white into black, column 2
        // still paints white (the screen was discarded), column 0 black.
        let mut bright = vec![0xFF_u8; 64];
        assert!(apply(&mut bright, 4, 4, pose, &shape).is_some());
        assert_eq!(&bright[4..8], &[0, 0, 0, 0xFF]);
        assert_eq!(&bright[8..12], &[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(&bright[0..4], &[0, 0, 0, 0xFF]);
    }

    /// Masked color: mask 0 replaces, mask 0xFF XORs the RGB trio over the
    /// screen.
    #[test]
    fn masked_color_replaces_and_inverts() {
        let mut frame = vec![0x10_u8; 4 * 4];
        // Replace (0,0) with blue; invert (1,0) through 0x0F; replace (2,0)
        // with the black color; mask 0.
        let shape = Shape {
            kind: MASKED_COLOR,
            bytes: vec![0xFF, 0, 0, 0, 0, 0, 0x0F, 0xFF, 0, 0, 0, 0],
            width: 3,
            height: 1,
            pitch: 12,
        };
        let pose = Pose {
            left: 0,
            top: 0,
            visible: true,
        };
        assert!(apply(&mut frame, 4, 1, pose, &shape).is_some());
        let pixel = |x: usize| &frame[x * 4..x * 4 + 4];
        assert_eq!(pixel(0), &[0xFF, 0, 0, 0xFF]);
        assert_eq!(pixel(1), &[0x10, 0x10, 0x1F, 0xFF]);
        assert_eq!(pixel(2), &[0, 0, 0, 0xFF]);
    }

    /// A bitmap hanging off the edge paints its visible part only; one
    /// fully outside paints nothing.
    #[test]
    fn clipping_keeps_frames_boring() {
        // 2x2 replace-by-blue pointer shape.
        let mut bytes = vec![0_u8; 16];
        for pair in bytes.chunks_mut(4) {
            pair.copy_from_slice(&[9, 0, 0, 0]);
        }
        let shape = Shape {
            kind: MASKED_COLOR,
            bytes,
            width: 2,
            height: 2,
            pitch: 8,
        };
        let mut frame = vec![0_u8; 64];
        let edge = Pose {
            left: 3,
            top: 0,
            visible: true,
        };
        let area = apply(&mut frame, 4, 4, edge, &shape).expect("the visible part paints");
        // The answer is exactly the clipped rectangle that was composited.
        assert_eq!((area.left, area.top, area.right, area.bottom), (3, 0, 4, 2),);
        // Only frame column 3 got the blue replacement, on both rows.
        assert_eq!(&frame[12..16], &[9, 0, 0, 0xFF]);
        assert_eq!(&frame[28..32], &[9, 0, 0, 0xFF]);
        // Untouched neighbors stay put.
        assert_eq!(&frame[16..20], &[0, 0, 0, 0]);
        let mut clean = vec![0_u8; 64];
        let outside = Pose {
            left: 8,
            top: 0,
            visible: true,
        };
        assert!(apply(&mut clean, 4, 4, outside, &shape).is_none());
        assert_eq!(clean, vec![0; 64]);
    }

    /// Hidden poses and malformed shapes always decline to paint.
    #[test]
    fn hidden_and_broken_shapes_decline() {
        let mut frame = vec![0_u8; 16];
        let blank = Pose {
            left: 0,
            top: 0,
            visible: false,
        };
        let shape = Shape {
            kind: 99,
            bytes: vec![0; 8],
            width: 2,
            height: 2,
            pitch: 8,
        };
        assert!(apply(&mut frame, 4, 4, blank, &shape).is_none());
        let shown = Pose {
            left: 0,
            top: 0,
            visible: true,
        };
        assert!(apply(&mut frame, 4, 4, shown, &shape).is_none());
    }
}
