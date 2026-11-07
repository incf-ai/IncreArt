//! What a server at a URL is: sd-server's API (sd-server, or a proxy that
//! speaks it), KoboldCpp, or an OpenAI-compatible chat server. A few quick
//! GETs, for connecting to servers already running (the setup wizard, the
//! preset editor, Server settings).

use std::time::{Duration, Instant};

use serde_json::Value;
use ureq::Agent;

use crate::config::ServerConfig;
use crate::kobold::{KoboldChat, KoboldRun, KoboldServer};
use crate::pe::{PeRun, PeServer};

/// What a connected server is used as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// sd-server's API (`/sdcpp/v1`): sd-server, or a proxy speaking it.
    SdServer,
    /// KoboldCpp: images (its image model), prompt enhance (its text model).
    KoboldCpp,
    /// An OpenAI-compatible chat server (`/v1/models`), for prompt enhance.
    Chat,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::SdServer, Kind::KoboldCpp, Kind::Chat];

    pub fn label(self) -> &'static str {
        match self {
            Kind::SdServer => "sd-server API",
            Kind::KoboldCpp => "KoboldCpp",
            Kind::Chat => "chat server (OpenAI-compatible)",
        }
    }
}

/// What a server said about itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    pub kind: Kind,
    /// Its version, when it says (KoboldCpp).
    pub version: Option<String>,
    /// It makes images (sd-server's API; KoboldCpp with an image model).
    pub images: bool,
    /// The image model it runs or has loaded, or the model files it lists
    /// (sd-server's API).
    pub image_models: Vec<String>,
    /// The chat models it answers with (KoboldCpp's text model, a chat
    /// server's `/v1/models`).
    pub chat_models: Vec<String>,
    /// It asked for a password (KoboldCpp's `--password`) on a call.
    pub needs_password: bool,
    /// The recipes it offers (sd-server's API: `/sdcpp/v1/recipes`).
    pub recipes: Vec<Recipe>,
}

/// A recipe a server offers: its ID, name and model slots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    /// Slot IDs, each with whether it must have a file.
    pub slots: Vec<(String, bool)>,
    /// Its default steps and CFG, when it says.
    pub steps: Option<u32>,
    pub cfg: Option<f32>,
}

impl Recipe {
    /// "checkpoint — Checkpoint (model)": the slots a file is needed in
    /// in brackets, the others after "optional".
    /// The slot a model made of one file puts it in: the recipe's only
    /// required slot, when it has just one.
    pub fn single_slot(&self) -> Option<&str> {
        match self.slots.iter().filter(|(_, r)| *r).collect::<Vec<_>>().as_slice() {
            [(slot, _)] => Some(slot),
            _ => None,
        }
    }

    pub fn label(&self) -> String {
        let need: Vec<&str> = self.slots.iter().filter(|(_, r)| *r).map(|(s, _)| s.as_str()).collect();
        let may: Vec<&str> = self.slots.iter().filter(|(_, r)| !*r).map(|(s, _)| s.as_str()).collect();
        let mut t = self.id.clone();
        if !self.name.is_empty() && self.name != self.id {
            t += &format!(" — {}", self.name);
        }
        t += &format!(" ({})", need.join(", "));
        if !may.is_empty() {
            t += &format!(", optional: {}", may.join(", "));
        }
        t
    }
}

impl Detected {
    /// What it can load, as the engine reads it once connected: the files
    /// sd-server's API lists (matched by name too), the image model a
    /// KoboldCpp runs. None for a chat server.
    pub fn loadable(&self) -> Option<crate::sdserver::Loadable> {
        use crate::sdserver::Loadable;
        match self.kind {
            Kind::SdServer => Some(Loadable::listed(self.image_models.iter().cloned())),
            Kind::KoboldCpp if self.images => Some(Loadable::Only(self.image_models.first().cloned())),
            _ => None,
        }
    }

    /// One line: "KoboldCpp 1.117 · images: x.gguf · chat: Qwen3-VL".
    pub fn summary(&self) -> String {
        let mut parts = vec![match &self.version {
            Some(v) => format!("{} {v}", self.kind.label()),
            None => self.kind.label().to_string(),
        }];
        let list = |v: &[String]| match v {
            [one] => short(one),
            [first, rest @ ..] => format!("{} and {} more", short(first), rest.len()),
            [] => String::new(),
        };
        match (self.kind, self.images, self.image_models.is_empty()) {
            (Kind::SdServer, _, true) => parts.push("no model files listed".into()),
            (Kind::SdServer, _, false) => parts.push(format!(
                "{} model file{} listed ({})",
                self.image_models.len(),
                if self.image_models.len() == 1 { "" } else { "s" },
                list(&self.image_models)
            )),
            (Kind::KoboldCpp, false, _) => parts.push("no image model".into()),
            (Kind::KoboldCpp, true, _) => parts.push(format!("images: {}", list(&self.image_models))),
            (Kind::Chat, ..) => {}
        }
        if !self.chat_models.is_empty() {
            parts.push(format!("chat: {}", list(&self.chat_models)));
        } else if self.kind == Kind::KoboldCpp {
            parts.push("no text model".into());
        }
        if self.needs_password {
            parts.push("asks for a password".into());
        }
        parts.join(" · ")
    }
}

/// A file ID or model name without its folders.
fn short(s: &str) -> String {
    s.rsplit(['/', '\\']).next().unwrap_or(s).to_string()
}

/// The address as servers are connected to: `http://` added when no
/// scheme is given, and an API path pasted with it taken off
/// (`/sdcpp/v1`, `/v1`, `/api`, `/sdapi/v1`), with the trailing `/`.
pub fn normalize(url: &str) -> String {
    let mut u = url.trim().trim_end_matches('/').to_string();
    if !u.is_empty() && !u.contains("://") {
        u = format!("http://{u}");
    }
    for tail in ["/sdcpp/v1", "/sdapi/v1", "/v1", "/api"] {
        if let Some(stripped) = u.strip_suffix(tail) {
            u = stripped.trim_end_matches('/').to_string();
            break;
        }
    }
    u
}

fn agent() -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(4)))
        .timeout_connect(Some(Duration::from_secs(3)))
        .build()
        .into()
}

/// A GET: (status, JSON or Null). Errors are the request's (no answer).
fn get(agent: &Agent, base: &str, path: &str, password: &str) -> Result<(u16, Value), String> {
    let url = format!("{base}{path}");
    let t = Instant::now();
    let mut rq = agent.get(&url);
    if !password.is_empty() {
        rq = rq.header("Authorization", format!("Bearer {password}"));
    }
    let answer = rq
        .call()
        .map_err(|e| e.to_string())
        .and_then(|mut r| Ok((r.status().as_u16(), r.body_mut().read_to_string().map_err(|e| e.to_string())?)));
    crate::traffic::exchange("GET", &url, None, &answer, t.elapsed());
    let (status, body) = answer?;
    Ok((status, serde_json::from_str(&body).unwrap_or(Value::Null)))
}

fn strings(v: Option<&Value>, key: &str) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| x.get(key).and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// `/sdcpp/v1/recipes`: `{"recipes": [{"id", "name", "slots": [{"id",
/// "required"}]}]}`.
fn recipes(v: &Value) -> Vec<Recipe> {
    v.get("recipes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let defaults = r.get("defaults");
            let default = |k: &str| defaults.and_then(|d| d.get(k)).and_then(Value::as_f64);
            Some(Recipe {
                steps: default("steps").filter(|s| *s >= 1.0).map(|s| s as u32),
                cfg: default("cfg_scale").map(|c| c as f32),
                id: r.get("id")?.as_str()?.to_string(),
                name: r.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                slots: r
                    .get("slots")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|s| {
                        let id = s.get("id")?.as_str()?.to_string();
                        Some((id, s.get("required").and_then(Value::as_bool).unwrap_or(false)))
                    })
                    .collect(),
            })
        })
        .collect()
}

/// Asks the server at `url` (see [`normalize`]) what it is: KoboldCpp
/// (`/api/extra/version`), then sd-server's API (`/sdcpp/v1/capabilities`),
/// then a chat server (`/v1/models`). Err says why none answered as one.
pub fn detect(url: &str, password: &str) -> Result<Detected, String> {
    let base = normalize(url);
    if base.is_empty() {
        return Err("no address".into());
    }
    let agent = agent();
    // the first call says whether anything answers there at all
    let (status, v) = get(&agent, &base, "/api/extra/version", password).map_err(|e| format!("no answer: {e}"))?;
    if status == 200 && v.get("result").and_then(Value::as_str) == Some("KoboldCpp") {
        let mut d = Detected {
            kind: Kind::KoboldCpp,
            version: v.get("version").and_then(Value::as_str).map(str::to_string),
            images: v.get("txt2img").and_then(Value::as_bool) == Some(true),
            image_models: vec![],
            chat_models: vec![],
            needs_password: false,
            recipes: vec![],
        };
        if d.images {
            match get(&agent, &base, "/sdapi/v1/sd-models", password) {
                Ok((401, _)) => d.needs_password = true,
                Ok((_, v)) => d.image_models = strings(Some(&v), "filename"),
                Err(_) => {}
            }
        }
        match get(&agent, &base, "/api/v1/model", password) {
            Ok((401, _)) => d.needs_password = true,
            Ok((_, v)) => {
                if let Some(m) = v.get("result").and_then(Value::as_str).filter(|m| !m.is_empty() && *m != "inactive") {
                    d.chat_models = vec![m.trim_start_matches("koboldcpp/").to_string()];
                }
            }
            Err(_) => {}
        }
        return Ok(d);
    }
    if let Ok((200, caps)) = get(&agent, &base, "/sdcpp/v1/capabilities", password)
        && caps.is_object()
    {
        // as the engine lists them (another folder's `@…/` taken off)
        let image_models = match get(&agent, &base, "/sdcpp/v1/model-files", password) {
            Ok((200, v)) => strings(v.get("files"), "id")
                .into_iter()
                .map(|id| crate::sdserver::portable_id(&id).to_string())
                .collect(),
            _ => vec![],
        };
        let recipes = match get(&agent, &base, "/sdcpp/v1/recipes", password) {
            Ok((200, v)) => recipes(&v),
            _ => vec![],
        };
        return Ok(Detected {
            kind: Kind::SdServer,
            version: None,
            images: true,
            image_models,
            chat_models: vec![],
            needs_password: false,
            recipes,
        });
    }
    match get(&agent, &base, "/v1/models", password) {
        Ok((200, v)) if v.get("data").is_some_and(Value::is_array) => Ok(Detected {
            kind: Kind::Chat,
            version: None,
            images: false,
            image_models: vec![],
            chat_models: strings(v.get("data"), "id"),
            needs_password: false,
            recipes: vec![],
        }),
        Ok((401 | 403, _)) => Err("it asks for a key or password (a chat server's /v1/models answered 401)".into()),
        _ => Err(format!(
            "it answers (HTTP {status}), but not as sd-server's API, KoboldCpp or an OpenAI-compatible chat server"
        )),
    }
}

/// The preset entry for a server connected as `kind`, named `name`: an
/// sd-server's API is a `Connect`; KoboldCpp a connected `Kobold`
/// (prompt enhance's chat server too when it has a text model, or when
/// that isn't known), or a chat server only without an image model; a
/// chat server a connected `Chat`.
pub fn server_config(kind: Kind, name: &str, url: &str, password: &str, found: Option<&Detected>) -> ServerConfig {
    let url = normalize(url);
    let chat_only = || {
        ServerConfig::Chat(PeServer {
            name: name.into(),
            run: PeRun::Connect { url: url.clone() },
            ..Default::default()
        })
    };
    match kind {
        Kind::SdServer => ServerConfig::Connect { name: name.into(), url: url.clone(), device: String::new() },
        Kind::KoboldCpp if found.is_some_and(|d| d.kind == Kind::KoboldCpp && !d.images) => chat_only(),
        Kind::KoboldCpp => ServerConfig::Kobold(KoboldServer {
            name: name.into(),
            run: KoboldRun::Connect { url: url.clone(), password: password.into(), device: String::new() },
            chat: found.is_none_or(|d| !d.chat_models.is_empty()).then(KoboldChat::default),
            swap: false,
        }),
        Kind::Chat => chat_only(),
    }
}

/// A name for a server connected as `kind` not taken by `taken`:
/// "sd-server", "koboldcpp", "chat", then "… 2" and on.
pub fn unique_name(kind: Kind, taken: &[&str]) -> String {
    let base = match kind {
        Kind::SdServer => "sd-server",
        Kind::KoboldCpp => "koboldcpp",
        Kind::Chat => "chat",
    };
    (1..)
        .map(|n| if n == 1 { base.to_string() } else { format!("{base} {n}") })
        .find(|n| !taken.contains(&n.as_str()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalized() {
        assert_eq!(normalize(" 192.168.2.136:5000/ "), "http://192.168.2.136:5000");
        assert_eq!(normalize("http://box:7800/sdcpp/v1/"), "http://box:7800");
        assert_eq!(normalize("https://chat.example/v1"), "https://chat.example");
        assert_eq!(normalize("http://box:5001/api"), "http://box:5001");
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn a_proxys_recipes_are_read() {
        // as imgproxy answers
        let v: Value = serde_json::from_str(
            r#"{"recipes":[{"schema_version":1,"id":"checkpoint","name":"Checkpoint","slots":[{"id":"model","required":true,"label":"Model"}],"defaults":{"cfg_scale":7,"steps":20}}],"errors":[]}"#,
        )
        .unwrap();
        let r = recipes(&v);
        assert_eq!(
            r,
            [Recipe {
                id: "checkpoint".into(),
                name: "Checkpoint".into(),
                slots: vec![("model".into(), true)],
                steps: Some(20),
                cfg: Some(7.0),
            }]
        );
        assert_eq!(r[0].single_slot(), Some("model"));
        assert_eq!(r[0].label(), "checkpoint — Checkpoint (model)");
    }

    #[test]
    fn names_are_unique() {
        assert_eq!(unique_name(Kind::Chat, &[]), "chat");
        assert_eq!(unique_name(Kind::SdServer, &["sd-server", "sd-server 2"]), "sd-server 3");
    }

    #[test]
    fn a_koboldcpp_without_an_image_model_is_a_chat_server() {
        let d = Detected {
            kind: Kind::KoboldCpp,
            version: Some("1.117".into()),
            images: false,
            image_models: vec![],
            chat_models: vec!["Qwen3".into()],
            needs_password: false,
            recipes: vec![],
        };
        assert!(matches!(server_config(Kind::KoboldCpp, "k", "box:5001", "", Some(&d)), ServerConfig::Chat(_)));
        let d = Detected { images: true, image_models: vec!["x.gguf".into()], ..d };
        match server_config(Kind::KoboldCpp, "k", "box:5001", "pw", Some(&d)) {
            ServerConfig::Kobold(k) => {
                assert!(k.chat.is_some());
                assert_eq!(
                    k.run,
                    KoboldRun::Connect { url: "http://box:5001".into(), password: "pw".into(), device: String::new() }
                );
            }
            s => panic!("{s:?}"),
        }
        assert_eq!(d.summary(), "KoboldCpp 1.117 · images: x.gguf · chat: Qwen3");
    }
}
