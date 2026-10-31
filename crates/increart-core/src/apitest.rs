//! `increart-apitest`'s protocol (DESIGN.md, Validating servers): the
//! cases it has (`list --json`), and a run's events as JSON lines on its
//! stdout (`run --jsonl`), which the app's **Validate servers** window
//! reads. A `stop` line on its stdin stops the run (cancelling its jobs on
//! the servers), on Windows too.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The program, beside the app's in a release.
pub const BINARY: &str = "increart-apitest";

/// What a case costs the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cost {
    /// Calls only: nothing loaded, nothing generated.
    Light,
    /// Loads a model preset (or starts a server).
    Loads,
    /// Generates: GPU time.
    Gpu,
}

impl Cost {
    pub fn label(self) -> &'static str {
        match self {
            Cost::Light => "light",
            Cost::Loads => "loads a model",
            Cost::Gpu => "GPU time",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    /// It works, but not as IncreArt would like.
    Warn,
    Fail,
    /// Not for this server or setup (the case says why).
    NotApplicable,
    /// Not run: what it needs failed, or the run was stopped.
    Skipped,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Warn => "warning",
            Outcome::Fail => "FAIL",
            Outcome::NotApplicable => "n/a",
            Outcome::Skipped => "skipped",
        }
    }

    /// Worse first: a group shows its worst.
    pub fn rank(self) -> u8 {
        match self {
            Outcome::Fail => 0,
            Outcome::Warn => 1,
            Outcome::Skipped => 2,
            Outcome::Pass => 3,
            Outcome::NotApplicable => 4,
        }
    }
}

/// One test case of one server (or of a preset's servers together).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Case {
    /// `preset/server/group.key`, or `preset/group.key` for the servers
    /// together.
    pub id: String,
    pub preset: String,
    /// None: the preset's servers together.
    pub server: Option<String>,
    pub group: String,
    pub title: String,
    pub cost: Cost,
    /// One of the basic cases ([`crate::validation::BASIC`]): a run of all
    /// of a preset's stamps it validated, or not.
    #[serde(default)]
    pub basic: bool,
    /// Why it isn't for this server or setup (it's then n/a).
    pub not_applicable: Option<String>,
    /// What IncreArt expects of the server here (for whoever reads the
    /// report: the API contract in a few lines).
    pub expects: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The cases, in the order they run.
    Plan {
        cases: Vec<Case>,
    },
    Start {
        id: String,
    },
    /// A line of a case's log (None: of the run, between cases).
    Log {
        id: Option<String>,
        text: String,
    },
    /// A request and its answer, during a case.
    Traffic {
        id: Option<String>,
        text: String,
    },
    End {
        id: String,
        outcome: Outcome,
        summary: String,
        secs: f64,
    },
    /// The run is over: its report's folder and zip.
    Done {
        counts: Counts,
        report: Option<PathBuf>,
        zip: Option<PathBuf>,
    },
    /// It couldn't run at all (no config, a bad argument).
    Error {
        text: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub pass: u32,
    pub warn: u32,
    pub fail: u32,
    pub not_applicable: u32,
    pub skipped: u32,
}

impl Counts {
    pub fn add(&mut self, o: Outcome) {
        match o {
            Outcome::Pass => self.pass += 1,
            Outcome::Warn => self.warn += 1,
            Outcome::Fail => self.fail += 1,
            Outcome::NotApplicable => self.not_applicable += 1,
            Outcome::Skipped => self.skipped += 1,
        }
    }

    pub fn line(&self) -> String {
        format!(
            "{} passed, {} warnings, {} failed, {} skipped, {} n/a",
            self.pass, self.warn, self.fail, self.skipped, self.not_applicable
        )
    }
}

/// `increart-apitest` beside this program (a release has both), else
/// its bare name: then it's looked for on PATH.
pub fn binary() -> PathBuf {
    let name = format!("{BINARY}{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(&name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Whether `pattern` (`*` for any text) picks the case `id`.
pub fn matches(pattern: &str, id: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == id;
    }
    let mut rest = id;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(at) = rest.find(part) else { return false };
            rest = &rest[at + part.len()..];
        }
    }
    true
}

/// The folders sd-server looks for model files in, under each model
/// directory (its `model_catalog.cpp`; matched ignoring case).
const SD_SERVER_FOLDERS: &[&str] = &[
    "checkpoints",
    "stable-diffusion",
    "diffusion_models",
    "text_encoders",
    "clip_vision",
    "tokenizers",
    "vae",
    "vae_approx",
    "controlnet",
    "ipadapter",
    "motion_modules",
    "photomaker",
    "pulid",
    "loras",
    "lora",
];

/// What to try, for a case that failed or warned, from what it said: the
/// common causes, in words for someone setting up servers. None when
/// there's nothing better to say than the result itself.
pub fn hint(outcome: Outcome, summary: &str) -> Option<String> {
    if !matches!(outcome, Outcome::Fail | Outcome::Warn) {
        return None;
    }
    let has = |t: &str| summary.contains(t);
    if let Some(file) = summary.split("it doesn't have ").nth(1) {
        let file = file.split([';', ')', ' ']).next().unwrap_or(file);
        let folder = file.split('/').next().unwrap_or(file);
        let standard = SD_SERVER_FOLDERS.iter().any(|f| f.eq_ignore_ascii_case(folder));
        let kinds = "diffusion_models/ for the model, text_encoders/ for text encoders and mmproj files, vae/ \
                     for the VAE";
        return Some(if standard || !file.contains('/') {
            format!(
                "No server lists {file}. For a started sd-server, put it in that folder of one of the model \
                 directories (Server settings…); a connected-to server looks in its own model folders. Or change \
                 the model preset's files (Edit model presets…)."
            )
        } else {
            format!(
                "sd-server only looks in a model directory's standard folders ({kinds}, loras/, checkpoints/…), \
                 and {folder}/ isn't one, so it can't see {file}. If this model preset is meant for KoboldCpp, \
                 that's expected: a KoboldCpp preset runs it. To run it on sd-server, move or link its files \
                 into the right folders and pick them again in Edit model presets…."
            )
        });
    }
    if has("no server answers on") {
        return Some(
            "Nothing answered at that address. Check the server is running and the preset's URL and port are \
             right; for another machine, that it listens on its network address (sd-server --listen-ip 0.0.0.0) \
             and no firewall blocks the port."
                .into(),
        );
    }
    if has("not found: it isn't a file") {
        return Some("The server program isn't where the settings say: set its path in Server settings….".into());
    }
    if has("exited (") && (has("start-up") || has("while starting")) {
        return Some(
            "The server stopped as it started: the end of its log (in the report zip, or Logs… on the Servers \
             screen) says why. Often a device that doesn't exist, a build for another GPU, or the port already \
             in use by another server."
                .into(),
        );
    }
    if has("its log quiet for") {
        return Some(
            "The server went quiet before answering: it may be stuck, or slow to start on this machine. Check \
             the end of its log; if it was still loading, raise Timeouts: Server start in Server settings…."
                .into(),
        );
    }
    if has("413") {
        return Some(
            "Something in front of the server limits a request's size, often a proxy: raise its limit (nginx: \
             client_max_body_size 64m). Until then runs with large reference images fail."
                .into(),
        );
    }
    if has("out of memory") || has("OOM") {
        return Some(
            "The GPU ran out of memory: try a smaller quantisation of the model, another GPU, or close what \
             else is using it."
                .into(),
        );
    }
    if has("couldn't load a model preset") || has("loading failed") || has("refused it") || has("listed but refused") {
        return Some(
            "The server has the files but couldn't load them: the server's log says why (a file that's \
             damaged or of a kind this build doesn't support, or too little memory)."
                .into(),
        );
    }
    if has("401") || has("403") || has("password") {
        return Some("The server wants a password, or another one: check the one in the preset.".into());
    }
    if has("404") {
        return Some(
            "Something answers there, but not with the API IncreArt uses: check it's sd-server or KoboldCpp, \
             recent enough, and that the URL has no extra path."
                .into(),
        );
    }
    if has("has no img_gen") {
        return Some("The server doesn't offer image generation: check what it was started with.".into());
    }
    if has("may not take images") {
        return Some(
            "Asking about an image needs a vision model: start KoboldCpp with the model's mmproj file.".into(),
        );
    }
    if has("slow for twice-a-second polls") {
        return Some(
            "The server answers slowly: over Wi-Fi or a VPN, or a busy machine. Runs still work, more slowly.".into(),
        );
    }
    if has("a bug in increart-apitest") {
        return Some("That's a bug in the validation itself, not your servers: please send the report.".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_name_the_common_causes() {
        let routing = "Qwen-Image 2.1 GGUF Q8_0 (CFG 2, refs): no server has its files (sd-rocm0: it doesn't have \
                       koboldcpp/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf; sd-rocm1: it doesn't have \
                       koboldcpp/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf)";
        let h = hint(Outcome::Warn, routing).unwrap();
        assert!(
            h.contains("koboldcpp/ isn't one") && h.contains("koboldcpp/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf"),
            "{h}"
        );
        let h =
            hint(Outcome::Warn, "x: no server; its runs would wait (gpu0: it doesn't have vae/x.safetensors)").unwrap();
        assert!(h.starts_with("No server lists vae/x.safetensors."), "{h}");
        let h = hint(Outcome::Fail, "1 of 1 didn't open: box: box: no server answers on http://10.0.0.2:1234").unwrap();
        assert!(h.contains("--listen-ip"), "{h}");
        let h = hint(Outcome::Warn, "413 for 9.1 MB (Payload Too Large): runs with large reference images will fail")
            .unwrap();
        assert!(h.contains("client_max_body_size"), "{h}");
        assert!(hint(Outcome::Fail, "gpu0: the server exited (exit status: 1) during start-up; log x").is_some());
        assert_eq!(hint(Outcome::Pass, "no server answers on x"), None, "only for failures and warnings");
        assert_eq!(hint(Outcome::Fail, "something else"), None);
    }

    #[test]
    fn patterns() {
        assert!(matches("Remote/proxy/jobs.cancel-queued", "Remote/proxy/jobs.cancel-queued"));
        assert!(!matches("Remote/proxy", "Remote/proxy/jobs.queue"));
        assert!(matches("Remote/proxy/*", "Remote/proxy/jobs.queue"));
        assert!(matches("*/jobs.*", "Remote/proxy/jobs.queue"));
        assert!(matches("*cancel*", "Remote/proxy/jobs.cancel-queued"));
        assert!(!matches("*/models.*", "Remote/proxy/jobs.queue"));
    }

    #[test]
    fn events_are_json_lines() {
        let e = Event::End { id: "a/b".into(), outcome: Outcome::NotApplicable, summary: "x".into(), secs: 1.5 };
        let text = serde_json::to_string(&e).unwrap();
        assert_eq!(text, r#"{"event":"end","id":"a/b","outcome":"not_applicable","summary":"x","secs":1.5}"#);
        assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), e);
    }
}
