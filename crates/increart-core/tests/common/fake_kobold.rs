//! An in-process fake KoboldCpp image server: `/api/extra/version`,
//! `/sdapi/v1/*` and admin `reload_config`, with shapes as the real one
//! answers (KoboldCpp 1.123.1). Generating blocks for `gen_ms`; a reload
//! "restarts" (answers 500 for `restart_ms`) and loads the `--sdmodel` of
//! the settings file, which [`export_script`] writes in place of KoboldCpp.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tiny_http::{Header, Method, Response, Server};

#[derive(Default)]
pub struct State {
    /// The image model file running ("": none).
    pub sdmodel: String,
    /// The admin dir and password; None: no admin.
    pub admin: Option<(PathBuf, String)>,
    /// Model files whose load fails (it comes back with none).
    pub broken: Vec<String>,
    pub gen_ms: u64,
    pub restart_ms: u64,
    pub down_until: Option<Instant>,
    /// Sampling step of the generation going on (0: none).
    pub step: u32,
    /// Every request: (method, path, JSON body).
    pub log: Vec<(String, String, Value)>,
    /// Generations that ran to the end.
    pub finished: u32,
    /// The LoRAs it found in the `--sdlora` folder when it loaded.
    pub loras: Vec<String>,
}

pub struct FakeKobold {
    pub url: String,
    pub state: Arc<Mutex<State>>,
    server: Arc<Server>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for FakeKobold {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A stand-in for the KoboldCpp binary that only exports: it writes its
/// arguments, one per line, to the `--exportconfig` file. (A shell script:
/// the tests that switch models run on Unix.)
#[cfg(unix)]
pub fn export_script(dir: &Path) -> String {
    let path = dir.join("fake-koboldcpp");
    std::fs::write(
        &path,
        "#!/bin/sh\nout=''\nprev=''\nfor a in \"$@\"; do [ \"$prev\" = --exportconfig ] && out=\"$a\"; prev=\"$a\"; done\nprintf '%s\\n' \"$@\" > \"$out\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

impl FakeKobold {
    pub fn start(sdmodel: &str) -> FakeKobold {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let state =
            Arc::new(Mutex::new(State { sdmodel: sdmodel.into(), gen_ms: 60, restart_ms: 150, ..Default::default() }));
        let (srv, st) = (server.clone(), state.clone());
        let thread = std::thread::spawn(move || {
            for mut req in srv.incoming_requests() {
                let st = st.clone();
                std::thread::spawn(move || {
                    let mut body = String::new();
                    let _ = req.as_reader().read_to_string(&mut body);
                    let json: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    let auth = req
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Authorization"))
                        .map(|h| h.value.as_str().to_string())
                        .unwrap_or_default();
                    let (status, out) = handle(&st, req.method(), req.url(), json, &auth);
                    let resp = Response::from_string(out.to_string())
                        .with_status_code(status)
                        .with_header(Header::from_bytes("content-type", "application/json").unwrap());
                    let _ = req.respond(resp);
                });
            }
        });
        FakeKobold { url, state, server, thread: Some(thread) }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    /// Generation bodies received (without the warm-up).
    pub fn generations(&self) -> Vec<(String, Value)> {
        self.with(|s| {
            s.log
                .iter()
                .filter(|(_, p, b)| p.contains("2img") && b["prompt"] != "warm-up")
                .map(|(_, p, b)| (p.clone(), b.clone()))
                .collect()
        })
    }

    pub fn count(&self, part: &str) -> usize {
        self.with(|s| s.log.iter().filter(|(_, p, _)| p.contains(part)).count())
    }
}

fn png_b64(seed: u64) -> String {
    use base64::Engine;
    let img = image::RgbImage::from_fn(8, 8, |x, y| image::Rgb([seed as u8, x as u8, y as u8]));
    let mut png = vec![];
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(png)
}

fn handle(st: &Mutex<State>, method: &Method, path: &str, body: Value, auth: &str) -> (u16, Value) {
    let mut s = st.lock().unwrap();
    s.log.push((method.to_string(), path.to_string(), body.clone()));
    if s.down_until.is_some_and(|t| Instant::now() < t) {
        return (500, json!({ "error": "restarting" }));
    }
    let has = !s.sdmodel.is_empty();
    match (method, path.split('?').next().unwrap()) {
        (Method::Get, "/api/extra/version") => (
            200,
            json!({"result": "KoboldCpp", "version": "1.123.1", "txt2img": has, "admin": if s.admin.is_some() { 2 } else { 0 }}),
        ),
        (Method::Get, "/sdapi/v1/sd-models") if has => {
            let name = Path::new(&s.sdmodel).file_stem().unwrap().to_string_lossy().into_owned();
            (200, json!([{"title": name, "model_name": name, "filename": s.sdmodel}]))
        }
        (Method::Get, "/sdapi/v1/sd-models") => (200, json!([])),
        (Method::Get, "/sdapi/v1/samplers") if has => (
            200,
            json!([{"name": "Euler", "aliases": ["euler", "k_euler"], "options": {}}, {"name": "Euler a", "aliases": ["euler_a"], "options": {}}]),
        ),
        (Method::Get, "/sdapi/v1/loras") => (
            200,
            s.loras
                .iter()
                .map(|p| json!({"name": p.rsplit_once('.').map_or(p.as_str(), |x| x.0), "path": p}))
                .collect(),
        ),
        (Method::Get, "/sdapi/v1/progress") => {
            let step = s.step;
            let state = json!({"job_count": 1, "sampling_step": step, "sampling_steps": 20});
            (
                200,
                json!({"progress": 0.5, "state": state, "current_image": if step > 0 && path.contains("skip_current_image=false") { json!(png_b64(1)) } else { Value::Null }}),
            )
        }
        (Method::Post, "/sdapi/v1/txt2img" | "/sdapi/v1/img2img") => {
            if !has {
                return (200, json!({"images": [""], "parameters": {}, "info": "{}"}));
            }
            let (gen_ms, seed) = (s.gen_ms, body["seed"].as_u64().unwrap_or(0));
            drop(s);
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(gen_ms) {
                st.lock().unwrap().step = (t.elapsed().as_millis() as u64 * 20 / gen_ms.max(1)) as u32 + 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            let mut s = st.lock().unwrap();
            s.step = 0;
            s.finished += 1;
            (200, json!({"images": [png_b64(seed)], "parameters": {}, "info": "{}"}))
        }
        (Method::Post, "/api/admin/reload_config") => {
            let Some((dir, pw)) = s.admin.clone() else { return (200, json!({"success": false})) };
            if auth != format!("Bearer {pw}") {
                return (200, json!({"success": false}));
            }
            let file = body["filename"].as_str().unwrap_or("");
            let args: Vec<String> = match file {
                "initial_model" => vec![],
                f => match std::fs::read_to_string(dir.join(f)) {
                    Ok(t) => t.lines().map(String::from).collect(),
                    Err(_) => return (200, json!({"success": false})),
                },
            };
            let model = args.iter().skip_while(|a| *a != "--sdmodel").nth(1).cloned().unwrap_or_default();
            s.sdmodel = if s.broken.contains(&model) { String::new() } else { model };
            let lora_dir = args.iter().skip_while(|a| *a != "--sdlora").nth(1).cloned();
            s.loras = lora_dir
                .and_then(|d| std::fs::read_dir(d).ok())
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            s.down_until = Some(Instant::now() + Duration::from_millis(s.restart_ms));
            (200, json!({"success": true}))
        }
        _ => (404, json!({"error": "not found"})),
    }
}
