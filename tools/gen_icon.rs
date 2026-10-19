//! Standalone icon generator for the tray: renders the screencapture mark
//! at several sizes and packs them into `assets/icon.ico` (ICO/BI_RGB,
//! 32-bit BGRA, alpha with all-zero masks). std only.
//!
//! Regenerate with:
//!     rustc --edition 2024 tools/gen_icon.rs -o target/gen_icon.exe
//!     .\target\gen_icon.exe
//!
//! Design: an ink rounded-square tile with a light "screen" panel and a
//! red record dot in the top-right, matching a screen-recorder's role.

use std::fs::File;
use std::io::BufWriter;
use std::io::Write as _;
use std::path::Path;

/// Output sizes in the .ico (Explorer picks the best tray match).
const SIZES: [usize; 5] = [64, 48, 32, 24, 16];

/// Supersampling factor; the mark is vector-drawn at 4x then box-filtered.
const SS: usize = 4;

const TILE: Rgba = Rgba { r: 23, g: 27, b: 38, a: 255 };
const SCREEN: Rgba = Rgba { r: 244, g: 246, b: 250, a: 255 };
const DOT: Rgba = Rgba { r: 229, g: 72, b: 77, a: 255 };

#[derive(Clone, Copy)]
struct Rgba {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

/// Straight-alpha RGBA canvas of `size` x `size` pixels.
struct Canvas {
    size: usize,
    pixels: Vec<Rgba>,
}

impl Canvas {
    fn new(size: usize) -> Self {
        Self {
            size,
            pixels: vec![Rgba { r: 0, g: 0, b: 0, a: 0 }; size * size],
        }
    }

    fn blend(&mut self, x: usize, y: usize, color: Rgba, coverage: f32) {
        let coverage = coverage.clamp(0.0, 1.0);
        if coverage <= f32::EPSILON {
            return;
        }
        let slot = &mut self.pixels[y * self.size + x];
        let cover = coverage as f32;
        slot.r = mix(slot.r, color.r, color.a, cover);
        slot.g = mix(slot.g, color.g, color.a, cover);
        slot.b = mix(slot.b, color.b, color.a, cover);
        slot.a = over(slot.a, color.a, cover);
    }

    /// Axis-aligned rounded rectangle with a smooth edge.
    fn rounded_rect(&mut self, rect: Rect, radius: f32, color: Rgba) {
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let center = Self::point(x, y);
                let coverage = rounded_coverage(&center, &rect, radius);
                self.blend(x, y, color, coverage);
            }
        }
    }

    /// Antialiased disk.
    fn circle(&mut self, center: (f32, f32), radius: f32, color: Rgba) {
        let radius = radius.max(1.0);
        let x0 = (center.0 - radius).max(0.0).floor() as usize;
        let y0 = (center.1 - radius).max(0.0).floor() as usize;
        let x1 = (center.0 + radius + 1.0).min(self.size as f32) as usize;
        let y1 = (center.1 + radius + 1.0).min(self.size as f32) as usize;
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - center.0;
                let dy = y as f32 + 0.5 - center.1;
                let distance = (dx * dx + dy * dy).sqrt();
                self.blend(x, y, color, radius + 0.5 - distance);
            }
        }
    }

    fn point(x: usize, y: usize) -> (f32, f32) {
        (x as f32 + 0.5, y as f32 + 0.5)
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

fn mix(base: u8, color: u8, alpha: u8, coverage: f32) -> u8 {
    let weight = (alpha as f32 / 255.0) * coverage;
    let out = base as f32 + (color as f32 - base as f32) * weight;
    out.clamp(0.0, 255.0) as u8
}

fn over(base: u8, alpha: u8, coverage: f32) -> u8 {
    let weight = (alpha as f32 / 255.0) * coverage;
    let out = base as f32 * (1.0 - weight) + 255.0 * weight;
    out.clamp(0.0, 255.0) as u8
}

/// Signed distance coverage for a rounded rect (1 inside, 0 outside,
/// smooth 1px falloff).
fn rounded_coverage(point: &(f32, f32), rect: &Rect, radius: f32) -> f32 {
    let cx0 = rect.x0 as f32 + radius;
    let cx1 = rect.x1 as f32 - radius;
    let cy0 = rect.y0 as f32 + radius;
    let cy1 = rect.y1 as f32 - radius;
    let dx = (point.0.max(cx0) - point.0.min(cx1)).abs().max(0.0);
    let dy = (point.1.max(cy0) - point.1.min(cy1)).abs().max(0.0);
    let distance = (dx * dx + dy * dy).sqrt();
    radius + 1.0 - distance
}

/// Draw the mark on an SS x SS canvas dedicated to one output size.
fn draw_mark(size: usize) -> Canvas {
    let span = size * SS;
    let mut canvas = Canvas::new(span);
    let unit = span as f32;
    let tile = Rect {
        x0: 0,
        y0: 0,
        x1: span,
        y1: span,
    };
    canvas.rounded_rect(tile, unit * 0.22, TILE);
    let inset = unit * 0.16;
    let body = Rect {
        x0: inset.round() as usize,
        y0: inset.round() as usize,
        x1: (unit - inset).round() as usize,
        y1: (unit - inset).round() as usize,
    };
    canvas.rounded_rect(body, unit * 0.10, SCREEN);
    canvas.circle((unit * 0.72, unit * 0.26), unit * 0.085, DOT);
    canvas
}

/// Box-average the SS canvas down to one output canvas.
fn downsample(supersampled: &Canvas, size: usize) -> Canvas {
    let step = supersampled.size / size;
    let mut out = Canvas::new(size);
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..step {
                for sx in 0..step {
                    let at = supersampled.pixels[(y * step + sy) * supersampled.size
                        + (x * step + sx)];
                    r += u32::from(at.r);
                    g += u32::from(at.g);
                    b += u32::from(at.b);
                    a += u32::from(at.a);
                }
            }
            let cells = u32::try_from(step * step).unwrap_or(1);
            out.pixels[y * size + x] = Rgba {
                r: (r / cells) as u8,
                g: (g / cells) as u8,
                b: (b / cells) as u8,
                a: (a / cells) as u8,
            };
        }
    }
    out
}

/// Pack the canvas as a 32bpp bottom-up image plus an all-zero mask.
fn pack_bitmap(canvas: &Canvas) -> Vec<u8> {
    let size = canvas.size;
    let mask_bytes_per_row = size.div_ceil(32) * 4;
    let bytes = Vec::with_capacity(40 + size * size * 4 + mask_bytes_per_row * size);
    let mut sink = bytes;
    sink.extend_from_slice(&40u32.to_le_bytes()); // header size
    sink.extend_from_slice(&u32::try_from(size).unwrap_or(0).to_le_bytes());
    sink.extend_from_slice(&(2 * u32::try_from(size).unwrap_or(0)).to_le_bytes()); // doubled height
    sink.extend_from_slice(&1u16.to_le_bytes()); // planes
    sink.extend_from_slice(&32u16.to_le_bytes()); // bits
    sink.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB, no compression
    sink.extend_from_slice(&0u32.to_le_bytes()); // size image
    sink.extend_from_slice(&0u32.to_le_bytes());
    sink.extend_from_slice(&0u32.to_le_bytes());
    sink.extend_from_slice(&0u32.to_le_bytes());
    sink.extend_from_slice(&0u32.to_le_bytes());
    for row in (0..size).rev() {
        for column in 0..size {
            let at = canvas.pixels[row * size + column];
            sink.push(at.b);
            sink.push(at.g);
            sink.push(at.r);
            sink.push(at.a);
        }
    }
    sink.resize(sink.len() + mask_bytes_per_row * size, 0);
    sink
}

/// Full .ico: directory plus stacked images.
fn pack_ico(images: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut file: Vec<u8> = Vec::with_capacity(6 + 16 * images.len());
    // ICONDIR: reserved, type (1 = icon), entry count.
    file.extend_from_slice(&0u16.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&u16::try_from(images.len()).unwrap_or(0).to_le_bytes());
    let mut offset: usize = 6 + 16 * images.len();
    for (size, bytes) in images {
        let dimension = u8::try_from(*size).unwrap_or(255);
        file.push(dimension);
        file.push(dimension);
        file.push(0); // palette
        file.push(0); // reserved
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(&u32::try_from(offset).unwrap_or(0).to_le_bytes());
        offset += bytes.len();
    }
    for (_, bytes) in images {
        file.extend_from_slice(bytes);
    }
    file
}

fn main() {
    let rendered: Vec<Canvas> = SIZES.iter().map(|size| downsample(&draw_mark(*size), *size)).collect();
    let images: Vec<(usize, Vec<u8>)> =
        SIZES.iter().copied().zip(rendered.iter().map(pack_bitmap)).collect();
    let packed = pack_ico(&images);
    let destination = Path::new("assets/icon.ico");
    if let Some(parent) = destination.parent() {
        _ = std::fs::create_dir_all(parent);
    }
    let written = File::create(destination).and_then(|file| {
        let mut buffered = BufWriter::new(file);
        buffered.write_all(&packed)
    });
    match written {
        Ok(()) => println!("wrote {} ({} bytes)", destination.display(), packed.len()),
        Err(error) => {
            eprintln!("error: cannot write icon: {error}");
            std::process::exit(1);
        }
    }
}
