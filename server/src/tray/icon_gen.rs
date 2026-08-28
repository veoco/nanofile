//! Byte-level helpers for the compile-time tray and exe icon generation.
//!
//! Included twice: by `build.rs` via `#[path]` (where the SVG is rendered and
//! the outputs are written to `OUT_DIR`) and by `src/tray` (for the icon size
//! constant and the unit tests). Pure `std` only — no rendering crates.

/// Edge length of the tray icon rasterized into `$OUT_DIR/tray_icon.rgba`.
pub const TRAY_ICON_SIZE: u32 = 32;

// The following are used by the build-script copy of this module (`build.rs`
// includes this file via `#[path]`); the runtime copy only needs the constant
// above and the tests below.
/// Edge lengths embedded into the Windows exe icon (ICO DIB entries).
#[allow(dead_code)]
pub const EXE_ICON_SIZES: [u32; 5] = [16, 24, 32, 48, 64];

/// Packs straight-alpha RGBA pixels into a 32bpp Windows device-independent
/// bitmap — the classic ICO entry format: `BITMAPINFOHEADER` followed by
/// bottom-up BGRA pixel rows and a (zeroed, unused) 1bpp AND mask. The alpha
/// channel is carried by the BGRA rows.
#[allow(dead_code)]
pub fn dib_from_rgba(size: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        (size * size * 4) as usize,
        "RGBA buffer size mismatch"
    );
    let mut dib = Vec::with_capacity(dib_len(size) as usize);

    // BITMAPINFOHEADER
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&(size as i32).to_le_bytes()); // biWidth
    // For icon DIBs biHeight counts both the pixel rows and the AND mask.
    dib.extend_from_slice(&((size as i32) * 2).to_le_bytes()); // biHeight
    dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    dib.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    dib.extend_from_slice(&0u32.to_le_bytes()); // biSizeImage (BI_RGB: may be 0)
    dib.extend_from_slice(&0u32.to_le_bytes()); // biXPelsPerMeter
    dib.extend_from_slice(&0u32.to_le_bytes()); // biYPelsPerMeter
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

    // Pixel rows, bottom-up, BGRA.
    for y in (0..size).rev() {
        let row = &rgba[(y * size * 4) as usize..((y + 1) * size * 4) as usize];
        for [r, g, b, a] in row.as_chunks::<4>().0 {
            dib.extend_from_slice(&[*b, *g, *r, *a]);
        }
    }

    // AND mask: 1bpp rows padded to 32 bits, all transparent (alpha wins).
    let mask_row = size.div_ceil(32) * 4;
    dib.resize(dib.len() + (mask_row * size) as usize, 0);
    dib
}

/// Total byte length of a `dib_from_rgba` payload for the given edge length.
#[allow(dead_code)]
pub fn dib_len(size: u32) -> u32 {
    let mask_row = size.div_ceil(32) * 4;
    40 + size * size * 4 + mask_row * size
}

/// Assembles an ICO file from `(edge length, DIB payload)` images.
#[allow(dead_code)]
pub fn build_ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());

    // Directory entries precede the image data; offsets are cumulative.
    let mut offset = (6 + 16 * images.len()) as u32;
    for (size, data) in images {
        ico.push(if *size < 256 { *size as u8 } else { 0 });
        ico.push(if *size < 256 { *size as u8 } else { 0 });
        ico.push(0); // color count
        ico.push(0); // reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // color planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        ico.extend_from_slice(data);
    }
    ico
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2 opaque red square.
    fn sample_rgba(size: u32) -> Vec<u8> {
        (0..size * size).flat_map(|_| [255u8, 0, 0, 255]).collect()
    }

    #[test]
    fn dib_layout_is_bottom_up_bgra_with_header() {
        let dib = dib_from_rgba(2, &sample_rgba(2));
        assert_eq!(dib.len(), dib_len(2) as usize);
        assert_eq!(dib[0..4], 40u32.to_le_bytes()); // biSize
        assert_eq!(dib[4..8], 2i32.to_le_bytes()); // biWidth
        assert_eq!(dib[8..12], 4i32.to_le_bytes()); // biHeight = 2 * size
        assert_eq!(dib[12..14], 1u16.to_le_bytes());
        assert_eq!(dib[14..16], 32u16.to_le_bytes());

        // First pixel row (top row in the image) is the last DIB row: BGRA.
        assert_eq!(&dib[40..44], &[0, 0, 255, 255]);
        assert_eq!(&dib[44..48], &[0, 0, 255, 255]);
        assert_eq!(&dib[48..52], &[0, 0, 255, 255]);
        assert_eq!(&dib[52..56], &[0, 0, 255, 255]);

        // AND mask rows padded to 4 bytes, all zero.
        assert!(dib[56..].iter().all(|&b| b == 0));
    }

    #[test]
    fn ico_header_and_offsets() {
        let images: Vec<(u32, Vec<u8>)> = EXE_ICON_SIZES
            .iter()
            .map(|&s| (s, dib_from_rgba(s, &sample_rgba(s))))
            .collect();
        let ico = build_ico(&images);

        assert_eq!(&ico[0..2], &0u16.to_le_bytes());
        assert_eq!(&ico[2..4], &1u16.to_le_bytes()); // ICO type
        assert_eq!(&ico[4..6], &(EXE_ICON_SIZES.len() as u16).to_le_bytes());

        // First entry: 16px, followed by entries then data.
        assert_eq!(ico[6], 16);
        assert_eq!(ico[7], 16);
        let first_len = dib_len(16);
        assert_eq!(&ico[6 + 8..6 + 12], &first_len.to_le_bytes());
        assert_eq!(&ico[6 + 12..6 + 16], &86u32.to_le_bytes()); // data starts at 6 + 16*5

        let total: u32 = 6 + 16 * 5 + EXE_ICON_SIZES.iter().map(|&s| dib_len(s)).sum::<u32>();
        assert_eq!(ico.len(), total as usize);
    }
}
