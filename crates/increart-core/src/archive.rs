//! Library archives (DESIGN.md, Library archives): a whole library as one
//! `.zip` or `.tar.gz`, and an archive unpacked into a library again.
//!
//! An archive holds one folder, the library, without its caches or lock.
//! Each link goes in as a link to its store image, whatever its kind in the
//! library (so each image is stored once): a tar symlink in a `.tar.gz`, a
//! stand-in file holding the target path in a `.zip` (every unzip tool
//! keeps those; some skip symlinks). `library.rsn` names no link mode, so
//! the library picks one when it's first opened and makes its links that
//! kind (see [`link::replace_stand_ins`]).

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::error::{Error, IoContext, PathParts, Result};
use crate::model::{LibraryMeta, Store, StoreFormat};
use crate::{fsutil, link, rsnfmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

impl ArchiveFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::TarGz => "tar.gz",
        }
    }

    /// From a file name: `.zip`, `.tar.gz` or `.tgz`.
    pub fn of_path(p: &Path) -> Option<ArchiveFormat> {
        let name = p.file_name()?.to_string_lossy().to_lowercase();
        if name.ends_with(".zip") {
            Some(ArchiveFormat::Zip)
        } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Some(ArchiveFormat::TarGz)
        } else {
            None
        }
    }
}

/// Told how far it is (bytes done, of how many); `false` stops.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64) -> bool;

const CANCELLED: &str = "cancelled";

/// Whether an error is a stop asked for by the progress callback.
pub fn is_cancelled(e: &Error) -> bool {
    matches!(e, Error::Refused(m) if m == CANCELLED)
}

enum Item {
    Dir,
    File(PathBuf),
    /// `library.rsn`, without its link mode.
    Meta(Vec<u8>),
    /// A link: its target, relative, with `/`.
    Link(String),
}

/// What goes in, in order: folders and records, then links, then store
/// images. A seed stored meanwhile is then either missing (its run record
/// read before) or whole, and every link's image is there.
fn plan(root: &Path) -> Result<Vec<(String, Item, u64)>> {
    let top = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "library".into());
    let (mut records, mut links, mut stores) = (vec![], vec![], vec![]);
    for p in fsutil::walk(root)? {
        let rel = p.relative_to(root)?;
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let first = parts[0].as_str();
        if matches!(first, "cache" | "thumbs" | ".link-probe" | "library.lock") || first.starts_with(".increart-") {
            continue;
        }
        let name = format!("{top}/{}", parts.join("/"));
        let meta = fs::symlink_metadata(&p).at(&p)?;
        if meta.is_dir() {
            records.push((name, Item::Dir, 0));
            continue;
        }
        if first == "input_images" || first == "output_images" {
            stores.push((name, Item::File(p), meta.len()));
            continue;
        }
        if parts.len() == 1 && first == "library.rsn" {
            let mut m: LibraryMeta = rsnfmt::read(&p)?;
            m.link_mode = None;
            records.push((name, Item::Meta(rsnfmt::to_string(&m).into_bytes()), meta.len()));
            continue;
        }
        let is_image = StoreFormat::of_name(&parts[parts.len() - 1]).is_some();
        let store = if first == "input" { Store::Input } else { Store::Output };
        let to = if is_image { link::read_ref(&p, store).ok().flatten() } else { None };
        let target = to.map(|r| r.path(root)).filter(|t| t.is_file());
        match target {
            Some(t) => {
                let up = "../".repeat(parts.len() - 1);
                let file = t.name()?.to_string_lossy();
                let store_dir = t.parent_dir()?.name()?.to_string_lossy();
                links.push((name, Item::Link(format!("{up}{store_dir}/{file}")), 0));
            }
            // a broken symlink: left out (the check reports it at home)
            None if meta.file_type().is_symlink() && !p.exists() => {}
            None => records.push((name, Item::File(p), meta.len())),
        }
    }
    records.extend(links);
    records.extend(stores);
    Ok(records)
}

/// Writes the library at `root` to `dest` (replaced if there; written beside
/// it first). Returns the archive's size. The library may be in use: this
/// only reads it.
pub fn export(root: &Path, dest: &Path, format: ArchiveFormat, progress: Progress) -> Result<u64> {
    if dest.starts_with(root) {
        return Err(Error::Refused(format!("{} is inside the library", dest.display())));
    }
    let items = plan(root)?;
    let total: u64 = items.iter().map(|(_, _, n)| n).sum();
    let tmp = dest.with_file_name(format!(".{}.part", dest.file_name().unwrap_or_default().to_string_lossy()));
    let r = (|| {
        let file = File::create(&tmp).at(&tmp)?;
        let file = match format {
            ArchiveFormat::Zip => write_zip(file, &items, total, progress),
            ArchiveFormat::TarGz => write_tar_gz(file, &items, total, progress),
        }
        .map_err(|e| match e.kind() {
            io::ErrorKind::Interrupted => Error::Refused(CANCELLED.into()),
            _ => Error::Io { path: dest.to_path_buf(), source: e },
        })?;
        fsutil::sync(&file).at(&tmp)?;
        drop(file);
        fs::rename(&tmp, dest).at(dest)?;
        Ok(fs::metadata(dest).at(dest)?.len())
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// An entry's modification time, in seconds: a file's own, else now.
fn mtime(item: &Item, now: i64) -> i64 {
    let file_time = match item {
        Item::File(p) => fs::metadata(p).and_then(|m| m.modified()).ok(),
        _ => None,
    };
    file_time.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(now, |d| d.as_secs() as i64)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Calls `progress`; `Interrupted` when it says stop.
fn tick(progress: &mut Progress, done: u64, total: u64) -> io::Result<()> {
    if progress(done, total) { Ok(()) } else { Err(io::ErrorKind::Interrupted.into()) }
}

fn write_zip(file: File, items: &[(String, Item, u64)], total: u64, mut progress: Progress) -> io::Result<File> {
    use zip::write::SimpleFileOptions;
    let mut z = zip::ZipWriter::new(file);
    let base = SimpleFileOptions::default();
    let (mut done, now) = (0, now());
    for (name, item, size) in items {
        // PNG and WebP are compressed already
        let stored = StoreFormat::of_name(name).is_some();
        // zip times are the local time
        let local = chrono::DateTime::from_timestamp(mtime(item, now), 0).map(|t| t.with_timezone(&chrono::Local));
        let when = local.and_then(|t| {
            use chrono::{Datelike, Timelike};
            let b = |v: u32| v as u8;
            zip::DateTime::from_date_and_time(
                t.year() as u16,
                b(t.month()),
                b(t.day()),
                b(t.hour()),
                b(t.minute()),
                b(t.second()),
            )
            .ok()
        });
        let base = base.last_modified_time(when.unwrap_or_default());
        let opts = base
            .compression_method(if stored { zip::CompressionMethod::Stored } else { zip::CompressionMethod::Deflated })
            .unix_permissions(0o644)
            .large_file(*size >= u32::MAX as u64);
        match item {
            Item::Dir => z.add_directory(name.as_str(), base.unix_permissions(0o755))?,
            Item::Meta(bytes) => {
                z.start_file(name.as_str(), opts)?;
                z.write_all(bytes)?;
            }
            Item::Link(target) => {
                z.start_file(name.as_str(), opts)?;
                z.write_all(target.as_bytes())?;
            }
            Item::File(p) => {
                z.start_file(name.as_str(), opts)?;
                io::copy(&mut File::open(p)?, &mut z)?;
            }
        }
        done += size;
        tick(&mut progress, done, total)?;
    }
    z.finish().map_err(io::Error::other)
}

fn write_tar_gz(file: File, items: &[(String, Item, u64)], total: u64, mut progress: Progress) -> io::Result<File> {
    let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut t = tar::Builder::new(gz);
    let (mut done, now) = (0, now());
    for (name, item, size) in items {
        let mut h = tar::Header::new_gnu();
        h.set_mtime(mtime(item, now) as u64);
        match item {
            Item::Dir => {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(0o755);
                h.set_size(0);
                t.append_data(&mut h, name, io::empty())?;
            }
            Item::Meta(bytes) => {
                h.set_mode(0o644);
                h.set_size(bytes.len() as u64);
                t.append_data(&mut h, name, bytes.as_slice())?;
            }
            Item::Link(target) => {
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_mode(0o777);
                h.set_size(0);
                t.append_link(&mut h, name, target)?;
            }
            Item::File(p) => {
                let mut f = File::open(p)?;
                h.set_mode(0o644);
                h.set_size(f.metadata()?.len());
                t.append_data(&mut h, name, &mut f)?;
            }
        }
        done += size;
        tick(&mut progress, done, total)?;
    }
    t.into_inner()?.finish()
}

/// An entry's path, made safe: only plain names (no root, drive or `..`).
fn safe_path(name: &str) -> Option<PathBuf> {
    if name.starts_with(['/', '\\']) {
        return None;
    }
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.contains(':') => return None,
            p => out.push(p),
        }
    }
    let plain = out.components().all(|c| matches!(c, Component::Normal(_)));
    (plain && out.components().next().is_some()).then_some(out)
}

/// Counts the bytes read from the archive file, for progress.
struct Counted<'a, R> {
    inner: R,
    read: u64,
    total: u64,
    progress: Progress<'a>,
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        tick(&mut self.progress, self.read, self.total)?;
        Ok(n)
    }
}

impl<R: io::Seek> io::Seek for Counted<'_, R> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        let at = self.inner.seek(pos)?;
        self.read = at;
        Ok(at)
    }
}

/// Unpacks a library archive into the folder `into`, as the folder the
/// archive holds (refused if that is there already). Links come out as
/// stand-ins, and `library.rsn` names no link mode: opening the library
/// makes them links of the kind this disk takes. Returns the library.
pub fn unpack(archive: &Path, into: &Path, progress: Progress) -> Result<PathBuf> {
    let format = ArchiveFormat::of_path(archive)
        .ok_or_else(|| Error::Refused(format!("{}: not a .zip or .tar.gz", archive.display())))?;
    let file = File::open(archive).at(archive)?;
    let total = file.metadata().at(archive)?.len();
    let staging = into.join(format!(".increart-unpack-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging).at(&staging)?;
    }
    fs::create_dir_all(&staging).at(&staging)?;
    let r = (|| {
        let counted = Counted { inner: io::BufReader::new(file), read: 0, total, progress };
        match format {
            ArchiveFormat::Zip => unpack_zip(counted, &staging),
            ArchiveFormat::TarGz => unpack_tar_gz(counted, &staging),
        }
        .map_err(|e| match e {
            Error::Io { source, .. } if source.kind() == io::ErrorKind::Interrupted => Error::Refused(CANCELLED.into()),
            e => e,
        })?;
        // the library: the archive's one folder, or the archive itself
        let (lib, name) = if staging.join("library.rsn").is_file() {
            let stem = archive.file_name().unwrap_or_default().to_string_lossy();
            let stem =
                [".tar.gz", ".tgz", ".zip"].iter().find_map(|e| stem.strip_suffix(e)).unwrap_or(&stem).to_string();
            (staging.clone(), stem)
        } else {
            let dirs: Vec<PathBuf> = fs::read_dir(&staging).at(&staging)?.flatten().map(|e| e.path()).collect();
            match dirs.as_slice() {
                [d] if d.join("library.rsn").is_file() => {
                    (d.clone(), d.file_name().unwrap_or_default().to_string_lossy().into_owned())
                }
                _ => return Err(Error::Refused(format!("{}: not an IncreArt library", archive.display()))),
            }
        };
        let meta_path = lib.join("library.rsn");
        let mut meta: LibraryMeta = rsnfmt::read(&meta_path)?;
        meta.link_mode = None;
        rsnfmt::write(&meta_path, &meta)?;
        let dest = into.join(&name);
        if fsutil::exists(&dest) {
            return Err(Error::Refused(format!("{} is there already", dest.display())));
        }
        if lib == staging {
            fs::rename(&staging, &dest).at(&dest)?;
        } else {
            fs::rename(&lib, &dest).at(&dest)?;
        }
        Ok(dest)
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    r
}

fn unpack_zip<R: Read + io::Seek>(reader: R, to: &Path) -> Result<()> {
    let bad = |e: zip::result::ZipError| match e {
        zip::result::ZipError::Io(e) => Error::Io { path: to.to_path_buf(), source: e },
        e => Error::Refused(format!("not a readable zip: {e}")),
    };
    let mut z = zip::ZipArchive::new(reader).map_err(bad)?;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(bad)?;
        let name = e.name().map_err(bad)?.into_owned();
        let Some(rel) = safe_path(&name) else {
            return Err(Error::Refused(format!("the archive names a file outside its folder: {name}")));
        };
        let at = to.join(rel);
        if e.is_dir() {
            fs::create_dir_all(&at).at(&at)?;
            continue;
        }
        let dir = at.parent_dir()?;
        fs::create_dir_all(dir).at(dir)?;
        // a symlink's data is its target: written as its stand-in
        let mut out = File::create(&at).at(&at)?;
        io::copy(&mut e, &mut out).at(&at)?;
    }
    Ok(())
}

fn unpack_tar_gz<R: Read>(reader: R, to: &Path) -> Result<()> {
    let mut t = tar::Archive::new(flate2::read::GzDecoder::new(reader));
    let entries = t.entries().at(to)?;
    for e in entries {
        let mut e = e.at(to)?;
        let name = e.path_bytes().into_owned();
        let name = String::from_utf8_lossy(&name).into_owned();
        let Some(rel) = safe_path(&name) else {
            return Err(Error::Refused(format!("the archive names a file outside its folder: {name}")));
        };
        let at = to.join(&rel);
        let kind = e.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(&at).at(&at)?;
            continue;
        }
        let dir = at.parent_dir()?;
        fs::create_dir_all(dir).at(dir)?;
        if kind.is_symlink() {
            let target = e.link_name_bytes().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            fs::write(&at, target).at(&at)?;
        } else if kind.is_hard_link() {
            // another entry of the archive, unpacked already
            let target = e.link_name_bytes().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            let Some(from) = safe_path(&target) else {
                return Err(Error::Refused(format!("the archive links outside its folder: {name}")));
            };
            fs::copy(to.join(from), &at).at(&at)?;
        } else if kind.is_file() || kind == tar::EntryType::Continuous {
            let mut out = File::create(&at).at(&at)?;
            io::copy(&mut e, &mut out).at(&at)?;
        }
        // anything else (devices, fifos): not part of a library
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_paths_stay_inside() {
        assert_eq!(safe_path("lib/projects/a.png"), Some(PathBuf::from("lib/projects/a.png")));
        assert_eq!(safe_path("./lib/x"), Some(PathBuf::from("lib/x")));
        assert_eq!(safe_path("lib\\x"), Some(PathBuf::from("lib/x")));
        for bad in ["../x", "lib/../../x", "/etc/passwd", "C:/x", "C:\\x", "", "."] {
            assert_eq!(safe_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn formats_by_name() {
        assert_eq!(ArchiveFormat::of_path(Path::new("a/My lib.ZIP")), Some(ArchiveFormat::Zip));
        assert_eq!(ArchiveFormat::of_path(Path::new("x.tar.gz")), Some(ArchiveFormat::TarGz));
        assert_eq!(ArchiveFormat::of_path(Path::new("x.tgz")), Some(ArchiveFormat::TarGz));
        assert_eq!(ArchiveFormat::of_path(Path::new("x.tar")), None);
    }
}
