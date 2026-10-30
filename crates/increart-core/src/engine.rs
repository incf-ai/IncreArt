//! Runs generation: the [`Scheduler`] on a coordinator thread, one worker
//! thread per sd-server doing the HTTP, and finished images written into the
//! library. The GUI talks to it with [`Engine::queue_run`] /
//! [`Engine::cancel_run`] and reads [`Event`]s.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value;

use crate::id::Id;
use crate::library::Library;
use crate::model::Run;
use crate::request::{self, GenOptions, Provenance};
use crate::scheduler::{Action, LoadResult, Order, Outcome, Scheduler, ServerIdx, ServerState, Ticket};
use crate::sdserver::{ApiError, Client, Job, JobStatus, ModelKey, ModelStatus};

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
}

/// A server's last reported state, model, idleness and runs in flight.
type Shown = (String, Option<ModelKey>, bool, Vec<Id>);

/// How a cancel treats the images being generated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// They finish and are kept; jobs still waiting are cancelled.
    Finish,
    /// They are cancelled too (sd-server stops them at its next checkpoint).
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
    Probe,
    WarmUp(Value),
    Load(ModelKey),
    Submit {
        ticket: Ticket,
        body: Value,
    },
    /// `waiting_only`: leave the job be if it is generating already.
    Cancel {
        ticket: Ticket,
        waiting_only: bool,
    },
}

enum Report {
    Probed(Result<ModelStatus, ApiError>),
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
}

pub struct Engine {
    tx: Sender<Msg>,
    events: Receiver<Event>,
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
        let workers = servers
            .iter()
            .enumerate()
            .map(|(i, ep)| {
                let (ctx, crx) = mpsc::channel();
                let client = ep.client.clone();
                let report = tx.clone();
                let poll = opts.poll_interval;
                std::thread::Builder::new()
                    .name(format!("sd-server {}", ep.name))
                    .spawn(move || worker(i, client, crx, report, poll))
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
            events: etx,
            notify,
        };
        let thread = std::thread::Builder::new().name("engine".into()).spawn(move || co.run(rx)).expect("spawn engine");
        Engine { tx, events, thread: Some(thread) }
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

    fn lib(&self) -> std::sync::MutexGuard<'_, Library> {
        self.lib.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn step(&mut self) {
        let now = Instant::now();
        for action in self.sched.poll(now) {
            self.perform(action);
        }
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

    fn perform(&mut self, action: Action) {
        let now = Instant::now();
        match action {
            Action::Probe { server } => self.send(server, Cmd::Probe),
            Action::WarmUp { server } if self.opts.warm_up => self.send(server, Cmd::WarmUp(request::warmup_body())),
            Action::WarmUp { server } => {
                self.sched.warmed(server, true, now);
            }
            Action::Load { server, model } => self.send(server, Cmd::Load(model)),
            Action::Cancel { server, ticket } => self.send(server, Cmd::Cancel { ticket, waiting_only: false }),
            Action::Submit { server, ticket, seed } => {
                let body = match self.runs.get(&seed.run) {
                    Some(run) => request::img_gen_body(&self.lib(), run, seed.seed, &self.opts.generation)
                        .map_err(|e| e.to_string()),
                    None => Err(format!("run {} is not loaded", seed.run)),
                };
                match body {
                    Ok(body) => self.send(server, Cmd::Submit { ticket, body }),
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
            Msg::Queue(run) => self.queue(&run),
            Msg::Order(order) => self.sched.order = order,
            Msg::Cancel(run, how) => {
                let (dropped, actions) = self.sched.cancel_run(&run);
                let mut seeds: Vec<u64> = dropped.iter().map(|(_, s)| *s).collect();
                // finishing: in-flight seeds are recorded as their jobs end cancelled
                if how == Stop::Now {
                    for i in 0..self.servers.len() {
                        seeds.extend(self.sched.in_flight(i).filter(|(_, r)| r.run == run).map(|(_, r)| r.seed));
                    }
                }
                self.record_cancelled(&run, &seeds);
                for a in actions {
                    match a {
                        Action::Cancel { server, ticket } => {
                            self.send(server, Cmd::Cancel { ticket, waiting_only: how == Stop::Finish })
                        }
                        a => self.perform(a),
                    }
                }
                self.check_done(&run);
            }
            Msg::Report(s, report) => match report {
                Report::Probed(r) => self.sched.probed(s, r.map(|st| st.loaded()).map_err(drop), now),
                Report::Warmed(r) => {
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
                Report::Loaded(r) => {
                    self.waiting[s] = None;
                    let target = match self.sched.server_state(s) {
                        ServerState::Loading { target } => target.clone(),
                        _ => return,
                    };
                    let result = match r {
                        Ok(st) if st.loaded().as_ref() == Some(&target) => LoadResult::Ok,
                        Ok(st) => LoadResult::Broken { still_loaded: st.loaded() },
                        Err(ApiError::InvalidModelConfig(m)) => {
                            self.emit(Event::Problem(format!(
                                "{}: can't load {}: {m}",
                                self.servers[s].name, target.recipe
                            )));
                            LoadResult::Broken { still_loaded: self.sched.model_of(s).cloned() }
                        }
                        Err(ApiError::ModelLoadFailed(m)) => {
                            self.emit(Event::Problem(format!(
                                "{}: loading {} failed: {m}",
                                self.servers[s].name, target.recipe
                            )));
                            LoadResult::Broken { still_loaded: None }
                        }
                        Err(_) => LoadResult::ServerError,
                    };
                    self.sched.loaded(s, result, now);
                }
                Report::SubmitFailed { ticket, error } => match error {
                    ApiError::ModelNotLoaded => {
                        self.sched.lost_model(s);
                        self.sched.finished(ticket, Outcome::Retry);
                    }
                    ApiError::BadRequest(m) => self.fail_seed(ticket, &m),
                    _ => {
                        self.sched.finished(ticket, Outcome::Retry);
                        self.sched.server_failed(s, now);
                    }
                },
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
                Report::Finished { ticket, job, step_s } => self.finished(s, ticket, *job, step_s),
                Report::Gone { ticket } => {
                    self.sched.finished(ticket, Outcome::Retry);
                }
                Report::NotSubmitted { ticket } => {
                    if let Some(seed) = self.sched.finished(ticket, Outcome::Dropped) {
                        self.record_cancelled(&seed.run, &[seed.seed]);
                        self.check_done(&seed.run);
                    }
                }
                Report::PollFailed(e) => {
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
                self.sched.finished(ticket, Outcome::Done);
                self.emit(Event::Output { run: seed.run.clone(), rev: seed.rev, seed: seed.seed, image, link });
                self.check_done(&seed.run);
            }
            Err(e) => self.fail_seed(ticket, &format!("storing the result: {e}")),
        }
    }

    fn check_done(&mut self, run: &Id) {
        if self.runs.contains_key(run) && self.sched.run_counts(run).is_none() {
            self.runs.remove(run);
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

/// Loads `model`. On this machine: after any other server's load, and once
/// the RAM it pins is free with [`RAM_HEADROOM_GIB`] to spare. `None`: the
/// engine has gone.
fn load(client: &Client, model: &ModelKey, report: &impl Fn(Report) -> bool) -> Option<Result<ModelStatus, ApiError>> {
    if !client.is_local() {
        return Some(client.load_model(model));
    }
    let _one = match LOCAL_LOADS.try_lock() {
        Ok(g) => g,
        Err(_) => {
            if !report(Report::Waiting(Some("waiting for another server's load".into()))) {
                return None;
            }
            LOCAL_LOADS.lock().unwrap_or_else(|e| e.into_inner())
        }
    };
    const GIB: f64 = (1u64 << 30) as f64;
    let need = client.load_ram_need(model) as f64 / GIB;
    if need > 0.0 {
        if let Some(total) = crate::tiers::total_ram_gib()
            && need + RAM_HEADROOM_GIB > total
        {
            return Some(Err(ApiError::ModelLoadFailed(format!(
                "it needs {need:.0} GiB of RAM for its offloaded weights; this machine has {total:.0} GiB"
            ))));
        }
        let mut shown = None;
        while let Some(free) = crate::tiers::available_ram_gib()
            && free < need + RAM_HEADROOM_GIB
        {
            let text = format!("waiting for RAM: needs {need:.0} GiB, {free:.0} GiB free");
            if shown.as_ref() != Some(&text) {
                if !report(Report::Waiting(Some(text.clone()))) {
                    return None;
                }
                shown = Some(text);
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    report(Report::Waiting(None));
    Some(client.load_model(model))
}

fn worker(server: ServerIdx, client: Client, cmds: Receiver<Cmd>, reports: Sender<Msg>, poll: Duration) {
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
            Some(Cmd::Probe) => report(Report::Probed(client.model_status())),
            Some(Cmd::Load(model)) => match load(&client, &model, &report) {
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
                            jobs.push(Polled::new(ticket, id));
                            true
                        }
                        Err(error) => report(Report::SubmitFailed { ticket, error }),
                    }
                }
            }
            Some(Cmd::Cancel { ticket, waiting_only }) => {
                match jobs.iter().find(|j| j.ticket == ticket) {
                    Some(j) if waiting_only && j.generating => {}
                    Some(j) => {
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
                    j.generating |= job.status == JobStatus::Generating;
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
