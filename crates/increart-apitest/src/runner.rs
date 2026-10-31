//! Running the chosen cases, preset by preset: its servers opened, its
//! cases in order, its started servers stopped; every request logged to
//! the case under way; the report written at the end.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use increart_core::apitest::{Case, Counts, Event, Outcome, matches};
use increart_core::config::{Config, ServerPreset};
use increart_core::sdserver::ModelKey;

use crate::catalogue::{Target, cases_of, defs};
use crate::ctx::{ChatSrv, Ctx, ImageSrv, Model, Out, Verdict, skipped};
use crate::report::{Report, presets_text};

pub struct Options {
    pub config: Config,
    pub config_path: Option<PathBuf>,
    /// The presets to test.
    pub presets: Vec<ServerPreset>,
    /// Patterns of the case IDs to run (`*` for any text); empty: all.
    pub cases: Vec<String>,
    /// Only the light cases (nothing loaded, nothing generated).
    pub light: bool,
    /// The report's folder (zipped beside it).
    pub out: PathBuf,
}

/// Whether the case is chosen (the presets' `open` is, with any other).
fn chosen(o: &Options, c: &Case, cost: increart_core::apitest::Cost) -> bool {
    (!o.light || cost == increart_core::apitest::Cost::Light)
        && (o.cases.is_empty() || o.cases.iter().any(|p| matches(p, &c.id)))
}

/// Runs them, sending each event to `emit` as it happens; the counts.
pub fn run(o: &Options, cancel: &AtomicBool, emit: Box<dyn FnMut(&Event) + Send>) -> Counts {
    let defs = defs();
    let all = cases_of(&o.config, &o.presets);
    let mut picked: Vec<bool> = all.iter().map(|(c, d, _)| chosen(o, c, defs[*d].cost)).collect();
    // a preset with any case chosen opens its servers first
    for (i, (c, d, _)) in all.iter().enumerate() {
        if defs[*d].key == "open" {
            picked[i] = all.iter().zip(&picked).any(|((x, ..), p)| *p && x.preset == c.preset);
        }
    }
    let run: Vec<_> = all.into_iter().zip(picked).filter(|(_, p)| *p).map(|(x, _)| x).collect();
    let report = Report::new(crate::version(), o.config_path.clone(), presets_text(&o.presets));
    let out = Arc::new(Out::new(emit, report));
    out.event(Event::Plan { cases: run.iter().map(|(c, ..)| c.clone()).collect() });
    let sink = out.clone();
    increart_core::traffic::set_sink(Some(Box::new(move |text| sink.traffic(text))));
    let models: Vec<Model> = o
        .config
        .recipes
        .iter()
        .map(|r| Model { preset: r.clone(), key: ModelKey { recipe: r.recipe.clone(), models: r.files.clone() } })
        .collect();
    for p in &o.presets {
        let cases: Vec<_> = run.iter().filter(|(c, ..)| c.preset == p.name).collect();
        if cases.is_empty() {
            continue;
        }
        out.log(format!("preset {}", p.name));
        let setup = crate::catalogue::setup(&o.config, p);
        let mut cx = Ctx {
            config: &o.config,
            images: setup
                .images
                .into_iter()
                .map(|config| ImageSrv { config, proc: None, failed: None, gone: false, loadable: None })
                .collect(),
            chats: setup.chats.into_iter().map(|server| ChatSrv { server, proc: None, failed: None }).collect(),
            models: models.clone(),
            work: o.out.clone(),
            cancel,
            out: &out,
        };
        for (case, d, target) in cases {
            out.event(Event::Start { id: case.id.clone() });
            let t = Instant::now();
            let v = verdict(&mut cx, case, &defs[*d], *target);
            let v = if cx.stopped() && v.outcome == Outcome::Fail {
                skipped(format!("stopped ({})", v.summary))
            } else {
                v
            };
            out.event(Event::End {
                id: case.id.clone(),
                outcome: v.outcome,
                summary: v.summary,
                secs: t.elapsed().as_secs_f64(),
            });
        }
        let started: Vec<String> =
            cx.images.iter().filter_map(|s| s.proc.as_ref().filter(|p| p.launched()).map(|p| p.name.clone())).collect();
        if !started.is_empty() {
            out.log(format!("stopping the servers the suite started: {}", started.join(", ")));
        }
        drop(cx);
    }
    increart_core::traffic::set_sink(None);
    let counts = out.with_report(|r| r.counts());
    let written = out.with_report(|r| r.write(&o.out));
    match written {
        Ok(zip) => out.event(Event::Done { counts, report: Some(o.out.clone()), zip: Some(zip) }),
        Err(e) => {
            out.log(format!("the report couldn't be written to {}: {e}", o.out.display()));
            out.event(Event::Done { counts, report: None, zip: None });
        }
    }
    counts
}

fn verdict(cx: &mut Ctx, case: &Case, d: &crate::catalogue::Def, target: Target) -> Verdict {
    if cx.stopped() {
        return skipped("stopped");
    }
    if let Some(why) = &case.not_applicable {
        return crate::ctx::not_applicable(why.clone());
    }
    match target {
        Target::Image(i) if cx.images[i].gone => return skipped("stopped by the failover case"),
        Target::Image(i) if cx.images[i].proc.is_none() => {
            return skipped(format!("it didn't open: {}", cx.images[i].failed.as_deref().unwrap_or("not opened")));
        }
        Target::Chat(i) if cx.chats[i].proc.is_none() => {
            return skipped(format!("it didn't open: {}", cx.chats[i].failed.as_deref().unwrap_or("not opened")));
        }
        _ => {}
    }
    let run = d.run;
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(cx, target))) {
        Ok(v) => v,
        Err(p) => {
            let why = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            crate::ctx::fail(format!("the case itself failed (a bug in increart-apitest): {why}"))
        }
    }
}
