//! The app's own log, `increart.log` in the log folder beside the servers'
//! (DESIGN.md, Server logs): when it started and as what, every message
//! the Messages list shows, and a panic's. Nothing is written until
//! [`start`] (tests never call it).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub const FILE: &str = "increart.log";
/// The one before, once [`FILE`] grew past [`ROTATE`] (also `.log`: the
/// log viewer lists it).
pub const PREVIOUS: &str = "increart-previous.log";
const ROTATE: u64 = 2 << 20;

static LOG: OnceLock<Mutex<File>> = OnceLock::new();
static ABOUT: OnceLock<String> = OnceLock::new();

/// Opens `dir`/[`FILE`] to add to (the old one moved to [`PREVIOUS`] once
/// it's grown past 2 MB) and writes a start line with `about` (the version,
/// the config used). Once only; a log that can't be opened is no log.
pub fn start(dir: &Path, about: &str) {
    let _ = ABOUT.set(about.to_string());
    let path = dir.join(FILE);
    if fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE) {
        let _ = fs::rename(&path, dir.join(PREVIOUS));
    }
    let file = fs::create_dir_all(dir).and_then(|_| OpenOptions::new().create(true).append(true).open(&path));
    if let Ok(mut f) = file {
        let _ = f.write_all(crate::process::log_note(&format!("started, {}", about.replace('\n', "; "))).as_bytes());
        let _ = LOG.set(Mutex::new(f));
    }
}

/// What [`start`] was given; None before.
pub fn about() -> Option<&'static str> {
    ABOUT.get().map(String::as_str)
}

/// Adds a line, `time kind: text` (a text of several lines indented after
/// its first). Nothing before [`start`].
pub fn line(kind: &str, text: &str) {
    let Some(log) = LOG.get() else { return };
    let time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let text = text.trim_end().replace('\n', "\n    ");
    if let Ok(mut f) = log.lock() {
        let _ = writeln!(f, "{time} {kind}: {text}");
    }
}

/// Logs panics (then does what was done before: prints them).
pub fn log_panics() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let at = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        line("panic", &format!("thread '{thread}'{at}: {what}"));
        before(info);
    }));
}
