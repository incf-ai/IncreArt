//! Prompt enhance (PE): an OpenAI-compatible chat server, KoboldCpp started
//! on demand or one already running, that suggests prompt text, looking at
//! the reference images when the model has vision (DESIGN.md, Prompt
//! rewriting).

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ureq::Agent;

/// The job filling in a sweep's `{blanks}`.
pub const FILL_BLANKS: &str = "fill_blanks";

/// The built-in system prompt for [`FILL_BLANKS`].
pub const DEFAULT_FILL_BLANKS: &str = "You help fill in the blanks of prompts for an image generator. \
A blank is written {name} in a prompt. You are given the subject (the user's template), the style \
prompt(s) the blank appears in, the blanks already filled in, and possibly reference images: the first \
image attached is <image1>, the second <image2>, and so on. When there are images, look at them closely \
and suggest values that suit what they show (their subject, materials, colours, setting, era, text). \
Reply with only a JSON array of strings: distinct suggestions, each a short phrase (1 to 6 words) that \
reads naturally in place of the blank, concrete and visual, and varied from each other. No explanations.";

/// The job rewriting a prompt (the Generate prompt, an idea's text).
pub const REWRITE: &str = "rewrite";

/// The built-in system prompt for [`REWRITE`].
pub const DEFAULT_REWRITE: &str = "You rewrite prompts for an image generator (a diffusion model that also \
edits images) so they give better pictures: clearer, more specific and more visual, describing the subject, \
composition, setting, lighting, colours, materials and style. Keep the user's intent and everything they \
asked for; don't add subjects they didn't ask for. Keep these exactly as written, wherever they make sense: \
tags like <image1> and <image2> (they name the attached reference images, the first image attached is \
<image1>), blanks like {name} and {preset}. When images are attached, look at them and use what they show. \
Follow the user's instructions for the rewrite if there are any. Write it in the prompt's language. Reply \
with only the rewritten prompt: no title, no quotes, no explanations.";

/// The job improving a style sweep's prompt for one style.
pub const STYLE_PROMPT: &str = "style_prompt";

/// The built-in system prompt for [`STYLE_PROMPT`].
pub const DEFAULT_STYLE_PROMPT: &str = "You improve prompts for an image generator (a diffusion model that \
also edits images). The user is trying one subject in many styles: the template is the subject, the style is \
one way of picturing it, and the prompt is the two put together. Make the prompt give a better picture in that \
style: clearer, more specific and more visual, with the composition, setting, lighting, colours, materials and \
textures the style calls for. Keep the subject and everything the template asks for, and keep the style's look; \
don't add subjects that aren't asked for. Fill every blank like {material} with something that suits the style \
and the subject (and the images, when attached), so none is left. Keep tags like <image1> exactly as written \
(they name the attached reference images, the first image attached is <image1>). When images are attached, look \
at them and use what they show. Follow the user's instructions if there are any. Write it in the prompt's \
language. Reply with only the improved prompt: no title, no quotes, no explanations.";

/// The job describing an image.
pub const DESCRIBE: &str = "describe";

/// The built-in system prompt for [`DESCRIBE`].
pub const DEFAULT_DESCRIBE: &str = "You describe images as prompts for an image generator. Look closely at \
the attached image and describe it so the generator could make a picture like it: the subject and what it \
is doing, the composition and framing, the setting, lighting, colours, materials and textures, and the \
style or medium (photo, painting, 3D render, …), with any text in it quoted. Follow the user's \
instructions if there are any. Reply with only the description, one paragraph: no title, no preamble.";

/// The jobs a server's system prompts are for, with their built-in prompt.
pub const JOBS: [(&str, &str); 4] = [
    (FILL_BLANKS, DEFAULT_FILL_BLANKS),
    (STYLE_PROMPT, DEFAULT_STYLE_PROMPT),
    (REWRITE, DEFAULT_REWRITE),
    (DESCRIBE, DEFAULT_DESCRIBE),
];

/// Prompt enhance settings shared by every chat server; the servers
/// themselves are entries of the server presets ([`crate::config::ServerConfig::Chat`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Pe", default)]
pub struct PeConfig {
    /// System prompts by job ([`FILL_BLANKS`]); a missing or empty one is
    /// the built-in default.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub system_prompts: BTreeMap<String, String>,
    /// Sampling by job; a missing one is [`Sampling::default_for`] it.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub sampling: BTreeMap<String, Sampling>,
    /// Before chat servers were preset entries: the one used, by name
    /// (read, never written; [`crate::config::Config::normalize`] moves them).
    #[serde(skip_serializing)]
    pub server: String,
    #[serde(skip_serializing)]
    pub servers: Vec<OldPeServer>,
}

impl PeConfig {
    /// The system prompt for `job`: the one set, else the default.
    pub fn system_prompt(&self, job: &str) -> &str {
        match self.system_prompts.get(job) {
            Some(p) if !p.trim().is_empty() => p,
            _ => JOBS.iter().find(|(j, _)| *j == job).map_or("", |(_, p)| p),
        }
    }

    /// The sampling for `job`: the one set, else the default.
    pub fn sampling(&self, job: &str) -> Sampling {
        self.sampling.get(job).copied().unwrap_or_else(|| Sampling::default_for(job))
    }
}

/// A chat server: how it runs and when it starts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "PeServer", default)]
pub struct PeServer {
    pub name: String,
    pub run: PeRun,
    /// Started (or connected to) when first asked, or when its preset opens.
    #[serde(skip_serializing_if = "is_default")]
    pub start: PeStart,
    /// Known to have no vision though `run` connects: a KoboldCpp image
    /// server's chat side started without a projector. Not saved.
    #[serde(skip)]
    pub text_only: bool,
}

/// A PE server as config files had them, with its own system prompts and
/// sampling.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename = "PeServer", default)]
pub struct OldPeServer {
    pub name: String,
    pub run: PeRun,
    pub start: PeStart,
    pub system_prompts: BTreeMap<String, String>,
    pub sampling: BTreeMap<String, Sampling>,
}

/// How the answer's words are drawn: hotter is more varied, and `min_p`
/// drops words less than that fraction as likely as the likeliest, keeping
/// a hot answer sensible (0: off).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Sampling")]
pub struct Sampling {
    pub temperature: f32,
    pub min_p: f32,
}

// never NaN: set from the settings' sliders or config.rsn
impl Eq for Sampling {}

impl Sampling {
    /// Suggestions want variety; rewrites and descriptions less so.
    pub fn default_for(job: &str) -> Sampling {
        match job {
            FILL_BLANKS => Sampling { temperature: 1.1, min_p: 0.05 },
            _ => Sampling { temperature: 0.9, min_p: 0.0 },
        }
    }
}

/// When the app starts a prompt-enhance server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeStart {
    /// When first asked (the first ask waits for the model to load).
    #[default]
    OnDemand,
    /// When its preset opens (or it's picked to continue without
    /// sd-servers).
    AtLaunch,
}

/// What KoboldCpp runs the model on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeDevice {
    /// KoboldCpp's own choice (or what `args` say).
    #[default]
    Auto,
    /// The CPU, with this many threads (0: KoboldCpp's choice).
    Cpu { threads: u32 },
    /// GPUs, through `runtime` (numbered as its [`list_gpus`] lists them),
    /// every layer on them; none listed: every GPU.
    Gpu {
        devices: Vec<u32>,
        #[serde(default, skip_serializing_if = "is_default")]
        runtime: GpuRuntime,
    },
}

/// How KoboldCpp reaches the GPUs: the build must have it (CUDA and ROCm
/// builds are separate downloads).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GpuRuntime {
    /// Any GPU with a Vulkan driver; every KoboldCpp build has it.
    #[default]
    Vulkan,
    /// NVIDIA.
    Cuda,
    /// AMD, with a ROCm (hipBLAS) build of KoboldCpp.
    Rocm,
    /// Apple silicon: KoboldCpp's Mac build, its only GPU runtime.
    Metal,
}

impl GpuRuntime {
    pub const ALL: [GpuRuntime; 4] = [GpuRuntime::Vulkan, GpuRuntime::Cuda, GpuRuntime::Rocm, GpuRuntime::Metal];

    /// The ones this OS has: Metal on a Mac, the others elsewhere.
    pub fn here() -> Vec<GpuRuntime> {
        GpuRuntime::ALL.into_iter().filter(|r| (*r == GpuRuntime::Metal) == cfg!(target_os = "macos")).collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            GpuRuntime::Vulkan => "Vulkan",
            GpuRuntime::Cuda => "CUDA",
            GpuRuntime::Rocm => "ROCm",
            GpuRuntime::Metal => "Metal",
        }
    }

    /// What lists the GPUs, numbered as KoboldCpp numbers them (Metal: the
    /// app itself, from the RAM).
    pub fn tool(self) -> &'static str {
        match self {
            GpuRuntime::Vulkan => "vulkaninfo",
            GpuRuntime::Cuda => "nvidia-smi",
            GpuRuntime::Rocm => "rocm-smi",
            GpuRuntime::Metal => "",
        }
    }

    /// Metal on a Mac; else CUDA when `nvidia-smi` runs, else Vulkan (ROCm
    /// only when chosen: it needs a ROCm build).
    pub fn likely() -> GpuRuntime {
        if cfg!(target_os = "macos") {
            return GpuRuntime::Metal;
        }
        let mut cmd = Command::new("nvidia-smi");
        let cuda = crate::process::background(&mut cmd, false)
            .arg("-L")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if cuda { GpuRuntime::Cuda } else { GpuRuntime::Vulkan }
    }

    /// Whether `binary` (by its file name) can't have this runtime:
    /// KoboldCpp's `nocuda` builds have Vulkan only.
    pub fn missing_from(self, binary: &str) -> bool {
        matches!(self, GpuRuntime::Cuda | GpuRuntime::Rocm) && binary.to_lowercase().contains("nocuda")
    }
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

impl PeServer {
    /// Whether `other` runs as the same process (only its name or when it
    /// starts differ).
    pub fn same_process(&self, other: &PeServer) -> bool {
        self.run == other.run
    }

    pub fn base_url(&self) -> String {
        match &self.run {
            PeRun::KoboldCpp { host, port, .. } => format!("http://{host}:{port}"),
            PeRun::Connect { url } => url.trim_end_matches('/').to_string(),
        }
    }

    /// Whether it sees images, when that's known: a KoboldCpp with
    /// `mmproj` does, one without doesn't (unless its `.kcpps` names the
    /// model: unknown), a Connect server is unknown.
    pub fn vision_known(&self) -> Option<bool> {
        match &self.run {
            _ if self.text_only => Some(false),
            PeRun::KoboldCpp { mmproj, .. } if !mmproj.is_empty() => Some(true),
            PeRun::KoboldCpp { model, config, .. } if model.is_empty() && !config.is_empty() => None,
            PeRun::KoboldCpp { .. } => Some(false),
            PeRun::Connect { .. } => None,
        }
    }

    /// Whether requests carry images: unless it's known to have no vision.
    pub fn vision(&self) -> bool {
        self.vision_known() != Some(false)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeRun {
    /// The app starts KoboldCpp with this model when first asked, and
    /// stops it on quitting.
    KoboldCpp {
        binary: String,
        /// The GGUF model (empty: the one `config` names).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        model: String,
        /// The vision projector (GGUF); empty: text only.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        mmproj: String,
        /// A KoboldCpp settings file (`.kcpps`, saved from its own
        /// launcher) to start from; the settings here go over it.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        config: String,
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "is_default")]
        device: PeDevice,
        /// `--contextsize` (0: KoboldCpp's default).
        #[serde(default, skip_serializing_if = "is_default")]
        context: u32,
        /// More arguments (`--blasthreads 24`, …).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    /// An OpenAI-compatible chat server already running.
    Connect { url: String },
}

impl Default for PeRun {
    fn default() -> Self {
        PeRun::Connect { url: "http://127.0.0.1:5001".into() }
    }
}

/// The GPUs `runtime` sees, as (number for [`PeDevice::Gpu`], name).
pub fn list_gpus(runtime: GpuRuntime) -> Result<Vec<(u32, String)>, String> {
    if runtime != GpuRuntime::Vulkan {
        return crate::tiers::detect_gpus(runtime).map(|l| l.into_iter().map(|g| (g.number, g.name)).collect());
    }
    let mut cmd = Command::new("vulkaninfo");
    let out = crate::process::background(&mut cmd, false)
        .arg("--summary")
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("vulkaninfo: {e} (install vulkan-tools to list the GPUs)"))?;
    let list = parse_vulkaninfo(&String::from_utf8_lossy(&out.stdout));
    if list.is_empty() {
        return Err("vulkaninfo listed no GPU".into());
    }
    Ok(list)
}

/// `vulkaninfo --summary`'s devices: `GPU1:` then `deviceType = …` and
/// `deviceName = …` lines (KoboldCpp numbers them the same, leaving out the
/// CPU ones, which come last).
pub fn parse_vulkaninfo(text: &str) -> Vec<(u32, String)> {
    let mut list = vec![];
    let mut current: Option<(u32, bool)> = None;
    for l in text.lines().map(str::trim) {
        if let Some(n) = l.strip_prefix("GPU").and_then(|r| r.strip_suffix(':')).and_then(|n| n.parse().ok()) {
            current = Some((n, true));
        } else if let Some((key, value)) = l.split_once('=') {
            match (key.trim(), &mut current) {
                ("deviceType", Some((_, real))) => *real = !value.contains("CPU"),
                ("deviceName", Some((n, true))) => list.push((*n, value.trim().to_string())),
                _ => {}
            }
        }
    }
    list
}

/// The KoboldCpp command line (empty for Connect). A settings file comes
/// first: KoboldCpp lets what follows it on the command line override it.
pub fn command_args(run: &PeRun) -> Vec<String> {
    let PeRun::KoboldCpp { model, mmproj, config, host, port, device, context, args, .. } = run else {
        return vec![];
    };
    let mut v: Vec<String> = vec![];
    if !config.is_empty() {
        v.extend(["--config".into(), config.clone()]);
    }
    if !model.is_empty() {
        v.extend(["--model".into(), model.clone()]);
    }
    if !mmproj.is_empty() {
        v.extend(["--mmproj".into(), mmproj.clone()]);
    }
    v.extend(["--host".into(), host.clone(), "--port".into(), port.to_string()]);
    v.extend(["--skiplauncher", "--quiet", "--jinja"].map(String::from));
    v.extend(device_args(device));
    if *context > 0 {
        v.extend(["--contextsize".into(), context.to_string()]);
    }
    v.extend(args.iter().cloned());
    v
}

/// The environment for [`command_args`].
pub fn run_env(run: &PeRun) -> Vec<(String, String)> {
    match run {
        PeRun::KoboldCpp { device, .. } => device_env(device),
        PeRun::Connect { .. } => vec![],
    }
}

/// KoboldCpp's flags for `device`.
pub fn device_args(device: &PeDevice) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    match device {
        PeDevice::Auto => {}
        PeDevice::Cpu { threads } => {
            v.push("--usecpu".into());
            if *threads > 0 {
                v.extend(["--threads".into(), threads.to_string()]);
            }
        }
        PeDevice::Gpu { devices, runtime: GpuRuntime::Vulkan } => {
            v.push("--usevulkan".into());
            v.extend(devices.iter().map(u32::to_string));
            v.extend(["--gpulayers".into(), "999".into()]);
        }
        // the Mac build's one GPU, through Metal without asking
        PeDevice::Gpu { runtime: GpuRuntime::Metal, .. } => v.extend(["--gpulayers", "999"].map(String::from)),
        // the GPUs picked by `device_env` (`--usecuda` takes one or all)
        PeDevice::Gpu { .. } => v.extend(["--usecuda", "--gpulayers", "999"].map(String::from)),
    }
    v
}

/// The environment KoboldCpp needs for `device`: CUDA and ROCm see only
/// the GPUs picked, CUDA numbering them as `nvidia-smi` does.
pub fn device_env(device: &PeDevice) -> Vec<(String, String)> {
    let PeDevice::Gpu { devices, runtime } = device else { return vec![] };
    let ids = devices.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let mut v = vec![];
    match runtime {
        GpuRuntime::Vulkan | GpuRuntime::Metal => {}
        GpuRuntime::Cuda => {
            v.push(("CUDA_DEVICE_ORDER".into(), "PCI_BUS_ID".into()));
            if !devices.is_empty() {
                v.push(("CUDA_VISIBLE_DEVICES".into(), ids));
            }
        }
        GpuRuntime::Rocm if !devices.is_empty() => v.push(("HIP_VISIBLE_DEVICES".into(), ids)),
        GpuRuntime::Rocm => {}
    }
    v
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

/// Talks to a chat server.
#[derive(Clone)]
pub struct Client {
    base: String,
    agent: Agent,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pe::Client({})", self.base)
    }
}

/// A chat completion's text, or what went wrong.
pub type ChatResult = Result<String, String>;

impl Client {
    pub fn new(base: &str) -> Client {
        // on CPU an answer with images can take minutes: given up only
        // when no word comes for the config's chat timeout
        let stall = crate::config::timeouts().chat();
        Client { base: base.trim_end_matches('/').to_string(), agent: agent(None, Some(stall)) }
    }

    /// Whether a chat server answers.
    pub fn ready(&self) -> bool {
        let quick = agent(Some(Duration::from_secs(3)), None);
        let url = format!("{}/v1/models", self.base);
        let t = std::time::Instant::now();
        let answer = quick
            .get(&url)
            .call()
            .map_err(|e| e.to_string())
            .and_then(|mut r| Ok((r.status().as_u16(), r.body_mut().read_to_string().map_err(|e| e.to_string())?)));
        crate::traffic::exchange("GET", &url, None, &answer, t.elapsed());
        answer.is_ok_and(|(status, _)| (200..300).contains(&status))
    }

    /// One chat completion: the system prompt, then the user's text with
    /// these images (PNG or JPEG bytes) after it.
    pub fn chat(
        &self,
        system: &str,
        text: &str,
        images: &[Vec<u8>],
        max_tokens: u32,
        sampling: Sampling,
    ) -> ChatResult {
        self.chat_stream(system, text, images, max_tokens, sampling, |_, _| true)
    }

    /// [`Client::chat`], streamed: `on` gets the pieces so far (about one
    /// per token) and the text so far, and stops the answer by returning
    /// false. A server that doesn't stream answers all at once.
    pub fn chat_stream(
        &self,
        system: &str,
        text: &str,
        images: &[Vec<u8>],
        max_tokens: u32,
        sampling: Sampling,
        on: impl FnMut(u32, &str) -> bool,
    ) -> ChatResult {
        // the images first: asks that share them then share the start of
        // the request, which KoboldCpp doesn't read again (an image costs
        // seconds even on a GPU)
        let mut parts = vec![];
        for img in images {
            let kind = if img.starts_with(&[0x89, b'P', b'N', b'G']) { "png" } else { "jpeg" };
            let data = base64::engine::general_purpose::STANDARD.encode(img);
            parts.push(json!({"type": "image_url", "image_url": {"url": format!("data:image/{kind};base64,{data}")}}));
        }
        parts.push(json!({"type": "text", "text": text}));
        let body = json!({
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": parts},
            ],
            "max_tokens": max_tokens,
            "temperature": sampling.temperature,
            "min_p": sampling.min_p,
            // KoboldCpp can otherwise answer the same ask word for word
            "seed": fresh_seed(),
            "stream": true,
        });
        let url = format!("{}/v1/chat/completions", self.base);
        let t = std::time::Instant::now();
        let r = self.send_chat(&url, &body, on);
        // the answer as one text (streamed or not); an error as no answer
        let logged = r.as_ref().map(|a| (200, a.clone())).map_err(Clone::clone);
        crate::traffic::exchange("POST", &url, Some(&body), &logged, t.elapsed());
        r
    }

    fn send_chat(&self, url: &str, body: &Value, mut on: impl FnMut(u32, &str) -> bool) -> ChatResult {
        // a KoboldCpp swapping with images: after the image it's making
        let _turn = crate::kobold_image::chat_turn(&self.base).map_err(|e| format!("{url}: {e}"))?;
        let mut resp = self.agent.post(url).send_json(body).map_err(|e| format!("{url}: {e}"))?;
        let status = resp.status().as_u16();
        let sse = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.starts_with("text/event-stream"));
        if !(200..300).contains(&status) || !sse {
            let text = resp.body_mut().read_to_string().map_err(|e| format!("{url}: {e}"))?;
            if !(200..300).contains(&status) {
                return Err(format!("{url}: HTTP {status}: {text:.300}"));
            }
            let v: Value = serde_json::from_str(&text).map_err(|e| format!("{url}: {e}: {text:.200}"))?;
            return v["choices"][0]["message"]["content"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{url}: no answer in {text:.200}"));
        }
        let (mut answer, mut pieces) = (String::new(), 0);
        let reader = std::io::BufReader::new(resp.body_mut().as_reader());
        for line in std::io::BufRead::lines(reader) {
            let line = line.map_err(|e| format!("{url}: {e}"))?;
            let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
            if data == "[DONE]" {
                break;
            }
            let v: Value = serde_json::from_str(data).map_err(|e| format!("{url}: {e}: {data:.200}"))?;
            if !v["error"].is_null() {
                return Err(format!("{url}: {:.300}", v["error"].to_string()));
            }
            if let Some(piece) = v["choices"][0]["delta"]["content"].as_str().filter(|p| !p.is_empty()) {
                answer.push_str(piece);
                pieces += 1;
                if !on(pieces, &answer) {
                    return Err("stopped".into());
                }
            }
        }
        Ok(answer)
    }

    /// Asks KoboldCpp to stop the answer it's writing (best effort).
    pub fn abort(&self) {
        let quick = agent(Some(Duration::from_secs(3)), None);
        let _ = quick.post(format!("{}/api/extra/abort", self.base)).send_json(json!({}));
    }
}

/// A PE server the app talks to; dropping it stops a KoboldCpp the app
/// started.
#[derive(Debug)]
pub struct PeProcess {
    pub client: Client,
    child: Option<Child>,
    pub log: Option<PathBuf>,
}

impl Drop for PeProcess {
    fn drop(&mut self) {
        if let Some(c) = self.child.take() {
            crate::process::stop(c);
        }
    }
}

/// Where a started KoboldCpp logs.
pub fn log_path(log_dir: &Path, server: &PeServer) -> PathBuf {
    log_dir.join(log_name(&server.name))
}

/// A started chat server's log file name, by the server's name.
pub fn log_name(server: &str) -> String {
    format!("pe-{server}.log")
}

/// Connects to the server, starting KoboldCpp first if it doesn't answer
/// (logging to [`log_path`]) and waiting for its model to load (given up
/// after `wait` without a new line in its log).
pub fn open(server: &PeServer, log_dir: &Path, wait: Duration, cancel: &AtomicBool) -> Result<PeProcess, String> {
    let client = Client::new(&server.base_url());
    let mut proc = PeProcess { client: client.clone(), child: None, log: None };
    if client.ready() {
        return Ok(proc);
    }
    let PeRun::KoboldCpp { binary, .. } = &server.run else {
        return Err(format!("{}: no chat server answers on {}", server.name, server.base_url()));
    };
    let log = log_path(log_dir, server);
    // (started from the PE thread, which lasts as long as the app)
    let (child, _) = crate::kobold::spawn(binary, &command_args(&server.run), &run_env(&server.run), &log, true)
        .map_err(|e| format!("{}: {e}", server.name))?;
    proc.child = Some(child);
    proc.log = Some(log.clone());
    let mut stall = crate::process::LogStall::new(&log, wait);
    loop {
        if client.ready() {
            return Ok(proc);
        }
        if let Ok(Some(status)) = proc.child.as_mut().unwrap().try_wait() {
            proc.child = None;
            return Err(format!("{}: KoboldCpp exited ({status}) while starting; log {}", server.name, log.display()));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(format!("{}: cancelled", server.name));
        }
        if stall.stalled() {
            return Err(format!(
                "{}: KoboldCpp didn't answer, its log quiet for {} s (the server start timeout); log {}",
                server.name,
                wait.as_secs(),
                log.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// An image for a request: at most `max` pixels on a side, as PNG. A
/// transparent one is shown over a checkerboard (models read the colour
/// of fully transparent pixels as if it were there).
pub fn image_for_request(path: &Path, max: u32) -> Result<Vec<u8>, String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let img = if img.width() > max || img.height() > max { img.thumbnail(max, max) } else { img };
    let img = if img.color().has_alpha() { on_checkerboard(&img.to_rgba8()).into() } else { img };
    let mut buf = Cursor::new(vec![]);
    img.write_to(&mut buf, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}

fn on_checkerboard(img: &image::RgbaImage) -> image::RgbImage {
    image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let bg = if (x / 16 + y / 16) % 2 == 0 { 255.0 } else { 204.0 };
        let p = img.get_pixel(x, y).0;
        let a = p[3] as f32 / 255.0;
        image::Rgb([0, 1, 2].map(|i| (p[i] as f32 * a + bg * (1.0 - a)).round() as u8))
    })
}

/// Asking for values for one blank: of the shared value (every style that
/// needs it), or of one style's own.
#[derive(Clone, Debug, PartialEq)]
pub struct BlankAsk {
    pub slot: String,
    /// The template (`{preset}` marks where the style goes).
    pub template: String,
    /// (name, prompt) of the styles the value is for.
    pub styles: Vec<(String, String)>,
    /// Blanks filled in already, by name.
    pub filled: BTreeMap<String, String>,
    /// How many reference images are attached (`<image1>`…).
    pub images: usize,
    /// The img2img start image is attached too, after them.
    pub start: bool,
    pub count: usize,
    /// Values offered for it before, not to offer again.
    pub avoid: Vec<String>,
    /// The user's instructions ("only stones", "stranger").
    pub how: String,
}

/// The user message for a [`BlankAsk`].
pub fn blank_message(a: &BlankAsk) -> String {
    // what every blank's ask has first (see [`Client::chat_stream`]), then
    // this blank's
    let mut s = String::new();
    match a.images {
        0 => {}
        1 => s.push_str("The reference image <image1> is attached.\n\n"),
        n => s.push_str(&format!("The reference images <image1> to <image{n}> are attached, in order.\n\n")),
    }
    if a.start {
        s.push_str("The start image (the picture being transformed) is attached last.\n\n");
    }
    s.push_str(&format!("Subject (template): {}\n\n", a.template.trim()));
    match a.styles.len() {
        0 => {}
        1 => s.push_str(&format!("Style \"{}\": {}\n\n", a.styles[0].0, a.styles[0].1)),
        n => {
            s.push_str(&format!("One value is used in all {n} of these styles:\n"));
            for (name, p) in &a.styles {
                s.push_str(&format!("- \"{name}\": {p}\n"));
            }
            s.push('\n');
        }
    }
    let filled: Vec<_> = a.filled.iter().filter(|(k, v)| **k != a.slot && !v.trim().is_empty()).collect();
    if !filled.is_empty() {
        s.push_str("Already filled in: ");
        s.push_str(&filled.iter().map(|(k, v)| format!("{{{k}}} = {v}")).collect::<Vec<_>>().join(", "));
        s.push_str("\n\n");
    }
    if !a.avoid.is_empty() {
        s.push_str("Already suggested (offer different ones, not these or close variations of them): ");
        s.push_str(&a.avoid.join("; "));
        s.push_str("\n\n");
    }
    if !a.how.trim().is_empty() {
        s.push_str(&format!("Instructions: {}\n\n", a.how.trim()));
    }
    s.push_str(&format!("Suggest {} values for {{{}}}.", a.count, a.slot));
    s
}

/// Asking for a rewrite of `original` (an image described, when empty).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextAsk {
    pub original: String,
    /// What it is: "prompt", "idea (a prompt draft, notes, or both)".
    pub what: String,
    /// The user's instructions ("more cinematic").
    pub how: String,
    /// How many reference images are attached (`<image1>`…).
    pub images: usize,
    /// The img2img start image is attached too, after them.
    pub start: bool,
}

/// The user message for a [`TextAsk`].
pub fn text_message(a: &TextAsk) -> String {
    let mut s = if a.original.trim().is_empty() {
        "Describe the attached image.\n\n".to_string()
    } else {
        format!("Rewrite this {}:\n\n{}\n\n", a.what, a.original.trim())
    };
    if !a.original.trim().is_empty() {
        match a.images {
            0 => {}
            1 => s.push_str("The reference image <image1> is attached.\n\n"),
            n => s.push_str(&format!("The reference images <image1> to <image{n}> are attached, in order.\n\n")),
        }
        if a.start {
            s.push_str("The start image (the picture being transformed) is attached last.\n\n");
        }
    }
    if !a.how.trim().is_empty() {
        s.push_str(&format!("Instructions: {}\n\n", a.how.trim()));
    }
    s.truncate(s.trim_end().len());
    s
}

/// Asking for a style's prompt improved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleAsk {
    /// The sweep's template (`{preset}` where the style goes).
    pub template: String,
    pub style: String,
    /// The style preset's own prompt.
    pub style_prompt: String,
    /// The prompt to improve: the template with the style and the values
    /// in, or as it is now.
    pub prompt: String,
    /// The user's instructions ("more cinematic").
    pub how: String,
    pub images: usize,
    pub start: bool,
}

/// The user message for a [`StyleAsk`]: the images first (as for blanks),
/// then the parts, the prompt last before the instructions.
pub fn style_message(a: &StyleAsk) -> String {
    let mut s = String::new();
    match a.images {
        0 => {}
        1 => s.push_str("The reference image <image1> is attached.\n\n"),
        n => s.push_str(&format!("The reference images <image1> to <image{n}> are attached, in order.\n\n")),
    }
    if a.start {
        s.push_str("The start image (the picture being transformed) is attached last.\n\n");
    }
    if a.template.trim() != crate::prompts::PRESET_SLOT && !a.template.trim().is_empty() {
        s.push_str(&format!("Template (the subject): {}\n\n", a.template.trim()));
    }
    s.push_str(&format!("Style \"{}\": {}\n\n", a.style, a.style_prompt.trim()));
    s.push_str(&format!("Improve this prompt:\n\n{}\n\n", a.prompt.trim()));
    let blanks = crate::prompts::slot_names([a.prompt.as_str()]);
    if !blanks.is_empty() {
        let list: Vec<String> = blanks.iter().map(|b| format!("{{{b}}}")).collect();
        s.push_str(&format!("Blanks to fill: {}\n\n", list.join(", ")));
    }
    if !a.how.trim().is_empty() {
        s.push_str(&format!("Instructions: {}\n\n", a.how.trim()));
    }
    s.truncate(s.trim_end().len());
    s
}

/// An answer as text to use: without code fences, a leading "Rewritten
/// prompt:" and quotes around it all.
pub fn clean_text(answer: &str) -> String {
    let mut s = answer.trim();
    if let Some(rest) = s.strip_prefix("```") {
        s = rest.split_once('\n').map_or("", |(_, r)| r);
        s = s.trim_end().strip_suffix("```").unwrap_or(s).trim();
    }
    if let Some((head, rest)) = s.split_once(':')
        && !head.contains('\n')
        && head.len() < 40
        && ["prompt", "rewrite", "rewritten", "description"].iter().any(|w| head.to_lowercase().contains(w))
    {
        s = rest.trim();
    }
    for (a, b) in [("\"", "\""), ("“", "”"), ("'", "'")] {
        if s.len() > 2 && s.starts_with(a) && s.ends_with(b) && !s[a.len()..s.len() - b.len()].contains(a) {
            s = s[a.len()..s.len() - b.len()].trim();
        }
    }
    s.to_string()
}

/// The `<imageN>` tags and `{blanks}` of `original` that `text` lost.
pub fn lost_tags(original: &str, text: &str) -> Vec<String> {
    let mut tags: Vec<String> = vec![];
    for (open, close) in [('<', '>'), ('{', '}')] {
        let mut rest = original;
        while let Some(a) = rest.find(open) {
            let Some(b) = rest[a..].find(close) else { break };
            let tag = &rest[a..a + b + 1];
            let inner = &tag[1..tag.len() - 1];
            let ok = if open == '<' {
                inner.strip_prefix("image").is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
            } else {
                !inner.is_empty() && inner.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            };
            if ok && !text.contains(tag) && !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_string());
            }
            rest = &rest[a + 1..];
        }
    }
    tags
}

/// A piece of a word diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Diff {
    Same,
    Added,
    Removed,
}

/// `a` to `b` word by word (whitespace kept with the word before it).
pub fn word_diff<'a>(a: &'a str, b: &'a str) -> Vec<(Diff, &'a str)> {
    fn words(s: &str) -> Vec<&str> {
        let mut v = vec![];
        let mut start = 0;
        let mut in_space = false;
        for (i, c) in s.char_indices() {
            if c.is_whitespace() {
                in_space = true;
            } else if in_space {
                v.push(&s[start..i]);
                (start, in_space) = (i, false);
            }
        }
        if start < s.len() {
            v.push(&s[start..]);
        }
        v
    }
    let (a, b) = (words(a), words(b));
    let key = |w: &str| w.trim_end().to_string();
    // longest common subsequence, from the end
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if key(a[i]) == key(b[j]) { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, vec![]);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && key(a[i]) == key(b[j]) {
            out.push((Diff::Same, b[j]));
            (i, j) = (i + 1, j + 1);
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push((Diff::Removed, a[i]));
            i += 1;
        } else {
            out.push((Diff::Added, b[j]));
            j += 1;
        }
    }
    out
}

/// A short fingerprint of a system prompt, for a variant's origin.
pub fn prompt_hash(system: &str) -> String {
    use sha2::Digest;
    let d = sha2::Sha256::digest(system.as_bytes());
    d.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// The suggestions in an answer: its JSON array of strings, else its
/// lines (bullets and numbering dropped); no repeats, no blanks.
/// A different random number each call (31 bits, for any server).
fn fresh_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish() >> 33
}

/// The longest side of an image sent for suggesting a blank's values: they
/// need its gist, not its detail (Qwen3-VL reads a 512 px image as ~180
/// tokens, a 768 px one as ~400).
pub const BLANK_IMAGE_SIZE: u32 = 512;

/// The longest side of an image sent for rewriting or describing.
pub const TEXT_IMAGE_SIZE: u32 = 768;

/// Whether two suggestions are the same, or one is the other with words
/// added ("obsidian", "Black obsidian"), ignoring case and punctuation.
pub fn same_suggestion(a: &str, b: &str) -> bool {
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
    };
    let (a, b) = (words(a), words(b));
    let (short, long) = if a.len() <= b.len() { (&a, &b) } else { (&b, &a) };
    !short.is_empty() && long.windows(short.len()).any(|w| w == short.as_slice())
}

pub fn parse_suggestions(answer: &str) -> Vec<String> {
    let from_json = answer
        .find('[')
        .zip(answer.rfind(']'))
        .filter(|(a, b)| a < b)
        .and_then(|(a, b)| serde_json::from_str::<Vec<Value>>(&answer[a..=b]).ok())
        .map(|v| v.into_iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>());
    let raw = from_json.unwrap_or_else(|| {
        answer
            .lines()
            // a list without quotes: [chrome, soft felt]
            .flat_map(|l| match l.trim().strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                Some(inner) => inner.split(',').collect::<Vec<_>>(),
                None => vec![l],
            })
            .map(|l| l.trim().trim_start_matches(['-', '*', '•']).trim())
            .map(|l| l.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')').trim())
            .map(str::to_string)
            .collect()
    });
    let mut out: Vec<String> = vec![];
    for s in raw {
        let s = s.trim().trim_matches(['"', '\'', '{', '}']).trim().to_string();
        if !s.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&s)) {
            out.push(s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn koboldcpp_runs_on_the_cpu_or_gpus_it_is_told() {
        let run = |device, context| PeRun::KoboldCpp {
            binary: "kcpp".into(),
            model: "m.gguf".into(),
            mmproj: String::new(),
            config: String::new(),
            host: "127.0.0.1".into(),
            port: 1240,
            device,
            context,
            args: vec!["--blasthreads".into(), "24".into()],
        };
        let tail = |r| command_args(&r)[8..].join(" ");
        assert_eq!(tail(run(PeDevice::Auto, 0)), "--jinja --blasthreads 24");
        assert_eq!(
            tail(run(PeDevice::Cpu { threads: 8 }, 8192)),
            "--jinja --usecpu --threads 8 --contextsize 8192 --blasthreads 24"
        );
        let gpu = |devices: &[u32], runtime| PeDevice::Gpu { devices: devices.to_vec(), runtime };
        assert_eq!(
            tail(run(gpu(&[1], GpuRuntime::Vulkan), 0)),
            "--jinja --usevulkan 1 --gpulayers 999 --blasthreads 24"
        );
        assert_eq!(tail(run(gpu(&[], GpuRuntime::Vulkan), 0)), "--jinja --usevulkan --gpulayers 999 --blasthreads 24");
        assert!(run_env(&run(gpu(&[1], GpuRuntime::Vulkan), 0)).is_empty());
        // CUDA and ROCm: the GPUs picked are the only ones they see
        for runtime in [GpuRuntime::Cuda, GpuRuntime::Rocm] {
            let r = run(gpu(&[0, 2], runtime), 0);
            assert_eq!(tail(r.clone()), "--jinja --usecuda --gpulayers 999 --blasthreads 24");
            let env = run_env(&r);
            let visible = if runtime == GpuRuntime::Cuda { "CUDA_VISIBLE_DEVICES" } else { "HIP_VISIBLE_DEVICES" };
            assert!(env.contains(&(visible.into(), "0,2".into())), "{env:?}");
        }
        // Metal: the Mac build's GPU, no flag picks it
        let metal = run(gpu(&[0], GpuRuntime::Metal), 0);
        assert_eq!(tail(metal.clone()), "--jinja --gpulayers 999 --blasthreads 24");
        assert!(run_env(&metal).is_empty());
        let cuda_all = run_env(&run(gpu(&[], GpuRuntime::Cuda), 0));
        assert_eq!(cuda_all, [("CUDA_DEVICE_ORDER".to_string(), "PCI_BUS_ID".to_string())], "numbered as nvidia-smi");
        assert!(GpuRuntime::Cuda.missing_from("/k/koboldcpp-linux-x64-nocuda"));
        assert!(!GpuRuntime::Vulkan.missing_from("/k/koboldcpp-linux-x64-nocuda"));
        assert!(!GpuRuntime::Rocm.missing_from("/k/koboldcpp_rocm"));
        // a settings file first, so what follows overrides it
        let PeRun::KoboldCpp { binary, host, port, device, context, args, .. } = run(PeDevice::Auto, 0) else {
            unreachable!()
        };
        let from_file = PeRun::KoboldCpp {
            binary,
            model: String::new(),
            mmproj: String::new(),
            config: "q.kcpps".into(),
            host,
            port,
            device,
            context,
            args,
        };
        assert_eq!(command_args(&from_file)[..4].join(" "), "--config q.kcpps --host 127.0.0.1");
        // only what makes the process counts as a change
        let a = PeServer { run: run(PeDevice::Auto, 0), ..Default::default() };
        let mut b = PeServer { name: "other".into(), start: PeStart::AtLaunch, ..a.clone() };
        assert!(a.same_process(&b));
        b.run = run(PeDevice::Cpu { threads: 0 }, 0);
        assert!(!a.same_process(&b));
    }

    #[test]
    fn gpus_from_vulkaninfo() {
        let text = "Devices:\n========\nGPU0:\n\tdeviceType         = PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU\n\tdeviceName         = Intel(R) Graphics (RPL-S)\nGPU1:\n\tdeviceType         = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU\n\tdeviceName         = AMD Radeon AI PRO R9700 (RADV GFX1201)\nGPU3:\n\tdeviceType         = PHYSICAL_DEVICE_TYPE_CPU\n\tdeviceName         = llvmpipe (LLVM 19.1.7, 256 bits)\n";
        assert_eq!(
            parse_vulkaninfo(text),
            [(0, "Intel(R) Graphics (RPL-S)".to_string()), (1, "AMD Radeon AI PRO R9700 (RADV GFX1201)".to_string())]
        );
    }

    #[test]
    fn a_suggestion_with_words_added_is_the_same() {
        assert!(same_suggestion("obsidian", "Black Obsidian"));
        assert!(same_suggestion("polished black obsidian shards", "black obsidian"));
        assert!(same_suggestion("ice", "ICE!"));
        assert!(!same_suggestion("ice", "iced tea") && !same_suggestion("black glass", "black obsidian"));
        assert!(!same_suggestion("", "ice"));
    }

    #[test]
    fn suggestions_come_from_json_or_lines() {
        assert_eq!(parse_suggestions("Sure: [\"clay\", \"Clay\", \" glass \"]"), ["clay", "glass"]);
        assert_eq!(parse_suggestions("1. marble\n- \"bronze\"\n\n* {ice}"), ["marble", "bronze", "ice"]);
        assert_eq!(
            parse_suggestions("[glittering chrome, fuzzy felt , ice]"),
            ["glittering chrome", "fuzzy felt", "ice"]
        );
        assert_ne!(fresh_seed(), fresh_seed());
    }

    #[test]
    fn the_default_system_prompt_unless_one_is_set() {
        let mut s = PeConfig::default();
        assert_eq!(s.system_prompt(FILL_BLANKS), DEFAULT_FILL_BLANKS);
        s.system_prompts.insert(FILL_BLANKS.into(), " ".into());
        assert_eq!(s.system_prompt(FILL_BLANKS), DEFAULT_FILL_BLANKS, "empty: the default");
        s.system_prompts.insert(FILL_BLANKS.into(), "mine".into());
        assert_eq!(s.system_prompt(FILL_BLANKS), "mine");
        assert_eq!(s.sampling(FILL_BLANKS), Sampling { temperature: 1.1, min_p: 0.05 });
        assert_eq!(s.sampling(REWRITE), Sampling { temperature: 0.9, min_p: 0.0 });
        s.sampling.insert(REWRITE.into(), Sampling { temperature: 0.5, min_p: 0.1 });
        assert_eq!(s.sampling(REWRITE).temperature, 0.5);
    }

    #[test]
    fn a_shared_blank_names_every_style_and_what_is_filled() {
        let a = BlankAsk {
            slot: "material".into(),
            template: "a fox, {preset}".into(),
            styles: vec![("Sculpted".into(), "sculpted from {material}".into()), ("Melting".into(), "x".into())],
            filled: [("era".to_string(), "1920s".to_string()), ("material".into(), "old".into())].into(),
            images: 2,
            start: false,
            count: 5,
            avoid: vec![],
            how: String::new(),
        };
        let m = blank_message(&a);
        assert!(m.contains("all 2 of these styles") && m.contains("\"Sculpted\": sculpted from {material}"), "{m}");
        assert!(m.contains("{era} = 1920s") && !m.contains("= old"), "{m}");
        assert!(m.contains("<image1> to <image2>") && m.ends_with("Suggest 5 values for {material}."), "{m}");
        assert!(!m.contains("Already suggested"), "{m}");
        let m =
            blank_message(&BlankAsk { avoid: vec!["bronze".into(), "wet clay".into()], how: " stones ".into(), ..a });
        assert!(m.contains("not these or close variations of them): bronze; wet clay\n\n"), "{m}");
        assert!(m.ends_with("\n\nInstructions: stones\n\nSuggest 5 values for {material}."), "{m}");
    }

    #[test]
    fn rewrite_and_describe_messages() {
        let a = TextAsk {
            original: " a fox like <image1> ".into(),
            what: "prompt".into(),
            how: "moodier".into(),
            images: 1,
            start: true,
        };
        let m = text_message(&a);
        assert!(m.starts_with("Rewrite this prompt:\n\na fox like <image1>\n\nThe reference image <image1>"), "{m}");
        assert!(m.contains("attached last") && m.ends_with("Instructions: moodier"), "{m}");
        let d = text_message(&TextAsk { images: 1, ..Default::default() });
        assert_eq!(d, "Describe the attached image.");
    }

    #[test]
    fn answers_are_cleaned_and_lost_tags_found() {
        assert_eq!(clean_text("Rewritten prompt: \"a red fox\""), "a red fox");
        assert_eq!(clean_text("```text\na fox\n```"), "a fox");
        assert_eq!(clean_text("a fox: red"), "a fox: red");
        assert_eq!(clean_text("\"a\" and \"b\""), "\"a\" and \"b\"");
        let lost = lost_tags("<image1> and <image2> {material} {preset} <b>", "<image1> in {preset}");
        assert_eq!(lost, ["<image2>", "{material}"]);
    }

    #[test]
    fn transparent_images_go_over_a_checkerboard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        let mut img = image::RgbaImage::from_pixel(40, 40, image::Rgba([0, 200, 255, 0]));
        img.put_pixel(20, 20, image::Rgba([0, 200, 255, 255]));
        img.save(&path).unwrap();
        let sent = image::load_from_memory(&image_for_request(&path, 768).unwrap()).unwrap().to_rgb8();
        assert_eq!(sent.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(sent.get_pixel(16, 0).0, [204, 204, 204]);
        assert_eq!(sent.get_pixel(20, 20).0, [0, 200, 255]);
    }

    #[test]
    fn words_diff() {
        let d = word_diff("a red fox runs", "a small red fox  sits");
        let show: Vec<_> = d.iter().map(|(k, w)| format!("{k:?}:{}", w.trim())).collect();
        assert_eq!(show, ["Same:a", "Added:small", "Same:red", "Same:fox", "Removed:runs", "Added:sits"]);
    }
}
