//! Turning an outside image file into input store pixels (DESIGN.md, Image
//! stores, import steps 1–3).

use std::io::Cursor;
use std::path::Path;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use crate::id::Timestamp;
use crate::model::SourceInfo;
use crate::pixel::{Pixels, sha256_hex};
use crate::png::{self, Chunk};

/// An original file's details, as far as the caller knows them.
#[derive(Clone, Debug, Default)]
pub struct Original {
    pub name: Option<String>,
    pub path: Option<String>,
    pub mtime: Option<Timestamp>,
    pub ctime: Option<Timestamp>,
    pub atime: Option<Timestamp>,
    pub created: Option<Timestamp>,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

impl Original {
    /// The name, path, times, permissions and owner of a file. Call it
    /// before reading the file, which may change its access time.
    pub fn of_file(path: &Path, abs: &Path) -> Original {
        let whole = |t: std::time::SystemTime| {
            use chrono::SubsecRound;
            Timestamp::from(t).trunc_subsecs(0)
        };
        let meta = std::fs::metadata(path).ok();
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut o = Original {
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()),
            path: Some(abs.to_string_lossy().into_owned()),
            mtime: meta.as_ref().and_then(|m| m.modified().ok()).map(whole),
            atime: meta.as_ref().and_then(|m| m.accessed().ok()).map(whole),
            created: meta.as_ref().and_then(|m| m.created().ok()).map(whole),
            ctime: None,
            mode: None,
            uid: None,
            gid: None,
        };
        // the inode change time, permissions and owner exist on Unix only
        #[cfg(unix)]
        if let Some(m) = &meta {
            use std::os::unix::fs::MetadataExt;
            o.ctime = chrono::DateTime::from_timestamp(m.ctime(), 0);
            o.mode = Some(m.mode() & 0o7777);
            o.uid = Some(m.uid());
            o.gid = Some(m.gid());
        }
        o
    }
}

/// A decoded, converted image ready for the input store.
pub struct Prepared {
    pub pixels: Pixels,
    pub pixel_hash: String,
    pub blake3: String,
    /// `imported` is left `None`; set when the store file is written.
    pub source: SourceInfo,
    pub exif: Option<Vec<u8>>,
}

pub fn prepare(bytes: &[u8], original: &Original) -> Result<Prepared, image::ImageError> {
    let format = image::guess_format(bytes)?;
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format).into_decoder()?;
    let icc = decoder.icc_profile()?;
    let exif = decoder.exif_metadata()?;
    let orientation = decoder.orientation()?;
    let (original_width, original_height) = decoder.dimensions();
    let bits16 = decoder.color_type().bytes_per_pixel() / decoder.color_type().channel_count() > 1;
    let mut img = DynamicImage::from_decoder(decoder)?;

    let mut conversions = Vec::new();
    if orientation != Orientation::NoTransforms {
        img.apply_orientation(orientation);
        conversions.push(format!("exif-orientation: {}", orientation_name(orientation)));
    }
    if bits16 {
        conversions.push("16-bit -> 8-bit".into());
    }
    let mut pixels = Pixels::from_image(&img);
    if let Some(icc) = icc
        && let Some(note) = to_srgb(&icc, &mut pixels)
    {
        conversions.push(note);
    }
    if format != ImageFormat::Png {
        conversions.push(format!("{} -> png", format_name(format)));
    }

    let pixel_hash = pixels.hash();
    let blake3 = pixels.blake3();
    let source = SourceInfo {
        version: 1,
        original_name: original.name.clone(),
        original_path: original.path.clone(),
        original_format: format_name(format),
        original_size: bytes.len() as u64,
        original_sha256: sha256_hex(bytes),
        original_mtime: original.mtime,
        original_ctime: original.ctime,
        original_atime: original.atime,
        original_created: original.created,
        original_mode: original.mode,
        original_uid: original.uid,
        original_gid: original.gid,
        original_width,
        original_height,
        imported: None,
        conversions,
    };
    Ok(Prepared { pixels, pixel_hash, blake3, source, exif })
}

/// Converts 8-bit pixels from an embedded ICC profile to sRGB. Returns the
/// conversion note, or `None` when nothing was done (already sRGB, not an
/// RGB profile, or a profile moxcms can't use).
fn to_srgb(icc: &[u8], px: &mut Pixels) -> Option<String> {
    use moxcms::{ColorProfile, DataColorSpace, Layout, TransformOptions};
    let profile = ColorProfile::new_from_slice(icc).ok()?;
    if profile.color_space != DataColorSpace::Rgb {
        return None;
    }
    let name = profile_name(&profile).unwrap_or_else(|| "embedded profile".into());
    if name.to_ascii_lowercase().contains("srgb") {
        return None;
    }
    let layout = if px.has_alpha() { Layout::Rgba } else { Layout::Rgb };
    let srgb = ColorProfile::new_srgb();
    let t = profile.create_transform_8bit(layout, &srgb, layout, TransformOptions::default()).ok()?;
    let mut out = vec![0; px.data.len()];
    t.transform(&px.data, &mut out).ok()?;
    px.data = out;
    Some(format!("icc: {name} -> sRGB"))
}

fn profile_name(p: &moxcms::ColorProfile) -> Option<String> {
    use moxcms::ProfileText;
    let s = match p.description.as_ref()? {
        ProfileText::PlainString(s) => s.clone(),
        ProfileText::Localizable(v) => v.first()?.value.clone(),
        ProfileText::Description(d) => d.ascii_string.clone(),
    };
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
    (!s.is_empty()).then_some(s)
}

fn orientation_name(o: Orientation) -> &'static str {
    match o {
        Orientation::NoTransforms => "none",
        Orientation::Rotate90 => "rotate 90",
        Orientation::Rotate180 => "rotate 180",
        Orientation::Rotate270 => "rotate 270",
        Orientation::FlipHorizontal => "flip horizontal",
        Orientation::FlipVertical => "flip vertical",
        Orientation::Rotate90FlipH => "rotate 90, flip horizontal",
        Orientation::Rotate270FlipH => "rotate 270, flip horizontal",
    }
}

fn format_name(f: ImageFormat) -> String {
    match f {
        ImageFormat::Jpeg => "jpeg".into(),
        other => other.extensions_str().first().copied().unwrap_or("unknown").into(),
    }
}

/// Encodes canonical pixels as PNG and adds `chunks` before `IDAT`.
pub fn encode_png(pixels: &Pixels, chunks: &[Chunk]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    );
    let color = if pixels.has_alpha() { image::ExtendedColorType::Rgba8 } else { image::ExtendedColorType::Rgb8 };
    image::ImageEncoder::write_image(enc, &pixels.data, pixels.width, pixels.height, color)
        .map_err(|e| e.to_string())?;
    png::set_chunks(&out, chunks)
}

/// Canonical pixels as PNG, quickly and without chunks (for servers, which
/// don't read WebP).
pub fn encode_png_fast(pixels: &Pixels) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    );
    let color = if pixels.has_alpha() { image::ExtendedColorType::Rgba8 } else { image::ExtendedColorType::Rgb8 };
    image::ImageEncoder::write_image(enc, &pixels.data, pixels.width, pixels.height, color)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Decodes a store file (PNG or WebP) or server output to canonical pixels.
pub fn decode_pixels(bytes: &[u8]) -> Result<Pixels, image::ImageError> {
    Ok(Pixels::from_image(&image::load_from_memory(bytes)?))
}
