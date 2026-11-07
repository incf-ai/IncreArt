//! A model preset's Test against the fake sd-server.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use common::fake::Fake;
use increart_core::config::{PresetLora, RecipePreset};
use increart_core::presettest::{PresetTests, Stage, run, tried};
use increart_core::sdserver::Client;

fn preset() -> RecipePreset {
    RecipePreset {
        name: "Apple".into(),
        recipe: "qwen-image-21".into(),
        files: BTreeMap::from([("diffusion_model".to_string(), "apple-int8.safetensors".to_string())]),
        alternatives: BTreeMap::from([("diffusion_model".to_string(), vec!["apple-Q8_0.gguf".to_string()])]),
        steps: 3,
        cfg: 2.0,
        loras: vec![PresetLora { file: "loras/sketch.safetensors".into(), weight: 0.5, trigger: "sketch".into() }],
        ..Default::default()
    }
}

#[test]
fn it_loads_the_file_the_server_has_and_makes_the_image() {
    let a = Fake::start(None);
    a.with(|s| {
        s.model_files = Some(vec!["apple-Q8_0.gguf".into()]);
        s.strict_files = true;
        s.steps = 2;
    });
    let mut stages = vec![];
    let out = run(&Client::new(&a.url), &preset(), &AtomicBool::new(false), &mut |s| stages.push(s));
    assert_eq!(out.error, None);
    assert_eq!(stages, [Stage::Listing, Stage::Loading, Stage::Generating]);
    assert_eq!(out.files["diffusion_model"], "apple-Q8_0.gguf");
    assert!(out.load_s.is_some() && out.image_s.is_some());
    assert!(out.png.as_ref().is_some_and(|p| p.starts_with(b"\x89PNG")));
    // the request: its LoRA, its trigger before the prompt
    let body = a.with(|s| s.log.iter().rfind(|(_, p, _)| p.ends_with("/img_gen")).map(|(_, _, b)| b.clone())).unwrap();
    assert!(body["prompt"].as_str().unwrap().starts_with("sketch, "), "{body}");
    assert_eq!(body["lora"][0]["path"], "sketch.safetensors");
    // kept, with its image
    let dir = tempfile::tempdir().unwrap();
    let t = tried(&preset(), "sd0", out, Some(dir.path()));
    assert!(t.image.as_ref().is_some_and(|p| p.is_file()));
    let path = dir.path().join("preset-tests.rsn");
    let mut all = PresetTests::default();
    all.record(t.clone());
    all.save(&path).unwrap();
    assert_eq!(PresetTests::load(&path).tried, vec![t]);
}

#[test]
fn a_server_without_its_files_says_so_and_loads_nothing() {
    let a = Fake::start(None);
    a.with(|s| s.model_files = Some(vec!["other.safetensors".into()]));
    let out = run(&Client::new(&a.url), &preset(), &AtomicBool::new(false), &mut |_| {});
    let why = out.error.unwrap();
    assert!(why.starts_with("it can't load it: ") && why.contains("apple"), "{why}");
    assert_eq!(a.count("/models/load"), 0);
}

#[test]
fn a_failed_load_is_said() {
    let a = Fake::start(None);
    a.with(|s| {
        s.model_files = Some(vec!["apple-int8.safetensors".into()]);
        s.unloadable = Some("qwen-image-21".into());
    });
    let out = run(&Client::new(&a.url), &preset(), &AtomicBool::new(false), &mut |_| {});
    assert!(out.error.as_ref().is_some_and(|e| e.starts_with("loading failed")), "{:?}", out.error);
    assert_eq!(out.load_s, None);
}
