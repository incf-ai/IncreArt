#![allow(dead_code)]

pub mod cut;
pub mod fake;
pub mod fake_kobold;
pub mod sample;
pub mod verify;

use std::path::{Path, PathBuf};

/// Call first in every test: fsync is slow on overlay filesystems and tests
/// don't need durability.
pub fn init() {
    increart_core::fsutil::set_fsync(false);
}

pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// A copy of the sample library at `dir`. The generator runs once per test
/// binary; each caller gets its own copy.
pub fn sample_copy(dir: &Path) {
    init();
    static ONCE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let master = ONCE.get_or_init(|| {
        let t = tempfile::tempdir().unwrap();
        make_sample(&t.path().join("lib"));
        t
    });
    increart_core::fsutil::copy_tree(&master.path().join("lib"), dir).unwrap();
}

/// Writes a fresh sample library (sample.rs) into `dir`.
pub fn make_sample(dir: &Path) {
    sample::write(dir);
}

/// Checks the library at `dir` (verify.rs); returns its summary line.
pub fn verify(dir: &Path) -> String {
    verify::verify(dir)
}
