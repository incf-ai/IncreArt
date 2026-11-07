//! What a server at a URL is, asked as the setup wizard asks.

mod common;

use common::fake::Fake;
use common::fake_kobold::FakeKobold;
use increart_core::detect::{Kind, detect};

#[test]
fn an_sd_server_says_its_files() {
    let a = Fake::start(None);
    a.with(|st| st.model_files = Some(vec!["diffusion_models/q.safetensors".into(), "vae/v.safetensors".into()]));
    // pasted with its API path
    let d = detect(&format!("{}/sdcpp/v1/", a.url), "").unwrap();
    assert_eq!(d.kind, Kind::SdServer);
    assert!(d.images);
    assert_eq!(d.image_models.len(), 2);
    assert_eq!(d.summary(), "sd-server API · 2 model files listed (q.safetensors and 1 more)");
}

#[test]
fn a_koboldcpp_says_its_image_model() {
    let k = FakeKobold::start("/models/flux.gguf");
    let d = detect(&k.url, "").unwrap();
    assert_eq!(d.kind, Kind::KoboldCpp);
    assert_eq!(d.version.as_deref(), Some("1.123.1"));
    assert!(d.images);
    assert_eq!(d.image_models, ["/models/flux.gguf"]);
    let none = FakeKobold::start("");
    assert!(!detect(&none.url, "").unwrap().images);
}

#[test]
fn nothing_there_says_so() {
    // a port nothing listens on
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let e = detect(&free.to_string(), "").unwrap_err();
    assert!(e.starts_with("no answer"), "{e}");
}
