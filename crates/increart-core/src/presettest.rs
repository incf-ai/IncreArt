//! A model preset's **Test** (DESIGN.md, Testing a model preset): one
//! server loads it (each slot's file it has, as a run would) and makes one
//! small image with its settings and LoRAs. The results are kept per
//! preset and server in `preset-tests.rsn` in the app's state folder, the
//! images beside it in `preset-tests/`, so the presets list says which
//! were tested where.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::RecipePreset;
use crate::model::{LoraFile, Sample};
use crate::sdserver::{ApiError, Client, JobStatus, ModelKey};

pub const FILE: &str = "preset-tests.rsn";

/// The prompt of a test's image (the LoRAs' trigger words before it).
pub const PROMPT: &str = "a red apple on a wooden table, photo";

/// How long the image may take (a preset's full steps, 512 × 512).
const IMAGE_LIMIT: Duration = Duration::from_secs(15 * 60);

/// `preset-tests.rsn` in the app's state folder.
pub fn path() -> Option<PathBuf> {
    crate::config::user_dir("XDG_STATE_HOME", ".local/state", "LOCALAPPDATA").map(|d| d.join(FILE))
}

/// Where the images of the results kept at `path` go.
pub fn images_dir(path: &Path) -> PathBuf {
    path.with_file_name("preset-tests")
}

/// What a result holds for: the preset's model and settings (not its name,
/// note or download links). An edit since makes it out of date.
pub fn fingerprint(p: &RecipePreset) -> String {
    let what = (&p.recipe, &p.files, &p.alternatives, &p.loras, p.steps, p.cfg, p.flow_shift, &p.custom_sigmas);
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(&what).unwrap_or_default());
    h.update(serde_json::to_vec(&(&p.sampler, &p.scheduler)).unwrap_or_default());
    h.finalize()[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// One test's result.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tried {
    pub preset: String,
    pub server: String,
    /// The preset's [`fingerprint`] then.
    pub fingerprint: String,
    /// When (RFC 3339, local time).
    pub when: String,
    /// What it loaded, slot → file ID.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub load_s: Option<f32>,
    #[serde(default)]
    pub image_s: Option<f32>,
    /// Why it failed; None: it made its image.
    #[serde(default)]
    pub error: Option<String>,
    /// The image, a PNG.
    #[serde(default)]
    pub image: Option<PathBuf>,
}

impl Tried {
    pub fn passed(&self) -> bool {
        self.error.is_none()
    }

    /// Of `p` as it is now.
    pub fn current(&self, p: &RecipePreset) -> bool {
        self.fingerprint == fingerprint(p)
    }
}

/// The results, the latest per preset and server.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PresetTests {
    #[serde(default)]
    pub tried: Vec<Tried>,
}

impl PresetTests {
    /// Empty when there's none or it can't be read.
    pub fn load(path: &Path) -> PresetTests {
        crate::rsnfmt::read(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> crate::error::Result<()> {
        crate::rsnfmt::write(path, self)
    }

    /// Keeps `t`, in place of the last of its preset on its server.
    pub fn record(&mut self, t: Tried) {
        self.tried.retain(|o| !(o.preset == t.preset && o.server == t.server));
        self.tried.push(t);
    }

    /// The results of preset `name`, by server name.
    pub fn of<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Tried> + 'a {
        self.tried.iter().filter(move |t| t.preset == name)
    }

    /// A preset renamed: its results follow it.
    pub fn rename(&mut self, from: &str, to: &str) {
        for t in self.tried.iter_mut().filter(|t| t.preset == from) {
            t.preset = to.to_string();
        }
    }
}

/// Where a test is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Starting the server (or connecting).
    Opening,
    /// Asking which files it has.
    Listing,
    Loading,
    Generating,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Opening => "opening the server",
            Stage::Listing => "asking for its files",
            Stage::Loading => "loading",
            Stage::Generating => "making the image",
        }
    }
}

/// What the test found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub files: BTreeMap<String, String>,
    pub load_s: Option<f32>,
    pub image_s: Option<f32>,
    pub png: Option<Vec<u8>>,
    pub error: Option<String>,
}

impl Outcome {
    pub fn failed(why: impl Into<String>) -> Outcome {
        Outcome { error: Some(why.into()), ..Default::default() }
    }
}

/// The image request: 512 × 512, the preset's sampler, steps, guidance and
/// LoRAs, its trigger words before [`PROMPT`].
pub fn body(p: &RecipePreset) -> serde_json::Value {
    let sample = Sample {
        method: p.sampler.clone(),
        scheduler: p.scheduler.clone(),
        steps: p.steps.max(1),
        cfg: p.cfg,
        flow_shift: p.flow_shift,
        custom_sigmas: p.custom_sigmas.clone(),
    };
    let prompt = p.prompt_queued(PROMPT);
    let opts = crate::request::GenOptions::default();
    let sample = crate::request::sample_params(&sample);
    let mut body = crate::request::img_gen_body_from(&prompt, "", 512, 512, 1, sample, &[], &opts);
    if !p.loras.is_empty() {
        let loras: Vec<LoraFile> =
            p.loras.iter().map(|l| LoraFile { id: l.file.clone(), sha256: String::new(), weight: l.weight }).collect();
        body["lora"] = crate::request::lora_field(&loras);
    }
    body
}

/// Tests `p` on the server `client` talks to; `stage` hears each stage as
/// it starts. `cancel` stops it between calls (a load under way is waited
/// for; a job is cancelled).
pub fn run(client: &Client, p: &RecipePreset, cancel: &AtomicBool, stage: &mut dyn FnMut(Stage)) -> Outcome {
    let stopped = || cancel.load(Ordering::Relaxed);
    let mut out = Outcome::default();
    stage(Stage::Listing);
    out.files = match client.loadable() {
        Ok(l) => match l.why_not_preset(p) {
            Some(why) => return Outcome::failed(format!("it can't load it: {why}")),
            None => l.pick(p),
        },
        // no list: the preferred files, it says if it hasn't them
        Err(ApiError::Http { status: 404, .. }) => p.files.clone(),
        Err(e) => return Outcome::failed(format!("asking for its files failed: {e}")),
    };
    if stopped() {
        return Outcome { error: Some("stopped".into()), ..out };
    }
    stage(Stage::Loading);
    let key = ModelKey { recipe: p.recipe.clone(), models: out.files.clone() };
    let t = Instant::now();
    match client.load_model(&key) {
        Ok(st) if st.holds(&key) => out.load_s = Some(t.elapsed().as_secs_f32()),
        Ok(st) => {
            let answered = serde_json::json!({ "state": st.state, "recipe": st.recipe, "models": st.models });
            out.error = Some(format!("the load didn't take: it answered {answered}"));
            return out;
        }
        Err(e) => {
            out.error = Some(format!("loading failed: {e}"));
            return out;
        }
    }
    if stopped() {
        out.error = Some("stopped".into());
        return out;
    }
    stage(Stage::Generating);
    let t = Instant::now();
    let id = match client.img_gen(&body(p)) {
        Ok(id) => id,
        Err(e) => {
            out.error = Some(format!("the image request failed: {e}"));
            return out;
        }
    };
    let job = loop {
        if stopped() {
            let _ = client.cancel(&id);
            out.error = Some("stopped".into());
            return out;
        }
        match client.job(&id) {
            Ok(job) if job.status.is_terminal() => break job,
            Ok(_) => {}
            Err(e) => {
                out.error = Some(format!("asking for the image's job failed: {e}"));
                return out;
            }
        }
        if t.elapsed() > IMAGE_LIMIT {
            let _ = client.cancel(&id);
            out.error = Some(format!("no image after {} min", IMAGE_LIMIT.as_secs() / 60));
            return out;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    if job.status != JobStatus::Completed {
        let why = job.error.as_ref().map(|e| format!("{}: {}", e.code, e.message)).unwrap_or_default();
        out.error =
            Some(if why.is_empty() { "the image job failed".into() } else { format!("the image failed: {why}") });
        return out;
    }
    match job.image() {
        Ok(png) if png.starts_with(b"\x89PNG\r\n\x1a\n") => {
            out.image_s = Some(t.elapsed().as_secs_f32());
            out.png = Some(png);
        }
        Ok(_) => out.error = Some("its image isn't a PNG".into()),
        Err(e) => out.error = Some(format!("its image couldn't be read: {e}")),
    }
    out
}

/// Keeps `out` as `p`'s result on `server`: the image into `dir` (named
/// for both), the result returned for [`PresetTests::record`].
pub fn tried(p: &RecipePreset, server: &str, out: Outcome, dir: Option<&Path>) -> Tried {
    let image = match (&out.png, dir) {
        (Some(png), Some(dir)) => {
            let safe = |s: &str| s.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>();
            let path = dir.join(format!("{}--{}--{}.png", safe(&p.name), safe(server), fingerprint(p)));
            std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, png)).ok().map(|_| path)
        }
        _ => None,
    };
    Tried {
        preset: p.name.clone(),
        server: server.to_string(),
        fingerprint: fingerprint(p),
        when: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        files: out.files,
        load_s: out.load_s,
        image_s: out.image_s,
        error: out.error,
        image,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PresetLora;

    fn preset() -> RecipePreset {
        RecipePreset {
            name: "Apple".into(),
            recipe: "qwen-image-21".into(),
            files: BTreeMap::from([("diffusion_model".to_string(), "a.safetensors".to_string())]),
            steps: 8,
            cfg: 2.5,
            sampler: "euler".into(),
            loras: vec![PresetLora { file: "loras/s.safetensors".into(), weight: 0.7, trigger: "sketch".into() }],
            ..Default::default()
        }
    }

    #[test]
    fn the_request_has_the_presets_settings_and_loras() {
        let b = body(&preset());
        assert_eq!(b["prompt"], format!("sketch, {PROMPT}"));
        assert_eq!(b["width"], 512);
        assert_eq!(b["sample_params"]["sample_steps"], 8);
        assert_eq!(b["sample_params"]["sample_method"], "euler");
        assert_eq!(b["lora"][0]["multiplier"], 0.7);
    }

    #[test]
    fn an_edit_makes_a_result_out_of_date_a_rename_or_note_doesnt() {
        let p = preset();
        let t = tried(&p, "sd0", Outcome::default(), None);
        assert!(t.passed() && t.current(&p));
        let renamed = RecipePreset { name: "Pear".into(), note: "n".into(), ..p.clone() };
        assert!(t.current(&renamed));
        assert!(!t.current(&RecipePreset { steps: 9, ..p.clone() }));
        assert!(!t.current(&RecipePreset { scheduler: "karras".into(), ..p }));
    }

    #[test]
    fn the_latest_result_per_server_is_kept() {
        let p = preset();
        let mut all = PresetTests::default();
        all.record(tried(&p, "sd0", Outcome::failed("x"), None));
        all.record(tried(&p, "sd1", Outcome::default(), None));
        all.record(tried(&p, "sd0", Outcome::default(), None));
        assert_eq!(all.of("Apple").count(), 2);
        assert!(all.of("Apple").all(Tried::passed));
        all.rename("Apple", "Pear");
        assert_eq!(all.of("Pear").count(), 2);
    }
}
