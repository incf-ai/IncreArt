//! The pixel hashes (DESIGN.md, Pixel hash): SHA-256 (`increart:pixels`)
//! and BLAKE3 (`increart:blake3`) of a canonical 8-bit RGB/RGBA encoding of
//! the decoded pixels.

use image::{DynamicImage, ImageBuffer};
use sha2::{Digest, Sha256};

pub const TAG_V1: &[u8] = b"increart-pixels-v1";

/// Canonical pixels: 8 bits per channel, 3 (RGB) or 4 (RGBA) channels, rows
/// top to bottom with no padding. Every image the library writes holds
/// exactly these pixels.
#[derive(Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({}x{}x{})", self.width, self.height, self.channels)
    }
}

impl Pixels {
    /// Grey and palette images become RGB; alpha is kept exactly when the
    /// image has an alpha channel (the PNG decoder turns `tRNS` into one);
    /// 16-bit channels keep the high byte.
    pub fn from_image(img: &DynamicImage) -> Pixels {
        use DynamicImage::*;
        let (width, height) = (img.width(), img.height());
        let rgb = |data| Pixels { width, height, channels: 3, data };
        let rgba = |data| Pixels { width, height, channels: 4, data };
        let hi = |v: &[u16]| v.iter().map(|x| (x >> 8) as u8).collect::<Vec<u8>>();
        match img {
            ImageRgb8(b) => rgb(b.as_raw().clone()),
            ImageRgba8(b) => rgba(b.as_raw().clone()),
            ImageLuma8(b) => rgb(b.as_raw().iter().flat_map(|&l| [l, l, l]).collect()),
            ImageLumaA8(b) => rgba(b.as_raw().chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()),
            ImageRgb16(b) => rgb(hi(b.as_raw())),
            ImageRgba16(b) => rgba(hi(b.as_raw())),
            ImageLuma16(b) => rgb(hi(b.as_raw()).into_iter().flat_map(|l| [l, l, l]).collect()),
            ImageLumaA16(b) => rgba(hi(b.as_raw()).chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()),
            other if other.color().has_alpha() => rgba(other.to_rgba8().into_raw()),
            other => rgb(other.to_rgb8().into_raw()),
        }
    }

    pub fn has_alpha(&self) -> bool {
        self.channels == 4
    }

    /// Whether the image is transparent (DESIGN.md, Transparency): at least
    /// 5% of its pixels have alpha below 200. Qwen-Image 2.1 returns RGBA for
    /// every prompt, with alpha 253–255 on ordinary images, so the channel
    /// count alone says nothing.
    pub fn is_transparent(&self) -> bool {
        if !self.has_alpha() {
            return false;
        }
        let total = self.data.len() / 4;
        let clear = self.data.as_chunks::<4>().0.iter().filter(|p| p[3] < 200).count();
        total > 0 && clear * 20 >= total
    }

    pub fn hash(&self) -> String {
        let mut h = Sha256::new();
        h.update(TAG_V1);
        h.update(self.width.to_be_bytes());
        h.update(self.height.to_be_bytes());
        h.update([self.channels]);
        h.update(&self.data);
        hex(&h.finalize())
    }

    /// BLAKE3 of the same canonical encoding as [`hash`](Self::hash).
    pub fn blake3(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(TAG_V1);
        h.update(&self.width.to_be_bytes());
        h.update(&self.height.to_be_bytes());
        h.update(&[self.channels]);
        h.update(&self.data);
        h.finalize().to_hex().to_string()
    }

    /// `None` if the data doesn't fill the size.
    pub fn into_image(self) -> Option<DynamicImage> {
        if self.has_alpha() {
            ImageBuffer::from_raw(self.width, self.height, self.data).map(DynamicImage::ImageRgba8)
        } else {
            ImageBuffer::from_raw(self.width, self.height, self.data).map(DynamicImage::ImageRgb8)
        }
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, ImageBuffer, Luma, Rgb, RgbImage, Rgba, RgbaImage};

    #[test]
    fn canonical_encoding() {
        let img = RgbImage::from_pixel(2, 1, Rgb([1, 2, 3]));
        let px = Pixels::from_image(&DynamicImage::ImageRgb8(img));
        let mut expect = Vec::from(TAG_V1);
        expect.extend([0, 0, 0, 2, 0, 0, 0, 1, 3, 1, 2, 3, 1, 2, 3]);
        assert_eq!(px.hash(), sha256_hex(&expect));
        assert_eq!(px.blake3(), blake3::hash(&expect).to_hex().as_str());
    }

    #[test]
    fn grey_expands_and_16_bit_keeps_high_byte() {
        let grey = Pixels::from_image(&DynamicImage::ImageLuma8(GrayImage::from_pixel(1, 1, Luma([7]))));
        assert_eq!((grey.channels, grey.data.as_slice()), (3, &[7, 7, 7][..]));
        let wide: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::from_pixel(1, 1, Rgb([0x12ff, 0x3400, 0xffff]));
        let px = Pixels::from_image(&DynamicImage::ImageRgb16(wide));
        assert_eq!(px.data, [0x12, 0x34, 0xff]);
    }

    #[test]
    fn transparent_needs_5_percent_below_200() {
        // 20 pixels: n of them at alpha 199, the rest at 253 (Qwen's "opaque")
        let px = |n: u32, a: u8| {
            let img = RgbaImage::from_fn(20, 1, |x, _| Rgba([0, 0, 0, if x < n { a } else { 253 }]));
            Pixels::from_image(&DynamicImage::ImageRgba8(img))
        };
        assert!(!px(0, 199).is_transparent());
        assert!(px(1, 199).is_transparent(), "exactly 5%");
        assert!(!px(20, 200).is_transparent(), "200 is not below 200");
        assert!(!Pixels::from_image(&DynamicImage::ImageRgb8(RgbImage::new(4, 4))).is_transparent());
    }

    #[test]
    fn opaque_alpha_still_counts() {
        let rgba = RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]));
        let a = Pixels::from_image(&DynamicImage::ImageRgba8(rgba));
        let b = Pixels::from_image(&DynamicImage::ImageRgb8(RgbImage::from_pixel(1, 1, Rgb([1, 2, 3]))));
        assert_eq!(a.channels, 4);
        assert_ne!(a.hash(), b.hash());
    }
}
