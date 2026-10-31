//! Test (a server the app connects to) against the fake sd-server.

mod common;

use std::sync::atomic::AtomicBool;

use common::fake::Fake;
use increart_core::sdserver::{Client, ModelKey};
use increart_core::servertest::{Outcome, Step, as_text, run};
use serde_json::json;

fn models() -> Vec<(String, ModelKey)> {
    let key =
        |file: &str| ModelKey { recipe: "krea2".into(), models: [("diffusion_model".into(), file.into())].into() };
    vec![("krea2".into(), key("krea2.safetensors")), ("other".into(), key("missing.safetensors"))]
}

fn test(fake: &Fake, generate: Result<(), String>) -> Vec<Step> {
    test_stopping_at(fake, generate, usize::MAX)
}

/// Stops it as step `stop_at` starts.
fn test_stopping_at(fake: &Fake, generate: Result<(), String>, stop_at: usize) -> Vec<Step> {
    let mut steps: Vec<Step> = vec![];
    let stop = AtomicBool::new(false);
    run(&Client::new(&fake.url), &models(), generate, &stop, &mut |i, s| {
        if i == stop_at {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if i == steps.len() {
            steps.push(s);
        } else {
            steps[i] = s;
        }
    });
    steps
}

fn outcomes(steps: &[Step]) -> Vec<Outcome> {
    steps.iter().map(|s| s.outcome).collect()
}

#[test]
fn a_good_server_passes_every_step() {
    let a = Fake::start(None);
    a.with(|st| {
        st.model_files = Some(vec!["krea2.safetensors".into()]);
        st.steps = 3;
    });
    let steps = test(&a, Ok(()));
    let text = as_text("proxy", &steps);
    assert_eq!(outcomes(&steps), [Outcome::Ok; 6], "{text}");
    assert!(
        steps[1].detail.contains("model presets it can load: krea2; not other: it doesn't have missing.safetensors"),
        "{text}"
    );
    assert!(steps[4].detail.contains("generating → completed"), "{text}");
    assert!(steps[5].detail.contains("cancelled"), "{text}");
    assert!(text.starts_with("Test of proxy\nAnswers: ok: sd-server's API at http://"), "{text}");
}

#[test]
fn a_load_answered_otherwise_fails_and_says_both() {
    let a = Fake::start(None);
    a.with(|st| {
        st.model_files = Some(vec!["krea2.safetensors".into()]);
        st.load_answer =
            Some(json!({"state": "ready", "recipe": "krea2", "models": {"diffusion_model": "/m/krea2.safetensors"}}));
    });
    let steps = test(&a, Ok(()));
    let text = as_text("proxy", &steps);
    use Outcome::{Failed, Skipped};
    let ok = Outcome::Ok;
    assert_eq!(outcomes(&steps), [ok, ok, ok, Failed, Skipped, Skipped], "{text}");
    assert!(steps[3].detail.contains(r#"it answered {"models":{"diffusion_model":"/m/krea2.safetensors"}"#), "{text}");
    assert_eq!(a.count("/img_gen"), 0);
}

#[test]
fn without_a_list_or_with_generating_off() {
    let a = Fake::start(None);
    let steps = test(&a, Err("the engine uses it".into()));
    use Outcome::{Failed, Skipped, Warning};
    let ok = Outcome::Ok;
    assert_eq!(outcomes(&steps), [ok, Warning, ok, Skipped, Skipped, Skipped]);
    assert!(steps[1].detail.contains("no GET /sdcpp/v1/model-files (404)"), "{}", steps[1].detail);
    assert_eq!(steps[3].detail, "the engine uses it");
    assert_eq!(a.count("/models/load"), 0);
    // not answering
    a.with(|st| st.down = true);
    let steps = test(&a, Ok(()));
    assert_eq!(outcomes(&steps), [Failed, Skipped, Skipped, Skipped, Skipped, Skipped]);
}

#[test]
fn stopped_during_the_image_starts_no_other_job() {
    let a = Fake::start(None);
    a.with(|st| {
        st.model_files = Some(vec!["krea2.safetensors".into()]);
        st.steps = 50;
    });
    let steps = test_stopping_at(&a, Ok(()), 4);
    let ok = Outcome::Ok;
    assert_eq!(outcomes(&steps), [ok, ok, ok, ok, Outcome::Skipped, Outcome::Skipped], "{}", as_text("a", &steps));
    assert_eq!(steps[5].detail, "stopped");
    assert_eq!(a.count("/img_gen"), 1);
}
