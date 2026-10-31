//! The image servers' traffic, for debugging a server (DESIGN.md, Server
//! logs): each request to an sd-server or KoboldCpp image server and its
//! answer, in [`FILE`] in the log folder, while it's on (Server settings'
//! **Log server traffic**, or `INCREART_TRAFFIC_LOG=1`). Base64 data shows
//! as its length and long bodies are cut; a poll answered as the last time
//! isn't written again, the next different answer says how many were.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

pub const FILE: &str = "server-traffic.log";
/// The one before, once [`FILE`] grew past [`ROTATE`].
pub const PREVIOUS: &str = "server-traffic-previous.log";
const ROTATE: u64 = 8 << 20;
/// A body is cut after this many characters (base64 data shortened first).
const BODY_MAX: usize = 4000;
/// The environment variable that turns it on whatever the setting.
pub const ENV: &str = "INCREART_TRAFFIC_LOG";

static ON: AtomicBool = AtomicBool::new(false);
static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

struct Log {
    dir: PathBuf,
    file: Option<File>,
    /// Per GET URL: the last answer written, and how many like it since.
    last: HashMap<String, (String, u32)>,
}

/// Where it's written (once; nothing is before: tests never call it).
pub fn start(dir: &Path) {
    let _ = LOG.set(Mutex::new(Log { dir: dir.to_path_buf(), file: None, last: HashMap::new() }));
}

/// Turns it on or off ([`ENV`] keeps it on).
pub fn set_on(on: bool) {
    let was = enabled();
    ON.store(on, Ordering::Relaxed);
    if was != enabled() {
        note(if enabled() { "on" } else { "off" });
    }
}

pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed) || std::env::var_os(ENV).is_some_and(|v| !v.is_empty() && v != "0")
}

/// The file, when it's written to.
pub fn path() -> Option<PathBuf> {
    LOG.get().map(|l| l.lock().unwrap_or_else(|e| e.into_inner()).dir.join(FILE))
}

fn note(text: &str) {
    write(&format!("{} traffic log {text}\n", now()));
}

/// One request and its answer: `answer` is the status and the body, or
/// what went wrong (no answer, unreadable).
pub fn exchange(method: &str, url: &str, body: Option<&Value>, answer: &Result<(u16, String), String>, took: Duration) {
    if !enabled() {
        return;
    }
    let Some(log) = LOG.get() else { return };
    let ms = took.as_millis();
    let (head, reply) = match answer {
        Ok((status, text)) => (format!("{status} in {ms} ms"), shown_text(text)),
        Err(e) => (format!("no answer after {ms} ms: {e}"), String::new()),
    };
    let mut line = format!("{} {method} {url} → {head}", now());
    let mut l = log.lock().unwrap_or_else(|e| e.into_inner());
    if method == "GET" {
        // the answer without its time
        let key = match answer {
            Ok((status, _)) => format!("{status} {reply}"),
            Err(e) => e.clone(),
        };
        match l.last.get_mut(url) {
            Some((last, n)) if *last == key => {
                *n += 1;
                return;
            }
            Some((_, n)) if *n > 0 => line.push_str(&format!(" (after {n} answers as the one before)")),
            _ => {}
        }
        if l.last.len() > 1000 {
            l.last.clear();
        }
        l.last.insert(url.to_string(), (key, 0));
    }
    line.push('\n');
    if let Some(b) = body {
        line.push_str(&format!("  sent: {}\n", cut(&redacted(b).to_string())));
    }
    if !reply.is_empty() {
        line.push_str(&format!("  answer: {reply}\n"));
    }
    l.write(&line);
}

fn write(text: &str) {
    if let Some(log) = LOG.get() {
        log.lock().unwrap_or_else(|e| e.into_inner()).write(text);
    }
}

impl Log {
    fn write(&mut self, text: &str) {
        let path = self.dir.join(FILE);
        if self.file.is_some() && fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE) {
            self.file = None;
            let _ = fs::rename(&path, self.dir.join(PREVIOUS));
        }
        if self.file.is_none() {
            self.file = fs::create_dir_all(&self.dir)
                .and_then(|_| OpenOptions::new().create(true).append(true).open(&path))
                .ok();
        }
        if let Some(f) = &mut self.file {
            let _ = f.write_all(text.as_bytes());
        }
    }
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// A body as written: JSON with its base64 data shortened, else the text;
/// cut after [`BODY_MAX`] characters either way.
fn shown_text(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => cut(&redacted(&v).to_string()),
        Err(_) => cut(text.trim()),
    }
}

fn cut(s: &str) -> String {
    match s.char_indices().nth(BODY_MAX) {
        Some((i, _)) => format!("{}… ({} characters in all)", &s[..i], s.chars().count()),
        None => s.to_string(),
    }
}

/// `v` with base64 data (long strings without spaces) as its length.
pub fn redacted(v: &Value) -> Value {
    match v {
        Value::String(s) if s.len() > 200 && !s.contains(' ') => Value::String(format!("<{} characters>", s.len())),
        Value::Array(a) => Value::Array(a.iter().map(redacted).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), redacted(v))).collect()),
        v => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bodies_are_shortened() {
        let png = "A".repeat(5000);
        assert_eq!(shown_text(&json!({ "images": [png] }).to_string()), r#"{"images":["<5000 characters>"]}"#);
        assert_eq!(shown_text("  not json \n"), "not json");
        let long = "word ".repeat(2000);
        let shown = shown_text(&long);
        assert!(shown.ends_with("… (9999 characters in all)"), "{}", &shown[shown.len() - 40..]);
    }
}
