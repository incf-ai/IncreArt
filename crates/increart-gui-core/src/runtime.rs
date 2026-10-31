//! Carries out [`Effect`]s off the GUI thread and reports back as [`Msg`]s:
//! library operations in order on one worker thread, servers opened in
//! parallel, engine events forwarded on [`Runtime::poll`].

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use increart_core::config::{RecipePreset, SdServerConfig, ServerConfig};
use increart_core::engine::{Endpoint, Engine, EngineOptions};
use increart_core::launcher::{self, ServerProcess};
use increart_core::model::{IdeaSource, LoraFile, Mode, ModelFile, ModelSelection, Sample, StoreFormat};
use increart_core::request::GenOptions;
use increart_core::{Id, IdeaLoc, ImageRef, Library, NewIdea, NewRun, Tree, Verdict, modelhash};

use crate::state::{EntryKind, FvEntry, GenerateForm, PickKind};
use crate::update::{DraftSave, Effect, LibOp, Msg, OpOk, TileTarget};

#[derive(Default)]
struct Shared {
    library: Option<Arc<Mutex<Library>>>,
    /// Where libraries opened from now on put deleted things (tests); None:
    /// the system trash.
    disposal: Option<increart_core::library::Disposal>,
    /// Where started servers log from now on (tests); None: [`log_dir`].
    log_dir: Option<PathBuf>,
    /// How often engines started from now on poll their jobs, and their
    /// first retry's wait (tests); None: the engine's defaults.
    engine_timing: Option<(Duration, Duration)>,
    /// The sd-servers open, each with what it was opened as.
    servers: Vec<Option<(ServerConfig, ServerProcess)>>,
    /// The shared settings `servers` were opened with.
    opened_with: Option<SdServerConfig>,
    engine: Option<Engine>,
    /// The opening attempt `servers` belongs to.
    attempt: u32,
    /// Set to stop the current opening attempt (and servers it started).
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Servers still being opened.
    opening: Arc<std::sync::atomic::AtomicUsize>,
    /// The open library's watch for outside changes.
    watch: Option<crate::watch::LibraryWatch>,
    /// The PE servers in use, each answering asks from `pe_queue`.
    pe: Vec<PeSlot>,
    /// Asks waiting for a PE server.
    pe_queue: std::collections::VecDeque<crate::update::PeJob>,
    /// The status and ready count last reported for `pe`.
    pe_reported: Option<(crate::state::PeStatus, usize)>,
    /// The number of the next PE slot.
    pe_next: u64,
    /// Set to stop the store conversion going on.
    convert_cancel: Arc<std::sync::atomic::AtomicBool>,
    archive_cancel: Arc<std::sync::atomic::AtomicBool>,
    download_cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Set to stop the server test going on.
    server_test_cancel: Arc<std::sync::atomic::AtomicBool>,
    /// The suite's run going on (its number): its stdin, for `stop`.
    api_test_stdin: Option<(u32, std::process::ChildStdin)>,
    /// PE asks stopped: dropped if queued (or when they come), cut short
    /// if being answered.
    pe_stopped: std::collections::HashSet<u64>,
}

/// A PE server in use and the worker answering asks with it.
struct PeSlot {
    /// Its worker's number (gone from `Shared::pe`: the worker ends).
    id: u64,
    server: increart_core::pe::PeServer,
    /// Once open.
    process: Option<increart_core::pe::PeProcess>,
    /// Why it didn't open.
    failed: Option<String>,
    /// Set to stop it while starting.
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// The ask it's answering.
    current: Option<u64>,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct Runtime {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    work: Sender<Effect>,
    /// Effects sent to `work` and not yet done (their messages sent).
    queued: Arc<std::sync::atomic::AtomicUsize>,
    /// PE servers to use and asks for them, in order; None: stop them.
    pe_work: Sender<Option<(Vec<increart_core::pe::PeServer>, Vec<crate::update::PeJob>)>>,
    shared: Arc<Mutex<Shared>>,
    /// Wakes PE workers: asks queued, or their slot gone.
    pe_signal: Arc<std::sync::Condvar>,
    wake: Wake,
}

/// `logs` in the app's state folder (`~/.local/state/increart` on Linux;
/// see [`user_dir`](increart_core::config::user_dir)).
pub fn log_dir() -> PathBuf {
    increart_core::config::user_dir("XDG_STATE_HOME", ".local/state", "LOCALAPPDATA")
        .unwrap_or_else(|| std::env::temp_dir().join("increart"))
        .join("logs")
}

/// Where servers started now log: [`Runtime::set_log_dir`]'s, or [`log_dir`].
/// The server validation suite (beside the app), without a console.
fn apitest_command() -> std::process::Command {
    let mut cmd = std::process::Command::new(increart_core::apitest::binary());
    increart_core::process::background(&mut cmd, false);
    cmd
}

/// The suite couldn't be started: most likely, it isn't there.
fn apitest_missing(e: &std::io::Error) -> String {
    let bin = increart_core::apitest::binary();
    let mut why = increart_core::process::spawn_error(&bin.display().to_string(), e);
    if e.kind() == std::io::ErrorKind::NotFound {
        why.push_str(&format!(
            ". {} comes with the app, in its release zip; built from source: cargo build -p {}",
            increart_core::apitest::BINARY,
            increart_core::apitest::BINARY
        ));
    }
    why
}

fn apitest_silent(stderr: &str) -> String {
    format!("{} said nothing: {}", increart_core::apitest::BINARY, last_lines(stderr, 12))
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "(no output)".into();
    }
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn logs(shared: &Mutex<Shared>) -> PathBuf {
    shared.lock().unwrap().log_dir.clone().unwrap_or_else(log_dir)
}

impl Runtime {
    /// Libraries opened after this put what they delete in `dir` rather than
    /// the system trash (tests: macOS's trash goes through Finder).
    pub fn set_trash_dir(&self, dir: PathBuf) {
        self.shared.lock().unwrap().disposal = Some(increart_core::library::Disposal::Directory(dir));
    }

    /// Servers started after this log into `dir` rather than [`log_dir`].
    pub fn set_log_dir(&self, dir: PathBuf) {
        self.shared.lock().unwrap().log_dir = Some(dir);
    }

    /// Engines started after this poll every `poll` and first retry after
    /// `backoff` (tests: the defaults keep a stand-in server's runs waiting).
    pub fn set_engine_timing(&self, poll: Duration, backoff: Duration) {
        self.shared.lock().unwrap().engine_timing = Some((poll, backoff));
    }

    /// `wake` is called whenever a message is ready (e.g. request a repaint).
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Runtime {
        let (tx, rx) = mpsc::channel();
        let (work, jobs) = mpsc::channel::<Effect>();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let wake: Wake = Arc::new(wake);
        let queued = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (wtx, wshared, wwake, wqueued) = (tx.clone(), shared.clone(), wake.clone(), queued.clone());
        std::thread::Builder::new()
            .name("library".into())
            .spawn(move || {
                for fx in jobs {
                    // Opening goes through this queue so it can't overtake a
                    // StopServers sent just before it.
                    if let Effect::OpenServers { servers, shared, attempt } = fx {
                        open_servers(&wtx, &wwake, &wshared, servers, shared, attempt);
                    } else {
                        let started = matches!(fx, Effect::StartEngine { .. });
                        let msgs = work_on(&wshared, &wwake, fx);
                        for m in msgs {
                            let _ = wtx.send(m);
                        }
                        if started {
                            list_loadable(&wtx, &wwake, &wshared);
                        }
                        wwake();
                    }
                    wqueued.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                }
            })
            .expect("spawn library worker");
        let (pe_work, pe_jobs) = mpsc::channel();
        let pe_signal = Arc::new(std::sync::Condvar::new());
        let pe = PeCtx { tx: tx.clone(), wake: wake.clone(), shared: shared.clone(), signal: pe_signal.clone() };
        std::thread::Builder::new()
            .name("prompt enhance".into())
            .spawn(move || {
                for work in pe_jobs {
                    pe_configure(&pe, work);
                }
            })
            .expect("spawn PE worker");
        Runtime { tx, rx, work, queued, pe_work, shared, pe_signal, wake }
    }

    pub fn run(&self, fx: Effect) {
        match fx {
            Effect::PlaySound { sound, volume, player, test } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::SoundPlayed(increart_core::notify::play(&sound, volume, &player), test));
                    wake();
                });
            }
            Effect::ListDevices(binary) => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let r = launcher::list_devices(&binary).map_err(|e| format!("{binary}: {e}"));
                    let _ = tx.send(Msg::Devices(r));
                    wake();
                });
            }

            Effect::KoboldVersion(binary) => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let r = increart_core::kobold::version(&binary);
                    let _ = tx.send(Msg::KoboldVersion(binary, r));
                    wake();
                });
            }
            Effect::DetectGpus(runtime) => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let r = increart_core::tiers::detect_gpus(runtime);
                    let _ = tx.send(Msg::WizardGpus(r, increart_core::tiers::available_ram_gib()));
                    wake();
                });
            }
            Effect::DownloadModels { dir, files, model_dirs } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.shared.lock().unwrap().download_cancel = cancel.clone();
                std::thread::Builder::new()
                    .name("model download".into())
                    .spawn(move || {
                        let search = increart_core::tiers::search_dirs(&model_dirs);
                        let r = download_models(&dir, &files, &search, &cancel, &tx, &wake);
                        let _ = tx.send(Msg::WizardDownloaded(r));
                        wake();
                    })
                    .expect("spawn model download");
            }
            Effect::CancelDownload => {
                self.shared.lock().unwrap().download_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Effect::TestServer { run, server, models, generate } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.shared.lock().unwrap().server_test_cancel = cancel.clone();
                std::thread::Builder::new()
                    .name("server test".into())
                    .spawn(move || {
                        use increart_core::servertest;
                        if let Some(client) = servertest::client_for(&server) {
                            // a test's traffic is always logged, tagged with it
                            let tag = format!("test {}", server.name());
                            increart_core::traffic::tagged(&tag, || {
                                servertest::run(&client, &models, generate, &cancel, &mut |i, step| {
                                    let _ = tx.send(Msg::ServerTestStep(run, i, step));
                                    wake();
                                })
                            });
                        }
                        let _ = tx.send(Msg::ServerTestDone(run));
                        wake();
                    })
                    .expect("spawn server test");
            }
            Effect::StopServerTest => {
                self.shared.lock().unwrap().server_test_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Effect::ApiTestList { run, config } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::Builder::new()
                    .name("apitest list".into())
                    .spawn(move || {
                        let mut cmd = apitest_command();
                        cmd.args(["list", "--json", "--config"]).arg(&config);
                        let why = match cmd.output() {
                            Ok(out) => {
                                let mut said = false;
                                for line in String::from_utf8_lossy(&out.stdout).lines() {
                                    if let Ok(e) = serde_json::from_str(line) {
                                        said = true;
                                        let _ = tx.send(Msg::ApiTestEvent(run, e));
                                    }
                                }
                                (!said).then(|| apitest_silent(&String::from_utf8_lossy(&out.stderr)))
                            }
                            Err(e) => Some(apitest_missing(&e)),
                        };
                        let _ = tx.send(Msg::ApiTestEnded(run, why));
                        wake();
                    })
                    .expect("spawn apitest list");
            }
            Effect::ApiTestRun { run, config, cases } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                let out =
                    logs(&self.shared).join("apitest").join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
                let mut cmd = apitest_command();
                cmd.args(["run", "--jsonl", "--config"]).arg(&config).arg("--out").arg(&out);
                if !cases.is_empty() {
                    cmd.arg("--cases").arg(cases.join(","));
                }
                cmd.stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(Msg::ApiTestEnded(run, Some(apitest_missing(&e))));
                        wake();
                        return;
                    }
                };
                if let Some(stdin) = child.stdin.take() {
                    self.shared.lock().unwrap().api_test_stdin = Some((run, stdin));
                }
                let shared = self.shared.clone();
                std::thread::Builder::new()
                    .name("apitest run".into())
                    .spawn(move || {
                        use std::io::{BufRead, Read};
                        let mut stderr = child.stderr.take();
                        let errs = std::thread::spawn(move || {
                            let mut s = String::new();
                            if let Some(e) = stderr.as_mut() {
                                let _ = e.read_to_string(&mut s);
                            }
                            s
                        });
                        let mut finished = false;
                        if let Some(stdout) = child.stdout.take() {
                            for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                                let e = serde_json::from_str(&line)
                                    .unwrap_or(increart_core::apitest::Event::Log { id: None, text: line });
                                finished |= matches!(
                                    e,
                                    increart_core::apitest::Event::Done { .. }
                                        | increart_core::apitest::Event::Error { .. }
                                );
                                let _ = tx.send(Msg::ApiTestEvent(run, e));
                                wake();
                            }
                        }
                        let status = child.wait();
                        let errs = errs.join().unwrap_or_default();
                        {
                            let mut sh = shared.lock().unwrap();
                            if sh.api_test_stdin.as_ref().is_some_and(|(r, _)| *r == run) {
                                sh.api_test_stdin = None;
                            }
                        }
                        let why = (!finished).then(|| match status {
                            Ok(s) => format!("it ended ({s}) before finishing: {}", last_lines(&errs, 12)),
                            Err(e) => format!("it ended before finishing: {e}"),
                        });
                        let _ = tx.send(Msg::ApiTestEnded(run, why));
                        wake();
                    })
                    .expect("spawn apitest run");
            }
            Effect::ApiTestStop => {
                use std::io::Write;
                if let Some((_, stdin)) = self.shared.lock().unwrap().api_test_stdin.as_mut() {
                    let _ = writeln!(stdin, "stop").and_then(|_| stdin.flush());
                }
            }
            Effect::LoadValidations(path) => {
                let _ = self.tx.send(Msg::Validations(increart_core::validation::Validations::load(&path)));
                (self.wake)();
            }
            Effect::ValidationAsked(path, fingerprint) => {
                // not noted: asked again next time, no worse
                let _ = increart_core::validation::mark_asked(&path, &fingerprint);
            }
            Effect::FolderPreviews { root, dir, names } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let found = names
                        .into_iter()
                        .map(|n| (n.clone(), previews(&root.join(&dir).join(n), store_of(&dir))))
                        .collect();
                    let _ = tx.send(Msg::FvPreviews(dir, found));
                    wake();
                });
            }
            Effect::CheckUsage => {
                let Some(lib) = library(&self.shared) else { return };
                let (tx, wake, shared) = (self.tx.clone(), self.wake.clone(), self.shared.clone());
                std::thread::Builder::new()
                    .name("usage check".into())
                    .spawn(move || check_usage(&lib, &shared, &tx, &wake))
                    .expect("spawn usage check");
            }
            Effect::ConvertStore(to) => {
                let Some(lib) = library(&self.shared) else { return };
                let (tx, wake, shared) = (self.tx.clone(), self.wake.clone(), self.shared.clone());
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.shared.lock().unwrap().convert_cancel = cancel.clone();
                std::thread::Builder::new()
                    .name("store conversion".into())
                    .spawn(move || convert_store(&lib, &shared, to, &cancel, &tx, &wake))
                    .expect("spawn store conversion");
            }
            Effect::SaveBugReport { config, model_dirs, validations, dest } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                let report = increart_core::bugreport::Report {
                    config,
                    logs: logs(&self.shared),
                    model_dirs: model_dirs.iter().map(|d| increart_core::config::full_path(d)).collect(),
                    about: increart_core::applog::about().unwrap_or("increart (version not known)").to_string(),
                    validations,
                };
                std::thread::Builder::new()
                    .name("bug report".into())
                    .spawn(move || {
                        let r = increart_core::bugreport::write(&report, &dest);
                        let _ = tx.send(Msg::BugReportSaved(r.map(|size| (dest, size))));
                        wake();
                    })
                    .expect("spawn bug report");
            }
            Effect::ExportLibrary { root, dest, format } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.shared.lock().unwrap().archive_cancel = cancel.clone();
                std::thread::Builder::new()
                    .name("export library".into())
                    .spawn(move || {
                        let mut progress = archive_progress(&tx, &wake, &cancel);
                        let r = increart_core::archive::export(&root, &dest, format, &mut progress);
                        let _ = tx.send(Msg::LibraryExported(r.map(|size| (dest, size)).map_err(archive_error)));
                        wake();
                    })
                    .expect("spawn export library");
            }
            Effect::UnpackArchive { archive, into } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.shared.lock().unwrap().archive_cancel = cancel.clone();
                std::thread::Builder::new()
                    .name("unpack library".into())
                    .spawn(move || {
                        let mut progress = archive_progress(&tx, &wake, &cancel);
                        let r = increart_core::archive::unpack(&archive, &into, &mut progress);
                        let _ = tx.send(Msg::ArchiveUnpacked(r.map_err(archive_error)));
                        wake();
                    })
                    .expect("spawn unpack library");
            }
            Effect::CancelArchive => {
                self.shared.lock().unwrap().archive_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Effect::CancelConvert => {
                self.shared.lock().unwrap().convert_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Effect::WatchLibrary(root) => {
                let w = crate::watch::start(root, self.tx.clone(), self.wake.clone());
                self.shared.lock().unwrap().watch = Some(w);
            }
            Effect::ComposeTiles { root, layout, settings, to } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let r = crate::tile::compose(&root, &layout, &settings).and_then(|img| {
                        if let TileTarget::File(path) = &to {
                            let bytes =
                                increart_core::export::encode(&img, settings.save_format, settings.save_quality)
                                    .map_err(|e| format!("{}: {e}", path.display()))?;
                            std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                        }
                        Ok(crate::tile::TileImage(Arc::new(img)))
                    });
                    let _ = tx.send(Msg::TilesDone(to, r));
                    wake();
                });
            }
            Effect::PeAsk { servers, jobs } => {
                let _ = self.pe_work.send(Some((servers, jobs)));
            }
            Effect::PeStop(ids) => {
                let mut s = self.shared.lock().unwrap();
                // (asks may still be on their way to the queue)
                s.pe_stopped.extend(&ids);
                s.pe_queue.retain(|j| !ids.contains(&j.id));
                // the server stops writing (it would finish the answer
                // nobody reads before starting the next)
                for p in &s.pe {
                    if p.current.is_some_and(|c| ids.contains(&c))
                        && matches!(p.server.run, increart_core::pe::PeRun::KoboldCpp { .. })
                        && let Some(client) = p.process.as_ref().map(|p| p.client.clone())
                    {
                        std::thread::spawn(move || client.abort());
                    }
                }
            }
            Effect::PeShutdown => {
                // ones starting give up
                for p in &self.shared.lock().unwrap().pe {
                    p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                let _ = self.pe_work.send(None);
            }
            Effect::OpenInEditor { path, command } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    if let Err(e) = open_in_editor(&path, &command) {
                        let _ = tx.send(Msg::Problem(e));
                        wake();
                    }
                });
            }
            Effect::PeListGpus(runtime) => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::PeGpus(runtime, increart_core::pe::list_gpus(runtime)));
                    wake();
                });
            }
            Effect::Pick { kind, start } => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::Picked(kind, pick(kind, start)));
                    wake();
                });
            }
            other => {
                self.queued.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if self.work.send(other).is_err() {
                    self.queued.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }
    }

    /// Messages ready since the last call, including engine events.
    pub fn poll(&self) -> Vec<Msg> {
        let mut out: Vec<Msg> = self.rx.try_iter().collect();
        if let Some(engine) = &self.shared.lock().unwrap().engine {
            out.extend(engine.events().try_iter().map(Msg::Engine));
        }
        out
    }

    /// Carries out a draft save on the calling thread (quitting: the worker
    /// thread may not get to it).
    pub fn save_now(&self, fx: Effect) {
        if matches!(fx, Effect::SaveDraft(_)) {
            for m in work_on(&self.shared, &self.wake, fx) {
                let _ = self.tx.send(m);
            }
        }
    }

    /// Nothing sent to the library worker is still to do, and the messages
    /// of what it did are ready (tests: effects with no reply, like saving
    /// prompt presets or reading a history, are done).
    pub fn idle(&self) -> bool {
        self.queued.load(std::sync::atomic::Ordering::SeqCst) == 0
    }

    /// Waits up to `timeout` for at least one message (tests).
    pub fn wait(&self, timeout: Duration) -> Vec<Msg> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let m = self.poll();
            if !m.is_empty() || std::time::Instant::now() >= deadline {
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
        let opening = self.shared.lock().map(|s| {
            s.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            s.opening.clone()
        });
        if let Ok(s) = self.shared.lock() {
            for p in &s.pe {
                p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if let Ok(opening) = opening {
            let deadline = std::time::Instant::now() + Duration::from_millis(1500);
            while opening.load(std::sync::atomic::Ordering::Relaxed) > 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let mut lib = None;
        if let Ok(mut s) = self.shared.lock() {
            if let Some(engine) = s.engine.take() {
                engine.shutdown();
            }
            if leave_running {
                s.servers.iter_mut().flatten().for_each(|(_, p)| p.detach());
            }
            s.servers.clear();
            // a KoboldCpp the app started stops with it
            s.pe.clear();
            s.pe_queue.clear();
            // threads still going (one stopping a KoboldCpp, the worker)
            // hold `shared`, not the library; jobs after this find none
            s.watch = None;
            lib = s.library.take();
        }
        self.pe_signal.notify_all();
        // a job using it (a save, a usage check) ends first
        if let Some(lib) = lib {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while Arc::strong_count(&lib) > 1 && std::time::Instant::now() < deadline {
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

/// What PE threads share.
#[derive(Clone)]
struct PeCtx {
    tx: Sender<Msg>,
    wake: Wake,
    shared: Arc<Mutex<Shared>>,
    signal: Arc<std::sync::Condvar>,
}

impl PeCtx {
    fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        (self.wake)();
    }
}

/// Uses `servers` from now (None: none), keeping ones open or starting
/// that run as the same process, and queues `jobs` for them. Ones no
/// longer used stop before new ones start (one may want the other's
/// port).
fn pe_configure(pe: &PeCtx, work: Option<(Vec<increart_core::pe::PeServer>, Vec<crate::update::PeJob>)>) {
    let (servers, jobs) = work.unwrap_or_default();
    let (removed, started) = {
        let mut s = pe.shared.lock().unwrap();
        let mut old = std::mem::take(&mut s.pe);
        let mut started = vec![];
        for server in servers {
            match old.iter().position(|p| p.failed.is_none() && p.server.same_process(&server)) {
                Some(i) => s.pe.push(old.remove(i)),
                None => {
                    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let id = s.pe_next;
                    s.pe_next += 1;
                    started.push((id, server.clone(), cancel.clone()));
                    s.pe.push(PeSlot { id, server, process: None, failed: None, cancel, current: None });
                }
            }
        }
        for p in &old {
            p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if s.pe.is_empty() {
            s.pe_queue.clear();
        }
        for j in jobs {
            if !s.pe_stopped.remove(&j.id) {
                s.pe_queue.push_back(j);
            }
        }
        (old, started)
    };
    pe.signal.notify_all();
    // dropping them stops their processes (and waits for them)
    drop(removed);
    pe_report(pe);
    for (id, server, cancel) in started {
        let pe = pe.clone();
        std::thread::Builder::new()
            .name(format!("prompt enhance {}", server.name))
            .spawn(move || pe_worker(&pe, id, server, &cancel))
            .expect("spawn PE worker");
    }
}

/// Reports how the PE servers are getting on, when that changed: Ready
/// once any is, Failed (dropping the asks queued) once all failed.
fn pe_report(pe: &PeCtx) {
    use crate::state::PeStatus;
    let (status, ready) = {
        let mut s = pe.shared.lock().unwrap();
        let ready = s.pe.iter().filter(|p| p.process.is_some()).count();
        let status = if s.pe.is_empty() {
            PeStatus::Off
        } else if ready > 0 {
            PeStatus::Ready
        } else if s.pe.iter().any(|p| p.failed.is_none()) {
            PeStatus::Starting
        } else {
            let failed: Vec<String> = s.pe.iter().filter_map(|p| p.failed.clone()).collect();
            s.pe_queue.clear();
            PeStatus::Failed(failed.join("\n"))
        };
        let before = s.pe_reported.replace((status.clone(), ready));
        if before.as_ref() == Some(&(status.clone(), ready)) {
            return;
        }
        if before.is_some_and(|(b, _)| b == status) { (None, ready) } else { (Some(status), ready) }
    };
    pe.send(Msg::PeReady(ready));
    if let Some(status) = status {
        pe.send(Msg::PeStatus(status));
    }
}

/// Opens `server`, then answers queued asks with it, one at a time, until
/// its slot (`id`) is gone.
fn pe_worker(pe: &PeCtx, id: u64, server: increart_core::pe::PeServer, cancel: &std::sync::atomic::AtomicBool) {
    use increart_core::pe;
    let opened = pe::open(&server, &logs(&pe.shared), increart_core::config::timeouts().server_start(), cancel);
    let (client, unused, problem) = {
        let mut s = pe.shared.lock().unwrap();
        let others_ready = s.pe.iter().any(|p| p.id != id && p.process.is_some());
        match (s.pe.iter_mut().find(|p| p.id == id), opened) {
            (Some(slot), Ok(p)) => {
                let c = p.client.clone();
                slot.process = Some(p);
                (Some(c), None, None)
            }
            // stopped while starting: not a failure
            (None, r) => (None, r.ok(), None),
            (Some(slot), Err(e)) => {
                slot.failed = Some(e.clone());
                (None, None, others_ready.then(|| format!("{}: {e}", server.name)))
            }
        }
    };
    drop(unused);
    if let Some(p) = problem {
        pe.send(Msg::Problem(p));
    }
    pe_report(pe);
    let Some(client) = client else { return };
    loop {
        let job = {
            let mut s = pe.shared.lock().unwrap();
            loop {
                if !s.pe.iter().any(|p| p.id == id) {
                    return;
                }
                if let Some(j) = s.pe_queue.pop_front() {
                    if let Some(slot) = s.pe.iter_mut().find(|p| p.id == id) {
                        slot.current = Some(j.id);
                    }
                    break j;
                }
                s = pe.signal.wait(s).unwrap();
            }
        };
        pe.send(Msg::PeOn(job.id, server.name.clone()));
        let r = pe_answer(pe, &client, &job);
        let was_stopped = {
            let mut s = pe.shared.lock().unwrap();
            if let Some(slot) = s.pe.iter_mut().find(|p| p.id == id) {
                slot.current = None;
            }
            s.pe_stopped.remove(&job.id)
        };
        if !was_stopped {
            pe.send(Msg::PeAnswer(job.id, r));
        }
    }
}

/// One chat request for `job`, reporting the answer so far a few times a
/// second.
fn pe_answer(pe: &PeCtx, client: &increart_core::pe::Client, job: &crate::update::PeJob) -> Result<String, String> {
    use crate::state::PeStep;
    use increart_core::pe;
    if !job.images.is_empty() {
        pe.send(Msg::PeProgress(job.id, PeStep::Preparing));
    }
    let images: Result<Vec<_>, String> = job.images.iter().map(|p| pe::image_for_request(p, job.image_size)).collect();
    pe.send(Msg::PeProgress(job.id, PeStep::Reading { images: job.images.len() }));
    let mut last = Instant::now() - PE_PROGRESS_EVERY;
    images.and_then(|imgs| {
        client.chat_stream(&job.system, &job.text, &imgs, job.max_tokens, job.sampling, |pieces, text| {
            if last.elapsed() >= PE_PROGRESS_EVERY {
                last = Instant::now();
                pe.send(Msg::PeProgress(job.id, PeStep::Writing { pieces, text: text.to_string() }));
            }
            !pe.shared.lock().unwrap().pe_stopped.contains(&job.id)
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
fn list_loadable(tx: &Sender<Msg>, wake: &Wake, shared: &Arc<Mutex<Shared>>) {
    let clients: Vec<_> =
        shared.lock().unwrap().servers.iter().flatten().map(|(_, p)| (p.name.clone(), p.client.clone())).collect();
    let (tx, wake) = (tx.clone(), wake.clone());
    std::thread::spawn(move || {
        let found = clients.into_iter().filter_map(|(name, c)| c.loadable().ok().map(|l| (name, l))).collect();
        let _ = tx.send(Msg::ServerModels(found));
        wake();
    });
}

/// Opens the servers of one attempt in parallel; reports each, then settles.
fn open_servers(
    tx: &Sender<Msg>,
    wake: &Wake,
    shared: &Arc<Mutex<Shared>>,
    servers: Vec<ServerConfig>,
    cfg: SdServerConfig,
    attempt: u32,
) {
    let (cancel, opening, kept) = {
        let mut s = shared.lock().unwrap();
        // the attempt before (if still opening) and the engine stop; the
        // servers opened as asked again are kept, the others stop
        s.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(engine) = s.engine.take() {
            engine.shutdown();
        }
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
        // dropping a process the app started stops it (kill, then wait)
        let stopped = old.iter().flatten().filter(|(_, p)| p.launched()).map(|(_, p)| p.name.clone()).collect();
        drop(old);
        let _ = tx.send(Msg::ServersStopped(stopped));
        wake();
        s.servers = slots;
        s.opened_with = Some(cfg.clone());
        s.attempt = attempt;
        s.cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        s.opening.fetch_add(kept.iter().filter(|k| !**k).count(), std::sync::atomic::Ordering::Relaxed);
        (s.cancel.clone(), s.opening.clone(), kept)
    };
    let (tx, wake, shared) = (tx.clone(), wake.clone(), shared.clone());
    std::thread::spawn(move || {
        let handles: Vec<_> = servers
            .into_iter()
            .enumerate()
            .map(|(i, server)| {
                let (tx, wake, shared, cfg) = (tx.clone(), wake.clone(), shared.clone(), cfg.clone());
                let logs = logs(&shared);
                let (cancel, opening) = (cancel.clone(), opening.clone());
                let kept = kept[i];
                std::thread::spawn(move || {
                    if kept {
                        let _ = tx.send(Msg::ServerOpened(attempt, i, Ok(())));
                        wake();
                        return;
                    }
                    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
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
                    done.store(true, std::sync::atomic::Ordering::Relaxed);
                    opening.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    let msg = match opened {
                        Ok(p) => {
                            let mut s = shared.lock().unwrap();
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
        let _ = tx.send(Msg::ServersSettled(attempt));
        wake();
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
    done: &Arc<std::sync::atomic::AtomicBool>,
) {
    let log = launcher::log_path(logs, server);
    let start = std::fs::metadata(&log).map_or(0, |m| m.len());
    let (tx, wake, done) = (tx.clone(), wake.clone(), done.clone());
    std::thread::spawn(move || {
        let mut last = None;
        while !done.load(std::sync::atomic::Ordering::Relaxed) {
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
        // a project's discarded images are shown in Output library → Discarded
        if name.starts_with('.')
            || (name == increart_core::scan::DISCARDED_OUTPUTS && dir.join("project.rsn").is_file())
        {
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

fn work_on(shared: &Arc<Mutex<Shared>>, wake: &Wake, fx: Effect) -> Vec<Msg> {
    match fx {
        Effect::SaveConfig(config, path) => match config.save(&path) {
            Ok(()) => vec![],
            Err(e) => vec![Msg::Problem(format!("saving settings: {e}"))],
        },
        Effect::OpenLibrary { path, create } => {
            let opened = if create { Library::create(&path, None) } else { Library::open(&path) };
            vec![Msg::LibraryOpened(opened.map_err(err).and_then(|mut lib| {
                if let Some(d) = shared.lock().unwrap().disposal.clone() {
                    lib.set_disposal(d);
                }
                let ix = lib.scan().map_err(err)?;
                let root = lib.root().to_path_buf();
                shared.lock().unwrap().library = Some(Arc::new(Mutex::new(lib)));
                Ok((root, ix))
            }))]
        }
        Effect::StoreInfo => {
            let Some(lib) = library(shared) else { return vec![] };
            vec![Msg::StoreInfo(store_info(&lib.lock().unwrap()))]
        }
        Effect::SetStoreFormat(format) => {
            let Some(lib) = library(shared) else { return vec![] };
            let mut l = lib.lock().unwrap();
            vec![Msg::StoreInfo(l.set_store_format(format).map_err(err).and_then(|()| store_info(&l)))]
        }
        Effect::SaveDraft(d) => {
            let Some(lib) = library(shared) else { return vec![] };
            let l = lib.lock().unwrap();
            let r = match d {
                DraftSave::Generate(project, draft) => l.save_generate_draft(&project, &draft),
                DraftSave::Idea(draft) => l.save_idea_draft(&draft),
                DraftSave::ClearIdea => l.clear_idea_draft(),
            };
            vec![Msg::DraftSaved(r.map_err(err))]
        }
        Effect::LoadHistory(key) => {
            let Some(lib) = library(shared) else { return vec![] };
            let path = lib.lock().unwrap().history_path(&key);
            let entries = path.map(|p| increart_core::history::read(&p)).unwrap_or_default();
            vec![Msg::HistoryLoaded(key, entries)]
        }
        Effect::RecordHistory { key, mut entry, replace_last } => {
            let Some(lib) = library(shared) else { return vec![] };
            let path = lib.lock().unwrap().history_path(&key);
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
            let Some(lib) = library(shared) else { return vec![] };
            let r = lib.lock().unwrap().recipe(&id).ok().flatten();
            vec![Msg::RecipeLoaded(id, r)]
        }
        Effect::LoadInputMeta(ids) => {
            let Some(lib) = library(shared) else { return vec![] };
            let l = lib.lock().unwrap();
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
            let Some(lib) = library(shared) else { return vec![] };
            let l = lib.lock().unwrap();
            let mut ids = l.store_ids(increart_core::Store::Input).unwrap_or_default();
            ids.reverse();
            let list =
                ids.into_iter().take(200).filter_map(|id| l.source_info(&id).ok().flatten().map(|s| (id, s))).collect();
            vec![Msg::Imports(list)]
        }
        Effect::Export { images, dest, keep_metadata, by_id } => {
            let Some(lib) = library(shared) else { return vec![] };
            let l = lib.lock().unwrap();
            vec![Msg::Exported(export(&l, &images, &dest, keep_metadata, by_id))]
        }
        Effect::ShowInFileManager { path, command } => match show_in_file_manager(&path, &command) {
            Ok(()) => vec![],
            Err(e) => vec![Msg::Problem(e)],
        },
        Effect::ListDir(rel) => {
            let Some(lib) = library(shared) else { return vec![] };
            let root = lib.lock().unwrap().root().to_path_buf();
            vec![Msg::FvListed(rel.clone(), list_dir(&root, &rel))]
        }
        Effect::Rescan => match library(shared) {
            Some(lib) => lib.lock().unwrap().scan().map(Msg::Scanned).into_iter().collect(),
            None => vec![],
        },
        Effect::StopEngine => {
            let engine = shared.lock().unwrap().engine.take();
            if let Some(engine) = engine {
                engine.shutdown();
            }
            vec![]
        }
        Effect::SavePresets(presets, path) => match presets.save_dir(&path) {
            Ok(()) => vec![],
            Err(e) => vec![Msg::Problem(format!("saving prompt presets: {e}"))],
        },
        Effect::StopServers => {
            let mut s = shared.lock().unwrap();
            s.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(engine) = s.engine.take() {
                engine.shutdown();
            }
            // dropping a process the app started stops it (kill, then wait)
            let names = s.servers.iter().flatten().filter(|(_, p)| p.launched()).map(|(_, p)| p.name.clone()).collect();
            s.servers.clear();
            s.opened_with = None;
            vec![Msg::ServersStopped(names)]
        }
        Effect::SetJobOrder(order) => {
            if let Some(e) = &shared.lock().unwrap().engine {
                e.set_order(order);
            }
            vec![]
        }
        Effect::SetSkipRamCheck(skip) => {
            if let Some(e) = &shared.lock().unwrap().engine {
                e.set_skip_ram_check(skip);
            }
            vec![]
        }
        Effect::SetLogTraffic(on) => {
            increart_core::traffic::set_on(on);
            vec![]
        }
        Effect::StartEngine { shared: cfg, order, skip_ram_check } => {
            let Some(lib) = library(shared) else { return vec![] };
            // Runs with seeds left, found before taking any other lock (the
            // library lock is held once, never nested). From one scan: the
            // library's per-run look-up walks every project, which on a cold
            // disk held this queue (and Saves behind it) for half a minute.
            let scanned = lib.lock().unwrap().scan();
            let pending: Vec<_> = match scanned {
                Ok(ix) => ix
                    .projects
                    .iter()
                    .flat_map(|p| p.runs.iter().filter(|r| p.pending_seeds(r).next().is_some()))
                    .map(|r| r.id.clone())
                    .collect(),
                Err(_) => vec![],
            };
            let endpoints: Vec<Endpoint> = shared
                .lock()
                .unwrap()
                .servers
                .iter()
                .flatten()
                .map(|(_, p)| Endpoint { name: p.name.clone(), device: p.device.clone(), client: p.client.clone() })
                .collect();
            let mut opts = EngineOptions {
                generation: GenOptions { preview: cfg.preview.clone(), ..Default::default() },
                sdcpp: cfg.commit.clone(),
                order,
                skip_ram_check,
                ..Default::default()
            };
            if let Some((poll, backoff)) = shared.lock().unwrap().engine_timing {
                (opts.poll_interval, opts.backoff_base) = (poll, backoff);
            }
            let w = wake.clone();
            let engine = Engine::start(lib.clone(), endpoints, opts, Some(Box::new(move || w())));
            for r in &pending {
                engine.queue_run(r);
            }
            shared.lock().unwrap().engine = Some(engine);
            vec![Msg::RunsQueued(pending)]
        }
        Effect::QueueRun(id) => {
            if let Some(e) = &shared.lock().unwrap().engine {
                e.queue_run(&id);
            }
            vec![]
        }
        Effect::CancelRun(id, how) => {
            if let Some(e) = &shared.lock().unwrap().engine {
                e.cancel_run(&id, how);
            }
            vec![]
        }
        Effect::Lib(op) => {
            let Some(lib) = library(shared) else { return vec![Msg::OpDone(Err("no library open".into()), None)] };
            let queue_run = matches!(&op, LibOp::CreateRun { queue: true, .. });
            let op_queue = match &op {
                LibOp::CreateSweep { queue, .. } => Some(*queue),
                _ => None,
            };
            hash_ahead(&lib, &op);
            let result = {
                let mut l = lib.lock().unwrap();
                lib_op(&mut l, op)
            };
            let queue_run = queue_run || matches!(&op_queue, Some(true));
            if let Some(e) = &shared.lock().unwrap().engine
                && queue_run
            {
                match &result {
                    Ok(OpOk::Run(id, _)) => e.queue_run(id),
                    // all at once: the scheduler's round robin goes breadth-first
                    Ok(OpOk::Sweep(ids, ..)) => ids.iter().for_each(|id| e.queue_run(id)),
                    _ => {}
                }
            }
            let ix = lib.lock().unwrap().scan().ok();
            vec![Msg::OpDone(result, ix)]
        }
        Effect::ListDevices(_)
        | Effect::DetectGpus(_)
        | Effect::KoboldVersion(_)
        | Effect::DownloadModels { .. }
        | Effect::CancelDownload
        | Effect::TestServer { .. }
        | Effect::StopServerTest
        | Effect::ApiTestList { .. }
        | Effect::ApiTestRun { .. }
        | Effect::ApiTestStop
        | Effect::LoadValidations(_)
        | Effect::ValidationAsked(..)
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
            unreachable!("handled on their own threads")
        }
    }
}

/// The wizard's model files, one after the other, reporting progress a
/// few times a second.
fn download_models(
    dir: &str,
    files: &[increart_core::tiers::Download],
    search: &[PathBuf],
    cancel: &std::sync::atomic::AtomicBool,
    tx: &Sender<Msg>,
    wake: &Wake,
) -> Result<(), String> {
    use increart_core::tiers;
    use std::sync::atomic::Ordering;
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
fn check_usage(lib: &Arc<Mutex<Library>>, shared: &Arc<Mutex<Shared>>, tx: &Sender<Msg>, wake: &Wake) {
    let check = lib.lock().unwrap().usage_check();
    let Ok(check) = check else { return };
    let total = check.images.len();
    // shown from the start, however quick
    let _ = tx.send(Msg::UsageCheck(0, total));
    wake();
    let mut sent = std::time::Instant::now();
    for (i, r) in check.images.iter().enumerate() {
        if !library(shared).is_some_and(|l| Arc::ptr_eq(&l, lib)) {
            let _ = tx.send(Msg::UsageCheck(total, total));
            wake();
            return;
        }
        // read without the lock (a cold disk makes this the slow part, and
        // holding the lock through it kept Saves waiting); lock to fix
        if !check.covers(r) {
            lib.lock().unwrap().check_usage(&check, r);
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
    cancel: &'a std::sync::atomic::AtomicBool,
) -> impl FnMut(u64, u64) -> bool + 'a {
    let mut last = Instant::now() - Duration::from_secs(1);
    move |done, total| {
        if last.elapsed() >= Duration::from_millis(200) {
            last = Instant::now();
            let _ = tx.send(Msg::Archiving(done, total));
            wake();
        }
        !cancel.load(std::sync::atomic::Ordering::Relaxed)
    }
}

fn archive_error(e: increart_core::Error) -> String {
    if increart_core::archive::is_cancelled(&e) { crate::update::ARCHIVE_STOPPED.into() } else { e.to_string() }
}

fn convert_store(
    lib: &Arc<Mutex<Library>>,
    shared: &Arc<Mutex<Shared>>,
    to: StoreFormat,
    cancel: &std::sync::atomic::AtomicBool,
    tx: &Sender<Msg>,
    wake: &Wake,
) {
    let send = |m: Msg| {
        let _ = tx.send(m);
        wake();
    };
    let conv = match lib.lock().unwrap().conversion(to) {
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
        if cancel.load(std::sync::atomic::Ordering::Relaxed) || !library(shared).is_some_and(|l| Arc::ptr_eq(&l, lib)) {
            cancelled = true;
            break;
        }
        let encoded: Vec<_> = std::thread::scope(|s| {
            let parts: Vec<_> = batch
                .chunks(batch.len().div_ceil(threads))
                .map(|part| s.spawn(|| part.iter().map(|r| (r, conv.encode(r))).collect::<Vec<_>>()))
                .collect();
            parts.into_iter().flat_map(|h| h.join().expect("encoding thread")).collect()
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
        match lib.lock().unwrap().install(ready) {
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

fn library(shared: &Arc<Mutex<Shared>>) -> Option<Arc<Mutex<Library>>> {
    shared.lock().unwrap().library.clone()
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
        LibOp::NewIdeaNewProject(n, folder, name) => {
            // Save & new project may name a new library folder
            if let IdeaLoc::Library(f) = &n.loc {
                lib.make_library_folder(f).map_err(err)?;
            }
            let id = lib.new_idea(n).map_err(err)?;
            new_project_from_ideas(lib, &[id], &folder, &name)
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
        LibOp::RestoreProject(id) => lib.restore_project(&id).map(|_| OpOk::Done).map_err(err),
        LibOp::DeleteDiscardedProject(id) => lib.delete_discarded_project(&id).map(|_| OpOk::Done).map_err(err),
        LibOp::NewProjectFromIdeas(ids, folder, name) => new_project_from_ideas(lib, &ids, &folder, &name),
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
        LibOp::RestoreOutputs(links) => {
            for l in links {
                lib.restore_output_link(&l).map_err(err)?;
            }
            Ok(OpOk::Done)
        }
        LibOp::DeleteDiscardedOutputs(links) => {
            for l in links {
                lib.delete_discarded_output(&l).map_err(err)?;
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
fn new_project_from_ideas(lib: &mut Library, ids: &[Id], folder: &Path, name: &str) -> Result<OpOk, String> {
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
    Ok(OpOk::ProjectFromIdeas { project, prompt, refs, idea: ids.first().cloned() })
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
    let cache = lib.lock().unwrap().root().join("cache/model-hashes.rsn");
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
        let cancel = std::sync::atomic::AtomicBool::new(false);
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
