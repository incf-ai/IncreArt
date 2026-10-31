//! Encoding a composed image (a tiling) for saving, in the format chosen
//! (DESIGN.md, Tiling).

use std::io::Cursor;

use image::{ImageEncoder, RgbaImage};

use crate::config::TileFormat;
use crate::pixel::Pixels;

/// `img` as `format`; `quality` (percent) is for the lossy ones. JPEG has
/// no alpha: transparency is flattened onto white.
pub fn encode(img: &RgbaImage, format: TileFormat, quality: u8) -> Result<Vec<u8>, String> {
    let quality = quality.clamp(1, 100);
    let (w, h) = img.dimensions();
    let mut out = Vec::new();
    match format.usable() {
        TileFormat::Png => {
            img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        }
        TileFormat::Webp => {
            let px = Pixels { width: w, height: h, channels: 4, data: img.as_raw().clone() };
            out = crate::webp::encode(&px, &[]).ok_or_else(|| {
                format!("{w} × {h} is too large for WebP (at most {} on a side)", crate::webp::MAX_SIDE)
            })?;
        }
        TileFormat::Jpeg => {
            let rgb: Vec<u8> = img
                .pixels()
                .flat_map(|p| {
                    let a = p[3] as u32;
                    [0, 1, 2].map(|i| ((p[i] as u32 * a + 255 * (255 - a) + 127) / 255) as u8)
                })
                .collect();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
                .map_err(|e| e.to_string())?;
        }
        TileFormat::Avif => avif(img, quality, &mut out)?,
    }
    Ok(out)
}

#[cfg(feature = "avif")]
fn avif(img: &RgbaImage, quality: u8, out: &mut Vec<u8>) -> Result<(), String> {
    // speed 7 of 10: a large tiling takes seconds, not minutes
    image::codecs::avif::AvifEncoder::new_with_speed_quality(out, 7, quality)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "avif"))]
fn avif(_: &RgbaImage, _: u8, _: &mut Vec<u8>) -> Result<(), String> {
    // (TileFormat::usable() offers no AVIF without the avif feature)
    Err("this build can't write AVIF".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img() -> RgbaImage {
        RgbaImage::from_fn(40, 30, |x, y| image::Rgba([(x * 6) as u8, (y * 8) as u8, 90, if x < 5 { 0 } else { 255 }]))
    }

    #[test]
    fn each_format_reads_back() {
        for &f in TileFormat::available() {
            let bytes = encode(&img(), f, 70).unwrap();
            let back = image::load_from_memory(&bytes).unwrap();
            assert_eq!((back.width(), back.height()), (40, 30), "{f:?}");
            if !f.lossy() {
                assert_eq!(back.to_rgba8(), img(), "{f:?} is lossless");
            }
        }
        // JPEG: transparent onto white
        let jpeg = image::load_from_memory(&encode(&img(), TileFormat::Jpeg, 95).unwrap()).unwrap().to_rgb8();
        assert!(jpeg.get_pixel(1, 15).0.iter().all(|&c| c > 240));
    }

    #[test]
    fn quality_changes_the_size() {
        let big = RgbaImage::from_fn(256, 256, |x, y| image::Rgba([(x ^ y) as u8, (x * y) as u8, (x + y) as u8, 255]));
        let small = encode(&big, TileFormat::Jpeg, 30).unwrap().len();
        let large = encode(&big, TileFormat::Jpeg, 95).unwrap().len();
        assert!(small < large, "{small} < {large}");
    }
}
