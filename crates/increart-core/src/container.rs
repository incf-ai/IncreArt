//! Store files in either format, PNG or lossless WebP (DESIGN.md, Image
//! stores): their metadata (PNG text chunks and `eXIf`) read and replaced
//! without re-encoding the pixels, and pixels encoded as either.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::error::{Error, IoContext, Result};
use crate::import;
use crate::model::StoreFormat;
use crate::pixel::Pixels;
use crate::png::{self, Chunk};
use crate::webp;

pub fn format_of(bytes: &[u8]) -> Option<StoreFormat> {
    if bytes.starts_with(png::SIGNATURE) {
        Some(StoreFormat::Png)
    } else if webp::is_webp(bytes) {
        Some(StoreFormat::Webp)
    } else {
        None
    }
}

fn file_format(path: &Path) -> Result<StoreFormat> {
    let mut head = Vec::with_capacity(12);
    File::open(path).and_then(|f| f.take(12).read_to_end(&mut head)).at(path)?;
    format_of(&head).ok_or_else(|| Error::format(path, "neither PNG nor WebP"))
}

fn is_meta(c: &Chunk) -> bool {
    matches!(&c.kind, b"tEXt" | b"iTXt" | b"zTXt" | b"eXIf")
}

/// The metadata chunks: text chunks and `eXIf`.
pub fn metadata(bytes: &[u8]) -> Result<Vec<Chunk>, String> {
    match format_of(bytes) {
        Some(StoreFormat::Png) => Ok(png::read_chunks(bytes)?.into_iter().filter(is_meta).collect()),
        Some(StoreFormat::Webp) => webp::read_chunks(bytes),
        None => Err("neither PNG nor WebP".into()),
    }
}

pub fn texts(bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    Ok(metadata(bytes)?.iter().filter_map(Chunk::as_text).collect())
}

pub fn text(bytes: &[u8], key: &str) -> Result<Option<String>, String> {
    Ok(texts(bytes)?.into_iter().find(|(k, _)| k == key).map(|(_, v)| v))
}

/// Adds `new`, replacing text chunks with the same keys (and any `eXIf`, if
/// `new` has one).
pub fn set_chunks(bytes: &[u8], new: &[Chunk]) -> Result<Vec<u8>, String> {
    match format_of(bytes) {
        Some(StoreFormat::Png) => png::set_chunks(bytes, new),
        Some(StoreFormat::Webp) => webp::set_chunks(bytes, new),
        None => Err("neither PNG nor WebP".into()),
    }
}

/// The file without its metadata chunks.
pub fn strip_metadata(bytes: &[u8]) -> Result<Vec<u8>, String> {
    match format_of(bytes) {
        Some(StoreFormat::Png) => {
            let chunks = png::read_chunks(bytes)?;
            Ok(png::write_chunks(&chunks.into_iter().filter(|c| !is_meta(c)).collect::<Vec<_>>()))
        }
        Some(StoreFormat::Webp) => webp::strip(bytes),
        None => Err("neither PNG nor WebP".into()),
    }
}

/// Text chunks before the image data of a file, reading only that far
/// (where IncreArt always writes its own).
pub fn header_texts(path: &Path) -> Result<Vec<(String, String)>> {
    match file_format(path)? {
        StoreFormat::Png => png::header_texts(path),
        StoreFormat::Webp => webp::header_texts(path),
    }
}

pub fn header_text(path: &Path, key: &str) -> Result<Option<String>> {
    Ok(header_texts(path)?.into_iter().find(|(k, _)| k == key).map(|(_, v)| v))
}

/// A text chunk before the image data: where it starts, the chunk, and
/// whether its CRC is right (for [`patch_chunk`]).
pub fn find_header_chunk(path: &Path, key: &str) -> Result<Option<(u64, Chunk, bool)>> {
    match file_format(path)? {
        StoreFormat::Png => png::find_header_chunk(path, key),
        StoreFormat::Webp => webp::find_header_chunk(path, key),
    }
}

/// Writes `chunk` in place over the one [`find_header_chunk`] found at `at`
/// (same length).
pub fn patch_chunk(path: &Path, at: u64, chunk: &Chunk) -> Result<()> {
    match file_format(path)? {
        StoreFormat::Png => png::patch_chunk(path, at, chunk),
        StoreFormat::Webp => webp::patch_chunk(path, at, chunk),
    }
}

/// Encodes pixels as `format` with `chunks`. WebP can't hold a side over
/// 16383 pixels: such an image is PNG whatever was asked. Returns the
/// format used.
pub fn encode(format: StoreFormat, px: &Pixels, chunks: &[Chunk]) -> (StoreFormat, Vec<u8>) {
    if format == StoreFormat::Webp
        && let Some(bytes) = webp::encode(px, chunks)
    {
        return (StoreFormat::Webp, bytes);
    }
    (StoreFormat::Png, import::encode_png(px, chunks))
}

/// A store file re-encoded as `to` with the same pixels and metadata,
/// checked by decoding it again. Returns the format used (see [`encode`])
/// and the pixels' hash.
pub fn transcode(bytes: &[u8], to: StoreFormat) -> Result<(StoreFormat, Vec<u8>, String), String> {
    let px = import::decode_pixels(bytes).map_err(|e| e.to_string())?;
    let chunks = metadata(bytes)?;
    let (format, out) = encode(to, &px, &chunks);
    let hash = px.hash();
    let again = import::decode_pixels(&out).map_err(|e| e.to_string())?.hash();
    if again != hash {
        return Err(format!("{} pixels differ after encoding", format.name()));
    }
    Ok((format, out, hash))
}

/// PNG bytes for a store file (servers read PNG, not WebP): itself if it is
/// one, else its pixels quickly encoded, without metadata.
pub fn as_png(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if format_of(&bytes) == Some(StoreFormat::Png) {
        return Ok(bytes);
    }
    let px = import::decode_pixels(&bytes).map_err(|e| e.to_string())?;
    Ok(import::encode_png_fast(&px))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::png::{KEY_ID, KEY_RECIPE};

    #[test]
    fn transcode_both_ways_keeps_pixels_and_metadata() {
        let px = Pixels { width: 7, height: 5, channels: 4, data: (0..140).map(|i| (i * 13) as u8).collect() };
        let chunks = [Chunk::text(KEY_ID, "x"), Chunk::itxt(KEY_RECIPE, "Recipe { é }"), Chunk::exif(b"MM".to_vec())];
        let (f, png_bytes) = encode(StoreFormat::Png, &px, &chunks);
        assert_eq!(f, StoreFormat::Png);
        let (f, webp_bytes, hash) = transcode(&png_bytes, StoreFormat::Webp).unwrap();
        assert_eq!((f, hash), (StoreFormat::Webp, px.hash()));
        assert_eq!(metadata(&webp_bytes).unwrap(), chunks);
        let (f, back, _) = transcode(&webp_bytes, StoreFormat::Png).unwrap();
        assert_eq!(f, StoreFormat::Png);
        assert_eq!(metadata(&back).unwrap(), chunks);
        assert_eq!(import::decode_pixels(&back).unwrap().hash(), px.hash());
        let plain = as_png(webp_bytes).unwrap();
        assert_eq!(format_of(&plain), Some(StoreFormat::Png));
        assert_eq!(import::decode_pixels(&plain).unwrap().hash(), px.hash());
    }

    #[test]
    fn cut_short_or_damaged_files_are_errors_not_panics() {
        let px = Pixels { width: 3, height: 2, channels: 4, data: (0..24).map(|i| (i * 7) as u8).collect() };
        let chunks = [Chunk::text(KEY_ID, "x"), Chunk::exif(b"MM".to_vec())];
        for format in [StoreFormat::Png, StoreFormat::Webp] {
            let (_, bytes) = encode(format, &px, &chunks);
            let damaged = (0..bytes.len()).map(|i| {
                let mut b = bytes.clone();
                b[i] ^= 0xff;
                b
            });
            for b in (0..bytes.len()).map(|n| bytes[..n].to_vec()).chain(damaged) {
                let _ = metadata(&b);
                let _ = strip_metadata(&b);
                let _ = set_chunks(&b, &chunks);
            }
        }
    }
}
