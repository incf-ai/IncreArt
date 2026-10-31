//! KoboldCpp presets by VRAM (docs/vram-tiers.md): the shipped `.kcppt`
//! templates (`kcppt/`, from the `tier_kcppt` example) are the
//! settings; the setup wizard picks one per GPU by its memory, plans the
//! servers and recipe from them, and downloads their model files.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::config::{RecipePreset, ServerConfig};
use crate::kobold::{KoboldChat, KoboldRun, KoboldServer};
use crate::pe::{GpuRuntime, PeDevice, PeRun, PeServer, PeStart};

/// (card size in GB, template file name, its text).
const TEMPLATES: [(u32, &str, &str); 5] = [
    (8, "Qwen-Image-2.1-8GB.kcppt", include_str!("../kcppt/Qwen-Image-2.1-8GB.kcppt")),
    (12, "Qwen-Image-2.1-12GB.kcppt", include_str!("../kcppt/Qwen-Image-2.1-12GB.kcppt")),
    (16, "Qwen-Image-2.1-16GB.kcppt", include_str!("../kcppt/Qwen-Image-2.1-16GB.kcppt")),
    (24, "Qwen-Image-2.1-24GB.kcppt", include_str!("../kcppt/Qwen-Image-2.1-24GB.kcppt")),
    (32, "Qwen-Image-2.1-32GB.kcppt", include_str!("../kcppt/Qwen-Image-2.1-32GB.kcppt")),
];

/// docs/vram-tiers.md's measurements per card size: seconds per image,
/// peak VRAM and pinned RAM (GiB).
const MEASURED: [(u32, u32, f64, f64); 5] =
    [(8, 364, 6.3, 12.0), (12, 208, 8.7, 10.0), (16, 210, 12.3, 13.5), (24, 215, 18.1, 13.5), (32, 209, 22.7, 13.5)];

/// KoboldCpp's own RAM per server, beyond the pinned weights (GiB).
const SERVER_RAM_GIB: f64 = 1.5;

/// Where the wizard downloads the files, under the model directory.
pub const MODEL_SUBDIR: &str = "koboldcpp";

/// The chat model's PE port, after the image servers' (5001, …).
const FIRST_PORT: u16 = 5001;

/// One tier: a template's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Tier {
    /// The card size it's for, in GB.
    pub gb: u32,
    pub file_name: &'static str,
    /// The template as shipped (for saving as a standalone `.kcppt`).
    pub text: &'static str,
    pub image_url: String,
    pub llm_url: String,
    pub clip_url: String,
    pub vae_url: String,
    pub chat_url: String,
    pub mmproj_url: String,
    /// The text encoder on the GPU (else the CPU).
    pub clip_on_gpu: bool,
    /// `sdvramlimit` in MB (0: none).
    pub vram_limit_mb: u32,
    pub context: u32,
    /// The chat model and the image model take turns.
    pub swap: bool,
    /// Measured (docs/vram-tiers.md): seconds per image, the card's peak
    /// VRAM rise and the pinned RAM, in GiB.
    pub per_image_s: u32,
    pub peak_vram_gib: f64,
    pub pinned_ram_gib: f64,
    offload: bool,
    flash_attention: bool,
    tiled_vae: u32,
}

/// `None` if the template isn't JSON or the tier wasn't measured (neither
/// happens: `the_templates_as_tiers` reads every one).
fn parse(gb: u32, file_name: &'static str, text: &'static str) -> Option<Tier> {
    let v: Value = serde_json::from_str(text).ok()?;
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let n = |k: &str| v[k].as_i64().unwrap_or(0);
    let (_, per_image_s, peak_vram_gib, pinned_ram_gib) = *MEASURED.iter().find(|m| m.0 == gb)?;
    Some(Tier {
        gb,
        file_name,
        text,
        image_url: s("sdmodel"),
        llm_url: s("sdllm"),
        clip_url: s("sdclip1"),
        vae_url: s("sdvae"),
        chat_url: s("model_param"),
        mmproj_url: s("mmproj"),
        clip_on_gpu: n("sdclipdevice") != -2,
        vram_limit_mb: n("sdvramlimit").max(0) as u32,
        context: n("contextsize").max(0) as u32,
        swap: v["autoswapmode"].as_bool().unwrap_or(false),
        per_image_s,
        peak_vram_gib,
        pinned_ram_gib,
        offload: v["sdoffloadcpu"].as_bool().unwrap_or(false),
        flash_attention: v["sdflashattention"].as_bool().unwrap_or(false),
        tiled_vae: n("sdtiledvae").max(0) as u32,
    })
}

/// Every tier, smallest card first.
pub fn tiers() -> Vec<Tier> {
    TEMPLATES.iter().filter_map(|(gb, f, t)| parse(*gb, f, t)).collect()
}

/// The biggest tier a card with `vram_gib` takes (cards report a little
/// under their size: a "32 GB" one 29.9–31.9 GiB); None under 8 GB.
pub fn tier_for(vram_gib: f64) -> Option<Tier> {
    tiers().into_iter().rev().find(|t| vram_gib >= needs_gib(t.gb))
}

/// The tier for a card size (8, 12, …).
pub fn tier(gb: u32) -> Option<Tier> {
    tiers().into_iter().find(|t| t.gb == gb)
}

/// A card takes a tier from this much memory: 90% of its size.
pub fn needs_gib(gb: u32) -> f64 {
    gb as f64 * 0.9
}

/// A memory size, as GPUs report it: "23.98", "31.9", "24".
pub fn show_gb(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The RAM available now, in GiB.
pub fn available_ram_gib() -> Option<f64> {
    ram_gib().map(|(_, free)| free)
}

/// The machine's RAM, in GiB.
pub fn total_ram_gib() -> Option<f64> {
    ram_gib().map(|(total, _)| total)
}

/// (total, available): `/proc/meminfo`'s `MemTotal` and `MemAvailable`.
#[cfg(target_os = "linux")]
fn ram_gib() -> Option<(f64, f64)> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = |key: &str| -> Option<f64> {
        text.lines().find_map(|l| l.strip_prefix(key))?.split_whitespace().next()?.parse().ok()
    };
    let gib = (1u64 << 20) as f64;
    Some((kb("MemTotal:")? / gib, kb("MemAvailable:")? / gib))
}

/// (total, available): `GlobalMemoryStatusEx`'s physical memory.
#[cfg(windows)]
fn ram_gib() -> Option<(f64, f64)> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return None;
    }
    let gib = (1u64 << 30) as f64;
    Some((m.ullTotalPhys as f64 / gib, m.ullAvailPhys as f64 / gib))
}

/// (total, available): `hw.memsize`, and what `vm_stat` counts as free.
#[cfg(target_os = "macos")]
fn ram_gib() -> Option<(f64, f64)> {
    let mut total = 0u64;
    let mut len = std::mem::size_of::<u64>();
    let r = unsafe {
        libc::sysctlbyname(c"hw.memsize".as_ptr(), (&raw mut total).cast(), &mut len, std::ptr::null_mut(), 0)
    };
    if r != 0 {
        return None;
    }
    let out = Command::new("vm_stat").stderr(Stdio::null()).output().ok()?;
    let free = parse_vm_stat(&String::from_utf8_lossy(&out.stdout))?;
    Some((total as f64 / (1u64 << 30) as f64, free))
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn ram_gib() -> Option<(f64, f64)> {
    None
}

/// `vm_stat`'s free, inactive and speculative pages (what macOS hands out
/// without swapping), in GiB.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_vm_stat(text: &str) -> Option<f64> {
    let page: f64 = text.split("page size of ").nth(1)?.split_whitespace().next()?.parse().ok()?;
    let pages = |key: &str| -> Option<f64> {
        text.lines().find_map(|l| l.strip_prefix(key))?.trim().trim_end_matches('.').parse().ok()
    };
    let n =
        pages("Pages free:")? + pages("Pages inactive:").unwrap_or(0.0) + pages("Pages speculative:").unwrap_or(0.0);
    Some(n * page / (1u64 << 30) as f64)
}

/// A URL's file name.
fn file_of(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

impl Tier {
    /// The image model's quantization, from its file name (`Q8_0`).
    pub fn quant(&self) -> &str {
        let f = file_of(&self.image_url).trim_end_matches(".gguf");
        f.rsplit('-').next().unwrap_or(f)
    }

    /// KoboldCpp's image flags for it (the models come with each load).
    pub fn image_args(&self) -> Vec<String> {
        let mut v = vec![];
        if self.flash_attention {
            v.push("--sdflashattention".to_string());
        }
        if self.offload {
            v.push("--sdoffloadcpu".into());
        }
        if self.tiled_vae > 0 {
            v.extend(["--sdtiledvae".into(), self.tiled_vae.to_string()]);
        }
        v.extend(["--sdvaedevice".into(), "main".into()]);
        v.extend(["--sdclipdevice".into(), if self.clip_on_gpu { "main" } else { "CPU" }.into()]);
        if self.vram_limit_mb > 0 {
            v.extend(["--sdvramlimit".into(), self.vram_limit_mb.to_string()]);
        }
        v
    }

    /// One line for the wizard: "Q8_0, prompt enhance alongside".
    pub fn describe(&self, with_chat: bool) -> String {
        let mut parts = vec![format!("Qwen-Image {}", self.quant())];
        if self.vram_limit_mb > 0 {
            parts.push(format!("capped at {} GB", self.vram_limit_mb / 1024));
        }
        if !self.clip_on_gpu {
            parts.push("text encoder on the CPU".into());
        }
        if with_chat {
            parts.push(if self.swap { "prompt enhance swapped in" } else { "prompt enhance alongside" }.into());
        }
        parts.join(", ")
    }
}

/// A GPU as Vulkan lists it: KoboldCpp's device number, name, and memory.
#[derive(Clone, Debug, PartialEq)]
pub struct Gpu {
    pub number: u32,
    pub name: String,
    pub vram_gib: f64,
    /// Shares the system's memory: not ticked by default.
    pub integrated: bool,
}

/// The GPUs and their memory as `runtime` numbers them, from its
/// [`tool`](GpuRuntime::tool).
pub fn detect_gpus(runtime: GpuRuntime) -> Result<Vec<Gpu>, String> {
    if runtime == GpuRuntime::Metal {
        return total_ram_gib().map(|ram| vec![apple_gpu(ram)]).ok_or_else(|| "the RAM couldn't be read".into());
    }
    let tool = runtime.tool();
    let mut cmd = Command::new(tool);
    let args: &[&str] = match runtime {
        GpuRuntime::Vulkan => &[],
        GpuRuntime::Cuda => &["--query-gpu=index,name,memory.total", "--format=csv,noheader,nounits"],
        GpuRuntime::Rocm => &["--showproductname", "--showmeminfo", "vram", "--json"],
        GpuRuntime::Metal => &[],
    };
    let out = crate::process::background(&mut cmd, false)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("{tool}: {e} (install it, or set the GPUs by hand)"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let list = match runtime {
        GpuRuntime::Vulkan => parse_vulkaninfo(&text),
        GpuRuntime::Cuda => parse_nvidia_smi(&text),
        GpuRuntime::Rocm => parse_rocm_smi(&text),
        GpuRuntime::Metal => vec![],
    };
    if list.is_empty() {
        return Err(format!("{tool} listed no GPU"));
    }
    Ok(list)
}

/// A Mac's GPU: the share of its `ram_gib` macOS lets the GPU keep (its
/// recommended working set: about 2/3 up to 36 GB, 3/4 above).
pub fn apple_gpu(ram_gib: f64) -> Gpu {
    let share = if ram_gib > 36.0 { 0.75 } else { 2.0 / 3.0 };
    Gpu {
        number: 0,
        name: format!("Apple GPU ({} GB shared)", show_gb(ram_gib.round())),
        vram_gib: ram_gib * share,
        integrated: false,
    }
}

/// `nvidia-smi --query-gpu=index,name,memory.total --format=csv,noheader,nounits`
/// (memory in MiB); CUDA numbers them the same in PCI order.
pub fn parse_nvidia_smi(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split(',').map(str::trim);
            let number = f.next()?.parse().ok()?;
            let name = f.next()?.to_string();
            let mib: f64 = f.next()?.parse().ok()?;
            Some(Gpu { number, name, vram_gib: mib / 1024.0, integrated: false })
        })
        .collect()
}

/// `rocm-smi --showproductname --showmeminfo vram --json`'s cards, numbered
/// as HIP does: in KFD node order.
pub fn parse_rocm_smi(text: &str) -> Vec<Gpu> {
    let Ok(serde_json::Value::Object(cards)) = serde_json::from_str(text) else { return vec![] };
    let field = |c: &serde_json::Value, k: &str| c.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut list: Vec<(u64, Gpu)> = cards
        .values()
        .filter_map(|c| {
            let node = field(c, "Node ID").parse().ok()?;
            let bytes: f64 = field(c, "VRAM Total Memory (B)").parse().ok()?;
            let name = field(c, "Card Series");
            Some((node, Gpu { number: 0, name, vram_gib: bytes / (1u64 << 30) as f64, integrated: false }))
        })
        .collect();
    list.sort_by_key(|(node, _)| *node);
    list.into_iter().enumerate().map(|(i, (_, g))| Gpu { number: i as u32, ..g }).collect()
}

/// `vulkaninfo`'s devices (`GPU1:`), each with its type, name and largest
/// device-local memory heap; CPU (software) ones left out.
pub fn parse_vulkaninfo(text: &str) -> Vec<Gpu> {
    let mut list: Vec<Gpu> = vec![];
    let mut cpu = false;
    let mut size = 0u64;
    for l in text.lines().map(str::trim) {
        if let Some(n) = l.strip_prefix("GPU").and_then(|r| r.strip_suffix(':')).and_then(|n| n.parse().ok()) {
            if cpu {
                list.pop();
            }
            list.push(Gpu { number: n, name: String::new(), vram_gib: 0.0, integrated: false });
            cpu = false;
            continue;
        }
        let Some(g) = list.last_mut() else { continue };
        if let Some((key, value)) = l.split_once('=') {
            let value = value.trim();
            match key.trim() {
                "deviceType" if g.name.is_empty() => {
                    cpu = value.contains("CPU");
                    g.integrated = value.contains("INTEGRATED");
                }
                "deviceName" if g.name.is_empty() => g.name = value.to_string(),
                "size" => size = value.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0),
                _ => {}
            }
        } else if l == "MEMORY_HEAP_DEVICE_LOCAL_BIT" {
            g.vram_gib = g.vram_gib.max(size as f64 / (1u64 << 30) as f64);
        }
    }
    if cpu {
        list.pop();
    }
    list
}

/// With more than one GPU: an image server on every card (the default:
/// the most images at once), or prompt enhance on a card of its own (no
/// swapping).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    PeOwnCard,
    #[default]
    ImagePerCard,
}

/// A model file to fetch: from `url` to `file` (under the model directory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    pub url: String,
    pub file: String,
}

/// A line of the wizard's plan.
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    /// What a card runs.
    Plan(String),
    /// Why: its preset by its memory, or the user's choice.
    Why(String),
    /// May not fit.
    Warn(String),
}

/// What the wizard sets up.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub servers: Vec<ServerConfig>,
    pub recipe: RecipePreset,
    pub downloads: Vec<Download>,
    /// What each GPU does, image cards first.
    pub cards: Vec<Card>,
    /// "2 images at a time; prompt enhance always loaded on GPU 1."
    pub summary: String,
    pub notes: Vec<Note>,
}

/// A GPU in a plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub gpu: u32,
    /// Its tier's card size; None: prompt enhance only.
    pub preset: Option<u32>,
    pub role: Role,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Images,
    /// Images and prompt enhance, swapping or both loaded.
    ImagesAndPe {
        swap: bool,
    },
    PeOnly,
}

impl Role {
    /// For the GPU's row: "Images + prompt enhance (stays loaded)".
    pub fn describe(self) -> &'static str {
        match self {
            Role::Images => "Images",
            Role::ImagesAndPe { swap: false } => "Images + prompt enhance (stays loaded)",
            Role::ImagesAndPe { swap: true } => "Images + prompt enhance (swaps, ~15 s per image)",
            Role::PeOnly => "Prompt enhance only, always loaded",
        }
    }

    /// For a layout's choice: "images + prompt enhance".
    pub fn short(self) -> &'static str {
        match self {
            Role::Images => "images",
            Role::ImagesAndPe { .. } => "images + prompt enhance",
            Role::PeOnly => "prompt enhance only",
        }
    }
}

impl Plan {
    /// How many images run at once.
    pub fn images_at_once(&self) -> usize {
        self.cards.iter().filter(|c| c.role != Role::PeOnly).count()
    }

    /// The cards running prompt enhance.
    pub fn pe_cards(&self) -> Vec<&Card> {
        self.cards.iter().filter(|c| c.role != Role::Images).collect()
    }
}

/// Plans the servers for the GPUs ticked (`gpus`: each with the memory to
/// plan for, and the tier the user chose, by card size; None: the biggest
/// it holds): an image server per card as its tier says, prompt enhance
/// beside (or swapped with) the image model on the biggest card, or on
/// the next one with [`Layout::PeOwnCard`]; or on `pe_gpus` (GPU numbers)
/// that are ticked, each a server prompt enhance shares its asks with.
/// One quantization for all, the smallest tier's. `models_dir`: the folder
/// the files go in (absolute); `ram_gib`: the RAM available, for the
/// pinned weights.
pub fn plan(
    gpus: &[(Gpu, Option<u32>)],
    layout: Layout,
    pe_gpus: &[u32],
    runtime: GpuRuntime,
    binary: &str,
    models_dir: &Path,
    ram_gib: Option<f64>,
) -> Result<Plan, String> {
    if gpus.is_empty() {
        return Err("no GPU ticked".into());
    }
    let mut gpus = gpus.to_vec();
    gpus.sort_by(|a, b| b.0.vram_gib.total_cmp(&a.0.vram_gib).then(a.0.number.cmp(&b.0.number)));
    let own = layout == Layout::PeOwnCard && gpus.len() > 1;
    let mut pe_nums: Vec<u32> = gpus.iter().map(|(g, _)| g.number).filter(|n| pe_gpus.contains(n)).collect();
    if pe_nums.is_empty() {
        pe_nums.push(gpus[own as usize].0.number);
    }
    if own && pe_nums.len() >= gpus.len() {
        return Err("Leave a GPU for images, or make images on every GPU.".into());
    }
    let mut pe_cards: Vec<Gpu> = vec![];
    if own {
        pe_cards = gpus.iter().filter(|(g, _)| pe_nums.contains(&g.number)).map(|(g, _)| g.clone()).collect();
        gpus.retain(|(g, _)| !pe_nums.contains(&g.number));
    }
    // (images and prompt enhance on one card: their numbers)
    let pe_with: Vec<u32> = if own { vec![] } else { pe_nums };
    if let Some(g) = pe_cards.iter().find(|g| g.vram_gib < needs_gib(8)) {
        return Err(format!("GPU {} ({:.0} GB) has less than the 8 GB prompt enhance needs", g.number, g.vram_gib));
    }
    let mut notes = vec![];
    let mut cards: Vec<(Gpu, Tier)> = vec![];
    for (g, chosen) in &gpus {
        let with_chat = pe_with.contains(&g.number);
        let auto = tier_for(g.vram_gib);
        let mut t = match chosen.and_then(tier) {
            Some(t) => t,
            None => auto.clone().ok_or_else(|| {
                format!("GPU {} ({:.0} GB) has less than the 8 GB the smallest preset needs", g.number, g.vram_gib)
            })?,
        };
        if runtime == GpuRuntime::Metal {
            // the GPU's memory is the RAM: offloading only copies the weights
            t.offload = false;
            t.pinned_ram_gib = 0.0;
        }
        notes.push(Note::Plan(format!("GPU {} ({} GB preset): {}", g.number, t.gb, t.describe(with_chat))));
        notes.push(Note::Why(why(g, &t, auto.as_ref(), chosen.is_some(), with_chat)));
        if auto.as_ref().is_none_or(|a| t.gb > a.gb) {
            notes.push(Note::Warn(if t.peak_vram_gib > g.vram_gib {
                format!(
                    "GPU {}: the {} GB preset peaks at {:.1} GiB, more than its {} GB: expect out-of-memory errors or slow images.",
                    g.number, t.gb, t.peak_vram_gib, show_gb(g.vram_gib)
                )
            } else {
                format!(
                    "GPU {}: the {} GB preset peaks at {:.1} GiB of its {} GB, leaving little for other programs.",
                    g.number, t.gb, t.peak_vram_gib, show_gb(g.vram_gib)
                )
            }));
        }
        cards.push((g.clone(), t));
    }
    // the image model must be the same everywhere: the smallest tier's
    let Some(quant_tier) = cards.iter().map(|(_, t)| t).min_by_key(|t| t.gb).cloned() else {
        return Err("no GPU to plan for".into());
    };
    // the chat model's settings: the tier of the card it shares
    let first = &cards.iter().find(|(g, _)| pe_with.contains(&g.number)).unwrap_or(&cards[0]).1.clone();
    let local = |url: &str| format!("{MODEL_SUBDIR}/{}", file_of(url));
    let path = |url: &str| models_dir.join(local(url)).to_string_lossy().into_owned();
    let chat = KoboldChat { model: path(&first.chat_url), mmproj: path(&first.mmproj_url), context: first.context };
    let mut servers = vec![];
    let mut port = FIRST_PORT;
    let mut roles = vec![];
    for (g, t) in &cards {
        let with_chat = pe_with.contains(&g.number);
        let role = if with_chat { Role::ImagesAndPe { swap: t.swap } } else { Role::Images };
        roles.push(Card { gpu: g.number, preset: Some(t.gb), role });
        servers.push(ServerConfig::Kobold(KoboldServer {
            name: format!("kcpp-gpu{}", g.number),
            run: KoboldRun::Start {
                binary: binary.to_string(),
                config: String::new(),
                host: "127.0.0.1".into(),
                port,
                device: PeDevice::Gpu { devices: vec![g.number], runtime },
                args: t.image_args(),
            },
            chat: with_chat.then(|| chat.clone()),
            swap: with_chat && t.swap,
        }));
        port += 1;
    }
    for g in &pe_cards {
        servers.push(ServerConfig::Chat(PeServer {
            name: format!("pe-gpu{}", g.number),
            run: PeRun::KoboldCpp {
                binary: binary.to_string(),
                model: chat.model.clone(),
                mmproj: chat.mmproj.clone(),
                config: String::new(),
                host: "127.0.0.1".into(),
                port,
                device: PeDevice::Gpu { devices: vec![g.number], runtime },
                context: 8192,
                args: vec![],
            },
            start: PeStart::AtLaunch,
        }));
        roles.push(Card { gpu: g.number, preset: None, role: Role::PeOnly });
        notes.push(Note::Plan(format!("GPU {}: prompt enhance (Qwen3-VL 8B), always loaded", g.number)));
        port += 1;
    }
    // a swapping one isn't asked while one keeps the chat model loaded
    let kept = roles.iter().find(|c| matches!(c.role, Role::ImagesAndPe { swap: false } | Role::PeOnly));
    if let Some(kept) = kept {
        for c in roles.iter().filter(|c| c.role == Role::ImagesAndPe { swap: true }) {
            notes.push(Note::Warn(format!(
                "GPU {}: its preset swaps, so prompt enhance leaves it out while GPU {} keeps the chat model loaded.",
                c.gpu, kept.gpu
            )));
        }
    }
    if cards.iter().any(|(_, t)| t.quant() != quant_tier.quant()) {
        notes.push(Note::Why(format!(
            "All cards use {}, the smallest preset's, so one recipe runs on each.",
            quant_tier.quant()
        )));
    }
    let ram = cards.iter().map(|(_, t)| t.pinned_ram_gib).sum::<f64>() + SERVER_RAM_GIB * servers.len() as f64;
    notes.extend((runtime != GpuRuntime::Metal).then(|| match ram_gib {
        Some(free) if ram > free => Note::Warn(format!(
            "Needs about {ram:.0} GiB of RAM for the offloaded weights; {free:.0} GiB is available now."
        )),
        Some(free) => Note::Why(format!(
            "Needs about {ram:.0} GiB of RAM for the offloaded weights; {free:.0} GiB is available now."
        )),
        None => Note::Why(format!("Needs about {ram:.0} GiB of RAM for the offloaded weights.")),
    }));
    let files = [
        ("diffusion_model", &quant_tier.image_url),
        ("llm", &first.llm_url),
        ("clip_l", &first.clip_url),
        ("vae", &first.vae_url),
    ];
    let recipe = RecipePreset {
        name: format!("Qwen-Image 2.1 GGUF {} (CFG 2, refs)", quant_tier.quant()),
        recipe: "qwen-image-21".into(),
        files: files.iter().map(|(slot, url)| (slot.to_string(), local(url))).collect(),
        steps: 20,
        cfg: 2.0,
        flow_shift: None,
        custom_sigmas: vec![],
        loras: vec![],
    };
    let mut downloads: Vec<Download> = vec![];
    let urls =
        [&quant_tier.image_url, &first.llm_url, &first.clip_url, &first.vae_url, &first.chat_url, &first.mmproj_url];
    for url in urls {
        if !downloads.iter().any(|d| d.url == *url) {
            downloads.push(Download { url: url.clone(), file: local(url) });
        }
    }
    if runtime.missing_from(binary) {
        notes.push(Note::Warn(format!(
            "This KoboldCpp build has Vulkan only: get its {} build, or use Vulkan.",
            runtime.label()
        )));
    }
    if runtime != GpuRuntime::Vulkan {
        notes.push(Note::Why(format!(
            "Times and memory were measured on Linux with Vulkan; {} may differ.",
            runtime.label()
        )));
    }
    let summary = summary(&roles);
    Ok(Plan { servers, recipe, downloads, cards: roles, summary, notes })
}

/// "2 images at a time; prompt enhance always loaded on GPU 1 and GPU 2."
fn summary(cards: &[Card]) -> String {
    let n = cards.iter().filter(|c| c.role != Role::PeOnly).count();
    let images = if n == 1 { "1 image at a time".to_string() } else { format!("{n} images at a time") };
    let on = |swap: bool| -> Vec<String> {
        cards
            .iter()
            .filter(|c| c.role == Role::ImagesAndPe { swap } || !swap && c.role == Role::PeOnly)
            .map(|c| format!("GPU {}", c.gpu))
            .collect()
    };
    let list = |v: Vec<String>| match v.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => v.join(""),
    };
    let (kept, swapping) = (on(false), on(true));
    if !kept.is_empty() {
        format!("{images}; prompt enhance always loaded on {}.", list(kept))
    } else if !swapping.is_empty() {
        format!("{images}; prompt enhance swaps with images on {}.", list(swapping))
    } else {
        format!("{images}.")
    }
}

/// Why a card has tier `t`: the biggest its memory holds (`auto`), or the
/// user's choice and how it differs.
fn why(g: &Gpu, t: &Tier, auto: Option<&Tier>, chosen: bool, with_chat: bool) -> String {
    let n = g.number;
    match auto {
        Some(a) if !chosen || a.gb == t.gb => {
            let bigger = tiers().into_iter().find(|b| b.gb > a.gb);
            let next = match bigger {
                Some(b) => format!("; the {} GB preset needs {:.1} GB", b.gb, needs_gib(b.gb)),
                None => ", the biggest preset".into(),
            };
            let mine = if chosen { " (your choice too)" } else { "" };
            format!(
                "GPU {n}: {} GB is at least 90% of {} GB ({:.1}){next}{mine}.",
                show_gb(g.vram_gib),
                a.gb,
                needs_gib(a.gb)
            )
        }
        Some(a) if t.gb < a.gb => {
            let mut s = format!(
                "GPU {n}: your choice; its memory takes the {} GB preset. Leaves VRAM for other programs",
                a.gb
            );
            if with_chat && t.swap && !a.swap {
                s += ", but prompt enhance swaps with the image model (about 15 s more per image)";
            }
            s + "."
        }
        Some(a) => format!("GPU {n}: your choice; its memory takes the {} GB preset.", a.gb),
        None => format!("GPU {n}: your choice; its {} GB is under every preset.", show_gb(g.vram_gib)),
    }
}

/// The chat model and its projector the wizard downloads, if they're in
/// one of `model_dirs` (under `koboldcpp/`).
pub fn chat_files(model_dirs: &[String]) -> Option<(String, String)> {
    let t = tiers().into_iter().next()?;
    model_dirs.iter().find_map(|d| {
        let at = |url: &str| Path::new(d).join(MODEL_SUBDIR).join(file_of(url));
        let (model, mmproj) = (at(&t.chat_url), at(&t.mmproj_url));
        (model.exists() && mmproj.exists())
            .then(|| (model.to_string_lossy().into_owned(), mmproj.to_string_lossy().into_owned()))
    })
}

/// Where model files may already be: the model folders, and Hugging
/// Face's (`$HF_HOME`, else `~/.cache/huggingface`).
pub fn search_dirs(model_dirs: &[String]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = model_dirs.iter().filter(|d| !d.trim().is_empty()).map(PathBuf::from).collect();
    let hf = std::env::var_os("HF_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/huggingface")));
    dirs.extend(hf);
    dirs
}

/// Files named `name` under `dirs`, a few folders down (Hugging Face's
/// `hub/models--…/snapshots/…/` is four); folder links not followed.
pub fn find_named(name: &str, dirs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, name: &str, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(list) = std::fs::read_dir(dir) else { return };
        for e in list.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            let p = e.path();
            if kind.is_dir() && depth > 0 && !matches!(e.file_name().to_str(), Some("blobs" | "xet" | ".git")) {
                walk(&p, name, depth - 1, out);
            } else if !kind.is_dir() && e.file_name() == name && p.is_file() {
                out.push(p);
            }
        }
    }
    let mut out = vec![];
    for d in dirs {
        walk(d, name, 5, &mut out);
    }
    out
}

/// The size Hugging Face gives for `url` (following its redirect).
pub fn remote_size(url: &str) -> Option<u64> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(20))).build().into();
    let resp = agent.head(url).call().ok()?;
    resp.headers().get("content-length")?.to_str().ok()?.parse().ok()
}

/// A file already on disk for `url`: same name and, if `size` is known,
/// the same size.
pub fn reusable(url: &str, dirs: &[PathBuf], size: impl FnOnce() -> Option<u64>) -> Option<PathBuf> {
    let found = find_named(file_of(url), dirs);
    if found.is_empty() {
        return None;
    }
    let size = size();
    found.into_iter().find(|p| size.is_none_or(|s| std::fs::metadata(p).is_ok_and(|m| m.len() == s)))
}

/// `dest` made a link to `src` (its real path).
pub fn link(src: &Path, dest: &Path) -> Result<(), String> {
    let src = src.canonicalize().map_err(|e| format!("{}: {e}", src.display()))?;
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    #[cfg(unix)]
    let r = std::os::unix::fs::symlink(&src, dest);
    #[cfg(windows)]
    let r = std::os::windows::fs::symlink_file(&src, dest).or_else(|_| std::fs::hard_link(&src, dest));
    r.map_err(|e| format!("{}: {e}", dest.display()))
}

/// Fetches `url` to `dest` (through `dest.part`, so a broken download is
/// never taken for the file); one already there is kept. A `.part` left
/// by a broken one is carried on from where it stopped (when the server
/// takes a `Range`). Given up after the config's download timeout without
/// data, never for being slow. `progress` gets (bytes so far, total if
/// known); `cancel` stops it (the `.part` kept, to carry on later).
pub fn download(
    url: &str,
    dest: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    if dest.is_file() {
        return Ok(());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let stall = crate::config::timeouts().download();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        .timeout_per_read(Some(stall))
        .http_status_as_error(false)
        .build()
        .into();
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let had = std::fs::metadata(&part).map_or(0, |m| m.len());
    let mut rq = agent.get(url);
    if had > 0 {
        rq = rq.header("Range", format!("bytes={had}-"));
    }
    let mut resp = rq.call().map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status().as_u16();
    let length: Option<u64> =
        resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
    // carried on (206), or from the start (200: no ranges here)
    let (mut out, mut done, total) = match status {
        206 if had > 0 => {
            let out = std::fs::OpenOptions::new().append(true).open(&part);
            (out.map_err(|e| format!("{}: {e}", part.display()))?, had, length.map(|n| n + had))
        }
        200..=299 => (std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?, 0, length),
        // the .part is all of it already
        416 if had > 0 => {
            return std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()));
        }
        _ => return Err(format!("{url}: HTTP {status}")),
    };
    progress(done, total);
    let mut body = resp.body_mut().with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let n = body.read(&mut buf).map_err(|e| match e.kind() {
            std::io::ErrorKind::TimedOut => {
                format!("{url}: no data for {} s (download timeout; the rest is fetched next time)", stall.as_secs())
            }
            _ => format!("{url}: {e} (the rest is fetched next time)"),
        })?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", part.display()))?;
        done += n as u64;
        progress(done, total);
    }
    if let Some(t) = total.filter(|t| *t != done) {
        return Err(format!("{url}: got {done} of {t} bytes"));
    }
    out.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_templates_as_tiers() {
        let t = tiers();
        assert_eq!(t.iter().map(|t| t.gb).collect::<Vec<_>>(), [8, 12, 16, 24, 32]);
        let small = &t[0];
        assert_eq!((small.quant(), small.clip_on_gpu, small.vram_limit_mb, small.swap), ("Q4_K", false, 4096, true));
        assert_eq!(
            small.image_args().join(" "),
            "--sdflashattention --sdoffloadcpu --sdtiledvae 512 --sdvaedevice main --sdclipdevice CPU --sdvramlimit 4096"
        );
        let big = &t[4];
        assert_eq!((big.quant(), big.clip_on_gpu, big.vram_limit_mb, big.swap), ("Q8_0", true, 0, false));
        assert!(!big.image_args().contains(&"--sdvramlimit".to_string()));
        assert!(t.iter().all(|t| t.chat_url.ends_with("Qwen3VL-8B-Instruct-Q4_K_M.gguf") && t.chat_url == t.llm_url));
        assert!(t.iter().all(|t| serde_json::from_str::<Value>(t.text).unwrap()["launch"] == false));
    }

    #[test]
    fn a_card_takes_the_biggest_tier_it_holds() {
        let gb = |v: f64| tier_for(v).map(|t| t.gb);
        assert_eq!(
            [gb(31.86), gb(29.86), gb(23.9), gb(15.9), gb(11.9), gb(7.98), gb(6.0)],
            [Some(32), Some(32), Some(24), Some(16), Some(12), Some(8), None]
        );
    }

    #[test]
    fn gpus_and_their_memory_from_nvidia_smi_and_rocm_smi() {
        let nv = parse_nvidia_smi("0, NVIDIA GeForce RTX 4090, 24564\n1, NVIDIA RTX A4000, 16376\n");
        let nv: Vec<_> = nv.iter().map(|g| (g.number, g.name.as_str(), g.vram_gib.round())).collect();
        assert_eq!(nv, [(0, "NVIDIA GeForce RTX 4090", 24.0), (1, "NVIDIA RTX A4000", 16.0)]);
        // (this machine's two R9700s; HIP numbers them by node)
        let rocm = parse_rocm_smi(
            r#"{"card1": {"VRAM Total Memory (B)": "34208743424", "Card Series": "AMD Radeon AI PRO R9700", "Node ID": "2"},
                "card0": {"VRAM Total Memory (B)": "32061259776", "Card Series": "AMD Radeon AI PRO R9700", "Node ID": "1"}}"#,
        );
        let rocm: Vec<_> = rocm.iter().map(|g| (g.number, g.vram_gib.round())).collect();
        assert_eq!(rocm, [(0, 30.0), (1, 32.0)]);
        assert!(parse_rocm_smi("not json").is_empty());
    }

    #[test]
    fn gpus_and_their_memory_from_vulkaninfo() {
        let text = "Device Properties and Extensions:\n=================\nGPU0:\nVkPhysicalDeviceProperties:\n\tdeviceType        = PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU\n\tdeviceName        = Intel(R) Graphics\nVkPhysicalDeviceMemoryProperties:\n\tmemoryHeaps[0]:\n\t\tsize   = 101175293952 (0x178e847800) (94.23 GiB)\n\t\tflags:\n\t\t\tMEMORY_HEAP_DEVICE_LOCAL_BIT\nGPU1:\n\tdeviceType        = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU\n\tdeviceName        = AMD Radeon AI PRO R9700 (RADV GFX1201)\n\tmemoryHeaps[0]:\n\t\tsize   = 101175291904 (0x178e847000) (94.23 GiB)\n\t\tflags:\n\t\t\tNone\n\tmemoryHeaps[1]:\n\t\tsize   = 34208743424 (0x7f7000000) (31.86 GiB)\n\t\tflags:\n\t\t\tMEMORY_HEAP_DEVICE_LOCAL_BIT\nGPU2:\n\tdeviceType        = PHYSICAL_DEVICE_TYPE_CPU\n\tdeviceName        = llvmpipe\n\tmemoryHeaps[0]:\n\t\tsize   = 1000 (0x0)\n\t\tflags:\n\t\t\tMEMORY_HEAP_DEVICE_LOCAL_BIT\n";
        let g = parse_vulkaninfo(text);
        assert_eq!(g.len(), 2, "{g:?}");
        assert_eq!((g[0].number, g[0].integrated), (0, true));
        assert_eq!(
            (g[1].number, g[1].name.as_str(), g[1].integrated),
            (1, "AMD Radeon AI PRO R9700 (RADV GFX1201)", false)
        );
        assert!((g[1].vram_gib - 31.86).abs() < 0.01);
    }

    fn gpu(number: u32, vram_gib: f64) -> Gpu {
        Gpu { number, name: format!("gpu{number}"), vram_gib, integrated: false }
    }

    /// Each card's preset by its memory.
    fn plan_auto(gpus: &[Gpu], layout: Layout) -> Result<Plan, String> {
        let gpus: Vec<_> = gpus.iter().map(|g| (g.clone(), None)).collect();
        plan(&gpus, layout, &[], GpuRuntime::Vulkan, "kcpp", Path::new("/m"), None)
    }

    fn notes(p: &Plan) -> String {
        p.notes.iter().map(|n| format!("{n:?}\n")).collect()
    }

    #[test]
    fn one_card_runs_both_as_its_tier_says() {
        let p = plan_auto(&[gpu(1, 11.9)], Layout::PeOwnCard).unwrap();
        let [ServerConfig::Kobold(k)] = &p.servers[..] else { panic!("{:?}", p.servers) };
        assert!(k.swap);
        let chat = Path::new("/m").join("koboldcpp/Qwen3VL-8B-Instruct-Q4_K_M.gguf");
        assert_eq!(k.chat.as_ref().unwrap().model, chat.to_string_lossy());
        let KoboldRun::Start { port, device, args, .. } = &k.run else { panic!() };
        assert_eq!((*port, device), (5001, &PeDevice::Gpu { devices: vec![1], runtime: GpuRuntime::Vulkan }));
        assert!(args.join(" ").ends_with("--sdvramlimit 8192"));
        assert_eq!(p.recipe.files["diffusion_model"], "koboldcpp/qwen_image_2.1-Q4_K.gguf");
        assert_eq!(p.recipe.files["clip_l"], "koboldcpp/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf");
        // the chat model is the text encoder, its projector the vision one: fetched once
        assert_eq!(p.downloads.len(), 4);
        let big = plan_auto(&[gpu(1, 31.9)], Layout::PeOwnCard).unwrap();
        let [ServerConfig::Kobold(k)] = &big.servers[..] else { panic!() };
        assert!(!k.swap && k.chat.is_some());
        assert!(plan_auto(&[gpu(1, 6.0)], Layout::PeOwnCard).is_err());
        assert!(plan_auto(&[], Layout::PeOwnCard).is_err());
    }

    #[test]
    fn two_cards_give_prompt_enhance_its_own_or_both_make_images() {
        let gpus = [gpu(2, 15.9), gpu(1, 31.9)];
        let own = plan_auto(&gpus, Layout::PeOwnCard).unwrap();
        let [ServerConfig::Kobold(k), ServerConfig::Chat(c)] = &own.servers[..] else { panic!("{:?}", own.servers) };
        assert!(k.chat.is_none() && !k.swap);
        assert_eq!(k.device(), "Vulkan1", "images on the biggest card");
        let PeRun::KoboldCpp { device, port, .. } = &c.run else { panic!() };
        assert_eq!((device, *port), (&PeDevice::Gpu { devices: vec![2], runtime: GpuRuntime::Vulkan }, 5002));
        assert_eq!(c.start, PeStart::AtLaunch);
        // the 16 GB card's tier is Q8_0 too
        assert_eq!(own.recipe.files["diffusion_model"], "koboldcpp/qwen_image_2.1-Q8_0.gguf");
        let each = plan_auto(&gpus, Layout::ImagePerCard).unwrap();
        let [ServerConfig::Kobold(a), ServerConfig::Kobold(b)] = &each.servers[..] else { panic!() };
        assert!(a.chat.is_some() && !a.swap && b.chat.is_none());
        // a 12 GB card: Q4_K everywhere
        let mixed = plan_auto(&[gpu(1, 31.9), gpu(2, 11.9)], Layout::ImagePerCard).unwrap();
        assert_eq!(mixed.recipe.files["diffusion_model"], "koboldcpp/qwen_image_2.1-Q4_K.gguf");
        assert!(notes(&mixed).contains("All cards use Q4_K"), "{}", notes(&mixed));
        let presets = |p: &Plan| p.cards.iter().map(|c| (c.gpu, c.preset)).collect::<Vec<_>>();
        assert_eq!(presets(&mixed), [(1, Some(32)), (2, Some(12))]);
        // prompt enhance's own card takes no preset, nor sets the quantization
        let pe = plan_auto(&[gpu(1, 31.9), gpu(2, 11.9)], Layout::PeOwnCard).unwrap();
        assert_eq!(presets(&pe), [(1, Some(32)), (2, None)]);
        assert_eq!(pe.summary, "1 image at a time; prompt enhance always loaded on GPU 2.");
        assert_eq!(pe.recipe.files["diffusion_model"], "koboldcpp/qwen_image_2.1-Q8_0.gguf");
        assert!(plan_auto(&[gpu(1, 31.9), gpu(2, 6.0)], Layout::PeOwnCard).unwrap_err().contains("prompt enhance"));
    }

    #[test]
    fn two_equal_cards_two_images_and_prompt_enhance_where_asked() {
        let gpus = [(gpu(1, 31.86), None), (gpu(2, 31.86), None)];
        let at = |layout, pe: &[u32]| plan(&gpus, layout, pe, GpuRuntime::Vulkan, "k", Path::new("/m"), None).unwrap();
        let both = at(Layout::ImagePerCard, &[]);
        assert_eq!(both.summary, "2 images at a time; prompt enhance always loaded on GPU 1.");
        assert_eq!(both.cards[0].role, Role::ImagesAndPe { swap: false });
        assert_eq!((both.images_at_once(), both.cards[1].role), (2, Role::Images));
        // on GPU 2: its server has the chat model, and keeps its port order
        let second = at(Layout::ImagePerCard, &[2]);
        let [ServerConfig::Kobold(a), ServerConfig::Kobold(b)] = &second.servers[..] else { panic!() };
        assert!(a.chat.is_none() && b.chat.is_some() && !b.swap);
        assert_eq!(second.summary, "2 images at a time; prompt enhance always loaded on GPU 2.");
        // its own card: the one asked for, else the second
        assert_eq!(at(Layout::PeOwnCard, &[]).pe_cards()[0].gpu, 2);
        assert_eq!(at(Layout::PeOwnCard, &[1]).pe_cards()[0].gpu, 1);
        // on both: two chat servers, both kept loaded
        let both_pe = at(Layout::ImagePerCard, &[1, 2]);
        assert_eq!(both_pe.summary, "2 images at a time; prompt enhance always loaded on GPU 1 and GPU 2.");
        let chats = both_pe.servers.iter().filter(|s| matches!(s, ServerConfig::Kobold(k) if k.chat.is_some()));
        assert_eq!(chats.count(), 2);
        let rocm =
            plan(&gpus, Layout::ImagePerCard, &[], GpuRuntime::Rocm, "/k/koboldcpp-nocuda", Path::new("/m"), None);
        let rocm = rocm.unwrap();
        assert!(notes(&rocm).contains("Vulkan only"), "{}", notes(&rocm));
        let devices: Vec<String> = rocm
            .servers
            .iter()
            .map(|s| match s {
                ServerConfig::Kobold(k) => k.device(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(devices, ["ROCm1", "ROCm2"]);
        assert!(
            plan(&gpus, Layout::PeOwnCard, &[1, 2], GpuRuntime::Vulkan, "k", Path::new("/m"), None)
                .unwrap_err()
                .contains("Leave a GPU")
        );
        // three cards, two of them prompt enhance only
        let three = [(gpu(1, 31.9), None), (gpu(2, 15.9), None), (gpu(3, 15.9), None)];
        let own = plan(&three, Layout::PeOwnCard, &[2, 3], GpuRuntime::Vulkan, "k", Path::new("/m"), None).unwrap();
        assert_eq!(own.summary, "1 image at a time; prompt enhance always loaded on GPU 2 and GPU 3.");
        let ports: Vec<_> = own.servers.iter().map(|s| s.base_url()).collect();
        assert_eq!(ports, ["http://127.0.0.1:5001", "http://127.0.0.1:5002", "http://127.0.0.1:5003"]);
        // a swapping one beside one that isn't: warned it's left out
        let mixed =
            plan(&three, Layout::ImagePerCard, &[1, 2], GpuRuntime::Vulkan, "k", Path::new("/m"), None).unwrap();
        assert!(
            notes(&mixed).contains("GPU 2: its preset swaps, so prompt enhance leaves it out"),
            "{}",
            notes(&mixed)
        );
        assert_eq!(mixed.summary, "3 images at a time; prompt enhance always loaded on GPU 1.");
        // a swapping tier says so
        let small =
            plan(&[(gpu(1, 11.9), None)], Layout::PeOwnCard, &[9], GpuRuntime::Vulkan, "k", Path::new("/m"), None)
                .unwrap();
        assert_eq!(small.summary, "1 image at a time; prompt enhance swaps with images on GPU 1.");
    }

    #[test]
    fn a_mac_shares_its_ram_and_never_offloads() {
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                    Pages free:                               65536.\n\
                    Pages active:                            900000.\n\
                    Pages inactive:                          131072.\n\
                    Pages speculative:                        65536.\n";
        assert_eq!(parse_vm_stat(text), Some(4.0));
        assert_eq!(apple_gpu(32.0).vram_gib.round(), 21.0);
        assert_eq!(apple_gpu(64.0).vram_gib, 48.0);
        let mac =
            plan(&[(apple_gpu(64.0), None)], Layout::ImagePerCard, &[], GpuRuntime::Metal, "k", Path::new("/m"), None)
                .unwrap();
        let ServerConfig::Kobold(k) = &mac.servers[0] else { panic!() };
        let KoboldRun::Start { args, .. } = &k.run else { panic!() };
        assert!(!args.contains(&"--sdoffloadcpu".to_string()), "{args:?}");
        assert!(!notes(&mac).contains("offloaded weights"), "{}", notes(&mac));
    }

    #[test]
    fn why_each_preset_and_a_chosen_one_warned_not_refused() {
        let auto = plan_auto(&[gpu(1, 23.98)], Layout::PeOwnCard).unwrap();
        let why = notes(&auto);
        assert!(why.contains("23.98 GB is at least 90% of 24 GB (21.6); the 32 GB preset needs 28.8 GB"), "{why}");
        assert!(!why.contains("Warn"), "{why}");
        let top = notes(&plan_auto(&[gpu(1, 31.9)], Layout::PeOwnCard).unwrap());
        assert!(top.contains("the biggest preset"), "{top}");

        let chose = |vram, gb, ram| {
            plan(&[(gpu(1, vram), Some(gb))], Layout::PeOwnCard, &[], GpuRuntime::Vulkan, "k", Path::new("/m"), ram)
        };
        // smaller: said, and that prompt enhance now swaps
        let small = chose(23.98, 16, Some(100.0)).unwrap();
        let [ServerConfig::Kobold(k)] = &small.servers[..] else { panic!() };
        assert!(k.swap);
        assert!(notes(&small).contains("your choice; its memory takes the 24 GB preset. Leaves VRAM"));
        assert!(notes(&small).contains("swaps"));
        // bigger: warned, still planned; even under 8 GB
        let big = chose(15.6, 32, None).unwrap();
        assert!(notes(&big).contains("Warn(\"GPU 1: the 32 GB preset peaks at 22.7 GiB, more than its 15.6 GB"));
        let tiny = chose(6.0, 8, None).unwrap();
        assert!(notes(&tiny).contains("under every preset") && notes(&tiny).contains("Warn("));
        // the pinned weights against the RAM
        assert!(notes(&chose(31.9, 32, Some(100.0)).unwrap()).contains("Why(\"Needs about 15 GiB of RAM"));
        assert!(notes(&chose(31.9, 32, Some(8.0)).unwrap()).contains("Warn(\"Needs about 15 GiB of RAM"));
    }

    #[test]
    fn a_download_lands_whole_or_not_at_all() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f.gguf", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            for req in server.incoming_requests().take(1) {
                let _ = req.respond(tiny_http::Response::from_data(vec![7u8; 3 << 20]));
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("koboldcpp/f.gguf");
        let mut seen = vec![];
        download(&url, &dest, &AtomicBool::new(false), |d, t| seen.push((d, t))).unwrap();
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), 3 << 20);
        // (tiny_http sends it chunked: no length)
        assert_eq!(seen.last().map(|s| s.0), Some(3 << 20));
        assert!(!dir.path().join("koboldcpp/f.gguf.part").exists());
        // there already: not fetched again (the server is gone)
        download(&url, &dest, &AtomicBool::new(false), |_, _| panic!("fetched")).unwrap();
    }

    #[test]
    fn a_broken_download_carries_on_where_it_stopped() {
        let whole: Vec<u8> = (0..(3u32 << 20)).map(|i| (i % 251) as u8).collect();
        let served = whole.clone();
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f.gguf", server.server_addr().to_ip().unwrap());
        // the first asks for the rest (206); the second server knows no ranges (200)
        std::thread::spawn(move || {
            for (i, req) in server.incoming_requests().take(2).enumerate() {
                let range = req.headers().iter().find(|h| h.field.equiv("Range")).map(|h| h.value.to_string());
                let from: usize = range
                    .as_deref()
                    .and_then(|r| r.strip_prefix("bytes=")?.strip_suffix('-')?.parse().ok())
                    .unwrap_or(0);
                let r = if i == 0 && from > 0 {
                    tiny_http::Response::from_data(served[from..].to_vec()).with_status_code(206)
                } else {
                    tiny_http::Response::from_data(served.clone())
                };
                let _ = req.respond(r);
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f.gguf");
        let part = dir.path().join("f.gguf.part");
        std::fs::write(&part, &whole[..1 << 20]).unwrap();
        let mut seen = vec![];
        download(&url, &dest, &AtomicBool::new(false), |d, _| seen.push(d)).unwrap();
        assert_eq!(seen.first(), Some(&(1 << 20)), "from where it stopped");
        assert!(std::fs::read(&dest).unwrap() == whole && !part.exists());
        // no ranges: from the start, not appended
        std::fs::remove_file(&dest).unwrap();
        std::fs::write(&part, &whole[..1 << 20]).unwrap();
        download(&url, &dest, &AtomicBool::new(false), |_, _| {}).unwrap();
        assert!(std::fs::read(&dest).unwrap() == whole);
    }

    #[test]
    fn files_already_downloaded_are_found_by_name_and_size() {
        let t = tempfile::tempdir().unwrap();
        let snap = t.path().join("hf/hub/models--Qwen--X/snapshots/abc");
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::write(snap.join("m.gguf"), [0u8; 10]).unwrap();
        std::fs::create_dir_all(t.path().join("hf/hub/models--Qwen--X/blobs")).unwrap();
        std::fs::write(t.path().join("hf/hub/models--Qwen--X/blobs/m.gguf"), [0u8; 10]).unwrap();
        let dirs = [t.path().join("models"), t.path().join("hf")];
        let url = "https://h/x/resolve/main/m.gguf";
        assert_eq!(reusable(url, &dirs, || Some(10)), Some(snap.join("m.gguf")), "not the blobs");
        assert_eq!(reusable(url, &dirs, || None), Some(snap.join("m.gguf")));
        assert_eq!(reusable(url, &dirs, || Some(11)), None, "another size");
        assert_eq!(reusable("https://h/other.gguf", &dirs, || panic!("asked")), None);
        let dest = t.path().join("models/koboldcpp/m.gguf");
        link(&snap.join("m.gguf"), &dest).unwrap();
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), 10);
    }
}
