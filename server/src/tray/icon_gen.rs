//! Byte-level helpers for the compile-time tray and exe icon generation.
//!
//! Included twice — by `build.rs` via `#[path]`, which renders the SVG into
//! `OUT_DIR`, and by `src/tray` for the size constant and the tests. Pure `std`.

/// Edge length of the tray icon rasterized into `$OUT_DIR/tray_icon_*.rgba`.
pub const TRAY_ICON_SIZE: u32 = 32;

/// One fill pair of the brand mark: the tile and the glyph.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct MarkColors {
    pub tile: &'static str,
    pub glyph: &'static str,
}

/// The two literals `static/img/favicon.svg` carries for its light theme
/// (`--color-accent` / `--color-accent-ink`); [`recolor`] replaces both.
#[allow(dead_code)]
pub const FAVICON_TILE: &str = "#202020";
#[allow(dead_code)]
pub const FAVICON_GLYPH: &str = "#ffffff";

/// Tray icon for a light desktop: the plain brand mark.
#[allow(dead_code)]
pub const MARK_ON_LIGHT: MarkColors = MarkColors {
    tile: "#202020",
    glyph: "#ffffff",
};

/// Tray icon for a dark desktop: the inverse, i.e. the favicon's own dark pair.
#[allow(dead_code)]
pub const MARK_ON_DARK: MarkColors = MarkColors {
    tile: "#eeeeee",
    glyph: "#111111",
};

/// macOS menu-bar template image: the glyph alone, in black. The system inverts
/// a template itself, highlighted state included, so the tile must stay empty.
#[allow(dead_code)]
pub const MARK_TEMPLATE: MarkColors = MarkColors {
    tile: "none",
    glyph: "#000000",
};

/// The rim rect as the favicon carries it — the 32-unit geometry [`exe_mark`] rewrites.
#[allow(dead_code)]
pub const RIM_GEOMETRY: &str = r#"x="0.5" y="0.5" width="31" height="31" rx="5.5""#;

/// The favicon's rim stroke width, in viewBox units — the 32px value [`exe_mark`] rewrites.
#[allow(dead_code)]
pub const RIM_WIDTH: &str = r#"stroke-width="1""#;

/// The favicon paints the rim out with this literal — see [`exe_mark`] for why.
#[allow(dead_code)]
pub const RIM_OFF: &str = r#"stroke-opacity="0""#;

/// Opacity [`exe_mark`] substitutes in. 0.6 over the graphite tile is a mid
/// grey — silhouette on a dark shell, no bright ring on a light one.
#[allow(dead_code)]
pub const RIM_ON: &str = r#"stroke-opacity="0.6""#;

/// The Windows exe icon variant of the favicon: the plain brand mark plus the
/// rim, for one icon edge length.
///
/// The exe icon is one static asset that has to survive both shells: its
/// graphite tile is Windows 11's dark shell background, so unfilled it vanishes
/// there, while the rim is invisible on a light shell and outlines the tile on a
/// dark one. `size` matters because the rim must be one *device pixel* wide at
/// every size — usvg has no `vector-effect`, so `32 / size` units is one pixel,
/// and the rim is inset by half a stroke to stay inside the tile.
#[allow(dead_code)]
pub fn exe_mark(src: &str, size: u32) -> String {
    for literal in [RIM_GEOMETRY, RIM_WIDTH, RIM_OFF] {
        assert!(
            src.contains(literal),
            "favicon.svg no longer contains {literal}"
        );
    }

    let stroke = 32.0 / size as f32;
    let inset = stroke / 2.0;
    let edge = 32.0 - stroke;
    let geometry = format!(
        r#"x="{inset}" y="{inset}" width="{edge}" height="{edge}" rx="{}""#,
        6.0 - inset
    );
    let width = format!(r#"stroke-width="{stroke}""#);

    src.replace(RIM_GEOMETRY, &geometry)
        .replace(RIM_WIDTH, &width)
        .replace(RIM_OFF, RIM_ON)
}

/// Rewrites the mark's two fills in `src`; panics on a missing literal, so
/// recolouring the favicon breaks the build instead of shipping a wrong icon.
#[allow(dead_code)]
pub fn recolor(src: &str, colors: MarkColors) -> String {
    assert!(
        src.contains(FAVICON_TILE),
        "favicon.svg no longer contains {FAVICON_TILE}"
    );
    assert!(
        src.contains(FAVICON_GLYPH),
        "favicon.svg no longer contains {FAVICON_GLYPH}"
    );
    src.replace(FAVICON_TILE, colors.tile)
        .replace(FAVICON_GLYPH, colors.glyph)
}

/// Edge lengths embedded into the Windows exe icon: every size the shell asks
/// for at 100–250% scaling (16/20/24/32/40/48), 96 and 256 for the large-icon
/// views, plus 64 for the taskbar and Alt-Tab. A missing size is not an error:
/// the shell scales the nearest entry, which looks soft.
#[allow(dead_code)]
pub const EXE_ICON_SIZES: [u32; 10] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

/// The one entry stored as a PNG frame rather than a DIB: Vista and later read a
/// 256×256 entry as PNG, while the uncompressed form would be a 256 KiB DIB.
/// Smaller sizes stay DIBs for the older consumers that still look at them.
#[allow(dead_code)]
pub const EXE_ICON_PNG_SIZE: u32 = 256;

/// Packs straight-alpha RGBA pixels into a 32bpp Windows DIB — the classic ICO
/// entry format: `BITMAPINFOHEADER`, bottom-up BGRA rows and a 1bpp AND mask.
///
/// The mask is rebuilt from the alpha channel (a bit is set only where a pixel
/// is fully transparent) rather than zeroed, because Windows renders the wrong
/// thing when the mask calls a partly or fully opaque pixel transparent —
/// Chromium's `optimize-ico-files.py` recomputes it for that reason.
#[allow(dead_code)]
pub fn dib_from_rgba(size: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        (size * size * 4) as usize,
        "RGBA buffer size mismatch"
    );
    let mut dib = Vec::with_capacity(dib_len(size) as usize);

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

    for y in (0..size).rev() {
        let row = &rgba[(y * size * 4) as usize..((y + 1) * size * 4) as usize];
        for [r, g, b, a] in row.as_chunks::<4>().0 {
            dib.extend_from_slice(&[*b, *g, *r, *a]);
        }
    }

    // AND mask: 1bpp rows padded to a multiple of 4 bytes, bottom-up like the
    // pixel rows, most significant bit first within each byte.
    let mask_row = size.div_ceil(32) * 4;
    for y in (0..size).rev() {
        let row = &rgba[(y * size * 4) as usize..((y + 1) * size * 4) as usize];
        let mut bits = vec![0u8; mask_row as usize];
        for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
            if px[3] == 0 {
                bits[x / 8] |= 0x80 >> (x % 8);
            }
        }
        dib.extend_from_slice(&bits);
    }
    dib
}

/// Total byte length of a `dib_from_rgba` payload for the given edge length.
#[allow(dead_code)]
pub fn dib_len(size: u32) -> u32 {
    let mask_row = size.div_ceil(32) * 4;
    40 + size * size * 4 + mask_row * size
}

/// Assembles an ICO file from `(edge length, frame payload)` images: a payload
/// is a `dib_from_rgba` DIB or, for [`EXE_ICON_PNG_SIZE`], an encoded PNG.
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

        assert_eq!(&dib[40..44], &[0, 0, 255, 255]);
        assert_eq!(&dib[44..48], &[0, 0, 255, 255]);
        assert_eq!(&dib[48..52], &[0, 0, 255, 255]);
        assert_eq!(&dib[52..56], &[0, 0, 255, 255]);

        // Padded AND mask rows; the sample is fully opaque, so all bits are zero.
        assert!(dib[56..].iter().all(|&b| b == 0));
    }

    /// The mask must not call a pixel transparent unless it is fully
    /// transparent — Windows misrenders such icons, which is why Chromium's
    /// `optimize-ico-files.py` rebuilds it. Pins the bottom-up, MSB-first order.
    #[test]
    fn and_mask_marks_only_fully_transparent_pixels() {
        let size = 8;
        let mut rgba = sample_rgba(size);
        for x in [0usize, 3] {
            rgba[x * 4 + 3] = 0; // top row, x = 0 and 3: transparent
        }
        rgba[5 * 4 + 3] = 128; // half-transparent stays opaque to the mask

        let dib = dib_from_rgba(size, &rgba);
        let mask = &dib[40 + (size * size * 4) as usize..];
        assert_eq!(mask.len(), 4 * size as usize);

        assert_eq!(&mask[mask.len() - 4..], &[0x90, 0, 0, 0]); // x=0 and x=3
        assert!(
            mask[..mask.len() - 4].iter().all(|&b| b == 0),
            "only the top row is transparent"
        );
    }

    #[test]
    fn ico_header_and_offsets() {
        let images: Vec<(u32, Vec<u8>)> = EXE_ICON_SIZES
            .iter()
            .map(|&s| (s, dib_from_rgba(s, &sample_rgba(s))))
            .collect();
        let ico = build_ico(&images);
        let entries = images.len() as u32;

        assert_eq!(&ico[0..2], &0u16.to_le_bytes());
        assert_eq!(&ico[2..4], &1u16.to_le_bytes()); // ICO type
        assert_eq!(&ico[4..6], &(entries as u16).to_le_bytes());

        // First entry: 16px, followed by the other entries, then the data.
        assert_eq!(ico[6], 16);
        assert_eq!(ico[7], 16);
        let first_len = dib_len(16);
        assert_eq!(&ico[6 + 8..6 + 12], &first_len.to_le_bytes());
        assert_eq!(&ico[6 + 12..6 + 16], &(6 + 16 * entries).to_le_bytes());

        let total: u32 = 6 + 16 * entries + EXE_ICON_SIZES.iter().map(|&s| dib_len(s)).sum::<u32>();
        assert_eq!(ico.len(), total as usize);

        // A PNG frame is carried verbatim — only the directory entry wraps it.
        let png = b"\x89PNG\r\n\x1a\npayload".to_vec();
        let images = vec![
            (16, dib_from_rgba(16, &sample_rgba(16))),
            (EXE_ICON_PNG_SIZE, png.clone()),
        ];
        let ico = build_ico(&images);
        assert_eq!(ico[6 + 16], 0); // 256 is stored as 0 in the directory
        let offset = u32::from_le_bytes(ico[6 + 16 + 12..6 + 16 + 16].try_into().unwrap());
        assert_eq!(&ico[offset as usize..], &png[..]);
    }

    /// Every size the shell asks for is embedded, and the single PNG frame is the 256 entry.
    #[test]
    fn exe_icon_sizes_cover_the_shell() {
        assert!(EXE_ICON_SIZES.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(*EXE_ICON_SIZES.last().unwrap(), EXE_ICON_PNG_SIZE);
        for want in [16, 20, 24, 32, 40, 48, 64, 96, 128, 256] {
            assert!(EXE_ICON_SIZES.contains(&want), "no {want}px entry");
        }
    }

    /// The favicon's literals are what [`recolor`] substitutes: pin them here,
    /// so a recoloured favicon fails a test instead of shipping wrong colours.
    #[test]
    fn favicon_carries_the_substituted_fills() {
        let svg = include_str!("../../static/img/favicon.svg");

        assert_eq!(MARK_ON_LIGHT.tile, FAVICON_TILE);
        assert_eq!(MARK_ON_LIGHT.glyph, FAVICON_GLYPH);
        assert!(svg.contains(FAVICON_TILE), "base tile fill changed");
        assert!(svg.contains(FAVICON_GLYPH), "base glyph fill changed");

        // The dark variant must match the favicon's own dark rule, or tab and tray disagree.
        assert!(svg.contains(MARK_ON_DARK.tile), "dark tile fill changed");
        assert!(svg.contains(MARK_ON_DARK.glyph), "dark glyph fill changed");
    }

    #[test]
    fn recolor_swaps_both_fills() {
        let svg = format!(r#"<rect fill="{FAVICON_TILE}"/><path fill="{FAVICON_GLYPH}"/>"#);

        let dark = recolor(&svg, MARK_ON_DARK);
        assert!(dark.contains(r##"fill="#eeeeee""##));
        assert!(dark.contains(r##"fill="#111111""##));
        assert!(!dark.contains(FAVICON_TILE) && !dark.contains(FAVICON_GLYPH));

        let template = recolor(&svg, MARK_TEMPLATE);
        assert!(template.contains(r#"fill="none""#));
        assert!(template.contains(r##"fill="#000000""##));
    }

    /// The exe icon is the only mark with the rim on, and it keeps the favicon's
    /// light pair: the rim — not a colour swap — carries the silhouette there.
    #[test]
    fn exe_mark_is_the_brand_pair_with_the_rim_on() {
        let svg = include_str!("../../static/img/favicon.svg");

        assert!(svg.contains(RIM_OFF), "favicon.svg lost the rim");
        let exe = exe_mark(svg, 32);
        assert!(exe.contains(RIM_ON) && !exe.contains(RIM_OFF));
        assert!(exe.contains(FAVICON_TILE), "exe mark is not the light pair");
        assert!(
            exe.contains(FAVICON_GLYPH),
            "exe mark is not the light pair"
        );
        // 32px is the one size where the favicon's own rim geometry is already one pixel.
        assert!(exe.contains(RIM_GEOMETRY) && exe.contains(RIM_WIDTH));

        // Every other consumer keeps it painted out.
        assert!(recolor(svg, MARK_ON_LIGHT).contains(RIM_OFF));
        assert!(recolor(svg, MARK_ON_DARK).contains(RIM_OFF));
        assert!(recolor(svg, MARK_TEMPLATE).contains(RIM_OFF));
    }

    /// The rim is a device-pixel hairline: a fixed literal would be 8px at 256 and gone at 16.
    #[test]
    fn exe_mark_asks_for_a_one_pixel_rim_at_every_size() {
        let svg = include_str!("../../static/img/favicon.svg");

        let attrs = |size: u32| {
            let mark = exe_mark(svg, size);
            let start = mark.find("brand-rim").expect("rim element");
            let elem = &mark[start..];
            let elem = &elem[..elem.find("/>").expect("rim end")];
            let value = |name: &str| -> f32 {
                let prefix = format!("{name}=\"");
                let raw = elem
                    .split_whitespace()
                    .find_map(|token| token.strip_prefix(&prefix))
                    .unwrap_or_else(|| panic!("no {name} in {elem}"));
                raw.trim_end_matches('"').parse().unwrap()
            };
            (value("stroke-width"), value("x"), value("rx"))
        };

        for size in EXE_ICON_SIZES {
            let (stroke, inset, rx) = attrs(size);
            // One viewBox unit is `size / 32` pixels, so the stroke is one pixel.
            assert!(
                (stroke * size as f32 / 32.0 - 1.0).abs() < 1e-4,
                "{size}px rim is not one pixel wide: {stroke} units"
            );
            // Centred on the tile's edge, so it stays inside the icon.
            assert!(
                (inset - stroke / 2.0).abs() < 1e-4,
                "{size}px rim is not inset by half a stroke: {inset}"
            );
            assert!((rx - (6.0 - inset)).abs() < 1e-4, "{size}px rim rx");
        }
    }
}
