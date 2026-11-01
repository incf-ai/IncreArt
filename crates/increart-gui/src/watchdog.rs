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
