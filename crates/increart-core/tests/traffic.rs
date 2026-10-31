//! The servers' traffic log (its own process: it's one log for the app).

mod common;

use common::fake::Fake;
use increart_core::sdserver::Client;
use increart_core::traffic;
use serde_json::json;

#[test]
fn traffic_is_logged_when_on() {
    let dir = tempfile::tempdir().unwrap();
    traffic::start(dir.path());
    let a = Fake::start(Some(("qwen-image-21", &[("diffusion_model", "q.safetensors")])));
    a.with(|st| st.frozen = true);
    let c = Client::new(&a.url);
    c.model_status().unwrap();
    let log = dir.path().join(traffic::FILE);
    assert!(!log.exists(), "off: nothing written");

    traffic::set_on(true);
    let id = c.img_gen(&json!({"prompt": "a cat", "init_image": "A".repeat(3000)})).unwrap();
    for _ in 0..5 {
        c.job(&id).unwrap();
    }
    a.with(|st| st.frozen = false);
    c.job(&id).unwrap();
    a.with(|st| st.down = true);
    assert!(c.job(&id).is_err());
    traffic::set_on(false);
    c.model_status().unwrap_err();

    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].ends_with(" traffic log on"), "{text}");
    assert!(text.contains(&format!(" POST {}/sdcpp/v1/img_gen → 202 in ", a.url)), "{text}");
    assert!(text.contains(r#"  sent: {"init_image":"<3000 characters>","prompt":"a cat"}"#), "{text}");
    let polls: Vec<&&str> = lines.iter().filter(|l| l.contains(&format!("GET {}/sdcpp/v1/jobs/{id}", a.url))).collect();
    assert_eq!(polls.len(), 3, "the same answer once, the next with the count, the failure: {text}");
    assert!(polls[1].ends_with("(after 4 answers as the one before)"), "{text}");
    assert!(polls[2].contains("→ 500 in "), "{text}");
    assert!(text.contains(r#"  answer: {"error":"server_error"}"#), "{text}");
    assert!(lines.last().unwrap().ends_with(" traffic log off"), "{text}");
    assert!(!text.contains("models/status → 500"), "off again: not written");
}
