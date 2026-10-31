//! An image server's cases: sd-server's API (a proxy's, KoboldCpp's
//! through the same client) as the engine calls it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use increart_core::apitest::Outcome;
use increart_core::sdserver::{ApiError, JobStatus, ModelKey};
use increart_core::servertest;
use serde_json::json;

use crate::catalogue::Target;
use crate::ctx::*;

fn server(t: Target) -> Result<usize, String> {
    match t {
        Target::Image(s) => Ok(s),
        _ => Err("not an image server's case".into()),
    }
}

/// The model preset to generate with, loaded (or why not).
fn ready(cx: &mut Ctx, s: usize) -> Result<usize, Verdict> {
    let m = cx.model_for(s).map_err(skipped)?;
    cx.ensure_loaded(s, m).map_err(|e| fail(format!("couldn't load a model preset: {e}")))?;
    Ok(m)
}

fn secs(t: Instant) -> String {
    format!("{:.1} s", t.elapsed().as_secs_f32())
}

pub fn capabilities(cx: &mut Ctx, t: Target) -> Verdict {
    let client = tried!(cx.client(tried!(server(t))));
    match client.capabilities() {
        Ok(v) => {
            let what = client.software().unwrap_or_else(|| "sd-server's API".into());
            let modes = v.get("supported_modes").cloned().unwrap_or_default();
            cx.log(format!("capabilities: {}", increart_core::traffic::redacted(&v)));
            let img_gen = modes.as_array().is_none_or(|m| m.iter().any(|x| x == "img_gen"));
            // sd-server's modes come with the model it loads: none before one
            let state = v.pointer("/runtime/state").and_then(|s| s.as_str()).unwrap_or("");
            let empty = matches!(state, "" | "empty") && v.pointer("/runtime/recipe").is_none_or(|r| r.is_null());
            if img_gen {
                pass(format!("{what} at {}, modes {modes}", client.base_url()))
            } else if empty {
                pass(format!("{what} at {}, nothing loaded yet (its modes come with a model)", client.base_url()))
            } else {
                warn(format!("{what}: supported_modes {modes} has no img_gen"))
            }
        }
        Err(e) => fail(format!("GET /sdcpp/v1/capabilities: {e}")),
    }
}

pub fn latency(cx: &mut Ctx, t: Target) -> Verdict {
    let client = tried!(cx.client(tried!(server(t))));
    let mut ms = vec![];
    for _ in 0..10 {
        let at = Instant::now();
        if let Err(e) = client.capabilities() {
            return fail(format!("a GET of capabilities failed: {e}"));
        }
        ms.push(at.elapsed().as_millis() as u64);
    }
    ms.sort();
    let (median, max) = (ms[ms.len() / 2], ms[ms.len() - 1]);
    let text = format!("median {median} ms, slowest {max} ms");
    if median > 1000 { warn(format!("{text}: slow for twice-a-second polls")) } else { pass(text) }
}

pub fn parallel_clients(cx: &mut Ctx, t: Target) -> Verdict {
    let client = tried!(cx.client(tried!(server(t))));
    let at = Instant::now();
    let errors: Vec<String> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let c = client.clone();
                sc.spawn(move || {
                    (0..5).filter_map(|_| c.model_status().err().map(|e| e.to_string())).collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_else(|_| vec!["a thread panicked".into()])).collect()
    });
    match errors.first() {
        None => pass(format!("40 answers to 8 clients in {}", secs(at))),
        Some(e) => fail(format!("{} of 40 failed, first: {e}", errors.len())),
    }
}

pub fn poll_burst(cx: &mut Ctx, t: Target) -> Verdict {
    let client = tried!(cx.client(tried!(server(t))));
    let at = Instant::now();
    let mut slowest = Duration::ZERO;
    for i in 0..100 {
        let one = Instant::now();
        if let Err(e) = client.model_status() {
            return fail(format!("poll {} of 100 failed: {e}", i + 1));
        }
        slowest = slowest.max(one.elapsed());
    }
    let text = format!("100 polls in {}, slowest {} ms", secs(at), slowest.as_millis());
    if slowest > Duration::from_secs(2) { warn(text) } else { pass(text) }
}

pub fn unknown_job(cx: &mut Ctx, t: Target) -> Verdict {
    match tried!(cx.client(tried!(server(t)))).job("increart-apitest-no-such-job") {
        Err(ApiError::JobGone) => pass("404: an unknown job is gone"),
        Err(e) => warn(format!("answered {e}, not 404: the engine reads that as the server failing, not the job gone")),
        Ok(j) => fail(format!("answered a job for an ID it never gave ({})", job_text(&j))),
    }
}

pub fn list(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    match cx.loadable(s) {
        Err(e) => fail(format!("GET /sdcpp/v1/model-files: {e}")),
        Ok(None) => warn("no list (404): any model preset may be sent to it, and fail there if it hasn't the files"),
        Ok(Some(l)) => {
            let (mut can, mut cannot) = (vec![], vec![]);
            for m in &cx.models {
                match l.why_not(&m.key.models) {
                    None => can.push(m.preset.name.clone()),
                    Some(why) => cannot.push(format!("{}: {why}", m.preset.name)),
                }
            }
            if let increart_core::sdserver::Loadable::Files { ids, refused } = &l {
                cx.log(format!("{} files listed", ids.len()));
                for (id, why) in refused {
                    cx.log(format!("listed but refused: {id}: {why}"));
                }
            }
            for c in &cannot {
                cx.log(format!("not {c}"));
            }
            let text = format!(
                "model presets it can load: {}{}",
                if can.is_empty() { "none".into() } else { can.join(", ") },
                if cannot.is_empty() {
                    String::new()
                } else {
                    format!("; {} it can't (the log says why)", cannot.len())
                }
            );
            if can.is_empty() { warn(text) } else { pass(text) }
        }
    }
}

pub fn status(cx: &mut Ctx, t: Target) -> Verdict {
    match tried!(cx.client(tried!(server(t)))).model_status() {
        Ok(st) => {
            let loaded = st.loaded().map(|k| key_text(&k)).unwrap_or_else(|| "nothing".into());
            match st.state.as_str() {
                "empty" | "loading" | "ready" | "error" => pass(format!("{}, loaded {loaded}", st.state)),
                other => warn(format!("state \"{other}\" isn't one of empty, loading, ready, error")),
            }
        }
        Err(e) => fail(format!("GET /sdcpp/v1/models/status: {e}")),
    }
}

pub fn load_each(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let ms = cx.models_for(s);
    if ms.is_empty() {
        return skipped("none of the config's model presets has its files on it");
    }
    let (mut ok, mut failed) = (vec![], vec![]);
    for m in ms {
        if cx.stopped() {
            return skipped("stopped");
        }
        let at = Instant::now();
        match cx.ensure_loaded(s, m) {
            Ok(()) => ok.push(format!("{} ({})", cx.models[m].preset.name, secs(at))),
            Err(e) => {
                cx.log(&e);
                failed.push(e);
            }
        }
    }
    match failed.first() {
        None => pass(format!("loaded {}", ok.join(", "))),
        Some(e) => fail(format!("{} of {} failed: {e}", failed.len(), failed.len() + ok.len())),
    }
}

pub fn load_missing(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let client = tried!(cx.client(s));
    let recipe = cx.models.first().map(|m| m.key.recipe.clone()).unwrap_or_else(|| "qwen-image-21".into());
    let missing = "diffusion_models/increart-apitest-missing.safetensors";
    let key = ModelKey { recipe, models: BTreeMap::from([("diffusion_model".to_string(), missing.to_string())]) };
    let answer = client.load_model(&key);
    let after = client.model_status();
    let verdict = match &answer {
        Ok(st) if st.loaded().is_some_and(|k| k.models.values().any(|f| f.contains("increart-apitest-missing"))) => {
            fail("said it loaded a file that doesn't exist")
        }
        Ok(st) => warn(format!("answered 200 ({}) for a file it doesn't have, not an error", st.state)),
        Err(e) if e.is_server_problem() => {
            warn(format!("answered {e}: read as the server failing (it's retried), not as the file missing"))
        }
        Err(e) => pass(format!("refused: {e}")),
    };
    match after {
        Err(e) => fail(format!("it doesn't answer since ({e})")),
        Ok(_) => verdict,
    }
}

pub fn basic(cx: &mut Ctx, t: Target) -> Verdict {
    let client = tried!(cx.client(tried!(server(t))));
    let models: Vec<(String, ModelKey)> = cx.models.iter().map(|m| (m.preset.name.clone(), m.key.clone())).collect();
    let mut steps: Vec<servertest::Step> = vec![];
    let cx = &*cx;
    servertest::run(&client, &models, Ok(()), cx.cancel, &mut |i, step| {
        if step.outcome != servertest::Outcome::Running {
            cx.log(format!("{}: {:?}: {}", step.title, step.outcome, step.detail));
        }
        if i < steps.len() {
            steps[i] = step;
        } else {
            steps.push(step);
        }
    });
    use servertest::Outcome as O;
    let worst =
        steps.iter().find(|s| s.outcome == O::Failed).or_else(|| steps.iter().find(|s| s.outcome == O::Warning));
    match worst {
        Some(s) if s.outcome == O::Failed => fail(format!("{}: {}", s.title, s.detail)),
        Some(s) => warn(format!("{}: {}", s.title, s.detail)),
        None if steps.iter().any(|s| s.outcome == O::Skipped) => skipped("some steps skipped"),
        None => pass(format!("all {} steps ok", steps.len())),
    }
}

/// Makes one image with `body`; the verdict says how long and its size.
fn one_image(cx: &Ctx, s: usize, body: &serde_json::Value, what: &str) -> Verdict {
    let at = Instant::now();
    let job = match cx.generate(s, body, IMAGE_LIMIT) {
        Ok(j) => j,
        Err(e) => return fail(e),
    };
    match png_of(&job) {
        Ok((png, w, h)) if (w, h) == (512, 512) => {
            pass(format!("{what}: a {w}×{h} PNG ({} KB) in {}", png.len() / 1024, secs(at)))
        }
        Ok((_, w, h)) => warn(format!("{what}: asked 512×512, it made {w}×{h}")),
        Err(e) => fail(e),
    }
}

pub fn txt2img(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let m = match ready(cx, s) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let body = cx.body(m, 42, 4, &[]);
    one_image(cx, s, &body, &cx.models[m].preset.name)
}

pub fn same_seed(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let m = match ready(cx, s) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let body = cx.body(m, 7, 4, &[]);
    let mut pixels = vec![];
    for _ in 0..2 {
        let job = match cx.generate(s, &body, IMAGE_LIMIT) {
            Ok(j) => j,
            Err(e) => return fail(e),
        };
        match png_of(&job).and_then(|(png, ..)| image::load_from_memory(&png).map_err(|e| e.to_string())) {
            Ok(img) => pixels.push(img.to_rgb8().into_raw()),
            Err(e) => return fail(e),
        }
    }
    if pixels[0] == pixels[1] {
        pass("seed 7 twice: the same pixels")
    } else {
        let differ = pixels[0].iter().zip(&pixels[1]).filter(|(a, b)| a != b).count();
        warn(format!(
            "seed 7 twice: {:.1}% of the values differ: a seed made again won't be the same image",
            100.0 * differ as f64 / pixels[0].len().max(1) as f64
        ))
    }
}

pub fn reference(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let m = match ready(cx, s) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let body = cx.body(m, 11, 4, &[test_png(512, false)]);
    one_image(cx, s, &body, "with a 512² reference image")
}

pub fn large_request(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let m = match ready(cx, s) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let refs: Vec<Vec<u8>> = (0..3).map(|_| test_png(1024, true)).collect();
    let body = cx.body(m, 12, 4, &refs);
    let mb = body.to_string().len() as f64 / 1e6;
    cx.log(format!("a request of {mb:.1} MB"));
    let client = tried!(cx.client(s));
    let at = Instant::now();
    match client.img_gen(&body) {
        Ok(id) => {
            let _ = client.cancel(&id);
            let _ = cx.wait(&client, &id, CANCEL_LIMIT, |_| true);
            pass(format!("a {mb:.1} MB request taken in {}", secs(at)))
        }
        Err(ApiError::Http { status: 413, message }) => fail(format!(
            "413 for {mb:.1} MB ({message}): runs with large reference images will fail; raise its body limit"
        )),
        Err(e) => fail(format!("a {mb:.1} MB request: {e}")),
    }
}

pub fn lora(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let can = cx.models_for(s);
    let Some(m) = can.into_iter().find(|&m| cx.models[m].preset.loras.iter().any(|l| !l.file.is_empty())) else {
        return not_applicable("none of the model presets with a LoRA has its files on it");
    };
    if let Err(e) = cx.ensure_loaded(s, m) {
        return fail(e);
    }
    let loras: Vec<_> = cx.models[m]
        .preset
        .loras
        .iter()
        .filter(|l| !l.file.is_empty())
        .map(|l| increart_core::model::LoraFile { id: l.file.clone(), sha256: String::new(), weight: l.weight })
        .collect();
    let mut body = cx.body(m, 13, 4, &[]);
    body["lora"] = increart_core::request::lora_field(&loras);
    one_image(cx, s, &body, &format!("{} with its LoRA", cx.models[m].preset.name))
}

pub fn custom_sigmas(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let can = cx.models_for(s);
    let Some(m) = can.into_iter().find(|&m| !cx.models[m].preset.custom_sigmas.is_empty()) else {
        return not_applicable("none of the model presets with custom sigmas has its files on it");
    };
    if let Err(e) = cx.ensure_loaded(s, m) {
        return fail(e);
    }
    let mut body = cx.body(m, 14, 4, &[]);
    body["sample_params"]["custom_sigmas"] = json!(cx.models[m].preset.custom_sigmas);
    body["sample_params"]["sample_steps"] = json!(cx.models[m].preset.custom_sigmas.len().saturating_sub(1).max(1));
    one_image(cx, s, &body, &format!("{} with its sigmas", cx.models[m].preset.name))
}

pub fn queue(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let m = match ready(cx, s) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let client = tried!(cx.client(s));
    let a = match client.img_gen(&cx.body(m, 21, 8, &[])) {
        Ok(id) => id,
        Err(e) => return fail(format!("the first job: {e}")),
    };
    let b = match client.img_gen(&cx.body(m, 22, 8, &[])) {
        Ok(id) => id,
        Err(e) => {
            let _ = client.cancel(&a);
            return fail(format!("a second job while one runs was refused: {e} (the engine keeps two in flight)"));
        }
    };
    let mut waited = None;
    if let Ok(j) = client.job(&b) {
        waited = Some(job_text(&j));
    }
    let ra = cx.wait(&client, &a, IMAGE_LIMIT, |_| true);
    let rb = cx.wait(&client, &b, IMAGE_LIMIT, |_| true);
    match (ra.and_then(|j| png_of(&j)), rb.and_then(|j| png_of(&j))) {
        (Ok(_), Ok(_)) => pass(format!("both completed; the second was {} at first", waited.unwrap_or_default())),
        (Err(e), _) | (_, Err(e)) => fail(e),
    }
}

pub fn cancel_running(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let m = match ready(cx, s) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let client = tried!(cx.client(s));
    let id = match client.img_gen(&cx.body(m, 31, 30, &[])) {
        Ok(id) => id,
        Err(e) => return fail(format!("POST /sdcpp/v1/img_gen: {e}")),
    };
    // until it generates
    match cx.wait(&client, &id, IMAGE_LIMIT, |j| j.status == JobStatus::Queued) {
        Ok(j) if j.status.is_terminal() => {
            return warn(format!("it ended {} before it could be cancelled", job_text(&j)));
        }
        Ok(_) => {}
        Err(e) => return fail(e),
    }
    let at = Instant::now();
    if let Err(e) = client.cancel(&id) {
        return fail(format!("POST /sdcpp/v1/jobs/{id}/cancel: {e}"));
    }
    match cx.wait(&client, &id, CANCEL_LIMIT, |_| true) {
        Ok(j) if j.status == JobStatus::Cancelled => pass(format!("cancelled {} after asking", secs(at))),
        Ok(j) => warn(format!("it ended {} after the cancel: the cancel didn't stop it", job_text(&j))),
        Err(e) => fail(format!("{e}: a cancel doesn't stop it, and Stop now lets a run go while the server works on")),
    }
}

pub fn cancel_queued(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let m = match ready(cx, s) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let client = tried!(cx.client(s));
    let (a, b) = match (client.img_gen(&cx.body(m, 41, 30, &[])), client.img_gen(&cx.body(m, 42, 30, &[]))) {
        (Ok(a), Ok(b)) => (a, b),
        (Ok(a), Err(e)) => {
            let _ = client.cancel(&a);
            return fail(format!("the second job: {e}"));
        }
        (Err(e), _) => return fail(format!("the first job: {e}")),
    };
    let first = client.job(&b).map(|j| job_text(&j)).unwrap_or_default();
    let at = Instant::now();
    let r = client
        .cancel(&b)
        .map_err(|e| e.to_string())
        .and_then(|_| cx.wait(&client, &b, Duration::from_secs(20), |_| true));
    let a_went_on = client.job(&a).is_ok_and(|j| j.status != JobStatus::Cancelled);
    let _ = client.cancel(&a);
    let _ = cx.wait(&client, &a, CANCEL_LIMIT, |_| true);
    match r {
        Ok(j) if j.status == JobStatus::Cancelled && a_went_on => {
            pass(format!("the second job ({first}) cancelled in {}; the first went on", secs(at)))
        }
        Ok(j) if j.status == JobStatus::Cancelled => warn("the second job cancelled, but the first ended too"),
        Ok(j) => warn(format!("the second job ({first}) ended {} after the cancel", job_text(&j))),
        Err(e) => fail(e),
    }
}

pub fn after_cancel(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let m = match ready(cx, s) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let client = tried!(cx.client(s));
    if let Ok(id) = client.img_gen(&cx.body(m, 51, 30, &[])) {
        let _ = cx.wait(&client, &id, IMAGE_LIMIT, |j| j.status == JobStatus::Queued);
        let _ = client.cancel(&id);
        let _ = cx.wait(&client, &id, CANCEL_LIMIT, |_| true);
    }
    let body = cx.body(m, 52, 4, &[]);
    let v = one_image(cx, s, &body, "after a cancel");
    if v.outcome == Outcome::Fail { fail(format!("{}: a cancel left it stuck?", v.summary)) } else { v }
}

pub fn poll_finished(cx: &mut Ctx, t: Target) -> Verdict {
    let s = tried!(server(t));
    let m = match ready(cx, s) {
        Ok(m) => m,
        Err(v) => return v,
    };
    let job = match cx.generate(s, &cx.body(m, 61, 4, &[]), IMAGE_LIMIT) {
        Ok(j) => j,
        Err(e) => return fail(e),
    };
    if let Err(e) = png_of(&job) {
        return fail(e);
    }
    std::thread::sleep(Duration::from_secs(5));
    match tried!(cx.client(s)).job(&job.id) {
        Ok(j) if png_of(&j).is_ok() => pass("5 s later it still answers completed, with the image"),
        Ok(j) => warn(format!("5 s later it answers {} without the image", job_text(&j))),
        Err(ApiError::JobGone) => {
            warn("gone 5 s after completing: an image read a poll late is made again (sd-server keeps jobs minutes)")
        }
        Err(e) => fail(format!("polled again: {e}")),
    }
}
