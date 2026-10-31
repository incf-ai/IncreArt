//! The suite against fake servers: a good preset passes, a broken proxy
//! fails where it's broken, the report is written and zipped; with the
//! `fake` feature, servers the suite starts itself, a taken port, and a
//! failover.

#[allow(dead_code)]
#[path = "../../increart-core/tests/common/fake.rs"]
mod fake;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use fake::Fake;
use increart_apitest::{Options, run};
use increart_core::apitest::{Event, Outcome};
use increart_core::config::{Config, RecipePreset, ServerConfig, ServerPreset};
use increart_core::pe::{PeRun, PeServer};
use serde_json::{Value, json};

const FILE: &str = "diffusion_models/apple.safetensors";

fn recipe() -> RecipePreset {
    RecipePreset {
        name: "Apple".into(),
        recipe: "qwen-image-21".into(),
        files: BTreeMap::from([("diffusion_model".to_string(), FILE.to_string())]),
        steps: 4,
        cfg: 2.0,
        flow_shift: None,
        custom_sigmas: vec![],
        loras: vec![],
    }
}

fn good_fake() -> Fake {
    let f = Fake::start(None);
    f.with(|s| {
        s.model_files = Some(vec![FILE.into()]);
        s.strict_files = true;
        s.sized = true;
    });
    f
}

/// An OpenAI-compatible chat server answering at once: "Red" when the
/// ask has an image.
fn fake_chat() -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    std::thread::spawn(move || {
        for mut rq in server.incoming_requests() {
            let mut body = String::new();
            let _ = rq.as_reader().read_to_string(&mut body);
            let answer = if rq.url().ends_with("/v1/models") {
                json!({"data": [{"id": "fake"}]})
            } else {
                let word = if body.contains("image_url") { "Red" } else { "Blue" };
                json!({"choices": [{"message": {"content": word}}]})
            };
            let h = tiny_http::Header::from_bytes("content-type", "application/json").unwrap();
            let _ = rq.respond(tiny_http::Response::from_string(answer.to_string()).with_header(h));
        }
    });
    url
}

fn connect(name: &str, url: &str) -> ServerConfig {
    ServerConfig::Connect { name: name.into(), url: url.into(), device: String::new() }
}

struct Ran {
    outcomes: HashMap<String, (Outcome, String)>,
    events: Vec<Event>,
    dir: tempfile::TempDir,
}

impl Ran {
    fn of(&self, id: &str) -> (Outcome, String) {
        self.outcomes.get(id).cloned().unwrap_or_else(|| panic!("no case {id}: {:?}", self.outcomes.keys()))
    }
}

/// One suite at a time: the traffic goes to one (a run is one process).
static ONE: Mutex<()> = Mutex::new(());

fn run_suite(config: Config, presets: Vec<ServerPreset>, cases: &[&str]) -> Ran {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let opts = Options {
        config,
        config_path: None,
        presets,
        cases: cases.iter().map(|s| s.to_string()).collect(),
        light: false,
        out: dir.path().join("report"),
    };
    let events = Arc::new(Mutex::new(vec![]));
    let sink = events.clone();
    run(&opts, &AtomicBool::new(false), Box::new(move |e| sink.lock().unwrap().push(e.clone())));
    let events = std::mem::take(&mut *events.lock().unwrap());
    let outcomes = events
        .iter()
        .filter_map(|e| match e {
            Event::End { id, outcome, summary, .. } => Some((id.clone(), (*outcome, summary.clone()))),
            _ => None,
        })
        .collect();
    Ran { outcomes, events, dir }
}

fn zip_names(path: &std::path::Path) -> Vec<String> {
    let z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    z.file_names().map(|n| n.unwrap().to_string()).collect()
}

fn config(presets: &[ServerPreset]) -> Config {
    Config { recipes: vec![recipe()], server_presets: presets.to_vec(), ..Default::default() }
}

#[test]
fn a_good_preset_passes_and_the_report_is_zipped() {
    let (a, b) = (good_fake(), good_fake());
    let chat = fake_chat();
    let preset = ServerPreset {
        name: "Two".into(),
        servers: vec![
            connect("a", &a.url),
            connect("b", &b.url),
            ServerConfig::Chat(PeServer { name: "pe".into(), run: PeRun::Connect { url: chat }, ..Default::default() }),
        ],
    };
    let ran = run_suite(config(std::slice::from_ref(&preset)), vec![preset], &[]);
    let bad: Vec<_> = ran.outcomes.iter().filter(|(_, (o, _))| matches!(o, Outcome::Fail | Outcome::Warn)).collect();
    assert!(bad.is_empty(), "{bad:#?}");
    assert_eq!(ran.of("Two/together.open").0, Outcome::Pass);
    assert_eq!(ran.of("Two/a/models.load-missing").0, Outcome::Pass, "{:?}", ran.of("Two/a/models.load-missing"));
    assert!(ran.of("Two/together.end-to-end").1.contains("all 4 made"), "{:?}", ran.of("Two/together.end-to-end"));
    assert_eq!(ran.of("Two/pe/prompt-enhance.vision").0, Outcome::Pass);
    // no LoRA, nothing started: n/a, saying why
    assert_eq!(ran.of("Two/a/generate.lora").0, Outcome::NotApplicable);
    assert!(ran.of("Two/together.failover").1.contains("started by the suite"));
    // each case got its own traffic
    let traffic =
        |id: &str| ran.events.iter().filter(|e| matches!(e, Event::Traffic { id: Some(i), .. } if i == id)).count();
    assert!(traffic("Two/b/jobs.cancel-running") > 2);
    assert_eq!(traffic("Two/a/generate.lora"), 0);
    // the report
    let Some(Event::Done { zip: Some(zip), counts, .. }) = ran.events.last() else { panic!("{:?}", ran.events.last()) };
    assert_eq!(counts.fail, 0);
    let md = std::fs::read_to_string(ran.dir.path().join("report/report.md")).unwrap();
    assert!(md.contains("### `Two/b/jobs.cancel-running`: pass"), "{md}");
    assert!(md.contains("POST /sdcpp/v1/jobs/"), "{md}");
    let json: Value =
        serde_json::from_str(&std::fs::read_to_string(ran.dir.path().join("report/report.json")).unwrap()).unwrap();
    assert_eq!(json["cases"].as_array().unwrap().len(), ran.outcomes.len());
    let names = zip_names(zip);
    assert!(names.contains(&"report/report.md".to_string()), "{names:?}");
    assert!(names.contains(&"report/traffic.log".to_string()), "{names:?}");
}

#[test]
fn a_broken_proxy_fails_where_it_is_broken() {
    let proxy = good_fake();
    proxy.with(|s| {
        // it answers a load with another model, never cancels, and has no list
        s.load_answer = Some(
            json!({"state": "ready", "recipe": "qwen-image-21", "models": {"diffusion_model": "/m/apple.safetensors"}}),
        );
        s.model_files = None;
    });
    let gone = Fake::start(None);
    gone.with(|s| s.down = true);
    let preset =
        ServerPreset { name: "Remote".into(), servers: vec![connect("proxy", &proxy.url), connect("gone", &gone.url)] };
    let ran = run_suite(
        config(std::slice::from_ref(&preset)),
        vec![preset],
        &["*/models.*", "*/generate.txt2img", "*/together.routing"],
    );
    let (o, why) = ran.of("Remote/proxy/models.load-each");
    assert_eq!(o, Outcome::Fail);
    assert!(why.contains("it answered") && why.contains("/m/apple.safetensors"), "{why}");
    assert_eq!(ran.of("Remote/proxy/models.list").0, Outcome::Warn);
    assert_eq!(ran.of("Remote/proxy/generate.txt2img").0, Outcome::Fail);
    // one not answering: open fails, its cases are skipped saying why
    assert_eq!(ran.of("Remote/together.open").0, Outcome::Fail);
    let (o, why) = ran.of("Remote/gone/models.status");
    assert_eq!(o, Outcome::Skipped);
    assert!(why.contains("didn't open"), "{why}");
    // only what was asked (and open) ran
    assert!(!ran.outcomes.contains_key("Remote/proxy/jobs.queue"));
}

#[test]
fn light_only_loads_nothing() {
    let a = good_fake();
    let preset = ServerPreset { name: "One".into(), servers: vec![connect("a", &a.url)] };
    let dir = tempfile::tempdir().unwrap();
    let opts = Options {
        config: config(std::slice::from_ref(&preset)),
        config_path: None,
        presets: vec![preset],
        cases: vec![],
        light: true,
        out: dir.path().join("r"),
    };
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let counts = run(&opts, &AtomicBool::new(false), Box::new(|_| {}));
    assert_eq!(counts.fail, 0);
    assert_eq!(a.count("/models/load"), 0);
    assert_eq!(a.count("/img_gen"), 0);
}

#[cfg(feature = "fake")]
mod started {
    use super::*;

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn start(name: &str, port: u16) -> ServerConfig {
        ServerConfig::Start {
            name: name.into(),
            host: "127.0.0.1".into(),
            port,
            device: String::new(),
            args: vec!["--fake-files".into(), FILE.into(), "--fake-steps".into(), "6".into()],
        }
    }

    fn started_config(preset: &ServerPreset) -> Config {
        let mut c = config(std::slice::from_ref(preset));
        c.sdserver.binary = env!("CARGO_BIN_EXE_increart-fake-sd-server").into();
        c
    }

    #[test]
    fn servers_it_starts_are_tested_and_a_run_survives_one_going() {
        let preset = ServerPreset {
            name: "Local".into(),
            servers: vec![start("gpu0", free_port()), start("gpu1", free_port())],
        };
        let ran = run_suite(started_config(&preset), vec![preset], &["*/together.*"]);
        let (o, why) = ran.of("Local/together.open");
        assert_eq!(o, Outcome::Pass, "{why}");
        assert!(why.contains("gpu0 started"), "{why}");
        let (o, why) = ran.of("Local/together.failover");
        assert_eq!(o, Outcome::Pass, "{why}");
        let names = zip_names(&ran.dir.path().join("report.zip"));
        assert!(names.iter().any(|n| n.starts_with("report/servers/sd-server-gpu0")), "{names:?}");
    }

    #[test]
    fn a_taken_port_is_said_and_nothing_is_started() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let preset = ServerPreset { name: "Local".into(), servers: vec![start("gpu0", port)] };
        let ran = run_suite(started_config(&preset), vec![preset], &["*/connect.capabilities"]);
        let (o, why) = ran.of("Local/together.open");
        assert_eq!(o, Outcome::Fail);
        assert!(why.contains("gpu0 (port") && why.contains("Close them"), "{why}");
        assert_eq!(ran.of("Local/gpu0/connect.capabilities").0, Outcome::Skipped);
    }
}
