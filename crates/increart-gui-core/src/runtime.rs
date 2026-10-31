//! Carries out [`Effect`]s off the GUI thread and reports back as [`Msg`]s:
//! library operations in order on one worker thread, servers opened in
//! parallel, engine events forwarded on [`Runtime::poll`].
//!
//! Each piece of state has one owner where it can: the GUI thread keeps the
//! cancel flags, the library watch and the engine's events (in [`Runtime`]);
//! the PE dispatcher thread keeps the PE servers and their asks
//! (`PeDispatch`). What the library worker, the threads opening servers
//! and the GUI thread all reach (the library, the servers, the engine) is
//! the `Session`, behind a lock: shutting down needs it while the worker
//! may be busy for minutes (a scan, a large save).

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use increart_core::config::{RecipePreset, SdServerConfig, ServerConfig};
use increart_core::engine::{Endpoint, Engine, EngineOptions, Event};
use increart_core::launcher::{self, ServerProcess};
use increart_core::model::{IdeaSource, LoraFile, Mode, ModelFile, ModelSelection, Sample, StoreFormat};
use increart_core::request::GenOptions;
use increart_core::sync::LockExt;
use increart_core::{Id, IdeaLoc, ImageRef, Library, NewIdea, NewRun, Tree, Verdict, modelhash};

use crate::state::{EntryKind, FvEntry, GenerateForm, PickKind};
use crate::update::{DraftSave, Effect, LibOp, Msg, OpOk, PeJob, TileTarget};

/// The library and the image servers with the engine using them.
#[derive(Default)]
struct Session {
    library: Option<Arc<Mutex<Library>>>,
    /// The sd-servers open, each with what it was opened as.
    servers: Vec<Option<(ServerConfig, ServerProcess)>>,
    /// The shared settings `servers` were opened with.
    opened_with: Option<SdServerConfig>,
    engine: Option<Engine>,
    /// The opening attempt `servers` belongs to.
    attempt: u32,
    /// Set to stop the current opening attempt (and servers it started).
    cancel: Arc<AtomicBool>,
    /// Servers still being opened.
    opening: Arc<AtomicUsize>,
}

/// Set by tests before anything starts (a second setting is ignored).
#[derive(Default)]
struct Paths {
    /// Where libraries put deleted things; unset: the system trash.
    disposal: OnceLock<increart_core::library::Disposal>,
    /// Where started servers log; unset: [`log_dir`].
    log_dir: OnceLock<PathBuf>,
}

impl Paths {
    fn logs(&self) -> PathBuf {
        self.log_dir.get().cloned().unwrap_or_else(log_dir)
    }
}

type Wake = Arc<dyn Fn() + Send + Sync>;

/// What the library worker and the threads it starts share.
#[derive(Clone)]
struct Ctx {
    tx: Sender<Msg>,
    wake: Wake,
    session: Arc<Mutex<Session>>,
    paths: Arc<Paths>,
    /// A started engine's events, to the GUI thread (None: it stopped).
    engine_events: Sender<Option<Receiver<Event>>>,
}

impl Ctx {
    fn library(&self) -> Option<Arc<Mutex<Library>>> {
        self.session.lock_or_recover().library.clone()
    }

    /// The GUI thread stops reading the engine's events (taken out of the
    /// session by the caller).
    fn engine_stopped(&self) {
        let _ = self.engine_events.send(None);
    }
}

pub struct Runtime {
    ctx: Ctx,
    rx: Receiver<Msg>,
    work: Sender<Effect>,
    /// Effects sent to `work` and not yet done (their messages sent).
    queued: Arc<AtomicUsize>,
    /// To the PE dispatcher.
    pe: Sender<PeCmd>,
    /// The running engine's events, and its replacements as the library
    /// worker starts and stops engines.
    engine_events: Cell<Option<Receiver<Event>>>,
    engine_updates: Receiver<Option<Receiver<Event>>>,
    /// Set to stop the model download, store conversion or archive going on.
    download_cancel: Cell<Arc<AtomicBool>>,
    convert_cancel: Cell<Arc<AtomicBool>>,
    archive_cancel: Cell<Arc<AtomicBool>>,
    /// The open library's watch for outside changes.
    watch: Cell<Option<crate::watch::LibraryWatch>>,
}

/// `logs` in the app's state folder (`~/.local/state/increart` on Linux;
/// see [`user_dir`](increart_core::config::user_dir)).
pub fn log_dir() -> PathBuf {
    increart_core::config::user_dir("XDG_STATE_HOME", ".local/state", "LOCALAPPDATA")
        .unwrap_or_else(|| std::env::temp_dir().join("increart"))
        .join("logs")
}

/// A fresh cancel flag for an operation starting, kept in `slot` for its
/// Cancel.
fn new_cancel(slot: &Cell<Arc<AtomicBool>>) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    slot.set(cancel.clone());
    cancel
}

fn cancel(slot: &Cell<Arc<AtomicBool>>) {
    slot.take().store(true, Ordering::Relaxed);
}

impl Runtime {
    /// Libraries opened after this put what they delete in `dir` rather than
    /// the system trash (tests: macOS's trash goes through Finder).
    pub fn set_trash_dir(&self, dir: PathBuf) {
        let _ = self.ctx.paths.disposal.set(increart_core::library::Disposal::Directory(dir));
    }

    /// Servers started after this log into `dir` rather than [`log_dir`].
    pub fn set_log_dir(&self, dir: PathBuf) {
        let _ = self.ctx.paths.log_dir.set(dir);
    }

    /// `wake` is called whenever a message is ready (e.g. request a repaint).
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Runtime> {
        let (tx, rx) = mpsc::channel();
        let (work, jobs) = mpsc::channel::<Effect>();
        let (engine_events, engine_updates) = mpsc::channel();
        let ctx = Ctx { tx, wake: Arc::new(wake), session: Arc::default(), paths: Arc::default(), engine_events };
        let queued = Arc::new(AtomicUsize::new(0));
        let (wctx, wqueued) = (ctx.clone(), queued.clone());
        std::thread::Builder::new().name("library".into()).spawn(move || {
            for fx in jobs {
                // Opening goes through this queue so it can't overtake a
                // StopServers sent just before it.
                if let Effect::OpenServers { servers, shared, attempt } = fx {
                    open_servers(&wctx, servers, shared, attempt);
                } else {
                    let started = matches!(fx, Effect::StartEngine { .. });
                    let msgs = work_on(&wctx, fx);
                    for m in msgs {
                        let _ = wctx.tx.send(m);
                    }
                    if started {
                        list_loadable(&wctx);
                    }
                    (wctx.wake)();
                }
                wqueued.fetch_sub(1, Ordering::SeqCst);
            }
        })?;
        let (pe, cmds) = mpsc::channel();
        let dispatch = PeDispatch::new(PeCtx {
            tx: ctx.tx.clone(),
            wake: ctx.wake.clone(),
            cmds: pe.clone(),
            paths: ctx.paths.clone(),
        });
        std::thread::Builder::new().name("prompt enhance".into()).spawn(move || dispatch.run(cmds))?;
        Ok(Runtime {
            ctx,
            rx,
            work,
            queued,
            pe,
            engine_events: Cell::new(None),
            engine_updates,
            download_cancel: Cell::default(),
            convert_cancel: Cell::default(),
            archive_cancel: Cell::default(),
            watch: Cell::new(None),
        })
    }

    /// Runs `f` on a thread of its own, saying so if it can't be started.
    fn spawn(&self, name: &str, f: impl FnOnce() + Send + 'static) {
        if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
            let _ = self.ctx.tx.send(Msg::Problem(format!("couldn't start the {name} thread: {e}")));
            (self.ctx.wake)();
        }
    }

    /// Runs `f` on a thread of its own and sends what it returns.
    fn answer(&self, name: &str, f: impl FnOnce() -> Msg + Send + 'static) {
        let (tx, wake) = (self.ctx.tx.clone(), self.ctx.wake.clone());
        self.spawn(name, move || {
            let _ = tx.send(f());
            wake();
        });
    }

    pub fn run(&self, fx: Effect) {
        match fx {
            Effect::PlaySound { sound, volume, player, test } => self
                .answer("sound", move || Msg::SoundPlayed(increart_core::notify::play(&sound, volume, &player), test)),
            Effect::ListDevices(binary) => self.answer("list devices", move || {
                Msg::Devices(launcher::list_devices(&binary).map_err(|e| format!("{binary}: {e}")))
            }),
            Effect::KoboldVersion(binary) => self.answer("KoboldCpp version", move || {
                let r = increart_core::kobold::version(&binary);
                Msg::KoboldVersion(binary, r)
            }),
            Effect::DetectGpus(runtime) => self.answer("detect GPUs", move || {
                Msg::WizardGpus(increart_core::tiers::detect_gpus(runtime), increart_core::tiers::available_ram_gib())
            }),
            Effect::DownloadModels { dir, files, model_dirs } => {
                let (tx, wake) = (self.ctx.tx.clone(), self.ctx.wake.clone());
                let cancel = new_cancel(&self.download_cancel);
                self.spawn("model download", move || {
                    let search = increart_core::tiers::search_dirs(&model_dirs);
                    let r = download_models(&dir, &files, &search, &cancel, &tx, &wake);
                    let _ = tx.send(Msg::WizardDownloaded(r));
                    wake();
                });
            }
            Effect::CancelDownload => cancel(&self.download_cancel),
            Effect::FolderPreviews { root, dir, names } => self.answer("folder previews", move || {
                let found = names
                    .into_iter()
                    .map(|n| (n.clone(), previews(&root.join(&dir).join(n), store_of(&dir))))
                    .collect();
                Msg::FvPreviews(dir, found)
            }),
            Effect::CheckUsage => {
                let Some(lib) = self.ctx.library() else { return };
                let ctx = self.ctx.clone();
                self.spawn("usage check", move || check_usage(&lib, &ctx));
            }
            Effect::ConvertStore(to) => {
                let Some(lib) = self.ctx.library() else { return };
                let ctx = self.ctx.clone();
                let cancel = new_cancel(&self.convert_cancel);
                self.spawn("store conversion", move || convert_store(&lib, &ctx, to, &cancel));
            }
            Effect::SaveBugReport { config, model_dirs, dest } => {
                let report = increart_core::bugreport::Report {
                    config,
                    logs: self.ctx.paths.logs(),
                    model_dirs: model_dirs.iter().map(|d| increart_core::config::full_path(d)).collect(),
                    about: increart_core::applog::about().unwrap_or("increart (version not known)").to_string(),
                };
                self.answer("bug report", move || {
                    let r = increart_core::bugreport::write(&report, &dest);
                    Msg::BugReportSaved(r.map(|size| (dest, size)))
                });
            }
            Effect::ExportLibrary { root, dest, format } => {
                let (tx, wake) = (self.ctx.tx.clone(), self.ctx.wake.clone());
                let cancel = new_cancel(&self.archive_cancel);
                self.spawn("export library", move || {
                    let mut progress = archive_progress(&tx, &wake, &cancel);
                    let r = increart_core::archive::export(&root, &dest, format, &mut progress);
                    let _ = tx.send(Msg::LibraryExported(r.map(|size| (dest, size)).map_err(archive_error)));
                    wake();
                });
            }
            Effect::UnpackArchive { archive, into } => {
                let (tx, wake) = (self.ctx.tx.clone(), self.ctx.wake.clone());
                let cancel = new_cancel(&self.archive_cancel);
                self.spawn("unpack library", move || {
                    let mut progress = archive_progress(&tx, &wake, &cancel);
                    let r = increart_core::archive::unpack(&archive, &into, &mut progress);
                    let _ = tx.send(Msg::ArchiveUnpacked(r.map_err(archive_error)));
                    wake();
                });
            }
            Effect::CancelArchive => cancel(&self.archive_cancel),
            Effect::CancelConvert => cancel(&self.convert_cancel),
            Effect::WatchLibrary(root) => {
                let w = crate::watch::start(root, self.ctx.tx.clone(), self.ctx.wake.clone());
                self.watch.set(Some(w));
            }
            Effect::ComposeTiles { root, layout, settings, to } => self.answer("compose tiles", move || {
                let r = crate::tile::compose(&root, &layout, &settings).and_then(|img| {
                    if let TileTarget::File(path) = &to {
                        let bytes = increart_core::export::encode(&img, settings.save_format, settings.save_quality)
                            .map_err(|e| format!("{}: {e}", path.display()))?;
                        std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                    }
                    Ok(crate::tile::TileImage(Arc::new(img)))
                });
                Msg::TilesDone(to, r)
            }),
            Effect::PeAsk { servers, jobs } => {
                let _ = self.pe.send(PeCmd::Configure(servers, jobs));
            }
            Effect::PeStop(ids) => {
                let _ = self.pe.send(PeCmd::Stop(ids));
            }
            Effect::PeShutdown => {
                let _ = self.pe.send(PeCmd::Configure(vec![], vec![]));
            }
            Effect::OpenInEditor { path, command } => {
                let (tx, wake) = (self.ctx.tx.clone(), self.ctx.wake.clone());
                self.spawn("open in editor", move || {
                    if let Err(e) = open_in_editor(&path, &command) {
                        let _ = tx.send(Msg::Problem(e));
                        wake();
                    }
                });
            }
            Effect::PeListGpus(runtime) => {
                self.answer("list GPUs", move || Msg::PeGpus(runtime, increart_core::pe::list_gpus(runtime)))
            }
            Effect::Pick { kind, start } => self.answer("file picker", move || Msg::Picked(kind, pick(kind, start))),
            other => {
                self.queued.fetch_add(1, Ordering::SeqCst);
                if self.work.send(other).is_err() {
                    self.queued.fetch_sub(1, Ordering::SeqCst);
                }
            }
        }
    }

    /// Messages ready since the last call, including engine events.
    pub fn poll(&self) -> Vec<Msg> {
        let mut out: Vec<Msg> = self.rx.try_iter().collect();
        let mut events = self.engine_events.take();
        for update in self.engine_updates.try_iter() {
            events = update;
        }
        if let Some(events) = &events {
            out.extend(events.try_iter().map(Msg::Engine));
        }
        self.engine_events.set(events);
        out
    }

    /// Carries out a draft save on the calling thread (quitting: the worker
    /// thread may not get to it).
    pub fn save_now(&self, fx: Effect) {
        if matches!(fx, Effect::SaveDraft(_)) {
            for m in work_on(&self.ctx, fx) {
                let _ = self.ctx.tx.send(m);
            }
        }
    }

    /// Nothing sent to the library worker is still to do, and the messages
    /// of what it did are ready (tests: effects with no reply, like saving
    /// prompt presets or reading a history, are done).
    pub fn idle(&self) -> bool {
        self.queued.load(Ordering::SeqCst) == 0
    }

    /// Waits up to `timeout` for at least one message (tests).
    pub fn wait(&self, timeout: Duration) -> Vec<Msg> {
        let deadline = Instant::now() + timeout;
        loop {
            let m = self.poll();
            if !m.is_empty() || Instant::now() >= deadline {
                return m;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Stops the engine and the servers the app started, or with
    /// `leave_running` lets started servers outlive the app, and closes the
    /// library (its lock let go once a job using it ends).
    pub fn shutdown(&self, leave_running: bool) {
        // Stop servers still starting (they would outlive the app), and give
        // their threads a moment to do it.
        let opening = {
            let s = self.ctx.session.lock_or_recover();
            s.cancel.store(true, Ordering::Relaxed);
            s.opening.clone()
        };
        // the PE dispatcher stops its servers (a KoboldCpp the app started
        // stops with it) while these wait
        let (pe_done, pe_stopped) = mpsc::channel();
        let pe_stopping = self.pe.send(PeCmd::Shutdown(pe_done)).is_ok();
        let deadline = Instant::now() + Duration::from_millis(1500);
        while opening.load(Ordering::Relaxed) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let (engine, mut servers, lib) = {
            let mut s = self.ctx.session.lock_or_recover();
            // threads still going (one stopping a KoboldCpp, the worker)
            // hold the session, not the library; jobs after this find none
            (s.engine.take(), std::mem::take(&mut s.servers), s.library.take())
        };
        self.engine_events.set(None);
        if let Some(engine) = engine {
            engine.shutdown();
        }
        if leave_running {
            servers.iter_mut().flatten().for_each(|(_, p)| p.detach());
        }
        drop(servers);
        self.watch.set(None);
        if pe_stopping {
            let _ = pe_stopped.recv_timeout(Duration::from_secs(10));
        }
        // a job using it (a save, a usage check) ends first
        if let Some(lib) = lib {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Arc::strong_count(&lib) > 1 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

impl Drop for Runtime {
    /// Stops the engine and the servers the app started, before the process
    /// exits (the worker thread may never get to it). The app calls
    /// [`Runtime::shutdown`] first when started servers should stay up.
    fn drop(&mut self) {
        self.shutdown(false);
    }
}

/// How often an answer being written is reported.
const PE_PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// What the PE dispatcher and its workers share.
#[derive(Clone)]
struct PeCtx {
    tx: Sender<Msg>,
    wake: Wake,
    cmds: Sender<PeCmd>,
    paths: Arc<Paths>,
}

impl PeCtx {
    fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        (self.wake)();
    }
}

/// What the PE dispatcher is told: by the GUI thread, and by its workers.
enum PeCmd {
    /// Use these servers from now (none: stop them), keeping ones open or
    /// starting that run as the same process, and queue these asks.
    Configure(Vec<increart_core::pe::PeServer>, Vec<PeJob>),
    /// These asks are stopped: dropped if queued (or when they come), cut
    /// short if being answered.
    Stop(Vec<u64>),
    /// A worker's server opened, or why it didn't.
    Opened(u64, Result<increart_core::pe::PeProcess, String>),
    /// A worker answered its ask.
    Answered(u64),
    /// Stop every server, then say so.
    Shutdown(Sender<()>),
}

/// A PE server in use and the worker answering asks with it.
struct PeSlot {
    /// Its worker's number.
    id: u64,
    server: increart_core::pe::PeServer,
    /// Once open.
    process: Option<increart_core::pe::PeProcess>,
    /// Why it didn't open.
    failed: Option<String>,
    /// Set to stop it while starting.
    cancel: Arc<AtomicBool>,
    /// Asks for its worker, each with the flag that cuts it short; dropped,
    /// the worker ends.
    asks: Sender<(PeJob, Arc<AtomicBool>)>,
    /// The ask it's answering.
    current: Option<(u64, Arc<AtomicBool>)>,
}

/// Owns the PE servers in use and the asks waiting for them, on a thread of
/// its own; workers open a server each and answer the asks they are given.
struct PeDispatch {
    ctx: PeCtx,
    slots: Vec<PeSlot>,
    queue: std::collections::VecDeque<PeJob>,
    /// Asks stopped before they came.
    stopped: std::collections::HashSet<u64>,
    /// The status and ready count last reported.
    reported: Option<(crate::state::PeStatus, usize)>,
    /// The number of the next slot.
    next: u64,
}

impl PeDispatch {
    fn new(ctx: PeCtx) -> PeDispatch {
        PeDispatch {
            ctx,
            slots: vec![],
            queue: Default::default(),
            stopped: Default::default(),
            reported: None,
            next: 0,
        }
    }

    fn run(mut self, cmds: Receiver<PeCmd>) {
        for cmd in cmds {
            match cmd {
                PeCmd::Configure(servers, jobs) => self.configure(servers, jobs),
                PeCmd::Stop(ids) => self.stop(&ids),
                PeCmd::Opened(id, opened) => self.opened(id, opened),
                PeCmd::Answered(id) => {
                    if let Some(slot) = self.slots.iter_mut().find(|p| p.id == id) {
                        slot.current = None;
                    }
                }
                PeCmd::Shutdown(done) => {
                    for p in &self.slots {
                        p.cancel.store(true, Ordering::Relaxed);
                    }
                    // dropping them stops their processes (and waits for them)
                    self.slots.clear();
                    self.queue.clear();
                    let _ = done.send(());
                    return;
                }
            }
            self.dispatch();
            self.report();
        }
    }

    /// Ones no longer used stop before new ones start (one may want the
    /// other's port).
    fn configure(&mut self, servers: Vec<increart_core::pe::PeServer>, jobs: Vec<PeJob>) {
        let mut old = std::mem::take(&mut self.slots);
        let mut started = vec![];
        for server in servers {
            match old.iter().position(|p| p.failed.is_none() && p.server.same_process(&server)) {
                Some(i) => self.slots.push(old.remove(i)),
                None => {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let (asks, rx) = mpsc::channel();
                    let id = self.next;
                    self.next += 1;
                    started.push((id, server.clone(), cancel.clone(), rx));
                    self.slots.push(PeSlot { id, server, process: None, failed: None, cancel, asks, current: None });
                }
            }
        }
        for p in &old {
            p.cancel.store(true, Ordering::Relaxed);
        }
        if self.slots.is_empty() {
            self.queue.clear();
        }
        for j in jobs {
            if !self.stopped.remove(&j.id) {
                self.queue.push_back(j);
            }
        }
        // dropping them stops their processes (and waits for them)
        drop(old);
        for (id, server, cancel, asks) in started {
            let ctx = self.ctx.clone();
            let name = format!("prompt enhance {}", server.name);
            let spawned = std::thread::Builder::new()
                .name(name.clone())
                .spawn(move || pe_worker(&ctx, id, server, &cancel, asks));
            if let Err(e) = spawned
                && let Some(slot) = self.slots.iter_mut().find(|p| p.id == id)
            {
                slot.failed = Some(format!("couldn't start the {name} thread: {e}"));
            }
        }
    }

    fn stop(&mut self, ids: &[u64]) {
        for &id in ids {
            let queued = self.queue.len();
            self.queue.retain(|j| j.id != id);
            if self.queue.len() < queued {
                continue;
            }
            let Some(p) = self.slots.iter().find(|p| p.current.as_ref().is_some_and(|(c, _)| *c == id)) else {
                // (may still be on its way)
                self.stopped.insert(id);
                continue;
            };
            if let Some((_, stop)) = &p.current {
                stop.store(true, Ordering::Relaxed);
            }
            // the server stops writing (it would finish the answer nobody
            // reads before starting the next)
            if matches!(p.server.run, increart_core::pe::PeRun::KoboldCpp { .. })
                && let Some(client) = p.process.as_ref().map(|p| p.client.clone())
            {
                let _ = std::thread::Builder::new().name("PE abort".into()).spawn(move || client.abort());
            }
        }
    }

    fn opened(&mut self, id: u64, opened: Result<increart_core::pe::PeProcess, String>) {
        let others_ready = self.slots.iter().any(|p| p.id != id && p.process.is_some());
        // stopped while starting: not a failure (and the process, dropped,
        // stops)
        let Some(slot) = self.slots.iter_mut().find(|p| p.id == id) else { return };
        match opened {
            Ok(p) => slot.process = Some(p),
            Err(e) => {
                if others_ready {
                    self.ctx.send(Msg::Problem(format!("{}: {e}", slot.server.name)));
                }
                slot.failed = Some(e);
            }
        }
    }

    /// Gives queued asks to the open servers not answering one.
    fn dispatch(&mut self) {
        for slot in self.slots.iter_mut().filter(|p| p.process.is_some() && p.current.is_none()) {
            let Some(job) = self.queue.pop_front() else { return };
            let (id, stop) = (job.id, Arc::new(AtomicBool::new(false)));
            match slot.asks.send((job, stop.clone())) {
                Ok(()) => slot.current = Some((id, stop)),
                // its worker is gone: the ask waits for another
                Err(mpsc::SendError((job, _))) => {
                    self.queue.push_front(job);
                    slot.process = None;
                    slot.failed = Some(format!("{}: its worker ended", slot.server.name));
                }
            }
        }
    }

    /// Reports how the PE servers are getting on, when that changed: Ready
    /// once any is, Failed (dropping the asks queued) once all failed.
    fn report(&mut self) {
        use crate::state::PeStatus;
        let ready = self.slots.iter().filter(|p| p.process.is_some()).count();
        let status = if self.slots.is_empty() {
            PeStatus::Off
        } else if ready > 0 {
            PeStatus::Ready
        } else if self.slots.iter().any(|p| p.failed.is_none()) {
            PeStatus::Starting
        } else {
            let failed: Vec<String> = self.slots.iter().filter_map(|p| p.failed.clone()).collect();
            self.queue.clear();
            PeStatus::Failed(failed.join("\n"))
        };
        let before = self.reported.replace((status.clone(), ready));
        if before.as_ref() == Some(&(status.clone(), ready)) {
            return;
        }
        self.ctx.send(Msg::PeReady(ready));
        if before.is_none_or(|(b, _)| b != status) {
            self.ctx.send(Msg::PeStatus(status));
        }
    }
}

/// Opens `server`, then answers the asks it's given, one at a time, until
/// its slot is gone.
fn pe_worker(
    ctx: &PeCtx,
    id: u64,
    server: increart_core::pe::PeServer,
    cancel: &AtomicBool,
    asks: Receiver<(PeJob, Arc<AtomicBool>)>,
) {
    use increart_core::pe;
    let opened = pe::open(&server, &ctx.paths.logs(), increart_core::config::timeouts().server_start(), cancel);
    let client = opened.as_ref().ok().map(|p| p.client.clone());
    // (the dispatcher gone: the process, dropped with the command, stops)
    if ctx.cmds.send(PeCmd::Opened(id, opened)).is_err() {
        return;
    }
    let Some(client) = client else { return };
    for (job, stop) in asks {
        ctx.send(Msg::PeOn(job.id, server.name.clone()));
        let r = pe_answer(ctx, &client, &job, &stop);
        if !stop.load(Ordering::Relaxed) {
            ctx.send(Msg::PeAnswer(job.id, r));
        }
        let _ = ctx.cmds.send(PeCmd::Answered(id));
    }
}

/// One chat request for `job`, reporting the answer so far a few times a
/// second, until done or `stop` is set.
fn pe_answer(
    ctx: &PeCtx,
    client: &increart_core::pe::Client,
    job: &PeJob,
    stop: &AtomicBool,
) -> Result<String, String> {
    use crate::state::PeStep;
    use increart_core::pe;
    if !job.images.is_empty() {
        ctx.send(Msg::PeProgress(job.id, PeStep::Preparing));
    }
    let images: Result<Vec<_>, String> = job.images.iter().map(|p| pe::image_for_request(p, job.image_size)).collect();
    ctx.send(Msg::PeProgress(job.id, PeStep::Reading { images: job.images.len() }));
    let mut last = Instant::now() - PE_PROGRESS_EVERY;
    images.and_then(|imgs| {
        client.chat_stream(&job.system, &job.text, &imgs, job.max_tokens, job.sampling, |pieces, text| {
            if last.elapsed() >= PE_PROGRESS_EVERY {
                last = Instant::now();
                ctx.send(Msg::PeProgress(job.id, PeStep::Writing { pieces, text: text.to_string() }));
            }
            !stop.load(Ordering::Relaxed)
        })
    })
}

#[cfg(not(feature = "avif"))]
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];
#[cfg(feature = "avif")]
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "avif"];

/// The system file picker: the desktop portal when reachable, else zenity
/// (rfd's fallback). Empty when cancelled.
fn pick(kind: PickKind, start: Option<PathBuf>) -> Vec<PathBuf> {
    let mut d = rfd::FileDialog::new();
    if let Some(s) = start.clone().filter(|s| s.is_dir()) {
        d = d.set_directory(s);
    }
    match kind {
        // `start` is the library: beside it, named after it
        PickKind::LibraryArchiveOut(format) => {
            let lib = start.unwrap_or_default();
            if let Some(dir) = lib.parent().filter(|p| p.is_dir()) {
                d = d.set_directory(dir);
            }
            let name = lib.file_name().map_or("library".into(), |n| n.to_string_lossy().into_owned());
            let (label, exts): (&str, &[&str]) = match format {
                increart_core::archive::ArchiveFormat::Zip => ("Zip", &["zip"]),
                increart_core::archive::ArchiveFormat::TarGz => ("tar.gz", &["tar.gz", "tgz"]),
            };
            d.set_title("Export the library to")
                .set_file_name(format!("{name}.{}", format.ext()))
                .add_filter(label, exts)
                .save_file()
                .into_iter()
                .collect()
        }
        PickKind::LibraryArchiveIn => d
            .set_title("Library archive to unpack")
            .add_filter("Library archives", &["zip", "tar.gz", "tgz"])
            .pick_file()
            .into_iter()
            .collect(),
        PickKind::LibraryUnpackInto => d
            .set_title("Folder to unpack the library into (it gets a folder of its own there)")
            .pick_folder()
            .into_iter()
            .collect(),
        PickKind::LibraryFolder => d
            .set_title("Library folder (existing, or an empty one for a new library)")
            .pick_folder()
            .into_iter()
            .collect(),
        PickKind::ModelDir(_) => {
            d.set_title("Model directory (KoboldCpp and sd-server)").pick_folder().into_iter().collect()
        }
        PickKind::SdServerBinary => d.set_title("sd-server binary").pick_file().into_iter().collect(),
        PickKind::RecipeFile(slot) => {
            let slot = increart_core::config::MODEL_SLOTS.get(slot).copied().unwrap_or("model");
            d.set_title(format!("The {slot} file (in a model directory)"))
                .add_filter("Model files", &["gguf", "safetensors", "sft", "ckpt", "pt", "pth", "bin"])
                .add_filter("All files", &["*"])
                .pick_file()
                .into_iter()
                .collect()
        }
        PickKind::RecipeLora(_) => d
            .set_title("A LoRA (in a model directory's loras folder)")
            .add_filter("LoRA files", &["safetensors", "gguf", "sft", "ckpt"])
            .add_filter("All files", &["*"])
            .pick_file()
            .into_iter()
            .collect(),
        PickKind::KoboldBinary | PickKind::WizardBinary => {
            d.set_title("KoboldCpp binary").pick_file().into_iter().collect()
        }
        PickKind::WizardModels => d.set_title("Model directory for KoboldCpp").pick_folder().into_iter().collect(),
        PickKind::Pe(f) | PickKind::Kobold(_, f) => {
            let d = match f {
                crate::state::PeFile::Binary => d.set_title("KoboldCpp binary"),
                crate::state::PeFile::Model => d.set_title("Model (.gguf)").add_filter("GGUF", &["gguf"]),
                crate::state::PeFile::Mmproj => {
                    d.set_title("Vision projector (mmproj .gguf)").add_filter("GGUF", &["gguf"])
                }
                crate::state::PeFile::Config => {
                    d.set_title("KoboldCpp settings (.kcpps)").add_filter("KoboldCpp settings", &["kcpps", "json"])
                }
            };
            d.pick_file().into_iter().collect()
        }
        PickKind::IdeaImages => {
            d.set_title("Images for the new idea").add_filter("Images", IMAGE_EXTS).pick_files().unwrap_or_default()
        }
        PickKind::ExportDir => d.set_title("Export to").pick_folder().into_iter().collect(),
        PickKind::BugReport => d
            .set_title("Save for a bug report")
            .set_file_name(format!("increart-bug-report-{}.zip", chrono::Local::now().format("%Y-%m-%d-%H%M")))
            .add_filter("Zip", &["zip"])
            .save_file()
            .into_iter()
            .collect(),
        PickKind::TileFile(format) => d
            .set_title("Save the tiled image")
            .set_file_name(format!("tiles.{}", format.ext()))
            .add_filter(format.name(), &[format.ext()])
            .save_file()
            .into_iter()
            .collect(),
        PickKind::GenerateRefs => {
            d.set_title("Reference images").add_filter("Images", IMAGE_EXTS).pick_files().unwrap_or_default()
        }
        PickKind::ImportFiles => {
            let mut exts = IMAGE_EXTS.to_vec();
            exts.push("txt");
            d.set_title("Import as new ideas").add_filter("Images and text", &exts).pick_files().unwrap_or_default()
        }
    }
}

/// What each open server can load, for the model presets it can run, as
/// [`Msg::ServerModels`] (asked in the background: model folders are walked).
fn list_loadable(ctx: &Ctx) {
    let clients: Vec<_> = ctx
        .session
        .lock_or_recover()
        .servers
        .iter()
        .flatten()
        .map(|(_, p)| (p.name.clone(), p.client.clone()))
        .collect();
    let (tx, wake) = (ctx.tx.clone(), ctx.wake.clone());
    std::thread::spawn(move || {
        let found = clients.into_iter().filter_map(|(name, c)| c.loadable().ok().map(|l| (name, l))).collect();
        let _ = tx.send(Msg::ServerModels(found));
        wake();
    });
}

/// Opens the servers of one attempt in parallel; reports each, then settles.
fn open_servers(ctx: &Ctx, servers: Vec<ServerConfig>, cfg: SdServerConfig, attempt: u32) {
    let (cancel, opening, kept, engine, old) = {
        let mut s = ctx.session.lock_or_recover();
        // the attempt before (if still opening) and the engine stop; the
        // servers opened as asked again are kept, the others stop
        s.cancel.store(true, Ordering::Relaxed);
        let engine = s.engine.take();
        let same_settings = s.opened_with.as_ref() == Some(&cfg);
        let mut old = std::mem::take(&mut s.servers);
        let slots: Vec<_> = servers
            .iter()
            .map(|c| {
                let same = |o: &Option<(ServerConfig, ServerProcess)>| {
                    o.as_ref().is_some_and(|(x, _)| x == c && (same_settings || !c.is_started()))
                };
                old.iter().position(same).and_then(|i| old[i].take())
            })
            .collect();
        let kept: Vec<bool> = slots.iter().map(Option::is_some).collect();
        s.servers = slots;
        s.opened_with = Some(cfg.clone());
        s.attempt = attempt;
        s.cancel = Arc::new(AtomicBool::new(false));
        s.opening.fetch_add(kept.iter().filter(|k| !**k).count(), Ordering::Relaxed);
        (s.cancel.clone(), s.opening.clone(), kept, engine, old)
    };
    // (stopped without the session locked: the GUI thread may need it)
    if let Some(engine) = engine {
        ctx.engine_stopped();
        engine.shutdown();
    }
    // dropping a process the app started stops it (kill, then wait)
    let stopped = old.iter().flatten().filter(|(_, p)| p.launched()).map(|(_, p)| p.name.clone()).collect();
    drop(old);
    let _ = ctx.tx.send(Msg::ServersStopped(stopped));
    (ctx.wake)();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let handles: Vec<_> = servers
            .into_iter()
            .enumerate()
            .map(|(i, server)| {
                let (tx, wake, session, cfg) = (ctx.tx.clone(), ctx.wake.clone(), ctx.session.clone(), cfg.clone());
                let logs = ctx.paths.logs();
                let (cancel, opening) = (cancel.clone(), opening.clone());
                let kept = kept[i];
                std::thread::spawn(move || {
                    if kept {
                        let _ = tx.send(Msg::ServerOpened(attempt, i, Ok(())));
                        wake();
                        return;
                    }
                    let done = Arc::new(AtomicBool::new(false));
                    if server.is_started() {
                        watch_start_log(&server, &logs, attempt, i, &tx, &wake, &done);
                    }
                    let opened = launcher::open_cancellable(
                        &server,
                        &cfg,
                        &logs,
                        increart_core::config::timeouts().server_start(),
                        &cancel,
                    );
                    done.store(true, Ordering::Relaxed);
                    opening.fetch_sub(1, Ordering::Relaxed);
                    let msg = match opened {
                        Ok(p) => {
                            let mut s = session.lock_or_recover();
                            // Kept only for the current attempt; otherwise
                            // dropped, which stops a server this app started.
                            if s.attempt == attempt
                                && let Some(slot) = s.servers.get_mut(i)
                            {
                                *slot = Some((server, p));
                            }
                            Msg::ServerOpened(attempt, i, Ok(()))
                        }
                        Err(e) => Msg::ServerOpened(attempt, i, Err(e.to_string())),
                    };
                    let _ = tx.send(msg);
                    wake();
                })
            })
            .collect();
        for h in handles {
            let _ = h.join();
        }
        let _ = ctx.tx.send(Msg::ServersSettled(attempt));
        (ctx.wake)();
    });
}

/// While a server starts: what its log says, twice a second (loading
/// tensors 294/396 …), as [`Msg::ServerLog`].
fn watch_start_log(
    server: &ServerConfig,
    logs: &Path,
    attempt: u32,
    i: usize,
    tx: &Sender<Msg>,
    wake: &Wake,
    done: &Arc<AtomicBool>,
) {
    let log = launcher::log_path(logs, server);
    let start = std::fs::metadata(&log).map_or(0, |m| m.len());
    let (tx, wake, done) = (tx.clone(), wake.clone(), done.clone());
    std::thread::spawn(move || {
        let mut last = None;
        while !done.load(Ordering::Relaxed) {
            let line = launcher::latest_line(&log, start);
            if line.is_some() && line != last {
                let _ = tx.send(Msg::ServerLog(attempt, i, line.clone().unwrap_or_default()));
                wake();
                last = line;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

/// Copies store images to `dest`, named by image ID or by run / revision /
/// seed, with or without IncreArt's and sd-server's metadata chunks.
fn export(lib: &Library, images: &[ImageRef], dest: &Path, keep_metadata: bool, by_id: bool) -> Result<usize, String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let ix = lib.scan().map_err(err)?;
    let mut n = 0;
    for img in images {
        let bytes = std::fs::read(lib.image_path(img)).map_err(|e| e.to_string())?;
        let bytes = if keep_metadata { bytes } else { increart_core::container::strip_metadata(&bytes)? };
        let format = lib.format_of(img);
        let link = ix.projects.iter().flat_map(|p| &p.outputs).find(|o| &o.image == img);
        let stem = match (by_id, link) {
            (false, Some(o)) => format!("{}_r{}_s{:04}", o.name.run, o.name.rev, o.name.seed),
            _ => img.id.to_string(),
        };
        let mut target = dest.join(format!("{stem}.{}", format.ext()));
        let mut k = 2;
        while target.exists() {
            target = dest.join(format!("{stem} {k}.{}", format.ext()));
            k += 1;
        }
        std::fs::write(&target, bytes).map_err(|e| format!("{}: {e}", target.display()))?;
        n += 1;
    }
    Ok(n)
}

/// Opens a folder (a file: its folder) in the file manager: `command` if
/// set, else the desktop's (Explorer, Finder, or on Linux the first of
/// xdg-open, gio, thunar, nautilus, dolphin, nemo, pcmanfm that works).
fn show_in_file_manager(path: &Path, command: &str) -> Result<(), String> {
    let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
    let managers: &[&[&str]] = if cfg!(any(windows, target_os = "macos")) {
        &[]
    } else {
        &[&["thunar"], &["nautilus"], &["dolphin"], &["nemo"], &["pcmanfm"]]
    };
    open_with(dir, command, managers, false).map_err(|e| match command.trim().is_empty() {
        true => "No file manager found: install one (e.g. thunar) or set `file_manager` in config.rsn".into(),
        false => e,
    })
}

/// Opens a text file in `command` if set, else Notepad on Windows,
/// TextEdit (`open -t`) on macOS (`.rsn` has no app of its own there), or
/// on Linux the desktop's app for it or a known editor.
fn open_in_editor(path: &Path, command: &str) -> Result<(), String> {
    let (editors, first): (&[&[&str]], bool) = if cfg!(windows) {
        (&[&["notepad"]], true)
    } else if cfg!(target_os = "macos") {
        (&[&["open", "-t"]], true)
    } else {
        (&[&["gnome-text-editor"], &["gedit"], &["kate"], &["mousepad"], &["xed"]], false)
    };
    open_with(path, command, editors, first).map_err(|e| match command.trim().is_empty() {
        true => "No text editor found: install one (e.g. mousepad) or set `editor` in config.rsn".into(),
        false => e,
    })
}

/// Opens `target` with `command` (the target appended) if set, else the
/// desktop's opener, else the first of `apps` that starts (`apps_first`:
/// the apps before the opener).
fn open_with(target: &Path, command: &str, apps: &[&[&str]], apps_first: bool) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let spawn = |argv: &[&str], wait: bool| -> bool {
        let Some((prog, args)) = argv.split_first() else { return false };
        let mut c = Command::new(prog);
        increart_core::process::background(&mut c, false);
        c.args(args).arg(target).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if wait { c.status().is_ok_and(|s| s.success()) } else { c.spawn().is_ok() }
    };
    if !command.trim().is_empty() {
        let argv: Vec<&str> = command.split_whitespace().collect();
        return if spawn(&argv, false) { Ok(()) } else { Err(format!("couldn't run `{command}`")) };
    }
    // Openers exit at once, their status saying whether a handler was found
    // (not Explorer's: it is only started); the apps keep running.
    let opener = || {
        if cfg!(windows) {
            spawn(&["explorer"], false)
        } else if cfg!(target_os = "macos") {
            spawn(&["open"], true)
        } else {
            spawn(&["xdg-open"], true) || spawn(&["gio", "open"], true)
        }
    };
    let app = || apps.iter().any(|a| spawn(a, false));
    let (first, then): (&dyn Fn() -> bool, &dyn Fn() -> bool) =
        if apps_first { (&app, &opener) } else { (&opener, &app) };
    if first() || then() {
        return Ok(());
    }
    Err("nothing to open it with".into())
}

/// A folder of the library for the folder view: folders, projects, runs,
/// image links (with the image they point at) and metadata files.
fn list_dir(root: &Path, rel: &Path) -> Vec<FvEntry> {
    use increart_core::link;
    let dir = root.join(rel);
    let Ok(rd) = std::fs::read_dir(&dir) else { return vec![] };
    // Inside a project?
    let in_project = rel.ancestors().any(|a| root.join(a).join("project.rsn").is_file());
    // ideas link into the input store, everything else into the output store
    let store = store_of(rel);
    let mut out = vec![];
    for e in rd.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let meta = std::fs::metadata(&path).ok();
        let modified = meta.as_ref().and_then(|m| m.modified().ok()).map(chrono::DateTime::<chrono::Utc>::from);
        let (kind, size, info) = if path.is_dir() {
            let n = std::fs::read_dir(&path).map(|r| r.count() as u64).unwrap_or(0);
            if path.join("project.rsn").is_file() {
                (EntryKind::Project, n, "project".to_string())
            } else if let Ok(id) = name.parse::<increart_core::Id>()
                && path.join("idea.rsn").is_file()
            {
                let title = increart_core::rsnfmt::read::<increart_core::model::Idea>(&path.join("idea.rsn"))
                    .map(|i| i.title)
                    .unwrap_or_default();
                (EntryKind::Idea(id), n, if title.is_empty() { "idea".into() } else { title })
            } else if in_project && rel.file_name().is_some_and(|f| f == "runs") {
                let label = increart_core::rsnfmt::read::<increart_core::model::Run>(&path.join("run.rsn"))
                    .map(|r| format!("run {}", r.label))
                    .unwrap_or_else(|_| "run".into());
                (EntryKind::Run, n, label)
            } else if name == "runs" && dir.join("project.rsn").is_file() {
                (EntryKind::Folder, n, "runs".to_string())
            } else if in_project {
                (EntryKind::Folder, n, "review folder".to_string())
            } else {
                (EntryKind::Folder, n, "folder".to_string())
            }
        } else if name.ends_with(".rsn") || name.ends_with(".jsonl") {
            (EntryKind::Meta, meta.map_or(0, |m| m.len()), "metadata".to_string())
        } else if StoreFormat::of_name(&name).is_some() {
            match link::read_ref(&path, store) {
                Ok(Some(r)) => {
                    let what = rel.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                    (EntryKind::Image(r), meta.map_or(0, |m| m.len()), what)
                }
                _ => (EntryKind::File, meta.map_or(0, |m| m.len()), "file".to_string()),
            }
        } else {
            (EntryKind::File, meta.map_or(0, |m| m.len()), "file".to_string())
        };
        out.push(FvEntry { name, kind, size, modified, info });
    }
    out
}

/// Up to three images for a folder's icon: a project's cover first, then the
/// newest image links found breadth-first, looking at no more than a few
/// hundred entries; `bad/` only when nothing else is there. A run's images
/// are its outputs in the project's review folders; `runs/` shows the whole
/// project's.
fn previews(dir: &Path, store: increart_core::Store) -> Vec<ImageRef> {
    let found = previews_from(dir, store, true);
    if found.is_empty() { previews_from(dir, store, false) } else { found }
}

fn previews_from(dir: &Path, store: increart_core::Store, skip_bad: bool) -> Vec<ImageRef> {
    let is_project = |d: &Path| d.join("project.rsn").is_file();
    let parent = dir.parent().unwrap_or(dir);
    if parent.file_name().is_some_and(|f| f == "runs")
        && let Some(project) = parent.parent().filter(|p| is_project(p))
    {
        let prefix = format!("{}_r", dir.file_name().unwrap_or_default().to_string_lossy());
        return walk_images(project, vec![], store, skip_bad, |name| name.starts_with(&prefix));
    }
    let top = if dir.file_name().is_some_and(|f| f == "runs") && is_project(parent) { parent } else { dir };
    let cover = increart_core::rsnfmt::read::<increart_core::model::Project>(&top.join("project.rsn"))
        .ok()
        .and_then(|p| p.cover)
        .into_iter()
        .collect();
    walk_images(top, cover, store, skip_bad, |_| true)
}

/// The store a folder's image links point into (`rel` from the library
/// root): ideas' into the input store, the rest into the output store.
fn store_of(rel: &Path) -> increart_core::Store {
    if rel.starts_with("input") { increart_core::Store::Input } else { increart_core::Store::Output }
}

fn walk_images(
    dir: &Path,
    mut out: Vec<ImageRef>,
    store: increart_core::Store,
    skip_bad: bool,
    keep: impl Fn(&str) -> bool,
) -> Vec<ImageRef> {
    use increart_core::link;
    const WANT: usize = 3;
    const BUDGET: usize = 400;
    let mut queue = std::collections::VecDeque::from([dir.to_path_buf()]);
    let mut seen = 0;
    while let Some(d) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut names: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        // ids start with the date, so newest first
        names.sort_by(|a, b| b.cmp(a));
        for p in names {
            seen += 1;
            if out.len() >= WANT || seen > BUDGET {
                return out;
            }
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.starts_with('.') || (skip_bad && name == "bad") {
                continue;
            }
            if p.is_dir() {
                queue.push_back(p);
            } else if StoreFormat::of_name(&name).is_some()
                && keep(&name)
                && let Ok(Some(r)) = link::read_ref(&p, store)
                && !out.contains(&r)
            {
                out.push(r);
            }
        }
    }
    out
}

fn err(e: impl ToString) -> String {
    e.to_string()
}

fn work_on(ctx: &Ctx, fx: Effect) -> Vec<Msg> {
    match fx {
        Effect::SaveConfig(config, path) => match config.save(&path) {
            Ok(()) => vec![],
            Err(e) => vec![Msg::Problem(format!("saving settings: {e}"))],
        },
        Effect::OpenLibrary { path, create } => {
            let opened = if create { Library::create(&path, None) } else { Library::open(&path) };
            vec![Msg::LibraryOpened(opened.map_err(err).and_then(|mut lib| {
                if let Some(d) = ctx.paths.disposal.get() {
                    lib.set_disposal(d.clone());
                }
                let ix = lib.scan().map_err(err)?;
                let root = lib.root().to_path_buf();
                ctx.session.lock_or_recover().library = Some(Arc::new(Mutex::new(lib)));
                Ok((root, ix))
            }))]
        }
        Effect::StoreInfo => {
            let Some(lib) = ctx.library() else { return vec![] };
            vec![Msg::StoreInfo(store_info(&lib.lock_or_recover()))]
        }
        Effect::SetStoreFormat(format) => {
            let Some(lib) = ctx.library() else { return vec![] };
            let mut l = lib.lock_or_recover();
            vec![Msg::StoreInfo(l.set_store_format(format).map_err(err).and_then(|()| store_info(&l)))]
        }
        Effect::SaveDraft(d) => {
            let Some(lib) = ctx.library() else { return vec![] };
            let l = lib.lock_or_recover();
            let r = match d {
                DraftSave::Generate(project, draft) => l.save_generate_draft(&project, &draft),
                DraftSave::Idea(draft) => l.save_idea_draft(&draft),
                DraftSave::ClearIdea => l.clear_idea_draft(),
            };
            vec![Msg::DraftSaved(r.map_err(err))]
        }
        Effect::LoadHistory(key) => {
            let Some(lib) = ctx.library() else { return vec![] };
            let path = lib.lock_or_recover().history_path(&key);
            let entries = path.map(|p| increart_core::history::read(&p)).unwrap_or_default();
            vec![Msg::HistoryLoaded(key, entries)]
        }
        Effect::RecordHistory { key, mut entry, replace_last } => {
            let Some(lib) = ctx.library() else { return vec![] };
            let path = lib.lock_or_recover().history_path(&key);
            entry.time = increart_core::id::now();
            match path.and_then(|p| increart_core::history::record(&p, &entry, replace_last)) {
                Ok(()) => vec![],
                Err(e) => vec![Msg::Problem(format!("prompt history: {e}"))],
            }
        }
        Effect::ReadJobLog(id, path) => {
            vec![Msg::JobLog(id, increart_core::engine::read_run_log(&path, crate::update::JOB_LOG_LINES))]
        }
        Effect::LoadRecipe(id) => {
            let Some(lib) = ctx.library() else { return vec![] };
            let r = lib.lock_or_recover().recipe(&id).ok().flatten();
            vec![Msg::RecipeLoaded(id, r)]
        }
        Effect::LoadInputMeta(ids) => {
            let Some(lib) = ctx.library() else { return vec![] };
            let l = lib.lock_or_recover();
            let list = ids
                .into_iter()
                .map(|id| {
                    let source = l.source_info(&id).ok().flatten();
                    let blake3 = l.blake3_of(&ImageRef::input(id.clone())).ok();
                    (id, crate::state::InputMeta { source, blake3 })
                })
                .collect();
            vec![Msg::InputMetaLoaded(list)]
        }
        Effect::LoadImports => {
            let Some(lib) = ctx.library() else { return vec![] };
            let l = lib.lock_or_recover();
            let mut ids = l.store_ids(increart_core::Store::Input).unwrap_or_default();
            ids.reverse();
            let list =
                ids.into_iter().take(200).filter_map(|id| l.source_info(&id).ok().flatten().map(|s| (id, s))).collect();
            vec![Msg::Imports(list)]
        }
        Effect::Export { images, dest, keep_metadata, by_id } => {
            let Some(lib) = ctx.library() else { return vec![] };
            let l = lib.lock_or_recover();
            vec![Msg::Exported(export(&l, &images, &dest, keep_metadata, by_id))]
        }
        Effect::ShowInFileManager { path, command } => match show_in_file_manager(&path, &command) {
            Ok(()) => vec![],
            Err(e) => vec![Msg::Problem(e)],
        },
        Effect::ListDir(rel) => {
            let Some(lib) = ctx.library() else { return vec![] };
            let root = lib.lock_or_recover().root().to_path_buf();
            vec![Msg::FvListed(rel.clone(), list_dir(&root, &rel))]
        }
        Effect::Rescan => match ctx.library() {
            Some(lib) => lib.lock_or_recover().scan().map(Msg::Scanned).into_iter().collect(),
            None => vec![],
        },
        Effect::StopEngine => {
            let engine = ctx.session.lock_or_recover().engine.take();
            if let Some(engine) = engine {
                ctx.engine_stopped();
                engine.shutdown();
            }
            vec![]
        }
        Effect::SavePresets(presets, path) => match presets.save_dir(&path) {
            Ok(()) => vec![],
            Err(e) => vec![Msg::Problem(format!("saving prompt presets: {e}"))],
        },
        Effect::StopServers => {
            let (engine, servers) = {
                let mut s = ctx.session.lock_or_recover();
                s.cancel.store(true, Ordering::Relaxed);
                s.opened_with = None;
                (s.engine.take(), std::mem::take(&mut s.servers))
            };
            if let Some(engine) = engine {
                ctx.engine_stopped();
                engine.shutdown();
            }
            // dropping a process the app started stops it (kill, then wait)
            let names = servers.iter().flatten().filter(|(_, p)| p.launched()).map(|(_, p)| p.name.clone()).collect();
            drop(servers);
            vec![Msg::ServersStopped(names)]
        }
        Effect::SetJobOrder(order) => {
            if let Some(e) = &ctx.session.lock_or_recover().engine {
                e.set_order(order);
            }
            vec![]
        }
        Effect::SetSkipRamCheck(skip) => {
            if let Some(e) = &ctx.session.lock_or_recover().engine {
                e.set_skip_ram_check(skip);
            }
            vec![]
        }
        Effect::StartEngine { shared: cfg, order, skip_ram_check } => {
            let Some(lib) = ctx.library() else { return vec![] };
            // Runs with seeds left, found before taking any other lock (the
            // library lock is held once, never nested). From one scan: the
            // library's per-run look-up walks every project, which on a cold
            // disk held this queue (and Saves behind it) for half a minute.
            let scanned = lib.lock_or_recover().scan();
            let pending: Vec<_> = match scanned {
                Ok(ix) => ix
                    .projects
                    .iter()
                    .flat_map(|p| p.runs.iter().filter(|r| p.pending_seeds(r).next().is_some()))
                    .map(|r| r.id.clone())
                    .collect(),
                Err(_) => vec![],
            };
            let endpoints: Vec<Endpoint> = ctx
                .session
                .lock_or_recover()
                .servers
                .iter()
                .flatten()
                .map(|(_, p)| Endpoint { name: p.name.clone(), device: p.device.clone(), client: p.client.clone() })
                .collect();
            let opts = EngineOptions {
                generation: GenOptions { preview: cfg.preview.clone(), ..Default::default() },
                sdcpp: cfg.commit.clone(),
                order,
                skip_ram_check,
                ..Default::default()
            };
            let w = ctx.wake.clone();
            let mut engine = match Engine::start(lib.clone(), endpoints, opts, Some(Box::new(move || w()))) {
                Ok(e) => e,
                Err(e) => {
                    return vec![Msg::Problem(format!("couldn't start generating: {e}")), Msg::RunsQueued(vec![])];
                }
            };
            for r in &pending {
                engine.queue_run(r);
            }
            let _ = ctx.engine_events.send(Some(engine.take_events()));
            let before = ctx.session.lock_or_recover().engine.replace(engine);
            if let Some(before) = before {
                before.shutdown();
            }
            vec![Msg::RunsQueued(pending)]
        }
        Effect::QueueRun(id) => {
            if let Some(e) = &ctx.session.lock_or_recover().engine {
                e.queue_run(&id);
            }
            vec![]
        }
        Effect::CancelRun(id, how) => {
            if let Some(e) = &ctx.session.lock_or_recover().engine {
                e.cancel_run(&id, how);
            }
            vec![]
        }
        Effect::Lib(op) => {
            let Some(lib) = ctx.library() else { return vec![Msg::OpDone(Err("no library open".into()), None)] };
            let queue_run = matches!(&op, LibOp::CreateRun { queue: true, .. });
            let op_queue = match &op {
                LibOp::CreateSweep { queue, .. } => Some(*queue),
                _ => None,
            };
            hash_ahead(&lib, &op);
            let result = {
                let mut l = lib.lock_or_recover();
                lib_op(&mut l, op)
            };
            let queue_run = queue_run || matches!(&op_queue, Some(true));
            if let Some(e) = &ctx.session.lock_or_recover().engine
                && queue_run
            {
                match &result {
                    Ok(OpOk::Run(id, _)) => e.queue_run(id),
                    // all at once: the scheduler's round robin goes breadth-first
                    Ok(OpOk::Sweep(ids, ..)) => ids.iter().for_each(|id| e.queue_run(id)),
                    _ => {}
                }
            }
            let ix = lib.lock_or_recover().scan().ok();
            vec![Msg::OpDone(result, ix)]
        }
        Effect::ListDevices(_)
        | Effect::DetectGpus(_)
        | Effect::KoboldVersion(_)
        | Effect::DownloadModels { .. }
        | Effect::CancelDownload
        | Effect::PlaySound { .. }
        | Effect::OpenServers { .. }
        | Effect::Pick { .. }
        | Effect::PeAsk { .. }
        | Effect::PeStop(_)
        | Effect::PeShutdown
        | Effect::PeListGpus(_)
        | Effect::FolderPreviews { .. }
        | Effect::ComposeTiles { .. }
        | Effect::WatchLibrary(_)
        | Effect::OpenInEditor { .. }
        | Effect::ConvertStore(_)
        | Effect::CancelConvert
        | Effect::ExportLibrary { .. }
        | Effect::SaveBugReport { .. }
        | Effect::UnpackArchive { .. }
        | Effect::CancelArchive
        | Effect::CheckUsage => {
            // (handled on their own threads, never sent here)
            vec![Msg::Problem(format!("internal: {fx:?} reached the library worker"))]
        }
    }
}

/// The wizard's model files, one after the other, reporting progress a
/// few times a second.
fn download_models(
    dir: &str,
    files: &[increart_core::tiers::Download],
    search: &[PathBuf],
    cancel: &AtomicBool,
    tx: &Sender<Msg>,
    wake: &Wake,
) -> Result<(), String> {
    use increart_core::tiers;
    let mut at = crate::state::WizardDownload { checking: true, ..Default::default() };
    let send = |at: &crate::state::WizardDownload| {
        let _ = tx.send(Msg::WizardDownload(at.clone()));
        wake();
    };
    send(&at);
    // first what's there: each file there already, linked to one on disk
    // (same name and size), or to fetch (its size)
    let mut fetch: Vec<(usize, Option<u64>)> = vec![];
    for (i, f) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        at.file = i;
        send(&at);
        let dest = std::path::Path::new(dir).join(&f.file);
        if dest.is_file() {
            at.reused.push((f.file.clone(), String::new()));
            continue;
        }
        let size = tiers::remote_size(&f.url);
        if std::fs::symlink_metadata(&dest).is_err()
            && let Some(src) = tiers::reusable(&f.url, search, || size)
            && tiers::link(&src, &dest).is_ok()
        {
            at.reused.push((f.file.clone(), src.to_string_lossy().into_owned()));
            continue;
        }
        fetch.push((i, size));
    }
    at.checking = false;
    at.all_total = fetch.iter().map(|(_, s)| *s).sum();
    let started = Instant::now();
    // fetched this run (not a .part carried on), for the rate
    let mut fetched = 0u64;
    let mut before = 0u64;
    for (i, size) in fetch {
        let f = &files[i];
        (at.file, at.done, at.total) = (i, 0, size);
        at.all_done = before;
        send(&at);
        let mut last = Instant::now();
        let mut first: Option<u64> = None;
        let mut seen = 0u64;
        let dest = std::path::Path::new(dir).join(&f.file);
        tiers::download(&f.url, &dest, cancel, |done, total| {
            // (the first report starts where a .part left off)
            let from = *first.get_or_insert(done);
            fetched += done.saturating_sub(seen.max(from));
            seen = done;
            if last.elapsed().as_millis() >= 250 {
                last = Instant::now();
                (at.done, at.total) = (done, total.or(size));
                at.all_done = before + done;
                let secs = started.elapsed().as_secs_f64();
                at.rate = (fetched > 0 && secs > 1.0).then(|| fetched as f64 / secs);
                send(&at);
            }
        })?;
        before += size.or(at.total).unwrap_or(seen);
    }
    Ok(())
}

/// The usage check after opening, one image at a time so library
/// operations go on meanwhile; stops if another library is opened.
fn check_usage(lib: &Arc<Mutex<Library>>, ctx: &Ctx) {
    let (tx, wake) = (&ctx.tx, &ctx.wake);
    let check = lib.lock_or_recover().usage_check();
    let Ok(check) = check else { return };
    let total = check.images.len();
    // shown from the start, however quick
    let _ = tx.send(Msg::UsageCheck(0, total));
    wake();
    let mut sent = std::time::Instant::now();
    for (i, r) in check.images.iter().enumerate() {
        if !ctx.library().is_some_and(|l| Arc::ptr_eq(&l, lib)) {
            let _ = tx.send(Msg::UsageCheck(total, total));
            wake();
            return;
        }
        // read without the lock (a cold disk makes this the slow part, and
        // holding the lock through it kept Saves waiting); lock to fix
        if !check.covers(r) {
            lib.lock_or_recover().check_usage(&check, r);
        }
        if sent.elapsed() > std::time::Duration::from_millis(100) {
            sent = std::time::Instant::now();
            let _ = tx.send(Msg::UsageCheck(i + 1, total));
            wake();
        }
    }
    let _ = tx.send(Msg::UsageCheck(total, total));
    wake();
}

fn store_info(lib: &Library) -> Result<crate::state::StoreInfo, String> {
    Ok(crate::state::StoreInfo { format: lib.store_format(), counts: lib.store_counts().map_err(err)? })
}

/// Converts every store image to `to` (DESIGN.md, Image stores): a batch at
/// a time is re-encoded on a few threads without the library lock (the
/// slow part), then installed under it, so library operations and
/// generating go on meanwhile. Stops when cancelled or another library is
/// opened; what's done stays done.
/// An archive thread's progress callback: `Archiving` a few times a second,
/// stopping once Cancel is pressed.
fn archive_progress<'a>(
    tx: &'a Sender<Msg>,
    wake: &'a Wake,
    cancel: &'a AtomicBool,
) -> impl FnMut(u64, u64) -> bool + 'a {
    let mut last = Instant::now() - Duration::from_secs(1);
    move |done, total| {
        if last.elapsed() >= Duration::from_millis(200) {
            last = Instant::now();
            let _ = tx.send(Msg::Archiving(done, total));
            wake();
        }
        !cancel.load(Ordering::Relaxed)
    }
}

fn archive_error(e: increart_core::Error) -> String {
    if increart_core::archive::is_cancelled(&e) { crate::update::ARCHIVE_STOPPED.into() } else { e.to_string() }
}

type Encoded<'a> = Vec<(&'a ImageRef, increart_core::Result<Option<increart_core::library::Converted>>)>;

fn encode_part<'a>(conv: &increart_core::library::Conversion, part: &'a [ImageRef]) -> Encoded<'a> {
    part.iter().map(|r| (r, conv.encode(r))).collect()
}

fn convert_store(lib: &Arc<Mutex<Library>>, ctx: &Ctx, to: StoreFormat, cancel: &AtomicBool) {
    let send = |m: Msg| {
        let _ = ctx.tx.send(m);
        (ctx.wake)();
    };
    let conv = match lib.lock_or_recover().conversion(to) {
        Ok(c) => c,
        Err(e) => {
            return send(Msg::Converted { converted: 0, skipped: 0, failed: vec![e.to_string()], cancelled: false });
        }
    };
    let total = conv.images.len();
    send(Msg::Converting(0, total));
    // leave a core for the app and the servers
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1)).clamp(1, 8);
    let (mut done, mut converted, mut skipped, mut failed, mut cancelled) = (0, 0, 0, vec![], false);
    let name = |r: &ImageRef| format!("{:?} image {}", r.store, r.id);
    for batch in conv.images.chunks(threads * 2) {
        if cancel.load(Ordering::Relaxed) || !ctx.library().is_some_and(|l| Arc::ptr_eq(&l, lib)) {
            cancelled = true;
            break;
        }
        let conv = &conv;
        let encoded: Vec<_> = std::thread::scope(|s| {
            let parts: Vec<_> = batch
                .chunks(batch.len().div_ceil(threads))
                .map(|part| (part, std::thread::Builder::new().spawn_scoped(s, move || encode_part(conv, part))))
                .collect();
            parts
                .into_iter()
                .flat_map(|(part, h)| match h {
                    Ok(h) => h.join().unwrap_or_else(|_| {
                        let stopped = || increart_core::Error::Refused("its encoding thread stopped".into());
                        part.iter().map(|r| (r, Err(stopped()))).collect()
                    }),
                    // no thread for it: encoded here
                    Err(_) => encode_part(conv, part),
                })
                .collect()
        });
        let mut ready = vec![];
        for (r, res) in encoded {
            match res {
                Ok(Some(c)) => ready.push(c),
                Ok(None) => skipped += 1,
                Err(e) => failed.push(format!("{}: {e}", name(r))),
            }
        }
        let n = ready.len();
        match lib.lock_or_recover().install(ready) {
            Ok(f) => {
                converted += n - f.len();
                failed.extend(f.into_iter().map(|(r, e)| format!("{}: {e}", name(&r))));
            }
            Err(e) => {
                failed.push(e.to_string());
                break;
            }
        }
        done += batch.len();
        send(Msg::Converting(done, total));
    }
    send(Msg::Converted { converted, skipped, failed, cancelled });
}

/// The verdict a link has now, from its folder.
fn verdict_of(link: &Path) -> Option<Verdict> {
    match link.parent()?.file_name()?.to_str()? {
        "good" => Some(Verdict::Good),
        "neutral" => Some(Verdict::Neutral),
        "bad" => Some(Verdict::Bad),
        _ => None,
    }
}

pub fn lib_op(lib: &mut Library, op: LibOp) -> Result<OpOk, String> {
    match op {
        LibOp::NewIdea(n) => {
            // Add & save to input library may name a new folder
            if let IdeaLoc::Library(folder) = &n.loc {
                lib.make_library_folder(folder).map_err(err)?;
            }
            lib.new_idea(n).map(OpOk::Idea).map_err(err)
        }
        LibOp::NewIdeaPromote(n, folder, name) => {
            let id = lib.new_idea(n).map_err(err)?;
            promote(lib, &[id], &folder, &name)
        }
        LibOp::ImportFiles(paths) => {
            for p in paths {
                let is_text = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("txt"));
                let mut idea = NewIdea {
                    source: IdeaSource::DroppedFile { original_path: p.to_string_lossy().into() },
                    ..NewIdea::text("", "")
                };
                if is_text {
                    idea.text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?.trim().into();
                } else {
                    idea.images.push(lib.import_file(&p).map_err(err)?.image);
                }
                lib.new_idea(idea).map_err(err)?;
            }
            Ok(OpOk::Done)
        }
        LibOp::ImportImages(paths) => {
            let ids = paths.iter().map(|p| lib.import_file(p).map(|i| i.image)).collect::<Result<Vec<_>, _>>();
            ids.map(OpOk::Imported).map_err(err)
        }
        LibOp::ImportRefs(paths) => {
            let ids = paths.iter().map(|p| lib.import_file(p).map(|i| i.image)).collect::<Result<Vec<_>, _>>();
            ids.map(OpOk::ImportedRefs).map_err(err)
        }
        LibOp::ReviewIdea(id, v) => lib.review_idea(&id, v).map(|_| OpOk::Done).map_err(err),
        LibOp::SaveToLibrary(id, title, folder) => {
            lib.save_to_library(&id, title.as_deref(), &folder).map(|_| OpOk::Done).map_err(err)
        }
        LibOp::MoveIdea(id, loc) => lib.move_idea(&id, &loc).map(|_| OpOk::Done).map_err(err),
        LibOp::SetStar(id, on) => lib.update_idea(&id, |i| i.starred = on).map(|_| OpOk::Done).map_err(err),
        LibOp::DeleteDiscarded(id) => lib.delete_discarded_idea(&id).map(|_| OpOk::Done).map_err(err),
        LibOp::Promote(ids, folder, name) => promote(lib, &ids, &folder, &name),
        LibOp::CreateFolder(tree, parent) => {
            lib.create_folder(tree, &parent, None).map(|p| OpOk::Created(tree, p)).map_err(err)
        }
        LibOp::CreateProject(parent, name) => {
            lib.create_project(&parent, Some(&name), &[]).map(|(_, p)| OpOk::Created(Tree::Projects, p)).map_err(err)
        }
        LibOp::Rename(tree, path, name) => lib.rename_node(tree, &path, &name).map(|_| OpOk::Done).map_err(err),
        LibOp::MoveNode(tree, path, parent) => lib.move_node(tree, &path, &parent).map(|_| OpOk::Done).map_err(err),
        LibOp::DeleteNode(tree, path) => lib.delete_node(tree, &path).map(|_| OpOk::Done).map_err(err),
        LibOp::CreateSweep { project, runs, model_dirs, queued, .. } => {
            let name = runs.first().map(|(_, t, _)| t.name.clone()).unwrap_or_default();
            let mut ids = vec![];
            for (form, tag, preset) in runs {
                let mut run = new_run(lib, &form, &preset, &model_dirs);
                run.sweep = Some(tag);
                ids.push(lib.create_run(&project, run).map_err(err)?);
            }
            Ok(OpOk::Sweep(ids, name, queued))
        }
        LibOp::CreateRun { project, form, preset, model_dirs, queued, .. } => {
            let run = new_run(lib, &form, &preset, &model_dirs);
            lib.create_run(&project, run).map(|id| OpOk::Run(id, queued)).map_err(err)
        }
        LibOp::PrepareProject { parent, name, copy_of, form, then } => {
            let path = match copy_of {
                // the copy, renamed as asked (a taken name gets a number)
                Some(src) => {
                    let copy = lib.copy_node(Tree::Projects, &src, &parent).map_err(err)?;
                    if copy.file_name().is_some_and(|n| n.to_string_lossy() != name.as_str()) {
                        let free = lib.free_name(Tree::Projects, &parent, &name).map_err(err)?;
                        lib.rename_node(Tree::Projects, &copy, &free).map_err(err)?
                    } else {
                        copy
                    }
                }
                None => {
                    let base = increart_core::library::validate_name(&name)
                        .map(|_| name.clone())
                        .unwrap_or_else(|_| "new project".into());
                    let free = lib.free_name(Tree::Projects, &parent, &base).map_err(err)?;
                    lib.create_project(&parent, Some(&free), &[]).map_err(err)?.1
                }
            };
            Ok(OpOk::ProjectReady { path, form: Box::new(form), then })
        }
        LibOp::CopyProject(path, name) => lib.duplicate_project(&path, &name).map(OpOk::Duplicated).map_err(err),
        LibOp::EditedCopy { image, edits, title, to } => {
            let path = lib.image_path(&image);
            let img = image::open(&path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgba8();
            let out = increart_core::markup::render(&img, &edits);
            let mut png = std::io::Cursor::new(vec![]);
            out.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
            lib.edited_copy(png.get_ref(), image, edits, &title).map(|i| OpOk::Edited(i.image, to)).map_err(err)
        }
        LibOp::IdeaFromTiles { image, sources } => {
            let mut png = std::io::Cursor::new(vec![]);
            image.0.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
            let title = format!("Tiled {} images", sources.len());
            lib.idea_from_tiles(png.get_ref(), sources, &title, IdeaLoc::top_library()).map(OpOk::Tiled).map_err(err)
        }
        LibOp::IdeaFromOutput { image, with_prompt, loc } => {
            lib.idea_from_output(&image, with_prompt, loc).map(OpOk::Idea).map_err(err)
        }
        LibOp::AddSeeds(run, n) => lib.add_seeds(&run, n, false).map(|_| OpOk::SeedsAdded(run, n)).map_err(err),
        LibOp::AddVariant(id, v) => lib.update_idea(&id, |i| i.variants.push(v)).map(|_| OpOk::Done).map_err(err),
        LibOp::SaveIdea { id, title, text, tags, images } => lib
            .update_idea(&id, |i| {
                (i.title, i.text, i.tags, i.images) = (title, text, tags, images);
            })
            .map(|_| OpOk::Done)
            .map_err(err),
        LibOp::ImportInboxDrop => lib.import_inbox_drop().map(|_| OpOk::Done).map_err(err),
        LibOp::AddToCollection(folder, images) => {
            for img in images.iter().filter(|i| i.store == increart_core::Store::Output) {
                lib.add_to_collection(&folder, &img.id).map_err(err)?;
            }
            Ok(OpOk::Done)
        }
        LibOp::NewCollection(parent, images) => {
            let path = lib.create_folder(Tree::Collections, &parent, None).map_err(err)?;
            for img in images.iter().filter(|i| i.store == increart_core::Store::Output) {
                lib.add_to_collection(&path, &img.id).map_err(err)?;
            }
            Ok(OpOk::Created(Tree::Collections, path))
        }
        LibOp::CopyNode(tree, path, parent) => lib.copy_node(tree, &path, &parent).map(|_| OpOk::Done).map_err(err),
        LibOp::SetCover(project, img) => lib.set_cover(&project, img).map(|_| OpOk::Done).map_err(err),
        LibOp::SetTitle(project, image, title) => {
            lib.set_title(&project, &image, &title).map(|_| OpOk::Done).map_err(err)
        }
        LibOp::DeleteOutputs(links) => {
            for l in links {
                lib.delete_output_link(&l).map_err(err)?;
            }
            Ok(OpOk::Done)
        }
        LibOp::ReviewOutputs { links, undo } => {
            let mut done = vec![];
            for (link, verdict) in links {
                let previous = verdict_of(&link);
                match lib.review_output(&link, verdict) {
                    Ok(new) => done.push((new, previous)),
                    // nothing rated: an error as before; else keep what was
                    Err(e) if done.is_empty() => return Err(err(e)),
                    Err(e) => return Ok(OpOk::Reviewed { done, undo, error: Some(err(e)) }),
                }
            }
            Ok(OpOk::Reviewed { done, undo, error: None })
        }
    }
}

/// Keeps the ideas (inbox ones move to the top of the input library) and
/// makes one project from them, named after the first idea's title.
fn promote(lib: &mut Library, ids: &[Id], folder: &Path, name: &str) -> Result<OpOk, String> {
    // Create in folder: a new folder is made first
    if !lib.root().join("projects").join(folder).is_dir()
        && let (Some(parent), Some(last)) = (folder.parent(), folder.file_name())
    {
        lib.create_folder(Tree::Projects, parent, Some(&last.to_string_lossy())).map_err(err)?;
    }
    let mut ideas = vec![];
    for id in ids {
        if let (IdeaLoc::Unreviewed | IdeaLoc::Discarded, _) = lib.find_idea(id).map_err(err)? {
            lib.move_idea(id, &IdeaLoc::top_library()).map_err(err)?;
        }
        ideas.push(lib.load_idea(id).map_err(err)?);
    }
    let (project, _) = lib.create_project(folder, Some(name), ids).map_err(err)?;
    let prompt = ideas.iter().map(|i| i.text.trim()).find(|t| !t.is_empty()).unwrap_or("").to_string();
    let refs = ideas.iter().flat_map(|i| i.images.iter().cloned().map(ImageRef::input)).collect();
    Ok(OpOk::Promoted { project, prompt, refs, idea: ids.first().cloned() })
}

/// Hashes a new run's model files into the library's cache before the
/// library is locked for it: reading a model of many GB takes minutes,
/// and the engine (starting the runs queued before) needs the library.
fn hash_ahead(lib: &Mutex<Library>, op: &LibOp) {
    let (presets, model_dirs): (Vec<&RecipePreset>, _) = match op {
        LibOp::CreateRun { preset, model_dirs, .. } => (vec![preset], model_dirs),
        LibOp::CreateSweep { runs, model_dirs, .. } => (runs.iter().map(|(_, _, p)| p).collect(), model_dirs),
        _ => return,
    };
    let cache = lib.lock_or_recover().root().join("cache/model-hashes.rsn");
    let ids = presets.iter().flat_map(|p| p.files.values().chain(p.loras.iter().map(|l| &l.file)));
    let files: std::collections::BTreeSet<PathBuf> = ids
        .filter(|id| !id.starts_with('@'))
        .filter_map(|id| increart_core::config::model_file_on_disk(id, model_dirs))
        .collect();
    for f in files {
        let _ = modelhash::sha256_cached(&cache, &f);
    }
}

/// A run from the Generate form. Model files are hashed (cached in the
/// library's `cache/`) when they can be found in a model directory.
fn new_run(lib: &Library, f: &GenerateForm, preset: &RecipePreset, model_dirs: &[String]) -> NewRun {
    let cache = lib.root().join("cache/model-hashes.rsn");
    let sha256 = |id: &str| match increart_core::config::model_file_on_disk(id, model_dirs) {
        Some(file) if !id.starts_with('@') => modelhash::sha256_cached(&cache, &file).unwrap_or_default(),
        _ => String::new(),
    };
    let files = preset
        .files
        .iter()
        .map(|(slot, id)| (slot.clone(), ModelFile { id: id.clone(), sha256: sha256(id) }))
        .collect();
    let loras = preset
        .loras
        .iter()
        .filter(|l| !l.file.is_empty())
        .map(|l| LoraFile { id: l.file.clone(), sha256: sha256(&l.file), weight: l.weight })
        .collect();
    NewRun {
        sweep: None,
        label: f.label.clone(),
        prompt: f.prompt.clone(),
        negative_prompt: f.negative.clone(),
        // the prompt as typed (the old Transparent wrapper broke edits)
        alpha: false,
        width: f.width,
        height: f.height,
        sample: Sample {
            method: "euler".into(),
            scheduler: String::new(),
            steps: f.steps,
            cfg: f.cfg,
            flow_shift: f.flow_shift,
            custom_sigmas: preset.custom_sigmas.clone(),
        },
        mode: match &f.init() {
            Some(init) => Mode::Edit { init: Some(init.clone()), strength: f.strength },
            None => Mode::Generate,
        },
        refs: f.refs(),
        model: ModelSelection { recipe: preset.recipe.clone(), files, loras },
        source_idea: f.source_idea.clone(),
        first_seed: f.first_seed,
        seeds: f.seeds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serves `files` (name, size) with HEAD and GET until dropped.
    fn serve(files: Vec<(&'static str, usize)>) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            for rq in server.incoming_requests() {
                let name = rq.url().trim_start_matches('/');
                let Some((_, size)) = files.iter().find(|(n, _)| *n == name) else {
                    let _ = rq.respond(tiny_http::Response::empty(404));
                    continue;
                };
                let _ = rq.respond(tiny_http::Response::from_data(vec![7u8; *size]));
            }
        });
        base
    }

    #[test]
    fn models_checked_then_fetched_with_overall_progress() {
        let base = serve(vec![("a.gguf", 1000), ("b.gguf", 2000), ("c.gguf", 3000), ("d.gguf", 500)]);
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        // a: there already; b: on disk elsewhere, same size (linked)
        std::fs::create_dir_all(dir.path().join("koboldcpp")).unwrap();
        std::fs::write(dir.path().join("koboldcpp/a.gguf"), [1u8; 10]).unwrap();
        std::fs::write(other.path().join("b.gguf"), [7u8; 2000]).unwrap();
        let files: Vec<_> = ["a", "b", "c", "d"]
            .map(|n| increart_core::tiers::Download {
                url: format!("{base}/{n}.gguf"),
                file: format!("koboldcpp/{n}.gguf"),
            })
            .to_vec();
        let (tx, rx) = mpsc::channel();
        let wake: Wake = Arc::new(|| {});
        let cancel = AtomicBool::new(false);
        let d = dir.path().to_string_lossy().into_owned();
        download_models(&d, &files, &[other.path().to_path_buf()], &cancel, &tx, &wake).unwrap();
        let said: Vec<crate::state::WizardDownload> =
            rx.try_iter().filter_map(|m| if let Msg::WizardDownload(w) = m { Some(w) } else { None }).collect();
        assert!(said[0].checking);
        let last = said.last().unwrap();
        assert!(!last.checking);
        assert_eq!(last.all_total, Some(3500), "c and d only");
        assert_eq!(last.reused.len(), 2);
        assert_eq!(last.reused[0], ("koboldcpp/a.gguf".to_string(), String::new()));
        assert!(last.reused[1].1.ends_with("b.gguf"));
        assert_eq!(std::fs::read(dir.path().join("koboldcpp/b.gguf")).unwrap().len(), 2000, "linked");
        assert_eq!(std::fs::metadata(dir.path().join("koboldcpp/d.gguf")).unwrap().len(), 500);
        // the last file starts after the bytes before it
        assert!(said.iter().any(|w| w.file == 3 && w.all_done == 3000), "{said:?}");
    }
}
