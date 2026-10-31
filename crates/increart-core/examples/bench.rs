//! Benchmarks one server configuration for image editing (Qwen-Image 2.1, one
//! reference) and/or prompt enhance (Qwen3-VL through KoboldCpp), through the
//! app's own launcher and clients: load time, seconds per image, PE speed,
//! and peak VRAM, GTT and host RAM. Results: `docs/benchmark-servers.html`;
//! the runner is `scripts/bench-servers.sh`.
//!
//!     cargo run --release -p increart-core --example bench -- '<case JSON>' <out dir>
//!
//! The case names the card (`card1`/`card2` in `/sys/class/drm`) its servers
//! run on. VRAM and GTT are the servers' own, from the kernel's per-process
//! accounting (`/proc/<pid>/fdinfo`, that card only), and host RAM their
//! resident memory, so other users of the cards don't count and two cases
//! on different cards can run at once.
//!
//! The model and binary paths below default to the development machine's;
//! each is overridden by the environment variable named beside it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use increart_core::config::{SdServerConfig, ServerConfig};
use increart_core::kobold::{KoboldChat, KoboldRun, KoboldServer};
use increart_core::pe::{self, PeDevice, PeRun, PeServer, PeStart};
use increart_core::request::{GenOptions, img_gen_body_from, warmup_body};
use increart_core::sdserver::{Client, JobStatus, ModelKey};
use serde::Deserialize;
use serde_json::{Value, json};

const MODELS: (&str, &str) = ("BENCH_MODELS", "/mnt/storage/comfy-new/ComfyUI/models");
const KOBOLDCPP: (&str, &str) = ("BENCH_KOBOLDCPP", "/workspace/local/koboldcpp/koboldcpp");
const REF: (&str, &str) = ("BENCH_REF", "/mnt/storage/increart_library/input_images/2026.10.24-22.01-0000.png");
const PE_MODEL: (&str, &str) = (
    "BENCH_PE_MODEL",
    "/mnt/storage/huggingface/hub/models--Qwen--Qwen3-VL-8B-Instruct-GGUF/snapshots/f982a07559d4a2f6c8744d840bf6fccab30eea96/Qwen3VL-8B-Instruct-Q4_K_M.gguf",
);
const PE_MMPROJ: (&str, &str) = (
    "BENCH_PE_MMPROJ",
    "/mnt/storage/huggingface/hub/models--Qwen--Qwen3-VL-8B-Instruct-GGUF/snapshots/f982a07559d4a2f6c8744d840bf6fccab30eea96/mmproj-Qwen3VL-8B-Instruct-F16.gguf",
);
/// A path constant: its environment variable when set, else the default.
fn path((var, default): (&str, &str)) -> String {
    std::env::var(var).unwrap_or_else(|_| default.into())
}

const PROMPT: &str = "Turn this into a winter scene: the same three mime women, same poses and outfits, \
    now wearing red scarves and standing in falling snow in a quiet Paris street at dusk, warm lamp light.";

#[derive(Deserialize)]
struct Case {
    id: String,
    /// The DRM card the servers run on (1 or 2).
    card: u32,
    image: Option<ImageCase>,
    pe: Option<PeCase>,
    /// Timed images after the first (more if they spread over 3%).
    #[serde(default = "three")]
    count: usize,
}

fn three() -> usize {
    3
}

#[derive(Deserialize)]
struct ImageCase {
    /// `sd` (sd-server) or `kobold` (KoboldCpp).
    server: String,
    #[serde(default)]
    binary: String,
    /// sd-server `--backend` device, or KoboldCpp's Vulkan device number.
    device: String,
    #[serde(default)]
    args: Vec<String>,
    port: u16,
    /// Model files in place of the defaults (`model_key`), by role; paths
    /// relative to the model folder, or absolute.
    #[serde(default)]
    models: std::collections::BTreeMap<String, String>,
    /// KoboldCpp: the PE chat model beside the image model, in the same
    /// server (its GPU layers set by `args`).
    #[serde(default)]
    chat: bool,
}

#[derive(Deserialize)]
struct PeCase {
    /// KoboldCpp's Vulkan device number; none: the CPU.
    gpu: Option<u32>,
    port: u16,
    #[serde(default = "three")]
    asks: usize,
    /// Ask the image server's chat model (at `port`) instead of starting
    /// one, after the images rather than during them.
    #[serde(default)]
    same: bool,
}

/// Peak memory seen by the sampler, and a coarse timeline for charts.
#[derive(Default)]
struct Mem {
    /// The whole card's VRAM in use at the start, and its peak since (a
    /// cross-check: other users of the card count there).
    base_card: u64,
    card: u64,
    vram: u64,
    gtt: u64,
    rss: u64,
    phase: String,
    /// Peaks per phase: (phase, vram, gtt, rss).
    phases: Vec<(String, u64, u64, u64)>,
    /// (seconds, vram, gtt, rss) every 0.5 s.
    timeline: Vec<(f64, u64, u64, u64)>,
}

fn read_u64(p: &str) -> u64 {
    std::fs::read_to_string(p).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

fn card_mem(card: u32) -> (u64, u64) {
    let d = format!("/sys/class/drm/card{card}/device");
    (read_u64(&format!("{d}/mem_info_vram_used")), read_u64(&format!("{d}/mem_info_gtt_used")))
}

/// The card's PCI address, as `drm-pdev` in fdinfo spells it.
fn card_pci(card: u32) -> String {
    std::fs::read_link(format!("/sys/class/drm/card{card}/device"))
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One `drm-*` value of an fdinfo, in bytes.
fn fdinfo_bytes(text: &str, key: &str) -> u64 {
    let Some(v) = text.lines().find_map(|l| l.strip_prefix(key)) else { return 0 };
    let mut it = v.trim_start_matches(':').split_whitespace();
    let n: u64 = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    n * match it.next() {
        Some("KiB") => 1 << 10,
        Some("MiB") => 1 << 20,
        Some("GiB") => 1 << 30,
        _ => 1,
    }
}

/// Of every process descended from this one (the servers it started and
/// theirs): resident host memory, and resident VRAM and GTT on the card
/// at `pci`, in bytes.
fn tree_mem(pci: &str) -> (u64, u64, u64) {
    let me = std::process::id();
    let mut parent = std::collections::HashMap::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        // the name may hold spaces: fields after its closing parenthesis
        let Some(rest) = stat.rsplit_once(')').map(|r| r.1) else { continue };
        let ppid: u32 = rest.split_whitespace().nth(1).and_then(|p| p.parse().ok()).unwrap_or(0);
        parent.insert(pid, ppid);
    }
    let under = |mut p: u32| {
        while p > 1 {
            p = parent.get(&p).copied().unwrap_or(0);
            if p == me {
                return true;
            }
        }
        false
    };
    let (mut rss, mut vram, mut gtt) = (0, 0, 0);
    // a GPU client may be open on several fds (and processes): once each
    let mut clients = std::collections::HashSet::new();
    for (&pid, _) in parent.iter().filter(|(p, _)| under(**p)) {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let kb = status.lines().find_map(|l| l.strip_prefix("VmRSS:")).and_then(|v| v.split_whitespace().next());
        rss += kb.and_then(|k| k.parse::<u64>().ok()).unwrap_or(0) * 1024;
        for fd in std::fs::read_dir(format!("/proc/{pid}/fdinfo")).into_iter().flatten().flatten() {
            let Ok(info) = std::fs::read_to_string(fd.path()) else { continue };
            if !info.lines().any(|l| l.starts_with("drm-pdev:") && l.ends_with(pci)) {
                continue;
            }
            let client = info.lines().find(|l| l.starts_with("drm-client-id:")).unwrap_or("").to_string();
            if clients.insert(client) {
                vram += fdinfo_bytes(&info, "drm-resident-vram");
                gtt += fdinfo_bytes(&info, "drm-resident-gtt");
            }
        }
    }
    (rss, vram, gtt)
}

fn sampler(card: u32, mem: Arc<Mutex<Mem>>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let t0 = Instant::now();
        let mut last_kept = -1.0;
        let pci = card_pci(card);
        while !stop.load(Ordering::Relaxed) {
            let (rss, v, g) = tree_mem(&pci);
            let whole = card_mem(card).0;
            let mut m = mem.lock().unwrap();
            m.card = m.card.max(whole.saturating_sub(m.base_card));
            m.vram = m.vram.max(v);
            m.gtt = m.gtt.max(g);
            m.rss = m.rss.max(rss);
            if let Some(p) = m.phases.last_mut() {
                p.1 = p.1.max(v);
                p.2 = p.2.max(g);
                p.3 = p.3.max(rss);
            }
            let t = t0.elapsed().as_secs_f64();
            if t - last_kept >= 0.5 {
                m.timeline.push((t, v, g, rss));
                last_kept = t;
            }
            drop(m);
            std::thread::sleep(Duration::from_millis(100));
        }
    })
}

fn phase(mem: &Mutex<Mem>, name: &str) {
    let mut m = mem.lock().unwrap();
    m.phase = name.into();
    m.phases.push((name.into(), 0, 0, 0));
}

fn gib(b: u64) -> f64 {
    b as f64 / (1u64 << 30) as f64
}

/// The reference image at 1024×1024 (1 MP), as PNG.
fn reference() -> Vec<u8> {
    let img = image::open(path(REF)).expect("reference image").to_rgb8();
    let img = image::imageops::resize(&img, 1024, 1024, image::imageops::FilterType::Lanczos3);
    let mut png = vec![];
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    png
}

fn model_key() -> ModelKey {
    ModelKey {
        recipe: "qwen-image-21".into(),
        models: [
            ("diffusion_model", "diffusion_models/qwen_image_2.1_int8_convrot.safetensors"),
            ("llm", "text_encoders/qwen3vl_8b_int8_convrot.safetensors"),
            ("vae", "vae/qwen_image_2.1_vae_bf16.safetensors"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
    }
}

/// The body the app sends for one seed of a 1024² Qwen-Image 2.1 run with
/// one reference (recipe "CFG 2, refs": euler, 20 steps, CFG 2), with the
/// app's preview setting.
fn body(reference: &[u8], seed: u64) -> Value {
    img_gen_body_from(
        PROMPT,
        "",
        1024,
        1024,
        seed,
        json!({ "sample_method": "euler", "sample_steps": 20, "guidance": { "txt_cfg": 2.0 } }),
        &[reference.to_vec()],
        &GenOptions { preview: "proj".into(), preview_interval: 1 },
    )
}

struct Gen {
    wall_s: f64,
    server_s: Option<f64>,
}

/// Submits and waits (polling every 0.5 s, as the app does).
fn generate(client: &Client, body: &Value, save: Option<&Path>) -> Result<Gen, String> {
    let t = Instant::now();
    let id = client.img_gen(body).map_err(|e| e.to_string())?;
    loop {
        let job = client.job(&id).map_err(|e| e.to_string())?;
        match job.status {
            JobStatus::Completed => {
                let wall_s = t.elapsed().as_secs_f64();
                if let Some(p) = save {
                    let bytes = job.image().map_err(|e| e.to_string())?;
                    std::fs::write(p, bytes).map_err(|e| e.to_string())?;
                }
                return Ok(Gen { wall_s, server_s: job.elapsed_s() });
            }
            JobStatus::Failed | JobStatus::Cancelled => {
                return Err(job.error.map(|e| e.message).unwrap_or_else(|| "job failed".into()));
            }
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

fn spread(v: &[f64]) -> f64 {
    let (lo, hi) = v.iter().fold((f64::MAX, f64::MIN), |(l, h), x| (l.min(*x), h.max(*x)));
    if v.is_empty() { 0.0 } else { (hi - lo) / median(v) }
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    if s.is_empty() { 0.0 } else { s[s.len() / 2] }
}

fn open_image(c: &ImageCase, logs: &Path) -> Result<(increart_core::launcher::ServerProcess, f64), String> {
    let mut shared = SdServerConfig { model_dirs: vec![path(MODELS)], ..Default::default() };
    let server = match c.server.as_str() {
        "sd" => {
            shared.binary = c.binary.clone();
            shared.args = c.args.clone();
            ServerConfig::Start {
                name: format!("bench{}", c.port),
                host: "127.0.0.1".into(),
                port: c.port,
                device: c.device.clone(),
                args: vec![],
            }
        }
        _ => ServerConfig::Kobold(KoboldServer {
            name: format!("bench{}", c.port),
            run: KoboldRun::Start {
                binary: path(KOBOLDCPP),
                config: String::new(),
                host: "127.0.0.1".into(),
                port: c.port,
                device: PeDevice::Gpu { devices: vec![c.device.parse().unwrap()], runtime: Default::default() },
                args: c.args.clone(),
            },
            chat: c.chat.then(|| KoboldChat { model: path(PE_MODEL), mmproj: path(PE_MMPROJ), context: 8192 }),
            swap: false,
        }),
    };
    let t = Instant::now();
    let proc =
        increart_core::launcher::open(&server, &shared, logs, Duration::from_secs(1800)).map_err(|e| e.to_string())?;
    Ok((proc, t.elapsed().as_secs_f64()))
}

fn open_pe(c: &PeCase, logs: &Path) -> Result<(pe::PeProcess, f64), String> {
    let run = if c.same {
        PeRun::Connect { url: format!("http://127.0.0.1:{}", c.port) }
    } else {
        PeRun::KoboldCpp {
            binary: path(KOBOLDCPP),
            model: path(PE_MODEL),
            mmproj: path(PE_MMPROJ),
            config: String::new(),
            host: "127.0.0.1".into(),
            port: c.port,
            device: match c.gpu {
                Some(g) => PeDevice::Gpu { devices: vec![g], runtime: Default::default() },
                None => PeDevice::Cpu { threads: 16 },
            },
            context: 8192,
            args: vec![],
        }
    };
    let server = PeServer { name: format!("bench{}", c.port), run, start: PeStart::OnDemand, ..Default::default() };
    let t = Instant::now();
    let proc = pe::open(&server, logs, Duration::from_secs(900), &AtomicBool::new(false))?;
    Ok((proc, t.elapsed().as_secs_f64()))
}

/// One rewrite ask with the reference (as the app sends it: 768 px), and
/// KoboldCpp's own timings for it from `/api/extra/perf`.
fn pe_ask(client: &pe::Client, port: u16, image: &[u8]) -> Result<Value, String> {
    let t = Instant::now();
    let text = format!("Rewrite this prompt for the image generator, keeping its meaning:\n\n{PROMPT}");
    let answer =
        client.chat(pe::DEFAULT_REWRITE, &text, &[image.to_vec()], 700, pe::Sampling::default_for(pe::REWRITE))?;
    let wall = t.elapsed().as_secs_f64();
    let perf: Value = ureq::get(format!("http://127.0.0.1:{port}/api/extra/perf"))
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    let n = |k: &str| perf[k].as_f64().unwrap_or(0.0);
    let (pp_s, gen_s) = (n("last_process_time"), n("last_eval_time"));
    let (inp, out) = (n("last_input_count"), n("last_token_count"));
    Ok(json!({
        "wall_s": wall,
        "prompt_tokens": inp,
        "prompt_s": pp_s,
        "prompt_tps": if pp_s > 0.0 { inp / pp_s } else { 0.0 },
        "gen_tokens": out,
        "gen_s": gen_s,
        "gen_tps": if gen_s > 0.0 { out / gen_s } else { 0.0 },
        "answer_chars": answer.len(),
    }))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let case: Case = serde_json::from_str(&args[1]).expect("case JSON");
    let out = PathBuf::from(&args[2]);
    let logs = out.join("logs").join(&case.id);
    std::fs::create_dir_all(&logs).unwrap();
    let reference = reference();
    let pe_image = pe::image_for_request(Path::new(&path(REF)), pe::TEXT_IMAGE_SIZE).unwrap();

    let mem = Arc::new(Mutex::new(Mem::default()));
    mem.lock().unwrap().base_card = card_mem(case.card).0;
    let stop = Arc::new(AtomicBool::new(false));
    let sampling = sampler(case.card, mem.clone(), stop.clone());
    let mut result = json!({ "id": case.id, "card": case.card, "started": chrono::Utc::now().to_rfc3339() });
    let mut error: Option<String> = None;

    // servers stay open to the end, so the peaks cover them together
    let mut image_proc = None;
    let mut pe_proc = None;
    if let Some(c) = &case.image {
        phase(&mem, "image load");
        match open_image(c, &logs) {
            Ok((proc, launch_s)) => {
                result["image"] = json!({ "launch_s": launch_s, "software": proc.client.software() });
                let t = Instant::now();
                let mut key = model_key();
                key.models.extend(c.models.clone());
                match proc.client.load_model(&key) {
                    Ok(_) => result["image"]["load_s"] = json!(t.elapsed().as_secs_f64()),
                    Err(e) => error = Some(format!("loading the model: {e}")),
                }
                image_proc = Some(proc);
            }
            Err(e) => error = Some(format!("starting: {e}")),
        }
    }
    if error.is_none()
        && let Some(c) = &case.pe
    {
        phase(&mem, "PE load");
        match open_pe(c, &logs) {
            Ok((proc, load_s)) => {
                result["pe"] = json!({ "load_s": load_s });
                pe_proc = Some(proc);
            }
            Err(e) => error = Some(format!("starting PE: {e}")),
        }
    }

    // PE alongside the image job: asks keep coming while images generate
    let pe_busy = Arc::new(AtomicBool::new(false));
    let pe_during = Arc::new(Mutex::new(vec![]));
    let pe_thread = match (&pe_proc, &image_proc, &case.pe) {
        (Some(p), Some(_), Some(c)) if error.is_none() && !c.same => {
            let (client, port, img, busy, during) =
                (p.client.clone(), c.port, pe_image.clone(), pe_busy.clone(), pe_during.clone());
            busy.store(true, Ordering::Relaxed);
            Some(std::thread::spawn(move || {
                while busy.load(Ordering::Relaxed) {
                    match pe_ask(&client, port, &img) {
                        Ok(v) => during.lock().unwrap().push(v),
                        Err(e) => during.lock().unwrap().push(json!({ "error": e })),
                    }
                }
            }))
        }
        _ => None,
    };

    if error.is_none()
        && let Some(proc) = &image_proc
    {
        phase(&mem, "generate");
        let t = Instant::now();
        let warm = generate(&proc.client, &warmup_body(), None);
        result["image"]["warmup_s"] = json!(t.elapsed().as_secs_f64());
        let mut times = vec![];
        let mut server_times = vec![];
        let mut first = None;
        let r = warm.and_then(|_| {
            let mut seed = 1;
            loop {
                let save = out.join(format!("{}-{seed}.png", case.id));
                let g = generate(&proc.client, &body(&reference, seed), Some(&save))?;
                eprintln!("{}: image {seed}: {:.1} s", case.id, g.wall_s);
                if first.is_none() {
                    first = Some(g.wall_s);
                } else {
                    times.push(g.wall_s);
                    server_times.extend(g.server_s);
                }
                seed += 1;
                let enough = times.len() >= case.count && (spread(&times) <= 0.03 || times.len() >= case.count + 3);
                if enough {
                    return Ok(());
                }
            }
        });
        if let Err(e) = r {
            error = Some(format!("generating: {e}"));
        }
        result["image"]["first_image_s"] = json!(first);
        result["image"]["image_s"] = json!(times);
        result["image"]["server_s"] = json!(server_times);
        result["image"]["median_s"] = json!(median(&times));
        result["image"]["median_server_s"] = json!(median(&server_times));
        result["image"]["spread"] = json!(spread(&times));
    }
    pe_busy.store(false, Ordering::Relaxed);
    if let Some(t) = pe_thread {
        let _ = t.join();
        result["pe"]["during_images"] = json!(*pe_during.lock().unwrap());
    }

    if error.is_none()
        && let (Some(p), Some(c)) = (&pe_proc, &case.pe)
    {
        phase(&mem, "PE asks");
        let mut asks = vec![];
        for _ in 0..c.asks {
            match pe_ask(&p.client, c.port, &pe_image) {
                Ok(v) => {
                    eprintln!("{}: PE ask: {v}", case.id);
                    asks.push(v);
                }
                Err(e) => {
                    error = Some(format!("PE ask: {e}"));
                    break;
                }
            }
        }
        result["pe"]["asks"] = json!(asks);
    }

    stop.store(true, Ordering::Relaxed);
    let _ = sampling.join();
    let m = mem.lock().unwrap();
    result["peak"] = json!({
        "vram_gib": gib(m.vram),
        "gtt_gib": gib(m.gtt),
        "host_gib": gib(m.rss),
        "card_vram_rise_gib": gib(m.card),
    });
    result["phases"] = json!(
        m.phases
            .iter()
            .map(|(p, v, g, r)| json!({ "phase": p, "vram_gib": gib(*v), "gtt_gib": gib(*g), "host_gib": gib(*r) }))
            .collect::<Vec<_>>()
    );
    result["timeline"] =
        json!(m.timeline.iter().map(|(t, v, g, r)| json!([t, gib(*v), gib(*g), gib(*r)])).collect::<Vec<_>>());
    // the other card, to catch a server on the wrong one
    let other = if case.card == 1 { 2 } else { 1 };
    result["other_card_vram_gib"] = json!(gib(card_mem(other).0));
    result["error"] = json!(error);
    drop(m);
    drop(image_proc);
    drop(pe_proc);
    std::fs::write(out.join(format!("{}.json", case.id)), serde_json::to_string_pretty(&result).unwrap()).unwrap();
    println!("{}", serde_json::to_string(&result["peak"]).unwrap());
}
