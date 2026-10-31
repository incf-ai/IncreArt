//! The app against a real KoboldCpp on the CPU: what the fakes can't show
//! about each OS (starting and stopping processes, admin reloads, sockets,
//! cancelling a generation, the files written). The images don't matter,
//! so the model set is the smallest there is (Q2_K diffusion, IQ1_S
//! encoder) at 256 x 256 and one step. Run by `.github/workflows/
//! koboldcpp-cpu.yml`; by hand:
//!
//!     INCREART_IT_KOBOLDCPP=<koboldcpp binary> INCREART_IT_MODELS=<dir> \
//!         cargo test --release -p increart-gui-core --test koboldcpp_cpu -- --ignored --nocapture
//!
//! `INCREART_IT_MODELS` holds the four files named below.
//! `INCREART_IT_VISION=0` leaves the vision projector out of image
//! generation (less memory; text-to-image doesn't need it).
//! `INCREART_IT_IMAGES=0` starts and stops the image server but generates
//! nothing, so the image model isn't loaded and there's no image to
//! describe (macOS runners: KoboldCpp's image generation uses Metal even
//! with `--usecpu`, and crashes on their virtual GPU).
//! `INCREART_IT_OUT`, if set, gets the servers' logs afterwards, failed or
//! not.

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use increart_core::config::{Config, RecipePreset, ServerConfig, ServerPreset};
use increart_core::engine::Stop;
use increart_core::kobold::{KoboldRun, KoboldServer};
use increart_core::pe::{PeDevice, PeRun, PeServer, PeStart};
use increart_core::{ImageRef, Library};
use increart_gui_core::update::PeUse;
use increart_gui_core::*;

const DIFFUSION: &str = "qwen-image-2.1-Q2_K.gguf";
const LLM: &str = "Qwen3-VL-8B-Instruct-UD-IQ1_S.gguf";
const MMPROJ: &str = "mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf";
const VAE: &str = "qwen_image_2.1_vae_bf16.safetensors";
const IMAGE_PORT: u16 = 5101;
const CHAT_PORT: u16 = 5102;

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

    /// Feeds the runtime's messages back until `done` (false: timed out).
    /// An error shown to the user fails the test at once: the app may
    /// retry what failed until the limit.
    #[track_caller]
    fn pump(&mut self, limit: Duration, done: impl Fn(&AppState) -> bool) -> bool {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            for m in self.rt.wait(Duration::from_millis(50)) {
                self.send(m);
            }
            let errors = self.errors();
            assert!(errors.is_empty(), "{errors:?}");
            if done(&self.state) {
                return true;
            }
        }
        false
    }

    fn errors(&self) -> Vec<String> {
        self.state.messages.iter().filter(|m| m.error).map(|m| m.text.clone()).collect()
    }

    fn outputs(&self) -> Vec<increart_core::scan::OutputLink> {
        self.state.index.projects.iter().flat_map(|p| p.outputs.iter().cloned()).collect()
    }

    /// A prompt enhance answer for `target` (the Generate prompt, or an
    /// image to describe); the window is closed afterwards.
    fn pe_answer(&mut self, target: PeTarget) -> String {
        let t = Instant::now();
        self.send(Msg::PeRewrite(target));
        let answered = self.pump(Duration::from_secs(600), |s| {
            s.pe.text.as_ref().is_some_and(|t| t.waiting.is_none() && (!t.text.is_empty() || t.error.is_some()))
        });
        let text = self.state.pe.text.clone();
        assert!(answered, "no answer: {:?} / PE {:?} / {:?}", text, self.state.pe.status, self.errors());
        let text = text.unwrap();
        assert!(text.error.is_none(), "PE failed: {:?}", text.error);
        self.log(&format!("answered in {:.1} s: {:?}", t.elapsed().as_secs_f64(), text.text));
        self.send(Msg::PeTextClose(PeUse::Discard));
        text.text
    }
}

fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(300)).is_ok()
}

/// Copies the servers' logs to `INCREART_IT_OUT` when the test ends.
struct KeepLogs;

impl Drop for KeepLogs {
    fn drop(&mut self) {
        let Some(out) = std::env::var_os("INCREART_IT_OUT").map(PathBuf::from) else { return };
        let _ = std::fs::create_dir_all(&out);
        let dir = increart_gui_core::runtime::log_dir();
        for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let _ = std::fs::copy(f.path(), out.join(f.file_name()));
        }
        eprintln!("logs of {} copied to {}", dir.display(), out.display());
    }
}

fn config(kcpp: &str, models: &Path, vision: bool) -> Config {
    let mut config = Config::default();
    let m = |f: &str| models.join(f).to_string_lossy().into_owned();
    let mut files: std::collections::BTreeMap<String, String> =
        [("diffusion_model", DIFFUSION), ("llm", LLM), ("vae", VAE)].map(|(s, f)| (s.into(), f.into())).into();
    if vision {
        files.insert("clip_l".into(), MMPROJ.into());
    }
    config.recipes = vec![RecipePreset {
        name: "Tiny (CI)".into(),
        recipe: "qwen-image-21".into(),
        files,
        steps: 1,
        cfg: 1.0,
        flow_shift: None,
        custom_sigmas: vec![],
        loras: vec![],
    }];
    // half the CPUs (the runners' vCPUs are hyperthreads: with all of them,
    // KoboldCpp on Windows stalled for minutes), but not KoboldCpp's own
    // choice of one on a macOS runner
    let threads = std::thread::available_parallelism().map_or(2, |n| (n.get() as u32 / 2).max(2));
    config.sdserver.model_dirs = vec![models.to_string_lossy().into_owned()];
    config.server_presets = vec![ServerPreset {
        name: "CPU".into(),
        servers: vec![
            ServerConfig::Kobold(KoboldServer {
                name: "cpu".into(),
                run: KoboldRun::Start {
                    binary: kcpp.into(),
                    config: String::new(),
                    host: "127.0.0.1".into(),
                    port: IMAGE_PORT,
                    device: PeDevice::Cpu { threads },
                    args: vec![],
                },
                chat: None,
                swap: false,
            }),
            // started when first asked, after the image server is stopped:
            // one model in memory at a time (macOS runners have 7 GB)
            ServerConfig::Chat(PeServer {
                name: "chat".into(),
                run: PeRun::KoboldCpp {
                    binary: kcpp.into(),
                    model: m(LLM),
                    mmproj: m(MMPROJ),
                    config: String::new(),
                    host: "127.0.0.1".into(),
                    port: CHAT_PORT,
                    device: PeDevice::Cpu { threads },
                    context: 2048,
                    args: vec![],
                },
                start: PeStart::OnDemand,
            }),
        ],
    }];
    config.normalize();
    config
}

#[test]
#[ignore = "needs KoboldCpp and the models (INCREART_IT_KOBOLDCPP, INCREART_IT_MODELS)"]
fn images_cancel_and_prompt_enhance() {
    let kcpp = std::env::var("INCREART_IT_KOBOLDCPP").expect("INCREART_IT_KOBOLDCPP");
    let models = PathBuf::from(std::env::var("INCREART_IT_MODELS").expect("INCREART_IT_MODELS"));
    for f in [DIFFUSION, LLM, MMPROJ, VAE] {
        assert!(models.join(f).is_file(), "{} missing", models.join(f).display());
    }
    let vision = std::env::var("INCREART_IT_VISION").map_or(true, |v| v != "0");
    let images = std::env::var("INCREART_IT_IMAGES").map_or(true, |v| v != "0");
    let _logs = KeepLogs;
    assert!(!listening(IMAGE_PORT) && !listening(CHAT_PORT), "the test's ports are taken");

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let rt = Runtime::new(|| {}).unwrap();
    rt.set_trash_dir(tmp.path().join("trash"));
    let mut app = App { state: AppState::new(config(&kcpp, &models, vision), None), rt, t0: Instant::now() };

    // ---- a new library, then the server preset
    app.send(Msg::SetLibraryPath(root.to_string_lossy().into_owned()));
    app.send(Msg::CreateLibrary);
    assert!(app.pump(Duration::from_secs(60), |s| s.library.is_some()), "no library: {:?}", app.errors());
    app.send(Msg::OpenPresetNow(0));
    let up = app.pump(Duration::from_secs(120), |s| s.engine_running);
    assert!(up, "KoboldCpp not up: {:?} / {:?}", app.state.servers, app.errors());
    assert!(listening(IMAGE_PORT));
    app.log(&format!("KoboldCpp up: {:?}", app.state.servers.iter().map(|s| &s.state).collect::<Vec<_>>()));

    // ---- a project to generate in
    app.send(Msg::NewProject(PathBuf::new()));
    app.send(Msg::CommitRename);
    assert!(app.pump(Duration::from_secs(30), |s| s.project.is_some() && s.saving == 0), "no project");
    let project = app.state.project.clone().unwrap();
    app.send(Msg::Tab(Tab::Projects));
    app.send(Msg::ProjectsTab(ProjectsTab::Generate));
    app.send(Msg::SelectProject(project.clone()));
    let outs = if images { make_images(&mut app, &root) } else { vec![] };

    // ---- the image server stopped: its process gone, the port free
    app.rt.run(Effect::StopServers);
    let freed = app.pump(Duration::from_secs(60), |_| !listening(IMAGE_PORT));
    assert!(freed, "KoboldCpp still listening on {IMAGE_PORT} after stopping");
    std::net::TcpListener::bind(("127.0.0.1", IMAGE_PORT)).expect("the image server's port can be used again");
    app.log("image server stopped");

    // ---- two prompt enhance answers, from a KoboldCpp started for them
    let mut f = app.state.generate.clone();
    f.prompt = "a {animal} in the snow".into();
    app.send(Msg::Generate(f));
    let text = app.pe_answer(PeTarget::Prompt);
    assert!(!text.trim().is_empty());
    if let Some(o) = outs.first() {
        let described = app.pe_answer(PeTarget::Describe(ImageRef::output(o.image.id.clone())));
        assert!(!described.trim().is_empty());
    }
    app.send(Msg::PeShutdown);
    // Off once its stop returns (the port goes before KoboldCpp's
    // launcher exits)
    let off = app.pump(Duration::from_secs(60), |s| s.pe.status == state::PeStatus::Off);
    assert!(off, "PE still {:?} after stopping", app.state.pe.status);
    let freed = app.pump(Duration::from_secs(60), |_| !listening(CHAT_PORT));
    assert!(freed, "the chat KoboldCpp still listening on {CHAT_PORT}");
    app.log("chat server stopped");

    // ---- the library as written
    app.send(Msg::SelectProjectFolder(None));
    assert!(app.pump(Duration::from_secs(10), |s| s.saving == 0), "drafts not saved");
    app.rt.shutdown(false);
    drop(app);
    let lib = Library::open(&root).expect("reopen");
    let report = increart_core::check::check(&lib).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!(report.outputs, outs.len(), "{}", report.summary());
    eprintln!("{}", report.summary());
}

/// Two images in the selected project, then a third stopped partway; the two kept.
fn make_images(app: &mut App, root: &Path) -> Vec<increart_core::scan::OutputLink> {
    // ---- two images
    let mut f = app.state.generate.clone();
    f.label = "pair".into();
    f.prompt = "a red fox in the snow".into();
    f.preset = 0;
    (f.width, f.height, f.steps, f.cfg) = (256, 256, 1, 1.0);
    (f.seeds, f.first_seed) = (2, 1);
    app.send(Msg::Generate(f.clone()));
    app.send(Msg::QueueGenerate);
    let t = Instant::now();
    // 304 s on a runner
    let made = app.pump(Duration::from_secs(900), |s| {
        s.index.projects.iter().map(|p| p.outputs.len()).sum::<usize>() >= 2 && s.active_runs.is_empty()
    });
    assert!(made, "two images not made: {:?}", app.errors());
    app.log(&format!("two images in {:.0} s (with the model load)", t.elapsed().as_secs_f64()));
    let outs = app.outputs();
    for o in &outs {
        let file = o.image.path(root);
        let img = image::open(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        assert_eq!((img.width(), img.height()), (256, 256), "{}", file.display());
    }

    // ---- a generation stopped partway: gone from the app at once
    f.label = "stopped".into();
    (f.steps, f.seeds, f.first_seed) = (8, 1, 3);
    app.send(Msg::Generate(f.clone()));
    app.send(Msg::QueueGenerate);
    let started = app.pump(Duration::from_secs(300), |s| s.progress.values().any(|(step, _)| *step >= 1));
    assert!(started, "the third image never started: {:?}", app.errors());
    let run = app.state.active_runs.iter().next().cloned().expect("an active run");
    app.log("generating; stopping it now");
    let t = Instant::now();
    app.send(Msg::CancelRun(run.clone(), Stop::Now));
    let stopped = app.pump(Duration::from_secs(30), |s| s.active_runs.is_empty() && s.progress.is_empty());
    assert!(stopped, "still generating 30 s after Stop now: {:?}", app.state.active_runs);
    let took = t.elapsed();
    app.log(&format!("stopped in {:.2} s", took.as_secs_f64()));
    assert!(took < Duration::from_secs(5), "stopping took {took:?}");
    assert_eq!(app.outputs().len(), 2, "the stopped image was kept");

    outs
}
