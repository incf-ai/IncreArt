//! Optional check against a real KoboldCpp, on the CPU only (no model is
//! loaded, so no GPU is touched). Skipped unless `INCREART_KOBOLDCPP` names
//! the binary. Starts it as an image server with admin on, asks for a model
//! that isn't there (KoboldCpp restarts, fails, recovers, and says why), and
//! checks stopping it stops all of its processes.

use std::time::Duration;

use increart_core::config::{Programs, ServerConfig};
use increart_core::kobold::{KoboldRun, KoboldServer};
use increart_core::pe::PeDevice;
use increart_core::sdserver::{ApiError, ModelKey};

#[test]
fn started_with_admin_and_stopped() {
    let Ok(binary) = std::env::var("INCREART_KOBOLDCPP") else {
        eprintln!("INCREART_KOBOLDCPP not set; skipping");
        return;
    };
    let state = tempfile::tempdir().unwrap();
    let logs = state.path().join("logs");
    let server = ServerConfig::Kobold(KoboldServer {
        name: "probe".into(),
        run: KoboldRun::Start {
            binary,
            config: String::new(),
            host: "127.0.0.1".into(),
            port: 5099,
            device: PeDevice::Cpu { threads: 4 },
            args: vec![],
        },
        chat: None,
        swap: false,
    });
    let shared = Programs { model_dirs: vec!["/nonexistent".into()], ..Default::default() };
    let proc = increart_core::launcher::open(&server, &shared, &logs, Duration::from_secs(120)).unwrap();
    assert!(proc.launched());
    let c = proc.client.clone();
    assert_eq!(c.model_status().unwrap().state, "empty");
    assert_eq!(c.software().as_deref().map(|s| s.starts_with("koboldcpp ")), Some(true));

    let key = ModelKey {
        recipe: "qwen-image-21".into(),
        models: [("diffusion_model".to_string(), "/nonexistent/q.safetensors".to_string())].into(),
    };
    match c.load_model(&key) {
        Err(ApiError::ModelLoadFailed(why)) => assert!(why.contains("Cannot find image model file"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(c.model_status().unwrap().state, "empty", "back as launched");
    assert!(std::fs::read_dir(state.path().join("koboldcpp/probe")).unwrap().count() >= 2, "password and settings");

    let log = proc.log.clone().unwrap();
    drop(proc);
    std::thread::sleep(Duration::from_secs(1));
    assert!(c.capabilities().is_err(), "stopped");
    let left = std::process::Command::new("pgrep").args(["-f", "koboldcpp.*--port 5099"]).output().unwrap();
    assert!(left.stdout.is_empty(), "every process stopped: {}", String::from_utf8_lossy(&left.stdout));
    assert!(std::fs::read_to_string(log).unwrap().contains("Reloading new model/config"));
}
