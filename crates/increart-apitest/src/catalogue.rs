//! The cases: for each image server, each chat server and each preset's
//! servers together, in the order they run.

use increart_core::apitest::{Case, Cost};
use increart_core::config::{Config, ServerConfig, ServerPreset};
use increart_core::kobold::KoboldRun;
use increart_core::pe::PeServer;

use crate::ctx::{Ctx, Verdict};

/// What a case tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// One image server (sd-server, a proxy, KoboldCpp).
    Image,
    /// One chat server prompt enhance asks.
    Chat,
    /// The preset's servers together.
    Preset,
}

/// What a case runs on: an image or chat server's index, or none.
#[derive(Clone, Copy, Debug)]
pub enum Target {
    Image(usize),
    Chat(usize),
    Preset,
}

/// The preset as the plan sees it (nothing opened yet).
pub struct Setup<'a> {
    pub config: &'a Config,
    pub images: Vec<ServerConfig>,
    pub chats: Vec<PeServer>,
}

pub struct Def {
    pub key: &'static str,
    pub group: &'static str,
    pub title: &'static str,
    pub scope: Scope,
    pub cost: Cost,
    pub expects: &'static str,
    /// Why it isn't for this target (n/a), seen before anything opens.
    pub applies: fn(&Setup, Target) -> Option<String>,
    pub run: fn(&mut Ctx, Target) -> Verdict,
}

fn always(_: &Setup, _: Target) -> Option<String> {
    None
}

fn image_of<'a>(s: &'a Setup, t: Target) -> &'a ServerConfig {
    match t {
        Target::Image(i) => &s.images[i],
        _ => unreachable!("an image server's case"),
    }
}

fn kobold_started(s: &Setup, t: Target) -> Option<String> {
    matches!(image_of(s, t), ServerConfig::Kobold(k) if matches!(k.run, KoboldRun::Start { .. }))
        .then(|| "a started KoboldCpp loads by restarting with the model: a missing file would stop it".into())
}

fn with_loras(s: &Setup, _: Target) -> Option<String> {
    (!s.config.recipes.iter().any(|r| r.loras.iter().any(|l| !l.file.is_empty())))
        .then(|| "no model preset of the config has a LoRA".into())
}

fn with_sigmas(s: &Setup, _: Target) -> Option<String> {
    (!s.config.recipes.iter().any(|r| !r.custom_sigmas.is_empty()))
        .then(|| "no model preset of the config has custom sigmas".into())
}

fn text_only(s: &Setup, t: Target) -> Option<String> {
    let Target::Chat(i) = t else { unreachable!() };
    (s.chats[i].vision_known() == Some(false))
        .then(|| "it has no vision projector: the app sends it no images (the roles grid says text only)".into())
}

fn two_images(s: &Setup, _: Target) -> Option<String> {
    (s.images.len() < 2).then(|| "the preset has one image server".into())
}

fn images_and_chat(s: &Setup, _: Target) -> Option<String> {
    if s.images.is_empty() {
        Some("the preset has no image server".into())
    } else if s.chats.is_empty() {
        Some("the preset has no chat server (prompt enhance)".into())
    } else {
        None
    }
}

fn has_images(s: &Setup, _: Target) -> Option<String> {
    s.images.is_empty().then(|| "the preset has no image server".into())
}

fn failover(s: &Setup, t: Target) -> Option<String> {
    two_images(s, t).or_else(|| {
        (!s.images.iter().any(ServerConfig::is_started))
            .then(|| "none of its image servers is started by the suite (only those can be stopped)".into())
    })
}

/// Every case, in the order they run (within a preset: the servers
/// together open first, the failover stops one last).
pub fn defs() -> Vec<Def> {
    use crate::{chat, image, together};
    use Cost::*;
    use Scope::*;
    vec![
        Def {
            key: "open",
            group: "together",
            title: "All servers open together",
            scope: Preset,
            cost: Loads,
            expects: "Each server answers: a started one within the server start timeout of quiet log (its port \
                      free beforehand: the suite starts its own), a connected one at once. sd-server answers \
                      GET /sdcpp/v1/capabilities; KoboldCpp GET /api/v1/info/version (or /sdapi/v1/options); \
                      a chat server GET /v1/models.",
            applies: always,
            run: together::open,
        },
        Def {
            key: "capabilities",
            group: "connect",
            title: "Answers its capabilities",
            scope: Image,
            cost: Light,
            expects: "GET /sdcpp/v1/capabilities answers 200 with JSON (supported_modes including img_gen).",
            applies: always,
            run: image::capabilities,
        },
        Def {
            key: "latency",
            group: "connect",
            title: "Answers quickly",
            scope: Image,
            cost: Light,
            expects: "Ten GETs of capabilities each answer, typically in well under a second: the engine polls \
                      every job twice a second.",
            applies: always,
            run: image::latency,
        },
        Def {
            key: "parallel-clients",
            group: "connect",
            title: "Answers several clients at once",
            scope: Image,
            cost: Light,
            expects: "Eight connections at once, each GET /sdcpp/v1/models/status five times: every one answers \
                      (the engine polls, loads and submits from more than one thread).",
            applies: always,
            run: image::parallel_clients,
        },
        Def {
            key: "poll-burst",
            group: "connect",
            title: "Takes a burst of polls",
            scope: Image,
            cost: Light,
            expects: "A hundred GET /sdcpp/v1/models/status back to back all answer, none slower than 2 s.",
            applies: always,
            run: image::poll_burst,
        },
        Def {
            key: "unknown-job",
            group: "connect",
            title: "Says an unknown job is gone",
            scope: Image,
            cost: Light,
            expects: "GET /sdcpp/v1/jobs/<unknown id> answers 404 (or 410): the engine takes that as a job the \
                      server purged, and makes the seed again. Another answer reads as the server failing.",
            applies: always,
            run: image::unknown_job,
        },
        Def {
            key: "list",
            group: "models",
            title: "Lists its model files",
            scope: Image,
            cost: Light,
            expects: "GET /sdcpp/v1/model-files answers {\"files\": [{\"id\": \"diffusion_models/x.safetensors\"}, …]} \
                      with IDs relative to its model folder, as the model presets name them. 404: no list (then \
                      any model preset may be sent to it). A KoboldCpp started elsewhere lists only the model it \
                      runs.",
            applies: always,
            run: image::list,
        },
        Def {
            key: "status",
            group: "models",
            title: "Says what it has loaded",
            scope: Image,
            cost: Light,
            expects: "GET /sdcpp/v1/models/status answers {state: empty|loading|ready|error, recipe, models: {slot: \
                      file}}.",
            applies: always,
            run: image::status,
        },
        Def {
            key: "load-each",
            group: "models",
            title: "Loads each model preset it has",
            scope: Image,
            cost: Loads,
            expects: "POST /sdcpp/v1/models/load {recipe, models} blocks until loaded and answers the status, \
                      state ready, naming the recipe and the same files: an answer naming others makes the \
                      engine think the load failed.",
            applies: always,
            run: image::load_each,
        },
        Def {
            key: "load-missing",
            group: "models",
            title: "Refuses a missing file cleanly",
            scope: Image,
            cost: Loads,
            expects: "Loading a file it doesn't have answers an error (400 invalid_model_config or 422 \
                      model_load_failed), not a 5xx (read as the server failing) and not ready; the server \
                      answers after.",
            applies: kobold_started,
            run: image::load_missing,
        },
        Def {
            key: "basic",
            group: "generate",
            title: "The Test button's steps",
            scope: Image,
            cost: Gpu,
            expects: "The preset editor's Test, as is: it answers, its list, its status, a load, a small image, a \
                      cancel.",
            applies: always,
            run: image::basic,
        },
        Def {
            key: "txt2img",
            group: "generate",
            title: "Makes an image",
            scope: Image,
            cost: Gpu,
            expects: "POST /sdcpp/v1/img_gen answers 202 {id}; GET /sdcpp/v1/jobs/{id} goes queued → generating → \
                      completed with result.images[0].b64_json a PNG of the size asked (512×512).",
            applies: always,
            run: image::txt2img,
        },
        Def {
            key: "same-seed",
            group: "generate",
            title: "The same seed makes the same image",
            scope: Image,
            cost: Gpu,
            expects: "Two jobs with the same body and seed give the same pixels: a seed made again (after a \
                      failure, or on another server of the same kind) is the same image.",
            applies: always,
            run: image::same_seed,
        },
        Def {
            key: "reference",
            group: "generate",
            title: "Takes a reference image",
            scope: Image,
            cost: Gpu,
            expects: "img_gen with ref_images: [base64 PNG] completes (the Generate form's reference images).",
            applies: always,
            run: image::reference,
        },
        Def {
            key: "large-request",
            group: "generate",
            title: "Takes a large request",
            scope: Image,
            cost: Loads,
            expects: "An img_gen of about 12 MB (three 1024² reference images that don't compress) is taken: a \
                      proxy with a small body limit fails runs with large references. (Cancelled once taken.)",
            applies: always,
            run: image::large_request,
        },
        Def {
            key: "lora",
            group: "generate",
            title: "Applies a LoRA",
            scope: Image,
            cost: Gpu,
            expects: "img_gen with lora: [{path, multiplier}] (paths inside its LoRA folder) completes.",
            applies: with_loras,
            run: image::lora,
        },
        Def {
            key: "custom-sigmas",
            group: "generate",
            title: "Takes custom sigmas",
            scope: Image,
            cost: Gpu,
            expects: "img_gen with sample_params.custom_sigmas completes.",
            applies: with_sigmas,
            run: image::custom_sigmas,
        },
        Def {
            key: "queue",
            group: "jobs",
            title: "Queues a second job",
            scope: Image,
            cost: Gpu,
            expects: "A second img_gen while one generates is taken (the engine keeps two in flight per server) \
                      and waits queued (queue_position) or runs; both complete.",
            applies: always,
            run: image::queue,
        },
        Def {
            key: "cancel-running",
            group: "jobs",
            title: "Cancels a job generating",
            scope: Image,
            cost: Gpu,
            expects: "POST /sdcpp/v1/jobs/{id}/cancel on a generating job: it ends cancelled within a few steps.",
            applies: always,
            run: image::cancel_running,
        },
        Def {
            key: "cancel-queued",
            group: "jobs",
            title: "Cancels a job waiting",
            scope: Image,
            cost: Gpu,
            expects: "Cancelling a queued job ends it cancelled at once; the one generating goes on.",
            applies: always,
            run: image::cancel_queued,
        },
        Def {
            key: "after-cancel",
            group: "jobs",
            title: "Works after a cancel",
            scope: Image,
            cost: Gpu,
            expects: "A job after cancelled ones completes: a cancel doesn't leave the server stuck.",
            applies: always,
            run: image::after_cancel,
        },
        Def {
            key: "poll-finished",
            group: "jobs",
            title: "Keeps a finished job a while",
            scope: Image,
            cost: Gpu,
            expects: "A completed job polled again 5 s later still answers completed with its image (the engine \
                      may read it a poll later).",
            applies: always,
            run: image::poll_finished,
        },
        Def {
            key: "answers",
            group: "prompt-enhance",
            title: "Answers",
            scope: Chat,
            cost: Light,
            expects: "GET /v1/models answers 200 (OpenAI-compatible).",
            applies: always,
            run: chat::answers,
        },
        Def {
            key: "chat",
            group: "prompt-enhance",
            title: "Writes an answer",
            scope: Chat,
            cost: Gpu,
            expects: "POST /v1/chat/completions {messages, max_tokens, stream: true} streams text (server-sent \
                      events, or one JSON answer).",
            applies: always,
            run: chat::chat,
        },
        Def {
            key: "stop",
            group: "prompt-enhance",
            title: "Stops an answer midway",
            scope: Chat,
            cost: Gpu,
            expects: "An answer cut off midway (the connection closed; KoboldCpp also POST /api/extra/abort) \
                      leaves the server answering the next ask.",
            applies: always,
            run: chat::stop,
        },
        Def {
            key: "vision",
            group: "prompt-enhance",
            title: "Sees an image",
            scope: Chat,
            cost: Gpu,
            expects: "An ask with an image (image_url data:image/png;base64) is answered about the image: asked \
                      the colour of a red square, it says red.",
            applies: text_only,
            run: chat::vision,
        },
        Def {
            key: "routing",
            group: "together",
            title: "Each model preset has a server",
            scope: Preset,
            cost: Light,
            expects: "Each model preset goes to the image servers whose list has all its files (a server with no \
                      list may get any; a KoboldCpp started elsewhere, matching by its running file's name only, \
                      after a server listing them). A model preset no server has waits.",
            applies: has_images,
            run: together::routing,
        },
        Def {
            key: "parallel",
            group: "together",
            title: "Two servers make images at once",
            scope: Preset,
            cost: Gpu,
            expects: "Image jobs on two servers at the same time both complete.",
            applies: two_images,
            run: together::parallel,
        },
        Def {
            key: "pe-with-images",
            group: "together",
            title: "Prompt enhance while images generate",
            scope: Preset,
            cost: Gpu,
            expects: "A chat ask while an image generates is answered, and the image completes (a KoboldCpp doing \
                      both takes turns).",
            applies: images_and_chat,
            run: together::pe_with_images,
        },
        Def {
            key: "end-to-end",
            group: "together",
            title: "A run through the engine",
            scope: Preset,
            cost: Gpu,
            expects: "IncreArt's own engine, as the app runs it, in a scratch library: a run of a few seeds spread \
                      over the servers that can take its model preset; every seed is made and stored.",
            applies: has_images,
            run: together::end_to_end,
        },
        Def {
            key: "failover",
            group: "together",
            title: "A run goes on when a server goes",
            scope: Preset,
            cost: Gpu,
            expects: "A run over two servers, one of them stopped after the first image: the other makes the rest \
                      of the seeds. (Runs last: the stopped server is gone for the cases after it.)",
            applies: failover,
            run: together::failover,
        },
    ]
}

/// The case ID for a def on a target.
pub fn case_id(preset: &str, server: Option<&str>, d: &Def) -> String {
    match server {
        Some(s) => format!("{preset}/{s}/{}.{}", d.group, d.key),
        None => format!("{preset}/{}.{}", d.group, d.key),
    }
}

/// The preset's image servers and chat servers, as the app opens them.
pub fn setup<'a>(config: &'a Config, preset: &ServerPreset) -> Setup<'a> {
    Setup { config, images: preset.sd_servers(), chats: preset.chat_pool() }
}

/// Every case of these presets, in the order they run, each with its def
/// and target.
pub fn cases_of(config: &Config, presets: &[ServerPreset]) -> Vec<(Case, usize, Target)> {
    let defs = defs();
    let mut out = vec![];
    for p in presets {
        let s = setup(config, p);
        let mut add = |i: usize, d: &Def, server: Option<&str>, t: Target| {
            let id = case_id(&p.name, server, d);
            out.push((
                Case {
                    basic: increart_core::validation::is_basic(&id),
                    id,
                    preset: p.name.clone(),
                    server: server.map(str::to_string),
                    group: d.group.into(),
                    title: d.title.into(),
                    cost: d.cost,
                    not_applicable: (d.applies)(&s, t),
                    expects: d.expects.into(),
                },
                i,
                t,
            ))
        };
        let together = |d: &Def| d.scope == Scope::Preset;
        // open first, failover last
        for (i, d) in defs.iter().enumerate().filter(|(_, d)| d.key == "open") {
            add(i, d, None, Target::Preset);
        }
        for (j, srv) in s.images.iter().enumerate() {
            for (i, d) in defs.iter().enumerate().filter(|(_, d)| d.scope == Scope::Image) {
                add(i, d, Some(srv.name()), Target::Image(j));
            }
        }
        for (j, c) in s.chats.iter().enumerate() {
            for (i, d) in defs.iter().enumerate().filter(|(_, d)| d.scope == Scope::Chat) {
                add(i, d, Some(&c.name), Target::Chat(j));
            }
        }
        for (i, d) in defs.iter().enumerate().filter(|(_, d)| together(d) && d.key != "open") {
            add(i, d, None, Target::Preset);
        }
    }
    out
}

/// Every case of these presets, in the order they run.
pub fn plan(config: &Config, presets: &[ServerPreset]) -> Vec<Case> {
    cases_of(config, presets).into_iter().map(|(c, ..)| c).collect()
}
