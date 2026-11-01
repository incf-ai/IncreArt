//! A preset's servers together: opening them as the app does, which
//! server each model preset goes to, two at once, prompt enhance beside
//! images, and the real engine end to end (and with a server going away).

use std::collections::BTreeMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use increart_core::config::ServerConfig;
use increart_core::engine::{Endpoint, Engine, EngineOptions, Event, Stop};
use increart_core::model::{Mode, ModelFile, ModelSelection, Sample};
use increart_core::pe::PeRun;
use increart_core::sdserver::Loadable;
use increart_core::{Library, NewIdea, NewRun};

use crate::catalogue::Target;
use crate::ctx::*;

/// Whether something answers on `host:port` (a started server's port
/// taken: the app's own servers, still open?).
fn taken(host: &str, port: u16) -> bool {
    let Ok(addrs) = (host, port).to_socket_addrs() else { return false };
    addrs.into_iter().any(|a| TcpStream::connect_timeout(&a, Duration::from_millis(500)).is_ok())
}

fn host_of(s: &ServerConfig) -> String {
    let url = s.base_url();
    let rest = url.split("://").nth(1).unwrap_or(&url);
    rest.rsplit_once(':').map_or(rest, |(h, _)| h).to_string()
}

pub fn open(cx: &mut Ctx, _: Target) -> Verdict {
    let log_dir = cx.work.join("servers");
    let wait = increart_core::config::timeouts().server_start();
    // the suite starts its own: a port answering already is someone else's
    let mut busy = vec![];
    for s in &cx.images {
        if let Some(port) = s.config.port().filter(|_| s.config.is_started())
            && taken(&host_of(&s.config), port)
        {
            busy.push(format!("{} (port {port})", s.config.name()));
        }
    }
    for c in &cx.chats {
        if let PeRun::KoboldCpp { host, port, .. } = &c.server.run
            && taken(host, *port)
        {
            busy.push(format!("{} (port {port})", c.server.name));
        }
    }
    if !busy.is_empty() {
        let why = format!(
            "something answers already where the suite would start {}: the app's servers? Close them (or the app) \
             first: the suite starts its own",
            busy.join(", ")
        );
        for s in &mut cx.images {
            s.failed = Some(why.clone());
        }
        for c in &mut cx.chats {
            c.failed = Some(why.clone());
        }
        return fail(why);
    }
    let (config, cancel) = (cx.config, cx.cancel);
    let opened: Vec<_> = std::thread::scope(|sc| {
        let handles: Vec<_> = cx
            .images
            .iter()
            .map(|s| {
                let (server, log_dir) = (s.config.clone(), log_dir.clone());
                sc.spawn(move || {
                    let t = Instant::now();
                    let r = increart_core::launcher::open_cancellable(
                        &server,
                        &config.programs.sd_server,
                        &log_dir,
                        wait,
                        cancel,
                    );
                    (r.map_err(|e| e.to_string()), t.elapsed())
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| (Err("panicked".into()), Duration::ZERO))).collect()
    });
    let mut said = vec![];
    let mut failed = vec![];
    for (s, (r, took)) in cx.images.iter_mut().zip(opened) {
        let name = s.config.name().to_string();
        match r {
            Ok(p) => {
                let how =
                    if p.launched() { format!("started in {:.1} s", took.as_secs_f32()) } else { "answers".into() };
                cx.out.log(format!("{name}: {how} at {}", p.client.base_url()));
                said.push(format!("{name} {how}"));
                s.proc = Some(p);
            }
            Err(e) => {
                cx.out.log(format!("{name}: {e}"));
                failed.push(format!("{name}: {e}"));
                s.failed = Some(e);
            }
        }
    }
    for c in &mut cx.chats {
        let t = Instant::now();
        match increart_core::pe::open(&c.server, &log_dir, wait, cancel) {
            Ok(p) => {
                let how = if p.log.is_some() {
                    format!("started in {:.1} s", t.elapsed().as_secs_f32())
                } else {
                    "answers".into()
                };
                cx.out.log(format!("{} (chat): {how}", c.server.name));
                said.push(format!("{} (chat) {how}", c.server.name));
                c.proc = Some(p);
            }
            Err(e) => {
                cx.out.log(format!("{} (chat): {e}", c.server.name));
                failed.push(format!("{} (chat): {e}", c.server.name));
                c.failed = Some(e);
            }
        }
    }
    if failed.is_empty() {
        pass(said.join(", "))
    } else {
        fail(format!("{} of {} didn't open: {}", failed.len(), failed.len() + said.len(), failed.join("; ")))
    }
}

pub fn routing(cx: &mut Ctx, _: Target) -> Verdict {
    let open = cx.open_images();
    if open.is_empty() {
        return skipped("no image server opened");
    }
    let lists: Vec<(usize, Result<Option<Loadable>, String>)> = open.iter().map(|&s| (s, cx.loadable(s))).collect();
    let mut problems = vec![];
    for m in &cx.models {
        let (mut listing, mut by_name, mut any, mut not) = (vec![], vec![], vec![], vec![]);
        for (s, l) in &lists {
            let name = cx.name(*s).to_string();
            match l {
                Ok(Some(l @ Loadable::Files { .. })) if l.runs(&m.key.models) => listing.push(name),
                Ok(Some(l @ Loadable::Only(_))) if l.runs(&m.key.models) => by_name.push(name),
                Ok(Some(l)) => not.push(format!("{name}: {}", l.why_not(&m.key.models).unwrap_or_default())),
                Ok(None) => any.push(name),
                Err(e) => not.push(format!("{name}: its list failed ({e})")),
            }
        }
        let mut line = vec![];
        if !listing.is_empty() {
            line.push(format!("{} (lists its files)", listing.join(", ")));
        }
        if !by_name.is_empty() {
            let when = if listing.is_empty() { "" } else { ", after those" };
            line.push(format!("{} (runs a file of that name{when})", by_name.join(", ")));
        }
        if !any.is_empty() {
            line.push(format!("{} may get it (no list)", any.join(", ")));
        }
        if line.is_empty() {
            problems.push(format!("{}: no server has its files ({})", m.preset.name, not.join("; ")));
            cx.log(format!("{}: no server; its runs would wait ({})", m.preset.name, not.join("; ")));
        } else {
            cx.log(format!("{}: {}", m.preset.name, line.join("; ")));
        }
    }
    let skipped_servers = cx.images.len() - open.len();
    let note = if skipped_servers > 0 { format!(" ({skipped_servers} servers not open)") } else { String::new() };
    if problems.is_empty() {
        pass(format!("each of the {} model presets has a server{note}", cx.models.len()))
    } else {
        warn(format!("{}{note}", problems.join("; ")))
    }
}

pub fn parallel(cx: &mut Ctx, _: Target) -> Verdict {
    let open = cx.open_images();
    if open.len() < 2 {
        return skipped("fewer than two image servers opened");
    }
    let mut jobs = vec![];
    for &s in &open[..2] {
        let m = match cx.model_for(s) {
            Ok(m) => m,
            Err(e) => return skipped(format!("{}: {e}", cx.name(s))),
        };
        if let Err(e) = cx.ensure_loaded(s, m) {
            return fail(format!("{}: {e}", cx.name(s)));
        }
        jobs.push((s, cx.body(m, 71, 8, &[])));
    }
    let at = Instant::now();
    let cx = &*cx;
    let results: Vec<(usize, Result<(), String>, Duration)> = std::thread::scope(|sc| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|(s, body)| {
                let job = std::thread::Builder::new().spawn_scoped(sc, move || {
                    let t = Instant::now();
                    let r = cx.generate(*s, body, IMAGE_LIMIT).and_then(|j| png_of(&j)).map(|_| ());
                    (r, t.elapsed())
                });
                (*s, job)
            })
            .collect();
        handles
            .into_iter()
            .map(|(s, job)| match job.map_err(|e| e.to_string()).map(|h| h.join()) {
                Ok(Ok((r, took))) => (s, r, took),
                Ok(Err(_)) => (s, Err("its job's thread stopped".into()), Duration::ZERO),
                Err(e) => (s, Err(format!("its job's thread didn't start: {e}")), Duration::ZERO),
            })
            .collect()
    });
    let all = at.elapsed().as_secs_f32();
    let mut each = vec![];
    for (s, r, took) in &results {
        if let Err(e) = r {
            return fail(format!("{}: {e}", cx.name(*s)));
        }
        each.push(format!("{} {:.1} s", cx.name(*s), took.as_secs_f32()));
    }
    pass(format!("both completed ({}), {all:.1} s in all", each.join(", ")))
}

pub fn pe_with_images(cx: &mut Ctx, _: Target) -> Verdict {
    let Some(&s) = cx.open_images().first() else { return skipped("no image server opened") };
    let Some(c) = cx.chats.iter().find_map(|c| c.proc.as_ref().map(|p| p.client.clone())) else {
        return skipped("no chat server opened");
    };
    let m = match cx.model_for(s) {
        Ok(m) => m,
        Err(e) => return skipped(e),
    };
    if let Err(e) = cx.ensure_loaded(s, m) {
        return fail(e);
    }
    let client = tried!(cx.client(s));
    let id = match client.img_gen(&cx.body(m, 81, 12, &[])) {
        Ok(id) => id,
        Err(e) => return fail(format!("the image: {e}")),
    };
    let at = Instant::now();
    let sampling = increart_core::pe::Sampling { temperature: 0.2, min_p: 0.0 };
    let answer = c.chat("Answer in one word.", "What colour is grass?", &[], 16, sampling);
    let chat_took = at.elapsed().as_secs_f32();
    let image = cx.wait(&client, &id, IMAGE_LIMIT, |_| true).and_then(|j| png_of(&j));
    match (answer, image) {
        (Ok(a), Ok(_)) => pass(format!(
            "the ask answered \"{}\" in {chat_took:.1} s; the image completed in {:.1} s",
            a.trim(),
            at.elapsed().as_secs_f32()
        )),
        (Err(e), _) => fail(format!("the ask beside an image: {e}")),
        (_, Err(e)) => fail(format!("the image beside an ask: {e}")),
    }
}

pub fn end_to_end(cx: &mut Ctx, _: Target) -> Verdict {
    engine_run(cx, false)
}

pub fn failover(cx: &mut Ctx, _: Target) -> Verdict {
    engine_run(cx, true)
}

/// A run of a few seeds through the engine over the open image servers,
/// in a scratch library; `stop_one`: a started server taking part is
/// stopped after the first image.
fn engine_run(cx: &mut Ctx, stop_one: bool) -> Verdict {
    let open = cx.open_images();
    if open.is_empty() {
        return skipped("no image server opened");
    }
    // the model preset most of them can load
    let can: Vec<Vec<usize>> = open.iter().map(|&s| cx.models_for(s)).collect();
    let Some(m) = (0..cx.models.len()).max_by_key(|m| (can.iter().filter(|c| c.contains(m)).count(), usize::MAX - m))
    else {
        return skipped("the config has no model presets");
    };
    let takers: Vec<usize> = open.iter().zip(&can).filter(|(_, c)| c.contains(&m)).map(|(s, _)| *s).collect();
    if takers.is_empty() {
        return skipped("no open server has the files of any model preset");
    }
    let victim = if stop_one {
        if takers.len() < 2 {
            return not_applicable(format!("only one open server can load {}", cx.models[m].preset.name));
        }
        match takers.iter().find(|&&s| cx.images[s].proc.as_ref().is_some_and(|p| p.launched())) {
            Some(&v) => Some(v),
            None => return not_applicable("no server the suite started can load the same model preset as another"),
        }
    } else {
        None
    };
    let seeds = if stop_one { 6 } else { (takers.len() as u64 * 2).clamp(2, 8) };
    let preset = cx.models[m].preset.clone();
    let dir = cx.work.join("work").join(if stop_one { "failover" } else { "end-to-end" });
    let _ = std::fs::remove_dir_all(&dir);
    let made = (|| -> increart_core::error::Result<(Library, increart_core::Id)> {
        let mut lib = Library::create(&dir.join("library"), None)?;
        let idea = lib.new_idea(NewIdea::text("Apple", "a red apple on a wooden table"))?;
        let (project, _) = lib.create_project(Path::new(""), Some("apitest"), &[idea])?;
        let files: BTreeMap<String, ModelFile> = preset
            .files
            .iter()
            .map(|(slot, id)| (slot.clone(), ModelFile { id: id.clone(), sha256: String::new() }))
            .collect();
        let run = NewRun {
            sweep: None,
            label: "apitest".into(),
            prompt: "a red apple on a wooden table, photo".into(),
            negative_prompt: String::new(),
            alpha: false,
            width: 512,
            height: 512,
            sample: Sample {
                method: "euler".into(),
                scheduler: String::new(),
                steps: preset.steps.clamp(1, 4),
                cfg: preset.cfg,
                flow_shift: preset.flow_shift,
                custom_sigmas: vec![],
            },
            mode: Mode::Generate,
            refs: vec![],
            model: ModelSelection { recipe: preset.recipe.clone(), files, loras: vec![] },
            source_idea: None,
            first_seed: 100,
            seeds,
        };
        let id = lib.create_run(&project, run)?;
        Ok((lib, id))
    })();
    let (lib, run) = match made {
        Ok(x) => x,
        Err(e) => return fail(format!("the scratch library: {e}")),
    };
    let endpoints: Vec<Endpoint> = open
        .iter()
        .filter_map(|&s| {
            // (all open: `open` lists those with a process)
            let p = cx.images[s].proc.as_ref()?;
            Some(Endpoint { name: p.name.clone(), device: p.device.clone(), client: p.client.clone() })
        })
        .collect();
    let names: Vec<String> = endpoints.iter().map(|e| e.name.clone()).collect();
    cx.log(format!(
        "{seeds} seeds of {} over {} (as the app opens them; {} can load it)",
        preset.name,
        names.join(", "),
        takers.iter().map(|&s| cx.name(s)).collect::<Vec<_>>().join(", ")
    ));
    let engine = match Engine::start(Arc::new(Mutex::new(lib)), endpoints, EngineOptions::default(), None) {
        Ok(e) => e,
        Err(e) => return fail(format!("couldn't start the engine: {e}")),
    };
    engine.queue_run(&run);
    let limit = IMAGE_LIMIT + Duration::from_secs(120 * seeds);
    let deadline = Instant::now() + limit;
    let (mut made, mut failed, mut problems) = (0u64, vec![], vec![]);
    let mut by_server: BTreeMap<String, u64> = BTreeMap::new();
    let mut seed_server: BTreeMap<u64, usize> = BTreeMap::new();
    // servers whose load of it failed in the run ("may have anything":
    // listing no files, they are sent it and refuse it)
    let mut refused: Vec<String> = vec![];
    let mut shown: BTreeMap<usize, String> = BTreeMap::new();
    let mut stopped_victim = false;
    let done = loop {
        if cx.stopped() {
            engine.cancel_run(&run, Stop::Now);
            break Err("stopped".to_string());
        }
        if Instant::now() > deadline {
            break Err(format!("not done after {} min", limit.as_secs() / 60));
        }
        let Ok(e) = engine.events().recv_timeout(Duration::from_millis(200)) else { continue };
        match e {
            Event::Server { server, state, .. } => {
                if shown.get(&server) != Some(&state) {
                    cx.log(format!("{}: {state}", names[server]));
                    shown.insert(server, state);
                }
            }
            Event::Progress { seed, server, .. } => {
                seed_server.insert(seed, server);
            }
            Event::Output { seed, .. } => {
                made += 1;
                let on = seed_server.get(&seed).map(|&i| names[i].clone()).unwrap_or_else(|| "?".into());
                cx.log(format!("seed {seed} made on {on}"));
                *by_server.entry(on).or_default() += 1;
                if let Some(v) = victim.filter(|_| !stopped_victim) {
                    cx.log(format!("stopping {} (the failover)", cx.name(v)));
                    if let Some(p) = cx.images[v].proc.as_mut() {
                        p.stop();
                    }
                    cx.images[v].gone = true;
                    stopped_victim = true;
                }
            }
            Event::SeedFailed { seed, error, .. } => {
                cx.log(format!("seed {seed} failed: {error}"));
                failed.push(format!("seed {seed}: {error}"));
            }
            Event::Problem(p) => {
                cx.log(format!("problem: {p}"));
                problems.push(p);
            }
            Event::Log { line, .. } => {
                // "[server] seed N: sent": where it went (Progress comes only
                // with steps polled, which a quick job may not have)
                if let Some((s, seed)) = sent_to(&line, &names) {
                    seed_server.insert(seed, s);
                }
                if let Some(n) = names.iter().find(|n| line.contains(&format!("[{n}] loading failed")))
                    && !refused.contains(n)
                {
                    refused.push(n.clone());
                }
                cx.log(format!("engine: {line}"));
            }
            Event::RunDone { .. } => break Ok(()),
        }
    };
    engine.shutdown();
    let spread: Vec<String> = by_server.iter().map(|(s, n)| format!("{n} on {s}")).collect();
    let spread = if spread.is_empty() { "none made".into() } else { spread.join(", ") };
    match done {
        Err(e) => fail(format!("{e}: {made} of {seeds} made ({spread})")),
        Ok(()) if !failed.is_empty() => {
            fail(format!("{} of {seeds} seeds failed ({spread}): {}", failed.len(), failed[0]))
        }
        Ok(()) if made < seeds => fail(format!("the run ended with {made} of {seeds} made ({spread})")),
        Ok(()) if stop_one => {
            pass(format!("all {seeds} made after {} went ({spread})", victim.map(|v| cx.name(v)).unwrap_or("")))
        }
        Ok(()) if takers.len() > 1 && by_server.len() < 2 => {
            let left = takers.iter().filter(|&&s| !refused.iter().any(|n| n == cx.name(s))).count();
            if left < 2 {
                pass(format!(
                    "all {seeds} made ({spread}); {} was sent it and refused it (the log says why)",
                    refused.join(", ")
                ))
            } else {
                warn(format!("all {seeds} made, but on one server though {} can load it ({spread})", takers.len()))
            }
        }
        Ok(()) if !problems.is_empty() => {
            warn(format!("all {seeds} made ({spread}); the engine said: {}", problems[0]))
        }
        Ok(()) => pass(format!("all {seeds} made ({spread})")),
    }
}

/// The server a run log line says a seed was sent to, and the seed.
fn sent_to(line: &str, names: &[String]) -> Option<(usize, u64)> {
    let (s, name) = names.iter().enumerate().find(|(_, n)| line.contains(&format!("[{n}] seed ")))?;
    let rest = line.split(&format!("[{name}] seed ")).nth(1)?;
    let (seed, text) = rest.split_once(':')?;
    (text.trim() == "sent").then_some(())?;
    Some((s, seed.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_sent_seed_is_found_in_the_run_log() {
        let names = vec!["sd-rocm0".to_string(), "sd-rocm1".to_string()];
        let at = "2026-10-31 16:43:56.368";
        assert_eq!(super::sent_to(&format!("{at} [sd-rocm1] seed 100: sent"), &names), Some((1, 100)));
        assert_eq!(super::sent_to(&format!("{at} [sd-rocm0] seed 7: done in 2.1 s"), &names), None);
        assert_eq!(super::sent_to(&format!("{at} [sd-rocm0] warm-up image done"), &names), None);
    }
}
