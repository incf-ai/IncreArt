//! KoboldCpp as an image server (DESIGN.md, KoboldCpp): its A1111-style API
//! behind sd-server's job interface, so the engine drives it as it drives an
//! sd-server.
//!
//! - Generating is one request that answers with the image, so jobs wait
//!   here and go one at a time; progress comes from `/sdapi/v1/progress`.
//! - A cancel closes the request's connection: KoboldCpp stops an image
//!   when its client goes away (it has no abort by key for images).
//! - Models are switched with its admin mode: KoboldCpp writes a `.kcpps`
//!   for the launch arguments plus the model files (`--exportconfig`), and
//!   `reload_config` restarts its worker with it. A server without admin
//!   runs the model it was started with.
//! - Swap mode ([`Admin::swap`]): the chat model and the image model take
//!   turns, for cards too small for both (docs/vram-tiers.md). It starts
//!   with the chat model; an image reloads it with only the image model and
//!   a prompt enhance ask with only the chat model again, each waiting for
//!   the other's work to finish (a reload would end it). Asks go first, so
//!   they wait at most one image.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use ureq::Agent;

use crate::sdserver::{
    ApiError, ApiResult, Job, JobError, JobResult, JobStatus, Loadable, ModelKey, ModelStatus, Preview, ResultImage,
};
use crate::sync::{LockExt, WaitExt};

/// How long a model may take to load (Qwen-Image from a cold disk).
const LOAD_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// How long the old worker may take to go after a reload is asked for.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);
/// One image: generous, a cancel ends it sooner.
const GENERATE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// LoRAs per image (`lora_filenames_max` in koboldcpp.py).
const MAX_LORAS: usize = 10;
/// Finished jobs kept for the engine to read.
const KEEP_DONE: usize = 64;

/// Model slot → KoboldCpp flag.
const SLOTS: &[(&str, &str)] = &[
    ("diffusion_model", "--sdmodel"),
    ("model", "--sdmodel"),
    ("llm", "--sdllm"),
    ("vae", "--sdvae"),
    ("clip_l", "--sdclip1"),
    ("clip_g", "--sdclip2"),
    ("t5xxl", "--sdclip2"),
];

/// What a started server needs to switch models.
#[derive(Clone, Debug)]
pub struct Admin {
    pub binary: String,
    /// Its launch arguments (address, devices, `--admin…`), which every
    /// model's settings keep.
    pub args: Vec<String>,
    /// `--admindir`: where the settings for each model are written.
    pub dir: PathBuf,
    pub password: String,
    /// Where model file IDs are looked up, in order.
    pub model_dirs: Vec<String>,
    /// Its log, for why a load failed.
    pub log: Option<PathBuf>,
    /// The chat model it was launched with and the image model take turns
    /// (see the module docs).
    pub swap: bool,
}

/// Swap mode: which model KoboldCpp has loaded, and who's using it.
#[derive(Default)]
struct Turns {
    side: Side,
    images: u32,
    chats: u32,
    chats_waiting: u32,
    switching: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Side {
    /// As launched: the chat model only.
    #[default]
    Chat,
    Image,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Queued,
    Generating,
    Done(JobStatus),
}

struct JobState {
    body: Option<(String, Value)>,
    preview: bool,
    state: State,
    created: i64,
    started: Option<i64>,
    completed: Option<i64>,
    image: Option<String>,
    error: Option<String>,
    cancel: bool,
    conn: Option<TcpStream>,
}

struct Inner {
    base: String,
    agent: Agent,
    password: String,
    admin: Option<Admin>,
    /// What this client loaded, and the file KoboldCpp reports for it.
    loaded: Mutex<Option<(ModelKey, String)>>,
    /// Sampler names and aliases (lowercase) of the loaded model; empty:
    /// not known.
    samplers: Mutex<Vec<String>>,
    /// Once it has answered (a reload restarts the same binary).
    version: OnceLock<String>,
    jobs: Mutex<HashMap<String, JobState>>,
    done: Mutex<VecDeque<String>>,
    /// Swap mode: the turns, and the settings file with the image model.
    turns: (Mutex<Turns>, Condvar),
    image_settings: Mutex<Option<String>>,
}

/// Swap-mode servers by address, for prompt enhance's asks.
static SWAPPING: Mutex<Vec<(String, Weak<Inner>)>> = Mutex::new(vec![]);

/// Prompt enhance's turn on a KoboldCpp in swap mode, held while it asks.
pub struct ChatTurn(Arc<Inner>);

impl Drop for ChatTurn {
    fn drop(&mut self) {
        self.0.end_turn(Side::Chat);
    }
}

/// Waits for prompt enhance's turn on the KoboldCpp at `base`, when it
/// swaps (its chat model loaded again if need be); None when it doesn't.
pub fn chat_turn(base: &str) -> Result<Option<ChatTurn>, String> {
    let base = base.trim_end_matches('/');
    let inner = SWAPPING.lock_or_recover().iter().find(|(b, _)| b == base).and_then(|(_, w)| w.upgrade());
    let Some(inner) = inner else { return Ok(None) };
    inner.take_turn(Side::Chat, None)?;
    Ok(Some(ChatTurn(inner)))
}

/// One KoboldCpp image server (see the module docs).
pub struct KoboldImage {
    inner: Arc<Inner>,
    queue: Sender<String>,
    next: AtomicU64,
}

impl std::fmt::Debug for KoboldImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KoboldImage({})", self.inner.base)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// `total`: a cap on the whole request; `stall`: on each read and write
/// (restarted by every byte, so a slow answer that keeps coming is never
/// cut off).
fn agent(total: Option<Duration>, stall: Option<Duration>) -> Agent {
    // a fresh connection per request: KoboldCpp closes one after answering
    Agent::config_builder()
        .max_idle_connections_per_host(0)
        .http_status_as_error(false)
        .timeout_global(total)
        .timeout_per_read(stall)
        .timeout_per_write(stall)
        .timeout_connect(Some(Duration::from_secs(5)))
        .build()
        .into()
}

/// A `.safetensors` with int8 weights (`I8` tensors, as sd-server's
/// `int8_convrot` files have): KoboldCpp's image code can't load them.
pub fn int8_safetensors(path: &Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) != Some("safetensors") {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut len = [0u8; 8];
    if f.read_exact(&mut len).is_err() {
        return false;
    }
    let len = u64::from_le_bytes(len);
    if len > 64 << 20 {
        return false;
    }
    let mut header = vec![0u8; len as usize];
    if f.read_exact(&mut header).is_err() {
        return false;
    }
    let Ok(Value::Object(tensors)) = serde_json::from_slice::<Value>(&header) else { return false };
    tensors.values().any(|t| t.get("dtype").and_then(Value::as_str) == Some("I8"))
}

/// The files under `dir` as model file IDs, `depth` folders deep.
fn model_ids(dir: &Path, prefix: &str, depth: u32, ids: &mut BTreeSet<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
        let id = format!("{prefix}{name}");
        match e.file_type() {
            Ok(t) if t.is_dir() || (t.is_symlink() && e.path().is_dir()) => {
                if depth > 0 {
                    model_ids(&e.path(), &format!("{id}/"), depth - 1, ids);
                }
            }
            Ok(_) => {
                ids.insert(id);
            }
            Err(_) => {}
        }
    }
}

fn file_name(p: &str) -> &str {
    Path::new(p).file_name().and_then(|f| f.to_str()).unwrap_or(p)
}

impl KoboldImage {
    /// `base`: e.g. `http://127.0.0.1:5001`. `password`: KoboldCpp's
    /// `--password` (or its admin password), sent with every request.
    pub fn new(base: &str, password: &str, admin: Option<Admin>) -> std::io::Result<KoboldImage> {
        let inner = Arc::new(Inner {
            base: base.trim_end_matches('/').to_string(),
            agent: agent(None, Some(crate::config::timeouts().request())),
            password: password.to_string(),
            admin,
            loaded: Mutex::new(None),
            samplers: Mutex::new(vec![]),
            version: OnceLock::new(),
            jobs: Mutex::new(HashMap::new()),
            done: Mutex::new(VecDeque::new()),
            turns: (Mutex::new(Turns::default()), Condvar::new()),
            image_settings: Mutex::new(None),
        });
        if inner.swaps() {
            let mut list = SWAPPING.lock_or_recover();
            list.retain(|(b, w)| w.strong_count() > 0 && *b != inner.base);
            list.push((inner.base.clone(), Arc::downgrade(&inner)));
        }
        let (tx, rx) = mpsc::channel();
        let runner = inner.clone();
        std::thread::Builder::new().name(format!("koboldcpp {base}")).spawn(move || run_jobs(&runner, &rx))?;
        Ok(KoboldImage { inner, queue: tx, next: AtomicU64::new(0) })
    }

    /// `koboldcpp <version>`, once it has answered, for recipes.
    pub fn software(&self) -> Option<String> {
        self.inner.version.get().map(|v| format!("koboldcpp {v}"))
    }

    pub fn capabilities(&self) -> ApiResult<Value> {
        let v: Value = self.inner.get("/api/extra/version")?;
        if v.get("result").and_then(Value::as_str) != Some("KoboldCpp") {
            return Err(ApiError::Transport(format!("{} isn't KoboldCpp", self.inner.base)));
        }
        if let Some(ver) = v.get("version").and_then(Value::as_str) {
            let _ = self.inner.version.set(ver.to_string());
        }
        Ok(v)
    }

    pub fn model_status(&self) -> ApiResult<ModelStatus> {
        self.inner.status()
    }

    /// The RAM loading `model` pins (bytes): with `--sdoffloadcpu` its
    /// files' size, less the model it runs now (the restart frees that).
    pub fn load_ram_need(&self, model: &ModelKey) -> u64 {
        let Some(admin) = self.inner.admin.as_ref().filter(|a| a.args.iter().any(|x| x == "--sdoffloadcpu")) else {
            return 0;
        };
        let size = |key: &ModelKey| {
            model_flags(key, &admin.model_dirs)
                .map(|flags| flags.iter().filter_map(|(_, p)| std::fs::metadata(p).ok()).map(|m| m.len()).sum())
                .unwrap_or(0u64)
        };
        let now = self.inner.loaded.lock_or_recover().as_ref().map_or(0, |(k, _)| size(k));
        size(model).saturating_sub(now)
    }

    /// With admin, the files in its model folders (as IDs, `/`-separated
    /// under a folder); without, only the image model it runs.
    pub fn loadable(&self) -> ApiResult<Loadable> {
        match &self.inner.admin {
            Some(admin) => {
                let mut ids = BTreeSet::new();
                for dir in &admin.model_dirs {
                    model_ids(Path::new(dir), "", 3, &mut ids);
                }
                // sd-server's int8 files: KoboldCpp falls back to a
                // full-size buffer for them (31 GB for Qwen-Image's)
                let refused = ids
                    .iter()
                    .filter(|id| {
                        admin
                            .model_dirs
                            .iter()
                            .map(|d| Path::new(d).join(id))
                            .find(|p| p.exists())
                            .is_some_and(|p| int8_safetensors(&p))
                    })
                    .map(|id| (id.clone(), "is in sd-server's int8 format, which KoboldCpp can't load".to_string()))
                    .collect::<BTreeMap<_, _>>();
                ids.retain(|id| !refused.contains_key(id));
                Ok(Loadable::Files { ids, refused })
            }
            None => Ok(Loadable::Only(self.inner.running()?.map(|f| file_name(&f).to_string()))),
        }
    }

    /// Loads `model`: nothing to do if it runs it already; else, with
    /// admin, KoboldCpp restarts with it (blocks until it's back).
    pub fn load_model(&self, model: &ModelKey) -> ApiResult<ModelStatus> {
        self.inner.load(model)
    }

    /// Back to how it was launched (no image model).
    pub fn unload_model(&self) -> ApiResult<ModelStatus> {
        if self.inner.admin.is_none() {
            return Err(ApiError::InvalidModelConfig("KoboldCpp was started without admin".into()));
        }
        if self.inner.swaps() {
            // the chat model's turn is how it was launched
            self.inner.take_turn(Side::Chat, None).map_err(ApiError::ModelLoadFailed)?;
            self.inner.end_turn(Side::Chat);
        } else {
            self.inner.reload("initial_model")?;
        }
        *self.inner.loaded.lock_or_recover() = None;
        *self.inner.image_settings.lock_or_recover() = None;
        self.inner.status()
    }

    /// Queues a job from an sd-server `img_gen` body; returns its ID.
    pub fn img_gen(&self, body: &Value) -> ApiResult<String> {
        let samplers = self.inner.samplers.lock_or_recover().clone();
        let n = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("kcpp_{:x}_{n}", now_ms());
        let (path, a1111) = a1111_body(body, &id, &samplers).map_err(ApiError::BadRequest)?;
        let preview = body.get("preview").and_then(Value::as_str).is_some_and(|p| p != "none");
        let job = JobState {
            body: Some((path, a1111)),
            preview,
            state: State::Queued,
            created: now_ms(),
            started: None,
            completed: None,
            image: None,
            error: None,
            cancel: false,
            conn: None,
        };
        self.inner.jobs.lock_or_recover().insert(id.clone(), job);
        self.queue.send(id.clone()).map_err(|_| ApiError::Transport("runner stopped".into()))?;
        Ok(id)
    }

    pub fn job(&self, id: &str) -> ApiResult<Job> {
        let (mut job, progress) = {
            let jobs = self.inner.jobs.lock_or_recover();
            let j = jobs.get(id).ok_or(ApiError::JobGone)?;
            (to_job(id, j), j.state == State::Generating)
        };
        if progress {
            let preview = jobs_preview(&self.inner, id);
            job.preview = preview;
        }
        Ok(job)
    }

    /// Cancels a job: one waiting goes at once; one generating stops when
    /// its connection is closed.
    pub fn cancel(&self, id: &str) -> ApiResult<Job> {
        let mut jobs = self.inner.jobs.lock_or_recover();
        let j = jobs.get_mut(id).ok_or(ApiError::JobGone)?;
        j.cancel = true;
        match j.state {
            State::Queued => {
                j.state = State::Done(JobStatus::Cancelled);
                j.completed = Some(now_ms());
                self.inner.retire(id);
            }
            State::Generating => {
                if let Some(c) = &j.conn {
                    let _ = c.shutdown(Shutdown::Both);
                }
            }
            State::Done(_) => {}
        }
        Ok(to_job(id, j))
    }
}

fn to_job(id: &str, j: &JobState) -> Job {
    let status = match j.state {
        State::Queued => JobStatus::Queued,
        State::Generating => JobStatus::Generating,
        State::Done(s) => s,
    };
    Job {
        id: id.to_string(),
        kind: "img_gen".into(),
        status,
        created: j.created,
        started: j.started,
        completed: j.completed,
        queue_position: 0,
        result: j.image.as_ref().map(|b64| JobResult {
            output_format: "png".into(),
            images: vec![ResultImage { index: 0, b64_json: b64.clone() }],
        }),
        error: j.error.as_ref().map(|m| JobError { code: "generation_failed".into(), message: m.clone() }),
        preview: None,
        cancel_requested: j.cancel,
    }
}

/// The sampling step of the job generating, with its preview if it asked
/// for previews.
fn jobs_preview(inner: &Inner, id: &str) -> Option<Preview> {
    let with_image = inner.jobs.lock_or_recover().get(id).is_some_and(|j| j.preview);
    let path = format!("/sdapi/v1/progress?genkey={id}&skip_current_image={}", !with_image);
    let v: Value = inner.get(&path).ok()?;
    let state = v.get("state")?;
    let step = state.get("sampling_step").and_then(Value::as_u64)? as u32;
    let total = state.get("sampling_steps").and_then(Value::as_u64).unwrap_or(0) as u32;
    if step == 0 {
        return None;
    }
    let image = v.get("current_image").and_then(Value::as_str).unwrap_or("");
    let b64 = image.split_once("base64,").map_or(image, |(_, b)| b).to_string();
    Some(Preview { pass: 1, step, total_steps: total, b64_json: b64 })
}

impl Inner {
    fn auth(
        &self,
        rq: ureq::RequestBuilder<ureq::typestate::WithoutBody>,
    ) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        if self.password.is_empty() { rq } else { rq.header("Authorization", format!("Bearer {}", self.password)) }
    }

    fn read(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> ApiResult<Value> {
        let mut resp = resp.map_err(|e| ApiError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().map_err(|e| ApiError::Transport(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(crate::sdserver::classify(status, &body, false));
        }
        serde_json::from_str(&body).map_err(|e| ApiError::Transport(format!("unexpected response ({e}): {body:.200}")))
    }

    fn get(&self, path: &str) -> ApiResult<Value> {
        Self::read(self.auth(self.agent.get(format!("{}{path}", self.base))).call())
    }

    fn post(&self, path: &str, body: &Value, password: &str) -> ApiResult<Value> {
        let mut rq = self.agent.post(format!("{}{path}", self.base));
        if !password.is_empty() {
            rq = rq.header("Authorization", format!("Bearer {password}"));
        }
        Self::read(rq.send_json(body))
    }

    /// The image model file KoboldCpp runs (None: no image model).
    fn running(&self) -> ApiResult<Option<String>> {
        let v = self.get("/api/extra/version")?;
        if let Some(ver) = v.get("version").and_then(Value::as_str) {
            let _ = self.version.set(ver.to_string());
        }
        if v.get("txt2img").and_then(Value::as_bool) != Some(true) {
            return Ok(None);
        }
        let models = self.get("/sdapi/v1/sd-models")?;
        Ok(models.get(0).and_then(|m| m.get("filename")).and_then(Value::as_str).map(str::to_string))
    }

    fn status(&self) -> ApiResult<ModelStatus> {
        // swapped out for the chat model: still this client's model
        if self.swaps() && self.turns.0.lock_or_recover().side == Side::Chat {
            return Ok(match self.loaded.lock_or_recover().clone() {
                Some((key, _)) => ModelStatus {
                    state: "ready".into(),
                    recipe: Some(key.recipe),
                    models: key.models,
                    ..Default::default()
                },
                None => ModelStatus { state: "empty".into(), ..Default::default() },
            });
        }
        let Some(file) = self.running()? else {
            return Ok(ModelStatus { state: "empty".into(), ..Default::default() });
        };
        let loaded = self.loaded.lock_or_recover().clone();
        let (recipe, models) = match loaded {
            Some((key, f)) if f == file => (key.recipe, key.models),
            // not loaded by this client: named by its file
            _ => ("koboldcpp".to_string(), BTreeMap::from([("diffusion_model".to_string(), file)])),
        };
        Ok(ModelStatus { state: "ready".into(), recipe: Some(recipe), models, ..Default::default() })
    }

    fn ready(&self, key: &ModelKey, file: String) -> ApiResult<ModelStatus> {
        *self.loaded.lock_or_recover() = Some((key.clone(), file));
        self.learn_samplers();
        self.status()
    }

    fn learn_samplers(&self) {
        let names = self.get("/sdapi/v1/samplers").ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let mut all = vec![];
        for s in names {
            all.extend(s.get("name").and_then(Value::as_str).map(str::to_lowercase));
            for a in s.get("aliases").and_then(Value::as_array).into_iter().flatten() {
                all.extend(a.as_str().map(str::to_lowercase));
            }
        }
        *self.samplers.lock_or_recover() = all;
    }

    fn load(&self, key: &ModelKey) -> ApiResult<ModelStatus> {
        let dirs = self.admin.as_ref().map(|a| a.model_dirs.clone()).unwrap_or_default();
        let flags = model_flags(key, &dirs).map_err(ApiError::InvalidModelConfig)?;
        // as its model list says: restarting with one would only fail, slowly
        if let Some((_, p)) = flags.iter().find(|(_, p)| int8_safetensors(Path::new(p))) {
            return Err(ApiError::InvalidModelConfig(format!(
                "{} is in sd-server's int8 format, which KoboldCpp can't load",
                file_name(p)
            )));
        }
        let wanted = flags.iter().find(|(f, _)| f == "--sdmodel").map(|(_, p)| p.clone()).unwrap_or_default();
        if let Some(admin) = self.admin.as_ref().filter(|a| a.swap) {
            return self.load_swapping(admin, key, &flags, &wanted);
        }
        let running = self.running()?;
        let already =
            self.loaded.lock_or_recover().as_ref().is_some_and(|(k, f)| k == key && Some(f) == running.as_ref());
        // the same diffusion model: taken as loaded (the other files can't be checked)
        let same_file = running.as_deref().is_some_and(|r| file_name(r) == file_name(&wanted));
        if let (true, Some(file)) = (already || (same_file && self.admin.is_none()), running.clone()) {
            return self.ready(key, file);
        }
        let Some(admin) = &self.admin else {
            let runs = running.as_deref().map_or("no image model".to_string(), |r| file_name(r).to_string());
            return Err(ApiError::InvalidModelConfig(format!(
                "it runs {runs} and can't switch to {} (KoboldCpp started elsewhere switches only with admin)",
                file_name(&wanted)
            )));
        };
        let name = export(admin, &flags).map_err(ApiError::ModelLoadFailed)?;
        let log_len = admin.log.as_ref().and_then(|l| std::fs::metadata(l).ok()).map_or(0, |m| m.len());
        self.reload(&name)?;
        match self.running()? {
            Some(file) if file_name(&file) == file_name(&wanted) => self.ready(key, file),
            _ => {
                *self.loaded.lock_or_recover() = None;
                let why = admin.log.as_deref().and_then(|l| load_error(l, log_len));
                Err(ApiError::ModelLoadFailed(why.unwrap_or_else(|| "KoboldCpp didn't load it (see its log)".into())))
            }
        }
    }

    fn swaps(&self) -> bool {
        self.admin.as_ref().is_some_and(|a| a.swap)
    }

    /// [`Inner::load`] in swap mode: the image model's settings, loaded in
    /// the image's turn (reported as loaded through the chat model's turns).
    fn load_swapping(
        &self,
        admin: &Admin,
        key: &ModelKey,
        flags: &[(String, String)],
        wanted: &str,
    ) -> ApiResult<ModelStatus> {
        if self.loaded.lock_or_recover().as_ref().is_some_and(|(k, _)| k == key) {
            return self.status();
        }
        let name = export(admin, flags).map_err(ApiError::ModelLoadFailed)?;
        let log_len = admin.log.as_ref().and_then(|l| std::fs::metadata(l).ok()).map_or(0, |m| m.len());
        *self.loaded.lock_or_recover() = None;
        *self.image_settings.lock_or_recover() = Some(name.clone());
        self.take_turn(Side::Image, Some(&name)).map_err(ApiError::ModelLoadFailed)?;
        let running = self.running();
        self.end_turn(Side::Image);
        match running? {
            Some(file) if file_name(&file) == file_name(wanted) => self.ready(key, file),
            _ => {
                *self.image_settings.lock_or_recover() = None;
                let why = admin.log.as_deref().and_then(|l| load_error(l, log_len));
                Err(ApiError::ModelLoadFailed(why.unwrap_or_else(|| "KoboldCpp didn't load it (see its log)".into())))
            }
        }
    }

    /// Waits for `side`'s turn (asks go before images; nothing while the
    /// other side works or a reload runs), then has that model loaded:
    /// `reload` names the settings to load whatever is loaded.
    fn take_turn(&self, side: Side, reload: Option<&str>) -> Result<(), String> {
        let (lock, cv) = &self.turns;
        let mut t = lock.lock_or_recover();
        if side == Side::Chat {
            t.chats_waiting += 1;
        }
        loop {
            let busy = match side {
                Side::Chat => t.images > 0,
                Side::Image => t.chats > 0 || t.chats_waiting > 0,
            };
            if !t.switching && !busy {
                break;
            }
            t = cv.wait_or_recover(t);
        }
        if side == Side::Chat {
            t.chats_waiting -= 1;
        }
        if t.side != side || reload.is_some() {
            t.switching = true;
            drop(t);
            let settings = match (side, reload) {
                (_, Some(name)) => Some(name.to_string()),
                (Side::Chat, None) => Some("initial_model".to_string()),
                (Side::Image, None) => self.image_settings.lock_or_recover().clone(),
            };
            let r = match settings {
                Some(name) => self.reload(&name).map_err(|e| e.to_string()),
                None => Err("no image model loaded".into()),
            };
            t = lock.lock_or_recover();
            t.switching = false;
            if r.is_ok() {
                t.side = side;
            }
            cv.notify_all();
            r?;
        }
        match side {
            Side::Chat => t.chats += 1,
            Side::Image => t.images += 1,
        }
        Ok(())
    }

    fn end_turn(&self, side: Side) {
        let (lock, cv) = &self.turns;
        let mut t = lock.lock_or_recover();
        match side {
            Side::Chat => t.chats = t.chats.saturating_sub(1),
            Side::Image => t.images = t.images.saturating_sub(1),
        }
        cv.notify_all();
    }

    /// Asks KoboldCpp to restart with `file` (from its admin dir) and waits
    /// for it to answer again.
    fn reload(&self, file: &str) -> ApiResult<()> {
        let Some(admin) = self.admin.as_ref() else {
            return Err(ApiError::InvalidModelConfig("reloading KoboldCpp needs its admin password".into()));
        };
        let r = self.post("/api/admin/reload_config", &json!({ "filename": file }), &admin.password)?;
        if r.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(ApiError::InvalidModelConfig(format!(
                "KoboldCpp refused to load {file} (admin password, folder?)"
            )));
        }
        // the old worker answers for a moment before it goes
        let t = Instant::now();
        while self.get("/api/extra/version").is_ok() {
            if t.elapsed() > STOP_TIMEOUT {
                return Err(ApiError::ModelLoadFailed("KoboldCpp didn't restart".into()));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let t = Instant::now();
        while self.get("/api/extra/version").is_err() {
            if t.elapsed() > LOAD_TIMEOUT {
                return Err(ApiError::Transport("KoboldCpp didn't come back after loading".into()));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Ok(())
    }

    /// Drops the oldest finished jobs past [`KEEP_DONE`].
    fn retire(&self, id: &str) {
        let mut done = self.done.lock_or_recover();
        done.push_back(id.to_string());
        while done.len() > KEEP_DONE {
            if let Some(old) = done.pop_front()
                && let Ok(mut jobs) = self.jobs.try_lock()
            {
                jobs.remove(&old);
            }
        }
    }
}

/// The model's files as KoboldCpp flags, found under `dirs` (an absolute
/// ID as is).
fn model_flags(key: &ModelKey, dirs: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut flags = vec![];
    for (slot, id) in &key.models {
        let Some((_, flag)) = SLOTS.iter().find(|(s, _)| s == slot) else {
            return Err(format!("KoboldCpp has no place for the {slot} file"));
        };
        let path = if Path::new(id).is_absolute() {
            Some(PathBuf::from(id))
        } else {
            dirs.iter().map(|d| Path::new(d).join(id)).find(|p| p.exists())
        };
        let path = match path {
            Some(p) => p,
            None if dirs.is_empty() => PathBuf::from(id),
            None => return Err(format!("{id} isn't in the model folders")),
        };
        flags.push((flag.to_string(), path.to_string_lossy().into_owned()));
    }
    if !flags.iter().any(|(f, _)| f == "--sdmodel") {
        return Err("no diffusion model file".into());
    }
    Ok(flags)
}

/// Has KoboldCpp write the settings for its launch arguments plus the
/// model's files into the admin dir; returns the file's name.
fn export(admin: &Admin, flags: &[(String, String)]) -> Result<String, String> {
    use std::hash::{Hash, Hasher};
    let mut args = admin.args.clone();
    // the launch ran without a model; this one has one
    args.retain(|a| a != "--nomodel");
    if admin.swap {
        args = without_chat(&args);
    }
    for (f, p) in flags {
        args.extend([f.clone(), p.clone()]);
    }
    // LoRAs chosen per request from every model directory's loras/, as
    // sd-server finds them (KoboldCpp loads none until asked; a path in
    // more than one is the first folder's)
    let loras = crate::config::lora_dirs(&admin.model_dirs);
    if !loras.is_empty() {
        args.push("--sdlora".to_string());
        args.extend(loras.iter().map(|d| d.to_string_lossy().into_owned()));
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    args.hash(&mut h);
    let name = format!("increart-{:016x}.kcpps", h.finish());
    let path = admin.dir.join(&name);
    std::fs::create_dir_all(&admin.dir).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&path);
    let mut cmd = crate::kobold::command(&admin.binary)?;
    cmd.args(&args);
    let text = crate::process::command_text(&cmd);
    crate::process::background(&mut cmd, false).arg("--exportconfig").arg(&path);
    let out = cmd.output().map_err(|e| format!("can't run {}: {e}", admin.binary))?;
    if !path.is_file() {
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = text.lines().rev().take(5).collect();
        return Err(format!(
            "KoboldCpp didn't write its settings: {}",
            tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
        ));
    }
    // the log shows what each reload loads, as it shows the launch
    if let Some(log) = &admin.log {
        use std::io::Write;
        let note = crate::process::log_note(&format!("reloading with {name}, the settings for: {text}"));
        let _ = std::fs::OpenOptions::new().append(true).open(log).and_then(|mut f| f.write_all(note.as_bytes()));
    }
    Ok(name)
}

/// Launch arguments without the chat model, for swap mode's image turns.
fn without_chat(args: &[String]) -> Vec<String> {
    let mut out = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--model" || a == "--mmproj" {
            it.next();
        } else {
            out.push(a.clone());
        }
    }
    out
}

/// The reason a load failed, from what the log gained since `from`.
fn load_error(log: &Path, from: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(log).ok()?;
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut text = String::new();
    f.take(1 << 20).read_to_string(&mut text).ok()?;
    // most telling first ("Unable to determine GPU Memory" is only a warning)
    let hints = ["cannot", "out of memory", "failed", "error"];
    let lines: Vec<&str> = text
        .lines()
        .map(crate::logview::unstamped)
        .filter(|l| !l.starts_with("Namespace(") && !l.starts_with("--- IncreArt"))
        .collect();
    hints.iter().find_map(|h| lines.iter().find(|l| l.to_lowercase().contains(h))).map(|l| l.trim().to_string())
}

/// The A1111 request (path, body) for an sd-server `img_gen` body.
/// `samplers`: what the model offers (empty: not known).
pub fn a1111_body(body: &Value, genkey: &str, samplers: &[String]) -> Result<(String, Value), String> {
    let sp = body.get("sample_params").cloned().unwrap_or(json!({}));
    let steps = sp.get("sample_steps").and_then(Value::as_u64).unwrap_or(20);
    let cfg = sp.get("guidance").and_then(|g| g.get("txt_cfg")).and_then(Value::as_f64).unwrap_or(1.0);
    if sp.get("custom_sigmas").and_then(Value::as_array).is_some_and(|s| !s.is_empty()) {
        return Err("KoboldCpp can't take custom sigmas".into());
    }
    let refs: Vec<Value> = body.get("ref_images").and_then(Value::as_array).cloned().unwrap_or_default();
    if refs.len() > 4 {
        return Err(format!("KoboldCpp takes at most 4 reference images ({} given)", refs.len()));
    }
    let mut out = json!({
        "prompt": body.get("prompt").cloned().unwrap_or(json!("")),
        "negative_prompt": body.get("negative_prompt").cloned().unwrap_or(json!("")),
        "width": body.get("width").cloned().unwrap_or(json!(512)),
        "height": body.get("height").cloned().unwrap_or(json!(512)),
        "seed": body.get("seed").cloned().unwrap_or(json!(-1)),
        "steps": steps,
        "cfg_scale": cfg,
        "extra_images": refs,
        "genkey": genkey,
        // the run's settings as given, not clamped for shared use
        "add_sd_step_limit": steps.max(80),
        "add_sd_cfg_limit": cfg.max(25.0),
        "remove_limits": 1,
    });
    if let Some(m) = sp.get("sample_method").and_then(Value::as_str).filter(|m| !m.is_empty()) {
        if !samplers.is_empty() && !samplers.iter().any(|s| *s == m.to_lowercase()) {
            return Err(format!("KoboldCpp has no sampler {m}"));
        }
        out["sampler_name"] = json!(m);
    }
    if let Some(s) = sp.get("scheduler").and_then(Value::as_str) {
        out["scheduler"] = json!(s);
    }
    if let Some(f) = sp.get("flow_shift") {
        out["flow_shift"] = f.clone();
    }
    if let Some(loras) = body.get("lora").and_then(Value::as_array).filter(|l| !l.is_empty()) {
        if loras.len() > MAX_LORAS {
            return Err(format!("KoboldCpp takes at most {MAX_LORAS} LoRAs ({} given)", loras.len()));
        }
        out["lora"] = json!(loras);
    }
    match body.get("init_image").and_then(Value::as_str) {
        Some(init) => {
            out["init_images"] = json!([init]);
            out["denoising_strength"] = body.get("strength").cloned().unwrap_or(json!(0.75));
            Ok(("/sdapi/v1/img2img".into(), out))
        }
        None => Ok(("/sdapi/v1/txt2img".into(), out)),
    }
}

/// Runs queued jobs one at a time until the client goes.
fn run_jobs(inner: &Inner, rx: &Receiver<String>) {
    while let Ok(id) = rx.recv() {
        let (path, body) = {
            let mut jobs = inner.jobs.lock_or_recover();
            let Some(j) = jobs.get_mut(&id) else { continue };
            if j.cancel || j.state != State::Queued {
                continue;
            }
            let Some(b) = j.body.take() else { continue };
            j.state = State::Generating;
            j.started = Some(now_ms());
            b
        };
        let result = if inner.swaps() {
            match inner.take_turn(Side::Image, None) {
                Ok(()) => {
                    let cancelled = inner.jobs.lock_or_recover().get(&id).is_none_or(|j| j.cancel);
                    let r = if cancelled { Err("cancelled".into()) } else { generate(inner, &id, &path, &body) };
                    inner.end_turn(Side::Image);
                    r
                }
                Err(e) => Err(format!("loading the image model: {e}")),
            }
        } else {
            generate(inner, &id, &path, &body)
        };
        let mut jobs = inner.jobs.lock_or_recover();
        let Some(j) = jobs.get_mut(&id) else { continue };
        j.conn = None;
        j.completed = Some(now_ms());
        j.state = State::Done(match result {
            _ if j.cancel => JobStatus::Cancelled,
            Ok(image) => {
                j.image = Some(image);
                JobStatus::Completed
            }
            Err(e) => {
                j.error = Some(e);
                JobStatus::Failed
            }
        });
        drop(jobs);
        inner.retire(&id);
    }
}

/// POSTs one generation on a connection of its own (so a cancel can close
/// it) and returns the image as base64.
fn generate(inner: &Inner, id: &str, path: &str, body: &Value) -> Result<String, String> {
    // KoboldCpp skips a LoRA it doesn't have without a word
    if let Some(asked) = body.get("lora").and_then(Value::as_array) {
        let known = inner.get("/sdapi/v1/loras").map_err(|e| format!("listing its LoRAs: {e}"))?;
        if let Some(missing) = missing_lora(asked, &known) {
            return Err(format!(
                "KoboldCpp doesn't have the LoRA {missing}: it finds LoRAs in the loras folder of each model \
                 directory, at most one folder down, when it loads the model (load it again after adding one; one started \
                 by hand needs --sdlora <that folder>)"
            ));
        }
    }
    let hostport = inner.base.trim_start_matches("http://").trim_end_matches('/');
    if inner.base.starts_with("https://") {
        return Err("KoboldCpp over https isn't supported".into());
    }
    let addr = hostport.to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(CANCEL_CHECK)).map_err(|e| e.to_string())?;
    if let Some(j) = inner.jobs.lock_or_recover().get_mut(id) {
        j.conn = stream.try_clone().ok();
    }
    let data = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let auth =
        if inner.password.is_empty() { String::new() } else { format!("Authorization: Bearer {}\r\n", inner.password) };
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {hostport}\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n",
        data.len()
    );
    stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&data)).map_err(|e| e.to_string())?;
    let reader = Cancellable { stream, inner, id, deadline: Instant::now() + GENERATE_TIMEOUT };
    let (status, reply) = read_response(reader).map_err(|e| format!("KoboldCpp: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "HTTP {status}: {}",
            String::from_utf8_lossy(&reply).chars().take(300).collect::<String>()
        ));
    }
    let v: Value = serde_json::from_slice(&reply).map_err(|e| format!("unexpected response: {e}"))?;
    match v.get("images").and_then(|i| i.get(0)).and_then(Value::as_str) {
        Some(img) if !img.is_empty() => Ok(img.to_string()),
        // what KoboldCpp answers with no image model, or when generating failed
        _ => Err("KoboldCpp returned no image (see its log)".into()),
    }
}

/// The first LoRA path in `asked` that `known` (`/sdapi/v1/loras`) lacks.
fn missing_lora(asked: &[Value], known: &Value) -> Option<String> {
    // its paths are joined with the OS's separator
    let known: Vec<String> = known
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l.get("path").and_then(Value::as_str))
        .map(|p| p.replace('\\', "/"))
        .collect();
    asked
        .iter()
        .filter_map(|l| l.get("path").and_then(Value::as_str))
        .find(|p| !known.iter().any(|k| k == p))
        .map(str::to_string)
}

/// How often a generation's read looks whether its job was cancelled.
const CANCEL_CHECK: Duration = Duration::from_millis(200);

/// A generation's connection, read until its job is cancelled. Cancelling
/// shuts the socket down, which wakes a blocked read on Linux and macOS but
/// not on Windows, so reads also time out every [`CANCEL_CHECK`] to look.
struct Cancellable<'a> {
    stream: TcpStream,
    inner: &'a Inner,
    id: &'a str,
    deadline: Instant,
}

impl Read for Cancellable<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::ErrorKind::{TimedOut, WouldBlock};
        loop {
            match self.stream.read(buf) {
                Err(e) if matches!(e.kind(), WouldBlock | TimedOut) => {
                    if self.inner.jobs.lock_or_recover().get(self.id).is_none_or(|j| j.cancel) {
                        return Err(std::io::Error::other("cancelled"));
                    }
                    if Instant::now() >= self.deadline {
                        return Err(e);
                    }
                }
                r => return r,
            }
        }
    }
}

fn read_response(stream: impl Read) -> std::io::Result<(u16, Vec<u8>)> {
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    if status == 0 {
        return Err(std::io::Error::other("connection closed"));
    }
    let mut len = None;
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![];
    match len {
        Some(n) => {
            body.resize(n, 0);
            r.read_exact(&mut body)?;
        }
        None => {
            r.read_to_end(&mut body)?;
        }
    }
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_sd_server_body_as_a1111() {
        let body = json!({
            "prompt": "a fox", "negative_prompt": "", "width": 1024, "height": 768, "seed": 7,
            "sample_params": { "sample_method": "euler", "sample_steps": 30, "guidance": { "txt_cfg": 2.5 }, "flow_shift": 3.0 },
            "ref_images": ["AAA", "BBB"], "preview": "tae",
        });
        let (path, a) = a1111_body(&body, "k1", &[]).unwrap();
        assert_eq!(path, "/sdapi/v1/txt2img");
        assert_eq!(a["steps"], 30);
        assert_eq!(a["cfg_scale"], 2.5);
        assert_eq!(a["sampler_name"], "euler");
        assert_eq!(a["flow_shift"], 3.0);
        assert_eq!(a["extra_images"], json!(["AAA", "BBB"]));
        assert_eq!(a["genkey"], "k1");
        assert_eq!(a["add_sd_step_limit"], 80);
        let mut edit = body.clone();
        edit["init_image"] = json!("III");
        edit["strength"] = json!(0.4);
        let (path, a) = a1111_body(&edit, "k2", &[]).unwrap();
        assert_eq!(
            (path.as_str(), &a["init_images"], &a["denoising_strength"]),
            ("/sdapi/v1/img2img", &json!(["III"]), &json!(0.4))
        );
        // what it can't do is said, not dropped
        assert!(a1111_body(&body, "k", &["euler a".into()]).unwrap_err().contains("no sampler euler"));
        let mut sigmas = body.clone();
        sigmas["sample_params"]["custom_sigmas"] = json!([1.0, 0.0]);
        assert!(a1111_body(&sigmas, "k", &[]).is_err());
        let mut many = body;
        many["ref_images"] = json!(["1", "2", "3", "4", "5"]);
        assert!(a1111_body(&many, "k", &[]).unwrap_err().contains("at most 4"));
    }

    #[test]
    fn int8_safetensors_are_told_by_their_header() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, header: &str| {
            let p = dir.path().join(name);
            let mut b = (header.len() as u64).to_le_bytes().to_vec();
            b.extend(header.as_bytes());
            std::fs::write(&p, b).unwrap();
            p
        };
        let int8 = write(
            "a.safetensors",
            r#"{"w":{"dtype":"I8","shape":[2],"data_offsets":[0,2]},"w_scale":{"dtype":"F32","shape":[1],"data_offsets":[2,6]}}"#,
        );
        let bf16 = write(
            "b.safetensors",
            r#"{"__metadata__":{"format":"pt"},"w":{"dtype":"BF16","shape":[1],"data_offsets":[0,2]}}"#,
        );
        let gguf = write("c.gguf", r#"{"w":{"dtype":"I8"}}"#);
        assert!(int8_safetensors(&int8));
        assert!(!int8_safetensors(&bf16));
        assert!(!int8_safetensors(&gguf));
        assert!(!int8_safetensors(&dir.path().join("missing.safetensors")));
    }

    #[test]
    fn model_folders_list_as_ids() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("koboldcpp/sub")).unwrap();
        std::fs::write(dir.path().join("koboldcpp/q8.gguf"), "").unwrap();
        std::fs::write(dir.path().join("koboldcpp/sub/vae.safetensors"), "").unwrap();
        std::fs::write(dir.path().join("top.gguf"), "").unwrap();
        let mut ids = BTreeSet::new();
        model_ids(dir.path(), "", 3, &mut ids);
        let want: BTreeSet<String> =
            ["koboldcpp/q8.gguf", "koboldcpp/sub/vae.safetensors", "top.gguf"].map(String::from).into();
        assert_eq!(ids, want);
    }

    #[test]
    fn model_files_as_flags() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("vae")).unwrap();
        std::fs::write(dir.path().join("vae/v.safetensors"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("dm")).unwrap();
        std::fs::write(dir.path().join("dm/d.gguf"), "").unwrap();
        let dirs = vec!["/nowhere".to_string(), dir.path().to_string_lossy().into_owned()];
        let key = |m: &[(&str, &str)]| ModelKey {
            recipe: "r".into(),
            models: m.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        };
        let flags =
            model_flags(&key(&[("diffusion_model", "dm/d.gguf"), ("vae", "vae/v.safetensors")]), &dirs).unwrap();
        let d = |rel| dir.path().join(rel).to_string_lossy().into_owned();
        assert_eq!(flags, [("--sdmodel".to_string(), d("dm/d.gguf")), ("--sdvae".to_string(), d("vae/v.safetensors"))]);
        assert!(
            model_flags(&key(&[("diffusion_model", "dm/x.gguf")]), &dirs)
                .unwrap_err()
                .contains("isn't in the model folders")
        );
        assert!(
            model_flags(&key(&[("diffusion_model", "dm/d.gguf"), ("lora", "l")]), &dirs)
                .unwrap_err()
                .contains("no place")
        );
        assert!(model_flags(&key(&[("vae", "vae/v.safetensors")]), &dirs).is_err());
    }

    #[test]
    fn loras_go_through_and_unknown_ones_are_found() {
        let body = json!({ "prompt": "p", "lora": [{ "path": "sub/a.safetensors", "multiplier": 0.5 }] });
        let (_, out) = a1111_body(&body, "k", &[]).unwrap();
        assert_eq!(out["lora"], body["lora"]);
        let eleven: Vec<Value> = (0..11).map(|i| json!({ "path": format!("{i}.gguf"), "multiplier": 1.0 })).collect();
        assert!(a1111_body(&json!({ "lora": eleven }), "k", &[]).unwrap_err().contains("at most 10"));
        let known = json!([{ "name": "a", "path": "sub\\a.safetensors" }]);
        assert_eq!(missing_lora(body["lora"].as_array().unwrap(), &known), None);
        assert_eq!(missing_lora(&[json!({ "path": "b.gguf" })], &known).as_deref(), Some("b.gguf"));
    }

    #[test]
    fn the_reason_a_load_failed_is_found_in_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("k.log");
        std::fs::write(&log, "old error\nRestarting KoboldCpp...\nNamespace(error=1)\nUnable to determine GPU Memory\nload failed\nCannot find image model file: /x.safetensors\nAttempting to recover\n").unwrap();
        assert_eq!(load_error(&log, 10).as_deref(), Some("Cannot find image model file: /x.safetensors"));
    }
}
