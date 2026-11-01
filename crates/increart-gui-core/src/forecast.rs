//! Roughly when each run in the engine starts and finishes (DESIGN.md,
//! Timing): the engine's own scheduler on a simulated clock, each image
//! taking what images like it took (the library's timed images) on that
//! server, and a model switch what its loads took.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use increart_core::Id;
use increart_core::model::Run;
use increart_core::scheduler::{Action, LoadResult, Order, Outcome, Scheduler, Ticket};
use increart_core::sdserver::ModelKey;

/// What an image's time depends on.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub recipe: String,
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub refs: usize,
    /// Model passes a step: 2 with CFG above 1 (the negative prompt too).
    pub passes: u32,
}

impl Shape {
    pub fn of(r: &Run) -> Shape {
        Shape {
            recipe: r.model.recipe.clone(),
            width: r.width,
            height: r.height,
            steps: r.sample.steps,
            refs: r.refs.len(),
            passes: if r.sample.cfg > 1.0 { 2 } else { 1 },
        }
    }

    /// A run the Generate form would make; its model preset's recipe.
    pub fn of_form(f: &crate::GenerateForm, recipes: &[increart_core::config::RecipePreset]) -> Shape {
        Shape {
            recipe: recipes.get(f.preset).map(|r| r.recipe.clone()).unwrap_or_default(),
            width: f.width,
            height: f.height,
            steps: f.steps,
            refs: f.refs().len(),
            passes: if f.cfg > 1.0 { 2 } else { 1 },
        }
    }

    /// Seconds per image on one R9700 (ROCm, int8), from the image's
    /// tokens and its references' (fitted to early measurements): for
    /// when nothing like it was timed.
    pub fn rough_image_s(&self) -> f64 {
        let t = (self.width / 16 * self.height / 16) as f64;
        let r = 4096.0 * self.refs as f64;
        let per_pass = 1.0e-4 * t + 6.9e-8 * t * (t + r);
        per_pass * self.passes.max(1) as f64 * self.steps as f64 + 3.0
    }

    fn mp(&self) -> f64 {
        (self.width as f64 * self.height as f64 / 1e6).max(0.01)
    }
}

/// A mean as it is summed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Mean(f64, u32);

impl Mean {
    fn add(&mut self, v: f64) {
        self.0 += v;
        self.1 += 1;
    }

    fn get(self) -> Option<f64> {
        (self.1 > 0).then(|| self.0 / self.1 as f64)
    }
}

/// An image just like it: (recipe, device, w, h, steps, refs, passes).
type Exact = (String, String, u32, u32, u32, usize, u32);

/// Seconds per image from the library's timed images, at levels from
/// images just like it on that device down to any image anywhere.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rates {
    /// (recipe, device, w, h, steps, refs, passes) → seconds per image.
    exact: HashMap<Exact, Mean>,
    /// (recipe, device, w, h, refs) → seconds per step and pass.
    sized: HashMap<(String, String, u32, u32, usize), Mean>,
    /// (recipe, device) → seconds per step, pass and megapixel.
    model_device: HashMap<(String, String), Mean>,
    /// recipe → seconds per step and megapixel, any device.
    model: HashMap<String, Mean>,
    /// device → seconds per step and megapixel, any model.
    device: HashMap<String, Mean>,
    any: Mean,
}

impl Rates {
    pub fn of<'a>(runs: impl IntoIterator<Item = &'a Run>) -> Rates {
        let mut r = Rates::default();
        for run in runs {
            let s = Shape::of(run);
            let steps = (s.steps.max(1) * s.passes.max(1)) as f64;
            for t in &run.timings {
                let e = t.elapsed_s as f64;
                if e <= 0.0 {
                    continue;
                }
                let d = t.device.clone();
                let per = e / steps / s.mp();
                let exact = (s.recipe.clone(), d.clone(), s.width, s.height, s.steps, s.refs, s.passes);
                r.exact.entry(exact).or_default().add(e);
                r.sized.entry((s.recipe.clone(), d.clone(), s.width, s.height, s.refs)).or_default().add(e / steps);
                r.model_device.entry((s.recipe.clone(), d.clone())).or_default().add(per);
                r.model.entry(s.recipe.clone()).or_default().add(per);
                r.device.entry(d).or_default().add(per);
                r.any.add(per);
            }
        }
        r
    }

    /// Roughly how long an image of this shape takes on `device`, and
    /// whether that's from images just like it (same model, size, steps
    /// and references there); `None` with no timed images at all.
    pub fn image_s(&self, s: &Shape, device: &str) -> Option<(f64, bool)> {
        let d = device.to_string();
        let steps = (s.steps.max(1) * s.passes.max(1)) as f64;
        let scaled = |m: Option<&Mean>| m.and_then(|m| m.get()).map(|per| per * steps * s.mp());
        if let Some(e) = self.exact.get(&(s.recipe.clone(), d.clone(), s.width, s.height, s.steps, s.refs, s.passes)) {
            return e.get().map(|e| (e, true));
        }
        self.sized
            .get(&(s.recipe.clone(), d.clone(), s.width, s.height, s.refs))
            .and_then(|m| m.get())
            .map(|per| per * steps)
            .or_else(|| scaled(self.model_device.get(&(s.recipe.clone(), d.clone()))))
            .or_else(|| scaled(self.model.get(&s.recipe)))
            .or_else(|| scaled(self.device.get(&d)))
            .or_else(|| scaled(Some(&self.any)))
            .map(|e| (e, false))
    }
}

/// A [`Plan`] on the app's clock (`AppState::now`), smoothed: each time
/// moves a third of the way to the newest plan's (all the way when that is
/// more than half again or less), so it counts down steadily instead of
/// jumping with each image.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Forecast {
    /// When it was made (`AppState::now`).
    pub made: f64,
    /// What it was made of ([`crate::AppState::plan_key`]): it's made again
    /// when that changes (an image done, a run queued, a server's state),
    /// and every [`REMAKE_S`] anyway; in between its times count down.
    pub key: u64,
    pub runs: HashMap<Id, (Option<f64>, Option<f64>)>,
}

impl Forecast {
    pub fn next(&self, plan: &Plan, now: f64) -> Forecast {
        let blend = |old: Option<f64>, new: Option<f64>| {
            let new = new.map(|s| now + s)?;
            Some(match old {
                Some(old) if (old - new).abs() <= 0.5 * (new - now).max(old - now).max(1.0) => old + (new - old) / 3.0,
                _ => new,
            })
        };
        let runs = plan
            .runs
            .iter()
            .map(|(id, (s, f))| {
                let (os, of) = self.runs.get(id).copied().unwrap_or_default();
                (id.clone(), (blend(os, *s), blend(of, *f)))
            })
            .collect();
        Forecast { made: now, key: self.key, runs }
    }
}

/// A run for [`simulate`].
#[derive(Clone, Debug, PartialEq)]
pub struct SimRun {
    pub id: Id,
    /// A sweep's runs share one ([`Order::BySweep`]).
    pub group: String,
    pub recipe: String,
    /// Seeds not started.
    pub seeds: usize,
    /// How far each seed being sampled now is, 0..1.
    pub sampling: Vec<f64>,
    /// Seconds per image on each server.
    pub image_s: Vec<f64>,
}

/// A server for [`simulate`].
#[derive(Clone, Debug, PartialEq)]
pub struct SimServer {
    /// Makes images or is about to (not connecting or unreachable).
    pub working: bool,
    pub recipe: Option<String>,
    /// Seconds to load each model.
    pub load_s: HashMap<String, f64>,
}

/// Seconds after which a forecast is made again even with nothing changed.
pub const REMAKE_S: f64 = 30.0;

/// Default seconds to load a model no load was timed of.
pub const LOAD_S: f64 = 60.0;

/// When each run starts and finishes, seconds from now; `None` for one
/// that wouldn't (no server can take it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub runs: HashMap<Id, (Option<f64>, Option<f64>)>,
}

impl Plan {
    /// When the last of them finishes; `None` when one wouldn't.
    pub fn end(&self) -> Option<f64> {
        self.runs.values().map(|(_, f)| *f).try_fold(0.0f64, |m, f| f.map(|f| m.max(f)))
    }
}

fn key(recipe: &str) -> ModelKey {
    // by recipe alone: what the GUI knows of a server's model
    ModelKey { recipe: recipe.into(), models: Default::default() }
}

/// Runs the scheduler on a simulated clock: each server generates its
/// jobs one after another (it holds `max_in_flight`), a seed being sampled
/// now takes what's left of it, and a load takes its server's time.
pub fn simulate(runs: &[SimRun], servers: &[SimServer], order: Order) -> Plan {
    let mut sched = Scheduler::new(servers.len());
    sched.order = order;
    for r in runs {
        let n = r.seeds + r.sampling.len();
        sched.add_seeds(&r.id, key(&r.recipe), (0..n as u64).map(|s| (0, s)));
        sched.set_group(&r.id, r.group.clone());
    }
    let index: HashMap<&Id, usize> = runs.iter().enumerate().map(|(i, r)| (&r.id, i)).collect();
    // what's left of the seeds sampling now, taken by the first submits
    let mut partial: Vec<VecDeque<f64>> = runs.iter().map(|r| r.sampling.iter().copied().collect()).collect();
    let mut queue: Vec<VecDeque<(Ticket, usize, f64)>> = vec![VecDeque::new(); servers.len()];
    // the job generating on each server: when it ends
    let mut head: Vec<Option<f64>> = vec![None; servers.len()];
    let mut loading: Vec<Option<f64>> = vec![None; servers.len()];
    let mut plan: HashMap<Id, (Option<f64>, Option<f64>)> = runs.iter().map(|r| (r.id.clone(), (None, None))).collect();
    let mut left: Vec<usize> = runs.iter().map(|r| r.seeds + r.sampling.len()).collect();
    let base = Instant::now();
    let mut t = 0.0f64;
    let start_next = |s: usize,
                      t: f64,
                      queue: &mut Vec<VecDeque<(Ticket, usize, f64)>>,
                      head: &mut Vec<Option<f64>>,
                      plan: &mut HashMap<Id, (Option<f64>, Option<f64>)>| {
        if head[s].is_none()
            && let Some((_, r, d)) = queue[s].front()
        {
            head[s] = Some(t + d);
            let e = plan.entry(runs[*r].id.clone()).or_default();
            e.0.get_or_insert(t);
        }
    };
    for _ in 0..200_000 {
        // carry out the scheduler's actions until it has none
        for _ in 0..100 {
            let actions = sched.poll(base + Duration::from_secs_f64(t));
            if actions.is_empty() {
                break;
            }
            let now = base + Duration::from_secs_f64(t);
            for a in actions {
                match a {
                    Action::Probe { server } if servers[server].working => {
                        sched.probed(server, Ok(servers[server].recipe.as_deref().map(key)), now)
                    }
                    // not working: never answers, so never takes a seed
                    Action::Probe { .. } => {}
                    Action::WarmUp { server } => {
                        sched.warmed(server, true, now);
                    }
                    Action::Load { server, model } => {
                        let s = servers[server].load_s.get(&model.recipe).copied().unwrap_or(LOAD_S);
                        loading[server] = Some(t + s);
                    }
                    Action::Submit { server, ticket, seed } => {
                        let Some(&r) = index.get(&seed.run) else { continue };
                        let full = runs[r].image_s.get(server).copied().unwrap_or(0.0);
                        let d = partial[r].pop_front().map_or(full, |done| full * (1.0 - done.clamp(0.0, 1.0)));
                        queue[server].push_back((ticket, r, d));
                        start_next(server, t, &mut queue, &mut head, &mut plan);
                    }
                }
            }
        }
        // the next job or load to end
        let next = head.iter().chain(loading.iter()).flatten().copied().fold(f64::INFINITY, f64::min);
        if !next.is_finite() {
            break;
        }
        t = next;
        let now = base + Duration::from_secs_f64(t);
        for s in 0..servers.len() {
            if loading[s].is_some_and(|e| e <= t) {
                loading[s] = None;
                sched.loaded(s, LoadResult::Ok, now);
            }
            if head[s].is_some_and(|e| e <= t) {
                head[s] = None;
                if let Some((ticket, r, _)) = queue[s].pop_front() {
                    sched.finished(ticket, Outcome::Done);
                    left[r] -= 1;
                    if left[r] == 0 {
                        plan.entry(runs[r].id.clone()).or_default().1 = Some(t);
                    }
                }
                start_next(s, t, &mut queue, &mut head, &mut plan);
            }
        }
    }
    Plan { runs: plan }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> Id {
        Id::new(chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(), n)
    }

    fn run(n: u32, recipe: &str, seeds: usize, image_s: &[f64]) -> SimRun {
        SimRun {
            id: id(n),
            group: id(n).to_string(),
            recipe: recipe.into(),
            seeds,
            sampling: vec![],
            image_s: image_s.to_vec(),
        }
    }

    fn server(recipe: &str) -> SimServer {
        SimServer { working: true, recipe: Some(recipe.into()), load_s: HashMap::new() }
    }

    fn at(p: &Plan, n: u32) -> (Option<f64>, Option<f64>) {
        p.runs[&id(n)]
    }

    #[test]
    fn spread_shares_the_servers_one_at_a_time_doesnt() {
        let runs = [run(1, "q", 4, &[10.0, 10.0]), run(2, "q", 4, &[10.0, 10.0])];
        let servers = [server("q"), server("q")];
        let p = simulate(&runs, &servers, Order::Spread);
        // 8 images on 2 servers, taking turns: both end at 40 s; each
        // server's first two go to both runs, so run 2's start after one
        assert_eq!(at(&p, 1), (Some(0.0), Some(40.0)));
        assert_eq!(at(&p, 2), (Some(10.0), Some(40.0)));
        assert_eq!(p.end(), Some(40.0));
        let p = simulate(&runs, &servers, Order::Sequential);
        assert_eq!(at(&p, 1), (Some(0.0), Some(20.0)));
        // run 2's seeds were queued behind run 1's (2 in flight per server)
        assert_eq!(at(&p, 2).1, Some(40.0));
    }

    #[test]
    fn a_partly_sampled_seed_takes_what_is_left_and_servers_keep_what_they_hold() {
        let mut r = run(1, "q", 5, &[10.0, 30.0]);
        r.sampling = vec![0.5];
        let p = simulate(&[r], &[server("q"), server("q")], Order::Spread);
        // 6 seeds, 2 held by each server: gpu0 half a seed (5 s) and one
        // (15 s), then 2 more (35 s); gpu1 its 2 at 30 s each, as the
        // engine would (no seed moves to a server once sent)
        assert_eq!(at(&p, 1), (Some(0.0), Some(60.0)));
    }

    #[test]
    fn a_model_no_server_has_waits_for_a_load() {
        let mut s = server("q");
        s.load_s.insert("k".into(), 50.0);
        let p = simulate(&[run(1, "k", 2, &[10.0])], &[s], Order::Spread);
        // loads (50 s), then 2 × 10 s
        assert_eq!(at(&p, 1), (Some(50.0), Some(70.0)));
    }

    #[test]
    fn no_working_server_no_end() {
        let mut s = server("q");
        s.working = false;
        let p = simulate(&[run(1, "q", 2, &[10.0])], &[s], Order::Spread);
        assert_eq!(at(&p, 1), (None, None));
        assert_eq!(p.end(), None);
        assert_eq!(simulate(&[], &[server("q")], Order::Spread).end(), Some(0.0));
    }

    #[test]
    fn rates_from_images_like_it_else_scaled() {
        use increart_core::model::SeedTiming;
        let mut r = test_run();
        let t = |device: &str, e: f32| SeedTiming {
            rev: 0,
            seed: 0,
            device: device.into(),
            done: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            elapsed_s: e,
            queued_s: 0.0,
            step_s: 0.0,
        };
        r.timings = vec![t("ROCm0", 30.0), t("ROCm0", 34.0), t("Vulkan0", 160.0)];
        let rates = Rates::of([&r]);
        let mut s = Shape::of(&r);
        assert_eq!(rates.image_s(&s, "ROCm0"), Some((32.0, true)));
        // 40 steps: 1.6 s a step there
        s.steps = 40;
        assert_eq!(rates.image_s(&s, "ROCm0"), Some((64.0, false)));
        // a quarter of the pixels, 20 steps
        s.steps = 20;
        s.width = 512;
        s.height = 512;
        let (e, exact) = rates.image_s(&s, "ROCm0").unwrap();
        assert!(!exact && (e - 8.0).abs() < 0.01, "{e}");
        // another model on a device: the device's
        s.recipe = "k".into();
        assert!((rates.image_s(&s, "Vulkan0").unwrap().0 - 40.0).abs() < 0.01);
        assert_eq!(Rates::default().image_s(&s, "ROCm0"), None);
    }

    fn test_run() -> Run {
        use increart_core::model::{Mode, ModelSelection, Sample};
        Run {
            id: id(1),
            label: "r01".into(),
            created: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            prompt: String::new(),
            negative_prompt: String::new(),
            alpha: false,
            width: 1024,
            height: 1024,
            sample: Sample {
                method: "euler".into(),
                scheduler: String::new(),
                steps: 20,
                cfg: 2.0,
                flow_shift: None,
                custom_sigmas: vec![],
            },
            mode: Mode::Generate,
            refs: vec![],
            model: ModelSelection { recipe: "q".into(), files: Default::default(), loras: vec![] },
            source_idea: None,
            revisions: vec![],
            cancelled: vec![],
            failed: vec![],
            sweep: None,
            timings: vec![],
            deleted: vec![],
        }
    }
}

#[cfg(test)]
mod bench {
    #[test]
    #[ignore]
    fn big_queue() {
        use super::*;
        let runs: Vec<SimRun> = (0..40u32)
            .map(|n| SimRun {
                id: Id::new(chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(), n),
                group: format!("g{}", n / 8),
                recipe: if n % 3 == 0 { "k".into() } else { "q".into() },
                seeds: 64,
                sampling: vec![],
                image_s: vec![34.0, 36.0],
            })
            .collect();
        let servers: Vec<SimServer> =
            (0..2).map(|_| SimServer { working: true, recipe: Some("q".into()), load_s: HashMap::new() }).collect();
        for order in [Order::Spread, Order::BySweep, Order::Sequential] {
            let t = std::time::Instant::now();
            let p = simulate(&runs, &servers, order);
            eprintln!("{order:?}: {:?}, end {:?}", t.elapsed(), p.end());
        }
    }
}
