//! Rough file sizes before writing (DESIGN.md, Size estimates): a few
//! patches of the image, at full resolution so their detail is the
//! image's, encoded as the file would be; their bytes per pixel times the
//! image's pixels. A guess within tens of percent: lossless coders do a
//! little better on a whole image than on patches.

use std::sync::Arc;

use image::RgbaImage;

use crate::config::TileFormat;

/// A patch's side, in pixels.
pub const PATCH: u32 = 256;
/// Patches taken (about 1 MP); AVIF, slow to encode, uses the first few.
pub const PATCHES: usize = 16;
const AVIF_PATCHES: usize = 4;

/// What an estimate is made from: patches standing for `pixels` pixels.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub patches: Vec<Arc<RgbaImage>>,
    pub pixels: u64,
    /// The files as they are (an export as stored), when known exactly.
    pub stored: Option<u64>,
}

impl PartialEq for Sample {
    fn eq(&self, other: &Sample) -> bool {
        self.pixels == other.pixels
            && self.stored == other.stored
            && self.patches.len() == other.patches.len()
            && self.patches.iter().zip(&other.patches).all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

/// `n` patches spread evenly over `img` (a grid of them, each centred in
/// its part); the whole image when it's small.
pub fn patches(img: &RgbaImage, n: usize) -> Vec<Arc<RgbaImage>> {
    let (w, h) = img.dimensions();
    if n == 0 || w == 0 || h == 0 {
        return vec![];
    }
    if (w as u64 * h as u64) <= (n as u64 * PATCH as u64 * PATCH as u64) {
        return vec![Arc::new(img.clone())];
    }
    let cols = ((n as f64 * w as f64 / h as f64).sqrt().round() as u32).clamp(1, n as u32);
    let rows = (n as u32).div_ceil(cols);
    let (pw, ph) = (PATCH.min(w), PATCH.min(h));
    let mut out = vec![];
    for r in 0..rows {
        for c in 0..cols {
            if out.len() == n {
                break;
            }
            let cx = (w as u64 * (2 * c as u64 + 1) / (2 * cols as u64)) as u32;
            let cy = (h as u64 * (2 * r as u64 + 1) / (2 * rows as u64)) as u32;
            let x = cx.saturating_sub(pw / 2).min(w - pw);
            let y = cy.saturating_sub(ph / 2).min(h - ph);
            out.push(Arc::new(image::imageops::crop_imm(img, x, y, pw, ph).to_image()));
        }
    }
    out
}

/// A sample of one image: its patches, standing for all of it.
pub fn sample(img: &RgbaImage) -> Sample {
    Sample { patches: patches(img, PATCHES), pixels: img.width() as u64 * img.height() as u64, stored: None }
}

/// Images an export's sample is taken from, spread through it.
pub const FILES: usize = 4;

/// A sample of image files (an export): patches from up to [`FILES`] of
/// them spread through the list, standing for all their pixels (each
/// one's size from its header), and their bytes as they are.
pub fn sample_files(paths: &[std::path::PathBuf]) -> Result<Sample, String> {
    let err = |p: &std::path::Path, e: &dyn std::fmt::Display| format!("{}: {e}", p.display());
    let (mut pixels, mut stored) = (0u64, 0u64);
    for p in paths {
        let (w, h) = image::image_dimensions(p).map_err(|e| err(p, &e))?;
        pixels += w as u64 * h as u64;
        stored += std::fs::metadata(p).map_err(|e| err(p, &e))?.len();
    }
    let k = paths.len().min(FILES);
    let mut out = vec![];
    for i in 0..k {
        let p = &paths[i * paths.len() / k];
        let bytes = std::fs::read(p).map_err(|e| err(p, &e))?;
        let px = crate::import::decode_pixels(&bytes).map_err(|e| err(p, &e))?;
        let img = match px.channels {
            4 => RgbaImage::from_raw(px.width, px.height, px.data),
            _ => image::RgbImage::from_raw(px.width, px.height, px.data)
                .map(|i| image::DynamicImage::ImageRgb8(i).to_rgba8()),
        }
        .ok_or_else(|| err(p, &"pixels don't fill the image"))?;
        out.extend(patches(&img, PATCHES / k));
    }
    Ok(Sample { patches: out, pixels, stored: Some(stored) })
}

/// About how many bytes `sample`'s image is as `format` at `quality`.
/// None when it can't be encoded (nothing to sample, too large for WebP
/// is not checked here).
pub fn size(sample: &Sample, format: TileFormat, quality: u8) -> Option<u64> {
    let take = if format == TileFormat::Avif { AVIF_PATCHES } else { usize::MAX };
    let (mut bytes, mut pixels) = (0u64, 0u64);
    for p in sample.patches.iter().take(take) {
        bytes += crate::export::encode(p, format, quality).ok()?.len() as u64;
        pixels += p.width() as u64 * p.height() as u64;
    }
    if pixels == 0 {
        return None;
    }
    Some((bytes as f64 / pixels as f64 * sample.pixels as f64).round() as u64)
}

/// A size for people: "830 KB", "12 MB", "1.4 GB" (1000s, two figures
/// at most past the point when small).
pub fn human(bytes: u64) -> String {
    let b = bytes as f64;
    let (v, unit) = match bytes {
        0..1_000 => return format!("{bytes} B"),
        1_000..1_000_000 => (b / 1e3, "KB"),
        1_000_000..1_000_000_000 => (b / 1e6, "MB"),
        _ => (b / 1e9, "GB"),
    };
    if v < 10.0 { format!("{v:.1} {unit}") } else { format!("{v:.0} {unit}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detail like a photo's: noise over gradients.
    fn photo(w: u32, h: u32) -> RgbaImage {
        let mut seed = 7u32;
        RgbaImage::from_fn(w, h, |x, y| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let n = (seed >> 27) as u8;
            image::Rgba([(x / 8) as u8 ^ n, (y / 6) as u8 + n, ((x + y) / 10) as u8, 255])
        })
    }

    #[test]
    fn patches_spread_and_small_images_whole() {
        let img = photo(2000, 1000);
        let p = patches(&img, 8);
        assert_eq!(p.len(), 8);
        assert!(p.iter().all(|p| p.dimensions() == (PATCH, PATCH)));
        let small = photo(300, 200);
        assert_eq!(patches(&small, 8)[0].dimensions(), (300, 200), "whole");
    }

    #[test]
    fn near_the_real_size_and_quality_moves_it() {
        let img = photo(1600, 1200);
        let s = sample(&img);
        for (f, q) in [(TileFormat::Png, 100), (TileFormat::Webp, 100), (TileFormat::Webp, 75), (TileFormat::Jpeg, 85)]
        {
            let real = crate::export::encode(&img, f, q).unwrap().len() as f64;
            let guess = size(&s, f, q).unwrap() as f64;
            let off = guess / real;
            assert!((0.6..1.6).contains(&off), "{f:?} {q}: {guess} for {real}");
        }
        let lossy = size(&s, TileFormat::Webp, 60).unwrap();
        assert!(lossy < size(&s, TileFormat::Webp, 100).unwrap());
        assert!(size(&s, TileFormat::Jpeg, 30).unwrap() < size(&s, TileFormat::Jpeg, 95).unwrap());
        assert_eq!(size(&Sample::default(), TileFormat::Png, 100), None);
    }

    #[test]
    fn files_sampled_their_bytes_exact() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = (0..6u32)
            .map(|i| {
                let p = dir.path().join(format!("{i}.png"));
                image::DynamicImage::ImageRgba8(photo(400 + i * 10, 300)).save(&p).unwrap();
                p
            })
            .collect();
        let s = sample_files(&paths).unwrap();
        let bytes: u64 = paths.iter().map(|p| std::fs::metadata(p).unwrap().len()).sum();
        assert_eq!(s.stored, Some(bytes));
        assert_eq!(s.pixels, (0..6u64).map(|i| (400 + i * 10) * 300).sum::<u64>());
        assert_eq!(s.patches.len(), FILES, "four of the six, each small enough to take whole");
        let guess = size(&s, TileFormat::Png, 100).unwrap() as f64;
        assert!((0.6..1.6).contains(&(guess / bytes as f64)), "{guess} for {bytes}");
    }

    #[test]
    fn sizes_for_people() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(830_000), "830 KB");
        assert_eq!(human(1_450_000), "1.4 MB");
        assert_eq!(human(31_200_000), "31 MB");
        assert_eq!(human(2_300_000_000), "2.3 GB");
    }
}
