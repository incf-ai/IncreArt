//! The sd-server client against the fake server, and the launcher against a
//! stand-in executable.

mod common;

#[cfg(unix)]
use std::time::Duration;

use common::fake::Fake;
#[cfg(unix)]
use increart_core::config::{Programs, SdServerConfig, ServerConfig};
#[cfg(unix)]
use increart_core::launcher::{self, LaunchError};
use increart_core::sdserver::{ApiError, Client, JobStatus, ModelKey};
use serde_json::json;

fn key(recipe: &str) -> ModelKey {
    ModelKey { recipe: recipe.into(), models: [("vae".to_string(), "vae/v.safetensors".to_string())].into() }
}

#[test]
fn job_lifecycle_and_errors() {
    let fake = Fake::start(None);
    let c = Client::new(&fake.url);
    assert!(c.capabilities().is_ok());
    assert_eq!(c.model_status().unwrap().loaded(), None);
    assert_eq!(c.img_gen(&json!({"prompt": "x", "seed": 1})), Err(ApiError::ModelNotLoaded));

    let st = c.load_model(&key("qwen-image-21")).unwrap();
    assert_eq!(st.loaded(), Some(key("qwen-image-21")));
    assert!(matches!(c.img_gen(&json!({"seed": 1})), Err(ApiError::BadRequest(_))));

    let id = c.img_gen(&json!({"prompt": "x", "seed": 3})).unwrap();
    assert_eq!(
        c.load_model(&key("other")).unwrap_err(),
        ApiError::ModelBusy("Finish or cancel pending generation jobs before switching models".into())
    );
    let mut job = c.job(&id).unwrap();
    while !job.status.is_terminal() {
        job = c.job(&id).unwrap();
    }
    assert_eq!(job.status, JobStatus::Completed);
    let png = job.image().unwrap();
    assert!(image::load_from_memory(&png).is_ok());
    assert_eq!(job.elapsed_s(), Some(33.0));

    // cancel while generating: stays generating with cancel_requested, then cancelled
    fake.with(|s| s.steps = 1000);
    let id = c.img_gen(&json!({"prompt": "x", "seed": 4})).unwrap();
    assert_eq!(c.job(&id).unwrap().status, JobStatus::Generating);
    let j = c.cancel(&id).unwrap();
    assert!(j.cancel_requested && j.status == JobStatus::Generating);
    let j = c.job(&id).unwrap();
    assert_eq!(j.status, JobStatus::Cancelled);
    assert_eq!(j.error.unwrap().code, "cancelled");

    assert_eq!(c.job("job_nope").unwrap_err(), ApiError::JobGone);
    fake.with(|s| s.submit_errors.push_back((429, json!({"error": "job queue is full"}))));
    assert_eq!(c.img_gen(&json!({"prompt": "x"})), Err(ApiError::QueueFull));
    fake.with(|s| s.unloadable = Some("broken".into()));
    assert!(matches!(c.load_model(&key("broken")), Err(ApiError::ModelLoadFailed(_))));
    assert_eq!(c.model_status().unwrap().loaded(), None, "a failed load leaves no model");
}

#[test]
fn unreachable_server_is_a_transport_error() {
    let c = Client::new("http://127.0.0.1:9");
    let e = c.model_status().unwrap_err();
    assert!(matches!(e, ApiError::Transport(_)) && e.is_server_problem(), "{e:?}");
}

#[cfg(unix)]
fn logs_for_slow() -> std::path::PathBuf {
    std::env::temp_dir().join("increart-test-logs")
}

#[cfg(unix)]
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
#[cfg(unix)] // the slow stand-in is a shell script
fn start_connect_and_stop() {
    let tmp = tempfile::tempdir().unwrap();
    // answers `capabilities` on `--listen-port` (tests/bin/sd-server.rs)
    let bin = env!("CARGO_BIN_EXE_increart-test-sd-server");
    let shared = Programs {
        model_dirs: vec!["/models".into()],
        sd_server: SdServerConfig { binary: bin.into(), ..Default::default() },
        ..Default::default()
    };
    let port = free_port();
    let start = ServerConfig::Start {
        name: "gpu0".into(),
        host: "127.0.0.1".into(),
        port,
        device: "ROCm0".into(),
        args: vec![],
    };
    let logs = tmp.path().join("logs");

    let mut proc = launcher::open(&start, &shared, &logs, Duration::from_secs(20)).unwrap();
    assert!(proc.launched());
    let argv = proc.client.capabilities().unwrap()["argv"].clone();
    assert_eq!(argv[5], "ROCm0");
    assert!(std::fs::read_to_string(logs.join("sd-server-gpu0.log")).unwrap().contains("listening"));

    // Connect, and Start on a port that already answers, both reuse it.
    let connect =
        ServerConfig::Connect { name: "same".into(), url: format!("http://127.0.0.1:{port}"), device: String::new() };
    let attached = launcher::open(&connect, &shared, &logs, Duration::from_secs(1)).unwrap();
    assert!(!attached.launched());
    assert!(!launcher::open(&start, &shared, &logs, Duration::from_secs(1)).unwrap().launched());
    drop(attached);
    assert!(proc.client.capabilities().is_ok(), "dropping an attached server leaves it running");

    proc.stop();
    assert!(proc.client.capabilities().is_err(), "stopped");
    assert!(matches!(
        launcher::open(&connect, &shared, &logs, Duration::from_secs(1)),
        Err(LaunchError::NotRunning { .. })
    ));

    // cancelling a start stops the process it launched
    let slow = tmp.path().join("slow");
    std::fs::write(&slow, "#!/bin/sh\nexec sleep 600\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let slow_cfg = Programs {
        sd_server: SdServerConfig { binary: slow.to_string_lossy().into(), ..Default::default() },
        ..Default::default()
    };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c2 = cancel.clone();
    let slow_server = ServerConfig::Start {
        name: "slow".into(),
        host: "127.0.0.1".into(),
        port: free_port(),
        device: "x".into(),
        args: vec![],
    };
    let started = std::time::Instant::now();
    let t = std::thread::spawn(move || {
        launcher::open_cancellable(&slow_server, &slow_cfg, &logs_for_slow(), Duration::from_secs(60), &c2)
    });
    std::thread::sleep(Duration::from_millis(500));
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(matches!(t.join().unwrap(), Err(LaunchError::Cancelled { .. })));
    assert!(started.elapsed() < Duration::from_secs(5), "cancelled promptly");

    let failing = ServerConfig::Start {
        name: "bad".into(),
        host: "127.0.0.1".into(),
        port: free_port(),
        device: "x".into(),
        args: vec!["--exit".into()],
    };
    match launcher::open(&failing, &shared, &logs, Duration::from_secs(20)) {
        Err(LaunchError::Exited { tail, .. }) => assert!(tail.contains("bad device"), "{tail}"),
        other => panic!("{other:?}"),
    }
}

#[test]
#[cfg(unix)]
fn a_started_server_that_crashes_says_how() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_increart-test-sd-server");
    let shared =
        Programs { sd_server: SdServerConfig { binary: bin.into(), ..Default::default() }, ..Default::default() };
    let server = ServerConfig::Start {
        name: "crashy".into(),
        host: "127.0.0.1".into(),
        port: free_port(),
        device: "x".into(),
        args: vec!["--crash-after".into(), "300".into()],
    };
    let mut proc = launcher::open(&server, &shared, &tmp.path().join("logs"), Duration::from_secs(20)).unwrap();
    assert_eq!(proc.exited(), None, "still running");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let exit = loop {
        if let Some(e) = proc.exited() {
            break e;
        }
        assert!(std::time::Instant::now() < deadline, "never exited");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(exit.status.contains("SIGABRT"), "{exit:?}");
    assert_eq!(exit.last.last().map(String::as_str), Some("generating"), "{exit:?}");
    assert!(!proc.launched() && proc.exited().is_none(), "said once");
}
