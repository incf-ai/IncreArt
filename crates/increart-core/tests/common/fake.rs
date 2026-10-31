//! An in-process fake sd-server (`/sdcpp/v1/...`) for engine and client
//! tests. Responses follow the shapes recorded from the real server
//! (tmp/job.json, tmp/job-result.json); behaviour is scripted through
//! [`Fake`]'s fields.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use base64::Engine;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Response, Server};

#[derive(Clone, Debug)]
pub struct FakeJob {
    pub status: &'static str,
    pub polls: u32,
    pub cancel_requested: bool,
    pub body: Value,
    pub created: i64,
}

#[derive(Default)]
pub struct State {
    /// `(recipe, slot → file id)` once loaded.
    pub loaded: Option<(String, BTreeMap<String, String>)>,
    pub jobs: HashMap<String, FakeJob>,
    pub next: u32,
    /// Every request: (method, path, JSON body).
    pub log: Vec<(String, String, Value)>,
    /// Polls a job takes from `generating` to `completed`.
    pub steps: u32,
    /// Answer every request with 500 (the server is "down").
    pub down: bool,
    /// Scripted answers for the next `img_gen` submissions.
    pub submit_errors: VecDeque<(u16, Value)>,
    /// Seeds whose job fails (`generation_failed`).
    pub fail_seeds: Vec<u64>,
    /// Seeds whose job is purged (404) as soon as it completes, once.
    pub lose_seeds: Vec<u64>,
    /// `models/load` answers 422 `model_load_failed` for this recipe.
    pub unloadable: Option<String>,
    /// One job generates at a time, the others wait queued (as sd-server).
    pub serial: bool,
    /// `model-files` lists these IDs (`None`: 404, as before it had one).
    pub model_files: Option<Vec<String>>,
    /// A cancel is taken but the job generates on, never ending.
    pub ignore_cancel: bool,
    /// `model-files` answers 500 (the rest answers).
    pub list_down: bool,
    /// Jobs stay as they are (queued stays queued), never ending.
    pub frozen: bool,
    /// `models/load` answers this as the status, whatever was asked.
    pub load_answer: Option<Value>,
}

pub struct Fake {
    pub url: String,
    pub state: Arc<Mutex<State>>,
    server: Arc<Server>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Fake {
    pub fn start(loaded: Option<(&str, &[(&str, &str)])>) -> Fake {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let state = Arc::new(Mutex::new(State {
            loaded: loaded
                .map(|(r, m)| (r.to_string(), m.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())),
            steps: 2,
            ..Default::default()
        }));
        let (srv, st) = (server.clone(), state.clone());
        let thread = std::thread::spawn(move || {
            for mut req in srv.incoming_requests() {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let json: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let method = req.method().clone();
                let path = req.url().to_string();
                let (status, out) = handle(&mut st.lock().unwrap(), &method, &path, json);
                let resp = Response::from_string(out.to_string())
                    .with_status_code(status)
                    .with_header(Header::from_bytes("content-type", "application/json").unwrap());
                let _ = req.respond(resp);
            }
        });
        Fake { url, state, server, thread: Some(thread) }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    /// `img_gen` request bodies received (without the warm-up).
    pub fn submissions(&self) -> Vec<Value> {
        self.with(|s| {
            s.log
                .iter()
                .filter(|(_, p, b)| p.ends_with("/img_gen") && b["prompt"] != "warm-up")
                .map(|(_, _, b)| b.clone())
                .collect()
        })
    }

    pub fn count(&self, suffix: &str) -> usize {
        self.with(|s| s.log.iter().filter(|(_, p, _)| p.ends_with(suffix)).count())
    }
}

fn err(status: u16, code: &str, msg: &str) -> (u16, Value) {
    (status, json!({ "error": msg, "code": code }))
}

fn model_status(s: &State) -> Value {
    match &s.loaded {
        Some((r, m)) => {
            json!({"state": "ready", "recipe": r, "models": m, "error": null, "revision": 2, "load_elapsed_seconds": 1.5})
        }
        None => {
            json!({"state": "empty", "recipe": null, "models": {}, "error": null, "revision": 0, "load_elapsed_seconds": null})
        }
    }
}

/// The PNG sd-server would return: a small image coloured by the seed, RGBA
/// for an RGBA prompt, with its own `parameters` chunk.
pub fn result_png(body: &Value) -> Vec<u8> {
    let seed = body["seed"].as_u64().unwrap_or(0);
    let alpha = body["prompt"].as_str().unwrap_or("").starts_with("This is an RGBA image");
    let (w, h) = (32, 32);
    let img = if alpha {
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([seed as u8, x as u8 * 8, y as u8 * 8, 128])
        }))
    } else {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([seed as u8, x as u8 * 8, y as u8 * 8])
        }))
    };
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let params = format!("{}\nSteps: 20, Seed: {seed}, Size: {w}x{h}", body["prompt"].as_str().unwrap_or(""));
    increart_core::png::set_chunks(&png, &[increart_core::png::Chunk::text("parameters", &params)]).unwrap()
}

fn job_json(id: &str, j: &FakeJob, result: Option<Value>, error: Value) -> Value {
    let done = matches!(j.status, "completed" | "failed" | "cancelled");
    json!({
        "id": id, "kind": "img_gen", "status": j.status,
        "created": j.created, "started": if j.status == "queued" { Value::Null } else { json!(j.created + 1000) },
        "completed": if done { json!(j.created + 34_000) } else { Value::Null },
        "queue_position": 0, "cancel_requested": j.cancel_requested,
        "result": result.unwrap_or(Value::Null), "error": error,
        "preview": if j.status == "generating" { json!({"pass": 1, "step": j.polls, "total_steps": 20, "b64_json": ""}) } else { Value::Null },
    })
}

fn handle(s: &mut State, method: &Method, path: &str, body: Value) -> (u16, Value) {
    s.log.push((method.to_string(), path.to_string(), body.clone()));
    if s.down {
        return (500, json!({"error": "server_error"}));
    }
    let Some(rest) = path.strip_prefix("/sdcpp/v1/") else { return (404, json!({"error": "not found"})) };
    let pending = s.jobs.values().filter(|j| matches!(j.status, "queued" | "generating")).count();
    match (method, rest) {
        (Method::Get, "capabilities") => (200, json!({"supported_modes": ["img_gen"], "runtime": model_status(s)})),
        (Method::Get, "models/status") => (200, model_status(s)),
        (Method::Get, "model-files") if s.list_down => (500, json!({"error": "server_error"})),
        (Method::Get, "model-files") => match &s.model_files {
            Some(ids) => (200, json!({"files": ids.iter().map(|id| json!({"id": id})).collect::<Vec<_>>()})),
            None => (404, json!({"error": "not found"})),
        },
        (Method::Post, "models/load") => {
            if pending > 0 {
                return err(409, "model_busy", "Finish or cancel pending generation jobs before switching models");
            }
            let recipe = body["recipe"].as_str().unwrap_or("").to_string();
            if s.unloadable.as_deref() == Some(&recipe) {
                s.loaded = None;
                return err(422, "model_load_failed", "Model initialization failed");
            }
            if let Some(answer) = &s.load_answer {
                return (200, answer.clone());
            }
            let models = serde_json::from_value(body["models"].clone()).unwrap_or_default();
            s.loaded = Some((recipe, models));
            (200, model_status(s))
        }
        (Method::Post, "img_gen") => {
            if let Some((status, out)) = s.submit_errors.pop_front() {
                return (status, out);
            }
            if s.loaded.is_none() {
                return err(503, "model_not_loaded", "No model is loaded");
            }
            if !body["prompt"].is_string() {
                return (400, json!({"error": "invalid json", "message": "prompt missing"}));
            }
            let id = format!("job_fake_{:08}", s.next);
            s.next += 1;
            let job = FakeJob { status: "queued", polls: 0, cancel_requested: false, body, created: 1792785695370 };
            s.jobs.insert(id.clone(), job);
            (
                202,
                json!({"id": id, "kind": "img_gen", "status": "queued", "created": 1792785695370_i64, "poll_url": format!("/sdcpp/v1/jobs/{id}")}),
            )
        }
        (Method::Get, p) if p.starts_with("jobs/") => {
            let id = &p[5..];
            let steps = s.steps;
            let busy = s.serial && s.jobs.iter().any(|(k, j)| k != id && j.status == "generating");
            let (fail, lose) = (s.fail_seeds.clone(), s.lose_seeds.clone());
            let Some(j) = s.jobs.get_mut(id) else { return (404, json!({"error": "job not found"})) };
            let seed = j.body["seed"].as_u64().unwrap_or(0);
            match j.status {
                _ if s.frozen => {}
                "queued" if !busy => j.status = "generating",
                "generating" if j.cancel_requested => j.status = "cancelled",
                "generating" => {
                    j.polls += 1;
                    if j.polls >= steps {
                        j.status = if fail.contains(&seed) { "failed" } else { "completed" };
                    }
                }
                _ => {}
            }
            let j = j.clone();
            if j.status == "completed" && lose.contains(&seed) {
                s.lose_seeds.retain(|x| *x != seed);
                s.jobs.remove(id);
                return (410, json!({"error": "job expired"}));
            }
            let result = (j.status == "completed").then(|| {
                let b64 = base64::engine::general_purpose::STANDARD.encode(result_png(&j.body));
                json!({"output_format": "png", "images": [{"index": 0, "b64_json": b64}]})
            });
            let error = match j.status {
                "failed" => json!({"code": "generation_failed", "message": "generate_image returned empty results"}),
                "cancelled" => json!({"code": "cancelled", "message": "job cancelled by client"}),
                _ => Value::Null,
            };
            (200, job_json(id, &j, result, error))
        }
        (Method::Post, p) if p.starts_with("jobs/") && p.ends_with("/cancel") => {
            let id = &p[5..p.len() - 7];
            let Some(j) = s.jobs.get_mut(id) else { return (404, json!({"error": "job not found"})) };
            match j.status {
                _ if s.ignore_cancel => {}
                "queued" => j.status = "cancelled",
                "generating" => j.cancel_requested = true,
                _ => {}
            }
            let j = j.clone();
            (200, job_json(id, &j, None, Value::Null))
        }
        _ => (404, json!({"error": "not found"})),
    }
}
