//! The engine against fake sd-servers and a real library: runs end to end,
//! model switching, failures, cancel, back-off and purged jobs.

mod common;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use common::fake::Fake;
use increart_core::engine::{Endpoint, Engine, EngineOptions, Event, Stop};
use increart_core::model::*;
use increart_core::sdserver::Client;
use increart_core::{Id, ImageRef, Library, NewIdea, NewRun, Original, Store, png};

const FILES: &[(&str, &str)] = &[("diffusion_model", "diffusion_models/q.safetensors"), ("vae", "vae/v.safetensors")];

fn model(recipe: &str) -> ModelSelection {
    let files = FILES
        .iter()
        .map(|(slot, id)| (slot.to_string(), ModelFile { id: id.to_string(), sha256: "cd".repeat(32) }))
        .collect::<BTreeMap<_, _>>();
    ModelSelection { recipe: recipe.into(), files, loras: vec![] }
}

struct Setup {
    _tmp: tempfile::TempDir,
    lib: Arc<Mutex<Library>>,
    project: Id,
    ref_image: Id,
}

fn setup() -> Setup {
    common::init();
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = Library::create(&tmp.path().join("lib"), None).unwrap();
    let img = image::RgbaImage::from_fn(16, 16, |x, y| image::Rgba([x as u8 * 16, y as u8 * 16, 9, 200]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    let ref_image =
        lib.import_bytes(&bytes, &Original { name: Some("robot.png".into()), ..Default::default() }).unwrap().image;
    let idea =
        lib.new_idea(NewIdea { images: vec![ref_image.clone()], ..NewIdea::text("Robot", "robot sticker") }).unwrap();
    let (project, _) = lib.create_project(Path::new(""), Some("stickers"), &[idea]).unwrap();
    Setup { _tmp: tmp, lib: Arc::new(Mutex::new(lib)), project, ref_image }
}

fn new_run(s: &Setup, recipe: &str, seeds: u64, alpha: bool) -> Id {
    let run = the_run(s, recipe, seeds, alpha);
    s.lib.lock().unwrap().create_run(&s.project, run).unwrap()
}

/// A run of `seeds` changed by `f`.
fn new_run_with(s: &Setup, seeds: u64, f: impl FnOnce(&mut NewRun)) -> Id {
    let mut run = the_run(s, "qwen-image-21", seeds, false);
    f(&mut run);
    s.lib.lock().unwrap().create_run(&s.project, run).unwrap()
}

fn the_run(s: &Setup, recipe: &str, seeds: u64, alpha: bool) -> NewRun {
    NewRun {
        sweep: None,
        label: "r01".into(),
        prompt: "cute robot sticker".into(),
        negative_prompt: String::new(),
        alpha,
        width: 1024,
        height: 1024,
        sample: Sample {
            method: "euler".into(),
            scheduler: "simple".into(),
            steps: 20,
            cfg: 6.0,
            flow_shift: Some(3.0),
            custom_sigmas: vec![],
        },
        mode: Mode::Generate,
        refs: vec![ImageRef::input(s.ref_image.clone())],
        model: model(recipe),
        source_idea: None,
        first_seed: 0,
        seeds,
    }
}

fn opts() -> EngineOptions {
    EngineOptions {
        poll_interval: Duration::from_millis(10),
        backoff_base: Duration::from_millis(50),
        sdcpp: "fd21ecf".into(),
        ..Default::default()
    }
}

fn start(s: &Setup, fakes: &[&Fake]) -> Engine {
    let servers = fakes
        .iter()
        .enumerate()
        .map(|(i, f)| Endpoint { name: format!("gpu{i}"), device: format!("ROCm{i}"), client: Client::new(&f.url) })
        .collect();
    Engine::start(s.lib.clone(), servers, opts(), None)
}

/// Events until `stop` returns true (panics after 20 s).
fn collect(engine: &Engine, mut stop: impl FnMut(&Event, &[Event]) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut out = vec![];
    while Instant::now() < deadline {
        if let Ok(e) = engine.events().recv_timeout(Duration::from_millis(100)) {
            let done = stop(&e, &out);
            out.push(e);
            if done {
                return out;
            }
        }
    }
    panic!("timed out; events so far: {out:#?}");
}

fn run_done(run: &Id) -> impl FnMut(&Event, &[Event]) -> bool + '_ {
    move |e, _| matches!(e, Event::RunDone { run: r } if r == run)
}

fn outputs(events: &[Event]) -> Vec<(u64, Id)> {
    let mut v: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Output { seed, image, .. } => Some((*seed, image.clone())),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

const LOADED: Option<(&str, &[(&str, &str)])> = Some(("qwen-image-21", FILES));

#[test]
fn run_end_to_end_on_two_servers() {
    let s = setup();
    let (a, b) = (Fake::start(LOADED), Fake::start(LOADED));
    let run = new_run(&s, "qwen-image-21", 6, true);
    let engine = start(&s, &[&a, &b]);
    engine.queue_run(&run);
    let events = collect(&engine, run_done(&run));
    let outs = outputs(&events);
    assert_eq!(outs.iter().map(|(seed, _)| *seed).collect::<Vec<_>>(), [0, 1, 2, 3, 4, 5]);

    // both servers did work; each warmed up once first
    let (sa, sb) = (a.submissions(), b.submissions());
    assert!(!sa.is_empty() && !sb.is_empty(), "{} / {}", sa.len(), sb.len());
    assert_eq!(sa.len() + sb.len(), 6);
    assert_eq!(a.count("/img_gen") - sa.len(), 1, "one warm-up");
    let body = &sa[0];
    assert_eq!(body["output_format"], "png");
    assert_eq!(body["sample_params"]["guidance"]["txt_cfg"], 6.0);
    assert!(body.get("lora").is_none(), "none without LoRAs");
    assert!(
        body["prompt"].as_str().unwrap().starts_with("This is an RGBA image with transparency. cute robot sticker.")
    );
    let lib = s.lib.lock().unwrap();
    let ref_bytes = std::fs::read(lib.image_path(&ImageRef::input(s.ref_image.clone()))).unwrap();
    let sent = base64::engine::general_purpose::STANDARD.decode(body["ref_images"][0].as_str().unwrap()).unwrap();
    assert_eq!(sent, ref_bytes, "ref sent as the store PNG");

    // stored with recipe, linked as unreviewed, nothing pending
    assert!(lib.pending_seeds(&run).unwrap().is_empty());
    let ix = lib.scan().unwrap();
    let p = ix.project(&s.project).unwrap();
    assert_eq!(p.outputs.len(), 6);
    assert!(p.outputs.iter().all(|o| o.verdict.is_none()));
    let mut devices = std::collections::BTreeSet::new();
    for (seed, image) in &outs {
        let recipe = lib.recipe(image).unwrap().unwrap();
        assert_eq!(recipe.seed, *seed);
        assert!(recipe.alpha && recipe.prompt_sent.starts_with("This is an RGBA"));
        assert_eq!(recipe.prompt_original, "cute robot sticker");
        assert_eq!((recipe.sdcpp.as_str(), recipe.elapsed_s), ("fd21ecf", 33.0));
        devices.insert(recipe.device);
        let file = std::fs::read(lib.image_path(&ImageRef::output(image.clone()))).unwrap();
        assert!(png::text(&file, "parameters").unwrap().is_some(), "sd-server's chunk kept");
        assert!(image::load_from_memory(&file).unwrap().color().has_alpha());
    }
    assert_eq!(devices.into_iter().collect::<Vec<_>>(), ["ROCm0", "ROCm1"]);
    // each image's timing, in run.rsn: generating, then waiting before it
    let (_, r) = lib.find_run(&run).unwrap();
    let mut seeds: Vec<u64> = r.timings.iter().map(|t| t.seed).collect();
    seeds.sort();
    assert_eq!(seeds, [0, 1, 2, 3, 4, 5]);
    assert!(r.timings.iter().all(|t| t.elapsed_s == 33.0 && t.queued_s == 1.0 && !t.device.is_empty()));
    drop(lib);
    let report = common::verify(s.lib.lock().unwrap().root());
    assert!(report.trim_end().ends_with("problems: none"), "{report}");
}

#[test]
fn loads_the_model_a_run_needs() {
    let s = setup();
    let a = Fake::start(Some(("other-recipe", &[])));
    let run = new_run(&s, "qwen-image-21", 2, false);
    let engine = start(&s, &[&a]);
    engine.queue_run(&run);
    let events = collect(&engine, run_done(&run));
    assert_eq!(outputs(&events).len(), 2);
    let loads: Vec<_> =
        a.with(|st| st.log.iter().filter(|(_, p, _)| p.ends_with("models/load")).map(|(_, _, b)| b.clone()).collect());
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0]["recipe"], "qwen-image-21");
    assert_eq!(loads[0]["models"]["vae"], "vae/v.safetensors");
    assert!(events.iter().any(|e| matches!(e, Event::Server { state, .. } if state.starts_with("loading"))));
}

#[test]
fn sd_server_is_sent_the_runs_loras() {
    let s = setup();
    let a = Fake::start(LOADED);
    let run = new_run_with(&s, 1, |r| {
        r.model.loras = vec![
            LoraFile { id: "loras/style.safetensors".into(), sha256: String::new(), weight: 0.8 },
            LoraFile { id: "Lora/sub/x.gguf".into(), sha256: String::new(), weight: -0.5 },
        ];
    });
    let engine = start(&s, &[&a]);
    engine.queue_run(&run);
    collect(&engine, run_done(&run));
    assert_eq!(
        a.submissions()[0]["lora"],
        serde_json::json!([{ "path": "style.safetensors", "multiplier": 0.8 }, { "path": "sub/x.gguf", "multiplier": -0.5 }])
    );
}

#[test]
fn failed_seeds_are_recorded_and_the_run_finishes() {
    let s = setup();
    let a = Fake::start(LOADED);
    a.with(|st| st.fail_seeds = vec![1]);
    let run = new_run(&s, "qwen-image-21", 4, false);
    let engine = start(&s, &[&a]);
    engine.queue_run(&run);
    let events = collect(&engine, run_done(&run));
    let failed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::SeedFailed { seed, error, .. } => Some((*seed, error.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, 1);
    assert!(failed[0].1.contains("generation_failed"));
    assert_eq!(outputs(&events).len(), 3);
    let lib = s.lib.lock().unwrap();
    let (_, r) = lib.find_run(&run).unwrap();
    assert_eq!(r.failed.len(), 1);
    assert!(lib.pending_seeds(&run).unwrap().is_empty(), "a failed seed isn't pending");
}

#[test]
fn bad_request_fails_only_that_seed() {
    let s = setup();
    let a = Fake::start(LOADED);
    let run = new_run(&s, "qwen-image-21", 3, false);
    let mut opts = opts();
    opts.warm_up = false;
    opts.max_in_flight = 1;
    let engine = Engine::start(
        s.lib.clone(),
        vec![Endpoint { name: "gpu0".into(), device: "ROCm0".into(), client: Client::new(&a.url) }],
        opts,
        None,
    );
    a.with(|st| st.submit_errors.push_back((422, serde_json::json!({"error": "width must be a multiple of 32"}))));
    engine.queue_run(&run);
    let events = collect(&engine, run_done(&run));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::SeedFailed { seed: 0, error, .. } if error.contains("multiple of 32")))
    );
    assert_eq!(outputs(&events).iter().map(|(s, _)| *s).collect::<Vec<_>>(), [1, 2]);
}

#[test]
fn cancel_stops_a_run() {
    let s = setup();
    let a = Fake::start(LOADED);
    a.with(|st| st.steps = 1_000_000); // jobs never finish on their own
    let run = new_run(&s, "qwen-image-21", 8, false);
    let mut o = opts();
    o.warm_up = false;
    let engine = Engine::start(
        s.lib.clone(),
        vec![Endpoint { name: "gpu0".into(), device: "ROCm0".into(), client: Client::new(&a.url) }],
        o,
        None,
    );
    engine.queue_run(&run);
    collect(&engine, |e, _| matches!(e, Event::Progress { .. }));
    engine.cancel_run(&run, Stop::Now);
    let events = collect(&engine, run_done(&run));
    assert!(outputs(&events).is_empty());
    assert_eq!(a.count("/cancel"), 2, "both in-flight jobs cancelled");
    let lib = s.lib.lock().unwrap();
    let (_, r) = lib.find_run(&run).unwrap();
    assert_eq!(r.cancelled, [Seeds { first: 0, count: 8 }]);
    assert!(lib.pending_seeds(&run).unwrap().is_empty());
}

#[test]
fn finish_and_stop_keeps_the_images_being_generated() {
    let s = setup();
    let a = Fake::start(LOADED);
    a.with(|st| st.steps = 30);
    let run = new_run(&s, "qwen-image-21", 8, false);
    let mut o = opts();
    o.warm_up = false;
    let engine = Engine::start(
        s.lib.clone(),
        vec![Endpoint { name: "gpu0".into(), device: "ROCm0".into(), client: Client::new(&a.url) }],
        o,
        None,
    );
    engine.queue_run(&run);
    // the fake generates both in-flight jobs at once
    collect(&engine, |e, _| matches!(e, Event::Progress { step: 3.., .. }));
    engine.cancel_run(&run, Stop::Finish);
    let events = collect(&engine, run_done(&run));
    assert_eq!(outputs(&events).len(), 2, "both generating images finished");
    assert_eq!(a.count("/cancel"), 0);
    let lib = s.lib.lock().unwrap();
    let (_, r) = lib.find_run(&run).unwrap();
    assert_eq!(r.cancelled, [Seeds { first: 2, count: 6 }]);
    assert!(lib.pending_seeds(&run).unwrap().is_empty());
}

#[test]
fn finish_and_stop_cancels_the_waiting_job() {
    let s = setup();
    let a = Fake::start(LOADED);
    a.with(|st| {
        st.steps = 30;
        st.serial = true;
    });
    let run = new_run(&s, "qwen-image-21", 8, false);
    let mut o = opts();
    o.warm_up = false;
    let engine = Engine::start(
        s.lib.clone(),
        vec![Endpoint { name: "gpu0".into(), device: "ROCm0".into(), client: Client::new(&a.url) }],
        o,
        None,
    );
    engine.queue_run(&run);
    collect(&engine, |e, _| matches!(e, Event::Progress { step: 3.., .. }));
    engine.cancel_run(&run, Stop::Finish);
    let events = collect(&engine, run_done(&run));
    assert_eq!(outputs(&events).len(), 1, "the generating image finished");
    assert_eq!(a.count("/cancel"), 1, "the waiting one cancelled");
    let lib = s.lib.lock().unwrap();
    let (_, r) = lib.find_run(&run).unwrap();
    assert_eq!(r.cancelled, [Seeds { first: 1, count: 7 }]);
}

#[test]
fn a_failing_server_hands_its_seeds_to_the_other() {
    let s = setup();
    let (a, b) = (Fake::start(LOADED), Fake::start(LOADED));
    a.with(|st| st.steps = 20);
    let run = new_run(&s, "qwen-image-21", 6, false);
    // No warm-up: both servers are ready at once, so server 0 surely gets
    // jobs (with a warm-up, server 1 could finish everything first).
    let mut o = opts();
    o.warm_up = false;
    let servers = [&a, &b]
        .iter()
        .enumerate()
        .map(|(i, f)| Endpoint { name: format!("gpu{i}"), device: format!("ROCm{i}"), client: Client::new(&f.url) })
        .collect();
    let engine = Engine::start(s.lib.clone(), servers, o, None);
    engine.queue_run(&run);
    // once server 0 has a job running, it goes down for good
    collect(&engine, |e, _| matches!(e, Event::Progress { server: 0, .. }));
    a.with(|st| st.down = true);
    let events = collect(&engine, run_done(&run));
    assert!(events.iter().any(|e| matches!(e, Event::Server { server: 0, state, .. } if state.contains("retrying"))));
    let lib = s.lib.lock().unwrap();
    assert!(lib.pending_seeds(&run).unwrap().is_empty());
    let ix = lib.scan().unwrap();
    assert_eq!(ix.project(&s.project).unwrap().outputs.len(), 6);
}

#[test]
fn purged_job_is_generated_again() {
    let s = setup();
    let a = Fake::start(LOADED);
    a.with(|st| st.lose_seeds = vec![1]);
    let run = new_run(&s, "qwen-image-21", 2, false);
    let engine = start(&s, &[&a]);
    engine.queue_run(&run);
    let events = collect(&engine, run_done(&run));
    assert_eq!(outputs(&events).len(), 2);
    let seed1 = a.submissions().iter().filter(|b| b["seed"] == 1).count();
    assert_eq!(seed1, 2, "submitted again after the result was lost");
    assert_eq!(s.lib.lock().unwrap().store_ids(Store::Output).unwrap().len(), 2);
}

#[test]
fn requeue_after_adding_seeds() {
    let s = setup();
    let a = Fake::start(LOADED);
    let run = new_run(&s, "qwen-image-21", 2, false);
    let engine = start(&s, &[&a]);
    engine.queue_run(&run);
    collect(&engine, run_done(&run));
    s.lib.lock().unwrap().add_seeds(&run, 3, true).unwrap();
    engine.queue_run(&run);
    let events = collect(&engine, run_done(&run));
    assert_eq!(outputs(&events).iter().map(|(s, _)| *s).collect::<Vec<_>>(), [2, 3, 4]);
    let names: Vec<_> =
        s.lib.lock().unwrap().scan().unwrap().project(&s.project).unwrap().outputs.iter().map(|o| o.name.rev).collect();
    assert_eq!(names.iter().filter(|r| **r == 1).count(), 3, "new seeds in revision 1");
}

#[test]
fn renaming_and_moving_a_project_mid_run_keeps_every_output() {
    let s = setup();
    let a = Fake::start(LOADED);
    a.with(|st| st.steps = 15); // slow enough that the run outlives the moves
    let run = new_run(&s, "qwen-image-21", 6, false);
    let engine = start(&s, &[&a]);
    engine.queue_run(&run);
    // after the first image: rename the project, then move it into a folder
    let first = collect(&engine, |e, _| matches!(e, Event::Output { .. }));
    {
        let lib = s.lib.lock().unwrap();
        lib.rename_node(increart_core::Tree::Projects, Path::new("stickers"), "robots").unwrap();
        lib.create_folder(increart_core::Tree::Projects, Path::new(""), Some("clients")).unwrap();
        lib.move_node(increart_core::Tree::Projects, Path::new("robots"), Path::new("clients")).unwrap();
    }
    let mut events = first;
    events.extend(collect(&engine, run_done(&run)));
    assert_eq!(outputs(&events).len(), 6);
    let lib = s.lib.lock().unwrap();
    assert!(lib.pending_seeds(&run).unwrap().is_empty());
    let ix = lib.scan().unwrap();
    let p = ix.project(&s.project).unwrap();
    assert_eq!(p.dir, Path::new("projects/clients/robots"));
    assert_eq!(p.outputs.len(), 6, "images finished after the moves land in the new place");
    drop(lib);
    let report = common::verify(s.lib.lock().unwrap().root());
    assert!(report.trim_end().ends_with("problems: none"), "{report}");
}

// ------------------------------------------------------------- KoboldCpp

use common::fake_kobold::FakeKobold;
#[cfg(unix)]
use common::fake_kobold::export_script;
use increart_core::kobold_image::Admin;

/// The run's model files under a model folder, for KoboldCpp to find.
fn model_dir(dir: &Path) -> String {
    for (_, id) in FILES {
        let p = dir.join("models").join(id);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "").unwrap();
    }
    dir.join("models").to_string_lossy().into_owned()
}

fn kobold_engine(s: &Setup, k: &FakeKobold, admin: Option<Admin>) -> Engine {
    let client = Client::kobold(&k.url, "", admin);
    let ep = Endpoint { name: "kcpp".into(), device: "Vulkan1".into(), client };
    Engine::start(s.lib.clone(), vec![ep], opts(), None)
}

#[cfg(unix)]
fn admin(k: &FakeKobold, dir: &Path) -> Admin {
    let admin_dir = dir.join("admin");
    k.with(|st| st.admin = Some((admin_dir.clone(), "pw".into())));
    Admin {
        binary: export_script(dir),
        args: vec!["--port".into(), "5001".into(), "--nomodel".into(), "--admin".into()],
        dir: admin_dir,
        password: "pw".into(),
        model_dirs: vec![model_dir(dir)],
        log: None,
        swap: false,
    }
}

#[test]
#[cfg(unix)]
fn koboldcpp_loads_the_model_and_generates() {
    let s = setup();
    let tmp = tempfile::tempdir().unwrap();
    let k = FakeKobold::start("");
    let admin = admin(&k, tmp.path());
    let models = admin.model_dirs[0].clone();
    let run = new_run(&s, "qwen-image-21", 2, false);
    let engine = kobold_engine(&s, &k, Some(admin));
    engine.queue_run(&run);
    let events = collect(&engine, run_done(&run));
    assert_eq!(outputs(&events).iter().map(|(seed, _)| *seed).collect::<Vec<_>>(), [0, 1]);
    // loaded by a reload with the settings KoboldCpp wrote: launch args, model files
    assert_eq!(k.count("reload_config"), 1);
    assert_eq!(k.with(|st| st.sdmodel.clone()), format!("{models}/diffusion_models/q.safetensors"));
    let kcpps = std::fs::read_dir(tmp.path().join("admin")).unwrap().flatten().next().unwrap().path();
    let args = std::fs::read_to_string(kcpps).unwrap();
    assert!(args.starts_with("--port\n5001\n--admin\n--sdmodel\n"), "{args}");
    assert!(args.contains(&format!("--sdvae\n{models}/vae/v.safetensors\n")), "{args}");
    // A1111 bodies: the run's settings, refs as extra images
    let gens = k.generations();
    assert_eq!(gens.len(), 2);
    let (path, body) = &gens[0];
    assert_eq!(path, "/sdapi/v1/txt2img");
    assert_eq!((body["steps"].as_u64(), body["cfg_scale"].as_f64()), (Some(20), Some(6.0)));
    assert_eq!((body["sampler_name"].as_str(), body["flow_shift"].as_f64()), (Some("euler"), Some(3.0)));
    assert_eq!(body["extra_images"].as_array().unwrap().len(), 1);
    // previews with steps came through; recipes say what generated
    assert!(events.iter().any(|e| matches!(e, Event::Progress { step, total: 20, .. } if *step > 0)));
    let lib = s.lib.lock().unwrap();
    let recipe = lib.recipe(&outputs(&events)[0].1).unwrap().unwrap();
    assert_eq!((recipe.sdcpp.as_str(), recipe.device.as_str()), ("koboldcpp 1.123.1", "Vulkan1"));
}

#[test]
#[cfg(unix)]
fn koboldcpp_applies_the_runs_loras() {
    let s = setup();
    let tmp = tempfile::tempdir().unwrap();
    let k = FakeKobold::start("");
    let admin = admin(&k, tmp.path());
    let models = admin.model_dirs[0].clone();
    std::fs::create_dir_all(format!("{models}/loras")).unwrap();
    std::fs::write(format!("{models}/loras/style.safetensors"), "").unwrap();
    let run = new_run_with(&s, 1, |r| {
        r.model.loras = vec![LoraFile { id: "loras/style.safetensors".into(), sha256: "ab".into(), weight: 0.8 }];
    });
    let missing = new_run_with(&s, 1, |r| {
        r.model.loras = vec![LoraFile { id: "loras/none.safetensors".into(), sha256: String::new(), weight: 1.0 }];
    });
    let engine = kobold_engine(&s, &k, Some(admin));
    engine.queue_run(&run);
    let events = collect(&engine, run_done(&run));
    // launched with the loras folder; sent the path inside it
    let kcpps = std::fs::read_dir(tmp.path().join("admin")).unwrap().flatten().next().unwrap().path();
    assert!(std::fs::read_to_string(kcpps).unwrap().contains(&format!("--sdlora\n{models}/loras\n")));
    let gens = k.generations();
    assert_eq!(gens[0].1["lora"], serde_json::json!([{ "path": "style.safetensors", "multiplier": 0.8 }]));
    let lib = s.lib.lock().unwrap();
    let recipe = lib.recipe(&outputs(&events)[0].1).unwrap().unwrap();
    assert_eq!(recipe.model.loras[0].id, "loras/style.safetensors");
    drop(lib);
    // one it doesn't have: refused, said, not generated without it
    engine.queue_run(&missing);
    let events = collect(&engine, run_done(&missing));
    assert!(
        events.iter().any(
            |e| matches!(e, Event::SeedFailed { error, .. } if error.contains("doesn't have the LoRA none.safetensors"))
        ),
        "{events:?}"
    );
    assert_eq!(k.generations().len(), 1);
}

#[test]
#[cfg(unix)]
fn koboldcpp_swapping_takes_turns_with_prompt_enhance() {
    let s = setup();
    let tmp = tempfile::tempdir().unwrap();
    let k = FakeKobold::start("");
    k.with(|st| st.gen_ms = 400);
    let mut admin = admin(&k, tmp.path());
    admin.args.extend(["--model", "/m/chat.gguf", "--mmproj", "/m/proj.gguf"].map(String::from));
    admin.swap = true;
    let run = new_run(&s, "qwen-image-21", 2, false);
    let engine = kobold_engine(&s, &k, Some(admin));
    engine.queue_run(&run);
    collect(&engine, |e, _| matches!(e, Event::Progress { step, .. } if *step > 0));
    // an ask while an image generates waits for it, then has the chat model back
    let finished = k.with(|st| st.finished);
    let turn = increart_core::kobold_image::chat_turn(&k.url).unwrap().expect("a swapping server");
    assert_eq!(k.with(|st| (st.finished, st.sdmodel.clone())), (finished + 1, String::new()));
    let reloads = k.count("reload_config");
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(k.count("reload_config"), reloads, "no image while the ask holds its turn");
    drop(turn);
    let events = collect(&engine, run_done(&run));
    assert_eq!(outputs(&events).len(), 2);
    // the image model's settings again, without the chat model
    let sdmodel = k.with(|st| st.sdmodel.clone());
    assert!(sdmodel.ends_with("diffusion_models/q.safetensors"), "{sdmodel}");
    let kcpps = std::fs::read_dir(tmp.path().join("admin")).unwrap().flatten().next().unwrap().path();
    let args = std::fs::read_to_string(kcpps).unwrap();
    assert!(!args.contains("--model\n") && !args.contains("chat.gguf"), "{args}");
    assert!(increart_core::kobold_image::chat_turn("http://127.0.0.1:1").unwrap().is_none());
}

#[test]
fn koboldcpp_cancel_closes_the_generation() {
    let s = setup();
    let tmp = tempfile::tempdir().unwrap();
    let models = model_dir(tmp.path());
    let k = FakeKobold::start(&format!("{models}/diffusion_models/q.safetensors"));
    k.with(|st| st.gen_ms = 5_000);
    let run = new_run(&s, "qwen-image-21", 3, false);
    // started elsewhere, running the run's model: used without admin
    let mut o = opts();
    o.warm_up = false;
    let ep = Endpoint { name: "kcpp".into(), device: String::new(), client: Client::kobold(&k.url, "", None) };
    let engine = Engine::start(s.lib.clone(), vec![ep], o, None);
    engine.queue_run(&run);
    collect(&engine, |e, _| matches!(e, Event::Progress { .. }));
    let t = Instant::now();
    engine.cancel_run(&run, Stop::Now);
    let events = collect(&engine, run_done(&run));
    assert!(t.elapsed() < Duration::from_secs(3), "stopped, not waited out: {:?}", t.elapsed());
    assert!(outputs(&events).is_empty());
    // the second in flight may start between the two cancels: closed too
    assert!(k.count("2img") <= 2, "the third never sent");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(k.with(|st| st.finished), 0, "none ran to the end");
    assert_eq!(k.count("reload_config"), 0);
}

#[test]
fn koboldcpp_without_admin_keeps_its_model() {
    let s = setup();
    let k = FakeKobold::start("/models/other.safetensors");
    let run = new_run(&s, "qwen-image-21", 1, false);
    let engine = kobold_engine(&s, &k, None);
    engine.queue_run(&run);
    let events = collect(&engine, |e, _| matches!(e, Event::Problem(_)));
    let Some(Event::Problem(p)) = events.last() else { unreachable!() };
    assert!(p.contains("it runs other.safetensors and can't switch to q.safetensors"), "{p}");
    engine.cancel_run(&run, Stop::Now);
}

#[test]
#[cfg(unix)]
fn koboldcpp_failed_load_says_why() {
    let s = setup();
    let tmp = tempfile::tempdir().unwrap();
    let k = FakeKobold::start("");
    let mut admin = admin(&k, tmp.path());
    let log = tmp.path().join("k.log");
    std::fs::write(&log, "earlier: error loading\n").unwrap();
    admin.log = Some(log.clone());
    let model = format!("{}/diffusion_models/q.safetensors", admin.model_dirs[0]);
    k.with(|st| st.broken.push(model.clone()));
    // the reason lands in the log while it restarts
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        let mut f = std::fs::OpenOptions::new().append(true).open(log).unwrap();
        std::io::Write::write_all(&mut f, b"Cannot find image model file: q.safetensors\n").unwrap();
    });
    let client = Client::kobold(&k.url, "", Some(admin));
    let key = increart_core::sdserver::ModelKey::from(&model_sel());
    let err = client.load_model(&key).unwrap_err();
    writer.join().unwrap();
    assert_eq!(err.to_string(), "model load failed: Cannot find image model file: q.safetensors");
    assert_eq!(client.model_status().unwrap().state, "empty");
    drop(s);
}

#[cfg(unix)]
fn model_sel() -> ModelSelection {
    model("qwen-image-21")
}
