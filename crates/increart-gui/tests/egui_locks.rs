//! egui's `Context` is one lock: a closure given to `ctx.input(|i| ..)`,
//! `ctx.data_mut(|d| ..)`, `ctx.memory(|m| ..)` and the like runs with it
//! held, so using the `Context` (or a `Ui`, or a `Painter`) inside one waits
//! on itself. Release builds hang for good (debug builds panic after 10 s).
//! This reads the sources for such closures that name `ctx`, `ui`, `context`
//! or `painter`.

use std::path::{Path, PathBuf};

/// The `Context` methods that run a closure with the lock held.
const LOCKED: &[&str] = &[
    "input",
    "input_mut",
    "data",
    "data_mut",
    "memory",
    "memory_mut",
    "options",
    "options_mut",
    "fonts",
    "fonts_mut",
    "viewport",
    "viewport_for",
    "output",
    "output_mut",
    "graphics",
    "graphics_mut",
    "tessellation_options",
];

/// What mustn't be used with the lock held.
const REENTRANT: &[&str] = &["ctx", "ui", "context", "painter"];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "target") {
                rust_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Whether `word` appears in `s` as a whole identifier.
fn names(s: &str, word: &str) -> bool {
    let b = s.as_bytes();
    s.match_indices(word).any(|(i, _)| {
        let end = i + word.len();
        (i == 0 || !ident_char(b[i - 1])) && (end == b.len() || !ident_char(b[end]))
    })
}

/// For each `.method(|..| body)` or `.method(move |..| body)` in `src` with
/// `method` one of [`LOCKED`]: the byte offset of the call and the body.
fn locked_closures(src: &str) -> Vec<(usize, &str)> {
    let b = src.as_bytes();
    let mut found = vec![];
    for (dot, _) in src.match_indices('.') {
        let rest = &src[dot + 1..];
        let Some(m) = LOCKED.iter().find(|m| rest.starts_with(**m) && rest[m.len()..].starts_with('(')) else {
            continue;
        };
        let open = dot + 1 + m.len();
        let after = src[open + 1..].trim_start();
        let after = after.strip_prefix("move").map_or(after, str::trim_start);
        let Some(args) = after.strip_prefix('|') else { continue };
        // past the closure's arguments
        let Some(bar) = args.find('|') else { continue };
        let body_start = src.len() - args.len() + bar + 1;
        let mut depth = 1;
        let mut end = open + 1;
        while end < b.len() {
            match b[end] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            end += 1;
        }
        if body_start < end {
            found.push((dot, &src[body_start..end]));
        }
    }
    found
}

#[test]
fn no_context_use_inside_its_lock() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = vec![];
    rust_files(&crates, &mut files);
    assert!(files.len() > 10, "found too few sources under {}", crates.display());
    let mut bad = vec![];
    // (not this file: its example is one)
    for f in files.iter().filter(|f| !f.ends_with("tests/egui_locks.rs")) {
        let Ok(src) = std::fs::read_to_string(f) else { continue };
        for (at, body) in locked_closures(&src) {
            if let Some(w) = REENTRANT.iter().find(|w| names(body, w)) {
                let line = src[..at].matches('\n').count() + 1;
                bad.push(format!("{}:{line}: uses `{w}` inside a locked closure", f.display()));
            }
        }
    }
    assert!(bad.is_empty(), "the egui Context is used with its lock held (deadlocks):\n{}", bad.join("\n"));
}

#[test]
fn the_check_finds_one() {
    let src = "let f = ctx.input(|i| (i.time, ctx.cumulative_pass_nr()));\nui.ctx().data_mut(move |d| d.clear());";
    let found = locked_closures(src);
    assert_eq!(found.len(), 2);
    assert!(names(found[0].1, "ctx"));
    assert!(!REENTRANT.iter().any(|w| names(found[1].1, w)));
}
