//! Writes the synthetic sample library (tests/common/sample.rs), or checks a
//! library with the tests' independent verifier (tests/common/verify.rs):
//!
//!     cargo run -p increart-core --example sample_library [out_dir]       # default: tmp/sample-library
//!     cargo run -p increart-core --example sample_library -- --verify [dir]
#![allow(clippy::disallowed_methods)] // fixtures on disk: no overlay here

#[path = "../tests/common/sample.rs"]
mod sample;
#[path = "../tests/common/verify.rs"]
mod verify;

use std::path::{Path, PathBuf};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.first().is_some_and(|a| a == "--verify");
    if check {
        args.remove(0);
    }
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/sample-library");
    let dir = args.first().map_or(default, PathBuf::from);
    if check {
        println!("{}", verify::verify(&dir));
    } else {
        sample::write(&dir);
        println!("{}", dir.display());
    }
}
