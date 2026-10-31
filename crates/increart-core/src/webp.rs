//! WebP store files: lossless, the pixels encoded by libwebp, the container
//! written here so IncreArt's metadata travels with them. Each PNG text
//! chunk (`tEXt`, `iTXt`, `zTXt`) is kept whole in a `PNGt` chunk (its type,
//! data and CRC as in PNG, so a torn write still shows), `eXIf` as `EXIF`.
//!
//! Layout (the extended format): `RIFF` size `WEBP`, `VP8X`, the `PNGt`
//! chunks, `VP8L`, `EXIF`. `VP8X`'s alpha flag follows the pixels' channel
//! count, so an opaque RGBA image decodes as RGBA again (the pixel hash
//! counts channels). Decoders skip the unknown `PNGt` chunks.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::error::{Error, IoContext, Result};
use crate::pixel::Pixels;
use crate::png::Chunk;

pub const PNG_CHUNK: [u8; 4] = *b"PNGt";

/// The largest width or height WebP can hold.
pub const MAX_SIDE: u32 = 16383;

const ALPHA: u8 = 0x10;
const EXIF: u8 = 0x08;

pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// Encodes pixels losslessly with `chunks` (PNG text chunks and `eXIf`).
/// `None` when a side is over [`MAX_SIDE`].
pub fn encode(px: &Pixels, chunks: &[Chunk]) -> Option<Vec<u8>> {
    if px.width > MAX_SIDE || px.height > MAX_SIDE || px.width == 0 || px.height == 0 {
        return None;
    }
    let enc = if px.has_alpha() {
        webp::Encoder::from_rgba(&px.data, px.width, px.height)
    } else {
        webp::Encoder::from_rgb(&px.data, px.width, px.height)
    };
    let mut cfg = webp::WebPConfig::new().expect("libwebp config");
    cfg.lossless = 1;
    // method 4 at quality 75: about 2 % larger than the slowest setting,
    // 30 times faster (~0.2 s for a 1152×512 image)
    cfg.method = 4;
    cfg.quality = 75.0;
    // keep the colour under fully transparent pixels (it's in the hash)
    cfg.exact = 1;
    let out = enc.encode_advanced(&cfg).ok()?;
    let image = riff_chunks(&out).ok()?.into_iter().find(|(k, _)| k == b"VP8L")?.1.to_vec();
    Some(assemble(&Parts { alpha: px.has_alpha(), width: px.width, height: px.height, meta: chunks.to_vec(), image }))
}

struct Parts {
    alpha: bool,
    width: u32,
    height: u32,
    /// PNG text chunks and `eXIf`, in order.
    meta: Vec<Chunk>,
    /// The `VP8L` payload.
    image: Vec<u8>,
}

/// A RIFF chunk: its type and data.
type RiffChunk<'a> = ([u8; 4], &'a [u8]);

/// A RIFF chunk's head: its type and its data's length.
fn chunk_head(h: [u8; 8]) -> ([u8; 4], usize) {
    let [k0, k1, k2, k3, l0, l1, l2, l3] = h;
    ([k0, k1, k2, k3], u32::from_le_bytes([l0, l1, l2, l3]) as usize)
}

/// The 8 bytes at `i`, if there are.
fn head_at(bytes: &[u8], i: usize) -> Option<[u8; 8]> {
    bytes.get(i..).and_then(<[u8]>::first_chunk).copied()
}

fn riff_chunks(bytes: &[u8]) -> Result<Vec<RiffChunk<'_>>, String> {
    if !is_webp(bytes) {
        return Err("not a WebP file".into());
    }
    let (_, size) = chunk_head(head_at(bytes, 0).ok_or("truncated")?);
    let end = 8 + size;
    if end > bytes.len() {
        return Err("truncated".into());
    }
    let mut out = Vec::new();
    let mut i = 12;
    while i + 8 <= end {
        let (kind, len) = chunk_head(head_at(bytes, i).ok_or("truncated chunk")?);
        let data = bytes.get(i + 8..i + 8 + len).filter(|_| i + 8 + len <= end).ok_or("truncated chunk")?;
        out.push((kind, data));
        i += 8 + len + (len & 1);
    }
    Ok(out)
}

fn parse(bytes: &[u8]) -> Result<Parts, String> {
    let mut vp8x = None;
    let mut meta = Vec::new();
    let mut image = None;
    for (kind, d) in riff_chunks(bytes)? {
        match &kind {
            b"VP8X" if d.len() >= 10 => {
                let u24 = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], 0]) + 1;
                vp8x = Some((d[0] & ALPHA != 0, u24(&d[4..7]), u24(&d[7..10])));
            }
            b"PNGt" => meta.push(unwrap(d).ok_or("bad CRC in a PNGt chunk")?),
            b"EXIF" => meta.push(Chunk::exif(d.to_vec())),
            b"VP8L" => image = Some(d.to_vec()),
            _ => return Err(format!("unexpected {} chunk", String::from_utf8_lossy(&kind))),
        }
    }
    let ((alpha, width, height), image) = vp8x.zip(image).ok_or("not a lossless extended WebP")?;
    Ok(Parts { alpha, width, height, meta, image })
}

fn crc(kind: &[u8; 4], data: &[u8]) -> [u8; 4] {
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    h.finalize().to_be_bytes()
}

fn wrap(c: &Chunk) -> Vec<u8> {
    let mut out = c.kind.to_vec();
    out.extend(&c.data);
    out.extend(crc(&c.kind, &c.data));
    out
}

/// A `PNGt` chunk's PNG chunk and whether its CRC is right.
fn unwrap_checked(d: &[u8]) -> Option<(Chunk, bool)> {
    let (kind, rest) = d.split_first_chunk::<4>()?;
    let (data, sum) = rest.split_last_chunk::<4>()?;
    Some((Chunk { kind: *kind, data: data.to_vec() }, crc(kind, data) == *sum))
}

fn unwrap(d: &[u8]) -> Option<Chunk> {
    unwrap_checked(d).filter(|(_, ok)| *ok).map(|(c, _)| c)
}

fn push(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend(kind);
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}

fn assemble(p: &Parts) -> Vec<u8> {
    let exif = p.meta.iter().find(|c| &c.kind == b"eXIf");
    let mut vp8x = vec![0; 10];
    vp8x[0] = if p.alpha { ALPHA } else { 0 } | if exif.is_some() { EXIF } else { 0 };
    vp8x[4..7].copy_from_slice(&(p.width - 1).to_le_bytes()[..3]);
    vp8x[7..10].copy_from_slice(&(p.height - 1).to_le_bytes()[..3]);
    let mut body = b"WEBP".to_vec();
    push(&mut body, b"VP8X", &vp8x);
    for c in p.meta.iter().filter(|c| &c.kind != b"eXIf") {
        push(&mut body, &PNG_CHUNK, &wrap(c));
    }
    push(&mut body, b"VP8L", &p.image);
    if let Some(e) = exif {
        push(&mut body, b"EXIF", &e.data);
    }
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

/// The metadata chunks: PNG text chunks and `eXIf`.
pub fn read_chunks(bytes: &[u8]) -> Result<Vec<Chunk>, String> {
    Ok(parse(bytes)?.meta)
}

/// Adds `new`, replacing text chunks with the same keys (and any `eXIf`, if
/// `new` has one). The pixels aren't re-encoded.
pub fn set_chunks(bytes: &[u8], new: &[Chunk]) -> Result<Vec<u8>, String> {
    let mut p = parse(bytes)?;
    let keys: Vec<String> = new.iter().filter_map(|c| c.as_text().map(|(k, _)| k)).collect();
    let new_exif = new.iter().any(|c| &c.kind == b"eXIf");
    p.meta.retain(|c| !(c.as_text().is_some_and(|(k, _)| keys.contains(&k)) || (new_exif && &c.kind == b"eXIf")));
    p.meta.extend(new.iter().cloned());
    Ok(assemble(&p))
}

/// The file without metadata chunks.
pub fn strip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut p = parse(bytes)?;
    p.meta.clear();
    Ok(assemble(&p))
}

/// Calls `f` with each chunk before the image data (offset of its header,
/// type, data), reading only that far; stops when `f` returns false.
fn walk_header(path: &Path, mut f: impl FnMut(u64, [u8; 4], Vec<u8>) -> bool) -> Result<()> {
    let mut r = BufReader::new(File::open(path).at(path)?);
    let mut head = [0; 12];
    r.read_exact(&mut head).at(path)?;
    if !is_webp(&head) {
        return Err(Error::format(path, "not a WebP file"));
    }
    let mut at = 12u64;
    loop {
        let mut h = [0; 8];
        if r.read_exact(&mut h).is_err() {
            return Ok(());
        }
        let (kind, len) = chunk_head(h);
        if matches!(&kind, b"VP8L" | b"VP8 " | b"ALPH" | b"ANIM" | b"ANMF") {
            return Ok(());
        }
        let padded = len + (len & 1);
        let data = if &kind == b"PNGt" {
            let mut d = vec![0; padded];
            r.read_exact(&mut d).at(path)?;
            d.truncate(len);
            d
        } else {
            r.seek_relative(padded as i64).at(path)?;
            vec![]
        };
        if !f(at, kind, data) {
            return Ok(());
        }
        at += 8 + padded as u64;
    }
}

/// Text chunks before the image data of a file, reading only that far.
pub fn header_texts(path: &Path) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    walk_header(path, |_, kind, d| {
        if &kind == b"PNGt"
            && let Some((c, _)) = unwrap_checked(&d)
            && let Some(t) = c.as_text()
        {
            out.push(t);
        }
        true
    })?;
    Ok(out)
}

/// A text chunk before the image data: where its `PNGt` chunk starts, the
/// chunk, and whether its CRC is right.
pub fn find_header_chunk(path: &Path, key: &str) -> Result<Option<(u64, Chunk, bool)>> {
    let mut found = None;
    walk_header(path, |at, kind, d| {
        if &kind == b"PNGt"
            && let Some((c, ok)) = unwrap_checked(&d)
            // a torn chunk may not parse as text: match its key by its bytes
            && c.data.starts_with(key.as_bytes())
            && c.data.get(key.len()) == Some(&0)
        {
            found = Some((at, c, ok));
            return false;
        }
        true
    })?;
    Ok(found)
}

/// Writes `chunk` over the `PNGt` chunk starting at `at`, which has the same
/// length, in one write, then a sync. The file keeps its size.
pub fn patch_chunk(path: &Path, at: u64, chunk: &Chunk) -> Result<()> {
    let f = std::fs::OpenOptions::new().read(true).write(true).open(path).at(path)?;
    let mut head = [0; 8];
    crate::fsutil::read_exact_at(&f, &mut head, at).at(path)?;
    let bytes = wrap(chunk);
    let (kind, len) = chunk_head(head);
    if &kind != b"PNGt" || len != bytes.len() {
        return Err(Error::format(path, "chunk to patch has another length"));
    }
    crate::fsutil::write_all_at(&f, &bytes, at + 8).and_then(|_| crate::fsutil::sync(&f)).at(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::png::{KEY_ID, KEY_RECIPE, KEY_USAGE, UsageCounts};

    fn decode(bytes: &[u8]) -> Pixels {
        Pixels::from_image(&image::load_from_memory(bytes).unwrap())
    }

    fn rgba(opaque: bool) -> Pixels {
        let data = (0..6 * 4u32)
            .flat_map(|i| [(i * 9) as u8, (i * 5) as u8, 200, if opaque { 255 } else { (i * 11) as u8 }])
            .collect();
        Pixels { width: 6, height: 4, channels: 4, data }
    }

    #[test]
    fn pixels_survive_exactly() {
        let rgb = Pixels { width: 5, height: 3, channels: 3, data: (0..45).map(|i| (i * 7) as u8).collect() };
        // an opaque RGBA image stays RGBA; colour under alpha 0 is kept
        for px in [rgb, rgba(true), rgba(false)] {
            let bytes = encode(&px, &[]).unwrap();
            assert_eq!(decode(&bytes).hash(), px.hash(), "{px:?}");
        }
    }

    #[test]
    fn chunks_set_read_strip() {
        let px = rgba(false);
        let a = encode(&px, &[Chunk::text(KEY_ID, "x"), Chunk::exif(b"MM\0\x2a".to_vec())]).unwrap();
        let b = set_chunks(&a, &[Chunk::text(KEY_ID, "y"), Chunk::itxt(KEY_RECIPE, "Recipe { é }")]).unwrap();
        let texts: Vec<_> = read_chunks(&b).unwrap().iter().filter_map(Chunk::as_text).collect();
        assert_eq!(texts, [(KEY_ID.into(), "y".into()), (KEY_RECIPE.into(), "Recipe { é }".into())]);
        assert!(read_chunks(&b).unwrap().iter().any(|c| &c.kind == b"eXIf"));
        assert_eq!(decode(&b).hash(), px.hash());
        let bare = strip(&b).unwrap();
        assert!(read_chunks(&bare).unwrap().is_empty());
        assert_eq!(decode(&bare).hash(), px.hash());
    }

    #[test]
    fn header_and_patch_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.webp");
        let usage = UsageCounts { refs: 1, starts: 0 };
        std::fs::write(&path, encode(&rgba(true), &[Chunk::text(KEY_ID, "x"), usage.chunk()]).unwrap()).unwrap();
        assert_eq!(header_texts(&path).unwrap()[0], (KEY_ID.into(), "x".into()));
        let (at, c, ok) = find_header_chunk(&path, KEY_USAGE).unwrap().unwrap();
        assert!(ok && c == usage.chunk());
        let more = UsageCounts { refs: 2, starts: 7 };
        let len = std::fs::metadata(&path).unwrap().len();
        patch_chunk(&path, at, &more.chunk()).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), len);
        let (_, c, ok) = find_header_chunk(&path, KEY_USAGE).unwrap().unwrap();
        assert!(ok && c == more.chunk());
        // a torn write shows as a bad CRC
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[at as usize + 8 + 4 + KEY_USAGE.len() + 6] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert!(!find_header_chunk(&path, KEY_USAGE).unwrap().unwrap().2);
        assert!(read_chunks(&bytes).is_err());
    }

    #[test]
    fn too_large_is_refused() {
        let px = Pixels { width: MAX_SIDE + 1, height: 1, channels: 3, data: vec![0; (MAX_SIDE as usize + 1) * 3] };
        assert!(encode(&px, &[]).is_none());
    }
}
