//! SHA-256 of model weight files, for recipes (DESIGN.md, Pixel hash: model
//! weights are identified by the hash of the file bytes). Hashing a 9 GB
//! file takes a while, so results are cached by path, size and mtime.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{IoContext, Result};
use crate::pixel::hex;
use crate::rsnfmt;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "ModelHashes")]
struct Cache {
    #[serde(default)]
    files: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Entry")]
struct Entry {
    size: u64,
    mtime: u64,
    sha256: String,
}

/// The file's SHA-256, from `cache` (an RSN file) when its size and mtime
/// are unchanged, else computed and cached.
pub fn sha256_cached(cache: &Path, file: &Path) -> Result<String> {
    sha256_cached_progress(cache, file, |_, _| true)
}

/// `file`'s hash when it's in `cache` (and still its size and time),
/// without reading the file.
pub fn cached(cache: &Path, file: &Path) -> Option<String> {
    let meta = std::fs::metadata(file).ok()?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let c: Cache = rsnfmt::read(cache).unwrap_or_default();
    c.files.get(&*file.to_string_lossy()).filter(|e| e.size == meta.len() && e.mtime == mtime).map(|e| e.sha256.clone())
}

/// Whether `file`'s hash is in `cache` (and still its size and time).
pub fn is_cached(cache: &Path, file: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(file) else { return false };
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let c: Cache = rsnfmt::read(cache).unwrap_or_default();
    c.files.get(&*file.to_string_lossy()).is_some_and(|e| e.size == meta.len() && e.mtime == mtime)
}

/// [`sha256_cached`], telling `progress` (bytes read, size) as it reads;
/// it returns false to stop (an `Interrupted` error, nothing cached).
pub fn sha256_cached_progress(cache: &Path, file: &Path, mut progress: impl FnMut(u64, u64) -> bool) -> Result<String> {
    let meta = std::fs::metadata(file).at(file)?;
    let size = meta.len();
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let key = file.to_string_lossy().into_owned();
    let mut c: Cache = rsnfmt::read(cache).unwrap_or_default();
    if let Some(e) = c.files.get(&key)
        && e.size == size
        && e.mtime == mtime
    {
        return Ok(e.sha256.clone());
    }
    let mut h = Sha256::new();
    let mut f = File::open(file).at(file)?;
    let mut buf = vec![0; 8 << 20];
    let mut read = 0u64;
    loop {
        let n = f.read(&mut buf).at(file)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        read += n as u64;
        if !progress(read, size) {
            let source = std::io::Error::new(std::io::ErrorKind::Interrupted, "stopped");
            return Err(crate::error::Error::Io { path: file.to_path_buf(), source });
        }
    }
    let sha256 = hex(&h.finalize());
    c.files.insert(key, Entry { size, mtime, sha256: sha256.clone() });
    rsnfmt::write(cache, &c)?;
    Ok(sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_until_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, f) = (dir.path().join("hashes.rsn"), dir.path().join("w.bin"));
        std::fs::write(&f, b"abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_cached(&cache, &f).unwrap(), abc);
        // a stale cache entry with matching size/mtime is trusted
        let text = std::fs::read_to_string(&cache).unwrap().replace(abc, "cached");
        std::fs::write(&cache, text).unwrap();
        assert_eq!(sha256_cached(&cache, &f).unwrap(), "cached");
        std::fs::write(&f, b"abcd").unwrap();
        assert_ne!(sha256_cached(&cache, &f).unwrap(), "cached");
    }

    #[test]
    fn stopped_hashing_caches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, f) = (dir.path().join("hashes.rsn"), dir.path().join("w.bin"));
        std::fs::write(&f, b"abc").unwrap();
        assert!(sha256_cached_progress(&cache, &f, |_, _| false).is_err());
        assert!(!is_cached(&cache, &f));
        assert!(sha256_cached_progress(&cache, &f, |_, _| true).is_ok());
        assert!(is_cached(&cache, &f));
    }
}
