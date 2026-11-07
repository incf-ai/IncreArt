//! Model presets' **Test** against the in-process fake sd-server, through
//! the reducer and the real runtime.

#[allow(dead_code)]
#[path = "../../increart-core/tests/common/fake.rs"]
mod fake;

use std::collections::BTreeMap;
use std::time::Duration;

use fake::Fake;
use increart_core::config::{Config, RecipePreset, ServerConfig, ServerPreset};
use increart_core::presettest::PresetTests;
use increart_gui_core::*;

const INT8: &str = "diffusion_models/apple-int8.safetensors";
const GGUF: &str = "diffusion_models/apple-Q8_0.gguf";

fn preset() -> RecipePreset {
    RecipePreset {
        name: "Apple".into(),
        recipe: "qwen-image-21".into(),
        files: BTreeMap::from([("diffusion_model".to_string(), INT8.to_string())]),
        alternatives: BTreeMap::from([("diffusion_model".to_string(), vec![GGUF.to_string()])]),
        steps: 4,
        cfg: 2.0,
        ..Default::default()
    }
}

/// The app at the servers screen, a server preset of one server to connect
/// to (the fake, which has only the GGUF), results kept in `dir`.
fn state(url: &str, dir: &std::path::Path) -> AppState {
    let mut config = Config::default();
    config.normalize();
    config.recipes = vec![preset()];
    let server = ServerConfig::Connect { name: "a".into(), url: url.into(), device: String::new() };
    config.server_presets = vec![ServerPreset { name: "One".into(), servers: vec![server] }];
    config.last_preset = Some("One".into());
    let mut s = AppState::new(config, None);
    s.preset_tests_path = Some(dir.join("preset-tests.rsn"));
    s
}

/// Sends `msg`, runs its effects and the answers until `done` holds.
fn drive(s: &mut AppState, rt: &Runtime, msg: Msg, done: impl Fn(&AppState) -> bool) {
    let mut pending = vec![msg];
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        for m in pending.drain(..) {
            for f in update(s, m) {
                rt.run(f);
            }
        }
        if done(s) {
            // what's left to write (the results file)
            std::thread::sleep(Duration::from_millis(50));
            return;
        }
        assert!(std::time::Instant::now() < deadline, "not done: {:?}", s.preset_test);
        pending = rt.wait(Duration::from_millis(50));
    }
}

fn fake() -> Fake {
    let a = Fake::start(None);
    a.with(|s| {
        s.model_files = Some(vec![GGUF.into()]);
        s.strict_files = true;
        s.steps = 2;
    });
    a
}

#[test]
fn an_unsaved_preset_is_tested_on_the_file_the_server_has_and_kept() {
    let a = fake();
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::new(|| {}).unwrap();
    rt.set_log_dir(dir.path().join("logs"));
    let mut s = state(&a.url, dir.path());
    assert_eq!(s.preset_test_servers(), ("One".to_string(), s.config.server_presets[0].servers.clone()));
    update(&mut s, Msg::OpenRecipes);
    // an edit not saved: tested as edited
    let mut e = s.recipe_editor.clone().unwrap();
    e.rows[0].preset.steps = 6;
    update(&mut s, Msg::EditRecipes(e));
    drive(&mut s, &rt, Msg::TestPreset(0, "a".into()), |s| s.preset_test.as_ref().is_some_and(|t| t.result.is_some()));
    let t = s.preset_test.as_ref().unwrap().result.clone().unwrap();
    assert_eq!(t.error, None);
    assert_eq!(t.files["diffusion_model"], GGUF);
    assert!(t.load_s.is_some() && t.image_s.is_some());
    assert!(t.image.as_ref().is_some_and(|p| p.is_file() && p.starts_with(dir.path().join("preset-tests"))));
    let sent = a.with(|s| s.log.iter().rfind(|(_, p, _)| p.ends_with("/img_gen")).map(|(_, _, b)| b.clone())).unwrap();
    assert_eq!(sent["sample_params"]["sample_steps"], 6);
    // kept, and current only for the preset as edited
    let kept = PresetTests::load(&dir.path().join("preset-tests.rsn"));
    assert_eq!(kept.tried, vec![t.clone()]);
    assert!(!t.current(&s.config.recipes[0]));
    // saved under another name: the result follows, still current
    let mut e = s.recipe_editor.clone().unwrap();
    e.rows[0].preset.name = "Red apple".into();
    update(&mut s, Msg::EditRecipes(e));
    for f in update(&mut s, Msg::SaveRecipes) {
        rt.run(f);
    }
    let t = s.preset_tests.of("Red apple").next().cloned().unwrap();
    assert!(t.current(&s.config.recipes[0]));
    assert_eq!(s.preset_tests.of("Apple").count(), 0);
}

#[test]
fn a_server_without_its_files_fails_saying_why_and_a_test_can_be_stopped() {
    let a = fake();
    a.with(|s| s.model_files = Some(vec!["other.gguf".into()]));
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::new(|| {}).unwrap();
    rt.set_log_dir(dir.path().join("logs"));
    let mut s = state(&a.url, dir.path());
    update(&mut s, Msg::OpenRecipes);
    drive(&mut s, &rt, Msg::TestPreset(0, "a".into()), |s| s.preset_test.as_ref().is_some_and(|t| t.result.is_some()));
    let t = s.preset_tests.of("Apple").next().unwrap();
    assert!(t.error.as_ref().is_some_and(|e| e.contains("can't load it")), "{t:?}");
    assert_eq!(a.count("/models/load"), 0);
    // a slow image, stopped: said so, the job cancelled
    a.with(|s| {
        s.model_files = Some(vec![GGUF.into()]);
        s.steps = 10_000;
    });
    let fx = update(&mut s, Msg::TestPreset(0, "a".into()));
    fx.into_iter().for_each(|f| rt.run(f));
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while a.count("/img_gen") == 0 {
        assert!(std::time::Instant::now() < deadline, "no image asked");
        for m in rt.wait(Duration::from_millis(20)) {
            update(&mut s, m).into_iter().for_each(|f| rt.run(f));
        }
    }
    // a second test meanwhile is refused
    update(&mut s, Msg::TestPreset(0, "a".into()));
    assert!(s.messages.iter().any(|m| m.error && m.text.contains("under way")));
    drive(&mut s, &rt, Msg::StopPresetTest, |s| s.preset_test.as_ref().is_some_and(|t| t.result.is_some()));
    let t = s.preset_test.as_ref().unwrap().result.clone().unwrap();
    assert_eq!(t.error.as_deref(), Some("stopped"));
    assert!(a.count("/cancel") > 0);
}

#[test]
fn the_server_preset_being_edited_is_offered_and_opening_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = state("http://127.0.0.1:9", dir.path());
    update(&mut s, Msg::LibraryOpened(Ok(("/lib".into(), increart_core::Index::default()))));
    assert_eq!(s.preset_test_servers().0, "One");
    // the preset being edited on the servers screen
    let Phase::Servers(d) = &mut s.phase else { panic!("{:?}", s.phase) };
    {
        d.editing = Some(Editing::New);
        d.draft = ServerPreset {
            name: "Draft".into(),
            servers: vec![ServerConfig::Connect {
                name: "b".into(),
                url: "http://127.0.0.1:8".into(),
                device: String::new(),
            }],
        };
    }
    assert_eq!(s.preset_test_servers().0, "Draft");
    if let Phase::Servers(d) = &mut s.phase {
        d.opening = true;
    }
    let b = s.preset_test_servers().1[0].clone();
    assert_eq!(b.name(), "b");
    assert!(s.why_no_preset_test(&b).is_some_and(|w| w.contains("being opened")));
}

#[test]
fn a_server_preset_covers_the_model_presets_its_servers_find_files_for() {
    use increart_core::kobold::{KoboldRun, KoboldServer};
    use increart_core::pe::PeDevice;
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models");
    std::fs::create_dir_all(models.join("diffusion_models")).unwrap();
    // an int8 safetensors (sd-server's) and a GGUF of another model
    let header = r#"{"w":{"dtype":"I8","shape":[2],"data_offsets":[0,2]}}"#;
    let mut b = (header.len() as u64).to_le_bytes().to_vec();
    b.extend(header.as_bytes());
    std::fs::write(models.join(INT8), b).unwrap();
    std::fs::write(models.join("diffusion_models/pear.gguf"), b"GGUF").unwrap();
    let mut s = state("http://127.0.0.1:9", dir.path());
    s.config.programs.model_dirs = vec![models.to_string_lossy().into_owned()];
    let pear = RecipePreset {
        name: "Pear".into(),
        files: BTreeMap::from([("diffusion_model".to_string(), "diffusion_models/pear.gguf".to_string())]),
        ..preset()
    };
    let gone = RecipePreset {
        name: "Gone".into(),
        files: BTreeMap::from([("diffusion_model".to_string(), "diffusion_models/gone.gguf".to_string())]),
        ..preset()
    };
    s.config.recipes = vec![preset(), pear, gone];
    let sd = ServerConfig::Start {
        name: "sd0".into(),
        host: "127.0.0.1".into(),
        port: 1,
        device: "ROCm0".into(),
        args: vec![],
    };
    let kcpp = ServerConfig::Kobold(KoboldServer {
        name: "kcpp".into(),
        run: KoboldRun::Start {
            binary: "koboldcpp".into(),
            config: String::new(),
            host: "127.0.0.1".into(),
            port: 2,
            device: PeDevice::default(),
            args: vec![],
        },
        chat: None,
        swap: false,
    });
    let remote =
        ServerConfig::Connect { name: "remote".into(), url: "http://127.0.0.1:3".into(), device: String::new() };
    let both = ServerPreset { name: "Both".into(), servers: vec![sd, kcpp.clone(), remote] };
    // not scanned yet: nothing said; the scan asked once
    assert_eq!(s.coverage(&both), None);
    let rt = Runtime::new(|| {}).unwrap();
    drive(&mut s, &rt, Msg::ScanModelFolders, |s| s.local_files.is_some());
    assert!(update(&mut s, Msg::ScanModelFolders).is_empty());
    let c = s.coverage(&both).unwrap();
    // the int8 file: KoboldCpp's second choice (at full size); the GGUF: both
    assert_eq!(c.can[0].0, "Apple");
    assert_eq!(c.can[0].1[0], "sd0");
    assert!(
        c.can[0].1[1].starts_with("kcpp (second choice: apple-int8.safetensors is in sd-server's int8"),
        "{:?}",
        c.can
    );
    assert_eq!(c.can[1], ("Pear".to_string(), vec!["sd0".to_string(), "kcpp".to_string()]));
    assert_eq!(c.cannot.len(), 1);
    assert_eq!(c.cannot[0].0, "Gone");
    assert!(c.cannot[0].1.iter().any(|w| w.starts_with("sd0: ") && w.contains("gone.gguf")), "{:?}", c.cannot);
    assert_eq!(c.asked, vec!["remote".to_string()]);
    assert_eq!(c.line(), "Model presets: 2 of 3 from the files here, + what remote lists once open");
    let alone = ServerPreset { name: "K".into(), servers: vec![kcpp] };
    assert_eq!(s.coverage(&alone).unwrap().can.len(), 2);
    // other folders: scanned again
    s.config.programs.model_dirs.push(dir.path().to_string_lossy().into_owned());
    assert!(s.local_files_stale());
}
