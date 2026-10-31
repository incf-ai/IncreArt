//! Blocking client for sd-server's native API, `/sdcpp/v1/...` (DESIGN.md,
//! Backend). One `Client` per server; it is cheap to clone.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ureq::Agent;

use crate::model::ModelSelection;

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
}

/// What a server has loaded, or should load: recipe + slot → file ID. Two
/// runs can share a server without a reload exactly when their keys match.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ModelKey {
    pub recipe: String,
    pub models: BTreeMap<String, String>,
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
    /// KoboldCpp's model folders), and those it has but can't load, with why.
    Files { ids: BTreeSet<String>, refused: BTreeMap<String, String> },
    /// KoboldCpp started elsewhere: only the diffusion model it runs (its
    /// file name), if any.
    Only(Option<String>),
}

impl Loadable {
    pub fn files(ids: impl IntoIterator<Item = String>) -> Loadable {
        Loadable::Files { ids: ids.into_iter().collect(), refused: BTreeMap::new() }
    }

    /// Whether it can load all of a preset's files (slot → file ID).
    pub fn runs(&self, files: &BTreeMap<String, String>) -> bool {
        self.why_not(files).is_none()
    }

    /// Why it can't load a preset's files, if it can't.
    pub fn why_not(&self, files: &BTreeMap<String, String>) -> Option<String> {
        let name = |p: &str| std::path::Path::new(p).file_name().and_then(|f| f.to_str()).unwrap_or(p).to_string();
        match self {
            Loadable::Files { ids, refused } => files.values().find_map(|id| {
                if let Some(why) = refused.get(id) {
                    Some(format!("{} {why}", name(id)))
                } else if ids.contains(id) || std::path::Path::new(id).is_absolute() {
                    None
                } else {
                    Some(format!("it doesn't have {id}"))
                }
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

    fn finish<T: DeserializeOwned>(
        resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
        job: bool,
    ) -> ApiResult<T> {
        let mut resp = resp.map_err(|e| ApiError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_string()
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(classify(status, &body, job));
        }
        serde_json::from_str(&body).map_err(|e| ApiError::Transport(format!("unexpected response ({e}): {body:.200}")))
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        Self::finish(self.agent.get(self.url(path)).call(), path.starts_with("jobs/"))
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> ApiResult<T> {
        Self::finish(self.agent.post(self.url(path)).send_json(body), path.starts_with("jobs/"))
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
        Ok(Loadable::files(self.model_files()?.into_iter().map(|f| portable_id(&f.id).to_string())))
    }

    pub fn refresh_models(&self) -> ApiResult<Value> {
        self.post("models/refresh", &serde_json::json!({}))
    }

    pub fn model_status(&self) -> ApiResult<ModelStatus> {
        if let Some(k) = &self.kobold {
            return k.model_status();
        }
        self.get("models/status").map(portable_status)
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
        let body = serde_json::json!({ "recipe": model.recipe, "models": models });
        Self::finish(self.load_agent.post(self.url("models/load")).send_json(&body), false).map(portable_status)
    }

    pub fn unload_model(&self) -> ApiResult<ModelStatus> {
        if let Some(k) = &self.kobold {
            return k.unload_model();
        }
        Self::finish(self.load_agent.post(self.url("models/unload")).send_json(serde_json::json!({})), false)
            .map(portable_status)
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
                    Self::finish(self.agent.get(format!("{}/sdapi/v1/loras", self.base)).call(), false)
                        .unwrap_or_default();
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
/// that path (`id` as it is when none has it, for its error).
fn server_id(listed: &[String], id: &str) -> String {
    if listed.iter().any(|l| l == id) {
        return id.to_string();
    }
    listed.iter().find(|l| portable_id(l) == id && l.as_str() != id).cloned().unwrap_or_else(|| id.to_string())
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
        };
        assert!(l.runs(&files("q8.gguf")));
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
}
