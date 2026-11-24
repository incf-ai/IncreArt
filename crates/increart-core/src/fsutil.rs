//! Small filesystem helpers.

use std::io::Write;

use crate::vfs as fs;
use std::path::{Path, PathBuf};

use crate::error::{IoContext, PathParts, Result};

static FSYNC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Turns `fsync` after writes off (tests: it can take a second per call on
/// some filesystems) or back on. On by default.
pub fn set_fsync(on: bool) {
    FSYNC.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// `sync_all` unless turned off with [`set_fsync`].
pub fn sync(f: &fs::File) -> std::io::Result<()> {
    if FSYNC.load(std::sync::atomic::Ordering::Relaxed) { f.sync_all() } else { Ok(()) }
}

/// Reads `buf.len()` bytes at `at` (the file's position may move).
pub fn read_exact_at(f: &fs::File, buf: &mut [u8], at: u64) -> std::io::Result<()> {
    f.read_exact_at(buf, at)
}

/// Writes all of `buf` at `at` (the file's position may move).
pub fn write_all_at(f: &fs::File, buf: &[u8], at: u64) -> std::io::Result<()> {
    f.write_all_at(buf, at)
}

pub(crate) fn read_exact_at_disk(f: &std::fs::File, buf: &mut [u8], at: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(f, buf, at)
    }
    #[cfg(windows)]
    {
        let mut done = 0;
        while done < buf.len() {
            match std::os::windows::fs::FileExt::seek_read(f, &mut buf[done..], at + done as u64)? {
                0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                n => done += n,
            }
        }
        Ok(())
    }
}

pub(crate) fn write_all_at_disk(f: &std::fs::File, buf: &[u8], at: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::write_all_at(f, buf, at)
    }
    #[cfg(windows)]
    {
        let mut done = 0;
        while done < buf.len() {
            match std::os::windows::fs::FileExt::seek_write(f, &buf[done..], at + done as u64)? {
                0 => return Err(std::io::ErrorKind::WriteZero.into()),
                n => done += n,
            }
        }
        Ok(())
    }
}

/// Writes via a temporary sibling and a rename, so readers never see a half
/// written file.
/// A symlink is written through: its target is replaced, the link kept.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let resolved;
    let path = if is_symlink(path) {
        resolved = fs::canonicalize(path).at(path)?;
        resolved.as_path()
    } else {
        path
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).at(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp).at(&tmp)?;
        f.write_all(data).at(&tmp)?;
        sync(&f).at(&tmp)?;
    }
    fs::rename(&tmp, path).at(path)
}

/// Every entry below `dir` (not following symlinks), parents before children.
/// A missing `dir`, or one that isn't a directory, yields nothing.
pub fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
        walk_into(dir, &mut out)?;
    }
    Ok(out)
}

fn walk_into(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = match fs::read_dir(dir) {
        Ok(rd) => rd.map(|e| e.map(|e| e.path())).collect::<Result<Vec<_>, _>>().at(dir)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).at(dir),
    };
    entries.sort();
    for p in entries {
        let is_dir = fs::symlink_metadata(&p).at(&p)?.is_dir();
        out.push(p.clone());
        if is_dir {
            walk_into(&p, out)?;
        }
    }
    Ok(())
}

/// Deletes `path` (a file, or a directory and everything in it) for good,
/// first overwriting each file on disk with random bytes. A file with another
/// hard link (an output link and its store image) is only unlinked: its data
/// belongs to the other name too. Symlinks are removed, never followed. The
/// overwrite is an attempt: a file that can't be written is still deleted.
pub fn shred(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).at(path)?;
    if meta.is_dir() {
        for p in walk(path)? {
            overwrite(&p);
        }
        return fs::remove_dir_all(path).at(path);
    }
    overwrite(path);
    fs::remove_file(path).at(path)
}

/// Writes random bytes over a regular file on disk with one link, from
/// start to end, and flushes them. Returns whether it did: a symlink, a file
/// sharing its data with another hard link, one in RAM or one that can't be
/// written is left as it is.
pub fn overwrite(path: &Path) -> bool {
    if !fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let Ok(f) = fs::OpenOptions::new().write(true).open(path) else { return false };
    let Some(disk) = f.disk() else { return false };
    if links(disk) != Some(1) {
        return false;
    }
    let Ok(len) = disk.metadata().map(|m| m.len()) else { return false };
    let mut buf = vec![0u8; 1 << 16];
    let mut at = 0;
    while at < len {
        let n = (len - at).min(buf.len() as u64) as usize;
        if getrandom::fill(&mut buf[..n]).is_err() || write_all_at(&f, &buf[..n], at).is_err() {
            return false;
        }
        at += n as u64;
    }
    sync(&f).is_ok()
}

/// How many hard links a file has.
#[cfg(unix)]
fn links(f: &std::fs::File) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    f.metadata().ok().map(|m| m.nlink())
}

#[cfg(windows)]
fn links(f: &std::fs::File) -> Option<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut info) } != 0;
    ok.then_some(info.nNumberOfLinks as u64)
}

#[cfg(not(any(unix, windows)))]
fn links(_: &std::fs::File) -> Option<u64> {
    None
}

/// Immediate subdirectories of `dir` (not symlinks), sorted.
pub fn subdirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).at(dir),
    };
    for e in rd {
        let e = e.at(dir)?;
        if e.file_type().at(e.path())?.is_dir() && !hidden(&e.path()) {
            out.push(e.path());
        }
    }
    out.sort();
    Ok(out)
}

/// `path` with `/` between its parts on every OS, for paths written into a
/// library's files or images (which move between OSes).
pub fn posix(path: &Path) -> String {
    let s = path.to_string_lossy();
    if cfg!(windows) { s.replace('\\', "/") } else { s.into_owned() }
}

/// Names starting with `.` (our temporary files, file manager metadata).
pub fn hidden(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

pub fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

pub fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Recursively copies a directory tree (files and symlinks as they are),
/// but not a library's `library.lock`: the copy is a library of its own, and
/// Windows can't read a lock another handle holds.
pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).at(to)?;
    for p in walk(from)? {
        let rel = p.relative_to(from)?;
        if rel == Path::new(crate::library::LOCK_FILE) {
            continue;
        }
        let dst = to.join(rel);
        let meta = fs::symlink_metadata(&p).at(&p)?;
        if meta.is_dir() {
            fs::create_dir_all(&dst).at(&dst)?;
        } else if meta.file_type().is_symlink() {
            crate::link::symlink(&fs::read_link(&p).at(&p)?, &dst).at(&dst)?;
        } else {
            fs::copy(&p, &dst).at(&dst)?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn shred_overwrites_a_file_then_deletes_it() {
        use crate::vfs;
        use std::io::Read;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        vfs::write(&p, vec![7u8; 200_000]).unwrap();
        let mut held = vfs::File::open(&p).unwrap();
        super::shred(&p).unwrap();
        assert!(!vfs::exists(&p));
        let mut after = vec![];
        held.read_to_end(&mut after).unwrap();
        assert_eq!(after.len(), 200_000);
        assert!(after.iter().filter(|b| **b == 7).count() < 10_000);
    }

    #[test]
    fn shred_keeps_what_other_names_share() {
        use crate::vfs;
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store.png");
        vfs::write(&store, b"pixels").unwrap();
        let dir = d.path().join("project");
        vfs::create_dir(&dir).unwrap();
        vfs::hard_link(&store, dir.join("hard.png")).unwrap();
        vfs::symlink(&store, dir.join("soft.png")).unwrap();
        vfs::write(dir.join("run.rsn"), b"prompt").unwrap();
        super::shred(&dir).unwrap();
        assert!(!vfs::exists(&dir));
        assert_eq!(vfs::read(&store).unwrap(), b"pixels");
    }

    #[test]
    fn writes_through_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let (target, link) = (d.path().join("real.rsn"), d.path().join("link.rsn"));
        crate::vfs::write(&target, "old").unwrap();
        crate::vfs::symlink(&target, &link).unwrap();
        super::write_atomic(&link, b"new").unwrap();
        assert!(super::is_symlink(&link));
        assert_eq!(crate::vfs::read_to_string(&target).unwrap(), "new");
    }
}
