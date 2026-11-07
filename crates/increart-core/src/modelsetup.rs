//! The **Models** window (DESIGN.md, Setting up models): where the model
//! files go, folders that already hold some, to add as model folders, and
//! model files kept elsewhere, to link.

use std::path::{Path, PathBuf};

/// A folder holding model files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundFolder {
    pub path: String,
    /// Model files in it (a few folders down) and their size.
    pub files: usize,
    pub bytes: u64,
}

/// Where the files go when there's no model folder yet.
pub fn default_dir(home: &Path) -> PathBuf {
    home.join("models")
}

/// Where other tools keep models: ComfyUI's, stable-diffusion.cpp's,
/// KoboldCpp's, and the app's own default.
pub fn candidates(home: &Path) -> Vec<PathBuf> {
    [
        "models",
        "ComfyUI/models",
        "comfyui/models",
        "ComfyUI_windows_portable/ComfyUI/models",
        "Documents/ComfyUI/models",
        "stable-diffusion.cpp/models",
        "sd.cpp/models",
        "koboldcpp",
        "koboldcpp/models",
        "IncreArt/models",
        "AI/models",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect()
}

/// Whether a file name is a model file's.
pub fn is_model_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".safetensors", ".gguf", ".ckpt", ".sft"].iter().any(|e| lower.ends_with(e))
}

/// The model files under `dir`, `depth` folders down: count and bytes.
pub fn count(dir: &Path, depth: u32) -> (usize, u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return (0, 0) };
    let (mut n, mut bytes) = (0, 0);
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                let (a, b) = count(&p, depth - 1);
                (n, bytes) = (n + a, bytes + b);
            }
        } else if e.file_name().to_str().is_some_and(is_model_file) {
            n += 1;
            bytes += std::fs::metadata(&p).map_or(0, |m| m.len());
        }
    }
    (n, bytes)
}

/// The `candidates` (and `more`) holding model files, each once (by its
/// real path), in that order.
pub fn find_folders(home: &Path, more: &[PathBuf]) -> Vec<FoundFolder> {
    let mut seen: Vec<PathBuf> = vec![];
    let mut out = vec![];
    for dir in candidates(home).into_iter().chain(more.iter().cloned()) {
        let Ok(real) = dir.canonicalize() else { continue };
        if seen.contains(&real) {
            continue;
        }
        seen.push(real);
        let (files, bytes) = count(&dir, 3);
        if files > 0 {
            out.push(FoundFolder { path: dir.to_string_lossy().into_owned(), files, bytes });
        }
    }
    out
}

/// The model files under some folders, by file name, with their size: so
/// a model's file kept somewhere else (another tool's folder, another
/// layout, Hugging Face's cache) is found and linked where the model wants
/// it, rather than downloaded again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileIndex {
    by_name: std::collections::BTreeMap<String, Vec<(PathBuf, u64)>>,
}

impl FileIndex {
    /// The model files under `dirs`, a few folders down (Hugging Face's
    /// `hub/models--…/snapshots/…/` is four); folder links not followed,
    /// each file once by name and real path.
    pub fn build(dirs: &[PathBuf]) -> FileIndex {
        fn walk(dir: &Path, depth: u32, seen: &mut std::collections::BTreeSet<(String, PathBuf)>, ix: &mut FileIndex) {
            let Ok(list) = std::fs::read_dir(dir) else { return };
            for e in list.flatten() {
                let Ok(kind) = e.file_type() else { continue };
                let p = e.path();
                if kind.is_dir() {
                    if depth > 0 && !matches!(e.file_name().to_str(), Some("blobs" | "xet" | ".git")) {
                        walk(&p, depth - 1, seen, ix);
                    }
                    continue;
                }
                let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
                if !is_model_file(&name) {
                    continue;
                }
                let Ok(meta) = std::fs::metadata(&p) else { continue };
                let real = p.canonicalize().unwrap_or_else(|_| p.clone());
                if !meta.is_file() || !seen.insert((name.clone(), real)) {
                    continue;
                }
                ix.by_name.entry(name).or_default().push((p, meta.len()));
            }
        }
        let mut ix = FileIndex::default();
        let mut seen = std::collections::BTreeSet::new();
        for d in dirs {
            walk(d, 5, &mut seen, &mut ix);
        }
        ix
    }

    /// A file named `name` (a file ID's last part), of `size` if that's
    /// known: the first found.
    pub fn find(&self, name: &str, size: Option<u64>) -> Option<(&Path, u64)> {
        let name = Path::new(name).file_name()?.to_str()?;
        self.by_name.get(name)?.iter().find(|(_, n)| size.is_none_or(|s| s == *n)).map(|(p, n)| (p.as_path(), *n))
    }

    /// How many files it knows.
    pub fn len(&self) -> usize {
        self.by_name.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_indexed_by_name_and_found_by_size() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        std::fs::create_dir_all(t.join("a/deep/er")).unwrap();
        std::fs::create_dir_all(t.join("b/blobs")).unwrap();
        std::fs::write(t.join("a/deep/er/vae.safetensors"), b"12345").unwrap();
        std::fs::write(t.join("b/vae.safetensors"), b"123").unwrap();
        std::fs::write(t.join("b/blobs/x.gguf"), b"1").unwrap();
        std::fs::write(t.join("b/notes.txt"), b"1").unwrap();
        // the same file twice under one name (a folder searched twice): once
        let ix = FileIndex::build(&[t.join("a"), t.join("b"), t.join("b"), t.join("missing")]);
        assert_eq!(ix.len(), 2, "{ix:?}");
        // by a file ID's name, the size picking
        assert_eq!(ix.find("vae/vae.safetensors", Some(5)).unwrap().1, 5);
        assert_eq!(ix.find("vae.safetensors", Some(3)).unwrap().1, 3);
        assert!(ix.find("vae.safetensors", Some(4)).is_none());
        assert!(ix.find("vae.safetensors", None).is_some());
        assert!(ix.find("x.gguf", None).is_none(), "Hugging Face's blobs aren't looked in");
    }

    #[test]
    fn folders_with_model_files_are_found_once() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join("ComfyUI/models/checkpoints")).unwrap();
        std::fs::write(h.join("ComfyUI/models/checkpoints/a.safetensors"), b"12345").unwrap();
        std::fs::write(h.join("ComfyUI/models/readme.txt"), b"x").unwrap();
        // an empty one, and one only of other files
        std::fs::create_dir_all(h.join("models")).unwrap();
        std::fs::create_dir_all(h.join("koboldcpp")).unwrap();
        std::fs::write(h.join("koboldcpp/koboldcpp.py"), b"x").unwrap();
        // another way to the same folder
        std::fs::create_dir_all(h.join("AI")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(h.join("ComfyUI/models"), h.join("AI/models")).unwrap();
        let found = find_folders(h, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].files, 1);
        assert_eq!(found[0].bytes, 5);
        assert!(found[0].path.ends_with("ComfyUI/models"));
        assert!(is_model_file("x.GGUF") && !is_model_file("x.txt"));
    }
}
