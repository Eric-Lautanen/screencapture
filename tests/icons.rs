//! Shipped icon coverage: the Windows ICO, macOS ICNS, and Linux PNGs must
//! carry the scales each platform asks for. This catches an asset refresh
//! that drops a size before a release is cut.

use std::fs;
use std::path::PathBuf;

fn asset(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

fn word_le(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(
        data.get(at..at + 4)
            .expect("four bytes inside the icon")
            .try_into()
            .expect("a four-byte word"),
    )
}

#[test]
fn windows_icon_covers_every_common_scale() {
    let data = asset("icon.ico");
    assert!(data.len() > 6, "the icon has a directory header");
    assert_eq!(&data[..4], &[0u8, 0, 1, 0]);
    let count = usize::from(u16::from_le_bytes([data[4], data[5]]));
    assert!(count >= 7, "at least seven directory entries");

    let mut widths = Vec::new();
    for index in 0..count {
        let base = 6 + index * 16;
        assert!(base + 16 <= data.len(), "entry {index} fits in the file");
        let width = if data[base] == 0 {
            256
        } else {
            u16::from(data[base])
        };
        let height = if data[base + 1] == 0 {
            256
        } else {
            u16::from(data[base + 1])
        };
        assert_eq!(width, height, "entry {index} is square");
        let bytes = word_le(&data, base + 8);
        let offset = word_le(&data, base + 12);
        let start = usize::try_from(offset).expect("offset fits usize");
        let length = usize::try_from(bytes).expect("length fits usize");
        assert!(length > 0, "entry {index} has an image");
        assert!(
            start
                .checked_add(length)
                .is_some_and(|end| end <= data.len()),
            "entry {index} points inside the file"
        );
        widths.push(width);
    }

    for wanted in [16, 24, 32, 48, 64, 128, 256] {
        assert!(widths.contains(&wanted), "the ICO is missing {wanted}px");
    }
}

#[test]
fn macos_icon_covers_retina_scales() {
    let data = asset("icon.icns");
    assert_eq!(&data[..4], b"icns");
    let declared = usize::try_from(u32::from_be_bytes(
        data.get(4..8)
            .expect("an ICNS length")
            .try_into()
            .expect("a four-byte length"),
    ))
    .expect("length fits usize");
    assert_eq!(declared, data.len(), "the ICNS length is exact");

    let mut kinds = Vec::new();
    let mut offset = 8;
    while offset < data.len() {
        assert!(data.len() - offset >= 8, "a block header fits");
        let kind = data
            .get(offset..offset + 4)
            .expect("a block type inside the file");
        let length = usize::try_from(u32::from_be_bytes(
            data.get(offset + 4..offset + 8)
                .expect("a block length")
                .try_into()
                .expect("a four-byte length"),
        ))
        .expect("length fits usize");
        assert!(length >= 8, "a block carries its own header");
        assert!(
            offset
                .checked_add(length)
                .is_some_and(|end| end <= data.len()),
            "the block stays inside the file"
        );
        kinds.push(kind);
        offset += length;
    }
    assert_eq!(offset, data.len(), "the blocks tile the file");

    for wanted in [
        b"icp4", b"icp5", b"icp6", b"ic07", b"ic08", b"ic09", b"ic10", b"ic11", b"ic12", b"ic13",
        b"ic14",
    ] {
        assert!(
            kinds.iter().any(|kind| *kind == &wanted[..]),
            "the ICNS is missing {:?}",
            String::from_utf8_lossy(&wanted[..])
        );
    }
}

#[test]
fn linux_icons_are_square_rgba_pngs() {
    for size in [16_u32, 24, 32, 48, 64, 128, 256, 512] {
        let data = asset(&format!("linux/icon-{size}.png"));
        assert!(data.len() >= 33, "the {size}px PNG holds a full IHDR");
        assert_eq!(
            &data[..8],
            &[137u8, 80, 78, 71, 13, 10, 26, 10],
            "the {size}px file is a PNG"
        );
        assert_eq!(&data[12..16], b"IHDR", "the {size}px PNG starts with IHDR");
        let width = u32::from_be_bytes(data[16..20].try_into().expect("a width"));
        let height = u32::from_be_bytes(data[20..24].try_into().expect("a height"));
        assert_eq!((width, height), (size, size), "the PNG is square");
        assert_eq!(data[24], 8, "the {size}px PNG is 8-bit");
        assert_eq!(data[25], 6, "the {size}px PNG is RGBA");
    }
}
