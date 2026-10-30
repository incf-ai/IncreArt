//! The generation scheduler (DESIGN.md, Scheduling) as a pure state machine:
//! events in, [`Action`]s out, no I/O or clocks of its own. The engine
//! carries the actions out and reports back.
//!
//! - Round-robin: a free server takes the next seed of the unfinished run with
//!   the fewest seeds dispatched since it was queued.
//! - Shallow submission: at most `max_in_flight` jobs per server.
//! - Order: [`Order::Spread`] (the default) is the round-robin above;
//!   [`Order::BySweep`] is the round-robin within the earliest queued group
//!   (a sweep's runs, or a lone run) with seeds left, so sweeps finish one
//!   after another; [`Order::Sequential`] gives every free slot to the
//!   earliest queued run with seeds left, so runs finish one after another.
//! - Ties: among equally due runs, the one that got a seed longest ago (so a
//!   round goes through the runs in queue order every time).
//! - Model switching: work for a model no server has makes one server
//!   *drain* (no new jobs) and then load it, but never the last server of a
//!   model that still has work; a server with no work for its model switches
//!   to the most due one.
//! - Back-off: a failing server's in-flight seeds go back to the pool and it
//!   is probed again after an exponentially growing delay.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::id::Id;
use crate::sdserver::ModelKey;

pub type ServerIdx = usize;
pub type Ticket = u64;

/// One seed of one revision of a run.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SeedRef {
    pub run: Id,
    pub rev: u32,
    pub seed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Ask the server what it has loaded; answer with [`Scheduler::probed`].
    Probe { server: ServerIdx },
    /// Run a small throwaway job; answer with [`Scheduler::warmed`].
    WarmUp { server: ServerIdx },
    /// Load a model (the server has drained); answer with [`Scheduler::loaded`].
    Load { server: ServerIdx, model: ModelKey },
    /// Submit one seed; answer with [`Scheduler::finished`].
    Submit { server: ServerIdx, ticket: Ticket, seed: SeedRef },
    /// Cancel a submitted seed (it still gets a [`Scheduler::finished`]).
    Cancel { server: ServerIdx, ticket: Ticket },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerState {
    /// Not probed yet.
    Unknown,
    Probing,
    /// Has a model but hasn't run its warm-up job.
    NeedsWarmUp,
    WarmingUp,
    Ready,
    /// Takes no new jobs; loads `target` once its in-flight jobs finish.
    Draining {
        target: ModelKey,
    },
    Loading {
        target: ModelKey,
    },
    Backoff {
        until: Instant,
    },
}

/// How a submitted seed ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Image stored.
    Done,
    /// Failed for good (bad request, generation error) or cancelled.
    Dropped,
    /// Not generated, try again (job purged, model not loaded, …).
    Retry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadResult {
    Ok,
    /// The model can't be loaded (invalid config or load failure): runs
    /// that need it are stuck until it is fixed. `still_loaded` is what the
    /// server has afterwards (the old model for an invalid config, none for
    /// a failed load).
    Broken {
        still_loaded: Option<ModelKey>,
    },
    /// The server failed (connection lost, …): back off.
    ServerError,
}

/// How the queued runs share the servers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Order {
    /// Round-robin: every run gets a seed in turn.
    #[default]
    Spread,
    /// Round-robin within one group (a sweep, or a lone run) at a time, in
    /// queue order, to completion.
    BySweep,
    /// One run at a time, in queue order, to completion.
    Sequential,
}

/// Warm-ups of one model that may fail in a row before it is given up on.
pub const WARM_UP_TRIES: u32 = 3;

#[derive(Debug)]
struct Server {
    state: ServerState,
    loaded: Option<ModelKey>,
    in_flight: Vec<(Ticket, SeedRef)>,
    failures: u32,
}

#[derive(Debug)]
struct RunQueue {
    id: Id,
    /// Runs of one sweep share it ([`Order::BySweep`]); else the run's id.
    group: String,
    model: ModelKey,
    pending: VecDeque<(u32, u64)>,
    dispatched: u64,
    /// The ticket of its last dispatched seed (`None`: none yet).
    last: Option<Ticket>,
    in_flight: usize,
}

#[derive(Debug)]
pub struct Scheduler {
    servers: Vec<Server>,
    runs: Vec<RunQueue>,
    broken: HashSet<ModelKey>,
    /// Warm-ups failed in a row, by model (a server that crashes on it
    /// would otherwise be restarted and reloaded forever).
    warm_failures: HashMap<ModelKey, u32>,
    next_ticket: Ticket,
    pub max_in_flight: usize,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub order: Order,
}

impl Scheduler {
    pub fn new(servers: usize) -> Scheduler {
        Scheduler {
            servers: (0..servers)
                .map(|_| Server { state: ServerState::Unknown, loaded: None, in_flight: vec![], failures: 0 })
                .collect(),
            runs: vec![],
            broken: HashSet::new(),
            warm_failures: HashMap::new(),
            next_ticket: 1,
            max_in_flight: 2,
            backoff_base: Duration::from_secs(2),
            backoff_max: Duration::from_secs(60),
            order: Order::Spread,
        }
    }

    // ---------------------------------------------------------- queries

    pub fn server_state(&self, s: ServerIdx) -> &ServerState {
        &self.servers[s].state
    }

    pub fn model_of(&self, s: ServerIdx) -> Option<&ModelKey> {
        self.servers[s].loaded.as_ref()
    }

    pub fn in_flight(&self, s: ServerIdx) -> impl Iterator<Item = (Ticket, &SeedRef)> {
        self.servers[s].in_flight.iter().map(|(t, r)| (*t, r))
    }

    /// The seed a ticket was issued for, while it is in flight.
    pub fn ticket(&self, ticket: Ticket) -> Option<(ServerIdx, &SeedRef)> {
        self.servers
            .iter()
            .enumerate()
            .find_map(|(i, s)| s.in_flight.iter().find(|(t, _)| *t == ticket).map(|(_, r)| (i, r)))
    }

    /// Seeds of a run not yet dispatched, and in flight.
    pub fn run_counts(&self, run: &Id) -> Option<(usize, usize)> {
        self.runs.iter().find(|r| &r.id == run).map(|r| (r.pending.len(), r.in_flight))
    }

    /// Runs with work left, oldest-queued first.
    pub fn runs(&self) -> impl Iterator<Item = &Id> {
        self.runs.iter().map(|r| &r.id)
    }

    /// Runs whose model failed to load: they wait until it is fixed (and
    /// [`Self::retry_model`] is called) or they are cancelled.
    pub fn stuck_runs(&self) -> Vec<&Id> {
        self.runs.iter().filter(|r| self.broken.contains(&r.model)).map(|r| &r.id).collect()
    }

    pub fn is_idle(&self) -> bool {
        self.runs.is_empty()
    }

    // ---------------------------------------------------------- work in

    /// Queues seeds of a run (a new run, or more seeds for one already queued).
    pub fn add_seeds(&mut self, run: &Id, model: ModelKey, seeds: impl IntoIterator<Item = (u32, u64)>) {
        let pos = match self.runs.iter().position(|r| &r.id == run) {
            Some(p) => p,
            None => {
                self.runs.push(RunQueue {
                    id: run.clone(),
                    group: run.to_string(),
                    model: model.clone(),
                    pending: VecDeque::new(),
                    dispatched: 0,
                    last: None,
                    in_flight: 0,
                });
                self.runs.len() - 1
            }
        };
        let r = &mut self.runs[pos];
        r.model = model;
        r.pending.extend(seeds);
        if r.pending.is_empty() && r.in_flight == 0 {
            self.runs.remove(pos);
        }
    }

    /// Replaces a run's pending seeds (keeping its round-robin count), e.g.
    /// after seeds were added to it. Queues the run if it is new.
    pub fn set_pending(&mut self, run: &Id, model: ModelKey, seeds: impl IntoIterator<Item = (u32, u64)>) {
        if let Some(r) = self.runs.iter_mut().find(|r| &r.id == run) {
            r.pending.clear();
        }
        self.add_seeds(run, model, seeds);
    }

    /// Puts a queued run in a group: the runs of one sweep share one.
    pub fn set_group(&mut self, run: &Id, group: impl Into<String>) {
        if let Some(r) = self.runs.iter_mut().find(|r| &r.id == run) {
            r.group = group.into();
        }
    }

    /// Drops a run's pending seeds and cancels its in-flight ones. Returns
    /// the dropped seeds and the cancel actions.
    pub fn cancel_run(&mut self, run: &Id) -> (Vec<(u32, u64)>, Vec<Action>) {
        let Some(pos) = self.runs.iter().position(|r| &r.id == run) else { return (vec![], vec![]) };
        let dropped: Vec<_> = self.runs[pos].pending.drain(..).collect();
        let mut actions = vec![];
        for (i, s) in self.servers.iter().enumerate() {
            for (t, r) in &s.in_flight {
                if &r.run == run {
                    actions.push(Action::Cancel { server: i, ticket: *t });
                }
            }
        }
        if self.runs[pos].in_flight == 0 {
            self.runs.remove(pos);
        }
        (dropped, actions)
    }

    /// Lets runs needing a model that failed to load try again.
    pub fn retry_model(&mut self, model: &ModelKey) {
        self.broken.remove(model);
    }

    // ---------------------------------------------------------- reports

    pub fn probed(&mut self, s: ServerIdx, result: Result<Option<ModelKey>, ()>, now: Instant) {
        if self.servers[s].state != ServerState::Probing {
            return;
        }
        match result {
            Ok(loaded) => {
                let srv = &mut self.servers[s];
                srv.state = if loaded.is_some() { ServerState::NeedsWarmUp } else { ServerState::Ready };
                srv.loaded = loaded;
            }
            Err(()) => self.back_off(s, now),
        }
    }

    /// A warm-up ended. Returns the model given up on, at its
    /// [`WARM_UP_TRIES`]th failure in a row: it is broken like a model that
    /// failed to load.
    pub fn warmed(&mut self, s: ServerIdx, ok: bool, now: Instant) -> Option<ModelKey> {
        if self.servers[s].state != ServerState::WarmingUp {
            return None;
        }
        let model = self.servers[s].loaded.clone();
        if ok {
            self.servers[s].state = ServerState::Ready;
            self.servers[s].failures = 0;
            if let Some(m) = &model {
                self.warm_failures.remove(m);
            }
            return None;
        }
        self.back_off(s, now);
        let m = model?;
        let n = self.warm_failures.entry(m.clone()).or_default();
        *n += 1;
        if *n < WARM_UP_TRIES {
            return None;
        }
        self.warm_failures.remove(&m);
        self.broken.insert(m.clone());
        Some(m)
    }

    pub fn loaded(&mut self, s: ServerIdx, result: LoadResult, now: Instant) {
        let ServerState::Loading { target } = self.servers[s].state.clone() else { return };
        match result {
            LoadResult::Ok => {
                self.servers[s].loaded = Some(target);
                self.servers[s].state = ServerState::NeedsWarmUp;
            }
            LoadResult::Broken { still_loaded } => {
                self.broken.insert(target);
                self.servers[s].loaded = still_loaded;
                self.servers[s].state = ServerState::Ready;
            }
            LoadResult::ServerError => {
                self.servers[s].loaded = None;
                self.back_off(s, now);
            }
        }
    }

    /// A submitted seed ended. Unknown tickets (abandoned by a back-off) are
    /// ignored; returns the seed otherwise.
    pub fn finished(&mut self, ticket: Ticket, outcome: Outcome) -> Option<SeedRef> {
        let (s, idx) = self
            .servers
            .iter()
            .enumerate()
            .find_map(|(i, srv)| srv.in_flight.iter().position(|(t, _)| *t == ticket).map(|p| (i, p)))?;
        let (_, seed) = self.servers[s].in_flight.remove(idx);
        if outcome == Outcome::Done {
            self.servers[s].failures = 0;
        }
        self.settle(&seed, outcome == Outcome::Retry);
        Some(seed)
    }

    /// The server answered `model_not_loaded`: it has no model after all.
    pub fn lost_model(&mut self, s: ServerIdx) {
        self.servers[s].loaded = None;
    }

    /// The server failed (unreachable, 5xx, …): its in-flight seeds go back
    /// to their runs and it backs off.
    pub fn server_failed(&mut self, s: ServerIdx, now: Instant) {
        // In reverse, so the seeds go back to the front in their old order.
        for (_, seed) in std::mem::take(&mut self.servers[s].in_flight).into_iter().rev() {
            self.settle(&seed, true);
        }
        self.back_off(s, now);
    }

    fn settle(&mut self, seed: &SeedRef, retry: bool) {
        let Some(pos) = self.runs.iter().position(|r| r.id == seed.run) else { return };
        let r = &mut self.runs[pos];
        r.in_flight -= 1;
        if retry {
            r.pending.push_front((seed.rev, seed.seed));
            r.dispatched = r.dispatched.saturating_sub(1);
        }
        if r.pending.is_empty() && r.in_flight == 0 {
            self.runs.remove(pos);
        }
    }

    fn back_off(&mut self, s: ServerIdx, now: Instant) {
        let srv = &mut self.servers[s];
        srv.failures += 1;
        let delay = self.backoff_base.saturating_mul(1 << (srv.failures - 1).min(16)).min(self.backoff_max);
        srv.state = ServerState::Backoff { until: now + delay };
    }

    // ---------------------------------------------------------- decide

    /// Advances the state machine; returns what to do now. Call after every
    /// report and periodically (back-off expiry).
    pub fn poll(&mut self, now: Instant) -> Vec<Action> {
        let mut actions = vec![];
        self.retarget();
        for (i, srv) in self.servers.iter_mut().enumerate() {
            match &srv.state {
                ServerState::Unknown => {
                    srv.state = ServerState::Probing;
                    actions.push(Action::Probe { server: i });
                }
                ServerState::Backoff { until } if now >= *until => {
                    srv.state = ServerState::Probing;
                    actions.push(Action::Probe { server: i });
                }
                ServerState::NeedsWarmUp => {
                    srv.state = ServerState::WarmingUp;
                    actions.push(Action::WarmUp { server: i });
                }
                ServerState::Draining { target } if srv.in_flight.is_empty() => {
                    let target = target.clone();
                    srv.state = ServerState::Loading { target: target.clone() };
                    actions.push(Action::Load { server: i, model: target });
                }
                _ => {}
            }
        }
        for i in 0..self.servers.len() {
            while self.servers[i].state == ServerState::Ready && self.servers[i].in_flight.len() < self.max_in_flight {
                let Some(a) = self.dispatch(i) else { break };
                actions.push(a);
            }
        }
        actions
    }

    /// How due run `i` is (lowest first): its group's place in the queue
    /// ([`Order::BySweep`]) or its own ([`Order::Sequential`]), then the
    /// round-robin's fewest dispatched, longest ago.
    fn due(&self, i: usize, r: &RunQueue) -> (usize, u64, Option<Ticket>, usize) {
        match self.order {
            Order::Spread => (0, r.dispatched, r.last, i),
            Order::BySweep => {
                let first = self.runs.iter().position(|q| q.group == r.group).unwrap_or(i);
                (first, r.dispatched, r.last, i)
            }
            Order::Sequential => (i, 0, None, i),
        }
    }

    /// Models with pending (dispatchable) work, most due first.
    fn demand(&self) -> Vec<ModelKey> {
        let mut runs: Vec<_> = self
            .runs
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.pending.is_empty() && !self.broken.contains(&r.model))
            .collect();
        runs.sort_by_key(|(i, r)| self.due(*i, r));
        let mut out: Vec<ModelKey> = vec![];
        for (_, r) in runs {
            if !out.contains(&r.model) {
                out.push(r.model.clone());
            }
        }
        out
    }

    /// Servers that serve (or are about to serve) `model`.
    fn coverage(&self, model: &ModelKey) -> usize {
        self.servers
            .iter()
            .filter(|s| match &s.state {
                ServerState::Draining { target } | ServerState::Loading { target } => target == model,
                ServerState::Ready | ServerState::NeedsWarmUp | ServerState::WarmingUp => {
                    s.loaded.as_ref() == Some(model)
                }
                _ => false,
            })
            .count()
    }

    fn retarget(&mut self) {
        let demand = self.demand();
        if demand.is_empty() {
            return;
        }
        // An uncovered model takes one server that can be spared: idle first.
        for model in &demand {
            if self.coverage(model) > 0 {
                continue;
            }
            let spare = (0..self.servers.len())
                .filter(|&i| self.servers[i].state == ServerState::Ready)
                .filter(|&i| match &self.servers[i].loaded {
                    None => true,
                    Some(own) => !demand.contains(own) || self.coverage(own) >= 2,
                })
                .min_by_key(|&i| (self.servers[i].in_flight.len(), i));
            if let Some(i) = spare {
                self.servers[i].state = ServerState::Draining { target: model.clone() };
            }
        }
        // A server with nothing to do for its model helps with the most due
        // one that no server is loading: until that load is done it stays
        // free for another model's work, queued meanwhile (loads on a machine
        // go one at a time anyway).
        for i in 0..self.servers.len() {
            let srv = &self.servers[i];
            let idle = srv.state == ServerState::Ready && srv.loaded.as_ref().is_none_or(|m| !demand.contains(m));
            if !idle {
                continue;
            }
            let loading = |m: &ModelKey| {
                self.servers.iter().any(|s| {
                    matches!(&s.state, ServerState::Draining { target } | ServerState::Loading { target } if target == m)
                })
            };
            if let Some(m) = demand.iter().find(|m| !loading(m)) {
                self.servers[i].state = ServerState::Draining { target: m.clone() };
            }
        }
    }

    fn dispatch(&mut self, s: ServerIdx) -> Option<Action> {
        let model = self.servers[s].loaded.clone()?;
        let pos = self
            .runs
            .iter()
            .enumerate()
            .filter(|(_, r)| r.model == model && !r.pending.is_empty())
            .min_by_key(|(i, r)| self.due(*i, r))
            .map(|(i, _)| i)?;
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let r = &mut self.runs[pos];
        let (rev, seed) = r.pending.pop_front().unwrap();
        r.dispatched += 1;
        r.last = Some(ticket);
        r.in_flight += 1;
        let seed = SeedRef { run: r.id.clone(), rev, seed };
        let srv = &mut self.servers[s];
        srv.in_flight.push((ticket, seed.clone()));
        Some(Action::Submit { server: s, ticket, seed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(name: &str) -> ModelKey {
        ModelKey { recipe: name.into(), models: Default::default() }
    }

    fn run(n: u32) -> Id {
        Id::new(chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(), n)
    }

    /// A scheduler whose servers are up with `loaded` and warmed.
    fn up(loaded: &[Option<&str>]) -> (Scheduler, Instant) {
        let now = Instant::now();
        let mut s = Scheduler::new(loaded.len());
        let probes = s.poll(now);
        assert_eq!(probes.len(), loaded.len());
        for (i, m) in loaded.iter().enumerate() {
            s.probed(i, Ok(m.map(model)), now);
        }
        for a in s.poll(now) {
            let Action::WarmUp { server } = a else { panic!("{a:?}") };
            s.warmed(server, true, now);
        }
        (s, now)
    }

    fn submits(actions: &[Action]) -> Vec<(ServerIdx, Ticket, SeedRef)> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Submit { server, ticket, seed } => Some((*server, *ticket, seed.clone())),
                _ => None,
            })
            .collect()
    }

    fn seeds(n: u64) -> impl Iterator<Item = (u32, u64)> {
        (0..n).map(|s| (0, s))
    }

    #[test]
    fn shallow_submission_two_per_server() {
        let (mut s, now) = up(&[Some("a"), Some("a")]);
        s.add_seeds(&run(0), model("a"), seeds(10));
        let sub = submits(&s.poll(now));
        assert_eq!(sub.len(), 4);
        assert_eq!(sub.iter().filter(|(srv, ..)| *srv == 0).count(), 2);
        assert!(s.poll(now).is_empty(), "nothing more until something finishes");
        s.finished(sub[0].1, Outcome::Done);
        let next = submits(&s.poll(now));
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].2.seed, 4, "seeds go out in order");
    }

    #[test]
    fn round_robin_lets_a_new_run_in() {
        let (mut s, now) = up(&[Some("a")]);
        s.max_in_flight = 1;
        s.add_seeds(&run(0), model("a"), seeds(100));
        let mut order = vec![];
        let step = |s: &mut Scheduler| {
            let sub = submits(&s.poll(now));
            let (_, t, seed) = sub[0].clone();
            s.finished(t, Outcome::Done);
            seed.run
        };
        for _ in 0..3 {
            order.push(step(&mut s));
        }
        s.add_seeds(&run(1), model("a"), seeds(3));
        for _ in 0..4 {
            order.push(step(&mut s));
        }
        // run 1 has had 0 dispatched vs 3: it goes next, until it catches up.
        assert_eq!(order, [run(0), run(0), run(0), run(1), run(1), run(1), run(0)]);
    }

    #[test]
    fn every_round_goes_in_queue_order() {
        // one server, as a style sweep on one GPU: 1 2 3, 1 2 3, … (not 3 1 2)
        for in_flight in [1, 2] {
            let (mut s, now) = up(&[Some("a")]);
            s.max_in_flight = in_flight;
            for r in 0..3 {
                s.add_seeds(&run(r), model("a"), seeds(3));
            }
            let mut order = vec![];
            let mut queued: VecDeque<Ticket> = VecDeque::new();
            loop {
                queued.extend(submits(&s.poll(now)).into_iter().map(|(_, t, seed)| {
                    order.push(seed.run);
                    t
                }));
                let Some(t) = queued.pop_front() else { break };
                s.finished(t, Outcome::Done);
            }
            let want: Vec<Id> = (0..3).flat_map(|_| (0..3).map(run)).collect();
            assert_eq!(order, want, "{in_flight} in flight");
        }
    }

    #[test]
    fn sequential_finishes_each_run_in_turn() {
        let (mut s, now) = up(&[Some("a")]);
        s.max_in_flight = 1;
        s.order = Order::Sequential;
        for r in 0..3 {
            s.add_seeds(&run(r), model("a"), seeds(2));
        }
        let mut order = vec![];
        let step = |s: &mut Scheduler, order: &mut Vec<Id>| {
            let sub = submits(&s.poll(now));
            let Some((_, t, seed)) = sub.first().cloned() else { return false };
            order.push(seed.run);
            s.finished(t, Outcome::Done);
            true
        };
        step(&mut s, &mut order);
        // switching back spreads what is left
        s.order = Order::Spread;
        while step(&mut s, &mut order) {}
        assert_eq!(order, [run(0), run(1), run(2), run(0), run(1), run(2)]);
        let (mut s, now) = up(&[Some("a")]);
        s.max_in_flight = 1;
        s.order = Order::Sequential;
        for r in 0..3 {
            s.add_seeds(&run(r), model("a"), seeds(2));
        }
        let mut order = vec![];
        while {
            let sub = submits(&s.poll(now));
            sub.first()
                .cloned()
                .map(|(_, t, seed)| {
                    order.push(seed.run);
                    s.finished(t, Outcome::Done);
                })
                .is_some()
        } {}
        assert_eq!(order, [run(0), run(0), run(1), run(1), run(2), run(2)]);
    }

    #[test]
    fn by_sweep_spreads_one_sweep_at_a_time() {
        let (mut s, now) = up(&[Some("a")]);
        s.max_in_flight = 1;
        s.order = Order::BySweep;
        // a sweep (0, 1), a lone run (2), another sweep (3, 4)
        for (r, group) in [(0, Some("s1")), (1, Some("s1")), (2, None), (3, Some("s2")), (4, Some("s2"))] {
            s.add_seeds(&run(r), model("a"), seeds(2));
            if let Some(g) = group {
                s.set_group(&run(r), g);
            }
        }
        let mut order = vec![];
        while {
            let sub = submits(&s.poll(now));
            sub.first()
                .cloned()
                .map(|(_, t, seed)| {
                    order.push(seed.run);
                    s.finished(t, Outcome::Done);
                })
                .is_some()
        } {}
        let want = [0, 1, 0, 1, 2, 2, 3, 4, 3, 4].map(run);
        assert_eq!(order, want);
    }

    #[test]
    fn two_servers_share_rounds_in_order() {
        let (mut s, now) = up(&[Some("a"), Some("a")]);
        s.max_in_flight = 1;
        s.add_seeds(&run(0), model("a"), seeds(10));
        s.add_seeds(&run(1), model("a"), seeds(10));
        let first = submits(&s.poll(now));
        assert_eq!(first.iter().map(|x| x.2.run.clone()).collect::<Vec<_>>(), [run(0), run(1)]);
        for (_, t, _) in &first {
            s.finished(*t, Outcome::Done);
        }
        // equally due again: the longest waiting run first
        let second = submits(&s.poll(now));
        assert_eq!(second.iter().map(|x| x.2.run.clone()).collect::<Vec<_>>(), [run(0), run(1)]);
    }

    #[test]
    fn drain_load_warm_resume() {
        let (mut s, now) = up(&[Some("a"), Some("a")]);
        s.add_seeds(&run(0), model("a"), seeds(10));
        let sub = submits(&s.poll(now));
        assert_eq!(sub.len(), 4);
        s.add_seeds(&run(1), model("b"), seeds(2));
        assert!(s.poll(now).is_empty());
        // one server (the first, both equally busy) drains for b
        assert_eq!(s.server_state(0), &ServerState::Draining { target: model("b") });
        assert_eq!(s.server_state(1), &ServerState::Ready);
        for (srv, t, _) in &sub {
            if *srv == 0 {
                s.finished(*t, Outcome::Done);
            }
        }
        let acts = s.poll(now);
        assert_eq!(acts, [Action::Load { server: 0, model: model("b") }]);
        assert!(matches!(s.server_state(0), ServerState::Loading { .. }));
        s.loaded(0, LoadResult::Ok, now);
        assert_eq!(s.poll(now), [Action::WarmUp { server: 0 }]);
        s.warmed(0, true, now);
        let b = submits(&s.poll(now));
        assert_eq!(b.len(), 2);
        assert!(b.iter().all(|(srv, _, seed)| *srv == 0 && seed.run == run(1)));
    }

    #[test]
    fn idle_server_follows_the_work() {
        let (mut s, now) = up(&[Some("a"), Some("b")]);
        s.add_seeds(&run(0), model("a"), seeds(10));
        let acts = s.poll(now);
        assert_eq!(submits(&acts).len(), 2, "only server 0 has a");
        // server 1 has no work for b and nothing in flight: it loads a at once
        assert!(acts.contains(&Action::Load { server: 1, model: model("a") }));
        assert_eq!(s.server_state(1), &ServerState::Loading { target: model("a") });
    }

    #[test]
    fn a_helper_waits_for_the_first_load() {
        let (mut s, now) = up(&[None, None]);
        s.add_seeds(&run(0), model("a"), seeds(4));
        assert_eq!(s.poll(now), [Action::Load { server: 0, model: model("a") }]);
        // server 1 doesn't load a too while server 0 loads it...
        assert_eq!(s.server_state(1), &ServerState::Ready);
        // ...so it's free for b, queued meanwhile
        s.add_seeds(&run(1), model("b"), seeds(4));
        assert_eq!(s.poll(now), [Action::Load { server: 1, model: model("b") }]);
        // with nothing else to do, it helps once the load is done
        let (mut s, now) = up(&[None, None]);
        s.add_seeds(&run(0), model("a"), seeds(10));
        s.poll(now);
        s.loaded(0, LoadResult::Ok, now);
        let acts = s.poll(now);
        assert!(
            acts.contains(&Action::WarmUp { server: 0 })
                && acts.contains(&Action::Load { server: 1, model: model("a") })
        );
    }

    #[test]
    fn single_server_does_not_thrash() {
        let (mut s, now) = up(&[Some("a")]);
        s.add_seeds(&run(0), model("a"), seeds(3));
        s.add_seeds(&run(1), model("b"), seeds(3));
        let mut ran = vec![];
        loop {
            let acts = s.poll(now);
            if acts.is_empty() {
                break;
            }
            for a in acts {
                match a {
                    Action::Submit { ticket, seed, .. } => {
                        ran.push(seed.run.clone());
                        s.finished(ticket, Outcome::Done);
                    }
                    Action::Load { server, .. } => s.loaded(server, LoadResult::Ok, now),
                    Action::WarmUp { server } => {
                        s.warmed(server, true, now);
                    }
                    other => panic!("{other:?}"),
                }
            }
        }
        // all of a, one load, then all of b
        assert_eq!(ran, [run(0), run(0), run(0), run(1), run(1), run(1)]);
        assert!(s.is_idle());
    }

    #[test]
    fn empty_server_loads_on_demand() {
        let (mut s, now) = up(&[None]);
        s.add_seeds(&run(0), model("a"), seeds(1));
        assert_eq!(s.poll(now), [Action::Load { server: 0, model: model("a") }]);
    }

    #[test]
    fn failing_server_backs_off_and_seeds_return() {
        let (mut s, now) = up(&[Some("a"), Some("a")]);
        s.add_seeds(&run(0), model("a"), seeds(6));
        let sub = submits(&s.poll(now));
        assert_eq!(sub.len(), 4);
        s.server_failed(0, now);
        assert!(matches!(s.server_state(0), ServerState::Backoff { .. }));
        assert_eq!(s.run_counts(&run(0)), Some((4, 2)));
        // a late report for an abandoned ticket is ignored
        assert_eq!(s.finished(sub[0].1, Outcome::Done), None);
        assert!(s.poll(now).is_empty());
        let later = now + Duration::from_secs(2);
        assert_eq!(s.poll(later), [Action::Probe { server: 0 }]);
        s.probed(0, Err(()), later);
        let ServerState::Backoff { until } = *s.server_state(0) else { panic!() };
        assert_eq!(until, later + Duration::from_secs(4), "doubles");
        s.probed(0, Ok(Some(model("a"))), later); // ignored: not probing
        s.finished(sub[2].1, Outcome::Done);
        let next = submits(&s.poll(later));
        assert_eq!(next[0].2.seed, 0, "returned seeds go first");
    }

    #[test]
    fn retry_and_dropped_seeds() {
        let (mut s, now) = up(&[Some("a")]);
        s.max_in_flight = 1;
        s.add_seeds(&run(0), model("a"), seeds(2));
        let (_, t, seed) = submits(&s.poll(now))[0].clone();
        assert_eq!(seed.seed, 0);
        s.finished(t, Outcome::Retry);
        let (_, t, seed) = submits(&s.poll(now))[0].clone();
        assert_eq!(seed.seed, 0, "retried");
        s.finished(t, Outcome::Dropped);
        let (_, t, seed) = submits(&s.poll(now))[0].clone();
        assert_eq!(seed.seed, 1);
        s.finished(t, Outcome::Done);
        assert!(s.is_idle());
    }

    #[test]
    fn cancel_drops_pending_and_cancels_in_flight() {
        let (mut s, now) = up(&[Some("a")]);
        s.add_seeds(&run(0), model("a"), seeds(5));
        let sub = submits(&s.poll(now));
        let (dropped, acts) = s.cancel_run(&run(0));
        assert_eq!(dropped.len(), 3);
        assert_eq!(acts.len(), 2);
        assert!(matches!(acts[0], Action::Cancel { server: 0, .. }));
        assert!(s.poll(now).is_empty());
        for (_, t, _) in sub {
            s.finished(t, Outcome::Dropped);
        }
        assert!(s.is_idle());
    }

    #[test]
    fn requeue_keeps_round_robin_count() {
        let (mut s, now) = up(&[Some("a")]);
        s.max_in_flight = 1;
        s.add_seeds(&run(0), model("a"), seeds(3));
        let (_, t, _) = submits(&s.poll(now))[0].clone();
        s.finished(t, Outcome::Done);
        s.add_seeds(&run(1), model("a"), seeds(3));
        s.set_pending(&run(0), model("a"), (1..10).map(|x| (0, x)));
        assert_eq!(s.run_counts(&run(0)), Some((9, 0)));
        assert_eq!(submits(&s.poll(now))[0].2.run, run(1), "run 0 already had one");
    }

    #[test]
    fn broken_model_leaves_runs_stuck() {
        let (mut s, now) = up(&[Some("a")]);
        s.add_seeds(&run(0), model("b"), seeds(1));
        assert_eq!(s.poll(now), [Action::Load { server: 0, model: model("b") }]);
        s.loaded(0, LoadResult::Broken { still_loaded: Some(model("a")) }, now);
        assert!(s.poll(now).is_empty(), "no reload loop");
        assert_eq!(s.stuck_runs(), [&run(0)]);
        assert_eq!(s.model_of(0).cloned(), Some(model("a")));
        s.retry_model(&model("b"));
        assert_eq!(s.poll(now), [Action::Load { server: 0, model: model("b") }]);
    }

    #[test]
    fn a_model_that_fails_its_warm_up_three_times_is_given_up() {
        let (mut s, mut now) = up(&[None]);
        s.add_seeds(&run(0), model("b"), seeds(1));
        for i in 1..=WARM_UP_TRIES {
            // the server crashed and came back empty: loaded again, warmed again
            loop {
                let actions = s.poll(now);
                if let [Action::Probe { .. }] = actions[..] {
                    s.probed(0, Ok(None), now);
                } else if let [Action::Load { .. }] = actions[..] {
                    s.loaded(0, LoadResult::Ok, now);
                } else if let [Action::WarmUp { .. }] = actions[..] {
                    break;
                } else {
                    now += Duration::from_secs(120);
                }
            }
            let gave_up = s.warmed(0, false, now);
            assert_eq!(gave_up, (i == WARM_UP_TRIES).then(|| model("b")), "try {i}");
        }
        assert_eq!(s.stuck_runs(), [&run(0)]);
        for _ in 0..5 {
            now += Duration::from_secs(120);
            for a in s.poll(now) {
                match a {
                    Action::Probe { .. } => s.probed(0, Ok(None), now),
                    a => panic!("{a:?}: not loaded again"),
                }
            }
        }
    }

    #[test]
    fn a_good_warm_up_starts_the_count_again() {
        let (mut s, now) = up(&[Some("b")]);
        s.add_seeds(&run(0), model("b"), seeds(1));
        for ok in [false, false, true, false, false] {
            s.servers[0].state = ServerState::WarmingUp;
            s.servers[0].loaded = Some(model("b"));
            assert_eq!(s.warmed(0, ok, now), None);
        }
        assert!(s.stuck_runs().is_empty());
    }
}
