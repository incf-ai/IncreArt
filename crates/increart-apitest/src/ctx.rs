//! What the cases share: the preset's open servers, the model presets,
//! the run's output, and helpers for the calls most cases make (load a
//! model preset, make an image, wait for a job).

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use increart_core::apitest::{Event, Outcome};
use increart_core::config::{Config, RecipePreset, ServerConfig};
use increart_core::launcher::ServerProcess;
use increart_core::pe::{PeProcess, PeServer};
use increart_core::sdserver::{Client, Job, JobStatus, Loadable, ModelKey};
use serde_json::{Value, json};

use crate::report::Report;

/// A case's result.
pub struct Verdict {
    pub outcome: Outcome,
    pub summary: String,
}

/// The value of `r`, or (from the case) a failed verdict with its error.
macro_rules! tried {
    ($r:expr) => {
        match $r {
            Ok(v) => v,
            Err(e) => return $crate::ctx::fail(e),
        }
    };
}
pub(crate) use tried;

pub fn pass(s: impl Into<String>) -> Verdict {
    Verdict { outcome: Outcome::Pass, summary: s.into() }
}
pub fn warn(s: impl Into<String>) -> Verdict {
    Verdict { outcome: Outcome::Warn, summary: s.into() }
}
pub fn fail(s: impl Into<String>) -> Verdict {
    Verdict { outcome: Outcome::Fail, summary: s.into() }
}
pub fn skipped(s: impl Into<String>) -> Verdict {
    Verdict { outcome: Outcome::Skipped, summary: s.into() }
}
pub fn not_applicable(s: impl Into<String>) -> Verdict {
    Verdict { outcome: Outcome::NotApplicable, summary: s.into() }
}

/// Where events go (stdout, the report), from any thread: the case under
/// way gets the log lines and traffic.
pub struct Out {
    state: Mutex<OutState>,
}

struct OutState {
    current: Option<String>,
    emit: Box<dyn FnMut(&Event) + Send>,
    report: Report,
}

impl Out {
    pub fn new(emit: Box<dyn FnMut(&Event) + Send>, report: Report) -> Out {
        Out { state: Mutex::new(OutState { current: None, emit, report }) }
    }

    pub fn event(&self, e: Event) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match &e {
            Event::Start { id } => s.current = Some(id.clone()),
            Event::End { .. } => s.current = None,
            _ => {}
        }
        s.report.record(&e);
        (s.emit)(&e);
    }

    pub fn log(&self, text: impl Into<String>) {
        let id = self.state.lock().unwrap_or_else(|e| e.into_inner()).current.clone();
        self.event(Event::Log { id, text: text.into() });
    }

    pub fn traffic(&self, text: &str) {
        let id = self.state.lock().unwrap_or_else(|e| e.into_inner()).current.clone();
        self.event(Event::Traffic { id, text: text.trim_end().to_string() });
    }

    pub fn with_report<R>(&self, f: impl FnOnce(&mut Report) -> R) -> R {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()).report)
    }
}

/// An image server of the preset.
pub struct ImageSrv {
    pub config: ServerConfig,
    pub proc: Option<ServerProcess>,
    /// Why it didn't open.
    pub failed: Option<String>,
    /// Stopped by a case (the failover's): the cases after it skip it.
    pub gone: bool,
    /// Its list, once asked (None inside: it has none, a 404).
    pub loadable: Option<Result<Option<Loadable>, String>>,
}

/// A chat server prompt enhance uses with the preset.
pub struct ChatSrv {
    pub server: PeServer,
    pub proc: Option<PeProcess>,
    pub failed: Option<String>,
}

/// A model preset of the config.
#[derive(Clone)]
pub struct Model {
    pub preset: RecipePreset,
    pub key: ModelKey,
}

pub struct Ctx<'a> {
    pub config: &'a Config,
    pub images: Vec<ImageSrv>,
    pub chats: Vec<ChatSrv>,
    pub models: Vec<Model>,
    /// The run's folder (the report's): started servers log in `servers/`.
    pub work: PathBuf,
    pub cancel: &'a AtomicBool,
    pub out: &'a Out,
}

/// How long the first image on a server may take (a lazy first load
/// included), and a cancelled job to end.
pub const IMAGE_LIMIT: Duration = Duration::from_secs(10 * 60);
pub const CANCEL_LIMIT: Duration = Duration::from_secs(2 * 60);

impl Ctx<'_> {
    pub fn log(&self, text: impl Into<String>) {
        self.out.log(text);
    }

    pub fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Its client (an error if it isn't open: a suite bug, a case the
    /// catalogue should have skipped).
    pub fn client(&self, s: usize) -> Result<Client, String> {
        let p = self.images[s].proc.as_ref().ok_or_else(|| format!("{} isn't open", self.name(s)))?;
        Ok(p.client.clone())
    }

    pub fn name(&self, s: usize) -> &str {
        self.images[s].config.name()
    }

    /// Image servers open and still there.
    pub fn open_images(&self) -> Vec<usize> {
        (0..self.images.len()).filter(|&i| self.images[i].proc.is_some() && !self.images[i].gone).collect()
    }

    /// Its list of model files (None: it has none, any may be sent).
    pub fn loadable(&mut self, s: usize) -> Result<Option<Loadable>, String> {
        if let Some(l) = &self.images[s].loadable {
            return l.clone();
        }
        let r = match self.client(s)?.loadable() {
            Ok(l) => Ok(Some(l)),
            Err(increart_core::sdserver::ApiError::Http { status: 404, .. }) => Ok(None),
            Err(e) => Err(e.to_string()),
        };
        self.images[s].loadable = Some(r.clone());
        r
    }

    /// The model presets it can load (all without a list).
    pub fn models_for(&mut self, s: usize) -> Vec<usize> {
        let l = self.loadable(s).ok().flatten();
        (0..self.models.len()).filter(|&m| l.as_ref().is_none_or(|l| l.runs(&self.models[m].key.models))).collect()
    }

    /// The model preset to make images with on it: the one loaded if
    /// it's one, else the first it can load.
    pub fn model_for(&mut self, s: usize) -> Result<usize, String> {
        let can = self.models_for(s);
        let loaded = self.client(s)?.model_status().ok().and_then(|st| st.loaded());
        can.iter()
            .copied()
            .min_by_key(|&m| Some(&self.models[m].key) != loaded.as_ref())
            .ok_or_else(|| "none of the config's model presets has its files on it".to_string())
    }

    /// Loads model preset `m` on it unless loaded; the answer must name it.
    pub fn ensure_loaded(&self, s: usize, m: usize) -> Result<(), String> {
        let client = self.client(s)?;
        let want = &self.models[m].key;
        if client.model_status().ok().and_then(|st| st.loaded()).as_ref() == Some(want) {
            return Ok(());
        }
        self.log(format!("loading {} on {}", self.models[m].preset.name, self.name(s)));
        let t = Instant::now();
        match client.load_model(want) {
            Ok(st) if st.loaded().as_ref() == Some(want) => {
                self.log(format!("loaded in {:.1} s", t.elapsed().as_secs_f32()));
                Ok(())
            }
            Ok(st) => Err(format!(
                "asked to load {}, it answered {}",
                key_text(want),
                serde_json::to_string(&json!({"state": st.state, "recipe": st.recipe, "models": st.models}))
                    .unwrap_or_default()
            )),
            Err(e) => Err(format!("loading {} failed: {e}", self.models[m].preset.name)),
        }
    }

    /// An `img_gen` body for model preset `m`: small and quick (512×512,
    /// at most `steps` steps).
    pub fn body(&self, m: usize, seed: u64, steps: u32, refs: &[Vec<u8>]) -> Value {
        let p = &self.models[m].preset;
        let mut sample = json!({
            "sample_method": "euler",
            "sample_steps": steps.min(p.steps.max(1)).max(1),
            "guidance": { "txt_cfg": p.cfg },
        });
        if let Some(shift) = p.flow_shift {
            sample["flow_shift"] = json!(shift);
        }
        let opts = increart_core::request::GenOptions::default();
        increart_core::request::img_gen_body_from(
            "a red apple on a wooden table, photo",
            "",
            512,
            512,
            seed,
            sample,
            refs,
            &opts,
        )
    }

    /// Submits `body` on it and waits for the job to end.
    pub fn generate(&self, s: usize, body: &Value, limit: Duration) -> Result<Job, String> {
        let client = self.client(s)?;
        let id = client.img_gen(body).map_err(|e| format!("POST /sdcpp/v1/img_gen failed: {e}"))?;
        self.wait(&client, &id, limit, |_| true)
    }

    /// Polls job `id` until it ends (or `limit`, or the run is stopped:
    /// it's cancelled then); its statuses go to the log. `on` sees each
    /// answer; false stops waiting (Ok with that answer).
    pub fn wait(
        &self,
        client: &Client,
        id: &str,
        limit: Duration,
        mut on: impl FnMut(&Job) -> bool,
    ) -> Result<Job, String> {
        let t = Instant::now();
        let mut said = String::new();
        loop {
            if self.stopped() {
                let _ = client.cancel(id);
                return Err("stopped".into());
            }
            let job = client.job(id).map_err(|e| format!("job {id}: GET /sdcpp/v1/jobs/{id} failed: {e}"))?;
            let now = job_text(&job);
            if now != said {
                self.log(format!("job {id}: {now} ({:.1} s)", t.elapsed().as_secs_f32()));
                said = now;
            }
            if !on(&job) || job.status.is_terminal() {
                return Ok(job);
            }
            if t.elapsed() > limit {
                let _ = client.cancel(id);
                return Err(format!("job {id}: still {said} after {} s; cancelled", limit.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}

/// A job's state in a few words.
pub fn job_text(j: &Job) -> String {
    let status = status_name(j.status);
    match (&j.preview, j.status) {
        (_, JobStatus::Queued) if j.queue_position > 0 => format!("queued, position {}", j.queue_position),
        (Some(p), JobStatus::Generating) if p.total_steps > 0 => {
            format!("generating, step {}/{}", p.step, p.total_steps)
        }
        (_, JobStatus::Failed) => {
            let e = j.error.as_ref().map(|e| format!(": {} {}", e.code, e.message)).unwrap_or_default();
            format!("failed{e}")
        }
        _ => status.to_string(),
    }
}

pub fn status_name(s: JobStatus) -> &'static str {
    match s {
        JobStatus::Queued => "queued",
        JobStatus::Generating => "generating",
        JobStatus::Completed => "completed",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
    }
}

pub fn key_text(k: &ModelKey) -> String {
    serde_json::to_string(&json!({"recipe": k.recipe, "models": k.models})).unwrap_or_default()
}

/// A completed job's image: a PNG, and its size.
pub fn png_of(job: &Job) -> Result<(Vec<u8>, u32, u32), String> {
    if job.status != JobStatus::Completed {
        return Err(format!("job {} ended {}", job.id, job_text(job)));
    }
    let bytes = job.image().map_err(|e| e.to_string())?;
    if !bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Err(format!("job {}: its image isn't a PNG ({} bytes)", job.id, bytes.len()));
    }
    let img = image::load_from_memory(&bytes).map_err(|e| format!("job {}: its PNG doesn't decode: {e}", job.id))?;
    Ok((bytes, img.width(), img.height()))
}

/// A PNG of `size`², a gradient (`noise`: random pixels, which don't
/// compress: a large request).
pub fn test_png(size: u32, noise: bool) -> Vec<u8> {
    let mut state = 0x2545_f491_u32;
    let img = image::RgbImage::from_fn(size, size, |x, y| {
        if noise {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let [a, b, c, _] = state.to_le_bytes();
            image::Rgb([a, b, c])
        } else {
            image::Rgb([(x * 255 / size) as u8, (y * 255 / size) as u8, 128])
        }
    });
    let mut out = vec![];
    // (writing to memory: an error would be the encoder's; an empty image
    // is then refused by the server, failing the case)
    if image::DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).is_err() {
        out.clear();
    }
    out
}

/// One colour, `size`² (for asking a vision model what it sees).
pub fn solid_png(size: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(size, size, image::Rgb(rgb));
    let mut out = vec![];
    // (writing to memory: an error would be the encoder's; an empty image
    // is then refused by the server, failing the case)
    if image::DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).is_err() {
        out.clear();
    }
    out
}
