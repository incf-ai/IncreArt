//! Optional checks against a real sd-server. Skipped unless
//! `INCREART_SDSERVER_URL` is set (e.g. `http://127.0.0.1:1234`). With
//! `INCREART_SDSERVER_GENERATE=1` it also runs one small job with whatever
//! model is loaded.
#![allow(clippy::disallowed_methods)] // fixtures on disk: no overlay here

use std::time::{Duration, Instant};

use increart_core::sdserver::{Client, JobStatus};

fn client() -> Option<Client> {
    match std::env::var("INCREART_SDSERVER_URL") {
        Ok(url) if !url.is_empty() => Some(Client::new(&url)),
        _ => {
            eprintln!("INCREART_SDSERVER_URL not set; skipping");
            None
        }
    }
}

#[test]
fn catalog_and_status() {
    let Some(c) = client() else { return };
    let caps = c.capabilities().unwrap();
    assert!(caps["supported_modes"].as_array().is_some_and(|m| m.iter().any(|x| x == "img_gen")));
    let files = c.model_files().unwrap();
    eprintln!("{} model files; status {:?}", files.len(), c.model_status().unwrap());
}

#[test]
fn one_small_job() {
    let Some(c) = client() else { return };
    if std::env::var("INCREART_SDSERVER_GENERATE").as_deref() != Ok("1") {
        return;
    }
    let status = c.model_status().unwrap();
    assert!(status.loaded().is_some(), "load a model first: {status:?}");
    let id = c
        .img_gen(&serde_json::json!({
            "prompt": "a red apple on a table", "width": 512, "height": 512, "seed": 0,
            "sample_params": {"sample_steps": 4}, "output_format": "png",
        }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let job = c.job(&id).unwrap();
        if job.status.is_terminal() {
            assert_eq!(job.status, JobStatus::Completed, "{job:?}");
            let png = job.image().unwrap();
            let img = image::load_from_memory(&png).unwrap();
            assert_eq!((img.width(), img.height()), (512, 512));
            eprintln!("generated in {:?} s", job.elapsed_s());
            return;
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(500));
    }
}
