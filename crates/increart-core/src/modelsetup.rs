//! **Set up models** (DESIGN.md, Setting up models): where the model files
//! go, folders that already hold some, to add as model folders.

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

#[cfg(test)]
mod tests {
    use super::*;

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
