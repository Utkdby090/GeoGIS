//! Reading image headers without decoding, and preparing images for the PDF.
//!
//! JPEGs are embedded as they are, minus the metadata segments a PDF reader never uses: a DJI or
//! FLIR R-JPEG carries its raw thermal data (often several MB) in APP segments, which would
//! otherwise make every report page several times larger than its picture.

use std::sync::Arc;

use crate::page::{MAX_PIXELS, MAX_PNG_PIXELS};
use crate::{Image, ImageInfo, ImageKind, PhotoIssue};

impl Image {
    /// Reads `bytes` as a JPEG or PNG. A JPEG is trimmed of segments the PDF doesn't need (see
    /// [`slim_jpeg`]); nothing is decoded.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Image, PhotoIssue> {
        let info = ImageInfo::read(&bytes)?;
        let bytes = match info.kind {
            ImageKind::Jpeg { .. } => slim_jpeg(&bytes).unwrap_or(bytes),
            ImageKind::Png => bytes,
        };
        Ok(Image { bytes: Arc::from(bytes), info })
    }
}

impl ImageInfo {
    /// The header of a JPEG or PNG: its kind, size and EXIF orientation.
    pub fn read(bytes: &[u8]) -> Result<ImageInfo, PhotoIssue> {
        let info = if bytes.starts_with(&[0xFF, 0xD8]) {
            jpeg_info(bytes)?
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            png_info(bytes)?
        } else {
            return Err(PhotoIssue::NotAnImage);
        };
        if info.width == 0 || info.height == 0 {
            return Err(PhotoIssue::Damaged("the image has no size".into()));
        }
        if u64::from(info.width) * u64::from(info.height) > MAX_PIXELS {
            return Err(PhotoIssue::Damaged(format!("{} × {} pixels is more than a report can hold", info.width, info.height)));
        }
        if info.kind == ImageKind::Png && u64::from(info.width) * u64::from(info.height) > MAX_PNG_PIXELS {
            return Err(PhotoIssue::Damaged(format!(
                "{} × {} pixels is too large for a PNG in a report (use a JPEG)",
                info.width, info.height
            )));
        }
        Ok(info)
    }
}

/// One JPEG marker segment: its marker byte and its payload (without the length field).
struct Segment<'a> {
    marker: u8,
    data: &'a [u8],
    /// Where the segment (its 0xFF) starts and ends in the file.
    start: usize,
    end: usize,
}

/// The marker segments before the image data, in order. Stops at start-of-scan (whose offset is
/// returned) and fails on a truncated or malformed header rather than reading past the end.
fn segments(bytes: &[u8]) -> Result<(Vec<Segment<'_>>, usize), PhotoIssue> {
    let damaged = |m: &str| PhotoIssue::Damaged(m.to_string());
    let mut out = Vec::new();
    let mut i = 2; // past SOI
    loop {
        // Markers may be padded with any number of 0xFF fill bytes.
        let Some(&ff) = bytes.get(i) else { return Err(damaged("the JPEG ends before its picture data")) };
        if ff != 0xFF {
            return Err(damaged("the JPEG header is not in the expected format"));
        }
        let mut j = i + 1;
        while bytes.get(j) == Some(&0xFF) {
            j += 1;
        }
        let Some(&marker) = bytes.get(j) else { return Err(damaged("the JPEG ends inside a marker")) };
        // Stand-alone markers carry no length.
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i = j + 1;
            continue;
        }
        let len = match (bytes.get(j + 1), bytes.get(j + 2)) {
            (Some(&hi), Some(&lo)) => usize::from(u16::from_be_bytes([hi, lo])),
            _ => return Err(damaged("the JPEG ends inside a segment header")),
        };
        if len < 2 {
            return Err(damaged("a JPEG segment has an impossible length"));
        }
        let end = j + 1 + len;
        let data = bytes.get(j + 3..end).ok_or_else(|| damaged("a JPEG segment runs past the end of the file"))?;
        out.push(Segment { marker, data, start: i, end });
        if marker == 0xDA {
            return Ok((out, i));
        }
        i = end;
    }
}

fn jpeg_info(bytes: &[u8]) -> Result<ImageInfo, PhotoIssue> {
    let (segs, _) = segments(bytes)?;
    let mut size = None;
    let mut orientation = 1;
    let mut adobe = false;
    for s in &segs {
        match s.marker {
            // Start-of-frame markers (all but DHT C4, JPG C8 and DAC CC).
            0xC0..=0xCF if !matches!(s.marker, 0xC4 | 0xC8 | 0xCC) => {
                let d = s.data;
                if d.len() < 6 {
                    return Err(PhotoIssue::Damaged("the JPEG frame header is too short".into()));
                }
                if s.marker != 0xC0 && s.marker != 0xC1 && s.marker != 0xC2 {
                    // Lossless, hierarchical and arithmetic-coded JPEGs: PDF readers can't show them.
                    return Err(PhotoIssue::Damaged("this kind of JPEG (lossless or arithmetic coded) can't go in a PDF".into()));
                }
                if d.first() != Some(&8) {
                    return Err(PhotoIssue::Damaged("only 8-bit JPEGs can go in a PDF".into()));
                }
                let h = u32::from(u16::from_be_bytes([d[1], d[2]]));
                let w = u32::from(u16::from_be_bytes([d[3], d[4]]));
                size = Some((w, h, d[5]));
            }
            0xE1 if s.data.starts_with(b"Exif\0\0") => {
                if let Some(o) = s.data.get(6..).and_then(exif_orientation) {
                    orientation = o;
                }
            }
            0xEE if s.data.starts_with(b"Adobe") => adobe = true,
            _ => {}
        }
    }
    let (width, height, components) = size.ok_or_else(|| PhotoIssue::Damaged("the JPEG has no frame header".into()))?;
    if !matches!(components, 1 | 3 | 4) {
        return Err(PhotoIssue::Damaged(format!("a JPEG with {components} colour channels can't go in a PDF")));
    }
    Ok(ImageInfo { kind: ImageKind::Jpeg { components, inverted: components == 4 && adobe }, width, height, orientation })
}

/// The orientation tag (0x0112) of the first IFD of a TIFF-structured EXIF block, if it is 1–8.
fn exif_orientation(tiff: &[u8]) -> Option<u8> {
    let le = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b = [*tiff.get(o)?, *tiff.get(o + 1)?];
        Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b = [*tiff.get(o)?, *tiff.get(o + 1)?, *tiff.get(o + 2)?, *tiff.get(o + 3)?];
        Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let count = usize::from(u16_at(ifd)?);
    // An IFD entry is 12 bytes; a damaged count can't make this loop run past the data.
    for k in 0..count.min(tiff.len() / 12) {
        let e = ifd.checked_add(2 + k * 12)?;
        if u16_at(e)? == 0x0112 {
            let v = u16_at(e + 8)?;
            return u8::try_from(v).ok().filter(|v| (1..=8).contains(v));
        }
    }
    None
}

/// The JPEG without APP1–APP13, APP15 and comment segments (EXIF, XMP, ICC, FLIR and DJI thermal
/// data, maker notes), keeping JFIF (APP0) and Adobe (APP14), which say how to read the colours.
/// None when nothing would be removed or the header can't be read (the original is then used).
pub fn slim_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    let (segs, sos) = segments(bytes).ok()?;
    let drop = |m: u8| (0xE1..=0xED).contains(&m) || m == 0xEF || m == 0xFE;
    if !segs.iter().any(|s| drop(s.marker)) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(bytes.get(..2)?);
    for s in segs.iter().filter(|s| s.marker != 0xDA && !drop(s.marker)) {
        out.extend_from_slice(bytes.get(s.start..s.end)?);
    }
    out.extend_from_slice(bytes.get(sos..)?);
    Some(out)
}

fn png_info(bytes: &[u8]) -> Result<ImageInfo, PhotoIssue> {
    // The IHDR chunk always comes first: length (4), "IHDR", width (4), height (4).
    let ihdr = bytes.get(8..24).ok_or_else(|| PhotoIssue::Damaged("the PNG ends inside its header".into()))?;
    if ihdr.get(4..8) != Some(b"IHDR".as_slice()) {
        return Err(PhotoIssue::Damaged("the PNG has no header chunk".into()));
    }
    let be = |o: usize| ihdr.get(o..o + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map_or(0, u32::from_be_bytes);
    Ok(ImageInfo { kind: ImageKind::Png, width: be(8), height: be(12), orientation: 1 })
}

/// A PNG decoded for the PDF: 8-bit RGB samples, and 8-bit alpha when it has transparency, each
/// zlib-compressed.
pub(crate) struct DecodedPng {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub alpha: Option<Vec<u8>>,
}

/// Decodes a PNG (any bit depth or colour type) to RGB(A) and compresses it for the PDF. The
/// decoder's memory is capped at [`MAX_PNG_PIXELS`] RGBA pixels.
pub(crate) fn decode_png(bytes: &[u8]) -> Result<DecodedPng, PhotoIssue> {
    use miniz_oxide::deflate::compress_to_vec_zlib;
    let damaged = |e: png::DecodingError| PhotoIssue::Damaged(e.to_string());
    // Decoded RGBA of the largest PNG allowed, with room for the decoder's own buffers.
    let limits = png::Limits { bytes: MAX_PNG_PIXELS as usize * 4 + (1 << 20) };
    let mut decoder = png::Decoder::new_with_limits(std::io::Cursor::new(bytes), limits);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(damaged)?;
    let size = reader.output_buffer_size().ok_or_else(|| PhotoIssue::Damaged("the PNG is too large".into()))?;
    let mut buf = vec![0; size];
    let frame = reader.next_frame(&mut buf).map_err(damaged)?;
    buf.truncate(frame.buffer_size());
    let (w, h) = (frame.width, frame.height);
    let px = w as usize * h as usize;
    let (mut rgb, mut alpha) = (Vec::with_capacity(px * 3), Vec::new());
    match frame.color_type {
        png::ColorType::Rgb => rgb = buf,
        png::ColorType::Rgba => {
            alpha.reserve(px);
            for p in buf.as_chunks::<4>().0 {
                rgb.extend_from_slice(&p[..3]);
                alpha.push(p[3]);
            }
        }
        png::ColorType::Grayscale => buf.iter().for_each(|&g| rgb.extend_from_slice(&[g, g, g])),
        png::ColorType::GrayscaleAlpha => {
            alpha.reserve(px);
            for p in buf.as_chunks::<2>().0 {
                rgb.extend_from_slice(&[p[0], p[0], p[0]]);
                alpha.push(p[1]);
            }
        }
        png::ColorType::Indexed => return Err(PhotoIssue::Damaged("the PNG palette could not be expanded".into())),
    }
    if rgb.len() != px * 3 {
        return Err(PhotoIssue::Damaged("the PNG has fewer pixels than its header says".into()));
    }
    // A fully opaque alpha channel adds nothing.
    let alpha = (!alpha.is_empty() && alpha.iter().any(|&a| a != 255)).then(|| compress_to_vec_zlib(&alpha, 6));
    Ok(DecodedPng { width: w, height: h, rgb: compress_to_vec_zlib(&rgb, 6), alpha })
}
