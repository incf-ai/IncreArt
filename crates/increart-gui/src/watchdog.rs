//! Notes in the app's log when the window stops updating: a frame asked for
//! that hasn't come in [`STALLED`] (a hang, say a lock waited on forever, or
//! something slow done on the GUI thread), and when it comes after all.
//! Release builds have no other trace of a hang (egui's own lock gives up
//! with a panic after 10 s in debug builds only).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long a frame asked for may take before it is noted.
const STALLED: Duration = Duration::from_secs(10);
/// How long the window may go without a frame before one is asked for.
const QUIET: Duration = Duration::from_secs(2);

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
/// When (ms after [`START`]) the last frame began.
static BEAT: AtomicU64 = AtomicU64::new(0);
/// When a frame was last asked for, as the window had been quiet.
static ASKED: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// A frame began (each [`eframe::App::logic`]).
pub fn beat() {
    BEAT.store(now_ms(), Ordering::Relaxed);
}

/// Starts the threads: one asking for a frame when the window has been quiet
/// (blocked for good if the hang holds egui's lock, so it is on its own),
/// one noting a frame asked for that doesn't come.
pub fn start(ctx: &eframe::egui::Context) {
    beat();
    let ctx = ctx.clone();
    let asker = std::thread::Builder::new().name("watchdog ask".into()).spawn(move || {
        loop {
            std::thread::sleep(QUIET);
            if now_ms().saturating_sub(BEAT.load(Ordering::Relaxed)) >= QUIET.as_millis() as u64 {
                ASKED.store(now_ms(), Ordering::Relaxed);
                ctx.request_repaint();
            }
        }
    });
    let watcher = std::thread::Builder::new().name("watchdog".into()).spawn(|| {
        let mut noted: Option<u64> = None;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let (beat, asked) = (BEAT.load(Ordering::Relaxed), ASKED.load(Ordering::Relaxed));
            match noted {
                None if asked > beat && now_ms() - asked > STALLED.as_millis() as u64 => {
                    increart_core::applog::line(
                        "hang",
                        &format!(
                            "the window hasn't drawn for {} s (a frame was asked for {} s ago){}",
                            (now_ms() - beat) / 1000,
                            (now_ms() - asked) / 1000,
                            gui_thread_state()
                        ),
                    );
                    noted = Some(beat);
                }
                Some(at) if BEAT.load(Ordering::Relaxed) != at => {
                    let took = BEAT.load(Ordering::Relaxed).saturating_sub(at) / 1000;
                    increart_core::applog::line("hang", &format!("the window is drawing again, after {took} s"));
                    noted = None;
                }
                _ => {}
            }
        }
    });
    for (what, r) in [("asker", asker), ("watcher", watcher)] {
        if let Err(e) = r {
            increart_core::applog::line("error", &format!("can't start the hang watchdog's {what}: {e}"));
        }
    }
}

/// What the GUI thread (the process's first) is waiting in, if the system
/// says: a `futex` wait is a lock (or a thread joined, or a channel).
#[cfg(target_os = "linux")]
fn gui_thread_state() -> String {
    let task = format!("/proc/self/task/{}", std::process::id());
    let read = |f: &str| std::fs::read_to_string(format!("{task}/{f}")).ok().map(|s| s.trim().to_string());
    // (stat: "pid (name) S ...": the state after the name's closing paren)
    let state =
        read("stat").and_then(|s| s.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().next().map(String::from)));
    match (state, read("wchan")) {
        (Some(s), Some(w)) => format!("; its GUI thread is in state {s}, waiting in {w}"),
        (Some(s), None) => format!("; its GUI thread is in state {s}"),
        _ => String::new(),
    }
}

#[cfg(not(target_os = "linux"))]
fn gui_thread_state() -> String {
    String::new()
}

/// A message whose update takes this long is noted in the app's log.
pub const SLOW_MSG: Duration = Duration::from_millis(250);
/// A frame taking this long to draw is noted.
pub const SLOW_FRAME: Duration = Duration::from_secs(1);

/// Notes `what` in the app's log when `took` reached `limit`.
pub fn note_slow(what: &str, took: Duration, limit: Duration) {
    if took >= limit {
        increart_core::applog::line("slow", &format!("{what} took {} ms", took.as_millis()));
    }
}

/// Times a frame's drawing, noted (on drop) when it reached [`SLOW_FRAME`].
pub struct SlowFrame(Instant);

impl SlowFrame {
    pub fn start() -> SlowFrame {
        SlowFrame(Instant::now())
    }
}

impl Drop for SlowFrame {
    fn drop(&mut self) {
        note_slow("drawing a frame", self.0.elapsed(), SLOW_FRAME);
    }
}

/// The start of `v`'s Debug, at most `max` bytes: formatting stops there,
/// so a message holding a whole index costs no more than a short one.
pub fn short_debug(v: &impl std::fmt::Debug, max: usize) -> String {
    struct Upto(String, usize);
    impl std::fmt::Write for Upto {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            let room = self.1 - self.0.len();
            if s.len() <= room {
                self.0.push_str(s);
                return Ok(());
            }
            let cut = (0..=room).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
            self.0.push_str(&s[..cut]);
            Err(std::fmt::Error)
        }
    }
    let mut w = Upto(String::new(), max);
    let full = std::fmt::write(&mut w, format_args!("{v:?}")).is_ok();
    if !full {
        w.0.push('…');
    }
    w.0
}

#[cfg(test)]
mod tests {
    use super::short_debug;

    #[test]
    fn short_debug_stops_at_max() {
        assert_eq!(short_debug(&Some(1), 80), "Some(1)");
        assert_eq!(short_debug(&"é".repeat(50), 9), "\"éééé…");
    }
}
