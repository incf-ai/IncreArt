//! Runs generation: the [`Scheduler`] on a coordinator thread, one worker
//! thread per sd-server doing the HTTP, and finished images written into the
//! library. The GUI talks to it with [`Engine::queue_run`] /
//! [`Engine::cancel_run`] and reads [`Event`]s.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value;

use crate::id::Id;
use crate::library::Library;
use crate::model::Run;
use crate::request::{self, GenOptions, Provenance};
use crate::scheduler::{Action, LoadResult, Order, Outcome, Scheduler, ServerIdx, ServerState, Ticket};
use crate::sdserver::{ApiError, Client, Job, JobStatus, Loadable, ModelKey, ModelStatus};

/// A run's generation log (DESIGN.md, Filesystem layout): what the engine
/// did for it and what its servers answered, a line each, next to its
/// `run.rsn`.
pub const RUN_LOG: &str = "generation.log";

/// The generation log of `run` in the project at `project_dir`.
pub fn run_log(project_dir: &Path, run: &Id) -> PathBuf {
    project_dir.join("runs").join(run.as_str()).join(RUN_LOG)
}

/// The last `max` lines of a generation log (none without one).
pub fn read_run_log(path: &Path, max: usize) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(max)..].iter().map(|l| l.to_string()).collect()
}

/// One server the engine may use.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub name: String,
    /// `--backend` device, recorded in recipes.
    pub device: String,
    pub client: Client,
}

#[derive(Clone, Debug)]
pub struct EngineOptions {
    pub generation: GenOptions,
    /// sd.cpp commit for recipes.
    pub sdcpp: String,
    /// How often in-flight jobs are polled.
    pub poll_interval: Duration,
    pub max_in_flight: usize,
    pub backoff_base: Duration,
    pub warm_up: bool,
    pub order: Order,
    /// Loads on this machine don't wait for free RAM (see
    /// [`Engine::set_skip_ram_check`]).
    pub skip_ram_check: bool,
}

impl Default for EngineOptions {
    fn default() -> Self {
        EngineOptions {
            generation: GenOptions::default(),
            sdcpp: String::new(),
            poll_interval: Duration::from_millis(500),
            max_in_flight: 2,
            backoff_base: Duration::from_secs(2),
            warm_up: true,
            order: Order::Spread,
            skip_ram_check: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A server's scheduler state changed (`state` is for display; `idle`:
    /// ready with no job in flight; `runs`: those with a job in flight
    /// there).
    Server { server: ServerIdx, state: String, model: Option<ModelKey>, idle: bool, runs: Vec<Id> },
    /// Sampling progress of a seed, with the latest preview PNG if any.
    Progress { run: Id, rev: u32, seed: u64, server: ServerIdx, step: u32, total: u32, preview: Option<Vec<u8>> },
    /// An image was stored and linked into the project's `unreviewed/`.
    Output { run: Id, rev: u32, seed: u64, image: Id, link: PathBuf },
    /// A seed failed for good (recorded in `run.rsn`).
    SeedFailed { run: Id, rev: u32, seed: u64, error: String },
    /// No seeds of the run are left to generate.
    RunDone { run: Id },
    /// Something the user should see that isn't about one seed.
    Problem(String),
    /// A line of a run's generation log (written to its [`RUN_LOG`] too).
    Log { run: Id, line: String },
}

/// A server's last reported state, model, idleness and runs in flight.
type Shown = (String, Option<ModelKey>, bool, Vec<Id>);

/// How a cancel treats the images being generated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// They finish and are kept; jobs still waiting are cancelled.
    Finish,
    /// They are cancelled too, and let go at once (sd-server stops them at
    /// its next checkpoint; the run doesn't wait for that).
    Now,
}

enum Msg {
    Queue(Id),
    Cancel(Id, Stop),
    Order(Order),
    Report(ServerIdx, Report),
    Shutdown,
}

enum Cmd {
    /// Its model list (for list round `round`), then its status.
    Probe {
        round: u64,
    },
    /// Its model list again.
    List {
        round: u64,
    },
    WarmUp(Value),
    Load(ModelKey),
    Submit {
        ticket: Ticket,
        body: Value,
    },
    /// Cancels a job and stops polling it (the coordinator has let its
    /// seed go already).
    Cancel {
        ticket: Ticket,
    },
}

enum Report {
    Probed(Result<ModelStatus, ApiError>),
    /// What it can load (`None`: it couldn't say).
    Listed {
        round: u64,
        loadable: Option<Loadable>,
    },
    /// A job was seen generating (not waiting in the server's queue).
    Generating {
        ticket: Ticket,
    },
    /// The server took a seed as this job.
    Submitted {
        ticket: Ticket,
        id: String,
    },
    Warmed(Result<(), ApiError>),
    Loaded(Result<ModelStatus, ApiError>),
    SubmitFailed {
        ticket: Ticket,
        error: ApiError,
    },
    Progress {
        ticket: Ticket,
        step: u32,
        total: u32,
        preview: Option<Vec<u8>>,
    },
    Finished {
        ticket: Ticket,
        job: Box<Job>,
        /// Seconds per sampling step, from the progress reports (0: unknown).
        step_s: f32,
    },
    /// The job vanished (purged after its TTL, or the server restarted).
    Gone {
        ticket: Ticket,
    },
    /// Cancelled before it was submitted.
    NotSubmitted {
        ticket: Ticket,
    },
    /// Polling failed: the server is unwell; its jobs are abandoned.
    PollFailed(ApiError),
    /// A load waits (for another server's load, or for free RAM): shown as
    /// its state until `None`.
    Waiting(Option<String>),
    /// A load went ahead after [`RAM_WAIT`] without the RAM it wanted free
    /// (the figures).
    ShortOfRam(String),
}

pub struct Engine {
    tx: Sender<Msg>,
    events: Receiver<Event>,
    /// No load waits for RAM.
    skip_ram_check: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    /// Starts the engine. `notify` is called after every event is sent (e.g.
    /// to request a GUI repaint).
    pub fn start(
        lib: Arc<Mutex<Library>>,
        servers: Vec<Endpoint>,
        opts: EngineOptions,
        notify: Option<Box<dyn Fn() + Send>>,
    ) -> Engine {
        let (tx, rx) = mpsc::channel();
        let (etx, events) = mpsc::channel();
        let skip_ram_check = Arc::new(AtomicBool::new(opts.skip_ram_check));
        let workers = servers
            .iter()
            .enumerate()
            .map(|(i, ep)| {
                let (ctx, crx) = mpsc::channel();
                let client = ep.client.clone();
                let report = tx.clone();
                let poll = opts.poll_interval;
                let skip = skip_ram_check.clone();
                std::thread::Builder::new()
                    .name(format!("sd-server {}", ep.name))
                    .spawn(move || worker(i, client, crx, report, poll, skip))
                    .expect("spawn worker");
                ctx
            })
            .collect();
        let mut sched = Scheduler::new(servers.len());
        sched.max_in_flight = opts.max_in_flight;
        sched.backoff_base = opts.backoff_base;
        sched.order = opts.order;
        let mut co = Coordinator {
            lib,
            servers,
            workers,
            sched,
            opts,
            runs: HashMap::new(),
            shown: vec![],
            waiting: vec![],
            list_round: 0,
            listed: vec![],
            relist: false,
            generating: HashSet::new(),
            told_stuck: HashSet::new(),
            logs: HashMap::new(),
            load_started: vec![],
            body_logged: HashSet::new(),
            events: etx,
            notify,
        };
        let thread = std::thread::Builder::new().name("engine".into()).spawn(move || co.run(rx)).expect("spawn engine");
        Engine { tx, events, skip_ram_check, thread: Some(thread) }
    }

    /// Queues the pending seeds of a run (again after adding seeds: only
    /// seeds not queued yet are added).
    pub fn queue_run(&self, run: &Id) {
        let _ = self.tx.send(Msg::Queue(run.clone()));
    }

    /// Drops a run's pending seeds and cancels its in-flight jobs (with
    /// [`Stop::Finish`], those not generating yet); the seeds are recorded as
    /// cancelled.
    pub fn cancel_run(&self, run: &Id, how: Stop) {
        let _ = self.tx.send(Msg::Cancel(run.clone(), how));
    }

    /// Spread the servers over the queued runs, or finish them one by one.
    pub fn set_order(&self, order: Order) {
        let _ = self.tx.send(Msg::Order(order));
    }

    /// Loads on this machine stop (or start again) waiting for free RAM;
    /// one waiting now goes ahead.
    pub fn set_skip_ram_check(&self, skip: bool) {
        self.skip_ram_check.store(skip, Ordering::Relaxed);
    }

    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }

    /// Stops the coordinator. Workers finish their current request and exit.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Coordinator {
    lib: Arc<Mutex<Library>>,
    servers: Vec<Endpoint>,
    workers: Vec<Sender<Cmd>>,
    sched: Scheduler,
    opts: EngineOptions,
    /// Definitions of queued runs.
    runs: HashMap<Id, Run>,
    /// Last server state sent as an event.
    shown: Vec<Option<Shown>>,
    /// Why a server's load waits, shown in place of "loading …".
    waiting: Vec<Option<String>>,
    /// The latest round of model lists asked for, and the round of each
    /// server's latest list (`None`: none yet).
    list_round: u64,
    listed: Vec<Option<u64>>,
    /// Runs were queued: the servers' model lists are asked for again.
    relist: bool,
    /// Tickets whose job was seen generating.
    generating: HashSet<Ticket>,
    /// Models said to be loadable nowhere (said once until that changes).
    told_stuck: HashSet<ModelKey>,
    /// Queued runs' generation logs.
    logs: HashMap<Id, PathBuf>,
    /// When each server's latest load was asked for.
    load_started: Vec<Option<Instant>>,
    /// Runs whose request body is in their log (the first seed's).
    body_logged: HashSet<Id>,
    events: Sender<Event>,
    notify: Option<Box<dyn Fn() + Send>>,
}

fn state_label(s: &ServerState) -> String {
    match s {
        ServerState::Unknown | ServerState::Probing => "connecting".into(),
        ServerState::NeedsWarmUp | ServerState::WarmingUp => "warming up".into(),
        ServerState::Ready => "ready".into(),
        ServerState::Draining { .. } => "draining".into(),
        ServerState::Loading { target } => format!("loading {}", target.recipe),
        ServerState::Backoff { .. } => "unreachable, retrying".into(),
    }
}

impl Coordinator {
    fn run(&mut self, rx: Receiver<Msg>) {
        self.shown = vec![None; self.servers.len()];
        self.waiting = vec![None; self.servers.len()];
        self.listed = vec![None; self.servers.len()];
        self.load_started = vec![None; self.servers.len()];
        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
            }
            // Drain whatever else arrived before deciding.
            while let Ok(msg) = rx.try_recv() {
                if matches!(msg, Msg::Shutdown) {
                    return;
                }
                self.handle(msg);
            }
            self.step();
        }
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
        if let Some(n) = &self.notify {
            n();
        }
    }

    /// Adds a line to a run's generation log (`server`: whose doing).
    fn log(&self, run: &Id, server: Option<ServerIdx>, text: &str) {
        let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let line = match server {
            Some(s) => format!("{at} [{}] {text}", self.servers[s].name),
            None => format!("{at} {text}"),
        };
        if let Some(path) = self.logs.get(run) {
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| writeln!(f, "{line}"));
        }
        self.emit(Event::Log { run: run.clone(), line });
    }

    /// Logs a line for every queued run of `model`.
    fn log_model(&self, model: &ModelKey, server: Option<ServerIdx>, text: &str) {
        let runs: Vec<&Id> =
            self.runs.iter().filter(|(_, r)| ModelKey::from(&r.model) == *model).map(|(id, _)| id).collect();
        for run in runs {
            self.log(run, server, text);
        }
    }

    /// Logs a line for every run with a seed in flight on `server`.
    fn log_server(&self, server: ServerIdx, text: &str) {
        let mut runs: Vec<&Id> = vec![];
        for (_, seed) in self.sched.in_flight(server) {
            if !runs.contains(&&seed.run) {
                runs.push(&seed.run);
            }
        }
        for run in runs {
            self.log(run, Some(server), text);
        }
    }

    /// Logs a line about a ticket's seed.
    fn log_ticket(&self, ticket: Ticket, server: ServerIdx, text: &str) {
        if let Some((_, seed)) = self.sched.ticket(ticket) {
            self.log(&seed.run, Some(server), &format!("seed {}: {text}", seed.seed));
        }
    }

    fn lib(&self) -> std::sync::MutexGuard<'_, Library> {
        self.lib.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn step(&mut self) {
        let now = Instant::now();
        if std::mem::take(&mut self.relist) {
            self.list_round += 1;
            for i in 0..self.servers.len() {
                self.send(i, Cmd::List { round: self.list_round });
            }
        }
        for action in self.sched.poll(now) {
            self.perform(action);
        }
        self.tell_stuck();
        for i in 0..self.servers.len() {
            let mut state = state_label(self.sched.server_state(i));
            if let (ServerState::Loading { .. }, Some(w)) = (self.sched.server_state(i), &self.waiting[i]) {
                state = w.clone();
            }
            let model = self.sched.model_of(i).cloned();
            let idle =
                matches!(self.sched.server_state(i), ServerState::Ready) && self.sched.in_flight(i).next().is_none();
            let mut runs: Vec<Id> = vec![];
            for (_, r) in self.sched.in_flight(i) {
                if !runs.contains(&r.run) {
                    runs.push(r.run.clone());
                }
            }
            let now = (state, model, idle, runs);
            if self.shown[i].as_ref() != Some(&now) {
                self.shown[i] = Some(now.clone());
                let (state, model, idle, runs) = now;
                self.emit(Event::Server { server: i, state, model, idle, runs });
            }
        }
    }

    /// Says which queued models no server can load, and why for each, once
    /// every server has given its latest model list.
    fn tell_stuck(&mut self) {
        if self.listed.iter().any(|l| *l != Some(self.list_round)) {
            return;
        }
        let stuck: Vec<ModelKey> =
            self.sched.stuck_models().into_iter().filter(|m| !self.sched.gave_up(m)).cloned().collect();
        self.told_stuck.retain(|m| stuck.contains(m));
        for m in stuck {
            if !self.told_stuck.insert(m.clone()) {
                continue;
            }
            let why: Vec<String> = (0..self.servers.len())
                .filter_map(|i| self.sched.why_not(i, &m).map(|w| format!("{}: {w}", self.servers[i].name)))
                .collect();
            let text = format!("no server can load {}, so its runs wait ({})", m.recipe, why.join("; "));
            self.log_model(&m, None, &text);
            self.emit(Event::Problem(text));
        }
    }

    fn perform(&mut self, action: Action) {
        let now = Instant::now();
        match action {
            Action::Probe { server } => self.send(server, Cmd::Probe { round: self.list_round }),
            Action::WarmUp { server } if self.opts.warm_up => self.send(server, Cmd::WarmUp(request::warmup_body())),
            Action::WarmUp { server } => {
                self.sched.warmed(server, true, now);
            }
            Action::Load { server, model } => {
                let others: Vec<String> = (0..self.servers.len())
                    .filter_map(|i| self.sched.why_not(i, &model).map(|w| format!("{}: {w}", self.servers[i].name)))
                    .collect();
                let mut text = format!("loading {}", model_text(&model));
                if !others.is_empty() {
                    text += &format!(" (not on {})", others.join("; "));
                }
                self.log_model(&model, Some(server), &text);
                self.load_started[server] = Some(now);
                self.send(server, Cmd::Load(model))
            }
            Action::Submit { server, ticket, seed } => {
                let body = match self.runs.get(&seed.run) {
                    Some(run) => request::img_gen_body(&self.lib(), run, seed.seed, &self.opts.generation)
                        .map_err(|e| e.to_string()),
                    None => Err(format!("run {} is not loaded", seed.run)),
                };
                match body {
                    Ok(body) => {
                        if self.body_logged.insert(seed.run.clone()) {
                            self.log(&seed.run, Some(server), &format!("img_gen request: {}", redacted(&body)));
                        }
                        self.log(&seed.run, Some(server), &format!("seed {}: sent", seed.seed));
                        self.send(server, Cmd::Submit { ticket, body })
                    }
                    Err(error) => self.fail_seed(ticket, &error),
                }
            }
        }
    }

    fn send(&mut self, server: ServerIdx, cmd: Cmd) {
        let _ = self.workers[server].send(cmd);
    }

    fn handle(&mut self, msg: Msg) {
        let now = Instant::now();
        match msg {
            Msg::Queue(run) => {
                self.queue(&run);
                self.relist = true;
            }
            Msg::Order(order) => self.sched.order = order,
            Msg::Cancel(run, how) => {
                let (dropped, in_flight) = self.sched.cancel_run(&run);
                let mut seeds: Vec<u64> = dropped.iter().map(|(_, s)| *s).collect();
                let kept = in_flight.iter().filter(|(_, t)| self.generating.contains(t)).count();
                self.log(
                    &run,
                    None,
                    &match how {
                        Stop::Now => format!("Stop now: {} waiting dropped, {} in flight cancelled", seeds.len(), in_flight.len()),
                        Stop::Finish => format!(
                            "Finish & stop: {} waiting dropped, {} sent but not generating cancelled, {kept} generating kept",
                            seeds.len(),
                            in_flight.len() - kept
                        ),
                    },
                );
                for (server, ticket) in in_flight {
                    // finishing: a job generating finishes and is kept
                    if how == Stop::Finish && self.generating.contains(&ticket) {
                        continue;
                    }
                    // let go now, not when its server says the job ended: one
                    // that never does (or a worker busy loading) would hold
                    // the run up for good
                    if let Some(seed) = self.sched.finished(ticket, Outcome::Dropped) {
                        seeds.push(seed.seed);
                    }
                    self.generating.remove(&ticket);
                    self.send(server, Cmd::Cancel { ticket });
                }
                self.record_cancelled(&run, &seeds);
                self.check_done(&run);
            }
            Msg::Report(s, report) => match report {
                Report::Probed(r) => self.sched.probed(s, r.map(|st| st.loaded()).map_err(drop), now),
                Report::Listed { round, loadable } => {
                    self.sched.set_loadable(s, loadable);
                    self.listed[s] = Some(round.max(self.listed[s].unwrap_or(0)));
                }
                Report::Generating { ticket } => {
                    if self.sched.ticket(ticket).is_some() {
                        self.generating.insert(ticket);
                        self.log_ticket(ticket, s, "generating");
                    }
                }
                Report::Submitted { ticket, id } => {
                    self.log_ticket(ticket, s, &format!("queued on the server as job {id}"))
                }
                Report::Warmed(r) => {
                    if let Some(m) = self.sched.model_of(s) {
                        let text = match &r {
                            Ok(()) => "warm-up image done".to_string(),
                            Err(e) => format!("warm-up image failed: {e}"),
                        };
                        self.log_model(m, Some(s), &text);
                    }
                    if let Err(e) = &r {
                        self.emit(Event::Problem(format!("{}: warm-up failed: {e}", self.servers[s].name)));
                    }
                    if let Some(model) = self.sched.warmed(s, r.is_ok(), now) {
                        self.emit(Event::Problem(format!(
                            "{}: gave up on {} after {} failed warm-ups; its runs wait (cancel them, or restart the server preset)",
                            self.servers[s].name,
                            model.recipe,
                            crate::scheduler::WARM_UP_TRIES
                        )));
                    }
                }
                Report::Waiting(w) => self.waiting[s] = w,
                Report::ShortOfRam(figures) => self.emit(Event::Problem(format!(
                    "{}: loading without enough free RAM after {} s: {figures}",
                    self.servers[s].name,
                    RAM_WAIT.as_secs()
                ))),
                Report::Loaded(r) => {
                    self.waiting[s] = None;
                    let target = match self.sched.server_state(s) {
                        ServerState::Loading { target } => target.clone(),
                        _ => return,
                    };
                    let took = self.load_started[s].take().map_or(0.0, |t| t.elapsed().as_secs_f32());
                    let text = match &r {
                        Ok(st) if st.loaded().as_ref() == Some(&target) => format!("loaded in {took:.1} s"),
                        Ok(st) => format!(
                            "the load answered with {} loaded",
                            st.loaded().map_or("nothing".to_string(), |m| model_text(&m))
                        ),
                        Err(e) => format!("loading failed after {took:.1} s: {e}"),
                    };
                    self.log_model(&target, Some(s), &text);
                    let result = match r {
                        Ok(st) if st.loaded().as_ref() == Some(&target) => LoadResult::Ok,
                        Ok(st) => LoadResult::Broken {
                            still_loaded: st.loaded(),
                            why: "it answered the load with another model loaded".into(),
                        },
                        Err(ApiError::InvalidModelConfig(m)) => {
                            self.emit(Event::Problem(format!(
                                "{}: can't load {}: {m}",
                                self.servers[s].name, target.recipe
                            )));
                            LoadResult::Broken { still_loaded: self.sched.model_of(s).cloned(), why: m }
                        }
                        Err(ApiError::ModelLoadFailed(m)) => {
                            self.emit(Event::Problem(format!(
                                "{}: loading {} failed: {m}",
                                self.servers[s].name, target.recipe
                            )));
                            LoadResult::Broken { still_loaded: None, why: format!("loading it failed: {m}") }
                        }
                        Err(_) => LoadResult::ServerError,
                    };
                    self.sched.loaded(s, result, now);
                }
                Report::SubmitFailed { ticket, error } => {
                    self.log_ticket(ticket, s, &format!("the server refused it: {error}"));
                    match error {
                        ApiError::ModelNotLoaded => {
                            self.sched.lost_model(s);
                            self.sched.finished(ticket, Outcome::Retry);
                        }
                        ApiError::BadRequest(m) => self.fail_seed(ticket, &m),
                        _ => {
                            self.sched.finished(ticket, Outcome::Retry);
                            self.sched.server_failed(s, now);
                        }
                    }
                }
                Report::Progress { ticket, step, total, preview } => {
                    if let Some((server, seed)) = self.sched.ticket(ticket) {
                        let e = Event::Progress {
                            run: seed.run.clone(),
                            rev: seed.rev,
                            seed: seed.seed,
                            server,
                            step,
                            total,
                            preview,
                        };
                        self.emit(e);
                    }
                }
                Report::Finished { ticket, job, step_s } => {
                    self.generating.remove(&ticket);
                    self.finished(s, ticket, *job, step_s)
                }
                Report::Gone { ticket } => {
                    self.log_ticket(
                        ticket,
                        s,
                        "its job is gone from the server (purged, or it restarted): queued again",
                    );
                    self.generating.remove(&ticket);
                    self.sched.finished(ticket, Outcome::Retry);
                }
                Report::NotSubmitted { ticket } => {
                    self.log_ticket(ticket, s, "cancelled before it was sent");
                    if let Some(seed) = self.sched.finished(ticket, Outcome::Dropped) {
                        self.record_cancelled(&seed.run, &[seed.seed]);
                        self.check_done(&seed.run);
                    }
                }
                Report::PollFailed(e) => {
                    self.log_server(s, &format!("asking for its jobs failed: {e}; their seeds go back to the queue"));
                    self.emit(Event::Problem(format!("{}: {e}", self.servers[s].name)));
                    self.sched.server_failed(s, now);
                }
            },
            Msg::Shutdown => {}
        }
    }

    fn queue(&mut self, run_id: &Id) {
        let found = {
            let lib = self.lib();
            lib.find_run(run_id).and_then(|found| Ok((found, lib.pending_seeds(run_id)?)))
        };
        let ((project, run), pending) = match found {
            Ok(x) => x,
            Err(e) => return self.emit(Event::Problem(format!("can't queue run {run_id}: {e}"))),
        };
        let mut queued = std::collections::HashSet::new();
        for i in 0..self.servers.len() {
            queued.extend(self.sched.in_flight(i).filter(|(_, r)| &r.run == run_id).map(|(_, r)| r.seed));
        }
        let fresh: Vec<_> = pending.into_iter().filter(|(_, s)| !queued.contains(s)).collect();
        let model = ModelKey::from(&run.model);
        self.logs.insert(run_id.clone(), run_log(&project, run_id));
        if !fresh.is_empty() {
            let seeds = match (fresh.first(), fresh.last()) {
                (Some(a), Some(b)) if fresh.len() > 1 => format!("{} seeds ({}…{})", fresh.len(), a.1, b.1),
                (Some(a), _) => format!("seed {}", a.1),
                _ => String::new(),
            };
            let text = format!("queued {seeds} of {}", model_text(&model));
            self.log(run_id, None, &text);
        }
        // a sweep's runs go together (sweep labels are unique per project)
        let group = run.sweep.as_ref().map(|t| format!("{}/{}", project.display(), t.name));
        self.runs.insert(run_id.clone(), run);
        self.sched.set_pending(run_id, model, fresh);
        if let Some(g) = group {
            self.sched.set_group(run_id, g);
        }
        self.check_done(run_id);
    }

    /// Records seeds as cancelled in `run.rsn` (again is harmless).
    fn record_cancelled(&mut self, run: &Id, seeds: &[u64]) {
        if let Err(e) = self.lib().cancel_seeds(run, seeds) {
            self.emit(Event::Problem(format!("recording cancelled seeds of {run}: {e}")));
        }
    }

    fn fail_seed(&mut self, ticket: Ticket, error: &str) {
        let Some(seed) = self.sched.finished(ticket, Outcome::Dropped) else { return };
        self.log(&seed.run, None, &format!("seed {} failed: {error}", seed.seed));
        if let Err(e) = self.lib().record_failure(&seed.run, seed.rev, seed.seed, error) {
            self.emit(Event::Problem(format!("recording failed seed: {e}")));
        }
        self.emit(Event::SeedFailed { run: seed.run.clone(), rev: seed.rev, seed: seed.seed, error: error.into() });
        self.check_done(&seed.run);
    }

    fn finished(&mut self, s: ServerIdx, ticket: Ticket, job: Job, step_s: f32) {
        match job.status {
            JobStatus::Completed => {}
            JobStatus::Cancelled => {
                self.log_ticket(ticket, s, "cancelled by the server");
                if let Some(seed) = self.sched.finished(ticket, Outcome::Dropped) {
                    self.record_cancelled(&seed.run, &[seed.seed]);
                    self.check_done(&seed.run);
                }
                return;
            }
            _ => {
                let error = job.error.map(|e| format!("{}: {}", e.code, e.message)).unwrap_or_else(|| "failed".into());
                return self.fail_seed(ticket, &error);
            }
        }
        let Some((_, seed)) = self.sched.ticket(ticket) else { return };
        let seed = seed.clone();
        let Some(run) = self.runs.get(&seed.run) else { return };
        let written = job.image().map_err(|e| e.to_string()).and_then(|png| {
            let p = Provenance {
                device: self.servers[s].device.clone(),
                elapsed_s: job.elapsed_s().unwrap_or(0.0) as f32,
                queued_s: job.started.map_or(0.0, |t| (t - job.created).max(0) as f32 / 1000.0),
                step_s,
                sdcpp: self.servers[s].client.software().unwrap_or_else(|| self.opts.sdcpp.clone()),
            };
            let recipe = request::recipe(run, seed.seed, &p);
            self.lib().write_output(&seed.run, seed.rev, seed.seed, &png, &recipe).map_err(|e| e.to_string())
        });
        match written {
            Ok((image, link)) => {
                self.log_ticket(ticket, s, &format!("done in {:.1} s", job.elapsed_s().unwrap_or(0.0)));
                self.sched.finished(ticket, Outcome::Done);
                self.emit(Event::Output { run: seed.run.clone(), rev: seed.rev, seed: seed.seed, image, link });
                self.check_done(&seed.run);
            }
            Err(e) => self.fail_seed(ticket, &format!("storing the result: {e}")),
        }
    }

    fn check_done(&mut self, run: &Id) {
        if self.runs.contains_key(run) && self.sched.run_counts(run).is_none() {
            self.log(run, None, "nothing left to generate");
            self.runs.remove(run);
            self.logs.remove(run);
            self.body_logged.remove(run);
            self.emit(Event::RunDone { run: run.clone() });
        }
    }
}

// ------------------------------------------------------------------ worker

/// A submitted job the worker polls.
struct Polled {
    ticket: Ticket,
    id: String,
    /// `(pass, step)` of the last preview reported.
    last: Option<(u32, u32)>,
    /// Seen generating (not waiting in the server's queue).
    generating: bool,
    /// When the first and the latest step of the first pass were seen, and
    /// which steps (for seconds per step).
    first_step: Option<(Instant, u32, u32)>,
    latest_step: Option<(Instant, u32)>,
}

impl Polled {
    fn new(ticket: Ticket, id: String) -> Polled {
        Polled { ticket, id, last: None, generating: false, first_step: None, latest_step: None }
    }

    fn step_seen(&mut self, pass: u32, step: u32) {
        let now = Instant::now();
        match self.first_step {
            None => self.first_step = Some((now, pass, step)),
            Some((_, p, _)) if p == pass => self.latest_step = Some((now, step)),
            Some(_) => {}
        }
    }

    /// Seconds per step between the first and latest step seen (0: too
    /// few seen to tell).
    fn step_s(&self) -> f32 {
        match (self.first_step, self.latest_step) {
            (Some((t0, _, s0)), Some((t1, s1))) if s1 > s0 => (t1 - t0).as_secs_f32() / (s1 - s0) as f32,
            _ => 0.0,
        }
    }
}

/// One model load at a time among the servers on this machine: two at
/// once read and pin tens of GB together, which stalled the desktop.
static LOCAL_LOADS: Mutex<()> = Mutex::new(());

/// The RAM a load leaves free for everything else (GiB).
const RAM_HEADROOM_GIB: f64 = 8.0;

/// How long a load waits for free RAM before it goes ahead anyway.
const RAM_WAIT: Duration = Duration::from_secs(2);

/// Loads `model`. On this machine: after any other server's load, and once
/// the RAM it pins is free with [`RAM_HEADROOM_GIB`] to spare, or
/// [`RAM_WAIT`] has passed (said in a [`Report::ShortOfRam`]); not at all
/// with `skip_ram_check`. Waiting for RAM doesn't hold up the other
/// servers' loads. `None`: the engine has gone.
fn load(
    client: &Client,
    model: &ModelKey,
    skip_ram_check: &AtomicBool,
    report: &impl Fn(Report) -> bool,
) -> Option<Result<ModelStatus, ApiError>> {
    if !client.is_local() {
        return Some(client.load_model(model));
    }
    const GIB: f64 = (1u64 << 30) as f64;
    let mut shown: Option<String> = None;
    let show = |shown: &mut Option<String>, text: String| {
        shown.replace(text.clone()).as_ref() == Some(&text) || report(Report::Waiting(Some(text)))
    };
    let since = Instant::now();
    let _one = loop {
        let one = match LOCAL_LOADS.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => {
                if !show(&mut shown, "waiting for another server's load".into()) {
                    return None;
                }
                LOCAL_LOADS.lock().unwrap_or_else(|e| e.into_inner())
            }
        };
        let need = client.load_ram_need(model) as f64 / GIB;
        if need <= 0.0 || skip_ram_check.load(Ordering::Relaxed) {
            break one;
        }
        if let Some(total) = crate::tiers::total_ram_gib()
            && need + RAM_HEADROOM_GIB > total
        {
            return Some(Err(ApiError::ModelLoadFailed(format!(
                "it needs {need:.0} GiB of RAM for its offloaded weights; this machine has {total:.0} GiB"
            ))));
        }
        let free = match crate::tiers::available_ram_gib() {
            Some(free) if free < need + RAM_HEADROOM_GIB => free,
            _ => break one,
        };
        let figures = format!(
            "{free:.1} of {:.1} GiB free ({need:.1} GiB to load + {RAM_HEADROOM_GIB:.0} to spare)",
            need + RAM_HEADROOM_GIB
        );
        if since.elapsed() >= RAM_WAIT {
            if !report(Report::ShortOfRam(figures)) {
                return None;
            }
            break one;
        }
        drop(one);
        if !show(&mut shown, format!("waiting for RAM: {figures}")) {
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    report(Report::Waiting(None));
    Some(client.load_model(model))
}

fn worker(
    server: ServerIdx,
    client: Client,
    cmds: Receiver<Cmd>,
    reports: Sender<Msg>,
    poll: Duration,
    skip_ram_check: Arc<AtomicBool>,
) {
    let report = |r: Report| reports.send(Msg::Report(server, r)).is_ok();
    let mut jobs: Vec<Polled> = vec![];
    let mut cancelled_early: Vec<Ticket> = vec![];
    loop {
        let cmd = match cmds.recv_timeout(if jobs.is_empty() { Duration::from_secs(3600) } else { poll }) {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let alive = match cmd {
            None => true,
            // the list first: a server is picked once it's probed
            Some(Cmd::Probe { round }) => {
                report(Report::Listed { round, loadable: client.loadable().ok() })
                    && report(Report::Probed(client.model_status()))
            }
            Some(Cmd::List { round }) => report(Report::Listed { round, loadable: client.loadable().ok() }),
            Some(Cmd::Load(model)) => match load(&client, &model, &skip_ram_check, &report) {
                Some(r) => report(Report::Loaded(r)),
                None => false,
            },
            Some(Cmd::WarmUp(body)) => report(Report::Warmed(warm_up(&client, &body, poll))),
            Some(Cmd::Submit { ticket, body }) => {
                if let Some(p) = cancelled_early.iter().position(|t| *t == ticket) {
                    cancelled_early.remove(p);
                    report(Report::NotSubmitted { ticket })
                } else {
                    match client.img_gen(&body) {
                        Ok(id) => {
                            jobs.push(Polled::new(ticket, id.clone()));
                            report(Report::Submitted { ticket, id })
                        }
                        Err(error) => report(Report::SubmitFailed { ticket, error }),
                    }
                }
            }
            Some(Cmd::Cancel { ticket }) => {
                match jobs.iter().position(|j| j.ticket == ticket) {
                    Some(i) => {
                        let j = jobs.remove(i);
                        let _ = client.cancel(&j.id);
                    }
                    None => cancelled_early.push(ticket),
                }
                true
            }
        };
        if !alive {
            return;
        }
        let mut i = 0;
        while i < jobs.len() {
            let ticket = jobs[i].ticket;
            match client.job(&jobs[i].id) {
                Ok(job) if job.status.is_terminal() => {
                    let step_s = jobs.remove(i).step_s();
                    if !report(Report::Finished { ticket, job: Box::new(job), step_s }) {
                        return;
                    }
                    continue;
                }
                Ok(job) => {
                    let j = &mut jobs[i];
                    if !j.generating && job.status == JobStatus::Generating {
                        j.generating = true;
                        if !report(Report::Generating { ticket }) {
                            return;
                        }
                    }
                    if let Some(p) = &job.preview
                        && j.last != Some((p.pass, p.step))
                    {
                        j.last = Some((p.pass, p.step));
                        j.step_seen(p.pass, p.step);
                        let preview = base64::engine::general_purpose::STANDARD
                            .decode(&p.b64_json)
                            .ok()
                            .filter(|b| !b.is_empty());
                        let _ = report(Report::Progress { ticket, step: p.step, total: p.total_steps, preview });
                    }
                }
                Err(ApiError::JobGone) => {
                    jobs.remove(i);
                    let _ = report(Report::Gone { ticket });
                    continue;
                }
                Err(e) => {
                    jobs.clear();
                    let _ = report(Report::PollFailed(e));
                    break;
                }
            }
            i += 1;
        }
    }
}

/// A model as logged: its preset and files.
fn model_text(m: &ModelKey) -> String {
    let files: Vec<&str> = m.models.values().map(String::as_str).collect();
    format!("{} ({})", m.recipe, files.join(", "))
}

/// A request body as logged: base64 data (no spaces, long) as its length.
fn redacted(v: &Value) -> Value {
    match v {
        Value::String(s) if s.len() > 200 && !s.contains(' ') => Value::String(format!("<{} characters>", s.len())),
        Value::Array(a) => Value::Array(a.iter().map(redacted).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), redacted(v))).collect()),
        v => v.clone(),
    }
}

fn warm_up(client: &Client, body: &Value, poll: Duration) -> Result<(), ApiError> {
    let id = client.img_gen(body)?;
    loop {
        let job = client.job(&id)?;
        match job.status {
            JobStatus::Completed => return Ok(()),
            JobStatus::Failed | JobStatus::Cancelled => {
                let msg = job.error.as_ref().map(|e| e.message.clone()).unwrap_or_default();
                return Err(ApiError::BadRequest(format!("warm-up job {}: {msg}", job.status_name())));
            }
            _ => std::thread::sleep(poll),
        }
    }
}

impl Job {
    fn status_name(&self) -> &'static str {
        match self.status {
            JobStatus::Queued => "queued",
            JobStatus::Generating => "generating",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
}
