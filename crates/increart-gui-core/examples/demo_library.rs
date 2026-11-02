//! Builds the demo library (README, Example library) from
//! `scripts/demo-library.json`: imports the public-domain source images
//! (`increart-core`'s `demo_sources` example) as ideas, creates the projects, then works
//! each project through the app's own Generate form and prompt enhance (the
//! reducer and runtime, no window): optionally a PE rewrite of the prompt,
//! then PE's suggestions for its blanks (the first picked; every project
//! has some), and for a style sweep its enhanced style prompts. Then it queues the runs on a server preset and
//! waits for them. `review` applies the spec's verdicts afterwards.
//!
//!     cargo run --release -p increart-gui-core --example demo_library -- \
//!         build <config.rsn> <server preset> <spec.json> <sources dir> <library>
//!     cargo run --release -p increart-gui-core --example demo_library -- \
//!         review <spec.json> <library>
//!
//! The config gives the server preset (its chat server does prompt
//! enhance), the model presets and PE's settings; it is never written.
//! Verdicts are per project, `g` / `n` / `b` (`-`: left unreviewed), over
//! its outputs in run then seed order (`review` again puts each back as
//! the spec says).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use increart_core::config::Config;
use increart_core::model::IdeaSource;
use increart_core::prompts::{PresetKey, SweepSource, SweepVary};
use increart_core::{IdeaLoc, ImageRef, Library, NewIdea, Original, Tree, Verdict};
use increart_gui_core::*;
use serde_json::Value;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["build", config, preset, spec, sources, library] => {
            build(Path::new(config), preset, &read_spec(Path::new(spec)), Path::new(sources), Path::new(library))
        }
        ["review", spec, library] => review(&read_spec(Path::new(spec)), Path::new(library)),
        _ => {
            eprintln!("usage: demo_library build <config.rsn> <preset> <spec.json> <sources> <library>");
            eprintln!("       demo_library review <spec.json> <library>");
            std::process::exit(2);
        }
    }
}

fn read_spec(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read spec")).expect("parse spec")
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_default()
}

fn strs(v: &Value, key: &str) -> Vec<String> {
    v[key].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// Each idea key's (idea, images).
type Ideas = BTreeMap<String, (increart_core::Id, Vec<increart_core::Id>)>;
/// Each project's ID by name.
type Projects = BTreeMap<String, increart_core::Id>;

/// The ideas and the empty projects, straight into the library.
fn create(spec: &Value, sources: &Path, library: &Path) -> (Ideas, Projects) {
    assert!(!library.exists(), "{} exists; the demo library is built fresh", library.display());
    let mut lib = Library::create(library, None).expect("create library");
    // the sources scaled down to keep the library small (0: as they are)
    let max_px = spec["max_source_px"].as_u64().unwrap_or(0) as u32;
    let mut ideas = BTreeMap::new();
    for i in spec["ideas"].as_array().expect("ideas") {
        // imported under the file name only: no path from this machine
        let images: Vec<_> = strs(i, "images")
            .iter()
            .map(|f| {
                let bytes = shrink(&fs::read(sources.join(f)).expect("read source image"), max_px);
                lib.import_bytes(&bytes, &Original { name: Some(f.clone()), ..Default::default() })
                    .expect("import")
                    .image
            })
            .collect();
        let (loc, collection) = match s(i, "in") {
            "inbox" => (IdeaLoc::Unreviewed, PathBuf::new()),
            "discarded" => (IdeaLoc::Discarded, PathBuf::new()),
            folder => (IdeaLoc::Library, PathBuf::from(folder)),
        };
        let id = lib
            .new_idea(NewIdea {
                title: s(i, "title").into(),
                text: s(i, "text").into(),
                tags: strs(i, "tags"),
                source: IdeaSource::Manual,
                images: images.clone(),
                starred: i["starred"].as_bool().unwrap_or(false),
                loc,
                collection,
            })
            .expect("new idea");
        ideas.insert(s(i, "key").to_string(), (id, images));
    }
    let mut projects = BTreeMap::new();
    for p in spec["projects"].as_array().expect("projects") {
        let folder = PathBuf::from(s(p, "folder"));
        make_folders(&lib, Tree::Projects, &folder);
        let sources: Vec<_> = strs(p, "sources").iter().map(|k| ideas[k].0.clone()).collect();
        let (id, _) = lib.create_project(&folder, Some(s(p, "name")), &sources).expect("create project");
        projects.insert(s(p, "name").to_string(), id);
    }
    (ideas, projects)
}

/// `bytes` as a PNG at most `max` pixels on its long side (as it is when
/// smaller, or `max` is 0).
fn shrink(bytes: &[u8], max: u32) -> Vec<u8> {
    let img = image::load_from_memory(bytes).expect("decode source image");
    if max == 0 || img.width().max(img.height()) <= max {
        return bytes.to_vec();
    }
    let small = img.resize(max, max, image::imageops::FilterType::Lanczos3);
    let mut out = std::io::Cursor::new(Vec::new());
    small.write_to(&mut out, image::ImageFormat::Png).expect("encode");
    out.into_inner()
}

fn make_folders(lib: &Library, tree: Tree, folder: &Path) {
    let mut parent = PathBuf::new();
    for part in folder.components() {
        let name = part.as_os_str().to_str().expect("utf-8 folder");
        if !lib.path(tree.base()).join(&parent).join(name).is_dir() {
            lib.create_folder(tree, &parent, Some(name)).expect("create folder");
        }
        parent.push(name);
    }
}

/// The app without its window.
struct App {
    state: AppState,
    rt: Runtime,
    t0: Instant,
}

impl App {
    fn send(&mut self, msg: Msg) {
        if let Msg::PeStatus(st) = &msg {
            self.log(&format!("prompt enhance: {st:?}"));
        }
        for f in update(&mut self.state, msg) {
            match f {
                // the user's settings stay as they are
                Effect::SaveConfig(..) | Effect::PlaySound { .. } => {}
                Effect::PeAsk { servers, jobs } => {
                    let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
                    self.log(&format!("PE ask: servers {names:?}, {} jobs", jobs.len()));
                    self.rt.run(Effect::PeAsk { servers, jobs });
                }
                Effect::PeShutdown => {
                    self.log("PE shutdown");
                    self.rt.run(Effect::PeShutdown);
                }
                f => self.rt.run(f),
            }
        }
        // say yes to whatever is asked
        if let Some(c) = self.state.confirm.take() {
            self.log(&format!("confirm: {}", c.text));
            self.send(*c.then);
        }
        if self.state.sweep_order_ask.is_some() {
            self.send(Msg::SweepOrderAnswer(Some(false)));
        }
        if self.state.pe.added_blanks_ask {
            self.send(Msg::PeAddedBlanks(false));
        }
    }

    fn log(&self, text: &str) {
        eprintln!("{:7.1}s {text}", self.t0.elapsed().as_secs_f64());
    }

    /// Feeds the runtime's messages back until `done` (false: timed out).
    fn pump(&mut self, limit: Duration, done: impl Fn(&AppState) -> bool) -> bool {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            for m in self.rt.wait(Duration::from_millis(50)) {
                self.send(m);
            }
            if done(&self.state) {
                return true;
            }
        }
        false
    }

    fn pe_idle(st: &AppState) -> bool {
        st.pe.jobs.is_empty() && st.pe.blank_tries.is_empty() && st.pe.style_tries.is_empty()
    }

    fn errors(&self) -> Vec<String> {
        self.state.messages.iter().filter(|m| m.error).map(|m| m.text.clone()).collect()
    }
}

fn build(config: &Path, preset: &str, spec: &Value, sources: &Path, library: &Path) {
    let (ideas, projects) = create(spec, sources, library);
    let (config, warnings) = Config::load(config).expect("load config");
    for w in warnings {
        eprintln!("config: {w}");
    }
    let preset_at = config.server_presets.iter().position(|p| p.name == preset).expect("server preset in config");
    let recipe = s(spec, "recipe");
    let recipe_at = config.recipes.iter().position(|r| r.name == recipe).expect("model preset in config");
    let mut app = App { state: AppState::new(config, None), rt: Runtime::new(|| {}).unwrap(), t0: Instant::now() };

    app.send(Msg::SetLibraryPath(library.to_string_lossy().into_owned()));
    app.send(Msg::OpenLibrary);
    assert!(app.pump(Duration::from_secs(60), |s| s.library.is_some()), "the library didn't open");
    app.send(Msg::OpenPresetNow(preset_at));
    let up = app.pump(Duration::from_secs(1200), |s| {
        (s.engine_running && s.pe.status == PeStatus::Ready) || matches!(s.pe.status, PeStatus::Failed(_))
    }) && app.state.pe.status == PeStatus::Ready;
    assert!(up, "servers not up: {:?} / PE {:?} / {:?}", app.state.servers, app.state.pe.status, app.errors());
    app.log("servers and prompt enhance ready");

    let mut expected = 0;
    for p in spec["projects"].as_array().expect("projects") {
        let name = s(p, "name");
        app.log(&format!("{name}: {}", s(p, "pe")));
        app.send(Msg::Tab(Tab::Projects));
        app.send(Msg::ProjectsTab(ProjectsTab::Generate));
        app.send(Msg::SelectProject(projects[name].clone()));
        app.pump(Duration::from_millis(300), |_| false);

        let refs: Vec<ImageRef> =
            strs(p, "refs").iter().flat_map(|k| ideas[k].1.iter().cloned().map(ImageRef::input)).collect();
        let mut f = app.state.generate.clone();
        f.label = s(p, "label").into();
        f.prompt = s(p, "prompt").into();
        f.preset = recipe_at;
        f.width = p["width"].as_u64().expect("width") as u32;
        f.height = p["height"].as_u64().expect("height") as u32;
        f.seeds = p["seeds"].as_u64().expect("seeds");
        f.first_seed = 1;
        f.images = images_of(&refs, None);
        f.source_idea = strs(p, "sources").first().map(|k| ideas[k].0.clone());
        let styles: Vec<PresetKey> = p["styles"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|k| PresetKey { task: k[0].as_str().unwrap().into(), name: k[1].as_str().unwrap().into() })
                    .collect()
            })
            .unwrap_or_default();
        if !styles.is_empty() {
            f.sweep.on = true;
            f.sweep.vary = SweepVary::Style;
            f.sweep.source = SweepSource::Selected;
            f.sweep.selected = styles.clone();
        }
        app.send(Msg::Generate(f));

        // a rewrite first, asked again (at most 4 times) when it loses one of
        // the prompt's blanks or <imageN> tags; else the prompt as written
        if s(p, "pe") == "rewrite" {
            let original = app.state.generate.prompt.clone();
            for attempt in 1..=4 {
                app.send(Msg::PeRewrite(PeTarget::Prompt));
                let answered = app.pump(Duration::from_secs(600), |s| {
                    s.pe.text.as_ref().is_some_and(|t| t.waiting.is_none() && (!t.text.is_empty() || t.error.is_some()))
                });
                let t = app.state.pe.text.clone().expect("rewrite window");
                assert!(answered && t.error.is_none(), "rewrite failed: {:?}", t.error);
                let lost = increart_core::pe::lost_tags(&original, &t.text);
                app.log(&format!("rewritten ({attempt}): {}", t.text));
                if lost.is_empty() {
                    app.send(Msg::PeTextClose(PeUse::Use));
                    break;
                }
                app.log(&format!("the rewrite lost {lost:?}; not used"));
                app.send(Msg::PeTextClose(PeUse::Discard));
            }
        }
        // every blank's suggestions (the chips, kept in the project's draft),
        // then the first offered picked
        let mut texts = vec![app.state.generate.prompt.clone()];
        if !styles.is_empty() {
            let list = &app.state.prompt_presets;
            texts.extend(
                increart_core::prompts::sweep_presets(list, &app.state.generate.sweep)
                    .iter()
                    .map(|(_, pr)| pr.prompt.clone()),
            );
        }
        let slots = increart_core::prompts::slot_names(texts.iter().map(String::as_str));
        assert!(!slots.is_empty(), "{name}: no {{blanks}} in {texts:?}");
        let keys: Vec<BlankKey> = slots.iter().map(|n| BlankKey { style: None, slot: n.clone() }).collect();
        app.send(Msg::PeSuggest(keys.clone()));
        assert!(app.pump(Duration::from_secs(900), App::pe_idle), "suggestions never came");
        let mut f = app.state.generate.clone();
        for k in &keys {
            let shown = app.state.blank_shown(k).cloned().unwrap_or_default();
            let v = shown.first().unwrap_or_else(|| panic!("no suggestions for {{{}}}", k.slot));
            app.log(&format!("{{{}}}: {shown:?} → {v}", k.slot));
            f.sweep.slots.insert(k.slot.clone(), v.clone());
        }
        app.send(Msg::Generate(f));
        if !styles.is_empty() {
            app.send(Msg::PeStyleEnhance(vec![]));
            assert!(app.pump(Duration::from_secs(900), App::pe_idle), "style prompts never came");
            for sp in &app.state.generate.sweep.style_prompts {
                app.log(&format!("{} (enhanced {}): {}", sp.preset.name, sp.enhanced, sp.prompt));
            }
        }
        let before = app.state.index.projects.iter().map(|p| p.runs.len()).sum::<usize>();
        app.send(Msg::QueueGenerate);
        let runs = styles.len().max(1);
        // the first run hashes the model files (minutes from a cold disk)
        let queued = app.pump(Duration::from_secs(1200), |s| {
            s.index.projects.iter().map(|p| p.runs.len()).sum::<usize>() >= before + runs
        });
        assert!(queued, "{name} didn't queue: {:?}", app.errors());
        expected += runs * p["seeds"].as_u64().unwrap() as usize;
        app.log(&format!("{name}: queued {runs} run(s)"));
    }

    // every seed generated
    let done = app.pump(Duration::from_secs(3 * 3600), |s| {
        s.index.projects.iter().map(|p| p.outputs.len()).sum::<usize>() >= expected
            && s.index.projects.iter().all(|p| p.runs.iter().all(|r| p.pending_seeds(r).next().is_none()))
    });
    // the last project's draft saved too (leaving its Generate form)
    app.send(Msg::SelectProjectFolder(None));
    assert!(app.pump(Duration::from_secs(10), |s| s.saving == 0), "the last draft wasn't saved");
    let errors = app.errors();
    assert!(done, "not finished: {errors:?}");
    app.log(&format!("{expected} images; messages: {errors:?}"));
    app.rt.shutdown(false);
}

fn review(spec: &Value, library: &Path) {
    let lib = Library::open(library).expect("open library");
    let index = lib.scan().expect("scan");
    for p in spec["projects"].as_array().expect("projects") {
        let verdicts = s(p, "verdicts");
        let Some(entry) = index.projects.iter().find(|e| e.name() == s(p, "name")) else { continue };
        let mut runs = entry.runs.clone();
        runs.sort_by_key(|r| (r.created, r.label.clone()));
        let outputs = runs.iter().flat_map(|r| {
            let mut o: Vec<_> = entry.outputs.iter().filter(|o| o.name.run == r.id).collect();
            o.sort_by_key(|o| o.name.seed);
            o
        });
        for (o, v) in outputs.zip(verdicts.chars()) {
            let verdict = match v {
                'g' => Some(Verdict::Good),
                'n' => Some(Verdict::Neutral),
                'b' => Some(Verdict::Bad),
                _ => None,
            };
            if o.verdict != verdict {
                lib.review_output(&o.path, verdict).expect("review");
            }
        }
    }
}
