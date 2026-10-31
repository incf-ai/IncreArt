//! **Test** on a server the app connects to (DESIGN.md, Testing a server):
//! the calls the engine makes, one step at a time, each saying what came
//! back and what IncreArt makes of it. For whoever runs the server, a proxy
//! in front of sd-server especially: where it answers otherwise than the
//! engine needs, it says so there, not in a run that never moves.
//!
//! It answers; its model list (and the model presets it can load); its
//! status; then, unless told not to (the engine uses it), it loads a
//! model preset, makes a small image and cancels a job. Each step runs
//! through [`Client`], as the engine's do, so an sd-server, a proxy and
//! a KoboldCpp started elsewhere are tested alike.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::sdserver::{ApiError, Client, JobStatus, Loadable, ModelKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Running,
    Ok,
    /// It works, but not as IncreArt would like (a run may wait, or go
    /// where it shouldn't).
    Warning,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub title: String,
    pub outcome: Outcome,
    pub detail: String,
}

/// How long the test image and the cancelled job may take.
const IMAGE_LIMIT: Duration = Duration::from_secs(10 * 60);
const CANCEL_LIMIT: Duration = Duration::from_secs(2 * 60);

const TITLES: [&str; 6] =
    ["Answers", "Model list", "Status", "Loads a model preset", "Makes a small image", "Cancels a job"];

/// Runs the test. `models`: the model presets, by name; `generate`:
/// `Err(why)` to skip loading and generating. `report(index, step)` is
/// called as each step starts and ends; `cancel` stops it between calls
/// (a load under way is waited for).
pub fn run(
    client: &Client,
    models: &[(String, ModelKey)],
    generate: Result<(), String>,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(usize, Step),
) {
    let mut t = Test { client, cancel, report, at: 0 };
    let Some(()) = t.answers() else { return t.skip_rest("it didn't answer") };
    let Some(loadable) = t.list(models) else { return t.skip_rest("its model list failed") };
    let Some(loaded) = t.status() else { return t.skip_rest("its status failed") };
    if let Err(why) = generate {
        return t.skip_rest(&why);
    }
    let pick = models
        .iter()
        .filter(|(_, m)| loadable.as_ref().is_none_or(|l| l.runs(&m.models)))
        .min_by_key(|(_, m)| Some(m) != loaded.as_ref());
    let Some((name, model)) = pick else {
        return t.skip_rest("none of the model presets' files are on it");
    };
    let Some(()) = t.load(name, model) else { return t.skip_rest("the load didn't work") };
    let Some(()) = t.image() else { return t.skip_rest("the image didn't work") };
    t.cancel_job();
}

struct Test<'a> {
    client: &'a Client,
    cancel: &'a AtomicBool,
    report: &'a mut dyn FnMut(usize, Step),
    /// The step under way.
    at: usize,
}

impl Test<'_> {
    fn start(&mut self) {
        let step = Step { title: TITLES[self.at].into(), outcome: Outcome::Running, detail: String::new() };
        (self.report)(self.at, step);
    }

    /// Ends the step under way; `Some(())` to go on (not failed, not
    /// stopped).
    fn end(&mut self, outcome: Outcome, detail: impl Into<String>) -> Option<()> {
        let step = Step { title: TITLES[self.at].into(), outcome, detail: detail.into() };
        (self.report)(self.at, step);
        self.at += 1;
        (outcome != Outcome::Failed && !self.stopped()).then_some(())
    }

    fn skip_rest(&mut self, why: &str) {
        let why = if self.stopped() { "stopped" } else { why };
        while self.at < TITLES.len() {
            self.end(Outcome::Skipped, why);
        }
    }

    fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn answers(&mut self) -> Option<()> {
        self.start();
        match self.client.capabilities() {
            Ok(v) => {
                let what = self.client.software().unwrap_or_else(|| "sd-server's API".into());
                let modes = v.get("supported_modes").map(Value::to_string).unwrap_or_default();
                self.end(Outcome::Ok, format!("{what} at {}{}", self.client.base_url(), with(" modes ", &modes)))
            }
            Err(e) => self.end(
                Outcome::Failed,
                format!("{} doesn't answer GET /sdcpp/v1/capabilities: {e}", self.client.base_url()),
            ),
        }
    }

    /// Its list (`None` inside: it has none, any model may go to it).
    fn list(&mut self, models: &[(String, ModelKey)]) -> Option<Option<Loadable>> {
        self.start();
        let fits = |l: Option<&Loadable>| {
            let (can, cannot): (Vec<_>, Vec<_>) = models.iter().partition(|(_, m)| l.is_none_or(|l| l.runs(&m.models)));
            let names = |v: &[&(String, ModelKey)]| v.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ");
            let why = cannot
                .iter()
                .filter_map(|(n, m)| Some(format!("{n}: {}", l?.why_not(&m.models)?)))
                .collect::<Vec<_>>()
                .join("; ");
            (can.is_empty(), format!("model presets it can load: {}{}", or_none(&names(&can)), with("; not ", &why)))
        };
        match self.client.loadable() {
            Ok(l) => {
                let what = match &l {
                    Loadable::Files { ids, refused } => {
                        let refused = if refused.is_empty() {
                            String::new()
                        } else {
                            format!(", {} it can't load", refused.len())
                        };
                        format!("{} file{}{refused}", ids.len(), if ids.len() == 1 { "" } else { "s" })
                    }
                    Loadable::Only(Some(f)) => format!(
                        "it runs {f}, started elsewhere: it can't switch, and is matched by that file's name only"
                    ),
                    Loadable::Only(None) => "it runs no image model, and can't load one (started without admin)".into(),
                };
                let (none, presets) = fits(Some(&l));
                let outcome = if none { Outcome::Warning } else { Outcome::Ok };
                self.end(outcome, format!("{what}; {presets}"))?;
                Some(Some(l))
            }
            Err(ApiError::Http { status: 404, .. }) => {
                let (_, presets) = fits(None);
                self.end(
                    Outcome::Warning,
                    format!(
                        "no GET /sdcpp/v1/model-files (404): IncreArt can't tell which files it has, so a run of any \
                         model preset may go to it; {presets}"
                    ),
                )?;
                Some(None)
            }
            Err(e) => self.end(Outcome::Failed, format!("GET /sdcpp/v1/model-files failed: {e}")).map(|_| None),
        }
    }

    /// The model it has loaded.
    fn status(&mut self) -> Option<Option<ModelKey>> {
        self.start();
        match self.client.model_status() {
            Ok(st) => {
                let loaded = st.loaded();
                let what = json!({ "state": st.state, "recipe": st.recipe, "models": st.models });
                let note = match (&*st.state, &loaded) {
                    ("ready", None) => " (ready without a recipe: taken as nothing loaded)",
                    ("ready", Some(_)) | ("empty", _) => "",
                    _ => " (not ready: runs wait for it)",
                };
                self.end(Outcome::Ok, format!("{what}{note}"))?;
                Some(loaded)
            }
            Err(e) => self.end(Outcome::Failed, format!("GET /sdcpp/v1/models/status failed: {e}")).map(|_| None),
        }
    }

    fn load(&mut self, name: &str, model: &ModelKey) -> Option<()> {
        self.start();
        let t = Instant::now();
        let asked = json!({ "recipe": model.recipe, "models": model.models });
        match self.client.load_model(model) {
            Ok(st) if st.loaded().as_ref() == Some(model) => {
                self.end(Outcome::Ok, format!("{name}: {asked} loaded in {:.1} s", t.elapsed().as_secs_f32()))
            }
            Ok(st) => {
                let answered = json!({ "state": st.state, "recipe": st.recipe, "models": st.models });
                self.end(
                    Outcome::Failed,
                    format!(
                        "{name}: asked to load {asked}, it answered {answered}. The answer must be state \"ready\" \
                         with the recipe and the files as asked, or IncreArt takes the load as failed and sends no \
                         run of {name} there"
                    ),
                )
            }
            Err(e) => self.end(Outcome::Failed, format!("{name}: loading {asked} failed: {e}")),
        }
    }

    fn image(&mut self) -> Option<()> {
        self.start();
        let t = Instant::now();
        let id = match self.client.img_gen(&crate::request::warmup_body()) {
            Ok(id) => id,
            Err(e) => return self.end(Outcome::Failed, format!("POST /sdcpp/v1/img_gen failed: {e}")),
        };
        let mut seen = vec![];
        let job = loop {
            if self.stopped() {
                let _ = self.client.cancel(&id);
                return self.end(Outcome::Skipped, "stopped");
            }
            match self.client.job(&id) {
                Ok(job) => {
                    let s = status_name(job.status);
                    if seen.last() != Some(&s) {
                        seen.push(s);
                    }
                    if job.status.is_terminal() {
                        break job;
                    }
                }
                Err(e) => return self.end(Outcome::Failed, format!("job {id}: GET /sdcpp/v1/jobs/{id} failed: {e}")),
            }
            if t.elapsed() > IMAGE_LIMIT {
                let _ = self.client.cancel(&id);
                return self.end(
                    Outcome::Failed,
                    format!(
                        "job {id}: still {} after {} min ({})",
                        seen.last().unwrap(),
                        IMAGE_LIMIT.as_secs() / 60,
                        seen.join(" → ")
                    ),
                );
            }
            std::thread::sleep(Duration::from_millis(500));
        };
        let path = seen.join(" → ");
        if job.status != JobStatus::Completed {
            let why = job.error.as_ref().map(|e| format!("{}: {}", e.code, e.message)).unwrap_or_default();
            return self.end(Outcome::Failed, format!("job {id}: {path}{}", with(": ", &why)));
        }
        match job.image() {
            Ok(png) if png.starts_with(b"\x89PNG\r\n\x1a\n") => self.end(
                Outcome::Ok,
                format!(
                    "512 × 512, 1 step: job {id}, {path} in {:.1} s, a {:.1} KB PNG",
                    t.elapsed().as_secs_f32(),
                    png.len() as f32 / 1024.0
                ),
            ),
            Ok(_) => self.end(Outcome::Failed, format!("job {id}: {path}, but its image isn't a PNG")),
            Err(e) => self.end(Outcome::Failed, format!("job {id}: {path}, but {e}")),
        }
    }

    fn cancel_job(&mut self) -> Option<()> {
        self.start();
        let mut body = crate::request::warmup_body();
        body["seed"] = json!(1);
        body["sample_params"]["sample_steps"] = json!(30);
        let id = match self.client.img_gen(&body) {
            Ok(id) => id,
            Err(e) => return self.end(Outcome::Failed, format!("POST /sdcpp/v1/img_gen failed: {e}")),
        };
        if let Err(e) = self.client.cancel(&id) {
            return self.end(Outcome::Failed, format!("job {id}: POST /sdcpp/v1/jobs/{id}/cancel failed: {e}"));
        }
        let t = Instant::now();
        loop {
            match self.client.job(&id) {
                Ok(job) if job.status == JobStatus::Cancelled => {
                    return self.end(Outcome::Ok, format!("job {id} cancelled in {:.1} s", t.elapsed().as_secs_f32()));
                }
                Ok(job) if job.status.is_terminal() => {
                    return self.end(
                        Outcome::Warning,
                        format!("job {id} ended {} after the cancel: it didn't stop it", status_name(job.status)),
                    );
                }
                Ok(job) if t.elapsed() > CANCEL_LIMIT => {
                    return self.end(
                        Outcome::Failed,
                        format!(
                            "job {id}: still {} {} min after the cancel; Stop now lets a run go, but the server keeps working",
                            status_name(job.status),
                            CANCEL_LIMIT.as_secs() / 60
                        ),
                    );
                }
                Ok(_) if self.stopped() => return self.end(Outcome::Skipped, "stopped"),
                Ok(_) => std::thread::sleep(Duration::from_millis(500)),
                Err(e) => return self.end(Outcome::Failed, format!("job {id}: GET /sdcpp/v1/jobs/{id} failed: {e}")),
            }
        }
    }
}

fn status_name(s: JobStatus) -> &'static str {
    match s {
        JobStatus::Queued => "queued",
        JobStatus::Generating => "generating",
        JobStatus::Completed => "completed",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
    }
}

fn with(prefix: &str, s: &str) -> String {
    if s.is_empty() { String::new() } else { format!("{prefix}{s}") }
}

fn or_none(s: &str) -> &str {
    if s.is_empty() { "none" } else { s }
}

/// The client a test of `server` talks through, as the launcher builds it;
/// None for a server the app starts (tested by opening it); an error if
/// the client's thread couldn't start.
pub fn client_for(server: &crate::config::ServerConfig) -> Option<std::io::Result<Client>> {
    use crate::config::ServerConfig;
    use crate::kobold::KoboldRun;
    match server {
        ServerConfig::Connect { url, .. } => Some(Ok(Client::new(url))),
        ServerConfig::Kobold(k) => match &k.run {
            KoboldRun::Connect { url, password, .. } => Some(Client::kobold(url, password, None)),
            KoboldRun::Start { .. } => None,
        },
        _ => None,
    }
}

/// The steps as text, for Copy.
pub fn as_text(server: &str, steps: &[Step]) -> String {
    let mut out = format!("Test of {server}\n");
    for s in steps {
        let mark = match s.outcome {
            Outcome::Running => "…",
            Outcome::Ok => "ok",
            Outcome::Warning => "warning",
            Outcome::Failed => "FAILED",
            Outcome::Skipped => "skipped",
        };
        out.push_str(&format!("{}: {mark}{}\n", s.title, with(": ", &s.detail)));
    }
    out
}
