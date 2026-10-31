//! PNG chunk-level reading and writing: the `increart:*` text chunks and
//! `eXIf`, added or replaced without re-encoding the pixels.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::error::{Error, IoContext, Result};

pub const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

pub const KEY_ID: &str = "increart:id";
pub const KEY_PIXELS: &str = "increart:pixels";
pub const KEY_BLAKE3: &str = "increart:blake3";
pub const KEY_USAGE: &str = "increart:usage";
pub const KEY_SOURCE: &str = "increart:source";
pub const KEY_RECIPE: &str = "increart:recipe";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub kind: [u8; 4],
    pub data: Vec<u8>,
}

impl Chunk {
    /// `tEXt` (Latin-1; our keys and values there are ASCII).
    pub fn text(key: &str, value: &str) -> Chunk {
        let mut data = latin1(key);
        data.push(0);
        data.extend(latin1(value));
        Chunk { kind: *b"tEXt", data }
    }

    /// Uncompressed `iTXt` (UTF-8), no language tag.
    pub fn itxt(key: &str, value: &str) -> Chunk {
        let mut data = latin1(key);
        data.extend([0, 0, 0, 0, 0]);
        data.extend(value.as_bytes());
        Chunk { kind: *b"iTXt", data }
    }

    pub fn exif(raw: Vec<u8>) -> Chunk {
        Chunk { kind: *b"eXIf", data: raw }
    }

    /// `(key, value)` of a `tEXt` or uncompressed `iTXt` chunk.
    pub fn as_text(&self) -> Option<(String, String)> {
        let nul = self.data.iter().position(|&b| b == 0)?;
        let key = from_latin1(&self.data[..nul]);
        let rest = &self.data[nul + 1..];
        match &self.kind {
            b"tEXt" => Some((key, from_latin1(rest))),
            b"iTXt" => {
                let (&compressed, rest) = rest.split_first()?;
                if compressed != 0 {
                    return None;
                }
                let rest = rest.get(1..)?; // compression method
                let lang = rest.iter().position(|&b| b == 0)?;
                let rest = &rest[lang + 1..];
                let translated = rest.iter().position(|&b| b == 0)?;
                Some((key, String::from_utf8(rest[translated + 1..].to_vec()).ok()?))
            }
            _ => None,
        }
    }

    fn key(&self) -> Option<String> {
        self.as_text().map(|(k, _)| k)
    }
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect()
}

fn from_latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// Every chunk of a PNG file, checking signature and CRCs.
pub fn read_chunks(bytes: &[u8]) -> Result<Vec<Chunk>, String> {
    let mut out = Vec::new();
    parse(bytes, |c| {
        out.push(c);
        true
    })?;
    Ok(out)
}

/// A chunk's head: its data's length and its type.
fn chunk_head(h: [u8; 8]) -> (usize, [u8; 4]) {
    let [l0, l1, l2, l3, k0, k1, k2, k3] = h;
    (u32::from_be_bytes([l0, l1, l2, l3]) as usize, [k0, k1, k2, k3])
}

fn parse(bytes: &[u8], mut f: impl FnMut(Chunk) -> bool) -> Result<(), String> {
    if bytes.get(..8) != Some(SIGNATURE) {
        return Err("not a PNG file".into());
    }
    let mut i = 8;
    while i < bytes.len() {
        let len = u32::from_be_bytes(*bytes.get(i..).and_then(<[u8]>::first_chunk).ok_or("truncated")?) as usize;
        let body = bytes.get(i + 4..i + 8 + len).ok_or("truncated chunk")?;
        let crc = bytes.get(i + 8 + len..i + 12 + len).ok_or("truncated chunk")?;
        let (kind, data) = body.split_first_chunk::<4>().ok_or("truncated chunk")?;
        if crc32fast::hash(body).to_be_bytes() != crc {
            return Err(format!("bad CRC in {} chunk", String::from_utf8_lossy(kind)));
        }
        let chunk = Chunk { kind: *kind, data: data.to_vec() };
        let end = &chunk.kind == b"IEND";
        if !f(chunk) || end {
            return Ok(());
        }
        i += 12 + len;
    }
    Err("missing IEND".into())
}

pub fn write_chunks(chunks: &[Chunk]) -> Vec<u8> {
    let mut out = SIGNATURE.to_vec();
    for c in chunks {
        out.extend((c.data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend(c.kind);
        out.extend(&c.data);
        let crc = crc32fast::hash(&out[start..]);
        out.extend(crc.to_be_bytes());
    }
    out
}

/// Inserts `new` before the first `IDAT`, replacing text chunks with the same
/// keys (and any `eXIf`, if `new` has one).
pub fn set_chunks(bytes: &[u8], new: &[Chunk]) -> Result<Vec<u8>, String> {
    let keys: Vec<String> = new.iter().filter_map(Chunk::key).collect();
    let new_exif = new.iter().any(|c| &c.kind == b"eXIf");
    let mut out = Vec::new();
    let mut inserted = false;
    for c in read_chunks(bytes)? {
        if c.key().is_some_and(|k| keys.contains(&k)) || (new_exif && &c.kind == b"eXIf") {
            continue;
        }
        if &c.kind == b"IDAT" && !inserted {
            out.extend(new.iter().cloned());
            inserted = true;
        }
        out.push(c);
    }
    if !inserted {
        return Err("no IDAT chunk".into());
    }
    Ok(write_chunks(&out))
}

/// Text chunks (`tEXt`, uncompressed `iTXt`) of PNG bytes.
pub fn texts(bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    Ok(read_chunks(bytes)?.iter().filter_map(Chunk::as_text).collect())
}

pub fn text(bytes: &[u8], key: &str) -> Result<Option<String>, String> {
    Ok(texts(bytes)?.into_iter().find(|(k, _)| k == key).map(|(_, v)| v))
}

/// Text chunks before the first `IDAT` of a file, reading only that far
/// (enough for `increart:id` / `increart:pixels`, which IncreArt always
/// writes there).
pub fn header_texts(path: &Path) -> Result<Vec<(String, String)>> {
    let mut r = BufReader::new(File::open(path).at(path)?);
    let mut sig = [0; 8];
    r.read_exact(&mut sig).at(path)?;
    if &sig != SIGNATURE {
        return Err(Error::format(path, "not a PNG file"));
    }
    let mut out = Vec::new();
    loop {
        let mut head = [0; 8];
        if r.read_exact(&mut head).is_err() {
            return Ok(out);
        }
        let (len, kind) = chunk_head(head);
        if &kind == b"IDAT" || &kind == b"IEND" {
            return Ok(out);
        }
        let mut data = vec![0; len + 4];
        r.read_exact(&mut data).at(path)?;
        data.truncate(len);
        if let Some(t) = (Chunk { kind, data }).as_text() {
            out.push(t);
        }
    }
}

pub fn header_text(path: &Path, key: &str) -> Result<Option<String>> {
    Ok(header_texts(path)?.into_iter().find(|(k, _)| k == key).map(|(_, v)| v))
}

/// How many runs ever used an image, as `increart:usage` (`tEXt`):
/// `ref=000000003 start=000000001`, always 29 bytes, so a new count is
/// written over the old one in place ([`patch_chunk`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsageCounts {
    /// Runs that named it as a reference.
    pub refs: u64,
    /// Runs that started from it (img2img).
    pub starts: u64,
}

impl UsageCounts {
    pub const MAX: u64 = 999_999_999;

    pub fn text(&self) -> String {
        format!("ref={:09} start={:09}", self.refs.min(Self::MAX), self.starts.min(Self::MAX))
    }

    /// The exact format only.
    pub fn parse(s: &str) -> Option<UsageCounts> {
        let digits =
            |d: &str| (d.len() == 9 && d.bytes().all(|b| b.is_ascii_digit())).then(|| d.parse().ok()).flatten();
        let (r, st) = s.strip_prefix("ref=")?.split_once(" start=")?;
        Some(UsageCounts { refs: digits(r)?, starts: digits(st)? })
    }

    pub fn chunk(&self) -> Chunk {
        Chunk::text(KEY_USAGE, &self.text())
    }

    /// Each count at least the other's.
    pub fn max(self, o: UsageCounts) -> UsageCounts {
        UsageCounts { refs: self.refs.max(o.refs), starts: self.starts.max(o.starts) }
    }
}

/// A text chunk before the first `IDAT` of a file: where it starts (its
/// length field), the chunk, and whether its CRC is right.
pub fn find_header_chunk(path: &Path, key: &str) -> Result<Option<(u64, Chunk, bool)>> {
    let mut r = BufReader::new(File::open(path).at(path)?);
    let mut sig = [0; 8];
    r.read_exact(&mut sig).at(path)?;
    if &sig != SIGNATURE {
        return Err(Error::format(path, "not a PNG file"));
    }
    let mut at = 8u64;
    loop {
        let mut head = [0; 8];
        if r.read_exact(&mut head).is_err() {
            return Ok(None);
        }
        let (len, kind) = chunk_head(head);
        if &kind == b"IDAT" || &kind == b"IEND" {
            return Ok(None);
        }
        let mut data = vec![0; len + 4];
        r.read_exact(&mut data).at(path)?;
        let crc = data.split_off(len);
        let mut body = kind.to_vec();
        body.extend(&data);
        let ok = crc32fast::hash(&body).to_be_bytes() == crc[..];
        let chunk = Chunk { kind, data };
        // a torn chunk may not parse as text: match its key by its bytes
        if chunk.data.starts_with(key.as_bytes()) && chunk.data.get(key.len()) == Some(&0) {
            return Ok(Some((at, chunk, ok)));
        }
        at += 12 + len as u64;
    }
}

/// Writes `chunk` over the chunk starting at `at`, which has the same length:
/// its type, data and CRC in one write, then a sync. The file keeps its size.
pub fn patch_chunk(path: &Path, at: u64, chunk: &Chunk) -> Result<()> {
    let f = std::fs::OpenOptions::new().read(true).write(true).open(path).at(path)?;
    let mut len = [0; 4];
    crate::fsutil::read_exact_at(&f, &mut len, at).at(path)?;
    if u32::from_be_bytes(len) as usize != chunk.data.len() {
        return Err(Error::format(path, "chunk to patch has another length"));
    }
    let mut bytes = chunk.kind.to_vec();
    bytes.extend(&chunk.data);
    let crc = crc32fast::hash(&bytes);
    bytes.extend(crc.to_be_bytes());
    crate::fsutil::write_all_at(&f, &bytes, at + 4).and_then(|_| crate::fsutil::sync(&f)).at(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_counts_are_fixed_width() {
        let c = UsageCounts { refs: 3, starts: 12 };
        assert_eq!(c.text(), "ref=000000003 start=000000012");
        assert_eq!(UsageCounts::parse(&c.text()), Some(c));
        assert_eq!(UsageCounts { refs: u64::MAX, starts: 0 }.text().len(), 29);
        for bad in ["ref=3 start=12", "ref=00000000x start=000000012", "ref=000000003  start=000000012", ""] {
            assert_eq!(UsageCounts::parse(bad), None, "{bad}");
        }
    }

    fn tiny_png() -> Vec<u8> {
        let mut out = Vec::new();
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([9, 8, 7]));
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn insert_replace_and_read_back() {
        let png = tiny_png();
        let a = set_chunks(&png, &[Chunk::text(KEY_ID, "x"), Chunk::itxt(KEY_RECIPE, "Recipe { é }")]).unwrap();
        assert_eq!(text(&a, KEY_ID).unwrap().as_deref(), Some("x"));
        assert_eq!(text(&a, KEY_RECIPE).unwrap().as_deref(), Some("Recipe { é }"));
        let b = set_chunks(&a, &[Chunk::text(KEY_ID, "y")]).unwrap();
        let t = texts(&b).unwrap();
        assert_eq!(t.iter().filter(|(k, _)| k == KEY_ID).count(), 1);
        assert_eq!(text(&b, KEY_ID).unwrap().as_deref(), Some("y"));
        // pixels unchanged, still decodes
        let img = image::load_from_memory(&b).unwrap().to_rgb8();
        assert_eq!(img.get_pixel(1, 1).0, [9, 8, 7]);
        // chunks land before IDAT
        let kinds: Vec<_> = read_chunks(&b).unwrap().iter().map(|c| c.kind).collect();
        let idat = kinds.iter().position(|k| k == b"IDAT").unwrap();
        assert!(kinds[..idat].contains(b"tEXt"));
    }

    #[test]
    fn bad_crc_is_rejected() {
        let mut png = tiny_png();
        let n = png.len();
        png[n - 20] ^= 1;
        assert!(read_chunks(&png).is_err());
    }
}
