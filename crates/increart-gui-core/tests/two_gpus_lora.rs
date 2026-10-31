//! The app on two GPUs at once, in one project: Z-Image Turbo on an
//! sd-server and on a KoboldCpp, each with and without a LoRA, the files
//! spread over three model directories. What it shows:
//!
//! - each model preset's files found in whichever folder holds them, by
//!   both servers (sd-server names those outside its first `--model-dir`
//!   `@<hash>/…`; KoboldCpp is given every folder's `loras/`);
//! - one model per server, both generating at the same time;
//! - the LoRA applied: asked for a photo, each server makes a child's
//!   crayon drawing on white paper with it and not without it (the same model, prompt and seed), and it's
//!   recorded with every file's SHA-256 in each image's recipe.
//!
//! Needs both GPUs and the files below, so it's run by hand:
//!
//!     INCREART_IT_SDSERVER=<sd-server binary> INCREART_IT_KOBOLDCPP=<koboldcpp binary> \
//!     INCREART_IT_MODEL_DIRS=<dir0>:<dir1>:<dir2> \
//!         cargo test --release -p increart-gui-core --test two_gpus_lora -- --ignored --nocapture
//!
//! The folders hold, by where they are ([`FILES`]): the VAE in the first,
//! the diffusion models and the text encoder in the second, the LoRA
//! (<https://huggingface.co/ostris/z_image_turbo_childrens_drawings>) in
//! the third's `loras/`. `INCREART_IT_SDSERVER_DEVICE` (default `ROCm1`)
//! and `INCREART_IT_KOBOLDCPP_GPU` (KoboldCpp's Vulkan number, default 1)
//! must be different GPUs. `INCREART_IT_OUT`, if set, gets the images and
//! the servers' logs afterwards, failed or not.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use increart_core::Library;
use increart_core::config::{Config, PresetLora, RecipePreset, SdServerConfig, ServerConfig, ServerPreset};
use increart_core::kobold::{KoboldRun, KoboldServer};
use increart_core::model::Recipe;
use increart_core::pe::PeDevice;
use increart_gui_core::*;

const BF16: &str = "diffusion_models/z_image_turbo_bf16.safetensors";
const Q8: &str = "diffusion_models/z-image-turbo-q8_0.gguf";
const LLM: &str = "text_encoders/qwen_3_4b.safetensors";
const VAE: &str = "vae/zit_vae_m0.safetensors";
const LORA: &str = "loras/z_image_turbo_childrens_drawings.safetensors";
/// Each file and the folder (of `INCREART_IT_MODEL_DIRS`) it must be in.
const FILES: [(&str, usize); 5] = [(VAE, 0), (BF16, 1), (Q8, 1), (LLM, 1), (LORA, 2)];
const SD_PORT: u16 = 1240;
const KCPP_PORT: u16 = 5111;
const SEEDS: u64 = 2;
/// A photo, asked of every preset: the LoRA's child's drawing is its own.
const PROMPT: &str = "A photograph of a lighthouse on a cliff at sunset, waves below, film grain.";
const SIZE: u32 = 512;

struct App {
    state: AppState,
    rt: Runtime,
    t0: Instant,
}

impl App {
    fn send(&mut self, msg: Msg) {
        for f in update(&mut self.state, msg) {
            match f {
                Effect::SaveConfig(..) | Effect::PlaySound { .. } => {}
                // never on the desktop
                Effect::Pick { .. } | Effect::OpenInEditor { .. } => {}
                f => self.rt.run(f),
            }
        }
        // yes to whatever is asked
        if let Some(c) = self.state.confirm.take() {
            self.log(&format!("confirm: {}", c.text));
            self.send(*c.then);
        }
    }

    fn log(&self, text: &str) {
        eprintln!("{:7.1}s {text}", self.t0.elapsed().as_secs_f64());
    }

    /// Feeds the runtime's messages back until `done` (false: timed out),
    /// calling `each` after every batch. An error shown to the user fails
    /// the test at once: the app may retry what failed until the limit.
    #[track_caller]
    fn pump(&mut self, limit: Duration, mut each: impl FnMut(&AppState), done: impl Fn(&AppState) -> bool) -> bool {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            for m in self.rt.wait(Duration::from_millis(50)) {
                self.send(m);
            }
            let errors: Vec<&String> = self.state.messages.iter().filter(|m| m.error).map(|m| &m.text).collect();
            assert!(errors.is_empty(), "{errors:?}");
            each(&self.state);
            if done(&self.state) {
                return true;
            }
        }
        false
    }
}

fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(300)).is_ok()
}

/// Copies the servers' logs (from the first folder) and the library's
/// images (in the second) to `INCREART_IT_OUT` when the test ends.
struct KeepLogs(PathBuf, PathBuf);

impl Drop for KeepLogs {
    fn drop(&mut self) {
        let Some(out) = std::env::var_os("INCREART_IT_OUT").map(PathBuf::from) else { return };
        let _ = std::fs::create_dir_all(&out);
        for f in std::fs::read_dir(&self.0).into_iter().flatten().flatten() {
            let _ = std::fs::copy(f.path(), out.join(f.file_name()));
        }
        for f in walk(&self.1) {
            if f.extension().is_some_and(|e| e == "png" || e == "webp")
                && !f.components().any(|c| c.as_os_str() == "cache")
            {
                let _ = std::fs::copy(&f, out.join(f.file_name().unwrap()));
            }
        }
        eprintln!("logs and images copied to {}", out.display());
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
    }
    out
}

fn preset(name: &str, diffusion: &str, lora: bool) -> RecipePreset {
    RecipePreset {
        name: name.into(),
        recipe: "z-image-turbo".into(),
        files: [("diffusion_model", diffusion), ("llm", LLM), ("vae", VAE)].map(|(s, f)| (s.into(), f.into())).into(),
        steps: 8,
        cfg: 1.0,
        flow_shift: None,
        custom_sigmas: vec![],
        loras: if lora { vec![PresetLora { file: LORA.into(), weight: 1.0 }] } else { vec![] },
    }
}

fn config(sd: &str, kcpp: &str, dirs: &[String], sd_device: &str, kcpp_gpu: u32) -> Config {
    let mut config = Config {
        // the LoRA-less presets load what their LoRA presets load: no reload
        recipes: vec![
            preset("ZIT bf16 + drawing", BF16, true),
            preset("ZIT Q8 + drawing", Q8, true),
            preset("ZIT bf16", BF16, false),
            preset("ZIT Q8", Q8, false),
        ],
        sdserver: SdServerConfig {
            binary: sd.into(),
            model_dirs: dirs.to_vec(),
            args: vec!["--fa".into()],
            preview: "none".into(),
            ..Default::default()
        },
        koboldcpp: kcpp.into(),
        ..Default::default()
    };
    config.server_presets = vec![ServerPreset {
        name: "sd-server + KoboldCpp".into(),
        servers: vec![
            ServerConfig::Start {
                name: "sd".into(),
                host: "127.0.0.1".into(),
                port: SD_PORT,
                device: sd_device.into(),
                args: vec![],
            },
            ServerConfig::Kobold(KoboldServer {
                name: "kcpp".into(),
                run: KoboldRun::Start {
                    binary: kcpp.into(),
                    config: String::new(),
                    host: "127.0.0.1".into(),
                    port: KCPP_PORT,
                    device: PeDevice::Gpu { devices: vec![kcpp_gpu], runtime: Default::default() },
                    args: ["--sdflashattention", "--sdoffloadcpu", "--sdtiledvae", "512"].map(String::from).into(),
                },
                chat: None,
                swap: false,
            }),
        ],
    }];
    config.normalize();
    config
}

#[test]
#[ignore = "needs both GPUs, sd-server, KoboldCpp and the models (INCREART_IT_SDSERVER, INCREART_IT_KOBOLDCPP, \
            INCREART_IT_MODEL_DIRS)"]
fn two_gpus_two_servers_loras_across_folders() {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} not set"));
    let (sd, kcpp) = (var("INCREART_IT_SDSERVER"), var("INCREART_IT_KOBOLDCPP"));
    let dirs: Vec<String> = var("INCREART_IT_MODEL_DIRS").split(':').map(String::from).collect();
    assert_eq!(dirs.len(), 3, "INCREART_IT_MODEL_DIRS: three folders");
    for (file, i) in FILES {
        let in_it = Path::new(&dirs[i]).join(file);
        assert!(in_it.is_file(), "{} missing", in_it.display());
        for (j, d) in dirs.iter().enumerate() {
            assert!(j == i || !Path::new(d).join(file).exists(), "{file} is in {d} too: spread them");
        }
    }
    let sd_device = std::env::var("INCREART_IT_SDSERVER_DEVICE").unwrap_or_else(|_| "ROCm1".into());
    let kcpp_gpu = std::env::var("INCREART_IT_KOBOLDCPP_GPU").map_or(1, |g| g.parse().expect("a GPU number"));
    assert!(!listening(SD_PORT) && !listening(KCPP_PORT), "the test's ports are taken");

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let logs = tmp.path().join("logs");
    let _logs = KeepLogs(logs.clone(), root.clone());
    let rt = Runtime::new(|| {}).unwrap();
    rt.set_trash_dir(tmp.path().join("trash"));
    // not the app's log folder
    rt.set_log_dir(logs);
    let config = config(&sd, &kcpp, &dirs, &sd_device, kcpp_gpu);
    let mut app = App { state: AppState::new(config, None), rt, t0: Instant::now() };

    // ---- a new library and a project, with a run per preset saved before
    // the servers start: they get them all at once (queued one by one,
    // each after its files are hashed, the first model is on both servers
    // before the second's runs come)
    app.send(Msg::SetLibraryPath(root.to_string_lossy().into_owned()));
    app.send(Msg::CreateLibrary);
    assert!(app.pump(Duration::from_secs(60), |_| {}, |s| s.library.is_some()), "no library");
    app.send(Msg::NewProject(PathBuf::new()));
    app.send(Msg::CommitRename);
    assert!(app.pump(Duration::from_secs(30), |_| {}, |s| s.project.is_some() && s.saving == 0), "no project");
    let project = app.state.project.clone().unwrap();
    app.send(Msg::Tab(Tab::Projects));
    app.send(Msg::ProjectsTab(ProjectsTab::Generate));
    app.send(Msg::SelectProject(project));
    // (no "no server is open" note first)
    app.state.no_server_warned = true;
    for (i, p) in app.state.config.recipes.clone().iter().enumerate() {
        let mut f = app.state.generate.clone();
        f.label = p.name.clone();
        // no drawing asked for: that's the LoRA's to bring
        f.prompt = PROMPT.into();
        f.preset = i;
        (f.width, f.height, f.steps, f.cfg) = (SIZE, SIZE, 8, 1.0);
        (f.seeds, f.first_seed) = (SEEDS, 7);
        app.send(Msg::Generate(f));
        app.send(Msg::QueueGenerate);
    }
    let n = app.state.config.recipes.len();
    let saved = app.pump(
        Duration::from_secs(1800),
        |_| {},
        |s| s.saving == 0 && s.index.projects.iter().map(|p| p.runs.len()).sum::<usize>() == n,
    );
    assert!(saved, "the {n} runs not saved");
    app.log("runs saved (their files hashed)");
    // the messages say they wait for servers
    app.state.messages.clear();

    // ---- both servers
    app.send(Msg::OpenPresetNow(0));
    let up = app.pump(Duration::from_secs(300), |_| {}, |s| s.engine_running);
    assert!(up, "servers not up: {:?}", app.state.servers);
    app.log(&format!("up: {:?}", app.state.servers.iter().map(|s| (&s.name, &s.state)).collect::<Vec<_>>()));

    // ---- every image, noting when both servers had jobs at once
    // (progress comes only with previews, which are off)
    let t = Instant::now();
    let total = SEEDS as usize * 4;
    let mut both_at_once = false;
    let mut shown = String::new();
    let t0 = app.t0;
    let made = app.pump(
        Duration::from_secs(3600),
        |s| {
            let now: Vec<String> =
                s.servers.iter().map(|v| format!("{} {} idle {}", v.name, v.state, v.idle_since.is_some())).collect();
            let now = now.join(" | ");
            if now != shown {
                eprintln!("{:7.1}s {now}", t0.elapsed().as_secs_f64());
                shown = now;
            }
            both_at_once |=
                s.servers.len() == 2 && s.servers.iter().all(|v| v.state == "ready" && v.idle_since.is_none());
        },
        |s| s.index.projects.iter().map(|p| p.outputs.len()).sum::<usize>() >= total && s.active_runs.is_empty(),
    );
    assert!(made, "not all {total} images made: {:?}", app.state.servers);
    app.log(&format!("{total} images in {:.0} s (with the loads)", t.elapsed().as_secs_f64()));

    // ---- what each image's recipe says
    app.send(Msg::SelectProjectFolder(None));
    assert!(app.pump(Duration::from_secs(10), |_| {}, |s| s.saving == 0), "drafts not saved");
    let outs: Vec<_> = app.state.index.projects.iter().flat_map(|p| p.outputs.clone()).collect();
    app.rt.shutdown(false);
    drop(app);
    let lib = Library::open(&root).expect("reopen");
    let report = increart_core::check::check(&lib).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    let mut recipes: Vec<(Recipe, PathBuf)> =
        outs.iter().map(|o| (lib.recipe(&o.image.id).unwrap().expect("a recipe"), o.image.path(&root))).collect();
    recipes.sort_by_key(|(r, _)| (r.model.files["diffusion_model"].id.clone(), r.seed, r.model.loras.len()));
    let mut device_of: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // (model, server, generation start and end)
    let mut made_when: Vec<(String, String, f64, f64)> = Vec::new();
    for (r, file) in &recipes {
        let diffusion = &r.model.files["diffusion_model"].id;
        eprintln!(
            "{diffusion} {} seed {}: {} ({}) in {:.1} s",
            r.model.loras_text().unwrap_or_else(|| "no LoRA".into()),
            r.seed,
            r.device,
            r.sdcpp,
            r.elapsed_s
        );
        device_of.entry(diffusion.clone()).or_default().insert(format!("{} {}", r.device, r.sdcpp));
        let end = std::fs::metadata(file).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap();
        let end = end.as_secs_f64();
        made_when.push((diffusion.clone(), format!("{} {}", r.device, r.sdcpp), end - f64::from(r.elapsed_s), end));
        for (slot, f) in &r.model.files {
            assert!(!f.sha256.is_empty(), "{slot} {} has no SHA-256 (found in no folder?)", f.id);
        }
        for l in &r.model.loras {
            assert_eq!(l.id, LORA);
            assert!(!l.sha256.is_empty(), "the LoRA has no SHA-256");
        }
        let img = image::open(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        assert_eq!((img.width(), img.height()), (SIZE, SIZE));
    }
    eprintln!("{device_of:?}");
    assert_eq!(device_of.len(), 2, "both models ran");
    // each server starts on a model of its own (one that runs out of work
    // may then load the other's to help: DESIGN.md, the scheduler), and
    // the two generate at once
    made_when.sort_by(|a, b| a.3.total_cmp(&b.3));
    let mut first: BTreeMap<&String, &String> = BTreeMap::new();
    for (model, server, ..) in &made_when {
        first.entry(server).or_insert(model);
    }
    let firsts: BTreeSet<&&String> = first.values().collect();
    assert_eq!((first.len(), firsts.len()), (2, 2), "each server's first image of its own model: {first:?}");
    let overlap =
        made_when.iter().any(|a| made_when.iter().any(|b| a.0 != b.0 && a.1 != b.1 && a.2 < b.3 && b.2 < a.3));
    assert!(overlap, "no two images of the two models made at once on the two servers: {made_when:?}");
    let servers: BTreeSet<&String> = device_of.values().flatten().collect();
    assert_eq!(servers.len(), 2, "both servers made images: {device_of:?}");
    // ---- the LoRA makes a drawing: each seed with it against without
    // (the same server, model, prompt and seed)
    for pair in recipes.chunk_by(|(a, _), (b, _)| a.model.files == b.model.files && a.seed == b.seed) {
        let [(plain, a), (lora, b)] = pair else { panic!("not a pair: {} images", pair.len()) };
        assert!(plain.model.loras.is_empty() && !lora.model.loras.is_empty());
        let (d, fa, fb) = (mean_diff(a, b), paper(a), paper(b));
        eprintln!(
            "{} seed {}: the LoRA changed the pixels by {d:.1} on average; white paper {:.0}% without it, {:.0}% with it",
            plain.model.files["diffusion_model"].id,
            plain.seed,
            fa * 100.0,
            fb * 100.0
        );
        assert!(d > 8.0, "the LoRA barely changed {} ({d:.1})", b.display());
        assert!(fb > fa + 0.2, "{} isn't a drawing on white paper next to {}", b.display(), a.display());
    }
    assert!(both_at_once, "the two servers never had jobs at the same time, on different models");
}

/// The share of white pixels (every channel over 230): a child's drawing
/// is mostly bare paper, a sunset photo has next to none (under 5%).
fn paper(f: &Path) -> f64 {
    let img = image::open(f).unwrap().to_rgb8();
    let white = img.pixels().filter(|p| p.0.iter().all(|&c| c > 230)).count();
    white as f64 / (img.width() * img.height()) as f64
}

/// The mean difference of two images' RGB values (0–255).
fn mean_diff(a: &Path, b: &Path) -> f64 {
    let (a, b) = (image::open(a).unwrap().to_rgb8(), image::open(b).unwrap().to_rgb8());
    let sum: u64 = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    sum as f64 / a.as_raw().len() as f64
}
