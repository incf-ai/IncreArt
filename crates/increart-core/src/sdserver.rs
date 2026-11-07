//! Blocking client for sd-server's native API, `/sdcpp/v1/...` (DESIGN.md,
//! Backend). One `Client` per server; it is cheap to clone.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ureq::Agent;

use crate::model::ModelSelection;
use crate::sync::LockExt;

/// `models/load` blocks until the load is done (minutes for Qwen-Image).
const LOAD_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// Response bodies: a 2048² RGBA PNG is ~16 MB, ~22 MB as base64.
const BODY_LIMIT: u64 = 256 << 20;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ApiError {
    /// 503 `model_not_loaded`.
    #[error("no model is loaded")]
    ModelNotLoaded,
    /// 503 `model_loading`.
    #[error("the server is loading or unloading a model")]
    ModelLoading,
    /// 409 `model_busy`: jobs still pending, or another model operation.
    #[error("model busy: {0}")]
    ModelBusy(String),
    /// 400 `invalid_model_config`: the previous model stays loaded.
    #[error("invalid model config: {0}")]
    InvalidModelConfig(String),
    /// 422 `model_load_failed`: the server is left with no model.
    #[error("model load failed: {0}")]
    ModelLoadFailed(String),
    /// Any other 400/422: a bug in the request; fail the seed, keep the run.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// 429: more than the server's queue limit pending.
    #[error("the server's job queue is full")]
    QueueFull,
    /// 404/410 from a job endpoint: unknown, or purged after its TTL.
    #[error("job not found (unknown or expired)")]
    JobGone,
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    /// Connection refused, timeout, unreadable response.
    #[error("{0}")]
    Transport(String),
}

impl ApiError {
    /// Errors that mean the server is unwell (back off), as opposed to
    /// errors about one request.
    pub fn is_server_problem(&self) -> bool {
        matches!(self, ApiError::Transport(_) | ApiError::ModelLoading | ApiError::QueueFull)
            || matches!(self, ApiError::Http { status, .. } if *status >= 500)
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Maps an error response to an [`ApiError`]. `job`: the request was to a
/// `jobs/{id}` endpoint (so 404/410 mean the job is gone).
pub fn classify(status: u16, body: &str, job: bool) -> ApiError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let message = match (text("error"), text("message")) {
        (Some(e), Some(m)) => format!("{e}: {m}"),
        (Some(e), None) => e,
        (None, Some(m)) => m,
        (None, None) => body.chars().take(500).collect(),
    };
    match (status, text("code").as_deref()) {
        (_, Some("model_not_loaded")) => ApiError::ModelNotLoaded,
        (_, Some("model_loading")) => ApiError::ModelLoading,
        (_, Some("model_busy")) => ApiError::ModelBusy(message),
        (_, Some("invalid_model_config")) => ApiError::InvalidModelConfig(message),
        (_, Some("model_load_failed")) => ApiError::ModelLoadFailed(message),
        (404 | 410, _) if job => ApiError::JobGone,
        (400 | 422, _) => ApiError::BadRequest(message),
        (429, _) => ApiError::QueueFull,
        _ => ApiError::Http { status, message },
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct ModelStatus {
    /// `empty | loading | ready | error`.
    pub state: String,
    #[serde(default)]
    pub recipe: Option<String>,
    /// Slot → file ID.
    #[serde(default)]
    pub models: BTreeMap<String, String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub load_elapsed_seconds: Option<f64>,
}

impl ModelStatus {
    /// The loaded model, if one is ready.
    pub fn loaded(&self) -> Option<ModelKey> {
        match (&*self.state, &self.recipe) {
            ("ready", Some(recipe)) => Some(ModelKey { recipe: recipe.clone(), models: self.models.clone() }),
            _ => None,
        }
    }

    /// Whether this answer to a load of `asked` says it's loaded: ready
    /// with the recipe and slots as asked, or the same files under the
    /// server's own recipe and slot names (a proxy in front of ComfyUI or
    /// A1111 has one recipe of its own: imgproxy answers recipe
    /// "checkpoint", slot "model").
    pub fn holds(&self, asked: &ModelKey) -> bool {
        self.loaded().is_some_and(|k| k == *asked || k.same_files(asked))
    }

    /// How this answer to a load of `asked` differs from it, said plainly
    /// (for whoever writes a server or proxy): the state, the recipe, then
    /// each slot missing, extra or holding another file. Empty when it
    /// matches.
    pub fn mismatch(&self, asked: &ModelKey) -> Vec<String> {
        let q = |s: &str| format!("\"{s}\"");
        let mut said = Vec::new();
        if self.state != "ready" {
            let error = self.error.as_deref().filter(|e| !e.is_empty()).map_or(String::new(), |e| format!(": {e}"));
            said.push(format!("state {} instead of \"ready\"{error}", q(&self.state)));
        }
        match &self.recipe {
            Some(r) if *r == asked.recipe => {}
            Some(r) => said.push(format!("recipe {} instead of {}", q(r), q(&asked.recipe))),
            None => said.push(format!("no recipe (asked {})", q(&asked.recipe))),
        }
        let extra: Vec<&String> = self.models.keys().filter(|k| !asked.models.contains_key(*k)).collect();
        let mut renamed = Vec::new();
        for (slot, file) in &asked.models {
            match self.models.get(slot) {
                Some(f) if f == file => {}
                Some(f) => said.push(format!("slot {} holds {} instead of {}", q(slot), q(f), q(file))),
                None => {
                    // the same file under another name: a slot renamed
                    match extra.iter().find(|k| !renamed.contains(*k) && self.models.get(**k) == Some(file)) {
                        Some(k) => {
                            renamed.push(*k);
                            said.push(format!("slot {} instead of {} for {}", q(k), q(slot), q(file)))
                        }
                        None => said.push(format!("no slot {} (asked {})", q(slot), q(file))),
                    }
                }
            }
        }
        for k in extra.into_iter().filter(|k| !renamed.contains(k)) {
            said.push(format!("slot {} not asked for", q(k)));
        }
        said
    }
}

/// What a server has loaded, or should load: recipe + slot → file ID. Two
/// runs can share a server without a reload exactly when their keys match.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ModelKey {
    pub recipe: String,
    pub models: BTreeMap<String, String>,
}

impl ModelKey {
    /// The same files (one or more), whatever the recipe and slots are
    /// called.
    pub fn same_files(&self, other: &ModelKey) -> bool {
        fn files(k: &ModelKey) -> Vec<&String> {
            let mut v: Vec<&String> = k.models.values().collect();
            v.sort();
            v
        }
        !self.models.is_empty() && files(self) == files(other)
    }
}

impl From<&ModelSelection> for ModelKey {
    fn from(m: &ModelSelection) -> ModelKey {
        ModelKey { recipe: m.recipe.clone(), models: m.files.iter().map(|(k, f)| (k.clone(), f.id.clone())).collect() }
    }
}

/// What a server can load, for the model presets it can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loadable {
    /// Model file IDs (sd-server's `model-files`, or the files in a started
    /// KoboldCpp's model folders), and those it has but can't load, with
    /// why. `second_choice`: listed files it loads less well (why), left
    /// to a server that lists the model's files without that, if one does.
    /// `by_name`: a file it doesn't list by its ID is the one it lists of
    /// the same file name (sd-server's API: its [`Client`] loads it by its
    /// own ID; a proxy or another folder layout lists `x.gguf` for
    /// `diffusion_models/x.gguf`).
    Files {
        ids: BTreeSet<String>,
        refused: BTreeMap<String, String>,
        second_choice: BTreeMap<String, String>,
        by_name: bool,
    },
    /// KoboldCpp started elsewhere: only the diffusion model it runs (its
    /// file name), if any.
    Only(Option<String>),
}

impl Loadable {
    pub fn files(ids: impl IntoIterator<Item = String>) -> Loadable {
        Loadable::Files {
            ids: ids.into_iter().collect(),
            refused: BTreeMap::new(),
            second_choice: BTreeMap::new(),
            by_name: false,
        }
    }

    /// As sd-server's API lists them: matched by file name too.
    pub fn listed(ids: impl IntoIterator<Item = String>) -> Loadable {
        match Loadable::files(ids) {
            Loadable::Files { ids, refused, second_choice, .. } => {
                Loadable::Files { ids, refused, second_choice, by_name: true }
            }
            l => l,
        }
    }

    /// Its ID for a file IncreArt names `id`: `id` when it lists it, else
    /// (by name) the one file it lists of the same name; None when it has
    /// none (or, by name, several).
    pub fn id_of<'a>(&'a self, id: &'a str) -> Option<&'a str> {
        let Loadable::Files { ids, refused, by_name, .. } = self else { return None };
        if ids.contains(id) || refused.contains_key(id) || std::path::Path::new(id).is_absolute() {
            return Some(id);
        }
        if !by_name {
            return None;
        }
        by_file_name(ids.iter().chain(refused.keys()).map(String::as_str), id)
    }

    /// Why another server that lists a preset's files should load it
    /// instead, if one should (a file it loads less well).
    pub fn second_choice(&self, files: &BTreeMap<String, String>) -> Option<String> {
        let Loadable::Files { second_choice, .. } = self else { return None };
        let name = |p: &str| std::path::Path::new(p).file_name().and_then(|f| f.to_str()).unwrap_or(p).to_string();
        files.values().find_map(|id| second_choice.get(self.id_of(id)?).map(|why| format!("{} {why}", name(id))))
    }

    /// Whether it has a file of a slot and can load it; a second choice
    /// counts only with `second`.
    pub fn has(&self, slot: &str, id: &str, second: bool) -> bool {
        let name = |p: &str| std::path::Path::new(p).file_name().and_then(|f| f.to_str()).unwrap_or(p).to_string();
        match self {
            Loadable::Files { refused, second_choice, .. } => self
                .id_of(id)
                .is_some_and(|their| !refused.contains_key(their) && (second || !second_choice.contains_key(their))),
            Loadable::Only(running) => {
                matches!(slot, "diffusion_model" | "model") && running.as_deref().is_some_and(|r| name(r) == name(id))
            }
        }
    }

    /// A preset's files as it would load them: each slot's first choice it
    /// has, one it loads less well only if it has no other.
    pub fn pick(&self, p: &crate::config::RecipePreset) -> BTreeMap<String, String> {
        p.files
            .iter()
            .map(|(slot, id)| {
                let choices: Vec<&String> = p.choices(slot).collect();
                let pick = choices
                    .iter()
                    .find(|f| self.has(slot, f, false))
                    .or_else(|| choices.iter().find(|f| self.has(slot, f, true)))
                    .copied()
                    .unwrap_or(id);
                (slot.clone(), pick.clone())
            })
            .collect()
    }

    /// Whether it can load a preset, with one of each slot's choices.
    pub fn runs_preset(&self, p: &crate::config::RecipePreset) -> bool {
        self.runs(&self.pick(p))
    }

    /// Why it can't load a preset with any of its choices.
    pub fn why_not_preset(&self, p: &crate::config::RecipePreset) -> Option<String> {
        self.why_not(&self.pick(p))
    }

    /// Whether it can load all of a preset's files (slot → file ID).
    pub fn runs(&self, files: &BTreeMap<String, String>) -> bool {
        self.why_not(files).is_none()
    }

    /// Why it can't load a preset's files, if it can't.
    pub fn why_not(&self, files: &BTreeMap<String, String>) -> Option<String> {
        let name = |p: &str| std::path::Path::new(p).file_name().and_then(|f| f.to_str()).unwrap_or(p).to_string();
        match self {
            Loadable::Files { ids, refused, by_name, .. } => files.values().find_map(|id| match self.id_of(id) {
                Some(their) => refused.get(their).map(|why| format!("{} {why}", name(id))),
                None if *by_name && ids.iter().filter(|l| name(l) == name(id)).count() > 1 => {
                    Some(format!("it lists more than one {}, so which is {id} isn't known", name(id)))
                }
                None => Some(format!("it doesn't have {id}")),
            }),
            Loadable::Only(running) => {
                let wanted = files.get("diffusion_model").or(files.get("model")).map(|f| name(f));
                match (running, wanted) {
                    (Some(r), Some(w)) if name(r) == w => None,
                    (Some(r), _) => Some(format!("it runs {} and can't switch (started without admin)", name(r))),
                    (None, _) => Some("it runs no image model and can't load one (started without admin)".into()),
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ModelFileEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub relative_path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Generating,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Job {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    pub status: JobStatus,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub started: Option<i64>,
    #[serde(default)]
    pub completed: Option<i64>,
    #[serde(default)]
    pub queue_position: i64,
    #[serde(default)]
    pub result: Option<JobResult>,
    #[serde(default, deserialize_with = "job_error")]
    pub error: Option<JobError>,
    #[serde(default)]
    pub preview: Option<Preview>,
    #[serde(default)]
    pub cancel_requested: bool,
}

impl Job {
    /// Generation time in seconds (`completed - started`).
    pub fn elapsed_s(&self) -> Option<f64> {
        Some((self.completed? - self.started?) as f64 / 1000.0)
    }

    /// The first result image, decoded from base64.
    pub fn image(&self) -> ApiResult<Vec<u8>> {
        use base64::Engine;
        let img = self.result.as_ref().and_then(|r| r.images.first());
        let img = img.ok_or_else(|| ApiError::Transport(format!("job {} completed without an image", self.id)))?;
        base64::engine::general_purpose::STANDARD
            .decode(&img.b64_json)
            .map_err(|e| ApiError::Transport(format!("job {}: bad base64 image: {e}", self.id)))
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct JobResult {
    #[serde(default)]
    pub output_format: String,
    #[serde(default)]
    pub images: Vec<ResultImage>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ResultImage {
    #[serde(default)]
    pub index: u32,
    pub b64_json: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct JobError {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
}

/// `error` is an object, but tolerate a bare string.
fn job_error<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<JobError>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(JobError { code: String::new(), message: s }),
        Some(v) => Some(serde_json::from_value(v).map_err(serde::de::Error::custom)?),
    })
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Preview {
    #[serde(default)]
    pub pass: u32,
    #[serde(default)]
    pub step: u32,
    #[serde(default)]
    pub total_steps: u32,
    #[serde(default)]
    pub b64_json: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Submitted {
    pub id: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Clone)]
pub struct Client {
    base: String,
    agent: Agent,
    load_agent: Agent,
    /// KoboldCpp behind the same calls, instead of sd-server.
    kobold: Option<std::sync::Arc<crate::kobold_image::KoboldImage>>,
    /// Its IDs for files IncreArt names otherwise (found by name when
    /// loading: its ID → IncreArt's), its answers read back through them.
    names: std::sync::Arc<std::sync::Mutex<BTreeMap<String, String>>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Client({})", self.base)
    }
}

/// `total`: a cap on the whole request; `stall`: on each read and write
/// (restarted by every byte: submissions carry base64 ref images and
/// results base64 PNGs, slow over a slow link but coming).
fn agent(total: Option<Duration>, stall: Option<Duration>) -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(total)
        .timeout_per_read(stall)
        .timeout_per_write(stall)
        .timeout_connect(Some(Duration::from_secs(5)))
        .build()
        .into()
}

impl Client {
    /// `base`: e.g. `http://127.0.0.1:1234` (no trailing `/sdcpp/v1`).
    pub fn new(base: &str) -> Client {
        Client {
            base: base.trim_end_matches('/').to_string(),
            agent: agent(None, Some(crate::config::timeouts().request())),
            // nothing comes until it's loaded: a cap, not a stall
            load_agent: agent(Some(LOAD_TIMEOUT), None),
            kobold: None,
            names: Default::default(),
        }
    }

    /// A KoboldCpp image server (DESIGN.md, KoboldCpp).
    pub fn kobold(base: &str, password: &str, admin: Option<crate::kobold_image::Admin>) -> std::io::Result<Client> {
        let kobold = crate::kobold_image::KoboldImage::new(base, password, admin)?;
        Ok(Client { kobold: Some(std::sync::Arc::new(kobold)), ..Client::new(base) })
    }

    pub fn is_kobold(&self) -> bool {
        self.kobold.is_some()
    }

    /// What generates, for recipes, when it isn't the configured sd-server
    /// (`koboldcpp <version>`).
    pub fn software(&self) -> Option<String> {
        self.kobold.as_ref().and_then(|k| k.software())
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}/sdcpp/v1/{path}", self.base)
    }

    /// GETs `url` (POSTs `body` there when given) with `agent`, written
    /// to the [traffic log](crate::traffic); `job`: a `jobs/{id}` endpoint.
    fn call<T: DeserializeOwned>(agent: &Agent, url: &str, body: Option<&Value>, job: bool) -> ApiResult<T> {
        let t = std::time::Instant::now();
        let resp = match body {
            Some(b) => agent.post(url).send_json(b),
            None => agent.get(url).call(),
        };
        let answer = resp.map_err(|e| e.to_string()).and_then(|mut r| {
            let status = r.status().as_u16();
            let text = r.body_mut().with_config().limit(BODY_LIMIT).read_to_string().map_err(|e| e.to_string())?;
            Ok((status, text))
        });
        crate::traffic::exchange(if body.is_some() { "POST" } else { "GET" }, url, body, &answer, t.elapsed());
        let (status, text) = answer.map_err(ApiError::Transport)?;
        if !(200..300).contains(&status) {
            return Err(classify(status, &text, job));
        }
        serde_json::from_str(&text).map_err(|e| ApiError::Transport(format!("unexpected response ({e}): {text:.200}")))
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        Self::call(&self.agent, &self.url(path), None, path.starts_with("jobs/"))
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> ApiResult<T> {
        let body = serde_json::to_value(body).map_err(|e| ApiError::BadRequest(e.to_string()))?;
        Self::call(&self.agent, &self.url(path), Some(&body), path.starts_with("jobs/"))
    }

    /// Health check and feature discovery (also reports `runtime` status).
    pub fn capabilities(&self) -> ApiResult<Value> {
        if let Some(k) = &self.kobold {
            return k.capabilities();
        }
        self.get("capabilities")
    }

    pub fn recipes(&self) -> ApiResult<Value> {
        self.get("recipes")
    }

    pub fn model_files(&self) -> ApiResult<Vec<ModelFileEntry>> {
        #[derive(Deserialize)]
        struct Files {
            files: Vec<ModelFileEntry>,
        }
        Ok(self.get::<Files>("model-files")?.files)
    }

    /// Whether it runs on this machine (its loads share this RAM).
    pub fn is_local(&self) -> bool {
        let host = self.base_url().split("://").nth(1).unwrap_or_default();
        let host = host.split('/').next().unwrap_or_default();
        let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
        matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "0.0.0.0")
    }

    /// The RAM loading `model` pins beyond what the server holds now, in
    /// bytes (KoboldCpp offloading its weights; 0 when unknown).
    pub fn load_ram_need(&self, model: &ModelKey) -> u64 {
        self.kobold.as_ref().map_or(0, |k| k.load_ram_need(model))
    }

    /// What it can load (see [`Loadable`]).
    pub fn loadable(&self) -> ApiResult<Loadable> {
        if let Some(k) = &self.kobold {
            return k.loadable();
        }
        Ok(Loadable::listed(self.model_files()?.into_iter().map(|f| portable_id(&f.id).to_string())))
    }

    pub fn refresh_models(&self) -> ApiResult<Value> {
        self.post("models/refresh", &serde_json::json!({}))
    }

    pub fn model_status(&self) -> ApiResult<ModelStatus> {
        if let Some(k) = &self.kobold {
            return k.model_status();
        }
        self.get("models/status").map(|s| self.ours(portable_status(s)))
    }

    /// A status with IncreArt's IDs for the files it was asked for by
    /// another (see `names`).
    fn ours(&self, mut s: ModelStatus) -> ModelStatus {
        let names = self.names.lock_or_recover();
        for id in s.models.values_mut() {
            if let Some(ours) = names.get(id.as_str()) {
                *id = ours.clone();
            }
        }
        s
    }

    /// Loads a model; blocks until the load has finished.
    pub fn load_model(&self, model: &ModelKey) -> ApiResult<ModelStatus> {
        if let Some(k) = &self.kobold {
            return k.load_model(model);
        }
        // its names for files outside its first --model-dir
        // (as they are when it can't list them: its load says what's wrong)
        let listed: Vec<String> = self.model_files().map(|f| f.into_iter().map(|f| f.id).collect()).unwrap_or_default();
        let models: BTreeMap<&String, String> =
            model.models.iter().map(|(slot, id)| (slot, server_id(&listed, id))).collect();
        {
            // found by another name: its answers read back as asked
            let mut names = self.names.lock_or_recover();
            for (slot, theirs) in &models {
                let ours = &model.models[*slot];
                let theirs = portable_id(theirs);
                if theirs != ours {
                    names.insert(theirs.to_string(), ours.clone());
                }
            }
        }
        let body = serde_json::json!({ "recipe": model.recipe, "models": models });
        Self::call(&self.load_agent, &self.url("models/load"), Some(&body), false)
            .map(|s| self.ours(portable_status(s)))
    }

    pub fn unload_model(&self) -> ApiResult<ModelStatus> {
        if let Some(k) = &self.kobold {
            return k.unload_model();
        }
        Self::call(&self.load_agent, &self.url("models/unload"), Some(&serde_json::json!({})), false)
            .map(|s| self.ours(portable_status(s)))
    }

    /// Submits an `img_gen` job; returns its ID.
    pub fn img_gen(&self, body: &Value) -> ApiResult<String> {
        if let Some(k) = &self.kobold {
            return k.img_gen(body);
        }
        match body.get("lora").and_then(Value::as_array).filter(|l| !l.is_empty()) {
            Some(loras) => {
                // its names for LoRAs outside its first --model-dir's loras/
                #[derive(Deserialize)]
                struct Lora {
                    path: String,
                }
                let listed: Vec<Lora> =
                    Self::call(&self.agent, &format!("{}/sdapi/v1/loras", self.base), None, false).unwrap_or_default();
                let listed: Vec<String> = listed.into_iter().map(|l| l.path).collect();
                let mut body = body.clone();
                body["lora"] = loras
                    .iter()
                    .map(|l| {
                        let mut l = l.clone();
                        if let Some(path) = l.get("path").and_then(Value::as_str) {
                            l["path"] = server_id(&listed, path).into();
                        }
                        l
                    })
                    .collect();
                Ok(self.post::<Submitted>("img_gen", &body)?.id)
            }
            None => Ok(self.post::<Submitted>("img_gen", body)?.id),
        }
    }

    pub fn job(&self, id: &str) -> ApiResult<Job> {
        if let Some(k) = &self.kobold {
            return k.job(id);
        }
        self.get(&format!("jobs/{id}"))
    }

    /// Cancels a job (a generating job keeps `generating`, with
    /// `cancel_requested`, until it stops at a checkpoint).
    pub fn cancel(&self, id: &str) -> ApiResult<Job> {
        if let Some(k) = &self.kobold {
            return k.cancel(id);
        }
        self.post(&format!("jobs/{id}/cancel"), &serde_json::json!({}))
    }
}

/// A model or LoRA file as IncreArt names it: its path inside the model
/// directory (or LoRA folder) that has it, as KoboldCpp finds it. sd-server
/// names those outside its first `--model-dir` `@<folder hash>/<path>`.
pub fn portable_id(id: &str) -> &str {
    match id.split_once('/') {
        Some((root, rest))
            if root.len() == 17 && root.starts_with('@') && root[1..].bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            rest
        }
        _ => id,
    }
}

/// sd-server's name for a file IncreArt names `id`, among those it lists:
/// `id` itself when its first folder has it, else another folder's file at
/// that path, else the one file it lists of that file name (a proxy, or
/// another folder layout: `x.gguf` for `diffusion_models/x.gguf`); `id` as
/// it is when none has it, for its error.
fn server_id(listed: &[String], id: &str) -> String {
    if listed.iter().any(|l| l == id) {
        return id.to_string();
    }
    if let Some(l) = listed.iter().find(|l| portable_id(l) == id && l.as_str() != id) {
        return l.clone();
    }
    let portable: Vec<&str> = listed.iter().map(|l| portable_id(l)).collect();
    match by_file_name(portable.iter().copied(), id) {
        Some(p) => listed[portable.iter().position(|x| *x == p).unwrap_or(0)].clone(),
        None => id.to_string(),
    }
}

/// Of `listed`, the one file named as `id`'s file name is (None: none, or
/// several: which is meant isn't known).
fn by_file_name<'a>(listed: impl Iterator<Item = &'a str>, id: &str) -> Option<&'a str> {
    let name = |p: &str| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string();
    let want = name(id);
    let mut found = listed.filter(|l| name(l) == want);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

fn portable_status(mut s: ModelStatus) -> ModelStatus {
    for id in s.models.values_mut() {
        *id = portable_id(id).to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_listed_under_another_folder_is_found_by_its_name() {
        let files = |m: &[(&str, &str)]| m.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let asked: BTreeMap<String, String> =
            files(&[("diffusion_model", "diffusion_models/q.gguf"), ("vae", "vae/v.safetensors")]);
        // sd-server's API (a proxy, or another layout): by name
        let listed = Loadable::listed(["q.gguf".to_string(), "other/v.safetensors".to_string()]);
        assert!(listed.runs(&asked));
        assert_eq!(listed.id_of("diffusion_models/q.gguf"), Some("q.gguf"));
        // a started KoboldCpp loads from the model folders: not by name
        let local = Loadable::files(["q.gguf".to_string(), "other/v.safetensors".to_string()]);
        assert_eq!(local.why_not(&asked).as_deref(), Some("it doesn't have diffusion_models/q.gguf"));
        // two of that name: which is meant isn't known
        let two = Loadable::listed(["a/q.gguf".to_string(), "b/q.gguf".to_string(), "v.safetensors".to_string()]);
        assert_eq!(
            two.why_not(&asked).as_deref(),
            Some("it lists more than one q.gguf, so which is diffusion_models/q.gguf isn't known")
        );
        // the server's IDs, when loading
        let listed = ["@0123456789abcdef/q.gguf".to_string(), "v.safetensors".to_string()];
        assert_eq!(server_id(&listed, "diffusion_models/q.gguf"), "@0123456789abcdef/q.gguf");
        assert_eq!(server_id(&listed, "vae/v.safetensors"), "v.safetensors");
        assert_eq!(server_id(&listed, "vae/none.safetensors"), "vae/none.safetensors", "for its error");
    }

    #[test]
    fn a_load_answered_with_the_same_files_under_other_names_holds_them() {
        let key = |recipe: &str, models: &[(&str, &str)]| ModelKey {
            recipe: recipe.into(),
            models: models.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        };
        let answer = |state: &str, k: &ModelKey| ModelStatus {
            state: state.into(),
            recipe: Some(k.recipe.clone()),
            models: k.models.clone(),
            ..Default::default()
        };
        let asked = key("krea2", &[("diffusion_model", "krea2.safetensors")]);
        assert!(answer("ready", &asked).holds(&asked));
        // as imgproxy answers in front of ComfyUI
        let proxy = key("checkpoint", &[("model", "krea2.safetensors")]);
        assert!(answer("ready", &proxy).holds(&asked));
        assert!(!answer("loading", &proxy).holds(&asked));
        // another file, or some of them only, isn't it
        assert!(!answer("ready", &key("checkpoint", &[("model", "other.gguf")])).holds(&asked));
        let three = key("qwen", &[("diffusion_model", "d"), ("llm", "l"), ("vae", "v")]);
        assert!(!answer("ready", &key("checkpoint", &[("model", "d")])).holds(&three));
        assert!(answer("ready", &key("q", &[("a", "v"), ("b", "d"), ("c", "l")])).holds(&three));
        assert!(!answer("ready", &key("x", &[])).holds(&key("y", &[])));
    }

    #[test]
    fn a_load_answered_with_another_model_says_how_it_differs() {
        let asked = ModelKey {
            recipe: "krea2".into(),
            models: [("diffusion_model".to_string(), "krea2.safetensors".to_string())].into(),
        };
        let answer = |state: &str, recipe: Option<&str>, models: &[(&str, &str)]| ModelStatus {
            state: state.into(),
            recipe: recipe.map(String::from),
            models: models.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..Default::default()
        };
        assert!(
            answer("ready", Some("krea2"), &[("diffusion_model", "krea2.safetensors")]).mismatch(&asked).is_empty()
        );
        // as reported: a proxy naming its own recipe and slot
        assert_eq!(
            answer("ready", Some("checkpoint"), &[("model", "krea2.safetensors")]).mismatch(&asked),
            [
                r#"recipe "checkpoint" instead of "krea2""#,
                r#"slot "model" instead of "diffusion_model" for "krea2.safetensors""#
            ]
        );
        assert_eq!(
            answer("ready", Some("krea2"), &[("diffusion_model", "other.gguf"), ("vae", "v.safetensors")])
                .mismatch(&asked),
            [
                r#"slot "diffusion_model" holds "other.gguf" instead of "krea2.safetensors""#,
                r#"slot "vae" not asked for"#
            ]
        );
        let mut failed = answer("error", None, &[]);
        failed.error = Some("out of memory".into());
        assert_eq!(
            failed.mismatch(&asked),
            [
                r#"state "error" instead of "ready": out of memory"#,
                r#"no recipe (asked "krea2")"#,
                r#"no slot "diffusion_model" (asked "krea2.safetensors")"#
            ]
        );
    }

    #[test]
    fn ids_outside_the_first_model_dir() {
        let h = "@0123456789abcdef";
        assert_eq!(portable_id(&format!("{h}/vae/v.safetensors")), "vae/v.safetensors");
        for kept in ["vae/v.safetensors", "@short/v.gguf", "@0123456789abcdeg/v.gguf", "@command-line"] {
            assert_eq!(portable_id(kept), kept);
        }
        let listed: Vec<String> = ["a.gguf", &format!("{h}/a.gguf"), &format!("{h}/b.gguf")].map(String::from).into();
        assert_eq!(server_id(&listed, "a.gguf"), "a.gguf", "the first folder's");
        assert_eq!(server_id(&listed, "b.gguf"), format!("{h}/b.gguf"));
        assert_eq!(server_id(&listed, "c.gguf"), "c.gguf", "unknown, for its error");
        let status =
            ModelStatus { models: BTreeMap::from([("vae".to_string(), format!("{h}/b.gguf"))]), ..Default::default() };
        assert_eq!(portable_status(status).models["vae"], "b.gguf");
    }

    #[test]
    fn local_servers_and_why_not() {
        assert!(Client::new("http://127.0.0.1:1234").is_local());
        assert!(Client::new("http://localhost:5001/").is_local());
        assert!(!Client::new("http://10.0.0.2:1234").is_local());
        let files = |m: &str| BTreeMap::from([("diffusion_model".to_string(), m.to_string())]);
        let l = Loadable::Files {
            ids: ["q8.gguf".to_string()].into(),
            refused: [("d/int8.safetensors".to_string(), "is int8".to_string())].into(),
            second_choice: [("q8.gguf".to_string(), "loads at full size".to_string())].into(),
            by_name: false,
        };
        assert!(l.runs(&files("q8.gguf")));
        assert_eq!(l.second_choice(&files("q8.gguf")).as_deref(), Some("q8.gguf loads at full size"));
        assert_eq!(Loadable::files(["q8.gguf".to_string()]).second_choice(&files("q8.gguf")), None);
        assert_eq!(l.why_not(&files("d/int8.safetensors")).as_deref(), Some("int8.safetensors is int8"));
        assert_eq!(l.why_not(&files("x.gguf")).as_deref(), Some("it doesn't have x.gguf"));
        let only = Loadable::Only(Some("/m/q8.gguf".into()));
        assert!(only.runs(&files("koboldcpp/q8.gguf")));
        assert!(only.why_not(&files("x.gguf")).unwrap().starts_with("it runs q8.gguf"));
    }

    #[test]
    fn error_classification() {
        let e = |s, b: &str| classify(s, b, false);
        assert_eq!(e(503, r#"{"error":"No model is loaded","code":"model_not_loaded"}"#), ApiError::ModelNotLoaded);
        assert_eq!(e(503, r#"{"error":"x","code":"model_loading"}"#), ApiError::ModelLoading);
        assert!(matches!(e(409, r#"{"error":"busy","code":"model_busy"}"#), ApiError::ModelBusy(_)));
        assert!(matches!(e(400, r#"{"error":"bad","code":"invalid_model_config"}"#), ApiError::InvalidModelConfig(_)));
        assert!(matches!(e(422, r#"{"error":"oom","code":"model_load_failed"}"#), ApiError::ModelLoadFailed(_)));
        assert_eq!(
            e(400, r#"{"error":"invalid json","message":"at 3"}"#),
            ApiError::BadRequest("invalid json: at 3".into())
        );
        assert_eq!(e(429, r#"{"error":"job queue is full"}"#), ApiError::QueueFull);
        assert_eq!(classify(410, "", true), ApiError::JobGone);
        assert!(matches!(e(404, "nope"), ApiError::Http { status: 404, .. }));
        assert!(e(500, "{}").is_server_problem());
        assert!(!e(400, "{}").is_server_problem());
    }

    #[test]
    fn recorded_job_shapes_parse() {
        let queued: Submitted = serde_json::from_str(
            r#"{"created":1792785695370,"id":"job_1a16a5ac28a_00000000","kind":"img_gen","poll_url":"/sdcpp/v1/jobs/job_1a16a5ac28a_00000000","status":"queued"}"#,
        )
        .unwrap();
        assert_eq!(queued.id, "job_1a16a5ac28a_00000000");
        let done: Job = serde_json::from_str(
            r#"{"cancel_requested":false,"completed":1792785827364,"created":1792785695370,"error":null,
                "id":"job_1a16a5ac28a_00000000","kind":"img_gen","queue_position":0,
                "result":{"images":[{"b64_json":"aGk=","index":0}],"output_format":"png"},
                "started":1792785695370,"status":"completed"}"#,
        )
        .unwrap();
        assert_eq!(done.status, JobStatus::Completed);
        assert_eq!(done.image().unwrap(), b"hi");
        assert_eq!(done.elapsed_s(), Some(131.994));
        let failed: Job = serde_json::from_str(
            r#"{"id":"j","status":"failed","error":{"code":"generation_failed","message":"empty results"}}"#,
        )
        .unwrap();
        assert_eq!(failed.error.unwrap().code, "generation_failed");
        let status: ModelStatus = serde_json::from_str(
            r#"{"state":"ready","recipe":"qwen-image-21","models":{"vae":"vae/v.safetensors"},"error":null,"revision":3,"load_elapsed_seconds":45.2}"#,
        )
        .unwrap();
        assert_eq!(status.loaded().unwrap().recipe, "qwen-image-21");
    }

    /// Each server takes the files of a slot it has, one it loads less
    /// well only when it has no other.
    #[test]
    fn a_server_picks_the_files_it_has() {
        let p = &crate::config::builtin_recipes()[0];
        let int8 = Loadable::files(p.files.values().cloned());
        assert_eq!(int8.pick(p), p.files);
        assert!(int8.runs_preset(p));
        let gguf: Vec<String> = p.alternatives.values().flatten().cloned().chain([p.files["vae"].clone()]).collect();
        let kcpp = Loadable::Files {
            ids: p.files.values().cloned().chain(gguf.clone()).collect(),
            refused: BTreeMap::new(),
            second_choice: p
                .files
                .values()
                .filter(|f| f.contains("int8"))
                .map(|f| (f.clone(), "is int8".into()))
                .collect(),
            by_name: false,
        };
        let picked = kcpp.pick(p);
        assert_eq!(picked["diffusion_model"], "diffusion_models/qwen_image_2.1-Q4_K.gguf");
        assert!(kcpp.second_choice(&picked).is_none());
        let none = Loadable::files(["vae/qwen_image_2.1_vae_bf16.safetensors".to_string()]);
        assert!(!none.runs_preset(p));
        assert!(none.why_not_preset(p).unwrap().contains("int8"), "named by its preferred file");
        let only = Loadable::Only(Some("/m/qwen_image_2.1-Q4_K.gguf".into()));
        assert!(only.runs_preset(p), "matched by an alternative's name");
    }
}
