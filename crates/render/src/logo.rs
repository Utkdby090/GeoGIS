//! The GeoGIS logo, drawn in code so the app needs no image files. It is a placeholder (see
//! [`pixmap`]) until GeoFlux has a designed mark.

use tiny_skia::{Color, FillRule, GradientStop, LinearGradient, Mask, Paint, PathBuilder, Pixmap, Point, SpreadMode, Stroke, Transform};

const C: f32 = 256.0;

fn gradient(a: (u8, u8, u8), b: (u8, u8, u8), x0: f32, y0: f32, x1: f32, y1: f32) -> Paint<'static> {
    let mut p = Paint { anti_alias: true, ..Paint::default() };
    let stops =
        vec![GradientStop::new(0.0, Color::from_rgba8(a.0, a.1, a.2, 255)), GradientStop::new(1.0, Color::from_rgba8(b.0, b.1, b.2, 255))];
    match LinearGradient::new(Point::from_xy(x0, y0), Point::from_xy(x1, y1), stops, SpreadMode::Pad, Transform::identity()) {
        Some(sh) => p.shader = sh,
        None => p.set_color_rgba8(a.0, a.1, a.2, 255),
    }
    p
}

fn solid(r: u8, g: u8, b: u8, a: u8) -> Paint<'static> {
    let mut p = Paint { anti_alias: true, ..Paint::default() };
    p.set_color_rgba8(r, g, b, a);
    p
}

/// The logo as a premultiplied RGBA pixmap of `size`×`size` pixels (16..=1024).
///
/// A placeholder GeoGIS mark until the real one is designed: a rounded navy-to-teal tile with
/// a faint map grid and a white location pin holding a thermal hot spot.
pub fn pixmap(size: u32) -> Option<Pixmap> {
    let size = size.clamp(16, 1024);
    let mut pm = Pixmap::new(size, size)?;
    let t = Transform::from_scale(size as f32 / 512.0, size as f32 / 512.0);

    // Tile.
    let tile = PathBuilder::from_rect(tiny_skia::Rect::from_xywh(16.0, 16.0, 480.0, 480.0)?);
    let mut mask = Mask::new(size, size)?;
    mask.fill_path(&rounded(16.0, 16.0, 480.0, 480.0, 104.0)?, FillRule::Winding, true, t);
    pm.fill_path(&tile, &gradient((0x13, 0x4B, 0x7A), (0x12, 0x9C, 0x86), 16.0, 16.0, 496.0, 496.0), FillRule::Winding, t, Some(&mask));

    // Map grid.
    let mut pb = PathBuilder::new();
    for k in 1..4 {
        let v = 16.0 + 120.0 * k as f32;
        pb.move_to(v, 16.0);
        pb.line_to(v, 496.0);
        pb.move_to(16.0, v);
        pb.line_to(496.0, v);
    }
    pm.stroke_path(&pb.finish()?, &solid(255, 255, 255, 46), &Stroke { width: 6.0, ..Stroke::default() }, t, Some(&mask));

    // Pin: a circle above a point, with a hole holding the hot spot.
    let mut pb = PathBuilder::new();
    pb.move_to(C, 432.0);
    pb.cubic_to(C - 40.0, 360.0, C - 132.0, 300.0, C - 132.0, 212.0);
    pb.cubic_to(C - 132.0, 139.0, C - 73.0, 80.0, C, 80.0);
    pb.cubic_to(C + 73.0, 80.0, C + 132.0, 139.0, C + 132.0, 212.0);
    pb.cubic_to(C + 132.0, 300.0, C + 40.0, 360.0, C, 432.0);
    pb.close();
    pm.fill_path(&pb.finish()?, &solid(255, 255, 255, 255), FillRule::Winding, t, None);
    pm.fill_path(
        &PathBuilder::from_circle(C, 212.0, 70.0)?,
        &gradient((0xFF, 0xC2, 0x3D), (0xE0, 0x3B, 0x24), C - 70.0, 142.0, C + 70.0, 282.0),
        FillRule::Winding,
        t,
        None,
    );
    pm.fill_path(&PathBuilder::from_circle(C, 212.0, 26.0)?, &solid(255, 248, 214, 255), FillRule::Winding, t, None);
    Some(pm)
}

/// A rounded rectangle.
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let k = 0.552_285 * r;
    let (x1, y1) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// The logo as straight (unpremultiplied) RGBA bytes, for window icons.
pub fn rgba(size: u32) -> Option<(u32, Vec<u8>)> {
    let pm = pixmap(size)?;
    let mut out = Vec::with_capacity(pm.data().len());
    for p in pm.pixels() {
        let c = p.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some((pm.width(), out))
}

/// The logo as an ICO file holding PNG images from 16 to 256 px (Windows Vista and later read
/// these), for the exe icon and the Windows installers.
pub fn ico() -> Option<Vec<u8>> {
    const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
    let mut images = Vec::new();
    for size in SIZES {
        images.push((size, pixmap(size)?.encode_png().ok()?));
    }
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&u16::try_from(images.len()).ok()?.to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, png) in &images {
        // A dimension of 0 means 256.
        let dim = u8::try_from(*size).unwrap_or(0);
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&u32::try_from(png.len()).ok()?.to_le_bytes());
        ico.extend_from_slice(&u32::try_from(offset).ok()?.to_le_bytes());
        offset += png.len();
    }
    for (_, png) in &images {
        ico.extend_from_slice(png);
    }
    Some(ico)
}

/// A 24-bit BMP of `width`×`height` white pixels with the logo (`size` px) drawn at `x`, `y`,
/// for installer wizard images. Sizes are clamped to 1..=2048.
pub fn bmp(size: u32, width: u32, height: u32, x: i32, y: i32) -> Option<Vec<u8>> {
    let (w, h) = (width.clamp(1, 2048), height.clamp(1, 2048));
    let (side, px) = rgba(size)?;
    let row = (w as usize * 3).div_ceil(4) * 4;
    let len = 54 + row * h as usize;
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&u32::try_from(len).ok()?.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&u32::try_from(row * h as usize).ok()?.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    // Rows run bottom to top, pixels are BGR, each row padded to four bytes.
    for r in (0..h as i64).rev() {
        let start = out.len();
        for c in 0..w as i64 {
            let (lx, ly) = (c - i64::from(x), r - i64::from(y));
            let mut bgr = [255u8; 3];
            if (0..i64::from(side)).contains(&lx) && (0..i64::from(side)).contains(&ly) {
                let i = usize::try_from((ly * i64::from(side) + lx) * 4).ok()?;
                if let Some(p) = px.get(i..i + 4) {
                    let a = u32::from(p[3]);
                    let over = |v: u8| ((u32::from(v) * a + 255 * (255 - a)) / 255) as u8;
                    bgr = [over(p[2]), over(p[1]), over(p[0])];
                }
            }
            out.extend_from_slice(&bgr);
        }
        out.resize(start + row, 0);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn draws_at_every_size() {
        for s in [0, 16, 32, 256, 5000] {
            let (w, px) = super::rgba(s).unwrap();
            assert_eq!(px.len() as u32, w * w * 4);
            assert!(px.chunks(4).any(|c| c[3] == 255), "logo has opaque pixels");
        }
    }

    #[test]
    fn bmp_has_padded_rows_and_the_logo() {
        let bmp = super::bmp(32, 50, 40, 9, 4).unwrap();
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(bmp.len(), 54 + 152 * 40);
        assert!(bmp[54..].chunks(3).any(|c| c != [255, 255, 255]), "logo pixels");
        // A logo placed off the canvas leaves it white.
        let blank = super::bmp(32, 10, 10, 500, -500).unwrap();
        assert!(blank[54..].chunks(32).all(|r| r[..30].iter().all(|&b| b == 255)));
    }

    #[test]
    fn ico_lists_seven_png_images() {
        let ico = super::ico().unwrap();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 7, 0]);
        // The first image starts right after the directory, with the PNG signature.
        let offset = u32::from_le_bytes(ico[18..22].try_into().unwrap()) as usize;
        assert_eq!(offset, 6 + 16 * 7);
        assert_eq!(&ico[offset..offset + 4], b"\x89PNG");
    }
}
