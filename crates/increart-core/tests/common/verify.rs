//! An independent check of a library, the sample one (sample.rs) or one the
//! core built: store files (name = increart:id, increart:pixels = real pixel
//! hash, recipe or source chunk present), every image link resolves into a
//! store, every ImageRef and idea image resolves, idea/project/run IDs
//! are well formed and unique, and so are UUIDs (UUIDv7s; on everything once
//! the library has one). Decodes PNGs itself (all five row filters) and
//! reads the RSN as text, not through the core. WebP store files (lossless)
//! get every check but the pixel hash: their metadata is read from the PNGt
//! chunks; the pixels are left to the app's own tests. By hand:
//! `cargo run -p increart-core --example sample_library -- --verify [dir]`.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

const EXTS: [&str; 2] = ["png", "webp"];

fn be32(d: &[u8]) -> usize {
    u32::from_be_bytes(d[..4].try_into().unwrap()) as usize
}

/// A PNG's chunks: (type, data).
fn chunks(d: &[u8]) -> Vec<(&[u8], &[u8])> {
    let (mut out, mut i) = (vec![], 8);
    while i + 8 <= d.len() {
        let n = be32(&d[i..]);
        let Some(body) = d.get(i + 8..i + 8 + n) else { break };
        out.push((&d[i + 4..i + 8], body));
        i += 12 + n;
    }
    out
}

/// A PNG's chunks, or those a WebP file carries in PNGt chunks.
fn chunks_any<'a>(d: &'a [u8], problems: &mut Vec<String>) -> Vec<(&'a [u8], &'a [u8])> {
    if !d.starts_with(b"RIFF") {
        return chunks(d);
    }
    let (mut out, mut i) = (vec![], 12);
    while i + 8 <= d.len() {
        let n = u32::from_le_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
        let Some(body) = d.get(i + 8..i + 8 + n) else { break };
        if &d[i..i + 4] == b"PNGt" && n >= 8 {
            if crc32fast::hash(&body[..n - 4]).to_be_bytes() != body[n - 4..] {
                problems.push("bad CRC in a PNGt chunk".into());
            }
            out.push((&body[..4], &body[4..n - 4]));
        }
        i += 8 + n + (n & 1);
    }
    out
}

fn inflate(d: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![];
    flate2::read::ZlibDecoder::new(d).read_to_end(&mut out).ok()?;
    Some(out)
}

/// The text of the first tEXt or iTXt chunk named `key`.
fn text(d: &[u8], key: &str, problems: &mut Vec<String>) -> Option<String> {
    for (kind, body) in chunks_any(d, problems) {
        let Some(z) = body.iter().position(|&b| b == 0).filter(|_| kind == b"tEXt" || kind == b"iTXt") else {
            continue;
        };
        let (k, rest) = (&body[..z], &body[z + 1..]);
        if k != key.as_bytes() {
            continue;
        }
        if kind == b"tEXt" {
            return Some(rest.iter().map(|&b| b as char).collect());
        }
        if kind == b"iTXt" && rest.len() >= 2 {
            // compression flag and method, language and translated keyword
            let mut parts = rest[2..].splitn(3, |&b| b == 0);
            let (_, _, t) = (parts.next(), parts.next(), parts.next()?);
            let t = if rest[0] == 1 { inflate(t)? } else { t.to_vec() };
            return Some(String::from_utf8_lossy(&t).into_owned());
        }
    }
    None
}

/// The pixel hash (docs/DESIGN.md, Pixel hash) of an 8-bit RGB or RGBA PNG.
fn phash(d: &[u8]) -> Option<String> {
    let (mut ihdr, mut idat) = (None, vec![]);
    for (kind, body) in chunks(d) {
        match kind {
            b"IHDR" => ihdr = Some((be32(body), be32(&body[4..]), body[9])),
            b"IDAT" => idat.extend(body),
            _ => {}
        }
    }
    let (w, h, ctype) = ihdr?;
    let ch = match ctype {
        2 => 3,
        6 => 4,
        _ => return None,
    };
    let (raw, stride) = (inflate(&idat)?, w * ch);
    let (mut out, mut prev, mut pos) = (Vec::with_capacity(stride * h), vec![0u8; stride], 0);
    for _ in 0..h {
        let f = *raw.get(pos)?;
        let mut line = raw.get(pos + 1..pos + 1 + stride)?.to_vec();
        pos += 1 + stride;
        for x in 0..stride {
            let a = if x >= ch { line[x - ch] as i32 } else { 0 };
            let (b, c) = (prev[x] as i32, if x >= ch { prev[x - ch] as i32 } else { 0 });
            let add = match f {
                1 => a,
                2 => b,
                3 => (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => 0,
            };
            line[x] = line[x].wrapping_add(add as u8);
        }
        out.extend(&line);
        prev = line;
    }
    let mut hash = Sha256::new();
    hash.update(b"increart-pixels-v1");
    hash.update((w as u32).to_be_bytes());
    hash.update((h as u32).to_be_bytes());
    hash.update([ch as u8]);
    hash.update(&out);
    Some(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `YYYY.MM.DD-HH.MM-NNNN` (four or more digits at the end).
fn is_id(s: &str) -> bool {
    let b = s.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| !d.is_empty() && d.iter().all(u8::is_ascii_digit));
    b.len() >= 21
        && [(4, b'.'), (7, b'.'), (10, b'-'), (13, b'.'), (16, b'-')].iter().all(|&(i, c)| b[i] == c)
        && [0..4, 5..7, 8..10, 11..13, 14..16, 17..b.len()].into_iter().all(digits)
}

/// Every file under `root` as a "/" path, sorted, as a glob sees them: dot
/// names skipped, links not followed.
fn files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let rel = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if e.file_type().unwrap().is_dir() {
                walk(&e.path(), &rel, out);
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = vec![];
    walk(root, "", &mut out);
    out.sort();
    out
}

fn stem(p: &str) -> &str {
    let name = p.rsplit('/').next().unwrap();
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

fn ext(p: &str) -> &str {
    p.rsplit_once('.').map_or("", |(_, e)| e)
}

fn parent_name(p: &str) -> String {
    let dir = p.rsplit_once('/').map_or("", |(d, _)| d);
    dir.rsplit('/').next().unwrap().to_string()
}

/// A list as Python prints one.
fn repr(xs: &[String]) -> String {
    let items: Vec<_> = xs.iter().map(|x| format!("'{}'", x.replace('\\', "\\\\").replace('\'', "\\'"))).collect();
    format!("[{}]", items.join(", "))
}

/// The lines `    id: "…",` say.
/// `8-4-4-4-12` lower-case hex, version 7, the RFC variant.
fn is_uuid_v7(s: &str) -> bool {
    let b = s.as_bytes();
    let dash = |i: usize| [8, 13, 18, 23].contains(&i);
    b.len() == 36
        && b.iter().enumerate().all(|(i, &c)| if dash(i) { c == b'-' } else { matches!(c, b'0'..=b'9' | b'a'..=b'f') })
        && b[14] == b'7'
        && matches!(b[19], b'8' | b'9' | b'a' | b'b')
}

/// The `uuid: "…"` of a record (`idea.rsn`, `project.rsn`, `run.rsn`,
/// `library.rsn`).
fn uuid_line(text: &str) -> Option<String> {
    // (the last field has no comma)
    let value = |l: &str| l.strip_prefix("    uuid: \"")?.trim_end_matches(',').strip_suffix('"').map(str::to_string);
    text.lines().find_map(value)
}

fn id_lines(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| l.strip_prefix("    id: \"")?.strip_suffix("\",")).map(str::to_string).collect()
}

/// Every `"…"` (not empty) in `s`.
fn quoted(s: &str) -> Vec<&str> {
    let (mut out, mut i) = (vec![], 0);
    while let Some(start) = s[i..].find('"').map(|n| i + n) {
        let Some(end) = s[start + 1..].find('"').map(|n| start + 1 + n) else { break };
        if end > start + 1 {
            out.push(&s[start + 1..end]);
            i = end + 1;
        } else {
            i = start + 1;
        }
    }
    out
}

/// Every `id: "<id>",<space>store: Input|Output` in `s`.
fn refs(s: &str) -> Vec<(String, &'static str)> {
    let (mut out, mut i) = (vec![], 0);
    while let Some(at) = s[i..].find("id: \"").map(|n| i + n) {
        i = at + 1;
        let rest = &s[at + 5..];
        let n = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(rest.len());
        let Some(after) = rest[n..].strip_prefix("\",").filter(|_| n > 0) else { continue };
        let after = after.trim_start().strip_prefix("store: ").unwrap_or("");
        if let Some(store) = ["Input", "Output"].into_iter().find(|st| after.starts_with(st)) {
            out.push((rest[..n].to_string(), store));
            i = s.len() - after.len() + store.len();
        }
    }
    out
}

#[cfg(unix)]
fn file_id(p: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let m = fs::metadata(p).unwrap();
    format!("{}:{}", m.dev(), m.ino())
}

#[cfg(windows)]
fn file_id(p: &Path) -> String {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    let f = fs::File::open(p).unwrap();
    // SAFETY: a plain struct filled in by the call, on a handle `f` keeps open
    let info = unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        assert!(GetFileInformationByHandle(f.as_raw_handle(), &mut info) != 0, "{}", p.display());
        info
    };
    format!("{}:{}:{}", info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow)
}

/// Checks the library at `root`; returns a summary line ending in
/// `problems: none` when all is well.
pub fn verify(root: &Path) -> String {
    let mut problems: Vec<String> = vec![];
    let all = files(root);
    let read = |p: &str| String::from_utf8_lossy(&fs::read(root.join(p)).unwrap()).into_owned();
    let is_link = |p: &str| fs::symlink_metadata(root.join(p)).unwrap().file_type().is_symlink();
    let images: Vec<&String> = all.iter().filter(|p| EXTS.contains(&ext(p))).collect();
    let under = |pattern: &dyn Fn(&[&str]) -> bool| -> Vec<&String> {
        all.iter().filter(|p| pattern(&p.split('/').collect::<Vec<_>>())).collect()
    };

    let mut store: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    // (what, its UUID if it has one)
    let mut uuids: Vec<(String, Option<String>)> = vec![];
    let mut unhashed = 0;
    let store_files =
        images.iter().filter(|p| matches!(p.split('/').collect::<Vec<_>>()[..], [d, _] if d.ends_with("_images")));
    for p in store_files {
        let d = fs::read(root.join(p)).unwrap();
        if ext(p) == "webp" {
            unhashed += 1;
            if !d.starts_with(b"RIFF") || d.get(8..16) != Some(b"WEBPVP8X") {
                problems.push(format!("not an extended WebP: {p}"));
            }
        } else if text(&d, "increart:pixels", &mut problems) != phash(&d) {
            problems.push(format!("increart:pixels != pixel hash: {p}"));
        }
        if text(&d, "increart:id", &mut problems).as_deref() != Some(stem(p)) {
            problems.push(format!("increart:id != name: {p}"));
        }
        if !is_id(stem(p)) {
            problems.push(format!("bad id format: {p}"));
        }
        uuids.push((p.to_string(), text(&d, "increart:uuid", &mut problems)));
        let other = if ext(p) == "webp" { "png" } else { "webp" };
        if root.join(format!("{}.{other}", &p[..p.len() - ext(p).len() - 1])).exists() {
            problems.push(format!("stored twice: {p}"));
        }
        if p.starts_with("input_images/") {
            let src = text(&d, "increart:source", &mut problems)
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
            let ok = src.is_some_and(|s| s["version"].as_i64() == Some(1) && s.get("original_format").is_some());
            if !ok {
                problems.push(format!("bad source json: {p}"));
            }
        } else if text(&d, "increart:recipe", &mut problems).is_none() {
            problems.push(format!("output without recipe: {p}"));
        }
        store.insert(p.to_string(), d);
    }

    // the library's link mode: Symlink links are symlinks; Hardlink ones share
    // a store file's inode; Copy ones its increart:id and pixels (not every
    // byte: the usage counts are updated in the store file only)
    let meta = read("library.rsn");
    let mode = meta
        .split_once("link_mode: ")
        .map(|(_, m)| m[..m.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(m.len())].to_string());
    // not saved yet: links made as copies
    let mode = mode.unwrap_or_else(|| "Copy".into());
    let outside: Vec<String> = images
        .iter()
        .filter(|p| {
            !is_link(p) && !["input_images/", "output_images/", "input/inbox_drop/"].iter().any(|s| p.starts_with(s))
        })
        .map(|p| p.to_string())
        .collect();
    if mode != "Symlink" {
        let same = |p: &str, problems: &mut Vec<String>| {
            if mode == "Hardlink" {
                return file_id(&root.join(p));
            }
            let d = fs::read(root.join(p)).unwrap();
            format!("{:?} {:?}", text(&d, "increart:id", problems), text(&d, "increart:pixels", problems))
        };
        let stores: HashSet<String> = store.keys().map(|p| same(p, &mut problems)).collect();
        for p in &outside {
            if !stores.contains(&same(p, &mut problems)) {
                problems.push(format!("{} not of a store file: {p}", mode.to_lowercase()));
            }
        }
    }
    let links: Vec<String> = images.iter().filter(|p| is_link(p)).map(|p| p.to_string()).collect();
    if mode != "Symlink" && !links.is_empty() {
        problems.push(format!("symlinks in a {mode} library: {}", repr(&links)));
    }
    // relative to the real root: a temp dir's path may itself go through a
    // link or (Windows) a short name
    let real_root = fs::canonicalize(root).unwrap();
    for p in &links {
        let target = fs::read_link(root.join(p)).unwrap();
        if target.is_absolute() {
            problems.push(format!("absolute link: {p}"));
        }
        // a dangling link's target as written, from its folder's real path
        let real = fs::canonicalize(root.join(p)).unwrap_or_else(|_| {
            let dir = fs::canonicalize(root.join(p).parent().unwrap()).unwrap();
            dir.join(&target).components().fold(PathBuf::new(), |mut acc, c| {
                if c == Component::ParentDir {
                    acc.pop();
                } else {
                    acc.push(c);
                }
                acc
            })
        });
        let t = match real.strip_prefix(&real_root) {
            Ok(rel) => rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"),
            Err(_) => real.display().to_string(),
        };
        if !store.contains_key(&t) {
            problems.push(format!("link not into a store: {p} -> {t}"));
        } else if ext(p) != ext(&t) {
            problems.push(format!("link's extension isn't its image's: {p} -> {t}"));
        }
    }
    if mode == "Symlink" && !outside.is_empty() {
        problems.push(format!("image files outside stores that aren't links: {}", repr(&outside)));
    }

    let mut blob: String = all.iter().filter(|p| ext(p) == "rsn").map(|p| read(p)).collect();
    for d in store.values() {
        blob += &text(d, "increart:recipe", &mut problems).unwrap_or_default();
    }
    let refs = refs(&blob);
    let in_store = |kind: &str, id: &str| EXTS.iter().any(|x| store.contains_key(&format!("{kind}_images/{id}.{x}")));
    for (id, s) in &refs {
        if !in_store(&s.to_lowercase(), id) {
            problems.push(format!("dangling ref {s} {id}"));
        }
    }
    let ideas = under(&|c| c.len() >= 2 && c[0] == "input" && c[c.len() - 1] == "idea.rsn");
    for f in ideas.iter().filter(|f| f.split('/').count() >= 3) {
        let text = read(f);
        let mut lines = text.lines().skip_while(|l| !l.starts_with("    images: "));
        let block: Option<Vec<&str>> = match lines.next() {
            Some("    images: [],") => Some(vec![]),
            Some("    images: [") => {
                let block: Vec<_> = lines.by_ref().take_while(|l| *l != "    ],").collect();
                Some(block)
            }
            _ => None,
        };
        let Some(block) = block else {
            problems.push(format!("no images list: {f}"));
            continue;
        };
        for id in block.iter().flat_map(|l| quoted(l)) {
            if !in_store("input", id) {
                problems.push(format!("idea image missing: {f} {id}"));
            }
        }
    }
    let projects = under(&|c| c[0] == "projects" && c[c.len() - 1] == "project.rsn");
    let runs = under(&|c| c.len() >= 2 && c[0] == "projects" && c[c.len() - 1] == "run.rsn");
    let ids = [
        ("idea", ideas.iter().map(|f| parent_name(f)).collect::<Vec<_>>()),
        ("project", projects.iter().flat_map(|f| id_lines(&read(f))).collect()),
        ("run", runs.iter().map(|f| parent_name(f)).collect()),
    ];
    for (kind, xs) in &ids {
        if xs.iter().collect::<HashSet<_>>().len() != xs.len() || !xs.iter().all(|x| is_id(x)) {
            problems.push(format!("{kind} ids bad or duplicated: {}", repr(xs)));
        }
    }
    for f in &ideas {
        if id_lines(&read(f)).first() != Some(&parent_name(f)) {
            problems.push(format!("idea dir != id: {f}"));
        }
    }

    let meta_uuid = uuid_line(&meta);
    for f in ideas.iter().chain(&projects).chain(&runs) {
        uuids.push((f.to_string(), uuid_line(&read(f))));
    }
    let mut seen = HashSet::new();
    for (what, u) in &uuids {
        match u {
            Some(u) if !is_uuid_v7(u) => problems.push(format!("not a UUIDv7: {what} {u}")),
            Some(u) if !seen.insert(u.clone()) || meta_uuid.as_ref() == Some(u) => {
                problems.push(format!("UUID twice: {what} {u}"))
            }
            Some(_) => {}
            // the core gives everything one on opening, the library its own
            None if meta_uuid.is_some() => problems.push(format!("no UUID: {what}")),
            None => {}
        }
    }
    let n_uuids = seen.len() + usize::from(meta_uuid.is_some());
    if let Some(u) = meta_uuid.filter(|u| !is_uuid_v7(u)) {
        problems.push(format!("not a UUIDv7: library.rsn {u}"));
    }

    let n_ids: usize = ids.iter().map(|(_, xs)| xs.len()).sum();
    let n_links = if mode == "Symlink" { links.len() } else { outside.len() };
    let webp = if unhashed > 0 { format!(", {unhashed} WebP pixel hashes left unchecked") } else { String::new() };
    let problems = if problems.is_empty() { "none".into() } else { repr(&problems) };
    format!(
        "{n_ids} idea/project/run ids, {n_uuids} UUIDs; {} store images, {n_links} links, {} refs{webp}; problems: {problems}",
        store.len(),
        refs.len()
    )
}
