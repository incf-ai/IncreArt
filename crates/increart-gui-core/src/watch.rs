//! Watching the library for changes made outside the app (DESIGN.md, Idea
//! inbox and Review): a file manager moving or deleting folders, a phone sync
//! filling `input/inbox_drop/`. Changes are gathered for a moment, then the
//! index is scanned again; files in the drop folder are imported once they
//! have stopped growing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::update::Msg;

/// Changes are gathered this long (a sync dropping 30 files: one scan).
const QUIET: Duration = Duration::from_millis(500);
/// A dropped file is imported once its size has held still this long.
const STABLE: Duration = Duration::from_secs(2);
/// Where inotify can't watch (network drives): a scan this often.
const POLL: Duration = Duration::from_secs(3);

const DROP: &str = "input/inbox_drop";
const IMPORTED: &str = "input/inbox_drop/imported";

/// The watched parts of the library; the image stores, thumbnails and
/// caches change all the time and never by hand.
const WATCHED: [&str; 3] = ["input", "projects", "collections"];

/// A running watch; dropping it stops it.
pub struct LibraryWatch {
    _watcher: Box<dyn Watcher + Send>,
    _stop: Sender<()>,
}

/// Watches `root`; sends [`Msg::LibraryChanged`], [`Msg::DropReady`] and
/// [`Msg::WatchStatus`] to `tx`, calling `wake` after each.
pub fn start(root: PathBuf, tx: Sender<Msg>, wake: Arc<dyn Fn() + Send + Sync>) -> LibraryWatch {
    let (events, rx) = mpsc::channel::<PathBuf>();
    let handler = move |res: notify::Result<notify::Event>| {
        if let Ok(e) = res
            && !matches!(e.kind, EventKind::Access(_))
        {
            for p in e.paths {
                let _ = events.send(p);
            }
        }
    };
    let (watcher, status): (Box<dyn Watcher + Send>, String) =
        match watch_all(notify::recommended_watcher(handler.clone()), &root) {
            Ok(w) => (w, "watching for changes".into()),
            Err(e) => {
                let config = notify::Config::default().with_poll_interval(POLL);
                match watch_all(notify::PollWatcher::new(handler, config), &root) {
                    Ok(w) => (w, format!("checking every {} s (no change notifications here: {e})", POLL.as_secs())),
                    Err(e) => (Box::new(NoWatcher), format!("not watching ({e}); use Refresh")),
                }
            }
        };
    let (stop, stopped) = mpsc::channel();
    let (rtx, rwake) = (tx.clone(), wake.clone());
    let spawned = std::thread::Builder::new()
        .name("library watch".into())
        .spawn(move || run(&root, &rx, &stopped, &rtx, &*rwake));
    let (watcher, status) = match spawned {
        Ok(_) => (watcher, status),
        Err(e) => (Box::new(NoWatcher) as Box<dyn Watcher + Send>, format!("not watching ({e}); use Refresh")),
    };
    let _ = tx.send(Msg::WatchStatus(status));
    wake();
    LibraryWatch { _watcher: watcher, _stop: stop }
}

fn watch_all<W: Watcher + Send + 'static>(
    w: notify::Result<W>,
    root: &Path,
) -> Result<Box<dyn Watcher + Send>, String> {
    let mut w = w.map_err(|e| e.to_string())?;
    for dir in WATCHED {
        let d = root.join(dir);
        if d.is_dir() {
            w.watch(&d, RecursiveMode::Recursive).map_err(|e| e.to_string())?;
        }
    }
    Ok(Box::new(w))
}

/// A stand-in when nothing can watch.
struct NoWatcher;

impl Watcher for NoWatcher {
    fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
        Ok(NoWatcher)
    }
    fn watch(&mut self, _: &Path, _: RecursiveMode) -> notify::Result<()> {
        Ok(())
    }
    fn unwatch(&mut self, _: &Path) -> notify::Result<()> {
        Ok(())
    }
    fn kind() -> notify::WatcherKind {
        notify::WatcherKind::NullWatcher
    }
}

/// The files waiting in the drop folder, with their sizes.
fn drop_sizes(root: &Path) -> BTreeMap<PathBuf, u64> {
    let Ok(rd) = std::fs::read_dir(root.join(DROP)) else { return BTreeMap::new() };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| (e.path(), e.metadata().map_or(0, |m| m.len())))
        .collect()
}

fn run(root: &Path, rx: &Receiver<PathBuf>, stop: &Receiver<()>, tx: &Sender<Msg>, wake: &(dyn Fn() + Send + Sync)) {
    let send = |m: Msg| {
        let _ = tx.send(m);
        wake();
    };
    // Something changed and wasn't reported yet; the drop folder changed (or
    // had files waiting at start) and its sizes when last looked at.
    let mut changed: Option<Instant> = None;
    let mut drop_pending = !drop_sizes(root).is_empty();
    let mut drop_seen: Option<(Instant, BTreeMap<PathBuf, u64>)> = None;
    // events may name the root's real path rather than the one opened (macOS
    // reports /private/var for /var; a library opened through a symlink)
    let real = root.canonicalize().ok();
    loop {
        if !matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            return; // the watch was dropped
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(p) => {
                let rel = p
                    .strip_prefix(root)
                    .ok()
                    .or_else(|| real.as_deref().and_then(|r| p.strip_prefix(r).ok()))
                    .unwrap_or(&p);
                if rel.starts_with(IMPORTED) {
                    continue; // where imported originals go: not news
                }
                changed = Some(Instant::now());
                if rel.starts_with(DROP) {
                    drop_pending = true;
                    drop_seen = None;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if changed.is_some_and(|t| t.elapsed() >= QUIET) {
            changed = None;
            send(Msg::LibraryChanged);
        }
        if drop_pending {
            let now = drop_sizes(root);
            match &drop_seen {
                _ if now.is_empty() => {
                    (drop_pending, drop_seen) = (false, None);
                }
                Some((since, sizes)) if *sizes == now => {
                    if since.elapsed() >= STABLE {
                        (drop_pending, drop_seen) = (false, None);
                        send(Msg::DropReady);
                    }
                }
                _ => drop_seen = Some((Instant::now(), now)),
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Opened through a symlink (and on macOS, under /var, which events name
    /// /private/var): a file dropped into the drop folder is still noticed.
    #[test]
    fn a_library_opened_through_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        for d in WATCHED {
            std::fs::create_dir_all(real.join(d)).unwrap();
        }
        std::fs::create_dir_all(real.join(DROP)).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (tx, rx) = mpsc::channel();
        let _watch = start(link, tx, Arc::new(|| {}));
        std::thread::sleep(Duration::from_millis(300)); // the watch is set up
        std::fs::write(real.join(DROP).join("photo.png"), b"not really").unwrap();
        let until = Instant::now() + Duration::from_secs(20);
        let mut changed = false;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(Msg::DropReady) => return assert!(changed, "DropReady without LibraryChanged"),
                Ok(Msg::LibraryChanged) => changed = true,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        panic!("the dropped file wasn't noticed (changed: {changed})");
    }
}
