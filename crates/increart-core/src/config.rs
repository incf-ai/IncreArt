//! The user's `config.rsn` (DESIGN.md, Config file).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::rsnfmt;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Config", default)]
pub struct Config {
    /// Libraries opened before, most recent first (offered at startup).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent_libraries: Vec<String>,
    /// The example library offered beside the app (its path, the last one
    /// offered): kept in `recent_libraries` until removed from it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub example_library: String,
    /// The example library was removed from the list: not offered again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub example_library_removed: bool,
    /// Older configs' single last library: read, folded into
    /// `recent_libraries` by [`normalize`](Self::normalize), never written.
    #[serde(skip_serializing)]
    pub library: Option<String>,
    pub resolutions: Vec<Resolution>,
    /// The command that opens a folder in the file manager (the folder is
    /// appended). Empty: try xdg-open, gio, thunar, nautilus, dolphin, nemo,
    /// pcmanfm.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub file_manager: String,
    /// The command that opens a text file, e.g. a server's log (the file is
    /// appended). Empty: the desktop's default app for it (xdg-open, gio).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub editor: String,
    /// Model presets for the Generate panel (recipe + model files + sampling
    /// defaults).
    /// Written as `model_presets` (older files: `recipes`).
    #[serde(
        rename = "model_presets",
        alias = "recipes",
        deserialize_with = "rsnfmt::any_names",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub recipes: Vec<RecipePreset>,
    /// Named server setups, chosen in the startup dialog.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub server_presets: Vec<ServerPreset>,
    /// The preset chosen last time (preselected in the startup dialog).
    #[serde(serialize_with = "crate::rsnfmt::bare", skip_serializing_if = "Option::is_none")]
    pub last_preset: Option<String>,
    /// The programs the app starts: sd-server's settings and KoboldCpp.
    #[serde(skip_serializing_if = "is_default")]
    pub programs: Programs,
    /// Older configs' `sdserver`: read, moved into `programs` by
    /// [`normalize`](Self::normalize), never written.
    #[serde(rename = "sdserver", skip_serializing)]
    pub old_sdserver: Option<rsnfmt::AnyName<SdServerConfig>>,
    /// Older configs' `koboldcpp`: as `old_sdserver`.
    #[serde(rename = "koboldcpp", skip_serializing)]
    pub old_koboldcpp: Option<String>,
    /// How long the servers may go quiet (see [`Timeouts`]).
    #[serde(skip_serializing_if = "is_default")]
    pub timeouts: Timeouts,
    /// How the job queue shares the servers (Jobs: spread all / sweep by
    /// sweep / one at a time).
    #[serde(skip_serializing_if = "is_default")]
    pub job_order: crate::scheduler::Order,
    /// Loads on this machine don't wait for free RAM (a KoboldCpp that
    /// offloads its weights otherwise waits until they fit, 8 GiB to spare).
    #[serde(skip_serializing_if = "is_default")]
    pub skip_ram_check: bool,
    /// Every request to the image servers and their answers go to
    /// `server-traffic.log` (see [`crate::traffic`]).
    #[serde(skip_serializing_if = "is_default")]
    pub log_traffic: bool,
    /// Sounds when a GPU is idle or the review queue is long.
    #[serde(skip_serializing_if = "is_default")]
    pub notifications: crate::notify::Notifications,
    /// The contact sheet's cell size in points (0: the default).
    #[serde(skip_serializing_if = "is_default")]
    pub sheet_cell: u32,
    /// The contact sheet's row headers: beside the row, or a caption above.
    #[serde(skip_serializing_if = "is_default")]
    pub sheet_rows: SheetRows,
    /// Review's Grid tile size in points (0: the default).
    #[serde(skip_serializing_if = "is_default")]
    pub review_grid_cell: u32,
    /// The Output library Grid's tile size in points (0: the default).
    #[serde(skip_serializing_if = "is_default")]
    pub output_grid_cell: u32,
    /// The Input library Grid's card width in points (0: the default).
    #[serde(skip_serializing_if = "is_default")]
    pub library_grid_cell: u32,
    /// The Idea inbox Grid's card width in points (0: the default).
    #[serde(skip_serializing_if = "is_default")]
    pub inbox_grid_cell: u32,
    /// The interface zoom in percent (0: 100%).
    #[serde(skip_serializing_if = "is_default")]
    pub ui_zoom: u32,
    /// The Tile dialog's settings, as last used.
    #[serde(skip_serializing_if = "is_default")]
    pub tile: TileSettings,
    /// The project tree doesn't list the selected project's runs.
    #[serde(skip_serializing_if = "is_default")]
    pub hide_project_runs: bool,
    /// How the project tree and grid order projects.
    #[serde(skip_serializing_if = "is_default")]
    pub project_order: TreeOrder,
    /// How the project tree orders folders (by the projects anywhere in
    /// them; empty folders last).
    #[serde(skip_serializing_if = "is_default")]
    pub folder_order: FolderOrder,
    /// Older configs' project order (A–Z): read, folded into
    /// `project_order` by [`normalize`](Self::normalize), never written.
    #[serde(skip_serializing)]
    pub projects_by_name: bool,
    /// Older configs' folder order (newest first): read, folded into
    /// `folder_order` by [`normalize`](Self::normalize), never written.
    #[serde(skip_serializing)]
    pub folders_by_newest: bool,
    /// How Review's scope tree orders folders and projects.
    #[serde(skip_serializing_if = "is_default")]
    pub review_order: ReviewOrder,
    /// Older configs' Review order (oldest to review): read, folded into
    /// `review_order` by [`normalize`](Self::normalize), never written.
    #[serde(skip_serializing)]
    pub review_oldest_first: bool,
    /// Review's scope tree also lists folders, projects, runs and sweeps
    /// with nothing to review (**hide reviewed** unticked).
    #[serde(skip_serializing_if = "is_default")]
    pub review_show_reviewed: bool,
    /// How Review's **Follow** paces the images.
    #[serde(skip_serializing_if = "is_default")]
    pub review_follow: FollowPace,
    /// Prompt enhance: the chat servers that suggest prompt text.
    #[serde(skip_serializing_if = "is_default")]
    pub pe: crate::pe::PeConfig,
    /// How the sweep's **Choose styles** window orders the presets.
    #[serde(skip_serializing_if = "is_default")]
    pub style_sort: StyleSort,
    /// Selecting a project never narrows the project grid (**narrow to
    /// siblings** unticked).
    #[serde(skip_serializing_if = "is_default")]
    pub project_click_keeps_scope: bool,
    /// Collapsed folders of the project tree (relative to `projects/`), per
    /// library path.
    #[serde(skip_serializing_if = "is_default")]
    pub collapsed_folders: std::collections::BTreeMap<String, Vec<PathBuf>>,
}

/// A model preset: which sd-server recipe and model files to load, and the
/// Generate panel's defaults for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "ModelPreset")]
pub struct RecipePreset {
    pub name: String,
    pub recipe: String,
    /// Slot → model file ID (as sd-server's `model-files` lists them).
    pub files: std::collections::BTreeMap<String, String>,
    pub steps: u32,
    pub cfg: f32,
    #[serde(default, serialize_with = "crate::rsnfmt::bare", skip_serializing_if = "Option::is_none")]
    pub flow_shift: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_sigmas: Vec<f32>,
    /// LoRAs applied to each image (no reload: both servers take them per
    /// request).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loras: Vec<PresetLora>,
}

/// A LoRA of a model preset: its file inside a model directory's `loras/`
/// (as [`lora_file_id`] writes it), and how strongly it applies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Lora")]
pub struct PresetLora {
    pub file: String,
    #[serde(default = "one")]
    pub weight: f32,
}

fn one() -> f32 {
    1.0
}

/// The folder names sd-server finds LoRAs in, inside a model directory
/// (matched ignoring case).
pub const LORA_DIRS: [&str; 2] = ["loras", "lora"];

/// The LoRA folder of a model directory as it is on disk, if it has one.
/// The first of `name(1)`, `name(2)`… that isn't `taken`.
pub fn first_free(name: impl Fn(u64) -> String, taken: impl Fn(&str) -> bool) -> String {
    let mut n = 1;
    loop {
        let candidate = name(n);
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

pub fn lora_dir(model_dir: &str) -> Option<PathBuf> {
    lora_dirs(&[model_dir.to_string()]).into_iter().next()
}

/// A LoRA as a model preset names it: its path inside the model directory
/// it's in, which must be in that folder's `loras/` (`loras/style.safetensors`).
/// Both servers look for LoRAs in every model directory's `loras/`, and are
/// sent the path inside it ([`lora_request_path`]).
pub fn lora_file_id(file: &Path, model_dirs: &[String]) -> std::result::Result<String, String> {
    if !model_dirs.iter().any(|d| !d.trim().is_empty()) {
        return Err("No model directory is set: set one in Server settings first.".into());
    }
    let outside = || {
        format!(
            "{} isn't in the loras folder of a model directory ({}): sd-server and KoboldCpp only load LoRAs from there.",
            file.display(),
            model_dirs.iter().filter(|d| !d.trim().is_empty()).cloned().collect::<Vec<_>>().join(", ")
        )
    };
    let id = model_file_id(file, model_dirs).map_err(|_| outside())?;
    match id.split_once('/') {
        Some((dir, rest)) if LORA_DIRS.contains(&dir.to_lowercase().as_str()) && !rest.is_empty() => Ok(id),
        _ => Err(outside()),
    }
}

/// Every LoRA folder of the model directories, in order (`loras` before
/// `Lora` in each, as sd-server orders them).
pub fn lora_dirs(model_dirs: &[String]) -> Vec<PathBuf> {
    model_dirs
        .iter()
        .flat_map(|d| {
            let mut found: Vec<PathBuf> = std::fs::read_dir(d)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| {
                    e.path().is_dir() && LORA_DIRS.contains(&e.file_name().to_string_lossy().to_lowercase().as_str())
                })
                .map(|e| e.path())
                .collect();
            found.sort_by_key(|p| (p.file_name().is_none_or(|n| n != "loras"), p.clone()));
            found
        })
        .collect()
}

/// The path a server is sent for a LoRA ID: inside the LoRA folder
/// (`loras/style.safetensors` → `style.safetensors`).
pub fn lora_request_path(id: &str) -> &str {
    match id.split_once('/') {
        Some((dir, rest)) if LORA_DIRS.contains(&dir.to_lowercase().as_str()) => rest,
        _ => id,
    }
}

/// The slots a model preset's files go in (sd-server's names; KoboldCpp
/// takes these too).
pub const MODEL_SLOTS: [&str; 7] = ["diffusion_model", "model", "llm", "vae", "clip_l", "clip_g", "t5xxl"];

pub fn builtin_recipes() -> Vec<RecipePreset> {
    let files = [
        ("diffusion_model", "diffusion_models/qwen_image_2.1_int8_convrot.safetensors"),
        ("llm", "text_encoders/qwen3vl_8b_int8_convrot.safetensors"),
        ("vae", "vae/qwen_image_2.1_vae_bf16.safetensors"),
    ]
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .into();
    vec![RecipePreset {
        name: "Qwen-Image 2.1".into(),
        recipe: "qwen-image-21".into(),
        files,
        steps: 20,
        cfg: 2.0,
        flow_shift: None,
        custom_sigmas: vec![],
        loras: vec![],
    }]
}

/// A named set of servers (e.g. "Both GPUs", "Remote box"): sd-servers,
/// and at most one chat server for prompt enhance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "ServerPreset")]
pub struct ServerPreset {
    pub name: String,
    /// Written as [`crate::serverwire`] says.
    #[serde(with = "crate::serverwire")]
    pub servers: Vec<ServerConfig>,
}

/// One server of a preset: an sd-server started by the app or a running one
/// to connect to, or the chat server prompt enhance uses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerConfig {
    Start {
        name: String,
        #[serde(default = "localhost")]
        host: String,
        port: u16,
        /// `--backend` device, e.g. `ROCm0`.
        device: String,
        /// Extra arguments for this server only (e.g. `--offload-to-cpu`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    Connect {
        name: String,
        /// e.g. `http://10.0.0.2:1234`.
        url: String,
        /// The device it runs on, for recipes (optional).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        device: String,
    },
    /// The chat server for prompt enhance (not an sd-server).
    Chat(crate::pe::PeServer),
    /// KoboldCpp generating images (in place of an sd-server).
    Kobold(crate::kobold::KoboldServer),
}

impl ServerConfig {
    pub fn name(&self) -> &str {
        match self {
            ServerConfig::Start { name, .. } | ServerConfig::Connect { name, .. } => name,
            ServerConfig::Chat(s) => &s.name,
            ServerConfig::Kobold(k) => &k.name,
        }
    }

    pub fn device(&self) -> String {
        match self {
            ServerConfig::Start { device, .. } | ServerConfig::Connect { device, .. } => device.clone(),
            ServerConfig::Chat(_) => String::new(),
            ServerConfig::Kobold(k) => k.device(),
        }
    }

    pub fn base_url(&self) -> String {
        match self {
            ServerConfig::Start { host, port, .. } => format!("http://{host}:{port}"),
            ServerConfig::Connect { url, .. } => url.trim_end_matches('/').to_string(),
            ServerConfig::Chat(s) => s.base_url(),
            ServerConfig::Kobold(k) => k.base_url(),
        }
    }

    /// Whether the app starts it (vs connecting to one running).
    pub fn is_started(&self) -> bool {
        matches!(
            self,
            ServerConfig::Start { .. }
                | ServerConfig::Kobold(crate::kobold::KoboldServer { run: crate::kobold::KoboldRun::Start { .. }, .. })
        )
    }

    pub fn is_chat(&self) -> bool {
        matches!(self, ServerConfig::Chat(_))
    }

    /// The port it's started on (None: connected to).
    pub fn port(&self) -> Option<u16> {
        match self {
            ServerConfig::Start { port, .. } => Some(*port),
            ServerConfig::Chat(crate::pe::PeServer { run: crate::pe::PeRun::KoboldCpp { port, .. }, .. }) => {
                Some(*port)
            }
            ServerConfig::Kobold(crate::kobold::KoboldServer {
                run: crate::kobold::KoboldRun::Start { port, .. },
                ..
            }) => Some(*port),
            _ => None,
        }
    }
}

impl ServerPreset {
    /// Its sd-servers, in order.
    pub fn sd_servers(&self) -> Vec<ServerConfig> {
        self.servers.iter().filter(|s| !s.is_chat()).cloned().collect()
    }

    /// Its chat server entry.
    pub fn chat(&self) -> Option<&crate::pe::PeServer> {
        self.servers.iter().find_map(|s| match s {
            ServerConfig::Chat(c) => Some(c),
            _ => None,
        })
    }

    /// The chat server prompt enhance uses with it: the first of
    /// [`chat_pool`](Self::chat_pool).
    pub fn chat_server(&self) -> Option<crate::pe::PeServer> {
        self.chat_pool().into_iter().next()
    }

    /// The chat servers prompt enhance shares its asks among: its chat
    /// entries, then the KoboldCpp image servers that are chat servers
    /// too; ones swapping with images left out when one that doesn't is
    /// there (each ask would cost an image a reload).
    pub fn chat_pool(&self) -> Vec<crate::pe::PeServer> {
        let mut list: Vec<(crate::pe::PeServer, bool)> = vec![];
        for s in &self.servers {
            if let ServerConfig::Chat(c) = s {
                list.push((c.clone(), false));
            }
        }
        for s in &self.servers {
            if let ServerConfig::Kobold(k) = s
                && let Some(c) = k.chat_server()
            {
                list.push((c, k.swap));
            }
        }
        if list.iter().any(|(_, swap)| !swap) {
            list.retain(|(_, swap)| !swap);
        }
        let mut pool: Vec<crate::pe::PeServer> = vec![];
        for (c, _) in list {
            if !pool.iter().any(|p| p.same_process(&c)) {
                pool.push(c);
            }
        }
        pool
    }
}

/// What one server of a preset is used for (DESIGN.md, Server presets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roles {
    pub name: String,
    /// Generates images: each run goes to an image server that has its
    /// model preset's files.
    pub images: bool,
    /// Prompt enhance's asks go to it (shared among all such).
    pub text: bool,
    /// With [`Self::text`]: whether its model sees the reference images
    /// sent with the asks (`None`: can't tell, e.g. one connected to;
    /// they're sent anyway).
    pub vision: Option<bool>,
    /// Why a server that can chat isn't asked.
    pub note: Option<String>,
}

impl ServerPreset {
    /// What each of its servers is used for, in order.
    pub fn roles(&self) -> Vec<Roles> {
        let pool = self.chat_pool();
        self.servers
            .iter()
            .map(|s| {
                let chat = match s {
                    ServerConfig::Chat(c) => Some(c.clone()),
                    ServerConfig::Kobold(k) => k.chat_server(),
                    _ => None,
                };
                let vision = chat.as_ref().and_then(|c| c.vision_known());
                let text = chat.as_ref().is_some_and(|c| pool.iter().any(|p| p.same_process(c)));
                let note = (chat.is_some() && !text).then(|| {
                    "not asked for prompt enhance: it swaps its chat model with images, and another chat server is there"
                        .to_string()
                });
                Roles { name: s.name().to_string(), images: !s.is_chat(), text, vision: vision.filter(|_| text), note }
            })
            .collect()
    }

    /// Its roles in a line: "images: kcpp, proxy · text: PE · vision: PE"
    /// ("images: gpu0 · no prompt enhance" without a chat server).
    pub fn roles_line(&self) -> String {
        let roles = self.roles();
        let names =
            |f: &dyn Fn(&Roles) -> bool| roles.iter().filter(|r| f(r)).map(|r| r.name.as_str()).collect::<Vec<_>>();
        let list = |v: Vec<&str>, none: &str| if v.is_empty() { none.to_string() } else { v.join(", ") };
        let images = list(names(&|r| r.images), "none");
        if !roles.iter().any(|r| r.text) {
            return format!("images: {images} · no prompt enhance");
        }
        let text = list(names(&|r| r.text), "none");
        let vision = list(names(&|r| r.vision != Some(false) && r.text), "none (asks go without images)");
        format!("images: {images} · text: {text} · vision: {vision}")
    }
}

/// The app's folder for one kind of per-user file: `increart` in `$<xdg>`
/// when set, else in `%<windows>%` on Windows, `~/Library/Application
/// Support` on macOS and `~/<unix>` elsewhere. Config: (`XDG_CONFIG_HOME`,
/// `.config`, `APPDATA`); state and logs: (`XDG_STATE_HOME`,
/// `.local/state`, `LOCALAPPDATA`).
pub fn user_dir(xdg: &str, unix: &str, windows: &str) -> Option<PathBuf> {
    let var = |v: &str| std::env::var_os(v).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = || var("HOME").or_else(|| var("USERPROFILE"));
    let base = var(xdg).or_else(|| {
        if cfg!(windows) {
            var(windows)
        } else if cfg!(target_os = "macos") {
            home().map(|h| h.join("Library/Application Support"))
        } else {
            home().map(|h| h.join(unix))
        }
    })?;
    Some(base.join("increart"))
}

/// `p` as a full path, so it means the same to every server whatever
/// folder starts it: a leading `~` is the home folder, a relative path is
/// taken from the folder the app was started in. Empty stays empty.
pub fn full_path(p: &str) -> String {
    let t = p.trim();
    if t.is_empty() {
        return String::new();
    }
    let home = || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let path = match (t.strip_prefix('~'), home()) {
        (Some(""), Some(h)) => h,
        (Some(rest), Some(h)) if rest.starts_with(['/', '\\']) => h.join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(t),
    };
    std::path::absolute(&path).map_or_else(|_| t.to_string(), |a| a.to_string_lossy().into_owned())
}

/// A binary as a [full path](full_path) when a path names it
/// (`./sd-server`, `~/bin/koboldcpp`); a bare name (looked up on `PATH`)
/// and a command line (`python3 koboldcpp.py`) stay as they are.
pub fn full_binary(b: &str) -> String {
    let t = b.trim();
    let full = full_path(t);
    let named = t.starts_with('~') || t.contains(['/', '\\']);
    if named && (!t.contains(char::is_whitespace) || Path::new(&full).exists()) { full } else { b.to_string() }
}

/// A model file as a model preset names it: its path inside the model
/// folder it's in (`/` between folders, as sd-server lists it). Err when
/// it's in none: sd-server loads only files inside its `--model-dir`s.
pub fn model_file_id(file: &Path, model_dirs: &[String]) -> std::result::Result<String, String> {
    let file = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    for d in model_dirs.iter().filter(|d| !d.trim().is_empty()) {
        let dir = std::path::absolute(full_path(d)).unwrap_or_else(|_| PathBuf::from(d));
        if let Ok(rel) = file.strip_prefix(&dir)
            && !rel.as_os_str().is_empty()
        {
            let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            return Ok(parts.join("/"));
        }
    }
    Err(match model_dirs.iter().find(|d| !d.trim().is_empty()) {
        Some(_) => format!(
            "{} isn't in a model directory ({}): sd-server only loads files inside them. \
             Move it into one, or add its folder in Server settings.",
            file.display(),
            model_dirs.join(", ")
        ),
        None => "No model directory is set: set one in Server settings first.".into(),
    })
}

/// Where a model preset's file is on disk: its path in the first model
/// folder that has it (a full path as it is).
pub fn model_file_on_disk(id: &str, model_dirs: &[String]) -> Option<PathBuf> {
    // a file: an empty ID would be the folder itself
    if Path::new(id).is_absolute() {
        return Some(PathBuf::from(id)).filter(|p| p.is_file());
    }
    model_dirs.iter().map(|d| Path::new(d).join(id)).find(|p| p.is_file())
}

/// Sets `s` to `f(s)`, noting the change in `said`.
fn make_full(s: &mut String, f: fn(&str) -> String, said: &mut Vec<String>) {
    let n = f(s);
    if n != *s {
        said.push(format!("{s} → {n}"));
        *s = n;
    }
}

impl ServerConfig {
    /// Its binaries, models and settings files as [full paths](full_path);
    /// each change noted in `said`.
    pub fn full_paths(&mut self, said: &mut Vec<String>) {
        match self {
            ServerConfig::Start { .. } | ServerConfig::Connect { .. } => {}
            ServerConfig::Chat(s) => s.run.full_paths(said),
            ServerConfig::Kobold(k) => {
                if let crate::kobold::KoboldRun::Start { binary, config, .. } = &mut k.run {
                    make_full(binary, full_binary, said);
                    make_full(config, full_path, said);
                }
                if let Some(c) = &mut k.chat {
                    make_full(&mut c.model, full_path, said);
                    make_full(&mut c.mmproj, full_path, said);
                }
            }
        }
    }
}

impl crate::pe::PeRun {
    /// As [`ServerConfig::full_paths`].
    pub fn full_paths(&mut self, said: &mut Vec<String>) {
        if let crate::pe::PeRun::KoboldCpp { binary, model, mmproj, config, .. } = self {
            make_full(binary, full_binary, said);
            for p in [model, mmproj, config] {
                make_full(p, full_path, said);
            }
        }
    }
}

impl Config {
    pub fn preset(&self, name: &str) -> Option<&ServerPreset> {
        self.server_presets.iter().find(|p| p.name == name)
    }

    /// The shared KoboldCpp binary set: the entries that started with the
    /// one before (a preset saved before KoboldCpp was found) follow it.
    pub fn set_koboldcpp(&mut self, binary: String) {
        let old = self.programs.koboldcpp.trim().to_string();
        if !old.is_empty() {
            for b in self.koboldcpp_binaries() {
                if b.trim() == old {
                    *b = binary.clone();
                }
            }
        }
        self.programs.koboldcpp = binary;
        self.fill_koboldcpp();
    }

    /// The presets' chat servers, one per process (the first preset's name
    /// for it).
    /// The KoboldCpp binary in use: the shared one, else a prompt-enhance
    /// server's, else a preset's KoboldCpp entry's.
    pub fn known_koboldcpp(&self) -> Option<String> {
        use crate::kobold::KoboldRun;
        use crate::pe::PeRun;
        let set = |b: &str| (!b.trim().is_empty()).then(|| b.trim().to_string());
        set(&self.programs.koboldcpp)
            .or_else(|| {
                self.chat_servers().into_iter().find_map(|c| match &c.run {
                    PeRun::KoboldCpp { binary, .. } => set(binary),
                    PeRun::Connect { .. } => None,
                })
            })
            .or_else(|| {
                self.server_presets.iter().flat_map(|p| &p.servers).find_map(|s| match s {
                    ServerConfig::Kobold(k) => match &k.run {
                        KoboldRun::Start { binary, .. } => set(binary),
                        KoboldRun::Connect { .. } => None,
                    },
                    _ => None,
                })
            })
    }

    pub fn chat_servers(&self) -> Vec<&crate::pe::PeServer> {
        let mut list: Vec<&crate::pe::PeServer> = vec![];
        for c in self.server_presets.iter().filter_map(ServerPreset::chat) {
            if !list.iter().any(|x| x.same_process(c)) {
                list.push(c);
            }
        }
        list
    }

    /// PE servers from before they were preset entries: the one used goes
    /// into every preset (its system prompts and sampling are now
    /// everyone's), each other one into a preset of its own.
    fn move_pe_servers(&mut self) -> Option<String> {
        use crate::pe::PeServer;
        let old = std::mem::take(&mut self.pe.servers);
        let chosen = std::mem::take(&mut self.pe.server);
        let used = old.iter().position(|s| s.name == chosen).unwrap_or(0);
        let first = old.get(used)?;
        if self.pe.system_prompts.is_empty() && self.pe.sampling.is_empty() {
            self.pe.system_prompts = first.system_prompts.clone();
            self.pe.sampling = first.sampling.clone();
        }
        let server = |o: &crate::pe::OldPeServer| PeServer {
            name: o.name.clone(),
            run: o.run.clone(),
            start: o.start,
            ..Default::default()
        };
        let had_presets = !self.server_presets.is_empty();
        for p in self.server_presets.iter_mut().filter(|p| p.chat().is_none()) {
            p.servers.push(ServerConfig::Chat(server(first)));
        }
        let mut alone = vec![];
        for (_, o) in old.iter().enumerate().filter(|(i, _)| *i != used || !had_presets) {
            let name = first_free(
                |n| if n == 1 { o.name.clone() } else { format!("{} {n}", o.name) },
                |n| self.preset(n).is_some(),
            );
            alone.push(name.clone());
            self.server_presets.push(ServerPreset { name, servers: vec![ServerConfig::Chat(server(o))] });
        }
        let mut said = vec![];
        if had_presets {
            said.push(format!("{} is now in every server preset", first.name));
        }
        if !alone.is_empty() {
            said.push(format!("new presets with only a chat server: {}", alone.join(", ")));
        }
        let text = format!("Prompt enhance servers moved into the server presets: {}", said.join("; "));
        Some(text)
    }
}

/// The programs the app starts, shared by every preset's entries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Programs", default)]
pub struct Programs {
    #[serde(skip_serializing_if = "is_default")]
    pub sd_server: SdServerConfig,
    /// The KoboldCpp binary new KoboldCpp entries and the setup wizard
    /// start (empty: the one in use, see `known_koboldcpp`; found at
    /// startup if none is), and the one an entry naming none starts.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub koboldcpp: String,
}

/// Settings shared by every sd-server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "SdServerSettings", default)]
pub struct SdServerConfig {
    pub binary: String,
    /// `--model-dir` roots, in order.
    pub model_dirs: Vec<String>,
    /// Arguments for every server.
    pub args: Vec<String>,
    /// Preview mode requested with each job: `none`, `proj`, `tae` or `vae`.
    pub preview: String,
    /// The sd.cpp commit the binary was built from, recorded in recipes.
    pub commit: String,
    /// Keep servers the app started running when it exits (the model stays
    /// loaded for the next session, which finds them on their ports).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub leave_running: bool,
}

impl Default for SdServerConfig {
    fn default() -> Self {
        SdServerConfig {
            binary: "sd-server".into(),
            model_dirs: vec![],
            args: ["--fa", "--conditioning-cache-size", "32"].map(String::from).to_vec(),
            preview: "none".into(),
            commit: String::new(),
            leave_running: false,
        }
    }
}

pub const MAX_RECENT_LIBRARIES: usize = 10;

/// How Review's Tile… lays images out (DESIGN.md, Tiling).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "TileSettings", default)]
pub struct TileSettings {
    /// Space between tiles and around them, in output pixels.
    pub gap: u32,
    /// RGBA; alpha 0 is transparent.
    pub background: [u8; 4],
    /// Crop to fill the cell (else fit inside it).
    pub fill: bool,
    /// Caption text height.
    pub caption_text: TextSize,
    pub caption_color: [u8; 4],
    /// The last caption format applied to every tile ("{style} · seed {seed}").
    pub caption_format: String,
    /// The title's text height (the subtitle's is half).
    pub title_text: TextSize,
    /// Output scale in percent of the images' own size.
    pub scale: u32,
    /// The cells' size (w, h) at 100 %, in pixels; none: the largest
    /// image's. Images fit (or fill) inside it either way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<[u32; 2]>,
    /// What Save writes.
    pub save_format: TileFormat,
    /// JPEG and AVIF quality, in percent.
    pub save_quality: u8,
}

/// A saved tiled image's format (DESIGN.md, Tiling).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "TileFormat")]
pub enum TileFormat {
    #[default]
    Png,
    /// Lossless, about half PNG's size.
    Webp,
    /// Lossy; transparency is flattened onto white.
    Jpeg,
    /// Lossy, smaller than JPEG at the same quality; only in builds with
    /// the `avif` feature.
    Avif,
}

impl TileFormat {
    /// The formats this build can write.
    pub fn available() -> &'static [TileFormat] {
        if cfg!(feature = "avif") {
            &[TileFormat::Png, TileFormat::Webp, TileFormat::Jpeg, TileFormat::Avif]
        } else {
            &[TileFormat::Png, TileFormat::Webp, TileFormat::Jpeg]
        }
    }

    /// This format if the build can write it, else PNG (a setting saved by
    /// a build with AVIF).
    pub fn usable(self) -> TileFormat {
        if Self::available().contains(&self) { self } else { TileFormat::Png }
    }

    pub fn ext(self) -> &'static str {
        match self {
            TileFormat::Png => "png",
            TileFormat::Webp => "webp",
            TileFormat::Jpeg => "jpg",
            TileFormat::Avif => "avif",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TileFormat::Png => "PNG",
            TileFormat::Webp => "WebP",
            TileFormat::Jpeg => "JPEG",
            TileFormat::Avif => "AVIF",
        }
    }

    pub fn lossy(self) -> bool {
        matches!(self, TileFormat::Jpeg | TileFormat::Avif)
    }
}

/// A Tile text's height (DESIGN.md, Tiling).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "TextSize")]
pub enum TextSize {
    /// To suit the tiling: captions 5 % of the cell's height; the title
    /// 4 % of the image's width, at least 10 % of the cell's height; a text
    /// block as large as fits its cells, up to the title's size.
    Auto,
    /// Percent of the cell's height.
    Cell(f32),
    /// Output pixels.
    Px(u32),
}

impl Default for TileSettings {
    fn default() -> TileSettings {
        TileSettings {
            gap: 16,
            background: [30, 30, 30, 255],
            fill: false,
            caption_text: TextSize::Auto,
            caption_color: [235, 235, 235, 255],
            caption_format: "{style} · seed {seed}".into(),
            title_text: TextSize::Auto,
            scale: 100,
            cell: None,
            save_format: TileFormat::Png,
            save_quality: 70,
        }
    }
}

/// How long, in seconds, something may go without progress before it's
/// given up on: each a stall limit, restarted by every sign of life, not a
/// cap on the whole (a slow download or answer that keeps coming is never
/// cut off). Read by the clients as they're made ([`timeouts`]; set with
/// [`set_timeouts`]). 0 isn't allowed: at least 1 s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Timeouts", default)]
pub struct Timeouts {
    /// A server being started: no new line in its log.
    pub server_start: u64,
    /// A request to an image server: no data (results, status).
    pub request: u64,
    /// Prompt enhance's answer: no word yet, or none since the last one
    /// (it reads the images first, minutes on a CPU).
    pub chat: u64,
    /// A model download: no data.
    pub download: u64,
}

impl Timeouts {
    pub const DEFAULT: Timeouts = Timeouts { server_start: 300, request: 60, chat: 300, download: 60 };

    pub fn server_start(&self) -> std::time::Duration {
        secs(self.server_start)
    }
    pub fn request(&self) -> std::time::Duration {
        secs(self.request)
    }
    pub fn chat(&self) -> std::time::Duration {
        secs(self.chat)
    }
    pub fn download(&self) -> std::time::Duration {
        secs(self.download)
    }
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts::DEFAULT
    }
}

fn secs(s: u64) -> std::time::Duration {
    std::time::Duration::from_secs(s.max(1))
}

static TIMEOUTS: std::sync::RwLock<Timeouts> = std::sync::RwLock::new(Timeouts::DEFAULT);

/// The timeouts in use (the config's, once [`set_timeouts`]).
pub fn timeouts() -> Timeouts {
    *TIMEOUTS.read().unwrap_or_else(|e| e.into_inner())
}

/// The config's timeouts for the clients made from now on.
pub fn set_timeouts(t: Timeouts) {
    *TIMEOUTS.write().unwrap_or_else(|e| e.into_inner()) = t;
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

fn localhost() -> String {
    "127.0.0.1".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Resolution")]
pub struct Resolution {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
}

impl Resolution {
    pub fn new(name: &str, width: u32, height: u32) -> Resolution {
        Resolution { name: name.into(), width, height, default: false }
    }

    /// `W × H` followed by the name, as the size dropdown shows it.
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            format!("{} × {}", self.width, self.height)
        } else {
            format!("{} × {}  {}", self.width, self.height, self.name)
        }
    }
}

pub fn builtin_resolutions() -> Vec<Resolution> {
    let mut list = vec![
        Resolution::new("Square", 1024, 1024),
        Resolution::new("Square L", 1344, 1344),
        Resolution::new("Square XL", 2048, 2048),
        Resolution::new("Portrait 2:3", 896, 1344),
        Resolution::new("Landscape 3:2", 1344, 896),
        Resolution::new("Wide 16:9", 1664, 928),
    ];
    list[0].default = true;
    list
}

/// Nearest multiple of 32 (halves round up), at least 32.
pub fn snap32(v: u32) -> u32 {
    (v.saturating_add(16) / 32 * 32).max(32)
}

impl Config {
    /// `config.rsn` in the app's [config folder](user_dir).
    pub fn default_path() -> Option<PathBuf> {
        Some(user_dir("XDG_CONFIG_HOME", ".config", "APPDATA")?.join("config.rsn"))
    }

    /// Reads and [normalizes](Self::normalize) a config; a missing file is
    /// the default config. Returns the warnings to log. Old PE servers moved
    /// into the presets are saved so at once, the file as it was kept
    /// beside it (`config.rsn.before-chat-presets`).
    pub fn load(path: &Path) -> Result<(Config, Vec<String>)> {
        let mut config: Config = if path.exists() { rsnfmt::read(path)? } else { Config::default() };
        let moving = !config.pe.servers.is_empty();
        let mut warnings = config.normalize();
        if moving {
            let mut backup = path.as_os_str().to_owned();
            backup.push(".before-chat-presets");
            let saved = fs::copy(path, &backup)
                .map_err(|e| e.to_string())
                .and_then(|_| config.save(path).map_err(|e| e.to_string()));
            match saved {
                Ok(()) => warnings.push(format!("the config as it was is kept in {}", Path::new(&backup).display())),
                Err(e) => warnings.push(format!("the moved PE servers weren't saved: {e}")),
            }
        }
        Ok((config, warnings))
    }

    /// As [`load`](Self::load), but a file that can't be read gives the
    /// default config, with a copy of the file kept beside it first
    /// (`config.rsn.unreadable`, or `.unreadable-2` and on if taken), since
    /// the next save writes the defaults over it.
    pub fn load_or_default(path: &Path) -> (Config, Vec<String>) {
        match Config::load(path) {
            Ok(loaded) => loaded,
            Err(e) => {
                let mut c = Config::default();
                let mut warnings = vec![format!("settings not loaded ({e}); using defaults")];
                match keep_unreadable(path) {
                    Ok(kept) => warnings.push(format!("the unreadable settings are kept in {}", kept.display())),
                    Err(e) => warnings.push(format!("the unreadable settings couldn't be kept: {e}")),
                }
                warnings.extend(c.normalize());
                (c, warnings)
            }
        }
    }

    /// Writes the config, entries' KoboldCpp binaries that are the shared
    /// one left out (read back from it, see [`fill_koboldcpp`](Self::fill_koboldcpp)).
    pub fn save(&self, path: &Path) -> Result<()> {
        let shared = self.programs.koboldcpp.trim();
        if shared.is_empty() {
            return rsnfmt::write_compact(path, self);
        }
        let mut c = self.clone();
        for b in c.koboldcpp_binaries() {
            if b.trim() == shared {
                b.clear();
            }
        }
        rsnfmt::write_compact(path, &c)
    }

    /// The KoboldCpp binaries the presets' started entries name.
    fn koboldcpp_binaries(&mut self) -> impl Iterator<Item = &mut String> {
        use crate::kobold::KoboldRun;
        use crate::pe::PeRun;
        self.server_presets.iter_mut().flat_map(|p| &mut p.servers).filter_map(|s| match s {
            ServerConfig::Kobold(k) => match &mut k.run {
                KoboldRun::Start { binary, .. } => Some(binary),
                KoboldRun::Connect { .. } => None,
            },
            ServerConfig::Chat(c) => match &mut c.run {
                PeRun::KoboldCpp { binary, .. } => Some(binary),
                PeRun::Connect { .. } => None,
            },
            _ => None,
        })
    }

    /// Entries naming no KoboldCpp binary start the shared one (when set).
    pub fn fill_koboldcpp(&mut self) {
        let shared = self.programs.koboldcpp.trim().to_string();
        if shared.is_empty() {
            return;
        }
        for b in self.koboldcpp_binaries() {
            if b.trim().is_empty() {
                *b = shared.clone();
            }
        }
    }

    /// Applies the load rules: old PE servers moved into the server presets,
    /// the servers' paths made [full](Self::full_paths),
    /// `library` folded into the recent libraries,
    /// built-in list if empty, sizes snapped to multiples of 32, exactly one
    /// default (the first marked, else the first).
    pub fn normalize(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(s) = self.old_sdserver.take() {
            self.programs.sd_server = s.0;
        }
        if let Some(k) = self.old_koboldcpp.take() {
            self.programs.koboldcpp = k;
        }
        warnings.extend(self.move_pe_servers());
        let said = self.full_paths();
        self.fill_koboldcpp();
        if !said.is_empty() {
            warnings.push(format!(
                "paths made full (a relative one is taken from the folder IncreArt started in): {}",
                said.join("; ")
            ));
        }
        if let Some(l) = self.library.take().filter(|l| !l.is_empty()) {
            self.add_recent_library(l);
        }
        if std::mem::take(&mut self.projects_by_name) {
            self.project_order = TreeOrder::Name;
        }
        if std::mem::take(&mut self.folders_by_newest) {
            self.folder_order = FolderOrder(TreeOrder::Created);
        }
        if std::mem::take(&mut self.review_oldest_first) {
            self.review_order = ReviewOrder::OldestToReview;
        }
        if self.recipes.is_empty() {
            self.recipes = builtin_recipes();
        }
        if self.resolutions.is_empty() {
            self.resolutions = builtin_resolutions();
            return warnings;
        }
        for r in &mut self.resolutions {
            let (w, h) = (snap32(r.width), snap32(r.height));
            if (w, h) != (r.width, r.height) {
                warnings.push(format!(
                    "resolution {:?}: {} × {} is not a multiple of 32, using {w} × {h}",
                    r.name, r.width, r.height
                ));
                (r.width, r.height) = (w, h);
            }
        }
        let first = self.resolutions.iter().position(|r| r.default).unwrap_or(0);
        for (i, r) in self.resolutions.iter_mut().enumerate() {
            r.default = i == first;
        }
        warnings
    }

    /// The binaries, model directories and files the servers are given as
    /// [full paths](full_path) (libraries are already); what changed, said
    /// as `old → new`.
    pub fn full_paths(&mut self) -> Vec<String> {
        let mut said = vec![];
        make_full(&mut self.programs.sd_server.binary, full_binary, &mut said);
        make_full(&mut self.programs.koboldcpp, full_binary, &mut said);
        for d in &mut self.programs.sd_server.model_dirs {
            make_full(d, full_path, &mut said);
        }
        let mut seen = vec![];
        self.programs.sd_server.model_dirs.retain(|d| {
            !d.is_empty() && !seen.contains(d) && {
                seen.push(d.clone());
                true
            }
        });
        for s in self.server_presets.iter_mut().flat_map(|p| &mut p.servers) {
            s.full_paths(&mut said);
        }
        for s in &mut self.pe.servers {
            s.run.full_paths(&mut said);
        }
        said
    }

    /// Puts `root` first in the recent libraries (once), keeping
    /// [`MAX_RECENT_LIBRARIES`] (the example library stays).
    pub fn add_recent_library(&mut self, root: String) {
        self.recent_libraries.retain(|r| *r != root);
        self.recent_libraries.insert(0, root);
        self.trim_recent_libraries();
    }

    fn trim_recent_libraries(&mut self) {
        while self.recent_libraries.len() > MAX_RECENT_LIBRARIES {
            let Some(i) = self.recent_libraries.iter().rposition(|r| *r != self.example_library) else { break };
            self.recent_libraries.remove(i);
        }
    }

    /// The example library beside the app, `lib`, in the recent list until
    /// removed from it: one from an earlier release, there, is replaced.
    pub fn offer_example_library(&mut self, lib: String) {
        if self.example_library_removed {
            return;
        }
        let old = std::mem::replace(&mut self.example_library, lib.clone());
        if self.recent_libraries.contains(&lib) {
            if old != lib {
                self.recent_libraries.retain(|r| *r != old);
            }
        } else if let Some(at) = self.recent_libraries.iter().position(|r| *r == old) {
            self.recent_libraries[at] = lib;
        } else {
            self.recent_libraries.push(lib);
            self.trim_recent_libraries();
        }
    }

    /// Drops `recent_libraries[i]`; the example library isn't offered again.
    pub fn forget_recent_library(&mut self, i: usize) {
        if i < self.recent_libraries.len() && self.recent_libraries.remove(i) == self.example_library {
            self.example_library_removed = true;
        }
    }

    pub fn default_resolution(&self) -> Option<&Resolution> {
        self.resolutions.iter().find(|r| r.default).or(self.resolutions.first())
    }
}

/// The folder beside the executable that release zips ship the example
/// library in.
pub const EXAMPLE_LIBRARY: &str = "Example library";

/// The example library shipped beside the running executable, if any
/// ([`Config::offer_example_library`]).
pub fn bundled_example_library() -> Option<PathBuf> {
    example_library_in(std::env::current_exe().ok()?.parent()?)
}

fn example_library_in(dir: &Path) -> Option<PathBuf> {
    let lib = dir.join(EXAMPLE_LIBRARY);
    lib.join("library.rsn").is_file().then_some(lib)
}

/// Copies a config that failed to load to the first free
/// `<name>.unreadable`, `<name>.unreadable-2`, …; returns where.
fn keep_unreadable(path: &Path) -> std::io::Result<PathBuf> {
    let mut n = 1;
    loop {
        let mut name = path.as_os_str().to_owned();
        name.push(if n == 1 { ".unreadable".to_string() } else { format!(".unreadable-{n}") });
        let kept = PathBuf::from(name);
        if !kept.exists() {
            fs::copy(path, &kept)?;
            return Ok(kept);
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_tree_orders_fold_in() {
        let mut c: Config = rsnfmt::from_str("Config { projects_by_name: true, folders_by_newest: true }").unwrap();
        c.normalize();
        assert_eq!((c.project_order, c.folder_order.0), (TreeOrder::Name, TreeOrder::Created));
        let text = rsnfmt::to_string(&c).unwrap();
        assert!(!text.contains("_by_"), "{text}");
        assert_eq!(rsnfmt::from_str::<Config>(&text).unwrap(), c);
        assert_eq!(Config::default().folder_order.0, TreeOrder::Name, "folders A–Z by default");
        assert_eq!(Config::default().review_order, ReviewOrder::LastRun, "Review last run first by default");
        // Follow's pace: the time an image makes at the lag, quicker behind it
        let f = FollowPace::default();
        assert_eq!(f.secs(4, Some(20.0)), 20.0);
        assert_eq!(f.secs(9, Some(20.0)), 10.0);
        assert_eq!(f.secs(0, Some(20.0)), 60.0, "at most max_secs");
        assert_eq!(f.secs(40, Some(20.0)), 3.0, "at least min_secs");
        assert_eq!(f.secs(4, None), 8.0, "nothing made lately: fixed");
        assert_eq!(FollowPace { auto: false, ..f }.secs(9, Some(20.0)), 8.0);
        let mut c: Config = rsnfmt::from_str("Config { review_oldest_first: true }").unwrap();
        c.normalize();
        assert_eq!(c.review_order, ReviewOrder::OldestToReview);
        assert!(!rsnfmt::to_string(&c).unwrap().contains("oldest"));
    }

    #[test]
    fn old_pe_servers_move_into_the_presets() {
        let text = r#"Config {
            server_presets: [
                ServerPreset { name: "Both", servers: [Start { name: "gpu0", port: 1234, device: "ROCm0" }] },
                ServerPreset { name: "CPU", servers: [] },
            ],
            pe: Pe {
                server: "B",
                servers: [
                    PeServer { name: "A", run: Connect { url: "http://a:1" } },
                    PeServer {
                        name: "B",
                        run: Connect { url: "http://b:1" },
                        start: AtLaunch,
                        system_prompts: { "rewrite": "mine" },
                    },
                ],
            },
        }"#;
        let mut c: Config = rsnfmt::from_str(text).unwrap();
        let w = c.normalize();
        assert!(w[0].contains("B is now in every server preset") && w[0].contains("only a chat server: A"), "{w:?}");
        let names: Vec<_> =
            c.server_presets.iter().map(|p| (p.name.as_str(), p.chat().map(|s| s.name.as_str()))).collect();
        assert_eq!(names, [("Both", Some("B")), ("CPU", Some("B")), ("A", Some("A"))]);
        assert_eq!(c.server_presets[0].sd_servers().len(), 1);
        assert_eq!(c.server_presets[0].chat().unwrap().start, crate::pe::PeStart::AtLaunch);
        assert_eq!(c.pe.system_prompt(crate::pe::REWRITE), "mine", "the used one's prompts are everyone's");
        assert_eq!(c.chat_servers().len(), 2, "one per process");
        // written the new way, and read back the same
        let out = rsnfmt::to_string(&c).unwrap();
        assert!(out.contains("OpenAiChat {") && !out.contains("servers: []") && !out.contains("server:"), "{out}");
        assert_eq!(rsnfmt::from_str::<Config>(&out).unwrap(), c);
        assert!(c.clone().normalize().is_empty(), "moved once");
    }

    #[test]
    fn loading_saves_the_moved_pe_servers_with_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.rsn");
        let old = r#"Config { pe: Pe { servers: [PeServer { name: "A", run: Connect { url: "http://a:1" } }] } }"#;
        fs::write(&path, old).unwrap();
        let (c, w) = Config::load(&path).unwrap();
        assert!(w.iter().any(|w| w.contains("before-chat-presets")), "{w:?}");
        assert_eq!(fs::read_to_string(dir.path().join("config.rsn.before-chat-presets")).unwrap(), old);
        assert_eq!(Config::load(&path).unwrap(), (c, vec![]));
    }

    #[test]
    fn the_example_library_is_found_beside_the_app_only_when_it_is_a_library() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(example_library_in(dir.path()), None);
        let lib = dir.path().join(EXAMPLE_LIBRARY);
        fs::create_dir(&lib).unwrap();
        assert_eq!(example_library_in(dir.path()), None, "a folder without library.rsn");
        fs::write(lib.join("library.rsn"), "").unwrap();
        assert_eq!(example_library_in(dir.path()), Some(lib));
    }

    #[test]
    fn an_unreadable_config_is_kept_before_defaults_are_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.rsn");
        fs::write(&path, "Config { oops").unwrap();
        let (c, w) = Config::load_or_default(&path);
        assert_eq!(c.recipes, builtin_recipes());
        assert!(w.iter().any(|w| w.contains("config.rsn.unreadable")), "{w:?}");
        assert_eq!(fs::read_to_string(dir.path().join("config.rsn.unreadable")).unwrap(), "Config { oops");
        // a second bad file doesn't replace the first copy
        fs::write(&path, "Config { again").unwrap();
        Config::load_or_default(&path);
        assert_eq!(fs::read_to_string(dir.path().join("config.rsn.unreadable")).unwrap(), "Config { oops");
        assert_eq!(fs::read_to_string(dir.path().join("config.rsn.unreadable-2")).unwrap(), "Config { again");
    }

    #[test]
    fn snapping() {
        assert_eq!(snap32(1024), 1024);
        assert_eq!(snap32(1000), 992);
        assert_eq!(snap32(1008), 1024);
        assert_eq!(snap32(0), 32);
    }

    #[test]
    fn normalize_rules() {
        let mut c: Config = rsnfmt::from_str(
            r#"Config { resolutions: [
                Resolution { width: 1000, height: 1024 },
                Resolution { name: "b", width: 512, height: 512, default: true },
                Resolution { name: "c", width: 640, height: 640, default: true },
            ] }"#,
        )
        .unwrap();
        let w = c.normalize();
        assert_eq!(w.len(), 1);
        assert_eq!(c.resolutions[0].width, 992);
        assert_eq!(c.resolutions.iter().filter(|r| r.default).count(), 1);
        assert_eq!(c.default_resolution().unwrap().name, "b");

        let mut none: Config = rsnfmt::from_str(
            "Config { resolutions: [Resolution { width: 64, height: 64 }, Resolution { width: 96, height: 96 }] }",
        )
        .unwrap();
        none.normalize();
        assert!(none.resolutions[0].default);

        let mut empty: Config = rsnfmt::from_str("Config {}").unwrap();
        empty.normalize();
        assert_eq!(empty.resolutions, builtin_resolutions());
    }

    #[test]
    fn tile_text_sizes() {
        // the old pixel sizes are dropped for auto; the rest kept
        let c: Config =
            rsnfmt::from_str("Config { tile: TileSettings { gap: 8, caption_size: 32, title_size: 64 } }").unwrap();
        assert_eq!((c.tile.gap, c.tile.caption_text, c.tile.title_text), (8, TextSize::Auto, TextSize::Auto));
        let c = Config {
            tile: TileSettings {
                caption_text: TextSize::Cell(4.5),
                title_text: TextSize::Px(90),
                ..Default::default()
            },
            ..Default::default()
        };
        let back: Config = rsnfmt::from_str(&rsnfmt::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.tile, c.tile);
    }

    #[test]
    fn recent_libraries() {
        let mut c: Config = rsnfmt::from_str(r#"Config { recent_libraries: ["/b", "/a"], library: "/a" }"#).unwrap();
        c.normalize();
        assert_eq!(c.recent_libraries, ["/a", "/b"], "the old last library goes first, once");
        assert!(!rsnfmt::to_string(&c).unwrap().contains("library:"), "the old field is not written back");
        for i in 0..MAX_RECENT_LIBRARIES + 2 {
            c.add_recent_library(format!("/{i}"));
        }
        assert_eq!(c.recent_libraries.len(), MAX_RECENT_LIBRARIES);
        assert_eq!(c.recent_libraries[0], format!("/{}", MAX_RECENT_LIBRARIES + 1));
    }

    #[test]
    fn the_example_library_stays_in_the_list_until_removed() {
        let mut c = Config::default();
        c.offer_example_library("/v1/Example library".into());
        c.add_recent_library("/a".into());
        c.offer_example_library("/v1/Example library".into());
        assert_eq!(c.recent_libraries, ["/a", "/v1/Example library"], "offered once, where it was");
        for i in 0..MAX_RECENT_LIBRARIES + 2 {
            c.add_recent_library(format!("/{i}"));
        }
        assert_eq!(c.recent_libraries.len(), MAX_RECENT_LIBRARIES);
        assert_eq!(c.recent_libraries.last().unwrap(), "/v1/Example library", "never pushed out");
        c.offer_example_library("/v2/Example library".into());
        assert_eq!(c.recent_libraries.last().unwrap(), "/v2/Example library", "a new release's in its place");
        assert!(!c.recent_libraries.iter().any(|r| r.starts_with("/v1")));
        let i = c.recent_libraries.len() - 1;
        c.forget_recent_library(i);
        c.offer_example_library("/v3/Example library".into());
        assert!(!c.recent_libraries.iter().any(|r| r.contains("Example")), "removed: not offered again");
        let back: Config = rsnfmt::from_str(&rsnfmt::to_string(&c).unwrap()).unwrap();
        assert!(back.example_library_removed);
    }

    #[test]
    fn koboldcpp_entries() {
        let text = r#"Config { server_presets: [ServerPreset { name: "K", servers: [
            Kobold(KoboldServer {
                name: "kcpp0",
                run: Start { binary: "/k/koboldcpp", port: 5001, device: Gpu { devices: [1] } },
                chat: Some(KoboldChat { model: "/m/c.gguf", context: 8192 }),
            }),
            Kobold(KoboldServer { name: "box", run: Connect { url: "http://10.0.0.2:5001", device: "Vulkan1" } }),
            Kobold(KoboldServer { name: "amd", run: Start { binary: "k", port: 5002, device: Gpu { devices: [0, 1], runtime: Rocm } } }),
        ] }] }"#;
        let c: Config = rsnfmt::from_str(text).unwrap();
        let p = &c.server_presets[0];
        assert_eq!(p.sd_servers().len(), 3, "image servers");
        assert_eq!(p.servers[2].device(), "ROCm0,1");
        assert_eq!(
            (p.servers[0].device(), p.servers[0].base_url()),
            ("Vulkan1".into(), "http://127.0.0.1:5001".into())
        );
        assert!(p.servers[0].is_started() && !p.servers[1].is_started());
        assert_eq!(p.chat_server().unwrap().base_url(), "http://127.0.0.1:5001", "its chat server too");
        let back: Config = rsnfmt::from_str(&rsnfmt::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.server_presets, c.server_presets);
    }

    #[test]
    fn prompt_enhance_shares_every_chat_server_but_swapping_ones() {
        let text = r#"ServerPreset { name: "two", servers: [
            Kobold(KoboldServer { name: "a", run: Start { binary: "k", port: 5001 }, chat: Some(KoboldChat { model: "m" }), swap: true }),
            Kobold(KoboldServer { name: "b", run: Start { binary: "k", port: 5002 }, chat: Some(KoboldChat { model: "m" }) }),
            Kobold(KoboldServer { name: "c", run: Start { binary: "k", port: 5003 }, chat: Some(KoboldChat { model: "m" }) }),
            Kobold(KoboldServer { name: "d", run: Start { binary: "k", port: 5004 } }),
        ] }"#;
        let mut p: ServerPreset = rsnfmt::from_str(text).unwrap();
        let urls = |p: &ServerPreset| p.chat_pool().iter().map(|c| c.base_url()).collect::<Vec<_>>();
        assert_eq!(urls(&p), ["http://127.0.0.1:5002", "http://127.0.0.1:5003"], "not the swapping one");
        assert_eq!(p.chat_server().unwrap().name, "b");
        p.servers.drain(1..3);
        assert_eq!(urls(&p), ["http://127.0.0.1:5001"], "swapping, if it's the only one");
    }

    #[test]
    fn each_servers_roles() {
        // as reported: KoboldCpp, an sd-server proxy, and a chat server
        let text = r#"ServerPreset { name: "mine", servers: [
            Kobold(KoboldServer { name: "kobold", run: Start { binary: "k", port: 5001 }, chat: Some(KoboldChat { model: "m" }), swap: true }),
            Connect { name: "proxy", url: "http://127.0.0.1:7860" },
            Chat(PeServer { name: "PE", run: KoboldCpp { binary: "k", model: "q.gguf", mmproj: "mm.gguf", host: "127.0.0.1", port: 5010 } }),
        ] }"#;
        let p: ServerPreset = rsnfmt::from_str(text).unwrap();
        let r = p.roles();
        fn flags(r: &Roles) -> (&str, bool, bool, Option<bool>) {
            (r.name.as_str(), r.images, r.text, r.vision)
        }
        assert_eq!(
            r.iter().map(flags).collect::<Vec<_>>(),
            [("kobold", true, false, None), ("proxy", true, false, None), ("PE", false, true, Some(true))]
        );
        assert!(r[0].note.as_deref().unwrap().contains("swaps its chat model"), "{:?}", r[0].note);
        assert_eq!(p.roles_line(), "images: kobold, proxy · text: PE · vision: PE");
        // KoboldCpp alone, chatting without a vision projector
        let text = r#"ServerPreset { name: "one", servers: [
            Kobold(KoboldServer { name: "k", run: Start { binary: "k", port: 5001 }, chat: Some(KoboldChat { model: "m" }) }),
        ] }"#;
        let p: ServerPreset = rsnfmt::from_str(text).unwrap();
        assert_eq!(p.roles()[0].vision, Some(false));
        assert_eq!(p.roles_line(), "images: k · text: k · vision: none (asks go without images)");
        // and prompt enhance's asks to it go without the images
        assert!(!p.chat_server().unwrap().vision());
        // with a projector, or one its .kcpps may name, or connected to: with them
        for run in [
            r#"run: Start { binary: "k", port: 5001 }, chat: Some(KoboldChat { model: "m", mmproj: "mm" })"#,
            r#"run: Start { binary: "k", port: 5001, config: "my.kcpps" }, chat: Some(KoboldChat { })"#,
            r#"run: Connect { url: "http://k:5001" }, chat: Some(KoboldChat { })"#,
        ] {
            let text =
                format!(r#"ServerPreset {{ name: "one", servers: [Kobold(KoboldServer {{ name: "k", {run} }})] }}"#);
            let p: ServerPreset = rsnfmt::from_str(&text).unwrap();
            assert!(p.chat_server().unwrap().vision(), "{run}");
        }
        let p = ServerPreset {
            name: "sd".into(),
            servers: vec![ServerConfig::Connect { name: "box".into(), url: "u".into(), device: String::new() }],
        };
        assert_eq!(p.roles_line(), "images: box · no prompt enhance");
    }

    #[test]
    fn server_presets() {
        let mut c: Config = rsnfmt::from_str(
            r#"Config {
                server_presets: [
                    ServerPreset { name: "Both GPUs", servers: [
                        Start { name: "gpu0", port: 1234, device: "ROCm0" },
                        Start { name: "gpu1", port: 1235, device: "ROCm1", args: ["--offload-to-cpu"] },
                    ] },
                    ServerPreset { name: "Remote", servers: [Connect { name: "box", url: "http://10.0.0.2:1234/" }] },
                ],
                last_preset: "Remote",
                sdserver: SdServer { binary: "/opt/sd-server", model_dirs: ["/models"] },
            }"#,
        )
        .unwrap();
        c.normalize();
        assert_eq!(c.programs.sd_server.binary, full_path("/opt/sd-server"), "the old field read");
        let both = c.preset("Both GPUs").unwrap();
        assert_eq!(both.servers[0].base_url(), "http://127.0.0.1:1234");
        assert_eq!(both.servers[1].device(), "ROCm1");
        assert_eq!(c.preset("Remote").unwrap().servers[0].base_url(), "http://10.0.0.2:1234");
        assert_eq!(
            c.programs.sd_server.args,
            ["--fa", "--conditioning-cache-size", "32"],
            "defaults for missing fields"
        );
        let again: Config = rsnfmt::from_str(&rsnfmt::to_string(&c).unwrap()).unwrap();
        assert_eq!(again, c);
    }

    /// Entries named for their program, read into the app's types; older
    /// configs' entries the same; saved in the new form, shared binaries
    /// left out.
    #[test]
    fn server_entries_as_written() {
        use crate::kobold::{KoboldChat, KoboldRun, KoboldServer};
        use crate::pe::{GpuRuntime, PeDevice, PeRun, PeServer, PeStart};
        let new = r#"Config {
            model_presets: [ModelPreset { name: "m", recipe: "r", files: {}, steps: 20, cfg: 2 }],
            server_presets: [ServerPreset { name: "p", servers: [
                SdServer { name: "sd0", run: Launch { port: 1234, gpu: Rocm(0), args: ["--offload-to-cpu"] } },
                SdServer { name: "box", run: Connect { url: "http://b:1", gpu: Backend("MTL0") } },
                KoboldCpp {
                    name: "k",
                    run: Launch { port: 5001, gpu: Vulkan([1, 2]) },
                    chat: Chat { model: "/m/chat.gguf", context: 8192 },
                    swap: true,
                },
                KoboldCpp { name: "pe", run: Launch { binary: "/own/kcpp", port: 5003, gpu: Cpu(8) }, images: false, when: AtOpen },
                OpenAiChat { name: "far", url: "http://far:1" },
            ] }],
            programs: Programs { sd_server: SdServerSettings { binary: "/opt/sd-server" }, koboldcpp: "/opt/kcpp" },
        }"#;
        let old = r#"Config {
            recipes: [RecipePreset { name: "m", recipe: "r", files: {}, steps: 20, cfg: 2 }],
            server_presets: [ServerPreset { name: "p", servers: [
                Start { name: "sd0", port: 1234, device: "ROCm0", args: ["--offload-to-cpu"] },
                Connect { name: "box", url: "http://b:1", device: "MTL0" },
                Kobold(KoboldServer {
                    name: "k",
                    run: Start { binary: "/opt/kcpp", port: 5001, device: Gpu { devices: [1, 2] } },
                    chat: KoboldChat { model: "/m/chat.gguf", context: 8192 },
                    swap: true,
                }),
                Chat(PeServer {
                    name: "pe",
                    run: KoboldCpp { binary: "/own/kcpp", host: "127.0.0.1", port: 5003, device: Cpu { threads: 8 } },
                    start: AtLaunch,
                }),
                Chat(PeServer { name: "far", run: Connect { url: "http://far:1" } }),
            ] }],
            sdserver: SdServer { binary: "/opt/sd-server" },
            koboldcpp: "/opt/kcpp",
        }"#;
        let read = |text: &str| {
            let mut c: Config = rsnfmt::from_str(text).unwrap();
            c.normalize();
            c
        };
        let c = read(new);
        assert_eq!(c, read(old), "the old form reads the same");
        assert_eq!(c.recipes[0].name, "m");
        assert_eq!(c.programs.sd_server.binary, full_path("/opt/sd-server"));
        let s = &c.server_presets[0].servers;
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            s[0],
            ServerConfig::Start {
                name: "sd0".into(),
                host: "127.0.0.1".into(),
                port: 1234,
                device: "ROCm0".into(),
                args: l(&["--offload-to-cpu"])
            }
        );
        assert_eq!(s[1], ServerConfig::Connect { name: "box".into(), url: "http://b:1".into(), device: "MTL0".into() });
        assert_eq!(
            s[2],
            ServerConfig::Kobold(KoboldServer {
                name: "k".into(),
                run: KoboldRun::Start {
                    binary: full_path("/opt/kcpp"),
                    config: String::new(),
                    host: "127.0.0.1".into(),
                    port: 5001,
                    device: PeDevice::Gpu { devices: vec![1, 2], runtime: GpuRuntime::Vulkan },
                    args: vec![],
                },
                chat: Some(KoboldChat { model: full_path("/m/chat.gguf"), mmproj: String::new(), context: 8192 }),
                swap: true,
            }),
            "the shared binary filled in"
        );
        let ServerConfig::Chat(PeServer { run: PeRun::KoboldCpp { binary, device, .. }, start, .. }) = &s[3] else {
            panic!("{:?}", s[3])
        };
        assert_eq!(
            (binary, device, *start),
            (&full_path("/own/kcpp"), &PeDevice::Cpu { threads: 8 }, PeStart::AtLaunch)
        );
        assert!(matches!(
            &s[4],
            ServerConfig::Chat(PeServer { run: PeRun::Connect { .. }, start: PeStart::OnDemand, .. })
        ));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.rsn");
        c.save(&path).unwrap();
        let out = fs::read_to_string(&path).unwrap();
        // full paths (on Windows with the drive), as RSN writes them
        let quoted = |p: &str| rsnfmt::to_string(&full_path(p)).unwrap();
        for want in [
            "model_presets: [",
            "ModelPreset {",
            "SdServer {",
            "gpu: Rocm(0)",
            r#"gpu: Backend("MTL0")"#,
            "gpu: Vulkan([1, 2])",
            "gpu: Cpu(8)",
            "chat: Chat {",
            "images: false",
            "when: AtOpen",
            "OpenAiChat {",
            "programs: Programs {",
            &format!("binary: {}", quoted("/own/kcpp")),
        ] {
            assert!(out.contains(want), "{want} in {out}");
        }
        for old in ["recipes", "Start {", "Kobold(", "PeServer", "Some(", "127.0.0.1"] {
            assert!(!out.contains(old), "{old} in {out}");
        }
        assert_eq!(out.matches(&quoted("/opt/kcpp")).count(), 1, "the shared binary only in programs: {out}");
        assert_eq!(Config::load(&path).unwrap().0, c, "read back the same");
    }

    #[test]
    fn server_entries_that_cant_be() {
        let entry = |e: &str| {
            let text = format!("Config {{ server_presets: [ServerPreset {{ name: \"p\", servers: [{e}] }}] }}");
            rsnfmt::from_str::<Config>(&text).map(|_| ()).map_err(|e| format!("{:?}", e.kind))
        };
        assert!(
            entry(r#"SdServer { name: "a", run: Launch { port: 1, gpu: Rocm([0, 1]) } }"#)
                .unwrap_err()
                .contains("one device")
        );
        assert!(
            entry(r#"KoboldCpp { name: "a", run: Launch { port: 1, gpu: Backend("X") } }"#)
                .unwrap_err()
                .contains("Backend")
        );
        assert!(
            entry(r#"KoboldCpp { name: "a", run: Launch { port: 1 }, when: AtOpen }"#)
                .unwrap_err()
                .contains("chat-only")
        );
        assert!(
            entry(r#"KoboldCpp { name: "a", run: Connect { url: "u" }, images: false }"#)
                .unwrap_err()
                .contains("OpenAiChat")
        );
        assert!(entry(r#"SdServer { name: "a", run: Launch { port: 1, gpu: Cuda(1) } }"#).is_ok());
    }

    #[test]
    fn model_files_named_inside_their_folder() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models").to_string_lossy().into_owned();
        let dirs = vec!["/elsewhere".to_string(), models.clone()];
        let file = Path::new(&models).join("vae").join("v.safetensors");
        assert_eq!(model_file_id(&file, &dirs).unwrap(), "vae/v.safetensors");
        assert!(model_file_id(Path::new(&models), &dirs).is_err(), "the folder itself");
        let outside = model_file_id(&dir.path().join("x.gguf"), &dirs).unwrap_err();
        assert!(outside.contains("isn't in a model directory"), "{outside}");
        assert!(model_file_id(&file, &[]).unwrap_err().contains("No model directory"));
        // on disk: the first folder that has it
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"").unwrap();
        assert_eq!(model_file_on_disk("vae/v.safetensors", &dirs), Some(file.clone()));
        assert_eq!(model_file_on_disk("vae/none.safetensors", &dirs), None);
        assert_eq!(model_file_on_disk("", &dirs), None, "not the folder");
        assert_eq!(model_file_on_disk(&file.to_string_lossy(), &[]), Some(file));
    }

    #[test]
    fn loras_named_inside_their_models_folder() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        let dirs = vec![models.to_string_lossy().into_owned(), dir.path().join("more").to_string_lossy().into_owned()];
        let id = |p: PathBuf| lora_file_id(&p, &dirs);
        assert_eq!(id(models.join("loras/style.safetensors")).unwrap(), "loras/style.safetensors");
        assert_eq!(id(models.join("Lora/sub/x.gguf")).unwrap(), "Lora/sub/x.gguf");
        assert_eq!(id(dir.path().join("more/loras/a.gguf")).unwrap(), "loras/a.gguf", "any model directory");
        for wrong in [models.join("vae/v.safetensors"), models.join("loras"), dir.path().join("other/loras/a.gguf")] {
            assert!(id(wrong).unwrap_err().contains("isn't in the loras folder"));
        }
        assert!(lora_file_id(&models.join("loras/a.gguf"), &[]).unwrap_err().contains("No model directory"));
        assert_eq!(lora_request_path("loras/style.safetensors"), "style.safetensors");
        assert_eq!(lora_request_path("LoRA/sub/x.gguf"), "sub/x.gguf");
        assert_eq!(lora_request_path("x.gguf"), "x.gguf");
        // on disk as it's spelt, `loras` first
        assert_eq!(lora_dir(&dirs[0]), None);
        std::fs::create_dir_all(models.join("Lora")).unwrap();
        assert_eq!(lora_dir(&dirs[0]), Some(models.join("Lora")));
        std::fs::create_dir_all(models.join("loras")).unwrap();
        assert_eq!(lora_dir(&dirs[0]), Some(models.join("loras")));
        std::fs::create_dir_all(dir.path().join("more/loras")).unwrap();
        let all = [models.join("loras"), models.join("Lora"), dir.path().join("more/loras")];
        assert_eq!(lora_dirs(&dirs), all, "every folder's, in order");
    }

    #[test]
    fn a_presets_loras_saved_and_read() {
        let mut c = Config { recipes: builtin_recipes(), ..Default::default() };
        c.recipes[0].loras = vec![PresetLora { file: "loras/style.safetensors".into(), weight: 0.8 }];
        let text = rsnfmt::to_string(&c).unwrap();
        assert!(text.contains("Lora"), "{text}");
        assert_eq!(rsnfmt::from_str::<Config>(&text).unwrap().recipes, c.recipes);
        // the weight defaults to 1
        let p: PresetLora = rsnfmt::from_str(r#"Lora { file: "loras/a.gguf" }"#).unwrap();
        assert_eq!(p.weight, 1.0);
    }

    #[test]
    fn paths_made_full() {
        let abs = |p: &str| std::path::absolute(p).unwrap().to_string_lossy().into_owned();
        let home = PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap());
        assert_eq!(full_path(" models "), abs("models"));
        assert_eq!(full_path(&abs("m")), abs("m"));
        assert_eq!(full_path("~"), home.to_string_lossy());
        assert_eq!(full_path("~/models"), home.join("models").to_string_lossy());
        assert_eq!(full_path("~user/m"), abs("~user/m"), "not the home folder");
        assert_eq!(full_path(""), "");
        // a bare name is looked up on PATH, a command line left alone
        assert_eq!(full_binary("koboldcpp"), "koboldcpp");
        assert_eq!(full_binary("./koboldcpp"), abs("./koboldcpp"));
        assert_eq!(full_binary("python3 k/koboldcpp.py"), "python3 k/koboldcpp.py");

        let mut c: Config = rsnfmt::from_str(
            r#"Config {
                server_presets: [ServerPreset { name: "k", servers: [
                    Kobold(KoboldServer { name: "gpu0", run: Start { binary: "bin/koboldcpp", port: 5001 },
                        chat: KoboldChat { model: "c.gguf" } }),
                    Chat(PeServer { name: "pe", run: KoboldCpp { binary: "koboldcpp", model: "p.gguf", host: "h", port: 1, context: 0 } }),
                ] }],
                sdserver: SdServer { binary: "sd-server", model_dirs: ["models", "", "models"] },
            }"#,
        )
        .unwrap();
        let w = c.normalize();
        assert!(w.iter().any(|w| w.contains(&format!("models → {}", abs("models")))), "{w:?}");
        assert_eq!(c.programs.sd_server.model_dirs, [abs("models")], "the empty and the repeated one dropped");
        assert_eq!(c.programs.sd_server.binary, "sd-server");
        let ServerConfig::Kobold(k) = &c.server_presets[0].servers[0] else { panic!() };
        let crate::kobold::KoboldRun::Start { binary, .. } = &k.run else { panic!() };
        assert_eq!(
            (binary.as_str(), k.chat.as_ref().unwrap().model.as_str()),
            (abs("bin/koboldcpp").as_str(), abs("c.gguf").as_str())
        );
        let ServerConfig::Chat(pe) = &c.server_presets[0].servers[1] else { panic!() };
        let crate::pe::PeRun::KoboldCpp { binary, model, .. } = &pe.run else { panic!() };
        assert_eq!((binary.as_str(), model.as_str()), ("koboldcpp", abs("p.gguf").as_str()));
        assert!(c.full_paths().is_empty(), "once");
    }

    #[test]
    fn roundtrip_and_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("increart/config.rsn");
        let (c, w) = Config::load(&path).unwrap();
        assert!(w.is_empty());
        assert_eq!(c.resolutions, builtin_resolutions());
        c.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("Config {\n    resolutions: [\n        Resolution {"), "{text}");
        assert_eq!(Config::load(&path).unwrap().0, c);
    }
}

/// The contact sheet's row headers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SheetRows {
    /// The style, its counts and buttons beside the row.
    #[default]
    Full,
    /// Full while the rows fit one per line; compact once they fold into
    /// bands side by side.
    Auto,
    /// The style as a caption above the row; its buttons in its menu.
    Compact,
}

impl SheetRows {
    pub fn name(self) -> &'static str {
        match self {
            SheetRows::Auto => "Auto",
            SheetRows::Full => "Full",
            SheetRows::Compact => "Compact",
        }
    }

    /// Full → Compact → Auto → Full.
    pub fn next(self) -> SheetRows {
        match self {
            SheetRows::Full => SheetRows::Compact,
            SheetRows::Compact => SheetRows::Auto,
            SheetRows::Auto => SheetRows::Full,
        }
    }
}

/// How the project tree orders projects or folders (a folder by the projects
/// anywhere in it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeOrder {
    /// Newest created first.
    #[default]
    Created,
    /// Last run (seeds queued) first; never run: by when created.
    LastRun,
    Name,
}

impl TreeOrder {
    pub const EVERY: [TreeOrder; 3] = [TreeOrder::Created, TreeOrder::LastRun, TreeOrder::Name];

    pub fn name(self) -> &'static str {
        match self {
            TreeOrder::Created => "Created",
            TreeOrder::LastRun => "Last run",
            TreeOrder::Name => "A–Z",
        }
    }
}

/// How Review's scope tree orders folders and projects: as the project tree
/// can (a folder by the projects anywhere in it), or oldest with images to
/// review first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewOrder {
    Created,
    #[default]
    LastRun,
    Name,
    /// The oldest projects with images to review first (a folder by the
    /// oldest such project in it), then those with none.
    OldestToReview,
}

impl ReviewOrder {
    pub const EVERY: [ReviewOrder; 4] =
        [ReviewOrder::Created, ReviewOrder::LastRun, ReviewOrder::Name, ReviewOrder::OldestToReview];

    pub fn name(self) -> &'static str {
        match self.tree() {
            Some(o) => o.name(),
            None => "Oldest to review",
        }
    }

    /// The project tree's order this is, if any.
    pub fn tree(self) -> Option<TreeOrder> {
        match self {
            ReviewOrder::Created => Some(TreeOrder::Created),
            ReviewOrder::LastRun => Some(TreeOrder::LastRun),
            ReviewOrder::Name => Some(TreeOrder::Name),
            ReviewOrder::OldestToReview => None,
        }
    }
}

/// How Review's **Follow** moves through the unreviewed images: each shown
/// for a while, then the next, up to the newest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "FollowPace", default)]
pub struct FollowPace {
    /// Each image as long as one takes to make (from the recent arrivals),
    /// sped up or slowed down to keep `lag` images ahead; else `fixed_secs`.
    pub auto: bool,
    /// Seconds each image, when not `auto` (and `auto`'s while nothing is
    /// being made).
    pub fixed_secs: f32,
    /// `auto`'s shortest and longest time an image.
    pub min_secs: f32,
    pub max_secs: f32,
    /// `auto`: how many images it keeps ahead of the one shown.
    pub lag: u32,
    /// Turned on, it starts at the newest image (else the one shown).
    pub start_newest: bool,
    /// No moving on while the pointer is over the image.
    pub pause_on_hover: bool,
}

impl Default for FollowPace {
    fn default() -> Self {
        FollowPace {
            auto: true,
            fixed_secs: 8.0,
            min_secs: 3.0,
            max_secs: 60.0,
            lag: 4,
            start_newest: false,
            pause_on_hover: false,
        }
    }
}

impl FollowPace {
    /// Seconds for the image shown, with `ahead` images after it and one
    /// image made every `interval` seconds lately (`None`: none lately).
    pub fn secs(&self, ahead: usize, interval: Option<f64>) -> f64 {
        let (min, max) = (self.min_secs.min(self.max_secs) as f64, self.max_secs.max(self.min_secs) as f64);
        match interval.filter(|_| self.auto) {
            // ahead of the lag: quicker, to catch up; short of it: slower
            Some(i) => (i * (self.lag as f64 + 1.0) / (ahead as f64 + 1.0)).clamp(min, max),
            None => self.fixed_secs.max(0.5) as f64,
        }
    }
}

/// The folders' [`TreeOrder`], A–Z by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FolderOrder(pub TreeOrder);

impl Default for FolderOrder {
    fn default() -> Self {
        FolderOrder(TreeOrder::Name)
    }
}

/// How the sweep's **Choose styles** window orders the presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StyleSort {
    /// As the presets list them (by task and group).
    #[default]
    Listed,
    Name,
    /// Most Good images first.
    MostGood,
    /// Last used first.
    Recent,
    /// Never used first, then least recently used.
    Untried,
}

impl StyleSort {
    pub const EVERY: [StyleSort; 5] =
        [StyleSort::Listed, StyleSort::Name, StyleSort::MostGood, StyleSort::Recent, StyleSort::Untried];

    pub fn name(self) -> &'static str {
        match self {
            StyleSort::Listed => "As listed",
            StyleSort::Name => "A–Z",
            StyleSort::MostGood => "Most Good",
            StyleSort::Recent => "Recently used",
            StyleSort::Untried => "Never used first",
        }
    }
}
