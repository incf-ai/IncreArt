//! GUI flows driven through the reducer, with the real runtime and a real
//! library on disk (no window, no servers).

use std::path::{Path, PathBuf};
use std::time::Duration;

use increart_core::config::{Config, ReviewOrder, ServerConfig, ServerPreset, TreeOrder};
use increart_core::pe::GpuRuntime;
use increart_core::{IdeaLoc, Verdict};
use increart_gui_core::*;

struct Harness {
    state: AppState,
    rt: Runtime,
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

/// How long `settle` waits for nothing more to come.
const QUIET_MS: u64 = 25;

impl Harness {
    fn new() -> Harness {
        increart_core::fsutil::set_fsync(false);
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        let mut config = Config::default();
        config.normalize();
        let rt = Runtime::new(|| {}).unwrap();
        // started servers (stand-ins) log here, not in the real log folder
        rt.set_log_dir(tmp.path().join("logs"));
        rt.set_trash_dir(tmp.path().join("trash"));
        // stand-in servers answer at once: no half-second polls
        rt.set_engine_timing(Duration::from_millis(10), Duration::from_millis(50));
        let mut h = Harness { state: AppState::new(config, None), rt, _tmp: tmp, root };
        h.send(Msg::SetLibraryPath(h.root.to_string_lossy().into()));
        h.send(Msg::CreateLibrary);
        h.settle();
        h.send(Msg::ContinueWithoutServers);
        // the once-a-session "no server" warning (tested on its own)
        h.state.no_server_warned = true;
        h
    }

    /// Applies a message and carries out its effects (not settling).
    fn send(&mut self, msg: Msg) -> Vec<Effect> {
        let fx = update(&mut self.state, msg);
        for f in &fx {
            // never on the desktop: a test answers a picker with Msg::Picked
            if !matches!(f, Effect::Pick { .. } | Effect::OpenInEditor { .. }) {
                self.rt.run(f.clone());
            }
        }
        fx
    }

    /// Feeds runtime messages back until nothing is pending.
    #[track_caller]
    fn settle(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            // before the wait: the worker sends what it did before it's idle
            // (effects with no reply, like reading a history, are done then)
            let idle = self.rt.idle();
            // a short quiet spell: what isn't the worker's (threads of their
            // own, the engine) has its chance to answer
            let msgs = self.rt.wait(Duration::from_millis(QUIET_MS));
            if msgs.is_empty() {
                // a library still opening sends nothing until it's open
                let opening = matches!(&self.state.phase, Phase::Library(d) if d.busy);
                if idle && self.state.busy == 0 && self.state.saving == 0 && !opening && self.state.archiving.is_none()
                {
                    return;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "operations still pending (busy {}, saving {}): {:?}",
                    self.state.busy,
                    self.state.saving,
                    self.errors()
                );
                continue;
            }
            for m in msgs {
                self.send(m);
            }
        }
    }

    fn errors(&self) -> Vec<&str> {
        self.state.messages.iter().filter(|m| m.error).map(|m| m.text.as_str()).collect()
    }
}

#[test]
fn create_library_then_ideas_review_new_project_generate() {
    let mut h = Harness::new();
    assert_eq!(h.state.phase, Phase::Main);
    assert_eq!(h.state.library.as_deref(), Some(h.root.as_path()));
    assert_eq!(h.state.config.recent_libraries, [h.root.to_str().unwrap()]);
    assert_eq!(h.state.inbox_tab, InboxTab::NewIdea, "empty inbox opens on New idea");

    // two ideas into the inbox, then review them
    for (t, x) in [("Fox", "a red fox in a café"), ("Lighthouse", "lighthouse at dusk")] {
        h.send(Msg::Draft(IdeaDraft {
            title: t.into(),
            text: x.into(),
            tags: "animals, photo".into(),
            images: vec![],
        }));
        h.send(Msg::AddIdea(AddIdea::Inbox));
        h.settle();
    }
    assert_eq!(h.state.draft, IdeaDraft::default(), "draft cleared");
    let inbox: Vec<_> = h.state.unreviewed_ideas().iter().map(|e| e.idea.title.clone()).collect();
    assert_eq!(inbox, ["Lighthouse", "Fox"], "newest first");
    assert_eq!(h.state.unreviewed_ideas()[0].idea.tags, ["animals", "photo"]);

    h.send(Msg::ReviewCurrentIdea(Verdict::Bad));
    h.settle();
    h.send(Msg::ReviewCurrentIdea(Verdict::Good));
    h.settle();
    assert!(h.state.unreviewed_ideas().is_empty());
    let kept = h.state.library_ideas();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].idea.starred);
    assert_eq!(h.state.discarded_ideas()[0].idea.title, "Lighthouse");

    // restore the discarded one to the library, then make a project from both
    let lighthouse = h.state.discarded_ideas()[0].idea.id.clone();
    h.send(Msg::Restore(lighthouse.clone(), IdeaLoc::top_library()));
    h.settle();
    let fox = h.state.library_ideas()[1].idea.id.clone(); // newest first: Lighthouse, Fox
    h.send(Msg::NewProjectFromIdeas(vec![fox.clone(), lighthouse]));
    // always named first: the first idea title is the guess
    assert_eq!(h.state.from_ideas.as_ref().unwrap().name, "Fox");
    h.send(Msg::ConfirmFromIdeas);
    h.settle();
    assert_eq!((h.state.tab, h.state.projects_tab), (Tab::Projects, ProjectsTab::Generate));
    let p = h.state.project_entry().unwrap();
    assert_eq!(p.name(), "Fox");
    assert_eq!(p.dir, Path::new("projects/Fox/Fox"), "in a folder of its name by default");
    assert_eq!(p.project.sources.len(), 2);
    assert_eq!(h.state.generate.prompt, "a red fox in a café");
    assert_eq!(h.state.generate.source_idea.as_ref(), Some(&fox));

    // queue without servers: the run is saved for later
    let mut f = h.state.generate.clone();
    f.seeds = 3;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    h.settle();
    assert!(h.errors().is_empty(), "{:?}", h.errors());
    let p = h.state.project_entry().unwrap();
    assert_eq!(p.runs.len(), 1);
    let run = &p.runs[0];
    assert_eq!((run.label.as_str(), run.alpha, run.seed_count(), run.sample.cfg), ("r00", false, 3, 2.0));
    assert_eq!(increart_core::request::prompt_sent(&run.prompt, run.alpha), run.prompt, "the prompt as typed");
    assert_eq!(run.model.recipe, "qwen-image-21");
    assert_eq!(h.state.generate.label, "r01", "next label");
    assert_eq!(h.state.generate.first_seed, 3, "first seed continues after the run's seeds");
    let mut f = h.state.generate.clone();
    f.seeds = 2;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    h.settle();
    let runs = &h.state.project_entry().unwrap().runs;
    assert_eq!(runs[1].revisions[0].seeds, increart_core::model::Seeds { first: 3, count: 2 });
    assert_eq!(h.state.generate.first_seed, 5);
    // another project starts at 0; coming back continues where it left off
    let fox_project = h.state.project.clone().unwrap();
    // New project asks the name first: cancelling makes nothing
    let before = h.state.index.projects.len();
    h.send(Msg::NewProject(PathBuf::new()));
    assert!(h.state.renaming.as_ref().is_some_and(|r| r.creating && r.name == "new project"));
    h.send(Msg::CancelRename);
    h.settle();
    assert_eq!(h.state.index.projects.len(), before, "cancelled: no project");
    assert_eq!(h.state.project.as_ref(), Some(&fox_project));
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    assert_eq!(h.state.generate.first_seed, 0, "new project: seed 0");
    h.send(Msg::SelectProject(fox_project));
    assert_eq!(h.state.generate.first_seed, 5, "remembered from the project's runs");
}

#[test]
fn shutdown_lets_the_library_go() {
    let h = Harness::new();
    assert!(matches!(increart_core::Library::open(&h.root), Err(increart_core::Error::Locked(_))));
    // its threads may outlive this (one stopping a KoboldCpp held it)
    h.rt.shutdown(false);
    increart_core::Library::open(&h.root).expect("opened again with the runtime still there");
}

#[test]
fn inbox_filters_narrow_the_triage_queue() {
    let mut h = Harness::new();
    for (t, x, tags) in
        [("Fox", "a red fox", "animals"), ("Owl", "an owl at night", "animals, night"), ("Sea", "waves", "")]
    {
        h.send(Msg::Draft(IdeaDraft { title: t.into(), text: x.into(), tags: tags.into(), images: vec![] }));
        h.send(Msg::AddIdea(AddIdea::Inbox));
        h.settle();
    }
    let titles = |s: &AppState| s.inbox_queue().iter().map(|e| e.idea.title.clone()).collect::<Vec<_>>();
    assert_eq!(titles(&h.state), ["Sea", "Owl", "Fox"]);
    h.send(Msg::InboxCursor(2));
    h.send(Msg::InboxFilter(InboxFilter { tag: Some("animals".into()), ..Default::default() }));
    assert_eq!(titles(&h.state), ["Owl", "Fox"]);
    assert_eq!(h.state.inbox_cursor, 0, "the cursor starts over");
    h.send(Msg::InboxFilter(InboxFilter { text: "NIGHT".into(), ..Default::default() }));
    assert_eq!(titles(&h.state), ["Owl"], "case-blind, tags too");
    h.send(Msg::InboxFilter(InboxFilter { origin: Some(IdeaOrigin::Clipboard), ..Default::default() }));
    assert!(titles(&h.state).is_empty());
    h.send(Msg::InboxFilter(InboxFilter { age: Some(IdeaAge::Older), ..Default::default() }));
    assert!(titles(&h.state).is_empty(), "all added just now");
    h.send(Msg::InboxFilter(InboxFilter { images: Some(false), oldest_first: true, ..Default::default() }));
    assert_eq!(titles(&h.state), ["Fox", "Owl", "Sea"]);
    // the rating keys go through what's shown
    h.send(Msg::InboxFilter(InboxFilter { tag: Some("night".into()), ..Default::default() }));
    h.send(Msg::ReviewCurrentIdea(Verdict::Bad));
    h.settle();
    assert_eq!(h.state.discarded_ideas()[0].idea.title, "Owl");
    assert!(h.state.inbox_queue().is_empty());
    assert_eq!(h.state.unreviewed_ideas().len(), 2, "the others wait");
}

#[test]
fn details_panels_start_shown_and_hide_for_the_session() {
    let mut h = Harness::new();
    assert!(h.state.info_hidden.is_empty(), "all shown at start");
    h.send(Msg::ShowInfo(InfoPanel::Output, false));
    assert!(h.state.info_hidden.contains(&InfoPanel::Output));
    h.send(Msg::ShowInfo(InfoPanel::Output, true));
    assert!(h.state.info_hidden.is_empty());
    h.send(Msg::ShowInfo(InfoPanel::Review, false));
    assert!(h.state.info_hidden.contains(&InfoPanel::Review));
}

#[test]
fn grids_zoom_each_on_its_own_within_the_thumbnails_size() {
    let mut h = Harness::new();
    assert_eq!(h.state.grid_cell_size(ZoomGrid::Review), 200, "the default");
    assert_eq!(h.state.grid_cell_size(ZoomGrid::Output), 180);
    h.send(Msg::GridZoom { grid: ZoomGrid::Review, size: 250, save: true });
    assert_eq!((h.state.grid_cell_size(ZoomGrid::Review), h.state.config.review_grid_cell), (250, 250));
    assert_eq!(h.state.grid_cell_size(ZoomGrid::Output), 180, "the other grid keeps its size");
    h.send(Msg::GridZoom { grid: ZoomGrid::Output, size: 10_000, save: false });
    assert_eq!(h.state.grid_cell_size(ZoomGrid::Output), GRID_CELL_MAX, "no larger than the thumbnails");
    h.send(Msg::GridZoom { grid: ZoomGrid::Output, size: 1, save: true });
    assert_eq!(h.state.grid_cell_size(ZoomGrid::Output), GRID_CELL_MIN);
}

#[test]
fn add_and_make_a_project_in_one_step() {
    let mut h = Harness::new();
    h.send(Msg::Draft(IdeaDraft {
        title: "Sticker set".into(),
        text: "cute robot sticker".into(),
        ..Default::default()
    }));
    h.send(Msg::AddIdea(AddIdea::NewProject));
    // named first; the draft waits, an empty or taken name is refused
    assert_eq!(h.state.from_ideas.as_ref().unwrap().name, "Sticker set");
    assert!(!h.state.draft.is_empty(), "kept until the project is named");
    h.send(Msg::EditFromIdeasName(" ".into()));
    h.send(Msg::ConfirmFromIdeas);
    assert!(h.state.from_ideas.is_some() && !h.errors().is_empty());
    h.send(Msg::EditFromIdeasName("Robots".into()));
    h.send(Msg::ConfirmFromIdeas);
    h.settle();
    assert!(h.state.from_ideas.is_none());
    assert_eq!(
        h.state.project_entry().unwrap().dir,
        Path::new("projects/Robots/Robots"),
        "the folder follows the name"
    );
    assert_eq!(h.state.library_ideas().len(), 1, "kept, not in the inbox");
    assert!(h.state.unreviewed_ideas().is_empty());
}

#[test]
fn dropped_files_become_ideas() {
    let mut h = Harness::new();
    let dir = h._tmp.path().join("drop");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("note.txt"), "otter with a teacup\n").unwrap();
    image::RgbImage::from_pixel(8, 8, image::Rgb([200, 10, 10])).save(dir.join("red.png")).unwrap();
    h.send(Msg::DropFiles(vec![dir.join("note.txt"), dir.join("red.png")]));
    h.settle();
    let inbox = h.state.unreviewed_ideas();
    assert_eq!(inbox.len(), 2);
    // newest first: the image was imported after the text
    assert_eq!(inbox[1].idea.text, "otter with a teacup");
    assert_eq!(inbox[0].idea.images.len(), 1);

    // the idea panels read its image's file metadata once
    let img = inbox[0].idea.images[0].clone();
    assert_eq!(h.send(Msg::NeedInputMeta(vec![img.clone()])).len(), 1);
    assert!(h.send(Msg::NeedInputMeta(vec![img.clone()])).is_empty(), "already loading");
    h.settle();
    let meta = h.state.input_meta[&img].clone().expect("loaded");
    assert_eq!(meta.source.unwrap().original_name.as_deref(), Some("red.png"));
    assert!(meta.blake3.is_some_and(|b| b.len() == 64));
}

#[test]
fn save_to_input_library_asks_the_name_and_folder() {
    let mut h = Harness::new();
    for (t, x) in [("Fox", "a red fox"), ("Owl", "an owl at night"), ("Sea", "waves")] {
        h.send(Msg::Draft(IdeaDraft { title: t.into(), text: x.into(), tags: String::new(), images: vec![] }));
        h.send(Msg::AddIdea(AddIdea::Inbox));
        h.settle();
    }
    let id = |h: &Harness, t: &str| h.state.inbox_queue().iter().find(|e| e.idea.title == t).unwrap().idea.id.clone();
    let fox = id(&h, "Fox");

    // one idea: named after its title; Cancel leaves it in the inbox
    h.send(Msg::SaveIdeas(vec![fox.clone()]));
    assert_eq!(h.state.save_idea.as_ref().unwrap().name, "Fox");
    h.send(Msg::CancelSaveIdea);
    assert!(h.state.save_idea.is_none());
    assert_eq!(h.state.inbox_queue().len(), 3);

    // renamed, into a new folder
    h.send(Msg::SaveIdeas(vec![fox.clone()]));
    h.send(Msg::EditSaveIdeaName("  ".into()));
    assert!(increart_gui_core::save_idea_problem(h.state.save_idea.as_ref().unwrap()).is_some(), "needs a name");
    h.send(Msg::ConfirmSaveIdea);
    assert!(h.state.save_idea.is_some(), "kept open");
    h.send(Msg::EditSaveIdeaName("Red fox".into()));
    h.send(Msg::SaveIdeaNewFolder(Some("Animals".into())));
    h.send(Msg::ConfirmSaveIdea);
    h.settle();
    assert!(h.state.save_idea.is_none());
    let e = h.state.idea(&fox).unwrap();
    assert_eq!((e.idea.title.as_str(), &e.loc), ("Red fox", &IdeaLoc::Library("Animals".into())));
    assert!(!e.idea.starred);
    assert!(h.state.index.library_folders.contains(&std::path::PathBuf::from("Animals")));

    // several: they keep their titles; the folder starts where the last went
    let (owl, sea) = (id(&h, "Owl"), id(&h, "Sea"));
    h.send(Msg::InboxPick(vec![owl.clone(), sea.clone()]));
    h.send(Msg::SaveIdeas(h.state.inbox_picked.clone()));
    let d = h.state.save_idea.as_ref().unwrap();
    assert_eq!(d.folder, std::path::PathBuf::from("Animals"));
    h.send(Msg::SaveIdeaNewFolder(Some("night".into())));
    h.send(Msg::ConfirmSaveIdea);
    h.settle();
    assert!(h.state.inbox_queue().is_empty() && h.state.inbox_picked.is_empty());
    for (i, t) in [(&owl, "Owl"), (&sea, "Sea")] {
        let e = h.state.idea(i).unwrap();
        assert_eq!((e.idea.title.as_str(), &e.loc), (t, &IdeaLoc::Library("Animals/night".into())));
    }

    // the New idea form's Add & save asks too; the draft waits for it
    h.send(Msg::Draft(IdeaDraft { title: "Moth".into(), text: "a moth".into(), ..Default::default() }));
    h.send(Msg::AddIdea(AddIdea::Keep));
    let d = h.state.save_idea.as_ref().unwrap();
    assert!(d.ideas.is_empty());
    assert_eq!((d.name.as_str(), &d.folder), ("Moth", &std::path::PathBuf::from("Animals/night")));
    h.send(Msg::CancelSaveIdea);
    assert_eq!(h.state.draft.title, "Moth", "Cancel keeps the draft");
    h.send(Msg::AddIdea(AddIdea::Keep));
    h.send(Msg::EditSaveIdeaName("Luna moth".into()));
    h.send(Msg::SaveIdeaFolder("Animals".into()));
    h.send(Msg::SaveIdeaNewFolder(Some("insects".into())));
    h.send(Msg::ConfirmSaveIdea);
    h.settle();
    assert_eq!(h.state.draft, IdeaDraft::default(), "draft cleared");
    let e = h.state.index.ideas.iter().find(|e| e.idea.title == "Luna moth").unwrap();
    assert_eq!(e.loc, IdeaLoc::Library("Animals/insects".into()));
}

#[test]
fn save_and_make_a_project_in_one_step() {
    let mut h = Harness::new();
    h.send(Msg::Draft(IdeaDraft { title: "Fox".into(), text: "a red fox".into(), ..Default::default() }));
    h.send(Msg::AddIdea(AddIdea::Inbox));
    h.settle();
    let fox = h.state.inbox_queue()[0].idea.id.clone();

    // an inbox idea: saved where asked, named as asked; the project too
    h.send(Msg::SaveIdeas(vec![fox.clone()]));
    h.send(Msg::EditSaveIdeaName("Red fox".into()));
    h.send(Msg::SaveIdeaNewFolder(Some("Animals".into())));
    h.send(Msg::SaveIdeaNewProject);
    h.settle();
    assert!(h.state.save_idea.is_none() && h.state.from_ideas.is_none(), "no dialog left");
    let e = h.state.idea(&fox).unwrap();
    assert_eq!((e.idea.title.as_str(), &e.loc), ("Red fox", &IdeaLoc::Library("Animals".into())));
    let dirs: Vec<_> = h.state.index.projects.iter().map(|p| p.dir.clone()).collect();
    assert_eq!(dirs, [Path::new("projects/Red fox/Red fox")]);

    // the draft: saved in the folder chosen, as the project is made
    h.send(Msg::Draft(IdeaDraft { title: "Moth".into(), text: "a moth".into(), ..Default::default() }));
    h.send(Msg::AddIdea(AddIdea::Keep));
    h.send(Msg::EditSaveIdeaName("Luna moth".into()));
    h.send(Msg::SaveIdeaNewFolder(Some("insects".into())));
    h.send(Msg::SaveIdeaNewProject);
    h.settle();
    assert_eq!(h.state.draft, IdeaDraft::default(), "draft cleared");
    let e = h.state.index.ideas.iter().find(|e| e.idea.title == "Luna moth").unwrap();
    assert_eq!(e.loc, IdeaLoc::Library("Animals/insects".into()));
    assert!(h.state.index.projects.iter().any(|p| p.dir == Path::new("projects/Luna moth/Luna moth")));

    // a name that won't do for a project: the New project dialog asks
    h.send(Msg::Draft(IdeaDraft { title: "Owl".into(), text: "an owl".into(), ..Default::default() }));
    h.send(Msg::AddIdea(AddIdea::Inbox));
    h.settle();
    let owl = h.state.inbox_queue()[0].idea.id.clone();
    h.send(Msg::SaveIdeas(vec![owl.clone()]));
    h.send(Msg::EditSaveIdeaName("owl/night".into()));
    h.send(Msg::SaveIdeaNewProject);
    h.settle();
    let p = h.state.from_ideas.as_ref().expect("asked");
    assert_eq!(p.ideas, std::slice::from_ref(&owl));
    assert_eq!(h.state.idea(&owl).unwrap().idea.title, "owl/night", "saved all the same");
}

#[test]
fn idea_picks_tile_and_triage_together() {
    let mut h = Harness::new();
    let dir = h._tmp.path().join("drop");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("note.txt"), "otter with a teacup\n").unwrap();
    for (n, c) in [("red", 200), ("green", 100)] {
        image::RgbImage::from_pixel(8, 8, image::Rgb([c, 10, 10])).save(dir.join(format!("{n}.png"))).unwrap();
    }
    h.send(Msg::DropFiles(vec![dir.join("note.txt"), dir.join("red.png"), dir.join("green.png")]));
    h.settle();
    h.send(Msg::Tab(Tab::IdeaInbox));
    let ids: Vec<_> = h.state.inbox_queue().iter().map(|e| e.idea.id.clone()).collect();
    assert_eq!(ids.len(), 3);
    let images = |h: &Harness, ids: &[increart_core::Id]| -> Vec<increart_core::ImageRef> {
        ids.iter()
            .flat_map(|id| h.state.idea(id).unwrap().idea.images.clone())
            .map(increart_core::ImageRef::input)
            .collect()
    };

    // the picks' images tiled, in pick order; the text idea adds none
    let picks: Vec<_> = ids.iter().rev().cloned().collect();
    h.send(Msg::InboxPick(picks.clone()));
    h.send(Msg::TileIdeas(h.state.inbox_picked.clone()));
    let tiled: Vec<_> = h.state.tile.as_ref().unwrap().layout.images().cloned().collect();
    assert_eq!(tiled, images(&h, &picks));
    assert_eq!(tiled.len(), 2);
    // an input has no seed or run: the caption format leaves it alone
    h.send(Msg::TileCaption(0, "red, by hand".into()));
    h.send(Msg::TileCaptionFormat("seed {seed}".into()));
    assert_eq!(h.state.tile.as_ref().unwrap().layout.tiles[0].caption, "red, by hand");
    h.send(Msg::CloseTile);
    let text = ids.iter().find(|id| h.state.idea(id).unwrap().idea.images.is_empty()).unwrap().clone();
    h.send(Msg::TileIdeas(vec![text.clone()]));
    assert!(h.state.tile.is_none(), "no images: nothing to tile");

    // all the picks kept at once; they leave the inbox and the picks
    let (two, rest): (Vec<_>, Vec<_>) = ids.iter().cloned().partition(|id| *id != text);
    h.send(Msg::InboxPick(two.clone()));
    h.send(Msg::ReviewPickedIdeas(Verdict::Neutral));
    h.settle();
    assert!(h.state.inbox_picked.is_empty());
    let inbox: Vec<_> = h.state.inbox_queue().iter().map(|e| e.idea.id.clone()).collect();
    assert_eq!(inbox, rest);
    // a pick that leaves the inbox leaves the picks; a tab change drops them
    h.send(Msg::InboxPick(rest.clone()));
    h.send(Msg::Tab(Tab::InputLibrary));
    assert!(h.state.inbox_picked.is_empty());

    // the Input library's selection set at once (a range, the band)
    h.send(Msg::LibSelectSet(two.clone()));
    assert_eq!(
        h.state.lib_selected.iter().cloned().collect::<std::collections::BTreeSet<_>>(),
        two.iter().cloned().collect()
    );
    h.send(Msg::TileIdeas(two.clone()));
    assert_eq!(h.state.tile.as_ref().unwrap().layout.images().count(), 2);
}

#[test]
fn clipboard_paths_and_images_start_a_tiling() {
    use increart_gui_core::tile::{TileContent, TileImage};
    let mut h = Harness::new();
    let dir = h._tmp.path().join("drop");
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbImage::from_pixel(8, 8, image::Rgb([200, 10, 10])).save(dir.join("red.png")).unwrap();
    h.send(Msg::DropFiles(vec![dir.join("red.png")]));
    h.settle();
    let idea = h.state.unreviewed_ideas()[0].clone();
    let red = increart_core::ImageRef::input(idea.idea.images[0].clone());

    // what pasted paths name: an idea's folder (relative, as the folder view
    // copies it, or absolute), a store file; files elsewhere aren't the library's
    assert_eq!(h.state.images_at(&idea.dir), std::slice::from_ref(&red));
    assert_eq!(h.state.images_at(&h.root.join(&idea.dir)), std::slice::from_ref(&red));
    assert_eq!(h.state.images_at(&red.path(&h.root)), std::slice::from_ref(&red));
    assert!(h.state.images_at(&dir.join("red.png")).is_empty());
    assert!(h.state.images_at(Path::new("notes/todo")).is_empty());
    assert!(h.state.images_at(&h.root).is_empty(), "not the whole library");
    // a folder: every image under it, once; a project: its outputs
    assert!(h.state.images_at(idea.dir.parent().unwrap()).contains(&red));
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let p = h.state.index.project(&project).unwrap();
    let outputs: Vec<_> = p.outputs.iter().map(|o| o.image.clone()).collect();
    assert_eq!(outputs.len(), 2);
    assert_eq!(h.state.images_at(&h.root.join(&p.dir)), outputs);

    // nothing: no tiling
    h.send(Msg::TileStart(vec![]));
    assert!(h.state.tile.is_none());
    assert!(h.state.messages.iter().any(|m| m.text == "Nothing to tile"));
    // a library image and a pasted one: a new tiling, square
    let pasted = || TileContent::Pasted(TileImage(std::sync::Arc::new(image::RgbaImage::new(8, 8))));
    h.send(Msg::TileStart(vec![TileContent::Image(red.clone()), pasted()]));
    let t = h.state.tile.as_ref().unwrap();
    assert_eq!(t.layout.cols, 2);
    assert_eq!(t.layout.tiles.iter().map(|x| (x.col, x.row)).collect::<Vec<_>>(), [(0, 0), (1, 0)]);
    assert_eq!(t.layout.tiles[0].image(), Some(&red));
    // with one open: after its last tile
    h.send(Msg::TileStart(vec![pasted()]));
    let t = h.state.tile.as_ref().unwrap();
    assert_eq!((t.layout.tiles.len(), t.layout.tiles[2].col, t.layout.tiles[2].row), (3, 0, 1));
    assert_eq!(t.selected, Some(2));
}

#[test]
fn trees_create_rename_move_delete() {
    let mut h = Harness::new();
    h.send(Msg::NewFolder(increart_core::Tree::Projects, PathBuf::new()));
    h.settle();
    let r = h.state.renaming.clone().expect("new folder opens for rename");
    assert_eq!(r.path, Path::new("new folder"));
    h.send(Msg::EditRename("clients".into()));
    h.send(Msg::CommitRename);
    h.settle();
    assert_eq!(h.state.index.project_folders, [PathBuf::from("clients")]);

    // in a folder: from its right-click menu
    h.send(Msg::NewProject("clients".into()));
    h.send(Msg::CommitRename);
    h.settle();
    let p = h.state.project_entry().expect("new project selected");
    assert_eq!(p.dir, Path::new("projects/clients/new project"));
    h.send(Msg::CancelRename);
    h.send(Msg::MoveNode(increart_core::Tree::Projects, "clients/new project".into(), PathBuf::new()));
    h.settle();
    assert_eq!(h.state.project_entry().unwrap().dir, Path::new("projects/new project"), "selection follows the id");

    h.send(Msg::StartRename(increart_core::Tree::Projects, "clients".into()));
    h.send(Msg::EditRename("a/b".into()));
    h.send(Msg::CommitRename);
    h.settle();
    assert_eq!(h.errors().len(), 1, "invalid name refused");
}

#[test]
fn discarded_projects_restore_and_delete_for_good() {
    use increart_core::Tree;
    let mut h = Harness::new();
    h.send(Msg::NewFolder(Tree::Projects, PathBuf::new()));
    h.settle();
    h.send(Msg::EditRename("clients".into()));
    h.send(Msg::CommitRename);
    h.settle();
    // "new project" and "new project 2"
    for _ in 0..2 {
        h.send(Msg::NewProject("clients".into()));
        h.send(Msg::CommitRename);
        h.settle();
        h.send(Msg::CancelRename);
    }
    let names = |h: &Harness| h.state.index.projects.iter().map(|p| p.name()).collect::<Vec<_>>();
    assert_eq!(names(&h).len(), 2);
    let id = |h: &Harness, n: &str| {
        let d = h.state.index.discarded_projects.iter().find(|d| d.discarded.name == n).unwrap();
        d.entry.project.id.clone()
    };

    // one project: to Discarded, from its folder
    h.send(Msg::DeleteNode(Tree::Projects, "clients/new project".into()));
    h.settle();
    assert_eq!(names(&h), ["new project 2"]);
    let d = &h.state.index.discarded_projects[0];
    assert_eq!((d.discarded.name.as_str(), d.discarded.from.as_path()), ("new project", Path::new("clients")));
    h.send(Msg::RestoreProject(id(&h, "new project")));
    h.settle();
    assert!(h.state.index.discarded_projects.is_empty());
    assert!(h.state.index.projects.iter().any(|p| p.dir == Path::new("projects/clients/new project")));

    // a folder: its projects to Discarded, the folder to the trash; restored
    // with the folder gone, to the top level
    h.send(Msg::DeleteNode(Tree::Projects, "clients".into()));
    h.settle();
    assert!(h.state.index.projects.is_empty() && h.state.index.project_folders.is_empty());
    assert_eq!(h.state.index.discarded_projects.len(), 2);
    h.send(Msg::RestoreProject(id(&h, "new project 2")));
    h.settle();
    assert_eq!(h.state.index.projects[0].dir, Path::new("projects/new project 2"));

    // for good
    h.send(Msg::EmptyDiscardedProjects);
    h.settle();
    assert!(h.state.index.discarded_projects.is_empty());
    assert_eq!(names(&h), ["new project 2"]);
    assert!(h.errors().is_empty(), "{:?}", h.errors());
}

#[test]
fn library_settings_convert_the_stores() {
    use increart_core::model::StoreFormat;
    use increart_gui_core::state::SettingsTab;
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::Settings(true));
    h.send(Msg::SettingsTab(SettingsTab::Library));
    h.settle();
    let info = h.state.store.unwrap();
    assert_eq!((info.format, info.counts.png.0, info.counts.webp.0), (StoreFormat::Png, 2, 0));
    h.send(Msg::SetStoreFormat(StoreFormat::Webp));
    h.settle();
    assert_eq!(h.state.store.unwrap().format, StoreFormat::Webp);
    // asks first
    h.send(Msg::ConvertStore);
    assert!(h.state.confirm.is_some());
    h.send(Msg::ConvertStoreConfirmed);
    assert!(h.state.converting.is_some());
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while h.state.converting.is_some() {
        assert!(std::time::Instant::now() < deadline, "conversion still going");
        h.settle();
    }
    h.settle();
    assert!(h.errors().is_empty(), "{:?}", h.errors());
    let info = h.state.store.unwrap();
    assert_eq!((info.counts.png.0, info.counts.webp.0), (0, 2));
    let links = h.state.review_queue();
    assert_eq!(links.len(), 2);
    assert!(links.iter().all(|o| o.path.extension().unwrap() == "webp"), "the index follows the new names");
    assert!(h.state.messages.iter().any(|m| m.text.starts_with("Converted 2 image(s)")));
}

#[test]
fn review_and_undo() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    // Outputs need sd-server; write two through a second handle on the
    // library files, as the engine would (the runtime's handle holds the lock).
    let run = {
        let lib = Library_::steal(&h.root);
        lib.write_two(&project)
    };
    h.rt.run(Effect::Rescan);
    h.settle();
    assert_eq!(h.state.review_queue().len(), 2);
    assert_eq!(h.state.unreviewed_outputs(), 2);
    h.send(Msg::ReviewCurrent(Verdict::Good));
    h.settle();
    assert_eq!(h.state.review_queue().len(), 1);
    assert_eq!(h.state.kept_outputs().len(), 1);
    h.send(Msg::UndoReview);
    h.settle();
    assert_eq!(h.state.review_queue().len(), 2, "undone");
    assert!(h.state.review_undo.is_empty());
    assert_eq!(h.state.index.project(&project).unwrap().runs[0].id, run);
}

#[test]
fn quick_ratings_go_to_the_next_image() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    {
        let lib = Library_::steal(&h.root);
        lib.write_two(&project);
    }
    h.rt.run(Effect::Rescan);
    h.settle();
    let queue: Vec<_> = h.state.review_queue().iter().map(|o| (o.image.clone(), o.path.clone())).collect();
    assert_eq!(queue.len(), 2);
    // the second key before the first rating's index is back
    h.send(Msg::ReviewCurrent(Verdict::Good));
    h.send(Msg::ReviewCurrent(Verdict::Bad));
    h.settle();
    assert_eq!(h.errors(), Vec::<&str>::new());
    assert!(h.state.reviewing.is_empty());
    let verdict = |h: &Harness, i: usize| h.state.output_link(&queue[i].0).unwrap().verdict;
    assert_eq!((verdict(&h, 0), verdict(&h, 1)), (Some(Verdict::Good), Some(Verdict::Bad)));
    // a rating by the path from before: the link where it is now
    h.send(Msg::ReviewLink(queue[0].1.clone(), Some(Verdict::Neutral)));
    h.settle();
    assert_eq!(h.errors(), Vec::<&str>::new());
    assert_eq!(verdict(&h, 0), Some(Verdict::Neutral));
    h.send(Msg::UndoReview);
    h.settle();
    assert_eq!(verdict(&h, 0), Some(Verdict::Good), "undo knows where it was");
}

#[test]
fn review_tree_sorts_and_hides_reviewed() {
    let mut h = Harness::new();
    let from_ideas = |h: &mut Harness, name: &str, folder: Option<&str>| {
        h.send(Msg::Draft(IdeaDraft { title: name.into(), text: "x".into(), ..Default::default() }));
        h.send(Msg::AddIdea(AddIdea::NewProject));
        h.send(Msg::FromIdeasParent(PathBuf::new()));
        h.send(Msg::FromIdeasNewFolder(folder.map(String::from)));
        h.send(Msg::ConfirmFromIdeas);
        h.settle();
        h.state.project.clone().unwrap()
    };
    let zeta = from_ideas(&mut h, "Zeta", Some("Zfold"));
    let mid = from_ideas(&mut h, "Mid", Some("Mfold"));
    let alpha = from_ideas(&mut h, "Alpha", None);
    let beta = from_ideas(&mut h, "Beta", None);
    for p in [&zeta, &mid, &beta] {
        Library_::steal(&h.root).write_two(p);
    }
    h.rt.run(Effect::Rescan);
    h.settle();
    let tree = |h: &Harness| {
        let (f, p) = h.state.review_tree_children(Path::new(""));
        let f: Vec<String> = f.iter().map(|f| f.display().to_string()).collect();
        (f, p.iter().map(|p| p.name()).collect::<Vec<_>>())
    };
    // A–Z, hiding what has nothing to review (Alpha)
    assert_eq!(tree(&h), (vec!["Mfold".into(), "Zfold".into()], vec!["Beta".to_string()]));
    // a folder by the oldest project in it with images to review
    h.send(Msg::ReviewOrder(ReviewOrder::OldestToReview));
    assert_eq!(h.state.config.review_order, ReviewOrder::OldestToReview);
    assert_eq!(tree(&h).0, ["Zfold", "Mfold"]);
    // shown again: those with nothing to review last
    h.send(Msg::ReviewHideReviewed(false));
    assert!(h.state.config.review_show_reviewed);
    assert_eq!(tree(&h).1, ["Beta", "Alpha"]);
    // newest created first, as the project tree can
    h.send(Msg::ReviewOrder(ReviewOrder::Created));
    assert_eq!(tree(&h), (vec!["Mfold".into(), "Zfold".into()], vec!["Beta".to_string(), "Alpha".into()]));
    h.send(Msg::ReviewOrder(ReviewOrder::Name));
    assert_eq!(tree(&h).1, ["Alpha", "Beta"]);
    // hidden, except the selected one
    h.send(Msg::ReviewHideReviewed(true));
    h.send(Msg::ReviewScope(Some(alpha)));
    assert_eq!(tree(&h).1, ["Alpha", "Beta"]);
}

/// Writes outputs without the lock: only for setting up `review_and_undo`.
struct Library_ {
    root: PathBuf,
}

impl Library_ {
    fn steal(root: &Path) -> Library_ {
        Library_ { root: root.to_path_buf() }
    }

    /// A run with two seeds and their output files and links, made with
    /// a throwaway copy of the library opened elsewhere.
    fn write_two(&self, project: &increart_core::Id) -> increart_core::Id {
        self.write_two_with(project, "r01", vec![])
    }

    /// A run `label` made from `refs`, with outputs for seeds 0 and 1.
    fn write_two_with(
        &self,
        project: &increart_core::Id,
        label: &str,
        refs: Vec<increart_core::ImageRef>,
    ) -> increart_core::Id {
        self.in_copy(|lib| {
            let mut cfg = Config::default();
            cfg.normalize();
            let form = GenerateForm::new(&cfg);
            let run = increart_core::NewRun {
                sweep: None,
                label: label.into(),
                prompt: "x".into(),
                negative_prompt: String::new(),
                alpha: false,
                width: 64,
                height: 64,
                sample: increart_core::model::Sample {
                    method: "euler".into(),
                    scheduler: String::new(),
                    steps: form.steps,
                    cfg: form.cfg,
                    flow_shift: None,
                    custom_sigmas: vec![],
                },
                mode: increart_core::model::Mode::Generate,
                refs,
                model: increart_core::model::ModelSelection {
                    recipe: "qwen-image-21".into(),
                    files: Default::default(),
                    loras: vec![],
                },
                source_idea: None,
                first_seed: 0,
                seeds: 2,
            };
            let id = lib.create_run(project, run).unwrap();
            write_pngs(lib, &id, &[0, 1]);
            id
        })
    }

    /// Outputs for these seeds of an existing run, as the engine would write.
    fn write_seeds(&self, run: &increart_core::Id, seeds: &[u64]) {
        self.in_copy(|lib| write_pngs(lib, run, seeds));
    }

    /// Runs `f` on a copy of the library, then moves the new files into the
    /// real one.
    fn in_copy<T>(&self, f: impl FnOnce(&mut increart_core::Library) -> T) -> T {
        let tmp = tempfile::tempdir().unwrap();
        let copy = tmp.path().join("copy");
        increart_core::fsutil::copy_tree(&self.root, &copy).unwrap();
        let mut lib = increart_core::Library::open(&copy).unwrap();
        // A past minute, so the IDs made here can't collide with ones the
        // app's own handle hands out later (it never sees this handle's).
        lib.set_clock(|| chrono::DateTime::from_timestamp(1_600_000_000, 0).unwrap());
        let out = f(&mut lib);
        drop(lib);
        for sub in ["output_images", "projects"] {
            for p in increart_core::fsutil::walk(&copy.join(sub)).unwrap() {
                let dst = self.root.join(p.strip_prefix(&copy).unwrap());
                if p.is_dir() && !increart_core::fsutil::is_symlink(&p) {
                    std::fs::create_dir_all(&dst).unwrap();
                } else if !increart_core::fsutil::exists(&dst) {
                    std::fs::rename(&p, &dst).unwrap();
                }
            }
        }
        out
    }
}

fn write_pngs(lib: &mut increart_core::Library, run: &increart_core::Id, seeds: &[u64]) {
    let (_, def) = lib.find_run(run).unwrap();
    for &seed in seeds {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([seed as u8, 0, 0])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let p = increart_core::request::Provenance::default();
        lib.write_output(run, 0, seed, &png, &increart_core::request::recipe(&def, seed, &p)).unwrap();
    }
}

// ---------------------------------------------------------------- server dialog (pure)

fn dialog_state(presets: Vec<ServerPreset>, last: Option<&str>) -> AppState {
    let mut config = Config::default();
    config.normalize();
    config.server_presets = presets;
    config.last_preset = last.map(String::from);
    let mut s = AppState::new(config, Some("/nonexistent/config.rsn".into()));
    let ix = increart_core::Index::default();
    update(&mut s, Msg::LibraryOpened(Ok(("/lib".into(), ix))));
    s
}

fn start(name: &str, port: u16) -> ServerConfig {
    ServerConfig::Start { name: name.into(), host: "127.0.0.1".into(), port, device: "ROCm0".into(), args: vec![] }
}

fn dialog(s: &AppState) -> &ServerDialog {
    match &s.phase {
        Phase::Servers(d) => d,
        other => panic!("{other:?}"),
    }
}

#[test]
fn command_line_servers_skip_the_screen_only_when_valid() {
    let open = |presets: Vec<ServerPreset>, last: Option<&str>, choice: StartupServers| {
        let mut config = Config::default();
        config.normalize();
        (config.server_presets, config.last_preset) = (presets, last.map(String::from));
        config.programs.sd_server.binary = "/nonexistent/sd-server".into();
        let mut s = AppState::new(config, Some("/nonexistent/config.rsn".into()));
        s.startup_servers = Some(choice);
        let fx = update(&mut s, Msg::LibraryOpened(Ok(("/lib".into(), increart_core::Index::default()))));
        (s, fx)
    };
    let remote = || ServerPreset {
        name: "Remote".into(),
        servers: vec![ServerConfig::Connect {
            name: "r".into(),
            url: "http://10.0.0.2:1234".into(),
            device: String::new(),
        }],
    };

    let (s, fx) = open(vec![], None, StartupServers::Without);
    assert_eq!((s.phase, fx.last()), (Phase::Main, Some(&Effect::StopServers)));

    let (s, fx) = open(vec![remote()], None, StartupServers::Preset("remote".into()));
    assert_eq!(s.phase, Phase::Main, "any case; the main window opens while it connects");
    assert!(s.background.is_some() && fx.iter().any(|f| matches!(f, Effect::OpenServers { .. })));
    let (s, _) = open(vec![remote()], Some("Remote"), StartupServers::Preset("last".into()));
    assert_eq!(s.phase, Phase::Main);

    for (presets, last, asked, why) in [
        (vec![remote()], None, "nope", "no preset by that name"),
        (vec![remote()], None, "last", "no preset opened yet"),
        (vec![ServerPreset { name: "Empty".into(), servers: vec![] }], None, "Empty", "has no servers"),
        (vec![ServerPreset { name: "GPU".into(), servers: vec![start("gpu0", 1234)] }], None, "GPU", "sd-server"),
    ] {
        let (s, fx) = open(presets, last, StartupServers::Preset(asked.into()));
        let notice = dialog(&s).notice.clone().unwrap_or_default();
        assert!(notice.contains(why) && !dialog(&s).opening, "{asked}: {notice}");
        assert!(!fx.iter().any(|f| matches!(f, Effect::OpenServers { .. })));
        assert_eq!(s.startup_servers, None, "used once");
    }
}

#[test]
fn server_dialog_click_opens_a_preset() {
    let a = ServerPreset { name: "A".into(), servers: vec![start("gpu0", 1234)] };
    let b = ServerPreset { name: "B".into(), servers: vec![start("gpu0", 1234), start("gpu1", 1235)] };
    let mut s = dialog_state(vec![a, b.clone()], Some("A"));
    assert_eq!(dialog(&s).editing, None, "nothing is edited until asked");
    let fx = update(&mut s, Msg::OpenPreset(1));
    assert!(matches!(fx.last(), Some(Effect::OpenServers { .. })), "no servers yet: opens without asking");
    assert!(s.confirm.is_none());
    assert!(dialog(&s).opening);
    assert_eq!(dialog(&s).draft, b);
    assert_eq!(s.config.last_preset.as_deref(), Some("B"));
}

#[test]
fn server_dialog_asks_before_replacing_servers_in_use() {
    let p = ServerPreset { name: "One".into(), servers: vec![start("gpu0", 1234)] };
    let mut s = dialog_state(vec![p], None);
    s.engine_running = true;
    let other = start("gpu1", 1235).with_device("ROCm1");
    s.servers = vec![ServerStatus {
        config: Some(other),
        name: "gpu1".into(),
        device: "ROCm1".into(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    }];
    assert!(update(&mut s, Msg::OpenPreset(0)).is_empty(), "asks first");
    assert!(s.confirm.as_ref().unwrap().text.contains("open One"));
    assert!(!dialog(&s).opening);
    let fx = update(&mut s, Msg::ConfirmYes);
    assert!(matches!(fx.last(), Some(Effect::OpenServers { .. })));
    assert!(dialog(&s).opening);
}

#[test]
fn server_dialog_new_edit_save_and_delete() {
    let mut s = dialog_state(vec![], None);
    update(&mut s, Msg::NewPreset);
    assert_eq!(dialog(&s).editing, Some(Editing::New));
    update(&mut s, Msg::AddStartServer);
    update(&mut s, Msg::AddStartServer);
    update(&mut s, Msg::AddConnectServer);
    let d = dialog(&s);
    assert_eq!(d.draft.servers[1], start("gpu1", 1235).clone().with_device("ROCm1"));
    assert!(matches!(&d.draft.servers[2], ServerConfig::Connect { .. }));
    update(&mut s, Msg::RemoveServer(2));

    assert!(update(&mut s, Msg::SavePreset).is_empty(), "needs a name");
    assert!(dialog(&s).editing.is_some(), "the editor stays open");
    update(&mut s, Msg::SetPresetName(" Both GPUs ".into()));
    let fx = update(&mut s, Msg::SavePreset);
    assert!(matches!(fx[..], [Effect::SaveConfig(..)]));
    assert_eq!(s.config.server_presets[0].name, "Both GPUs");
    assert_eq!(dialog(&s).editing, None, "saving closes the editor");
    assert_eq!(s.config.last_preset, None, "saving is not using");

    // a second one; renaming the first in place, not onto the second's name
    update(&mut s, Msg::NewPreset);
    update(&mut s, Msg::AddStartServer);
    update(&mut s, Msg::SetPresetName("One".into()));
    update(&mut s, Msg::SavePreset);
    s.config.last_preset = Some("Both GPUs".into());
    update(&mut s, Msg::EditPreset(0));
    update(&mut s, Msg::SetPresetName("One".into()));
    assert!(update(&mut s, Msg::SavePreset).is_empty(), "name taken");
    update(&mut s, Msg::SetPresetName("Two GPUs".into()));
    update(&mut s, Msg::SavePreset);
    let names: Vec<_> = s.config.server_presets.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Two GPUs", "One"]);
    assert_eq!(s.config.last_preset.as_deref(), Some("Two GPUs"), "the last used follows a rename");

    // Cancel drops the edits
    update(&mut s, Msg::EditPreset(1));
    update(&mut s, Msg::RemoveServer(0));
    update(&mut s, Msg::CancelEdit);
    assert_eq!(s.config.server_presets[1].servers.len(), 1);

    // deleting the edited preset closes the editor
    update(&mut s, Msg::EditPreset(1));
    update(&mut s, Msg::DeletePreset(1));
    assert_eq!(dialog(&s).editing, None);
    assert_eq!(s.config.server_presets.len(), 1);

    // Open without saving: the edited servers, the preset unchanged
    update(&mut s, Msg::EditPreset(0));
    update(&mut s, Msg::RemoveServer(1));
    let fx = update(&mut s, Msg::OpenDraft);
    assert!(matches!(fx.last(), Some(Effect::OpenServers { servers, .. }) if servers.len() == 1));
    assert_eq!(s.config.server_presets[0].servers.len(), 2);
    assert_eq!(dialog(&s).editing, None);
}

#[test]
fn server_dialog_save_and_open_then_partial_failure() {
    let mut s = dialog_state(vec![], None);
    update(&mut s, Msg::NewPreset);
    update(&mut s, Msg::AddStartServer);
    update(&mut s, Msg::AddStartServer);
    update(&mut s, Msg::SetPresetName("Both GPUs".into()));
    let fx = update(&mut s, Msg::SaveAndOpen);
    assert!(matches!(fx[0], Effect::SaveConfig(..)));
    assert!(matches!(fx.last(), Some(Effect::OpenServers { servers, .. }) if servers.len() == 2));
    assert_eq!(s.config.server_presets.len(), 1);
    assert_eq!(s.config.last_preset.as_deref(), Some("Both GPUs"));
    assert_eq!(s.servers.len(), 2);
    let a = s.open_attempt;
    // what each starting server's log says, until it's up; older attempts' lines are ignored
    update(&mut s, Msg::ServerLog(a, 0, "294/396 · 4.38GB/s".into()));
    update(&mut s, Msg::ServerLog(a - 1, 1, "stale".into()));
    assert_eq!(s.start_log, ["294/396 · 4.38GB/s", ""]);
    update(&mut s, Msg::ServerOpened(a, 0, Ok(())));
    update(&mut s, Msg::ServerLog(a, 0, "later".into()));
    assert_eq!(s.start_log[0], "294/396 · 4.38GB/s", "up: no longer starting");
    update(&mut s, Msg::ServerOpened(a, 1, Err("ROCm1: out of memory".into())));
    assert!(update(&mut s, Msg::ServersSettled(a)).is_empty(), "one failed: the dialog stays");
    assert_eq!(dialog(&s).progress[1], ServerProgress::Failed("ROCm1: out of memory".into()));
    let fx = update(&mut s, Msg::ContinueWithServers);
    assert!(matches!(fx[..], [Effect::StartEngine { .. }]));
    assert_eq!(s.phase, Phase::Main);
    assert_eq!(s.servers.len(), 1, "only the server that came up");

    // stopping: shown as stopping until the processes are gone
    update(&mut s, Msg::OpenServerDialog);
    let fx = update(&mut s, Msg::ContinueWithoutServers);
    assert_eq!(fx, [Effect::StopServers]);
    assert!(s.servers.is_empty());
    assert_eq!(s.stopping.len(), 1);
    update(&mut s, Msg::ServersStopped(vec!["gpu0".into()]));
    assert!(s.stopping.is_empty());
    assert!(s.messages.last().is_some_and(|m| m.text.contains("gpu0") && !m.error), "{:?}", s.messages);
}

#[test]
fn server_wizard_ticks_the_gpus_and_opens_them() {
    let mut s = dialog_state(vec![], None);
    let fx = update(&mut s, Msg::ServerWizard(true));
    assert_eq!(
        fx,
        [Effect::DetectGpus(GpuRuntime::Vulkan), Effect::KoboldVersion("koboldcpp".into())],
        "KoboldCpp first, its binary checked"
    );
    assert!(dialog(&s).wizard && dialog(&s).kobold.is_some());
    let fx = update(&mut s, Msg::WizardKobold(false));
    assert!(matches!(fx[..], [Effect::ListDevices(_)]), "lists the devices at once");
    assert!(dialog(&s).wizard && dialog(&s).kobold.is_none());
    let listed = ["ROCm0\tAMD Radeon AI PRO R9700", "ROCm1\tAMD Radeon AI PRO R9700", "CPU\tIntel"];
    update(&mut s, Msg::Devices(Ok(listed.iter().map(|l| l.to_string()).collect())));
    let ports: Vec<_> = dialog(&s)
        .draft
        .servers
        .iter()
        .map(|x| match x {
            ServerConfig::Start { device, port, .. } => (device.as_str(), *port),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(ports, [("ROCm0", 1234), ("ROCm1", 1235)], "every GPU, not the CPU");

    update(&mut s, Msg::DraftDevice("ROCm0".into(), false));
    update(&mut s, Msg::DraftDevice("ROCm1".into(), true));
    assert_eq!(dialog(&s).draft.servers.len(), 1, "ticking twice adds once");
    update(&mut s, Msg::DraftDevice("ROCm0".into(), true));
    let fx = update(&mut s, Msg::FinishServerWizard);
    assert!(matches!(fx[0], Effect::SaveConfig(..)));
    assert!(matches!(fx.last(), Some(Effect::OpenServers { servers, .. }) if servers.len() == 2));
    assert_eq!(s.config.server_presets[0].name, "ROCm1 + ROCm0");
    assert_eq!(s.config.last_preset.as_deref(), Some("ROCm1 + ROCm0"));
    assert!(!dialog(&s).wizard && dialog(&s).editing.is_none());

    // connections only; Cancel leaves nothing behind
    let mut s = dialog_state(vec![], None);
    update(&mut s, Msg::ServerWizard(true));
    update(&mut s, Msg::WizardKobold(false));
    update(&mut s, Msg::AddConnectServer);
    update(&mut s, Msg::FinishServerWizard);
    assert_eq!(s.config.server_presets[0].name, "Running server");
    let mut s = dialog_state(vec![], None);
    update(&mut s, Msg::ServerWizard(true));
    update(&mut s, Msg::WizardKobold(false));
    update(&mut s, Msg::AddConnectServer);
    update(&mut s, Msg::ServerWizard(false));
    assert!(dialog(&s).draft.servers.is_empty() && !dialog(&s).wizard);
    assert!(update(&mut s, Msg::FinishServerWizard).is_empty(), "only from the wizard");

    // KoboldCpp generating images, and prompt enhance's chat server too
    let mut s = dialog_state(vec![], None);
    update(&mut s, Msg::ServerWizard(true));
    update(&mut s, Msg::AddKoboldServer);
    let ServerConfig::Kobold(mut k) = dialog(&s).draft.servers[0].clone() else { panic!("a KoboldCpp entry") };
    assert_eq!(k.base_url(), "http://127.0.0.1:5001");
    k.chat = Some(Default::default());
    update(&mut s, Msg::EditServer(0, ServerConfig::Kobold(k)));
    let fx = update(&mut s, Msg::FinishServerWizard);
    assert!(matches!(fx.last(), Some(Effect::OpenServers { servers, .. }) if servers.len() == 1));
    assert_eq!(s.config.server_presets[0].name, "KoboldCpp");
    let chat = s.pe.server.clone().expect("its chat server in use");
    assert_eq!((chat.name.as_str(), chat.base_url().as_str()), ("kcpp0", "http://127.0.0.1:5001"));
}

#[test]
fn the_shared_koboldcpp_binary_starts_new_entries_and_is_checked() {
    let mut s = dialog_state(vec![], None);
    let fx = update(&mut s, Msg::ServerSettings(true));
    assert!(fx.contains(&Effect::KoboldVersion("koboldcpp".into())), "{fx:?}");
    let fx = update(&mut s, Msg::SetKoboldBinary("/opt/kcpp/koboldcpp".into()));
    assert!(matches!(fx[..], [Effect::SaveConfig(..)]), "not checked on every key: {fx:?}");
    assert_eq!(update(&mut s, Msg::CheckKoboldVersion), [Effect::KoboldVersion("/opt/kcpp/koboldcpp".into())]);
    update(&mut s, Msg::KoboldVersion("/opt/kcpp/koboldcpp".into(), Ok("1.123.1".into())));
    assert!(update(&mut s, Msg::CheckKoboldVersion).is_empty(), "known");
    update(&mut s, Msg::ServerSettings(false));
    update(&mut s, Msg::NewPreset);
    update(&mut s, Msg::AddKoboldServer);
    let ServerConfig::Kobold(k) = &dialog(&s).draft.servers[0] else { panic!() };
    let increart_core::kobold::KoboldRun::Start { binary, .. } = &k.run else { panic!() };
    // made full on closing (Windows' adds the drive)
    assert_eq!(*binary, increart_core::config::full_binary("/opt/kcpp/koboldcpp"));

    // picked: the entry's files, starting by what's there
    let dir = tempfile::tempdir().unwrap();
    let mut k = k.clone();
    k.chat = Some(increart_core::kobold::KoboldChat::default());
    s.config.programs.sd_server.model_dirs = vec![dir.path().to_string_lossy().into()];
    update(&mut s, Msg::EditServer(0, ServerConfig::Kobold(k)));
    let fx = update(&mut s, Msg::Pick(PickKind::Kobold(0, PeFile::Model)));
    assert_eq!(fx, [Effect::Pick { kind: PickKind::Kobold(0, PeFile::Model), start: Some(dir.path().into()) }]);
    update(&mut s, Msg::Picked(PickKind::Kobold(0, PeFile::Model), vec!["/m/chat.gguf".into()]));
    update(&mut s, Msg::Picked(PickKind::Kobold(0, PeFile::Config), vec!["/m/k.kcpps".into()]));
    let ServerConfig::Kobold(k) = &dialog(&s).draft.servers[0] else { panic!() };
    assert_eq!(k.chat.as_ref().unwrap().model, "/m/chat.gguf");
    let increart_core::kobold::KoboldRun::Start { config, .. } = &k.run else { panic!() };
    assert_eq!(config, "/m/k.kcpps");
    let fx = update(&mut s, Msg::Pick(PickKind::Kobold(0, PeFile::Mmproj)));
    assert!(matches!(&fx[..], [Effect::Pick { start: Some(p), .. }] if p == dir.path()), "/m isn't there: {fx:?}");
}

#[test]
fn koboldcpp_named_by_any_entry_is_the_one_used() {
    let mut s = dialog_state(vec![], None);
    assert_eq!(increart_gui_core::kobold_binary(&s), "koboldcpp");
    update(&mut s, Msg::NewPreset);
    update(&mut s, Msg::AddKoboldServer);
    let ServerConfig::Kobold(mut k) = dialog(&s).draft.servers[0].clone() else { panic!() };
    if let increart_core::kobold::KoboldRun::Start { binary, .. } = &mut k.run {
        *binary = "/x/koboldcpp-linux-x64".into();
    }
    s.config.server_presets.push(ServerPreset { name: "k".into(), servers: vec![ServerConfig::Kobold(k)] });
    assert_eq!(increart_gui_core::kobold_binary(&s), "/x/koboldcpp-linux-x64");
}

#[test]
fn kobold_wizard_plans_by_vram_downloads_then_opens() {
    use increart_core::pe::{PeDevice, PeRun};
    use increart_core::tiers::{Gpu, Layout};
    let gpu = |number, vram_gib, integrated| Gpu { number, name: format!("gpu{number}"), vram_gib, integrated };
    let mut s =
        dialog_state(vec![ServerPreset { name: "KoboldCpp Vulkan1".into(), servers: vec![start("x", 1)] }], None);
    s.config.programs.sd_server.model_dirs = vec![];
    update(&mut s, Msg::ServerWizard(true));
    update(&mut s, Msg::WizardGpus(Ok(vec![gpu(0, 94.2, true), gpu(1, 29.9, false), gpu(2, 11.9, false)]), Some(64.0)));
    let mut k = *dialog(&s).kobold.clone().unwrap();
    let ticks: Vec<bool> = k.gpus.as_ref().unwrap().as_ref().unwrap().iter().map(|g| g.on).collect();
    assert_eq!(ticks, [false, true, true], "not the integrated one");
    assert_eq!(update(&mut s, Msg::WizardKobold(true)), [Effect::DetectGpus(GpuRuntime::Vulkan)], "detect again");
    update(&mut s, Msg::WizardGpus(Ok(vec![gpu(0, 94.2, true), gpu(1, 29.9, false), gpu(2, 11.9, false)]), Some(64.0)));
    // another runtime numbers them its own way: listed again
    assert_eq!(update(&mut s, Msg::WizardRuntime(GpuRuntime::Rocm)), [Effect::DetectGpus(GpuRuntime::Rocm)]);
    assert!(dialog(&s).kobold.as_ref().unwrap().gpus.is_none());
    update(&mut s, Msg::WizardGpus(Ok(vec![gpu(0, 31.9, false), gpu(1, 29.9, false)]), Some(64.0)));
    let rocm = dialog(&s).kobold.as_ref().unwrap().plan().unwrap();
    // two 32 GB cards: room for prompt enhance beside the images on both
    assert_eq!(rocm.pe_cards().iter().map(|c| c.gpu).collect::<Vec<_>>(), [0, 1]);
    // unticking one is by hand; the other layout goes back to its default
    let mut w = *dialog(&s).kobold.clone().unwrap();
    w.pe_gpus = vec![1];
    update(&mut s, Msg::EditKoboldWizard(w.clone()));
    let w2 = dialog(&s).kobold.clone().unwrap();
    assert!(w2.pe_by_hand);
    assert_eq!(w2.plan().unwrap().pe_cards().iter().map(|c| c.gpu).collect::<Vec<_>>(), [1]);
    w.layout = Layout::PeOwnCard;
    update(&mut s, Msg::EditKoboldWizard(w));
    let w3 = dialog(&s).kobold.clone().unwrap();
    assert!(!w3.pe_by_hand && w3.plan().is_ok());
    for c in &rocm.servers {
        match c {
            ServerConfig::Kobold(k) => assert!(k.device().starts_with("ROCm"), "{}", k.device()),
            ServerConfig::Chat(c) => assert!(
                matches!(&c.run, PeRun::KoboldCpp { device: PeDevice::Gpu { runtime: GpuRuntime::Rocm, .. }, .. }),
                "{c:?}"
            ),
            other => panic!("{other:?}"),
        }
    }
    update(&mut s, Msg::WizardRuntime(GpuRuntime::Vulkan));
    update(&mut s, Msg::WizardGpus(Ok(vec![gpu(0, 94.2, true), gpu(1, 29.9, false), gpu(2, 11.9, false)]), Some(64.0)));

    // picked
    update(&mut s, Msg::Picked(PickKind::WizardModels, vec!["/picked".into()]));
    update(&mut s, Msg::Picked(PickKind::WizardBinary, vec!["/opt/k/koboldcpp".into()]));
    let picked = dialog(&s).kobold.clone().unwrap();
    assert_eq!((picked.models_dir.as_str(), picked.binary.as_str()), ("/picked", "/opt/k/koboldcpp"));

    // no folder: said, nothing fetched
    k.models_dir = " ".into();
    update(&mut s, Msg::EditKoboldWizard(k.clone()));
    let fx = update(&mut s, Msg::FinishKoboldWizard);
    assert!(fx.is_empty(), "{fx:?}");
    assert!(dialog(&s).kobold.as_ref().unwrap().error.as_ref().unwrap().contains("Choose a folder"));
    // a relative folder made full, then a card too small: said, nothing fetched
    k.models_dir = "models".into();
    k.gpus.as_mut().unwrap().as_mut().unwrap()[2].gpu.vram_gib = 6.0;
    update(&mut s, Msg::EditKoboldWizard(k.clone()));
    assert!(update(&mut s, Msg::FinishKoboldWizard).is_empty());
    let w = dialog(&s).kobold.clone().unwrap();
    assert!(w.error.as_ref().unwrap().contains("8 GB"));
    assert_eq!(std::path::Path::new(&w.models_dir), std::path::absolute("models").unwrap());
    // a full path on this OS ("/m" isn't one on Windows)
    let m = if cfg!(windows) { r"C:\m" } else { "/m" };
    k.models_dir = m.into();

    // a preset chosen counts only while choosing
    k.gpus.as_mut().unwrap().as_mut().unwrap()[2].gpu.vram_gib = 11.9;
    k.layout = Layout::ImagePerCard;
    k.gpus.as_mut().unwrap().as_mut().unwrap()[2].preset = Some(16);
    let presets = |p: increart_core::tiers::Plan| p.cards.iter().map(|c| c.preset).collect::<Vec<_>>();
    assert_eq!(presets(k.plan().unwrap()), [Some(32), Some(12)]);
    k.choose = true;
    let chosen = k.plan().unwrap();
    assert_eq!(presets(chosen.clone()), [Some(32), Some(16)]);
    assert!(chosen.notes.iter().any(|n| matches!(n, increart_core::tiers::Note::Warn(_))), "bigger: warned");
    assert_eq!(k.ram_gib, Some(64.0));

    // prompt enhance by default where there's room beside the images: the
    // 32 GB card keeps the chat model loaded, the 12 GB one would swap
    let pe = |p: increart_core::tiers::Plan| p.pe_cards().iter().map(|c| c.gpu).collect::<Vec<_>>();
    assert_eq!(pe(k.plan().unwrap()), [1]);
    // where asked
    (k.pe_gpus, k.pe_by_hand) = (vec![2], true);
    assert_eq!(pe(k.plan().unwrap()), [2]);
    assert_eq!(
        k.plan_as(Layout::PeOwnCard).unwrap().summary,
        "1 image at a time; prompt enhance always loaded on GPU 2."
    );
    (k.pe_gpus, k.pe_by_hand) = (vec![], false);

    // both making images: Q4_K for the 12 GB card, the chat model beside the 32 GB one's
    k.choose = false;
    update(&mut s, Msg::EditKoboldWizard(k));
    let fx = update(&mut s, Msg::FinishKoboldWizard);
    let [Effect::DownloadModels { dir, files, .. }] = &fx[..] else { panic!("{fx:?}") };
    assert_eq!((dir.as_str(), files.len()), (m, 4));
    assert!(files[0].file.ends_with("qwen_image_2.1-Q4_K.gguf"));
    update(&mut s, Msg::WizardDownload(WizardDownload { file: 1, done: 5, total: Some(10), ..Default::default() }));
    assert_eq!(dialog(&s).kobold.as_ref().unwrap().downloading.as_ref().unwrap().1.file, 1);
    assert!(update(&mut s, Msg::FinishKoboldWizard).is_empty(), "once");

    // cancelled: kept open to try again
    assert_eq!(update(&mut s, Msg::CancelWizardDownload), [Effect::CancelDownload]);
    update(&mut s, Msg::WizardDownloaded(Err("cancelled".into())));
    let k = dialog(&s).kobold.clone().unwrap();
    assert!(k.downloading.is_none() && k.error.unwrap().contains("cancelled"));
    assert!(s.config.recipes.iter().all(|r| !r.name.contains("GGUF Q4_K")));

    // Save, download later: saved, nothing fetched, the files said
    let mut later = s.clone();
    let fx = update(&mut later, Msg::SaveKoboldWizardLater);
    assert!(!fx.iter().any(|f| matches!(f, Effect::DownloadModels { .. })), "{fx:?}");
    assert!(fx.iter().any(|f| matches!(f, Effect::SaveConfig(..))), "{fx:?}");
    assert!(!dialog(&later).wizard && dialog(&later).kobold.is_none() && !dialog(&later).opening);
    assert_eq!(later.config.server_presets.last().unwrap().name, "KoboldCpp Vulkan1 + KoboldCpp Vulkan2");
    let said = later.messages.iter().map(|m| m.text.clone()).collect::<Vec<_>>().join("\n");
    assert!(said.contains("Saved without its models") && said.contains("qwen_image_2.1-Q4_K.gguf"), "{said}");
    assert!(dialog(&later).notice.as_ref().is_some_and(|n| n.contains("Saved without its models")));

    // KoboldCpp not found: Save anyway downloads and saves, doesn't open
    update(&mut s, Msg::KoboldVersion("koboldcpp".into(), Err("koboldcpp not found".into())));
    assert!(update(&mut s, Msg::CheckWizardBinary).is_empty(), "asked already");
    let mut anyway = s.clone();
    update(&mut anyway, Msg::SaveKoboldWizard);
    let fx = update(&mut anyway, Msg::WizardDownloaded(Ok(())));
    assert!(matches!(fx[..], [Effect::SaveConfig(..)]), "{fx:?}");
    assert!(!dialog(&anyway).wizard && dialog(&anyway).kobold.is_none() && !dialog(&anyway).opening);
    let saved = anyway.config.server_presets.last().unwrap().clone();
    assert_eq!(saved.name, "KoboldCpp Vulkan1 + KoboldCpp Vulkan2");
    // found later, in Server settings: its entries follow
    update(&mut anyway, Msg::SetKoboldBinary("/opt/k/koboldcpp".into()));
    let binaries: Vec<String> = anyway
        .config
        .server_presets
        .last()
        .unwrap()
        .servers
        .iter()
        .filter_map(|s| match s {
            ServerConfig::Kobold(k) => match &k.run {
                increart_core::kobold::KoboldRun::Start { binary, .. } => Some(binary.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(binaries, ["/opt/k/koboldcpp", "/opt/k/koboldcpp"]);

    update(&mut s, Msg::FinishKoboldWizard);
    let fx = update(&mut s, Msg::WizardDownloaded(Ok(())));
    assert!(matches!(fx[0], Effect::SaveConfig(..)));
    assert!(matches!(fx.last(), Some(Effect::OpenServers { servers, .. }) if servers.len() == 2));
    assert_eq!(s.config.programs.sd_server.model_dirs, [m]);
    assert_eq!(s.config.programs.koboldcpp, "koboldcpp", "the wizard's binary kept");
    let recipe = s.config.recipes.iter().find(|r| r.name.contains("GGUF Q4_K")).expect("the recipe");
    assert_eq!(recipe.files["vae"], "koboldcpp/qwen_image_2.1_vae_bf16.safetensors");
    let preset = s.config.server_presets.last().unwrap();
    assert_eq!(preset.name, "KoboldCpp Vulkan1 + KoboldCpp Vulkan2", "{:?}", s.config.server_presets);
    let chat = s.pe.server.clone().expect("prompt enhance in the first one");
    assert_eq!(chat.base_url(), "http://127.0.0.1:5001");
}

#[test]
fn queue_without_servers_warns_once_a_session() {
    let mut h = Harness::new();
    h.state.no_server_warned = false;
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let mut f = h.state.generate.clone();
    f.prompt = "a fox".into();
    h.send(Msg::Generate(f));
    assert!(h.send(Msg::QueueGenerate).is_empty(), "warns instead of queueing");
    assert!(h.state.no_server_warning && h.state.no_server_warned);
    h.send(Msg::CloseNoServerWarning);
    h.send(Msg::QueueGenerate);
    h.settle();
    assert!(!h.state.no_server_warning, "only the first time");
    let p = h.state.project_entry().unwrap().clone();
    assert!(h.state.latest_run(&p).is_some(), "queued (waiting for a server)");
}

#[test]
fn server_dialog_from_main_can_be_closed_and_status_shows() {
    let mut s = dialog_state(vec![], None);
    update(&mut s, Msg::AskContinueWithoutServers);
    assert!(matches!(s.phase, Phase::Servers(_)), "asks first");
    assert_eq!(dialog(&s).without.as_deref(), Some(&Without { chat: None }));
    update(&mut s, Msg::WithoutDialog(None));
    assert!(dialog(&s).without.is_none(), "Cancel");
    update(&mut s, Msg::AskContinueWithoutServers);
    update(&mut s, Msg::ContinueWithoutServers);
    assert_eq!(s.phase, Phase::Main);
    update(&mut s, Msg::ServerStatus(true));
    assert!(s.server_status);
    update(&mut s, Msg::OpenServerDialog);
    assert!(!s.server_status, "the dialog replaces the status window");
    assert!(dialog(&s).from_main);
    update(&mut s, Msg::CloseServerDialog);
    assert_eq!(s.phase, Phase::Main);

    // not at start-up, and not while opening
    let mut s = dialog_state(vec![], None);
    update(&mut s, Msg::CloseServerDialog);
    assert!(matches!(s.phase, Phase::Servers(_)), "at start-up there is no main window to go back to");
    update(&mut s, Msg::ContinueWithoutServers);
    update(&mut s, Msg::OpenServerDialog);
    update(&mut s, Msg::NewPreset);
    update(&mut s, Msg::AddStartServer);
    update(&mut s, Msg::OpenDraft);
    assert!(dialog(&s).opening);
    update(&mut s, Msg::CloseServerDialog);
    assert!(matches!(s.phase, Phase::Servers(_)), "Cancel or Continue in background while opening");
}

#[test]
fn editor_and_settings_list_the_devices_once() {
    let mut s = dialog_state(vec![], None);
    s.config.programs.sd_server.binary = "/opt/sd-server".into();
    let fx = update(&mut s, Msg::NewPreset);
    assert!(matches!(&fx[..], [Effect::ListDevices(b)] if b == "/opt/sd-server"));
    update(&mut s, Msg::Devices(Ok(vec!["ROCm0\tGPU".into(), "ROCm1\tGPU".into()])));
    assert!(dialog(&s).draft.servers.is_empty(), "the editor ticks nothing by itself");
    update(&mut s, Msg::DraftDevice("ROCm1".into(), true));
    assert!(
        matches!(&dialog(&s).draft.servers[..], [ServerConfig::Start { device, port: 1234, .. }] if device == "ROCm1")
    );
    update(&mut s, Msg::CancelEdit);
    let fx = update(&mut s, Msg::ServerSettings(true));
    assert!(matches!(&fx[..], [Effect::KoboldVersion(_)]), "listed already: only KoboldCpp checked");
    assert!(dialog(&s).settings);
    update(&mut s, Msg::ServerSettings(false));
    assert!(!dialog(&s).settings);
}

#[test]
fn all_servers_up_goes_straight_to_main() {
    let p = ServerPreset { name: "One".into(), servers: vec![start("gpu0", 1234)] };
    let mut s = dialog_state(vec![p], Some("One"));
    update(&mut s, Msg::OpenPreset(0));
    let a = s.open_attempt;
    update(&mut s, Msg::ServerOpened(a, 0, Ok(())));
    let fx = update(&mut s, Msg::ServersSettled(a));
    assert!(matches!(fx[..], [Effect::StartEngine { .. }]));
    assert_eq!(s.phase, Phase::Main);
    assert!(s.engine_running);
}

trait WithDevice {
    fn with_device(self, d: &str) -> Self;
}

impl WithDevice for ServerConfig {
    fn with_device(mut self, d: &str) -> Self {
        if let ServerConfig::Start { device, .. } = &mut self {
            *device = d.into();
        }
        self
    }
}

#[test]
fn picked_files() {
    let mut h = Harness::new();
    let png = h._tmp.path().join("pick.png");
    image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])).save(&png).unwrap();
    h.send(Msg::Picked(PickKind::IdeaImages, vec![png.clone()]));
    h.settle();
    assert_eq!(h.state.draft.images.len(), 1, "imported into the draft");
    h.send(Msg::Picked(PickKind::IdeaImages, vec![png]));
    h.settle();
    assert_eq!(h.state.draft.images.len(), 1, "same pixels: reused, not duplicated");
    let mut d = h.state.draft.clone();
    d.title = "with image".into();
    h.send(Msg::Draft(d));
    h.send(Msg::AddIdea(AddIdea::Inbox));
    h.settle();
    assert_eq!(h.state.unreviewed_ideas()[0].idea.images.len(), 1);

    let fx = h.send(Msg::Picked(PickKind::ModelDir(0), vec!["/models".into()]));
    assert!(fx.is_empty(), "no config path in tests: nothing saved");
    assert_eq!(h.state.config.programs.sd_server.model_dirs, ["/models"]);
    h.send(Msg::Picked(PickKind::ModelDir(1), vec!["/more".into()]));
    h.send(Msg::Picked(PickKind::ModelDir(2), vec!["/models".into()]));
    assert_eq!(h.state.config.programs.sd_server.model_dirs, ["/models", "/more"], "added once");
    h.send(Msg::Picked(PickKind::ModelDir(0), vec!["/other".into()]));
    assert_eq!(h.state.config.programs.sd_server.model_dirs, ["/other", "/more"], "replaced");
    h.send(Msg::Picked(PickKind::SdServerBinary, vec!["/opt/sd/sd-server".into()]));
    assert_eq!(h.state.config.programs.sd_server.binary, "/opt/sd/sd-server");
    assert!(h.send(Msg::Picked(PickKind::ImportFiles, vec![])).is_empty(), "cancelled");
}

#[test]
fn picked_library_folder_fills_the_dialog() {
    let mut s = AppState::new(Config::default(), None);
    update(&mut s, Msg::Picked(PickKind::LibraryFolder, vec!["/data/my library".into()]));
    assert!(matches!(&s.phase, Phase::Library(d) if d.path == "/data/my library"));
    let fx = update(&mut s, Msg::Pick(PickKind::LibraryFolder));
    assert!(matches!(&fx[..], [Effect::Pick { kind: PickKind::LibraryFolder, .. }]));
}

#[test]
fn startup_offers_recent_libraries() {
    let config = Config { recent_libraries: vec!["/libs/a".into(), "/libs/b".into()], ..Default::default() };
    let mut s = AppState::new(config, None);
    assert!(matches!(&s.phase, Phase::Library(d) if d.path.is_empty() && !d.busy), "nothing opens by itself");
    update(&mut s, Msg::ForgetLibrary(0));
    assert_eq!(s.config.recent_libraries, ["/libs/b"]);
    let fx = update(&mut s, Msg::OpenRecentLibrary(0));
    assert_eq!(fx, [Effect::OpenLibrary { path: "/libs/b".into(), create: false }]);
    assert!(matches!(&s.phase, Phase::Library(d) if d.busy && d.path == "/libs/b"));
    assert!(update(&mut s, Msg::OpenRecentLibrary(0)).is_empty(), "not twice while opening");
    update(&mut s, Msg::LibraryOpened(Err("/libs/b is not an IncreArt library (no library.rsn)".into())));
    assert!(matches!(&s.phase, Phase::Library(d) if !d.busy && d.error.is_some()));
}

#[test]
fn library_wizards_create_or_open_the_chosen_folder() {
    let mut s = AppState::new(Config::default(), None);
    update(&mut s, Msg::LibraryWizard(Some((LibraryWizard::Create, "/home/me/IncreArt".into()))));
    assert!(
        matches!(&s.phase, Phase::Library(d) if d.wizard == Some(LibraryWizard::Create) && d.path == "/home/me/IncreArt")
    );
    update(&mut s, Msg::SetLibraryPath("/work/IncreArt".into()));
    let fx = update(&mut s, Msg::CreateLibrary);
    assert_eq!(fx, [Effect::OpenLibrary { path: "/work/IncreArt".into(), create: true }]);
    assert!(update(&mut s, Msg::LibraryWizard(None)).is_empty());
    assert!(matches!(&s.phase, Phase::Library(d) if d.wizard.is_some()), "not closed while creating");
    update(&mut s, Msg::LibraryOpened(Err("/work/IncreArt is not empty".into())));
    assert!(
        matches!(&s.phase, Phase::Library(d) if d.wizard.is_some() && d.error.is_some()),
        "the error shows in the wizard"
    );

    // switching to the Open wizard (no folder yet) drops the error
    update(&mut s, Msg::LibraryWizard(Some((LibraryWizard::Open, String::new()))));
    assert!(
        matches!(&s.phase, Phase::Library(d) if d.wizard == Some(LibraryWizard::Open) && d.error.is_none() && d.path.is_empty())
    );
    update(&mut s, Msg::SetLibraryPath("/libs/a".into()));
    assert_eq!(update(&mut s, Msg::OpenLibrary), [Effect::OpenLibrary { path: "/libs/a".into(), create: false }]);
    update(&mut s, Msg::LibraryOpened(Err("locked".into())));
    update(&mut s, Msg::LibraryWizard(None));
    assert!(matches!(&s.phase, Phase::Library(d) if d.wizard.is_none() && d.error.is_none()));
}

#[test]
fn back_and_forward_retrace_the_places() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    h.send(Msg::Tab(Tab::Projects));
    h.send(Msg::Tab(Tab::Review));
    h.send(Msg::ShowOutputs(OutputSource::Project(project.clone())));
    h.send(Msg::OutputSource(OutputSource::AllProjects));
    h.send(Msg::ProjectSearch("fox".into()));
    assert!(h.state.nav.forward.is_empty());

    h.send(Msg::NavBack);
    assert_eq!((h.state.tab, &h.state.output_source), (Tab::OutputLibrary, &OutputSource::Project(project.clone())));
    h.send(Msg::NavBack);
    assert_eq!(h.state.tab, Tab::Review, "Show outputs was one step");
    h.send(Msg::NavBack);
    assert_eq!((h.state.tab, h.state.project.as_ref()), (Tab::Projects, Some(&project)));
    h.send(Msg::NavForward);
    h.send(Msg::NavForward);
    assert_eq!((h.state.tab, h.state.nav.forward.len()), (Tab::OutputLibrary, 1));
    h.send(Msg::Tab(Tab::IdeaInbox));
    assert!(h.state.nav.forward.is_empty(), "a new step drops the forward ones");
    h.send(Msg::NavBack);
    assert_eq!(h.state.tab, Tab::OutputLibrary);

    // subtabs are steps too, Review's view among them
    h.send(Msg::Tab(Tab::IdeaInbox));
    h.send(Msg::InboxTab(InboxTab::Grid));
    h.send(Msg::Tab(Tab::Review));
    h.send(Msg::ReviewView(ReviewView::Grid));
    h.send(Msg::NavBack);
    assert_eq!((h.state.tab, h.state.review_view), (Tab::Review, ReviewView::Single));
    h.send(Msg::NavBack);
    assert_eq!((h.state.tab, h.state.inbox_tab), (Tab::IdeaInbox, InboxTab::Grid));
    h.send(Msg::NavBack);
    assert_eq!(h.state.inbox_tab, InboxTab::NewIdea);
    h.send(Msg::NavForward);
    h.send(Msg::NavForward);
    h.send(Msg::NavForward);
    assert_eq!((h.state.tab, h.state.review_view), (Tab::Review, ReviewView::Grid));
}

#[test]
fn show_outputs_jumps_to_the_grid_and_flags_the_notice() {
    let mut s = AppState::new(Config::default(), None);
    s.tab = Tab::Projects;
    s.output_tab = OutputTab::Folder;
    s.output_preset = OutputPreset::Rejected;
    s.output_filter = OutputPreset::Rejected.filter();
    s.output_filter.text = "fox".into();
    update(&mut s, Msg::ShowOutputs(OutputSource::ProjectFolder("clients".into())));
    assert_eq!((s.tab, s.output_tab), (Tab::OutputLibrary, OutputTab::Grid));
    assert_eq!((s.output_preset, s.output_filter.ratings), (OutputPreset::Kept, Ratings::KEPT), "back on Kept");
    assert_eq!(s.output_source, OutputSource::ProjectFolder("clients".into()));
    assert!(s.output_notice);
    assert!(s.output_reveal, "the sources panel reveals it");
    assert_eq!(s.output_reveals, 1, "and starts the tree's open folders over");
    update(&mut s, Msg::OutputRevealed);
    assert!(!s.output_reveal);
    assert_eq!(s.output_filter.text, "fox", "filters are kept; the Grid offers a reset");
    // a project-name search that would hide the source goes; one that shows it stays
    s.output_search_projects = true;
    s.output_project_search = "LIEN".into();
    update(&mut s, Msg::ShowOutputs(OutputSource::ProjectFolder("clients".into())));
    assert_eq!(s.output_project_search, "LIEN");
    s.output_project_search = "fox".into();
    update(&mut s, Msg::ShowOutputs(OutputSource::ProjectFolder("clients".into())));
    assert_eq!(s.output_project_search, "");
    s.output_search_projects = false;
    update(&mut s, Msg::ResetOutputFilters);
    assert_eq!(s.output_filter, OutputFilter::default());
    update(&mut s, Msg::OutputSource(OutputSource::AllProjects));
    assert!(!s.output_notice, "picking another source drops the notice");
    update(&mut s, Msg::ShowOutputs(OutputSource::AllProjects));
    update(&mut s, Msg::DismissOutputNotice);
    assert!(!s.output_notice);
}

#[test]
fn job_progress() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let mut f = h.state.generate.clone();
    f.prompt = "a fox".into();
    f.seeds = 4;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    h.settle();
    let run = h.state.project_entry().unwrap().runs[0].id.clone();
    assert_eq!(h.state.job_summary(), JobSummary::default(), "not in the engine (no servers)");
    // as the engine would report it
    h.state.active_runs.insert(run.clone());
    h.send(Msg::Engine(increart_core::engine::Event::Progress {
        run: run.clone(),
        rev: 0,
        seed: 0,
        server: 0,
        step: 10,
        total: 20,
        preview: None,
    }));
    let p = h.state.run_progress(&run).unwrap();
    assert_eq!((p.done, p.total, p.sampling.clone()), (0, 4, vec![(0, 10, 20)]));
    assert!((p.fraction() - 0.125).abs() < 1e-6);
    let s = h.state.job_summary();
    assert_eq!((s.runs, s.done, s.total), (1, 0, 4));
    assert!((s.fraction - 0.125).abs() < 1e-6);

    // an image done: its server's speed this session; one unreachable
    // isn't working
    let server = |name: &str, state: &str| ServerStatus {
        config: None,
        name: name.into(),
        device: String::new(),
        state: state.into(),
        model: None,
        idle_since: None,
        worked: false,
    };
    h.state.servers = vec![server("gpu0", "ready"), server("gpu1", "unreachable, retrying")];
    assert_eq!(h.state.working_servers(), 1);
    h.send(Msg::Engine(increart_core::engine::Event::Output {
        run: run.clone(),
        rev: 0,
        seed: 1,
        image: increart_core::Id::new(increart_core::id::now(), 9),
        link: PathBuf::from("r01_s1.png"),
        server: 0,
        elapsed_s: 40.0,
        step_s: 0.0,
    }));
    let speed = h.state.server_speed("gpu0").unwrap();
    assert_eq!(speed.text(), "≈2.00 s/it · 40.0 s/image · 90/h", "estimated: the run has 20 steps");
    assert!(h.state.server_speed("gpu1").is_none());
    assert_eq!(h.state.job_summary().per_hour, 90.0);
}

#[test]
fn the_forecast_times_the_queue() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let mut f = h.state.generate.clone();
    f.prompt = "a fox".into();
    f.seeds = 4;
    f.steps = 20;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    h.settle();
    let run = h.state.project_entry().unwrap().runs[0].id.clone();
    h.state.active_runs.insert(run.clone());
    let recipe = h.state.index.run(&run).unwrap().1.model.recipe.clone();
    h.state.servers = vec![ServerStatus {
        config: None,
        name: "gpu0".into(),
        device: "ROCm0".into(),
        state: "ready".into(),
        model: Some(recipe),
        idle_since: None,
        worked: false,
    }];
    let progress = |step| {
        Msg::Engine(increart_core::engine::Event::Progress {
            run: run.clone(),
            rev: 0,
            seed: 0,
            server: 0,
            step,
            total: 20,
            preview: None,
        })
    };
    // nothing timed yet: the rough estimate (a minute or so an image)
    h.send(Msg::Tick(100.0));
    h.send(progress(10));
    h.send(Msg::Tick(101.0));
    assert!(h.state.run_left(&run).is_some_and(|s| s > 100.0), "{:?}", h.state.run_left(&run));
    // its live speed, at once: 1 s a step (20 s an image), seed 0 at 15
    // of 20: 5 s for it, then 3 more at 20 s on the one server
    h.send(Msg::Tick(105.0));
    h.send(progress(15));
    h.send(Msg::Tick(106.0));
    assert_eq!(h.state.run_left(&run), Some(65.0));
    assert_eq!(h.state.job_summary().eta_s, Some(65.0));
    // it counts down while nothing changes
    h.send(Msg::Tick(116.0));
    assert_eq!(h.state.run_left(&run), Some(55.0));
    let jobs = h.state.jobs();
    assert_eq!(h.state.group_progress(&jobs.iter().collect::<Vec<_>>()).3, Some(55.0));

    // the Generate form's: 2 more seeds alone, and with what's queued
    // (spread: sharing the server with it, not behind it)
    let shape = increart_gui_core::forecast::Shape::of_form(&h.state.generate, &h.state.config.recipes);
    let per = h.state.image_s_for(&shape).0;
    let e = h.state.estimate_new(&[(shape, 2)], false).unwrap();
    assert_eq!((e.servers, e.alone_s), (1, 2.0 * per));
    let with = e.with_queue_s.unwrap();
    assert!(with > e.alone_s && with < 65.0 + 2.0 * per, "{with}");
    // stopped: none
    h.state.active_runs.clear();
    h.send(Msg::Tick(117.0));
    assert_eq!(h.state.run_left(&run), None);
}

#[test]
fn compare_with_an_image_from_the_references_or_libraries() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    let lib = Library_::steal(&h.root);
    let r01 = lib.write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let first = h.state.review_queue().iter().find(|o| o.name.run == r01).unwrap().image.clone();
    let r02 = lib.write_two_with(&project, "r02", vec![first.clone()]);
    h.rt.run(Effect::Rescan);
    h.settle();

    // Review, on an image of r02: the picker opens on its references
    h.send(Msg::Tab(Tab::Review));
    let b = h.state.review_queue().iter().find(|o| o.name.run == r02).unwrap().image.clone();
    h.send(Msg::CompareWith(Some(b.clone())));
    let p = h.state.ref_picker.as_ref().unwrap();
    assert_eq!((&p.source, &p.target), (&RefSource::ImageInputs(b.clone()), &PickTarget::Compare));
    let found: Vec<_> = h.state.ref_candidates().into_iter().map(|c| c.image).collect();
    assert_eq!(found, std::slice::from_ref(&first));
    assert_eq!(h.state.input_role(&b, &first), Some(Some(1)), "B's reference 1");
    // one click: A, B stays, and the source isn't remembered for Generate
    h.send(Msg::RefPickerSource(RefSource::ProjectInputs(project.clone())));
    h.send(Msg::RefPickerToggle(first.clone()));
    assert!(h.state.ref_picker.is_none());
    assert_eq!((h.state.review_view, h.state.review_pinned.as_ref()), (ReviewView::Compare, Some(&first)));
    assert_eq!(h.state.compare_b(), Some(b));
    assert!(h.state.ref_picker_source.is_none());

    // the Output library: on an image made from text only, its project's references
    h.send(Msg::Tab(Tab::OutputLibrary));
    h.send(Msg::OutputSource(OutputSource::AllProjects));
    let second = h.state.index.projects[0].outputs.iter().find(|o| o.name.run == r01 && o.image != first).unwrap();
    let second = second.image.clone();
    h.send(Msg::CompareWith(Some(second.clone())));
    assert_eq!(h.state.output_selected.as_ref(), Some(&second));
    assert_eq!(h.state.ref_picker.as_ref().unwrap().source, RefSource::ProjectInputs(project.clone()));
    h.send(Msg::RefPickerToggle(first.clone()));
    assert_eq!(h.state.output_compare.as_ref(), Some(&first));
    // swap: the sides change places, A stays A
    h.send(Msg::SwapCompare);
    assert!(h.state.compare_flipped);
    assert_eq!((h.state.output_compare.as_ref(), h.state.output_selected.as_ref()), (Some(&first), Some(&second)));
    h.send(Msg::OutputCompare(None));
    assert_eq!(h.state.output_compare, None);
}

#[test]
fn review_views_compare_and_rejected() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let (first, second) = {
        let q = h.state.review_queue();
        (q[0].image.clone(), q[1].image.clone())
    };

    h.send(Msg::ToggleReviewGrid);
    assert_eq!(h.state.review_view, ReviewView::Grid);
    h.send(Msg::ToggleReviewGrid);
    assert_eq!(h.state.review_view, ReviewView::Single);

    // C: A = first, B = second
    h.send(Msg::Compare);
    assert_eq!(h.state.review_view, ReviewView::Compare);
    assert_eq!(h.state.review_pinned.as_ref(), Some(&first));
    assert_eq!(h.state.review_queue()[h.state.review_cursor].image, second);
    // swap: the sides change places, A stays pinned
    h.send(Msg::SwapCompare);
    assert!(h.state.compare_flipped);
    assert_eq!(h.state.review_pinned.as_ref(), Some(&first));
    assert_eq!(h.state.review_queue()[h.state.review_cursor].image, second);
    h.send(Msg::SwapCompare);
    assert!(!h.state.compare_flipped);
    // rate B (keys) and A (its own buttons)
    h.send(Msg::ReviewCurrent(Verdict::Bad));
    h.settle();
    h.send(Msg::ReviewPinned(Verdict::Good));
    h.settle();
    assert!(h.state.review_queue().is_empty());
    assert_eq!(h.state.output_link(&first).unwrap().verdict, Some(Verdict::Good), "A still shown after rating");
    h.send(Msg::Compare);
    assert_eq!((h.state.review_view, h.state.review_pinned.clone()), (ReviewView::Single, None));

    // output library: the preset tabs, and deleting the shown rejected
    h.send(Msg::OutputSource(OutputSource::AllProjects));
    h.send(Msg::OutputPreset(OutputPreset::Good));
    assert_eq!(h.state.output_source_images().len(), 1);
    h.send(Msg::OutputPreset(OutputPreset::All));
    assert_eq!(h.state.output_source_images().len(), 2);
    h.send(Msg::OutputPreset(OutputPreset::Unreviewed));
    assert!(h.state.output_source_images().is_empty());
    h.send(Msg::OutputPreset(OutputPreset::Rejected));
    assert_eq!(h.state.output_source_images().len(), 1);
    let mut f = h.state.output_filter.clone();
    f.ratings.good = true;
    h.send(Msg::OutputFilter(f));
    assert!(h.state.output_preset_modified(), "the tab shows a dot");
    h.send(Msg::OutputPreset(OutputPreset::Rejected));
    assert!(!h.state.output_preset_modified(), "clicking it again resets");
    h.send(Msg::OutputSource(OutputSource::ProjectFolder("no such folder".into())));
    assert!(h.state.output_source_images().is_empty(), "Rejected follows the source");
    h.send(Msg::OutputSource(OutputSource::AllProjects));
    h.send(Msg::Ask(Confirm::new("Delete these rejected?", "Delete", Msg::DeleteShownRejected)));
    assert!(h.state.confirm.is_some());
    h.send(Msg::ConfirmYes);
    h.settle();
    assert!(h.state.confirm.is_none());
    assert!(h.state.output_source_images().is_empty(), "{:?}", h.errors());
    assert!(first.path(&h.root).is_file(), "the image itself stays until Clean up");

    // the deleted image is in Discarded, rated as it was; restored, it's back
    let discarded =
        |h: &Harness| h.state.discarded_outputs().iter().map(|o| (o.image.clone(), o.verdict)).collect::<Vec<_>>();
    assert_eq!(discarded(&h), vec![(second.clone(), Some(Verdict::Bad))]);
    h.send(Msg::OutputTab(OutputTab::Discarded));
    let path = h.state.discarded_outputs()[0].path.clone();
    h.send(Msg::RestoreOutput(path));
    h.settle();
    assert!(discarded(&h).is_empty(), "{:?}", h.errors());
    assert_eq!(h.state.output_source_images().len(), 1);
    assert_eq!(h.state.output_link(&second).unwrap().verdict, Some(Verdict::Bad));
    // discarded again, then emptied: gone for good
    let link = h.state.output_link(&second).unwrap().path.clone();
    h.send(Msg::DeleteOutput(link));
    h.settle();
    assert_eq!(discarded(&h).len(), 1);
    h.send(Msg::Ask(Confirm::new("Empty discarded?", "Delete", Msg::EmptyDiscardedOutputs)));
    h.send(Msg::ConfirmYes);
    h.settle();
    assert!(discarded(&h).is_empty(), "{:?}", h.errors());
    assert!(h.state.output_link(&second).is_none());
}

#[test]
fn drafts_autosave_and_restore() {
    let mut h = Harness::new();
    let draft_file = |h: &Harness, p: &str| h.root.join("projects").join(p).join("draft.rsn");
    // two projects
    for _ in 0..2 {
        h.send(Msg::NewProject(PathBuf::new()));
        h.send(Msg::CommitRename);
        h.settle();
    }
    let a = h.state.index.projects.iter().find(|p| p.name() == "new project").unwrap().project.id.clone();
    let b = h.state.index.projects.iter().find(|p| p.name() == "new project 2").unwrap().project.id.clone();
    h.send(Msg::SelectProject(a.clone()));

    // an edit is saved only after 2 s without edits
    h.send(Msg::Tick(10.0));
    let mut f = h.state.generate.clone();
    f.prompt = "a fox in a café".into();
    h.send(Msg::Generate(f.clone()));
    assert!(h.send(Msg::Tick(11.0)).is_empty(), "too soon");
    f.prompt.push_str(", film photo");
    h.send(Msg::Generate(f.clone()));
    assert!(h.send(Msg::Tick(12.5)).is_empty(), "the clock restarts at each edit");
    let fx = h.send(Msg::Tick(13.1));
    assert!(
        matches!(&fx[0], Effect::SaveDraft(DraftSave::Generate(p, d)) if p == &a && d.prompt.ends_with("film photo"))
    );
    h.settle();
    assert!(draft_file(&h, "new project").is_file());
    assert!(h.state.draft_saved.is_some());
    assert!(h.send(Msg::Tick(20.0)).is_empty(), "nothing new to save");

    // switching projects flushes at once, and each project has its own form
    let mut g = h.state.generate.clone();
    g.prompt = "unsaved yet".into();
    h.send(Msg::Generate(g));
    let fx = h.send(Msg::SelectProject(b.clone()));
    assert!(matches!(&fx[0], Effect::SaveDraft(DraftSave::Generate(p, d)) if p == &a && d.prompt == "unsaved yet"));
    h.settle();
    assert_eq!(h.state.generate.prompt, "", "project b starts fresh");
    h.send(Msg::SelectProject(a.clone()));
    assert_eq!(h.state.generate.prompt, "unsaved yet");

    // queueing keeps the draft, with the next label and seed
    let mut g = h.state.generate.clone();
    g.seeds = 3;
    h.send(Msg::Generate(g));
    h.send(Msg::QueueGenerate);
    h.settle();
    h.send(Msg::FlushDrafts);
    h.settle();
    let saved = h.state.index.project(&a).unwrap().draft.clone().unwrap();
    assert_eq!((saved.label.as_str(), saved.first_seed, saved.prompt.as_str()), ("r01", 3, "unsaved yet"));

    // Clear resets the form
    h.send(Msg::ClearGenerate);
    assert_eq!(
        (h.state.generate.prompt.as_str(), h.state.generate.label.as_str(), h.state.generate.first_seed),
        ("", "r01", 3)
    );

    // the New idea draft: saved, restored when the library is opened again, removed when added
    h.send(Msg::Draft(IdeaDraft { title: "Otter".into(), text: "otter with a teacup".into(), ..Default::default() }));
    h.send(Msg::FlushDrafts);
    h.settle();
    assert!(h.root.join("input/draft.rsn").is_file());
    let ix = h.state.index.clone();
    let mut fresh = AppState::new(h.state.config.clone(), None);
    let mut ix2 = ix;
    ix2.idea_draft = Some(increart_core::model::IdeaDraftFile {
        title: "Otter".into(),
        text: "otter with a teacup".into(),
        ..Default::default()
    });
    update(&mut fresh, Msg::LibraryOpened(Ok((h.root.clone(), ix2))));
    assert_eq!(fresh.draft.title, "Otter", "restored on open");
    h.send(Msg::AddIdea(AddIdea::Inbox));
    h.settle();
    assert!(!h.root.join("input/draft.rsn").exists(), "removed once the idea is added");
}

#[test]
fn outputs_as_inputs() {
    use increart_core::model::{IdeaSource, Mode};
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::EditRename("fox".into()));
    h.send(Msg::CommitRename);
    h.settle();
    h.settle();
    let fox = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&fox);
    h.rt.run(Effect::Rescan);
    h.settle();
    let (first, second) = {
        let q = h.state.review_queue();
        (q[0].image.clone(), q[1].image.clone())
    };

    // E: a fresh edit in the image's own project, the image as <image1>
    h.send(Msg::Tab(Tab::Review));
    h.send(Msg::UseImage(first.clone(), UseTarget::Edit));
    assert_eq!((h.state.tab, h.state.projects_tab), (Tab::Projects, ProjectsTab::Generate));
    assert_eq!(h.state.project.as_ref(), Some(&fox));
    assert_eq!(h.state.generate.refs(), std::slice::from_ref(&first));
    assert_eq!(h.state.generate.prompt, "<image1> ");
    assert!(h.state.generate.init().is_none());

    // another output into the current form: the next reference; made the
    // img2img start image by hand in the Images pane
    h.send(Msg::UseImage(second.clone(), UseTarget::CurrentProject));
    assert!(h.state.messages.last().unwrap().text.ends_with("as <image2>"));
    h.send(Msg::ImageRoles { index: 1, reference: true, start: true });
    let mut f = h.state.generate.clone();
    f.prompt = "make it night".into();
    f.strength = 0.6;
    f.seeds = 1;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    // neither reference is named: asked first
    assert!(h.state.confirm.as_ref().is_some_and(|c| c.text.starts_with("<image1> and <image2> aren't named")));
    h.send(Msg::ConfirmYes);
    h.settle();
    let run = h.state.project_entry().unwrap().runs.iter().find(|r| r.prompt == "make it night").unwrap().clone();
    assert_eq!(run.mode, Mode::Edit { init: Some(second.clone()), strength: 0.6 });
    assert_eq!(run.refs, [first.clone(), second.clone()]);

    // N: a new project next to it, references (the default), prompt prefilled;
    // its name is asked first (Cancel makes nothing)
    h.send(Msg::UseImage(first.clone(), UseTarget::NewProject));
    assert_eq!(h.state.new_project.as_ref().map(|d| d.name.as_str()), Some("fox from s0000"));
    h.send(Msg::CancelNewProject);
    h.settle();
    assert_eq!(h.state.project.as_ref(), Some(&fox), "nothing made");
    h.send(Msg::UseImage(first.clone(), UseTarget::NewProject));
    h.send(Msg::ConfirmNewProject);
    h.settle();
    let p = h.state.project_entry().unwrap();
    assert_eq!(p.name(), "fox from s0000");
    assert_eq!(h.state.generate.refs(), std::slice::from_ref(&first));
    assert_eq!(h.state.generate.prompt, "x", "the image's prompt");
    assert!(h.state.renaming.is_none(), "named in the dialog");

    // D: a copy of the image's project, form from the image's run (which had
    // no images) with this one as <image1>, under the name typed (an invalid one is refused, the dialog stays)
    h.send(Msg::UseImage(first.clone(), UseTarget::DuplicateProject));
    h.send(Msg::EditNewProjectName("a/b".into()));
    h.send(Msg::ConfirmNewProject);
    assert!(h.state.new_project.is_some() && h.errors().iter().any(|e| e.contains("a/b")), "{:?}", h.errors());
    h.state.messages.clear();
    h.send(Msg::EditNewProjectName("fox night edits".into()));
    h.send(Msg::ConfirmNewProject);
    h.settle();
    let p = h.state.project_entry().unwrap();
    assert_eq!(p.name(), "fox night edits");
    assert_eq!(p.runs.len(), 2, "run settings copied as templates");
    assert!(p.outputs.is_empty(), "no images copied");
    assert_eq!(h.state.generate.refs(), std::slice::from_ref(&first));
    assert!(h.state.generate.init().is_none());
    // past the copied runs' r01 and r02
    assert_eq!((h.state.generate.label.as_str(), h.state.generate.first_seed), ("r03", 0));
    h.send(Msg::CancelRename);

    // tree: plain Duplicate
    h.send(Msg::DuplicateProject("fox".into()));
    assert_eq!(h.state.duplicate.as_ref().unwrap().name, "fox copy", "asks for a name first");
    h.send(Msg::EditDuplicateName("FOX".into()));
    h.send(Msg::ConfirmDuplicate);
    assert!(h.state.duplicate.is_some() && !h.errors().is_empty(), "a taken name keeps the dialog open");
    h.state.messages.clear();
    h.send(Msg::CancelDuplicate);
    assert!(h.state.duplicate.is_none());
    h.send(Msg::DuplicateProject("fox".into()));
    h.send(Msg::EditDuplicateName("fox 2".into()));
    h.send(Msg::ConfirmDuplicate);
    h.settle();
    assert_eq!(h.state.project_entry().unwrap().name(), "fox 2", "the copy is selected");
    assert!(h.state.renaming.is_none() && h.state.duplicate.is_none());

    // R and I
    h.send(Msg::ToInputLibrary(first.clone()));
    h.send(Msg::NewIdeaFromImage(second.clone()));
    h.settle();
    let kept = h.state.library_ideas();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].idea.source, IdeaSource::FromOutput { image: first.clone() });
    assert_eq!(kept[0].idea.text, "");
    let inbox = h.state.unreviewed_ideas();
    assert_eq!((inbox.len(), inbox[0].idea.text.as_str()), (1, "x"));
    assert!(h.errors().is_empty(), "{:?}", h.errors());
}

/// Regression: starting the engine with a run that has pending seeds used to
/// take the library lock twice (nested) and hang the runtime, and with it the
/// GUI thread.
#[test]
fn start_engine_with_pending_runs_does_not_hang() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let mut f = h.state.generate.clone();
    f.prompt = "a fox".into();
    f.seeds = 2;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    h.settle();
    let run = h.state.project_entry().unwrap().runs[0].id.clone();
    h.rt.run(Effect::StartEngine {
        shared: increart_core::config::SdServerConfig::default(),
        order: Default::default(),
        skip_ram_check: false,
    });
    let msgs = h.rt.wait(Duration::from_secs(10));
    assert!(msgs.contains(&Msg::RunsQueued(vec![run])), "{msgs:?}");
    // and the runtime still answers
    h.rt.run(Effect::Rescan);
    assert!(!h.rt.wait(Duration::from_secs(10)).is_empty());
}

#[test]
fn continue_in_background_and_cancel() {
    let p = ServerPreset { name: "Both".into(), servers: vec![start("gpu0", 1234), start("gpu1", 1235)] };
    let mut s = dialog_state(vec![p], Some("Both"));
    update(&mut s, Msg::OpenPreset(0));
    let first = s.open_attempt;
    // Cancel: stops what was started; late reports of that attempt are ignored
    let fx = update(&mut s, Msg::CancelOpening);
    assert_eq!(fx, [Effect::StopServers]);
    assert!(!dialog(&s).opening && s.servers.is_empty());
    update(&mut s, Msg::ServerOpened(first, 0, Ok(())));
    assert!(update(&mut s, Msg::ServersSettled(first)).is_empty());
    assert!(matches!(s.phase, Phase::Servers(_)), "a stale attempt changes nothing");

    // Again, then carry on in the main window while they open
    update(&mut s, Msg::OpenPreset(0));
    let a = s.open_attempt;
    assert_ne!(a, first);
    update(&mut s, Msg::ContinueInBackground);
    assert_eq!(s.phase, Phase::Main);
    assert!(s.servers.iter().all(|x| x.state == "starting…"));
    update(&mut s, Msg::ServerOpened(a, 1, Err("ROCm1: out of memory".into())));
    assert_eq!(s.servers[1].state, "failed");
    assert_eq!(s.messages.last().unwrap().text, "ROCm1: out of memory");
    update(&mut s, Msg::ServerOpened(a, 0, Ok(())));
    let fx = update(&mut s, Msg::ServersSettled(a));
    assert!(matches!(fx[..], [Effect::StartEngine { .. }]), "starts with the one that came up");
    assert_eq!(s.servers.len(), 1);
    assert!(s.engine_running && s.background.is_none());
}

#[test]
fn generate_more_seeds() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let mut f = h.state.generate.clone();
    f.prompt = "a fox".into();
    f.seeds = 4;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    h.settle();
    let p = h.state.project_entry().unwrap().clone();
    let run = h.state.latest_run(&p).unwrap().id.clone();
    h.send(Msg::MoreSeeds(run.clone(), 8));
    h.settle();
    let r = h.state.index.run(&run).unwrap().1.clone();
    assert_eq!((r.seed_count(), r.next_seed()), (12, 12), "continues after seed 3");
    assert_eq!(r.revisions.len(), 1, "same revision");
    assert!(h.state.messages.last().unwrap().text.contains("+8 seeds"));
    // with servers it is queued right away
    h.state.engine_running = true;
    let fx = h.send(Msg::MoreSeeds(run.clone(), 4));
    assert!(matches!(&fx[..], [Effect::Lib(op)] if matches!(**op, LibOp::AddSeeds(_, 4))));
    h.settle();
    assert!(h.state.active_runs.contains(&run));
    h.send(Msg::Tab(Tab::Review));
    h.send(Msg::OpenInGenerate(p.project.id.clone()));
    assert_eq!((h.state.tab, h.state.projects_tab), (Tab::Projects, ProjectsTab::Generate));
}

#[test]
fn form_from_latest_run_and_refs_from_files() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let p = h.state.project.clone().unwrap();
    // a reference from a file (file picker / drop on the Generate panel)
    let png = h._tmp.path().join("ref.png");
    image::RgbImage::from_pixel(32, 32, image::Rgb([9, 9, 9])).save(&png).unwrap();
    h.send(Msg::Picked(PickKind::GenerateRefs, vec![png.clone()]));
    h.settle();
    assert_eq!(h.state.generate.refs().len(), 1);
    assert_eq!(h.state.generate.refs()[0].store, increart_core::Store::Input);
    h.send(Msg::DropRefs(vec![png]));
    h.settle();
    assert_eq!(h.state.generate.refs().len(), 1, "same pixels: same image, not added twice");

    let mut f = h.state.generate.clone();
    f.prompt = "the robot from <image1>".into();
    f.width = 1152;
    f.height = 512;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    h.settle();
    // a project whose draft is gone (runs made before drafts existed)
    std::fs::remove_file(h.root.join("projects/new project/draft.rsn")).ok();
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::SelectProjectFolder(None));
    h.send(Msg::SelectProject(p));
    let g = &h.state.generate;
    assert_eq!((g.prompt.as_str(), g.refs().len(), g.width, g.height), ("the robot from <image1>", 1, 1152, 512));
    assert_eq!((g.label.as_str(), g.first_seed), ("r01", 4), "from the latest run, label and seed moved on");
}

fn history(h: &mut Harness, key: increart_core::HistoryKey) -> Vec<increart_core::history::HistoryEntry> {
    h.send(Msg::OpenHistory(key));
    h.settle();
    let v = h.state.history_view.clone().unwrap();
    h.send(Msg::CloseHistory);
    v.entries
}

#[test]
fn prompt_history_groups_and_restores() {
    use increart_core::HistoryKey;
    use increart_core::history::HistoryKind;
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let p = h.state.project.clone().unwrap();
    let key = HistoryKey::Project(p.clone());
    let type_at = |h: &mut Harness, t: f64, text: &str| {
        h.send(Msg::Tick(t));
        let mut f = h.state.generate.clone();
        f.prompt = text.into();
        h.send(Msg::Generate(f));
        h.send(Msg::Tick(t + 2.5));
        h.settle();
    };
    type_at(&mut h, 100.0, "a red fox reading a newspaper");
    type_at(&mut h, 110.0, "a red fox reading the newspaper"); // small, soon: merged
    type_at(&mut h, 200.0, "a red fox reading the newspapers"); // small but 90 s later: new
    type_at(&mut h, 205.0, "a red fox reading the newspapers in a sunlit café, film photograph"); // big: new
    let e = history(&mut h, key.clone());
    let texts: Vec<_> = e.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "a red fox reading the newspapers in a sunlit café, film photograph",
            "a red fox reading the newspapers",
            "a red fox reading the newspaper",
        ],
        "newest first; the first two edits merged"
    );

    // checkpoints: queued, before clear
    h.send(Msg::QueueGenerate);
    h.settle();
    h.send(Msg::ClearGenerate);
    h.settle();
    let e = history(&mut h, key.clone());
    assert_eq!(e[0].kind, HistoryKind::BeforeClear);
    assert_eq!(e[1].kind, HistoryKind::Queued("r00".into()));

    // restore an older entry: the current text is kept first
    let mut f = h.state.generate.clone();
    f.prompt = "something new".into();
    h.send(Msg::Generate(f));
    h.send(Msg::OpenHistory(key.clone()));
    h.settle();
    let i = h
        .state
        .history_view
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .position(|e| e.text == "a red fox reading the newspapers")
        .unwrap();
    h.send(Msg::HistorySelect(i));
    h.send(Msg::HistoryRestore(increart_gui_core::Restore::Text));
    h.settle();
    assert_eq!(h.state.generate.prompt, "a red fox reading the newspapers");
    let e = history(&mut h, key);
    assert_eq!((e[0].kind.clone(), e[0].text.as_str()), (HistoryKind::BeforeRestore, "something new"));

    // search and filter
    h.send(Msg::OpenHistory(HistoryKey::Project(p)));
    h.settle();
    h.send(Msg::HistorySearch("café".into()));
    assert!(h.state.history_view.as_ref().unwrap().shown().iter().all(|r| r.entry.text.contains("café")));
    h.send(Msg::HistorySearch(String::new()));
    h.send(Msg::HistoryCheckpointsOnly(true));
    assert!(h.state.history_view.as_ref().unwrap().shown().iter().all(|r| r.entry.kind.is_checkpoint()));
}

#[test]
fn edit_idea_window_and_its_history() {
    use increart_core::HistoryKey;
    let mut h = Harness::new();
    h.send(Msg::Draft(IdeaDraft { title: "Fox".into(), text: "a fox".into(), ..Default::default() }));
    h.send(Msg::AddIdea(AddIdea::Inbox));
    h.settle();
    let new_idea_hist = history(&mut h, HistoryKey::NewIdea);
    assert_eq!(new_idea_hist[0].text, "a fox", "the added idea is a checkpoint of the New idea form");
    let id = h.state.unreviewed_ideas()[0].idea.id.clone();
    h.send(Msg::OpenIdea(id.clone()));
    let mut ed = h.state.idea_edit.clone().unwrap();
    ed.text = "a red fox in a café, film photograph".into();
    ed.tags = "animals, photo".into();
    h.send(Msg::Tick(50.0));
    h.send(Msg::EditIdea(ed));
    h.send(Msg::Tick(53.0));
    h.send(Msg::SaveIdea);
    h.settle();
    let i = &h.state.idea(&id).unwrap().idea;
    assert_eq!((i.text.as_str(), i.tags.len()), ("a red fox in a café, film photograph", 2));
    assert!(h.state.idea_edit.is_none());
    let e = history(&mut h, HistoryKey::Idea(id));
    assert_eq!(e[0].text, "a red fox in a café, film photograph");
}

#[test]
fn library_filters_and_output_sources() {
    let mut h = Harness::new();
    for (t, x, star) in [("Fox", "a fox", true), ("Hut", "alpine hut", false)] {
        h.send(Msg::Draft(IdeaDraft { title: t.into(), text: x.into(), ..Default::default() }));
        h.send(Msg::AddIdea(AddIdea::Keep));
        h.send(Msg::ConfirmSaveIdea);
        h.settle();
        if star {
            let id = h.state.library_ideas().iter().find(|e| e.idea.title == t).unwrap().idea.id.clone();
            h.send(Msg::SetStar(id, true));
            h.settle();
        }
    }
    h.send(Msg::LibFilter(LibFilter { starred_only: true, ..Default::default() }));
    assert_eq!(h.state.library_ideas().len(), 1);
    h.send(Msg::LibFilter(LibFilter { text: "alpine".into(), ..Default::default() }));
    assert_eq!(h.state.library_ideas()[0].idea.title, "Hut");
    h.send(Msg::LibFilter(LibFilter { used_in_project: Some(true), ..Default::default() }));
    assert!(h.state.library_ideas().is_empty());
    h.send(Msg::LibFilter(LibFilter::default()));

    // outputs: collections, smart folders, cover, export
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let imgs: Vec<_> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    for i in &imgs {
        h.send(Msg::ReviewLink(h.state.output_link(i).unwrap().path.clone(), Some(Verdict::Good)));
        h.settle();
    }
    h.send(Msg::NewCollection(PathBuf::new(), vec![imgs[0].clone()]));
    h.settle();
    let coll = h.state.renaming.clone().unwrap().path;
    h.send(Msg::CancelRename);
    h.send(Msg::AddToCollection(coll.clone(), vec![imgs[1].clone()]));
    h.settle();
    h.send(Msg::OutputSource(OutputSource::Collection(coll)));
    assert_eq!(h.state.output_source_images().len(), 2);
    let day = imgs[0].id.as_str()[..10].to_string();
    assert_eq!(h.state.kept_days(), [(day.clone(), 2)]);
    h.send(Msg::OutputSource(OutputSource::Smart(Smart::Day(day))));
    assert_eq!(h.state.output_source_images().len(), 2);
    h.send(Msg::OutputFilter(OutputFilter { text: "no such prompt".into(), ..Default::default() }));
    assert!(h.state.output_source_images().is_empty());
    h.send(Msg::OutputFilter(OutputFilter::default()));

    // a project with only a bad image: nothing kept, all of it with any_rating
    h.send(Msg::ReviewLink(h.state.output_link(&imgs[0]).unwrap().path.clone(), Some(Verdict::Bad)));
    h.settle();
    h.send(Msg::ShowOutputs(OutputSource::Project(project.clone())));
    assert_eq!(h.state.output_source_images().len(), 1, "the other one is still good");
    let any = OutputPreset::All.filter();
    assert!(!any.narrows(), "ratings are the tabs' business");
    h.send(Msg::OutputFilter(any));
    assert_eq!(h.state.output_source_images().len(), 2, "bad included");
    h.send(Msg::ReviewLink(h.state.output_link(&imgs[0]).unwrap().path.clone(), Some(Verdict::Good)));
    h.settle();
    h.send(Msg::OutputFilter(OutputFilter::default()));

    h.send(Msg::SetCover(project.clone(), Some(imgs[1].clone())));
    h.settle();
    assert_eq!(h.state.index.project(&project).unwrap().project.cover.as_ref(), Some(&imgs[1]));

    let dest = h._tmp.path().join("export");
    h.send(Msg::OpenExport(imgs.clone(), "2 images".into()));
    let mut e = h.state.export.clone().unwrap();
    e.dest = dest.to_string_lossy().into();
    e.keep_metadata = false;
    h.send(Msg::EditExport(e));
    h.send(Msg::RunExport);
    h.settle();
    let names: Vec<_> =
        std::fs::read_dir(&dest).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names.len(), 2);
    assert!(names.iter().all(|n| n.contains("_r0_s")), "named by run / revision / seed: {names:?}");
    let bytes = std::fs::read(dest.join(&names[0])).unwrap();
    assert!(increart_core::png::texts(&bytes).unwrap().is_empty(), "metadata stripped");

    // file manager: with no command and none installed, a clear message
    h.state.config.file_manager = "definitely-not-a-file-manager".into();
    h.send(Msg::ShowInFileManager(PathBuf::from("projects")));
    h.settle();
    assert!(h.errors().iter().any(|e| e.contains("couldn't run")), "{:?}", h.errors());
}

#[test]
fn folder_view_browse_open_paste_delete() {
    let mut h = Harness::new();
    h.send(Msg::NewFolder(increart_core::Tree::Projects, PathBuf::new()));
    h.settle();
    h.send(Msg::EditRename("clients".into()));
    h.send(Msg::CommitRename);
    h.settle();
    h.send(Msg::NewProject("clients".into()));
    h.send(Msg::EditRename("fox".into()));
    h.send(Msg::CommitRename);
    h.settle();
    h.settle();
    let fox = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&fox);
    h.rt.run(Effect::Rescan);
    h.settle();

    h.send(Msg::ProjectsTab(ProjectsTab::Folder));
    h.send(Msg::FvGo("projects".into()));
    h.settle();
    let names = |h: &Harness| h.state.folder_view.shown().iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&h), ["clients"]);
    h.send(Msg::FvOpen("clients".into()));
    h.settle();
    assert_eq!(h.state.folder_view.entries[0].kind, EntryKind::Project);
    h.send(Msg::FvOpen("fox".into()));
    h.settle();
    assert_eq!(h.state.project.as_ref(), Some(&fox), "entering a project selects it");
    assert_eq!(names(&h), ["reviewed", "runs", "unreviewed"], "project.rsn hidden until Ctrl+H");
    h.send(Msg::FvShowMeta(true));
    assert!(names(&h).contains(&"project.rsn".to_string()));
    h.send(Msg::FvShowMeta(false));

    // back / forward / up
    h.send(Msg::FvBack);
    h.settle();
    assert_eq!(h.state.folder_view.path, PathBuf::from("projects/clients"));
    h.send(Msg::FvForward);
    h.settle();
    h.send(Msg::FvOpen("unreviewed".into()));
    h.settle();
    let links = names(&h);
    assert_eq!(links.len(), 2);
    assert!(matches!(h.state.folder_view.entries[0].kind, EntryKind::Image(_)));

    // open an unreviewed image: Review, at that image
    h.send(Msg::FvOpen(links[1].clone()));
    assert_eq!(h.state.tab, Tab::Review);
    assert_eq!(
        h.state.review_queue()[h.state.review_cursor].path,
        PathBuf::from("projects/clients/fox/unreviewed").join(&links[1])
    );
    h.send(Msg::Tab(Tab::Projects));

    // cut + paste into reviewed/good = a review
    h.send(Msg::FvSelect(links[0].clone(), SelectHow::Replace));
    h.send(Msg::FvCut);
    h.send(Msg::FvUp);
    h.settle();
    h.send(Msg::FvGo("projects/clients/fox/reviewed/good".into()));
    h.send(Msg::FvPaste(None));
    h.settle();
    assert_eq!(names(&h), [links[0].clone()], "relisted after the move");
    assert_eq!(h.state.kept_outputs().len(), 1);
    // …but not into another project's folder
    h.send(Msg::FvSelect(links[0].clone(), SelectHow::Replace));
    h.send(Msg::FvCut);
    h.send(Msg::FvGo("projects/clients".into()));
    h.send(Msg::FvPaste(None));
    assert!(h.errors().iter().any(|e| e.contains("only be moved between")));

    // copy + paste a project; range select; delete with confirmation
    h.send(Msg::FvGo("projects/clients".into()));
    h.settle();
    h.send(Msg::FvSelect("fox".into(), SelectHow::Replace));
    h.send(Msg::FvCopy);
    h.send(Msg::FvPaste(None));
    h.settle();
    assert_eq!(names(&h), ["fox", "fox 2"]);
    h.send(Msg::FvSelect("fox".into(), SelectHow::Replace));
    h.send(Msg::FvSelect("fox 2".into(), SelectHow::Range));
    assert_eq!(h.state.folder_view.selected.len(), 2);
    h.send(Msg::FvSelect("fox".into(), SelectHow::Toggle));
    h.send(Msg::FvDelete);
    assert!(h.state.confirm.is_some());
    h.send(Msg::ConfirmYes);
    h.settle();
    assert_eq!(names(&h), ["fox"]);
}

#[test]
fn drag_and_drop_moves_copies_and_refuses_bad_drops() {
    let mut h = Harness::new();
    for name in ["a", "b"] {
        h.send(Msg::NewFolder(increart_core::Tree::Projects, PathBuf::new()));
        h.settle();
        h.send(Msg::EditRename(name.into()));
        h.send(Msg::CommitRename);
        h.settle();
    }
    h.send(Msg::NewProject("a".into()));
    h.send(Msg::EditRename("fox".into()));
    h.send(Msg::CommitRename);
    h.settle();
    h.settle();
    let dir = |h: &Harness| h.state.index.projects[0].dir.clone();
    let p = PathBuf::from;

    // move a project, then copy a folder (Ctrl)
    h.send(Msg::DropInto { paths: vec![p("projects/a/fox")], dest: p("projects/b"), copy: false });
    h.settle();
    assert_eq!(dir(&h), p("projects/b/fox"));
    h.send(Msg::DropInto { paths: vec![p("projects/a")], dest: p("projects/b"), copy: true });
    h.settle();
    assert!(h.state.index.project_folders.contains(&p("a")) && h.state.index.project_folders.contains(&p("b/a")));

    // onto itself or where it already is: nothing, silently
    h.send(Msg::DropInto { paths: vec![p("projects/b")], dest: p("projects/b"), copy: false });
    h.send(Msg::DropInto { paths: vec![p("projects/b/fox")], dest: p("projects/b"), copy: false });
    h.settle();
    assert!(h.errors().is_empty(), "{:?}", h.errors());
    // into its own subfolder, or into a project: refused
    h.send(Msg::DropInto { paths: vec![p("projects/b")], dest: p("projects/b/a"), copy: false });
    h.send(Msg::DropInto { paths: vec![p("projects/a")], dest: p("projects/b/fox"), copy: false });
    h.settle();
    assert_eq!(h.errors().len(), 2, "{:?}", h.errors());
    assert!(h.state.index.project_folders.contains(&p("a")) && h.state.index.project_folders.contains(&p("b")));
}

#[test]
fn scale_dialog_resizes_the_generate_size_and_resolution_rows() {
    use increart_gui_core::{ScaleBy, ScaleTarget};
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    (h.state.generate.width, h.state.generate.height) = (1344, 896);

    h.send(Msg::OpenScale(ScaleTarget::Generate));
    let mut d = h.state.scale.clone().unwrap();
    assert_eq!((d.width, d.height, d.result()), (1344, 896, (1344, 896)), "opens on the current size");
    (d.by, d.side) = (ScaleBy::Width, 1920);
    h.send(Msg::EditScale(d));
    h.send(Msg::ApplyScale);
    assert_eq!((h.state.generate.width, h.state.generate.height), (1920, 1280));
    assert!(h.state.scale.is_none() && h.state.generate_dirty.is_some(), "closed, and the draft will be saved");

    // a row of the resolutions editor; Cancel leaves it alone
    h.send(Msg::OpenResolutions);
    let first = h.state.res_editor.as_ref().unwrap()[0].clone();
    h.send(Msg::OpenScale(ScaleTarget::Resolution(0)));
    let mut d = h.state.scale.clone().unwrap();
    d.megapixels = 2.0;
    h.send(Msg::EditScale(d.clone()));
    h.send(Msg::CloseScale);
    assert_eq!(h.state.res_editor.as_ref().unwrap()[0], first);
    h.send(Msg::OpenScale(ScaleTarget::Resolution(0)));
    h.send(Msg::EditScale(d.clone()));
    h.send(Msg::ApplyScale);
    let r = &h.state.res_editor.as_ref().unwrap()[0];
    assert_eq!((r.width, r.height), d.result());
    assert_eq!(h.state.config.resolutions[0], first, "saved only with the editor's Save");
}

#[test]
fn output_library_folder_tab_goes_to_the_source_picked_in_the_grid() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let imgs: Vec<_> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    for (i, v) in imgs.iter().zip([Verdict::Good, Verdict::Neutral]) {
        h.send(Msg::ReviewLink(h.state.output_link(i).unwrap().path.clone(), Some(v)));
        h.settle();
    }
    let dir = h.state.index.project(&project).unwrap().dir.clone();
    let smart_dir = PathBuf::from("collections/smart/by project").join(dir.strip_prefix("projects").unwrap());

    // the project picked in the Grid, then the Folder tab: its smart folder
    h.send(Msg::Tab(Tab::OutputLibrary));
    h.send(Msg::OutputSource(OutputSource::Project(project.clone())));
    h.send(Msg::OutputTab(OutputTab::Folder));
    h.settle();
    assert_eq!(h.state.folder_view.path, smart_dir);
    assert_eq!(h.state.folder_view.shown().len(), 2);

    // somewhere else in the folder view; Grid and back, the source the
    // same: it stays where it was left
    h.send(Msg::FvGo(PathBuf::from("collections/smart")));
    h.settle();
    h.send(Msg::OutputTab(OutputTab::Grid));
    h.send(Msg::OutputTab(OutputTab::Folder));
    h.settle();
    assert_eq!(h.state.folder_view.path, PathBuf::from("collections/smart"));

    // another source in the Grid, then another tab and back to the
    // Output library on its Folder tab: the new source
    h.send(Msg::OutputTab(OutputTab::Grid));
    h.send(Msg::OutputSource(OutputSource::Smart(Smart::Transparent)));
    h.send(Msg::OutputTab(OutputTab::Folder));
    h.send(Msg::Tab(Tab::Projects));
    h.send(Msg::OutputSource(OutputSource::Project(project.clone())));
    h.send(Msg::Tab(Tab::OutputLibrary));
    h.settle();
    assert_eq!(h.state.folder_view.path, smart_dir);
    // Back: where it was before
    h.send(Msg::FvBack);
    h.settle();
    assert_eq!(h.state.folder_view.path, PathBuf::from("collections/smart/transparent"));
}

#[test]
fn output_library_folder_view_collections_and_smart_folders() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let imgs: Vec<_> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    for (i, v) in imgs.iter().zip([Verdict::Good, Verdict::Neutral]) {
        h.send(Msg::ReviewLink(h.state.output_link(i).unwrap().path.clone(), Some(v)));
        h.settle();
    }
    let names = |h: &Harness| h.state.folder_view.shown().iter().map(|e| e.name.clone()).collect::<Vec<_>>();

    // the Projects folder view somewhere, to check each keeps its own place
    h.send(Msg::Tab(Tab::Projects));
    h.send(Msg::ProjectsTab(ProjectsTab::Folder));
    h.settle();
    h.send(Msg::FvOpen(names(&h)[0].clone()));
    h.settle();
    let projects_path = h.state.folder_view.path.clone();

    // Output library → Folder: collections, with smart/ first
    h.send(Msg::Tab(Tab::OutputLibrary));
    h.send(Msg::OutputTab(OutputTab::Folder));
    h.settle();
    assert_eq!(h.state.folder_view.root, PathBuf::from("collections"));
    assert_eq!(names(&h), ["smart"]);
    h.send(Msg::FvOpen("smart".into()));
    h.settle();
    assert_eq!(names(&h), ["by date", "by source idea", "by project", "transparent"]);
    h.send(Msg::FvGo(PathBuf::from("collections/smart/by project")));
    h.settle();
    h.send(Msg::FvOpen(names(&h)[0].clone()));
    h.settle();
    assert_eq!(h.state.folder_view.shown().len(), 2);
    let shown = h.state.folder_view.shown();
    let good = shown.iter().find(|e| matches!(&e.kind, EntryKind::Image(i) if *i == imgs[0])).unwrap().name.clone();
    // read-only: no delete, no new folder
    h.send(Msg::FvSelect(good.clone(), SelectHow::Replace));
    h.send(Msg::FvDelete);
    assert!(h.state.confirm.is_none());
    h.send(Msg::FvCopy);

    // a new collection; paste the smart folder's image into it
    h.send(Msg::FvGo(PathBuf::from("collections")));
    h.settle();
    h.send(Msg::FvNewFolder);
    h.settle();
    h.send(Msg::EditRename("Portfolio".into()));
    h.send(Msg::CommitRename);
    h.settle();
    assert_eq!(names(&h), ["smart", "Portfolio"]);
    h.send(Msg::FvPaste(Some("Portfolio".into())));
    h.settle();
    assert_eq!(h.state.index.collection_links.len(), 1);
    assert_eq!(h.state.index.collection_links[0].image, imgs[0]);

    // the sources panel drives the view on this tab
    h.send(Msg::OutputSource(OutputSource::Collection(PathBuf::from("Portfolio"))));
    h.settle();
    assert_eq!(h.state.folder_view.path, PathBuf::from("collections/Portfolio"));
    assert_eq!(h.state.folder_view.shown().len(), 1);
    // opening the image shows it in the Grid of that collection
    let link = names(&h)[0].clone();
    h.send(Msg::FvOpen(link.clone()));
    assert_eq!(h.state.output_tab, OutputTab::Grid);
    assert_eq!(h.state.output_selected.as_ref(), Some(&imgs[0]));
    h.send(Msg::OutputTab(OutputTab::Folder));
    h.settle();

    // the other view kept its place
    h.send(Msg::Tab(Tab::Projects));
    assert_eq!(
        (h.state.folder_view.root.clone(), h.state.folder_view.path.clone()),
        (PathBuf::from("projects"), projects_path)
    );
    h.send(Msg::Tab(Tab::OutputLibrary));
    h.settle();
    assert_eq!(h.state.folder_view.path, PathBuf::from("collections/Portfolio"));

    // removing it from the collection leaves the image and its rating
    h.send(Msg::FvSelect(link, SelectHow::Replace));
    h.send(Msg::FvDelete);
    h.send(Msg::ConfirmYes);
    h.settle();
    assert!(h.state.index.collection_links.is_empty());
    assert_eq!(h.state.output_link(&imgs[0]).unwrap().verdict, Some(Verdict::Good));

    // "by project" is the project tree, folders and all
    let name = h.state.project_entry().unwrap().name();
    h.send(Msg::NewFolder(increart_core::Tree::Projects, PathBuf::new()));
    h.settle();
    h.send(Msg::EditRename("clients".into()));
    h.send(Msg::CommitRename);
    h.settle();
    h.send(Msg::MoveNode(increart_core::Tree::Projects, name.clone().into(), "clients".into()));
    h.settle();
    h.send(Msg::FvGo(PathBuf::from("collections/smart/by project")));
    h.settle();
    assert_eq!(names(&h), ["clients"]);
    h.send(Msg::FvOpen("clients".into()));
    h.settle();
    assert_eq!(names(&h), std::slice::from_ref(&name));
    h.send(Msg::FvOpen(name.clone()));
    h.settle();
    assert_eq!(h.state.folder_view.shown().len(), 2, "the project's kept images");
    // a folder as the source shows its place in the tree
    h.send(Msg::OutputSource(OutputSource::ProjectFolder("clients".into())));
    h.settle();
    assert_eq!(h.state.folder_view.path, PathBuf::from("collections/smart/by project/clients"));
    assert!(h.errors().is_empty(), "{:?}", h.errors());
}

#[test]
fn input_library_and_inbox_folder_views_and_selection_keys() {
    let mut h = Harness::new();
    // two kept ideas (input/library), one in the inbox
    for (t, add) in [("Fox", AddIdea::Keep), ("Fern", AddIdea::Keep), ("Lighthouse", AddIdea::Inbox)] {
        h.send(Msg::Draft(IdeaDraft { title: t.into(), text: t.to_lowercase(), ..Default::default() }));
        h.send(Msg::AddIdea(add));
        // Add & save asks for the name and folder: as they are
        h.send(Msg::ConfirmSaveIdea);
        h.settle();
    }
    let shown =
        |h: &Harness| h.state.folder_view.shown().iter().map(|e| (e.name.clone(), e.info.clone())).collect::<Vec<_>>();

    // Input library → Folder: ideas are folders, titled in their info
    h.send(Msg::Tab(Tab::InputLibrary));
    h.send(Msg::LibTab(LibTab::Folder));
    h.settle();
    assert_eq!(h.state.folder_view.place, FvPlace::InputLibrary);
    let ideas = shown(&h);
    assert_eq!(ideas.iter().map(|(_, info)| info.as_str()).collect::<Vec<_>>(), ["Fox", "Fern"]);
    assert!(h.state.folder_view.shown().iter().all(|e| matches!(e.kind, EntryKind::Idea(_))));

    // a folder; drag (move) the Fox idea into it
    h.send(Msg::FvNewFolder);
    h.settle();
    h.send(Msg::EditRename("animals".into()));
    h.send(Msg::CommitRename);
    h.settle();
    let fox = ideas.iter().find(|(_, i)| i == "Fox").unwrap().0.clone();
    h.send(Msg::DropInto {
        paths: vec![PathBuf::from("input/library").join(&fox)],
        dest: PathBuf::from("input/library/animals"),
        copy: false,
    });
    h.settle();
    let loc =
        |h: &Harness, title: &str| h.state.index.ideas.iter().find(|e| e.idea.title == title).unwrap().loc.clone();
    assert_eq!(loc(&h, "Fox"), IdeaLoc::Library(PathBuf::from("animals")));
    // the tree's folders drive the view on this tab
    h.send(Msg::LibFolder(Some(PathBuf::from("animals"))));
    h.settle();
    assert_eq!(h.state.folder_view.path, PathBuf::from("input/library/animals"));
    assert_eq!(shown(&h).len(), 1);
    // ideas can't be renamed here; discarding one asks first
    h.send(Msg::FvSelectAll);
    h.send(Msg::FvRename);
    assert!(h.state.renaming.is_none());
    h.send(Msg::FvDelete);
    h.send(Msg::ConfirmYes);
    h.settle();
    assert_eq!(loc(&h, "Fox"), IdeaLoc::Discarded);

    // keys: type-ahead, arrows, rubber band (a set)
    h.send(Msg::FvGo(PathBuf::from("input/library")));
    h.settle();
    let names: Vec<String> = h.state.folder_view.shown().iter().map(|e| e.name.clone()).collect();
    assert_eq!(names[0], "animals", "folders first");
    h.send(Msg::FvMoveCursor { by: 1, extend: false });
    assert_eq!(h.state.folder_view.selected, [names[0].clone()].into(), "nothing selected: the first");
    h.send(Msg::FvMoveCursor { by: 1, extend: true });
    assert_eq!(h.state.folder_view.selected.len(), 2, "Shift extends from the anchor");
    h.send(Msg::FvMoveCursor { by: -100, extend: false });
    assert_eq!(h.state.folder_view.selected, [names[0].clone()].into(), "clamped at the start");
    h.state.now = 100.0;
    h.send(Msg::FvTypeAhead("a".into()));
    h.send(Msg::FvTypeAhead("n".into()));
    assert_eq!(h.state.folder_view.selected, ["animals".to_string()].into());
    h.state.now = 102.0; // after a pause, typing starts over
    h.send(Msg::FvTypeAhead("f".into()));
    assert_eq!(h.state.folder_view.selected, [names[1].clone()].into(), "an idea by its title (Fern)");
    h.send(Msg::FvSelectSet(names.iter().cloned().collect()));
    assert_eq!(h.state.folder_view.selected.len(), names.len());

    // Idea inbox → Folder: input/ without the library
    h.send(Msg::Tab(Tab::IdeaInbox));
    h.send(Msg::InboxTab(InboxTab::Folder));
    h.settle();
    assert_eq!(h.state.folder_view.place, FvPlace::Inbox);
    let top: Vec<String> = shown(&h).into_iter().map(|(n, _)| n).collect();
    assert!(top.contains(&"unreviewed".to_string()) && !top.contains(&"library".to_string()), "{top:?}");
    h.send(Msg::FvOpen("unreviewed".into()));
    h.settle();
    assert_eq!(shown(&h).iter().map(|(_, i)| i.as_str()).collect::<Vec<_>>(), ["Lighthouse"]);
    h.send(Msg::FvSelectAll);
    h.send(Msg::FvDelete);
    assert!(h.state.confirm.is_none(), "the inbox's ideas are reviewed in the Grid");

    // collections: "smart" is the smart folders' name at the top
    h.send(Msg::Tab(Tab::OutputLibrary));
    h.send(Msg::OutputTab(OutputTab::Folder));
    h.settle();
    h.send(Msg::FvNewFolder);
    h.settle();
    h.send(Msg::EditRename("Smart".into()));
    h.send(Msg::CommitRename);
    h.settle();
    assert!(h.errors().iter().any(|e| e.contains("reserved")), "{:?}", h.errors());
    // each view kept its place
    h.send(Msg::Tab(Tab::InputLibrary));
    assert_eq!(h.state.folder_view.path, PathBuf::from("input/library"));
}

#[test]
fn duplicate_keeps_the_runs_images_and_adds_the_picked_one() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    let run = Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let imgs: Vec<_> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    // the run had a reference and a start image
    let dir = h.state.index.project(&project).unwrap().dir.clone();
    let file = h.root.join(&dir).join("runs").join(run.as_str()).join("run.rsn");
    let mut r: increart_core::model::Run = increart_core::rsnfmt::read(&file).unwrap();
    r.refs = vec![imgs[1].clone()];
    r.mode = increart_core::model::Mode::Edit { init: Some(imgs[1].clone()), strength: 0.4 };
    increart_core::rsnfmt::write(&file, &r).unwrap();
    h.rt.run(Effect::Rescan);
    h.settle();

    h.send(Msg::UseImage(imgs[0].clone(), UseTarget::DuplicateProject));
    h.send(Msg::ConfirmNewProject);
    h.settle();
    let g = &h.state.generate;
    assert_eq!(g.refs(), [imgs[1].clone(), imgs[0].clone()], "the run's reference, then the picked image");
    assert_eq!((g.init(), g.strength), (Some(imgs[1].clone()), 0.4), "the run's start image, as it was");
    assert!(h.errors().is_empty(), "{:?}", h.errors());
}

#[test]
fn image_list_roles_order_and_prompt_retag_with_confirmation() {
    use increart_core::model::GenerateImage;
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let imgs: Vec<_> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    let (a, b) = (imgs[0].clone(), imgs[1].clone());

    // two references, named in the prompt
    h.send(Msg::AddRefs(vec![a.clone(), b.clone()]));
    let mut f = h.state.generate.clone();
    f.prompt = "the fox from <image1> beside the cat from <Image 2>".into();
    h.send(Msg::Generate(f));
    assert_eq!(h.state.generate.refs(), [a.clone(), b.clone()]);
    assert!(h.state.retag.is_none(), "adding at the end renumbers nothing");

    // b becomes the start image too (one start image only)
    h.send(Msg::ImageRoles { index: 1, reference: true, start: true });
    h.send(Msg::ImageRoles { index: 0, reference: true, start: true });
    assert_eq!(h.state.generate.init(), Some(a.clone()));
    assert_eq!(h.state.generate.images.iter().filter(|i| i.start).count(), 1);

    // moving swaps the numbers: asks before touching the prompt
    h.send(Msg::MoveImage { from: 1, to: 0 });
    let r = h.state.retag.clone().unwrap();
    assert_eq!(r.changes, [(1, Some(2)), (2, Some(1))]);
    assert_eq!(h.state.generate.prompt, "the fox from <image1> beside the cat from <Image 2>", "not yet");
    h.send(Msg::RetagConfirm);
    h.settle();
    assert_eq!(h.state.generate.prompt, "the fox from <image2> beside the cat from <image1>");
    assert!(h.state.retag.is_none());

    // parking b (neither role): <image1> is gone; leaving the prompt as it is
    h.send(Msg::ImageRoles { index: 0, reference: false, start: false });
    assert_eq!(h.state.retag.as_ref().unwrap().changes, [(1, None), (2, Some(1))]);
    h.send(Msg::RetagDismiss);
    assert_eq!(h.state.generate.prompt, "the fox from <image2> beside the cat from <image1>");
    assert_eq!(h.state.generate.images.len(), 2, "parked, still in the list");
    assert_eq!(h.state.generate.refs(), std::slice::from_ref(&a));

    // removing the start image clears it; the draft keeps the list as is
    h.send(Msg::ClearInit);
    assert!(h.state.generate.init().is_none());
    h.send(Msg::Tick(h.state.now + 10.0));
    h.settle();
    let d = h.state.index.project(&project).unwrap().draft.clone().unwrap();
    assert_eq!(
        d.images,
        [
            GenerateImage { image: b.clone(), reference: false, start: false },
            GenerateImage { image: a.clone(), reference: true, start: false }
        ]
    );
    assert_eq!((d.refs, d.init), (vec![a.clone()], None), "what it sends, for older readers");
    // and the history kept the prompt before the tags changed
    assert!(h.errors().is_empty(), "{:?}", h.errors());
}

#[test]
fn outside_changes_are_picked_up_and_the_drop_folder_imports_itself() {
    let mut h = Harness::new();
    let until = |h: &mut Harness, what: &str, ok: &dyn Fn(&AppState) -> bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !ok(&h.state) {
            assert!(std::time::Instant::now() < deadline, "timed out waiting for: {what}");
            for m in h.rt.wait(Duration::from_millis(100)) {
                h.send(m);
            }
        }
    };
    until(&mut h, "the watch to start", &|s| {
        s.watch_status.contains("watching") || s.watch_status.contains("checking")
    });

    // a phone sync drops a text file into inbox_drop: it becomes an idea
    std::fs::write(h.root.join("input/inbox_drop/note.txt"), "a heron at dawn").unwrap();
    until(&mut h, "the dropped file imported", &|s| {
        s.unreviewed_ideas().iter().any(|e| e.idea.text == "a heron at dawn")
    });
    assert!(h.root.join("input/inbox_drop/imported/note.txt").is_file(), "original kept in imported/");
    h.settle();

    // a file manager deletes that idea's folder: it leaves the inbox
    let dir = h.state.unreviewed_ideas()[0].dir.clone();
    std::fs::remove_dir_all(h.root.join(dir)).unwrap();
    until(&mut h, "the deleted idea gone", &|s| s.unreviewed_ideas().is_empty());

    // a folder made outside appears in the project tree
    std::fs::create_dir(h.root.join("projects/made-outside")).unwrap();
    until(&mut h, "the new folder listed", &|s| s.index.project_folders.contains(&PathBuf::from("made-outside")));
    assert!(h.errors().is_empty(), "{:?}", h.errors());
}

#[test]
fn a_failed_presets_save_leaves_the_operations_count_alone() {
    let mut h = Harness::new();
    // a folder that can't be made: its parent is a file
    let blocked = h.root.join("library.rsn").join("prompts");
    h.rt.run(Effect::SavePresets(h.state.prompt_presets.clone(), blocked));
    let mut msgs = vec![];
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !h.rt.idle() && std::time::Instant::now() < deadline {
        msgs.extend(h.rt.wait(Duration::from_millis(50)));
    }
    msgs.extend(h.rt.poll());
    // an operation of the library's still going
    h.state.busy = 1;
    for m in msgs {
        h.send(m);
    }
    assert_eq!(h.state.busy, 1, "an op not counted can't end one that is");
    assert!(h.errors().iter().any(|e| e.contains("saving prompt presets")), "{:?}", h.errors());
}

#[test]
fn prompt_presets_apply_insert_save_and_edit() {
    use increart_core::HistoryKey;
    use increart_core::history::HistoryKind;
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let p = h.state.project.clone().unwrap();
    let tasks = &h.state.prompt_presets.tasks;
    let (cel, solid) = (
        tasks[0].presets.iter().position(|x| x.name == "Anime cel-shaded").unwrap(),
        tasks[3].presets.iter().position(|x| x.name == "Solid colour background").unwrap(),
    );

    // a click replaces the prompt; the old one goes to the history
    let mut f = h.state.generate.clone();
    f.prompt = "a fox".into();
    h.send(Msg::Generate(f));
    h.send(Msg::PresetsPanel(true));
    assert_eq!(h.state.presets_panel.as_ref().unwrap().view, PresetsView::Task(0), "no favourites: the first task");
    h.send(Msg::ApplyPreset { task: 0, preset: cel, insert_at: None });
    h.settle();
    assert!(h.state.generate.prompt.starts_with("Convert <image1> into a flat 2D anime cel-shaded"));
    let n = h.state.generate.prompt.chars().count();
    assert_eq!(h.state.prompt_select, Some(n..n), "no slot: the cursor at the end");
    let e = history(&mut h, HistoryKey::Project(p));
    assert_eq!((e[0].kind.clone(), e[0].text.as_str()), (HistoryKind::BeforePreset, "a fox"));

    // Shift+click inserts at the cursor, its {slot} selected; Queue refuses it
    h.send(Msg::PromptSelected);
    let mut f = h.state.generate.clone();
    f.prompt = "ab".into();
    h.send(Msg::Generate(f));
    h.send(Msg::ApplyPreset { task: 3, preset: solid, insert_at: Some(1) });
    let prompt = h.state.generate.prompt.clone();
    assert!(prompt.starts_with("a Replace the background") && prompt.ends_with("clean edges. b"), "{prompt}");
    let sel = h.state.prompt_select.clone().unwrap();
    assert_eq!(prompt.chars().skip(sel.start).take(sel.len()).collect::<String>(), "{colour}");
    h.send(Msg::PromptSelected);
    h.send(Msg::QueueGenerate);
    assert!(h.errors().last().unwrap().contains("Fill in {colour}"), "{:?}", h.errors());
    assert!(h.state.prompt_select.is_some(), "the slot selected again");
    // filled in the blanks table: queued with the value in, the form's
    // prompt keeping its blank
    let mut f = h.state.generate.clone();
    f.sweep.slots.insert("colour".into(), "teal".into());
    h.send(Msg::Generate(f));
    let errors = h.errors().len();
    h.send(Msg::QueueGenerate);
    h.settle();
    assert_eq!(h.errors().len(), errors, "{:?}", h.errors());
    let want = prompt.replace("{colour}", "teal");
    assert!(h.state.project_entry().unwrap().runs.iter().any(|r| r.prompt == want), "queued filled");
    assert!(h.state.generate.prompt.contains("{colour}"));

    // favourites; not saved without a presets folder
    assert!(h.send(Msg::PresetFavourite(0, cel)).is_empty());
    let folder = h.root.join("prompt-presets");
    h.state.presets_path = Some(folder.clone());
    let fx = h.send(Msg::PresetFavourite(3, solid));
    assert!(matches!(&fx[..], [Effect::SavePresets(..)]));
    h.settle();
    let load = || increart_core::prompts::PromptPresets::load_dir(&folder).unwrap().0;
    assert_eq!(load(), h.state.prompt_presets);

    // Save as preset: a name taken in the task is refused, then saved
    h.send(Msg::PresetsShow(PresetsView::Task(3)));
    h.send(Msg::OpenSavePreset);
    let mut d = h.state.save_preset.clone().unwrap();
    assert_eq!((d.task, d.preset.prompt.as_str()), (3, prompt.as_str()));
    d.preset.name = "solid colour BACKGROUND".into();
    h.send(Msg::EditSavePreset(d.clone()));
    h.send(Msg::ConfirmSavePreset);
    assert!(h.state.save_preset.as_ref().unwrap().error.as_ref().unwrap().contains("already has"));
    d.preset.name = "  Mine ".into();
    // a {slot} isn't allowed in Lazy Modify Style
    h.send(Msg::EditSavePreset(SavePresetDialog { task: 0, ..d.clone() }));
    h.send(Msg::ConfirmSavePreset);
    assert!(h.state.save_preset.as_ref().unwrap().error.as_ref().unwrap().contains("can't have a {slot}"));
    h.send(Msg::EditSavePreset(d));
    h.send(Msg::ConfirmSavePreset);
    assert!(h.state.save_preset.is_none());
    assert_eq!(h.state.prompt_presets.tasks[3].presets.last().unwrap().name, "Mine");
    h.settle();
    assert!(folder.join("Remove background/Mine.rsn").is_file());
    assert_eq!(load(), h.state.prompt_presets);

    // the editor: checked on Save, dropped on Close, defaults restorable
    h.send(Msg::OpenPresetEditor(3, None));
    let mut e = h.state.presets_editor.clone().unwrap();
    e.list.tasks[2].name = " ".into();
    h.send(Msg::EditPresetEditor(e.clone()));
    h.send(Msg::SavePresetEditor);
    assert_eq!(h.state.presets_editor.as_ref().unwrap().error.as_deref(), Some("A task has no name"));
    let mut plain = e.clone();
    plain.list.tasks[2].name = increart_core::prompts::TEXT_STYLE.into();
    plain.list.tasks[1].presets[0].prompt = "Transform <image1> into a fox.".into();
    h.send(Msg::EditPresetEditor(plain));
    h.send(Msg::SavePresetEditor);
    let err = h.state.presets_editor.as_ref().unwrap().error.clone().unwrap();
    assert!(err.ends_with("needs a {slot} for the user to fill in"), "{err}");
    e.list.tasks.truncate(2);
    h.send(Msg::EditPresetEditor(e));
    h.send(Msg::SavePresetEditor);
    assert!(h.state.presets_editor.is_none());
    assert_eq!(h.state.prompt_presets.tasks.len(), 2);
    h.send(Msg::OpenPresetEditor(0, None));
    h.send(Msg::RestorePresetDefaults);
    h.send(Msg::SavePresetEditor);
    assert_eq!(h.state.prompt_presets, increart_core::prompts::builtin());
    h.settle();
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "only default-presets.rsn left");
}

#[test]
fn switching_libraries_keeps_the_servers_and_can_go_back() {
    let mut h = Harness::new();
    let first = h.root.clone();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    assert_eq!(h.state.index.projects.len(), 1);
    h.state.servers = vec![ServerStatus {
        config: None,
        name: "box".into(),
        device: String::new(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    }];
    let presets = h.state.prompt_presets.clone();
    // prompt enhance's chat servers, one answering
    let chat = |name: &str| increart_core::pe::PeServer { name: name.into(), ..Default::default() };
    h.state.pe.server = Some(chat("A"));
    h.state.pe.more = vec![chat("B")];
    (h.state.pe.status, h.state.pe.ready, h.state.pe.asks) = (PeStatus::Ready, 2, 7);

    // the library screen, then back: nothing changes
    h.send(Msg::OtherLibrary(None));
    assert!(matches!(h.state.phase, Phase::Library(_)) && h.state.switching.is_some());
    h.send(Msg::CancelSwitch);
    assert_eq!((h.state.phase.clone(), h.state.library.as_deref()), (Phase::Main, Some(first.as_path())));

    // create another: fresh state, same servers, straight to the main window
    let second = h._tmp.path().join("second");
    h.send(Msg::OtherLibrary(Some((LibraryWizard::Create, second.to_string_lossy().into()))));
    h.send(Msg::CreateLibrary);
    h.settle();
    assert_eq!(h.state.phase, Phase::Main);
    assert_eq!(h.state.library.as_deref(), Some(second.as_path()));
    assert!(h.state.index.projects.is_empty() && h.state.project.is_none());
    assert_eq!(h.state.servers.len(), 1);
    assert_eq!(h.state.prompt_presets, presets);
    assert_eq!((h.state.pe.server.clone(), h.state.pe.more.clone()), (Some(chat("A")), vec![chat("B")]), "chat kept");
    assert_eq!((h.state.pe.status.clone(), h.state.pe.ready, h.state.pe.asks), (PeStatus::Ready, 2, 7));
    assert!(h.state.switching.is_none() && h.state.nav.back.is_empty());

    // and back to the first from the menu (the open one is ignored)
    assert!(h.send(Msg::SwitchLibrary(second.to_string_lossy().into())).is_empty());
    h.send(Msg::SwitchLibrary(first.to_string_lossy().into()));
    h.settle();
    assert_eq!(h.state.library.as_deref(), Some(first.as_path()));
    assert_eq!(h.state.index.projects.len(), 1);
    assert_eq!(h.state.config.recent_libraries[0], first.to_string_lossy());

    // while runs generate, it asks first
    h.state.active_runs.insert(h.state.index.projects[0].project.id.clone());
    h.send(Msg::SwitchLibrary(second.to_string_lossy().into()));
    assert!(h.state.confirm.as_ref().unwrap().text.contains("1 run(s) are generating"));
    assert_eq!(h.state.confirm.as_ref().unwrap().verb, "Switch library", "the button says what it does");
    assert_eq!(h.state.phase, Phase::Main);
}

#[test]
fn style_sweeps_queue_review_on_a_contact_sheet_and_reuse_a_style() {
    use increart_core::prompts::{PromptPreset, PromptPresets, PromptTask, SweepSource};
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let preset =
        |name: &str, prompt: &str| PromptPreset { name: name.into(), prompt: prompt.into(), ..Default::default() };
    h.state.prompt_presets = PromptPresets {
        tasks: vec![PromptTask {
            name: "Looks".into(),
            description: String::new(),
            presets: vec![
                preset("Clay", "claymation"),
                preset("Ink", "ink in a {mood} mood"),
                preset("Restyle", "<image1> as a woodcut"),
            ],
        }],
    };

    // Selected: ticked in the Presets window; nothing ticked is refused
    let mut f = h.state.generate.clone();
    f.prompt = "a fox, {preset}".into();
    f.sweep.on = true;
    f.seeds = 2;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    assert!(h.errors().last().unwrap().contains("No styles chosen"), "{:?}", h.errors());
    for p in 0..3 {
        h.send(Msg::SweepToggle(0, p));
    }
    // Tick all / Untick all for those listed, Clear all ticks for every one
    h.send(Msg::SweepTicks(vec![(0, 0), (0, 1)], false));
    assert_eq!(h.state.generate.sweep.selected.len(), 1);
    h.send(Msg::SweepTicks(vec![(0, 0), (0, 1), (0, 2)], true));
    assert_eq!(h.state.generate.sweep.selected.len(), 3, "no preset twice");
    h.send(Msg::SweepClearTicks);
    assert!(h.state.generate.sweep.selected.is_empty());
    h.send(Msg::SweepTicks(vec![(0, 0), (0, 1), (0, 2)], true));

    // Choose styles opens on the ticks; Cancel puts them back, OK keeps them
    let all = h.state.generate.sweep.selected.clone();
    h.send(Msg::StylesOpen);
    assert_eq!(h.state.styles.as_ref().unwrap().view, StyleView::Ticked);
    h.send(Msg::SweepToggle(0, 1));
    h.send(Msg::StylesClose(false));
    assert!(h.state.styles.is_none());
    assert_eq!(h.state.generate.sweep.selected, all, "cancelled");
    h.send(Msg::StylesOpen);
    h.send(Msg::SweepToggle(0, 1));
    h.send(Msg::StylesClose(true));
    assert_eq!(h.state.generate.sweep.selected.len(), 2, "kept");
    h.send(Msg::SweepToggle(0, 1));
    // a tick whose preset is gone can be unticked by its key
    let gone = increart_core::prompts::PresetKey { task: "Looks".into(), name: "Old".into() };
    h.state.generate.sweep.selected.push(gone.clone());
    h.send(Msg::SweepToggleKey(gone));
    let sorted = |mut v: Vec<_>| {
        v.sort();
        v
    };
    assert_eq!(sorted(h.state.generate.sweep.selected.clone()), sorted(all.clone()));
    // search, filters and sort narrow and order what it lists
    h.send(Msg::StylesOpen);
    let rows = |h: &Harness| h.state.style_rows(h.state.styles.as_ref().unwrap());
    h.send(Msg::StylesShow(StyleView::All));
    assert_eq!(rows(&h), [(0, 0), (0, 1), (0, 2)]);
    h.send(Msg::StylesSearch("MOOD".into()));
    assert_eq!(rows(&h), [(0, 1)], "searches the prompts");
    h.send(Msg::StylesSearch(String::new()));
    h.send(Msg::StylesFilter(StyleFilter { usable: true, ..Default::default() }));
    assert_eq!(rows(&h), [(0, 0), (0, 1)], "Restyle needs a reference image");
    h.send(Msg::StylesFilter(StyleFilter::default()));
    h.send(Msg::StylesSort(increart_core::config::StyleSort::Name));
    h.send(Msg::StylesShow(StyleView::Task("Looks".into())));
    assert_eq!(rows(&h), [(0, 0), (0, 1), (0, 2)]);
    h.state.prompt_presets.tasks[0].presets[0].name = "Zinc".into();
    assert_eq!(rows(&h), [(0, 1), (0, 2), (0, 0)], "A–Z");
    h.state.prompt_presets.tasks[0].presets[0].name = "Clay".into();
    h.send(Msg::StylesSort(increart_core::config::StyleSort::Listed));
    h.send(Msg::StylesClose(true));
    // a draft from before chose a whole task: opening ticks its presets,
    // and Cancel puts the task back
    let ticks = h.state.generate.sweep.selected.clone();
    h.state.generate.sweep.source = SweepSource::Task("Looks".into());
    h.send(Msg::StylesOpen);
    assert_eq!(h.state.generate.sweep.source, SweepSource::Selected);
    assert_eq!(h.state.generate.sweep.selected.len(), 3);
    h.send(Msg::StylesClose(false));
    assert_eq!(h.state.generate.sweep.source, SweepSource::Task("Looks".into()));
    h.state.generate.sweep.source = SweepSource::Selected;
    h.state.generate.sweep.selected = ticks;
    // the one needing a reference image is left out; {mood} asked once
    let err = update::sweep_runs(&h.state).unwrap_err();
    assert!(err.contains("Fill in {mood}"), "{err}");
    let mut f = h.state.generate.clone();
    f.sweep.slots.insert("mood".into(), "gloomy".into());
    h.send(Msg::Generate(f));
    let (runs, unfit) = update::sweep_runs(&h.state).unwrap();
    assert_eq!(unfit, 1);
    let label = h.state.generate.label.clone();
    let names: Vec<_> = runs.iter().map(|(f, _)| (f.label.clone(), f.prompt.clone())).collect();
    assert_eq!(
        names,
        [
            (format!("{label} · Clay"), "a fox, claymation".to_string()),
            (format!("{label} · Ink"), "a fox, ink in a gloomy mood".to_string()),
        ]
    );
    // one at a time: asks to go sweep by sweep instead; Cancel queues nothing
    h.send(Msg::JobOrder(increart_core::scheduler::Order::Sequential));
    h.send(Msg::QueueGenerate);
    assert_eq!(h.state.sweep_order_ask, Some(false));
    h.send(Msg::SweepOrderAnswer(None));
    h.settle();
    assert!(h.state.project_entry().unwrap().runs.is_empty(), "cancelled");
    h.send(Msg::QueueGenerate);
    let fx = h.send(Msg::SweepOrderAnswer(Some(true)));
    assert!(fx.iter().any(|f| matches!(f, Effect::SetJobOrder(increart_core::scheduler::Order::BySweep))));
    assert_eq!(h.state.config.job_order, increart_core::scheduler::Order::BySweep);
    assert!(h.state.sweep_order_ask.is_none() && !h.state.sweep_order_asked);
    h.settle();
    let p = h.state.project_entry().unwrap();
    assert_eq!(p.runs.len(), 2, "{:?}", h.errors());
    assert!(p.runs.iter().all(|r| r.sweep.as_ref().unwrap().name == label));
    assert_eq!(
        p.runs[0].all_seeds().map(|(_, s)| s).collect::<Vec<_>>(),
        p.runs[1].all_seeds().map(|(_, s)| s).collect::<Vec<_>>(),
        "the same seeds"
    );
    assert_eq!(h.state.generate.label, "r01", "a sweep counts as one label");
    let project = p.project.id.clone();
    let ids: Vec<_> = {
        let mut v: Vec<_> = p.runs.iter().map(|r| r.id.clone()).collect();
        v.sort();
        v
    };
    let seeds: Vec<u64> = p.runs[0].all_seeds().map(|(_, s)| s).collect();

    // the label is taken now; a big sweep asks first
    let mut f = h.state.generate.clone();
    f.label = label.clone();
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    assert!(h.errors().last().unwrap().contains("already has"), "{:?}", h.errors());
    let mut f = h.state.generate.clone();
    f.label = "big".into();
    f.seeds = 60;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    assert!(h.state.confirm.as_ref().unwrap().text.contains("120 images"));
    h.state.confirm = None;

    // three of four images in; the sheet shows the missing one pending
    Library_::steal(&h.root).write_seeds(&ids[0], &seeds);
    Library_::steal(&h.root).write_seeds(&ids[1], &seeds[..1]);
    h.rt.run(Effect::Rescan);
    h.settle();
    // Choose styles says what each style has made
    h.send(Msg::StylesOpen);
    let stats = &h.state.styles.as_ref().unwrap().stats;
    let key = |name: &str| increart_core::prompts::PresetKey { task: "Looks".into(), name: name.into() };
    let ink = &stats[&key("Ink")];
    assert_eq!((ink.unrated, ink.here, ink.sample.is_some()), (1, 1, true));
    assert_eq!(ink.last.as_ref(), Some(&ids[1]));
    assert!(!stats.contains_key(&key("Restyle")), "never used");
    h.send(Msg::StylesClose(false));
    // from an image in Review, its sheet opens with that image selected
    h.send(Msg::ReviewScope(Some(project.clone())));
    let at = h.state.review_queue().iter().position(|o| o.name.run == ids[1]).unwrap();
    h.send(Msg::ReviewCursor(at));
    h.send(Msg::ReviewSweep(project.clone(), label.clone()));
    assert_eq!(h.state.review_view, ReviewView::Sheet);
    assert_eq!(h.state.sheet_cell, (1, 0));
    // the name filter: rows with every word, the seed columns stay
    h.send(Msg::SheetFilter("IN".into()));
    let sh = h.state.sheet().unwrap();
    assert_eq!((sh.rows.len(), sh.hidden, sh.seeds.len()), (1, 1, 2));
    assert_eq!(sh.rows[0].run.sweep.as_ref().unwrap().preset, "Ink");
    assert_eq!(h.state.sheet_cell, (0, 0));
    h.send(Msg::SheetFilter("ink nothing".into()));
    assert!(h.state.sheet().unwrap().rows.is_empty());
    h.send(Msg::SheetFilter(String::new()));
    assert_eq!(h.state.sheet().unwrap().rows.len(), 2);
    // zoom: kept in the config, within the thumbnails' size
    assert_eq!(h.state.sheet_cell_size(), SHEET_CELL_DEFAULT);
    h.send(Msg::SheetZoom { size: 5000, save: true });
    assert_eq!(h.state.sheet_cell_size(), SHEET_CELL_MAX);
    h.send(Msg::SheetZoom { size: 10, save: false });
    assert_eq!(h.state.config.sheet_cell, SHEET_CELL_MIN);
    h.send(Msg::SheetCell(0, 0));
    assert_eq!(h.state.review_queue().len(), 3);
    let sh = h.state.sheet().unwrap();
    assert_eq!(sh.seeds, seeds);
    assert!(matches!(sh.rows[1].cells[1], SheetCell::Pending));

    // keys rate the selected cell and move on; row buttons rate the rest
    h.send(Msg::SheetRate(Verdict::Good));
    h.settle();
    assert_eq!(h.state.sheet_cell, (0, 1));
    h.send(Msg::SheetRow(0, Verdict::Bad));
    h.settle();
    let sh = h.state.sheet().unwrap();
    let v: Vec<_> = sh.rows[0]
        .cells
        .iter()
        .map(|c| match c {
            SheetCell::Image(o) => o.verdict,
            _ => None,
        })
        .collect();
    assert_eq!(v, [Some(Verdict::Good), Some(Verdict::Bad)], "the reviewed one kept its verdict");
    h.send(Msg::SheetMove(5, 5));
    assert_eq!(h.state.sheet_cell, (1, 1));
    // rating the row's last seed goes on to the next row's first
    h.send(Msg::SheetCell(0, 1));
    h.send(Msg::SheetRate(Verdict::Bad));
    h.settle();
    assert_eq!(h.state.sheet_cell, (1, 0));
    // both rows side by side: the arrows move as shown, Tile places as shown
    h.send(Msg::SheetBands(2));
    h.send(Msg::SheetMove(0, -1));
    assert_eq!(h.state.sheet_cell, (0, 1), "back into the first band");
    h.send(Msg::SheetMove(1, 0));
    assert_eq!(h.state.sheet_cell, (0, 1), "one line only");
    h.send(Msg::SheetMove(0, 5));
    assert_eq!(h.state.sheet_cell, (1, 1));
    let row1 = match &h.state.sheet().unwrap().rows[1].cells[0] {
        SheetCell::Image(o) => o.image.clone(),
        c => panic!("{c:?}"),
    };
    assert_eq!(h.state.sheet_cells()[&row1], (2, 0));
    h.send(Msg::SheetBands(1));
    assert_eq!(h.state.sheet_cells()[&row1], (0, 1));
    h.send(Msg::SheetMove(5, 5));

    // the large preview steps through the sheet's images row by row,
    // skipping the pending cell, and the selected cell follows
    let row0: Vec<_> = match &h.state.sheet().unwrap().rows[0].cells[..] {
        [SheetCell::Image(a), SheetCell::Image(b)] => vec![a.image.clone(), b.image.clone()],
        c => panic!("{c:?}"),
    };
    h.send(Msg::Tab(Tab::Review));
    h.send(Msg::Preview(Some(row0[1].clone())));
    assert_eq!(h.state.preview_position(), Some((1, 3)));
    h.send(Msg::PreviewStep(1));
    assert_eq!(h.state.preview_position(), Some((2, 3)));
    assert_eq!(h.state.sheet_cell, (1, 0), "on to the next row");
    h.send(Msg::PreviewStep(1));
    assert_eq!(h.state.preview_position(), Some((2, 3)), "stops at the last");
    h.send(Msg::PreviewStep(-2));
    assert_eq!(h.state.preview.as_ref(), Some(&row0[0]));
    assert_eq!(h.state.sheet_cell, (0, 0));
    // rating there re-rates and steps on
    h.send(Msg::PreviewRate(Verdict::Neutral));
    h.settle();
    assert_eq!(h.state.preview.as_ref(), Some(&row0[1]));
    assert!(
        matches!(&h.state.sheet().unwrap().rows[0].cells[0], SheetCell::Image(o) if o.verdict == Some(Verdict::Neutral))
    );
    // ↑ / ↓: the same seed, a style up or down; the pending cell below
    // row 0 seed 1 has no image, so that stays
    h.send(Msg::PreviewRow(1));
    assert_eq!(h.state.preview.as_ref(), Some(&row0[1]));
    h.send(Msg::PreviewStep(-1));
    h.send(Msg::PreviewRow(1));
    assert_eq!(h.state.sheet_cell, (1, 0));
    assert_eq!(h.state.preview_position(), Some((2, 3)));
    h.send(Msg::PreviewRow(-1));
    assert_eq!(h.state.preview.as_ref(), Some(&row0[0]));
    assert_eq!(h.state.sheet_cell, (0, 0));
    h.send(Msg::Preview(None));

    // a job's details open for a run that exists, with its generation log
    let (p, _) = h.state.index.run(&ids[0]).unwrap();
    let file = increart_core::engine::run_log(&h.state.library.clone().unwrap().join(&p.dir), &ids[0]);
    std::fs::write(&file, "10:00 queued 3 seeds\n10:01 [gpu0] seed 0: sent\n").unwrap();
    h.send(Msg::JobInfo(Some(ids[0].clone())));
    h.settle();
    assert_eq!(h.state.job_info.as_ref(), Some(&ids[0]));
    assert_eq!(h.state.job_log, ["10:00 queued 3 seeds", "10:01 [gpu0] seed 0: sent"]);
    // lines the engine logs now: the open run's only
    h.send(Msg::Engine(increart_core::engine::Event::Log { run: ids[0].clone(), line: "10:02 done".into() }));
    h.send(Msg::Engine(increart_core::engine::Event::Log { run: ids[1].clone(), line: "10:02 other".into() }));
    assert_eq!(h.state.job_log.last().map(String::as_str), Some("10:02 done"));
    assert_eq!(h.state.job_log.len(), 3);
    // read again (as when it opens) with a line logged meanwhile: kept
    h.send(Msg::JobLog(ids[0].clone(), vec!["10:00 queued 3 seeds".into()]));
    assert_eq!(h.state.job_log, ["10:00 queued 3 seeds", "10:01 [gpu0] seed 0: sent", "10:02 done"]);
    h.send(Msg::JobInfo(Some("2001.01.01-00.00-0000".parse().unwrap())));
    assert!(h.state.job_info.is_none(), "no such run");
    assert!(h.state.job_log.is_empty());

    // "Use this style": its run back in Generate as a single prompt
    h.send(Msg::RunInGenerate(ids[1].clone()));
    assert_eq!(h.state.generate.prompt, "a fox, ink in a gloomy mood");
    assert!(!h.state.generate.sweep.on);
    assert_eq!(h.state.generate.sweep.source, SweepSource::Selected);

    // "Same recipe & seed": that image's run settings, its seed alone
    let o = row0[1].clone();
    let seed = h.state.output_link(&o).unwrap().name.seed;
    h.send(Msg::RecipeInGenerate(o));
    assert_eq!(h.state.generate.prompt, "a fox, claymation", "row 0's style");
    assert_eq!((h.state.generate.seeds, h.state.generate.first_seed), (1, seed));
    assert!(!h.state.generate.sweep.on);

    // leaving the scope drops the sheet
    h.send(Msg::ReviewScope(None));
    assert_eq!((h.state.review_sweep.as_ref(), h.state.review_view), (None, ReviewView::Single));

    // a single run next to the sweep: Review's scope lists both, the
    // sweep once, and scopes to the run alone
    let mut f = h.state.generate.clone();
    f.label = "single".into();
    f.sweep.on = false;
    f.seeds = 1;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    h.settle();
    let p = h.state.project_entry().unwrap();
    let single = p.runs.iter().find(|r| r.sweep.is_none()).unwrap().clone();
    let rows: Vec<_> = h
        .state
        .review_runs(p)
        .into_iter()
        .map(|r| match r {
            ReviewRun::Single(r) => r.label.clone(),
            ReviewRun::Sweep(name) => format!("sweep {name}"),
        })
        .collect();
    assert_eq!(rows, ["single".to_string(), format!("sweep {label}")], "newest first");
    let seeds: Vec<u64> = single.all_seeds().map(|(_, s)| s).collect();
    Library_::steal(&h.root).write_seeds(&single.id, &seeds);
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::ReviewSweep(project.clone(), label.clone()));
    h.send(Msg::ReviewRun(project.clone(), single.id.clone()));
    assert_eq!((h.state.review_sweep.as_ref(), h.state.review_view), (None, ReviewView::Single));
    let q = h.state.review_queue();
    assert!(!q.is_empty() && q.iter().all(|o| o.name.run == single.id), "only the run's images");
    let n = q.len();
    h.send(Msg::ReviewScope(Some(project.clone())));
    assert!(h.state.review_run.is_none());
    assert!(h.state.review_queue().len() > n);

    // picking on the sheet: Ctrl+click starts from the cursor, Shift+click
    // adds the range from the last picked
    h.send(Msg::Tab(Tab::Review));
    h.send(Msg::ReviewSweep(project.clone(), label.clone()));
    h.send(Msg::SheetCell(0, 0));
    let all = h.state.pickable();
    assert_eq!(all.len(), 3, "two styles: two images and one");
    h.send(Msg::PickImage(all[2].clone(), SelectHow::Toggle));
    assert_eq!(h.state.picked, [all[0].clone(), all[2].clone()]);
    h.send(Msg::PickImage(all[1].clone(), SelectHow::Range));
    assert_eq!(h.state.picked, [all[0].clone(), all[2].clone(), all[1].clone()]);
    h.send(Msg::PickImage(all[2].clone(), SelectHow::Toggle));
    h.send(Msg::PickImage(all[2].clone(), SelectHow::Toggle));
    assert_eq!(h.state.picked.last(), Some(&all[2]), "toggled off and on: picked last");

    // tiled as they sit on the sheet, captioned by style
    h.send(Msg::OpenTile);
    let cells: Vec<_> = h.state.tile.as_ref().unwrap().layout.tiles.iter().map(|t| (t.col, t.row)).collect();
    assert_eq!(cells, [(0, 0), (1, 0), (0, 1)], "in pick order, where they sit on the sheet");
    h.send(Msg::TileCaptions(CaptionFill::Style));
    let captions: Vec<_> = h.state.tile.as_ref().unwrap().layout.tiles.iter().map(|t| t.caption.clone()).collect();
    assert_eq!(captions, ["Clay", "Clay", "Ink"]);
    // a text block in the hole, a title above: composed, not a source
    h.send(Msg::TileAddText(1, 1));
    h.send(Msg::TileText(3, "Clay vs Ink".into()));
    h.send(Msg::TileTitle("Styles".into(), "r02".into()));
    let t = h.state.tile.as_ref().unwrap();
    assert_eq!((t.selected, t.layout.tiles[3].col, t.layout.tiles[3].row), (Some(3), 1, 1));
    h.send(Msg::TileCaptions(CaptionFill::Style));
    assert_eq!(h.state.tile.as_ref().unwrap().layout.tiles[3].caption, "", "text keeps no caption");
    // a caption format: kept as typed, filled per image (unknown ones stay)
    h.send(Msg::TileCaptionFormat("#{n} {style} · seed {seed} {nope}".into()));
    let t = h.state.tile.as_ref().unwrap();
    assert_eq!(t.layout.tiles[3].caption, "", "not text blocks");
    let seed = h.state.output_link(t.layout.tiles[1].image().unwrap()).unwrap().name.seed;
    let shown = h.state.caption_text(&t.layout.tiles[1].caption, t.layout.tiles[1].image(), 2);
    assert_eq!(shown, format!("#2 Clay · seed {seed} {{nope}}"));
    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("tiles");
    h.send(Msg::Picked(PickKind::TileFile(increart_core::config::TileFormat::Png), vec![file.clone()]));
    h.settle();
    let png = image::open(file.with_extension("png")).unwrap();
    assert!(png.width() > 0 && png.height() > 0, "{:?}", h.errors());
    // lossy JPEG at the default 70 %: a .png name becomes .jpg
    use increart_core::config::TileFormat;
    assert_eq!(h.state.config.tile.save_quality, 70);
    h.send(Msg::TileSettings(increart_core::config::TileSettings {
        save_format: TileFormat::Jpeg,
        ..h.state.config.tile.clone()
    }));
    h.send(Msg::Picked(PickKind::TileFile(TileFormat::Jpeg), vec![file.with_extension("png")]));
    h.settle();
    let jpg = std::fs::read(file.with_extension("jpg")).unwrap();
    assert_eq!(image::guess_format(&jpg).unwrap(), image::ImageFormat::Jpeg, "{:?}", h.errors());
    assert_eq!(image::load_from_memory(&jpg).unwrap().width(), png.width());
    h.send(Msg::TileOut(TileTarget::Library));
    h.settle();
    use increart_core::model::IdeaSource;
    let tiled = h.state.index.ideas.iter().find(|e| matches!(e.idea.source, IdeaSource::Tiled { .. })).unwrap();
    assert_eq!(tiled.idea.source, IdeaSource::Tiled { images: h.state.picked.clone() });
    assert_eq!(tiled.idea.images.len(), 1);
    // images from the libraries, from a cell on: the picker's, for the tiling
    let input = increart_core::ImageRef::input(tiled.idea.images[0].clone());
    h.send(Msg::TilePicker(Some((1, 1))));
    assert_eq!(h.state.ref_picker.as_ref().unwrap().target, PickTarget::Tile(Some((1, 1))));
    h.send(Msg::RefPickerSource(RefSource::Inputs(None)));
    assert!(h.state.ref_candidates().iter().any(|c| c.image == input));
    h.send(Msg::RefPickerToggle(input.clone()));
    h.send(Msg::RefPickerAdd);
    let t = h.state.tile.as_ref().unwrap();
    let i = t.layout.tiles.iter().position(|x| x.image() == Some(&input)).unwrap();
    assert_eq!((t.layout.tiles[i].col, t.layout.tiles[i].row), (0, 2), "the text has (1, 1): the next free");
    // pasted: a tile, captioned as such, composed but not a source
    let pasted = increart_gui_core::tile::TileImage(std::sync::Arc::new(image::RgbaImage::new(8, 8)));
    h.send(Msg::TileAdd(vec![increart_gui_core::tile::TileContent::Pasted(pasted)], None));
    h.send(Msg::TileCaption(5, "by hand".into()));
    // the format and the fills are for generated images only: the input
    // and the pasted one (no seed, no run) keep their own captions
    h.send(Msg::TileCaptionFormat("{kind}".into()));
    h.send(Msg::TileCaptions(CaptionFill::Seed));
    h.send(Msg::TileCaptionFormat("{kind}".into()));
    let t = h.state.tile.as_ref().unwrap();
    let kinds: Vec<_> =
        t.layout.tiles.iter().enumerate().map(|(n, x)| h.state.caption_text(&x.caption, x.image(), n + 1)).collect();
    assert_eq!(kinds, ["output", "output", "output", "", "", "by hand"]);
    h.send(Msg::TileCaptions(CaptionFill::Clear));
    assert!(h.state.tile.as_ref().unwrap().layout.tiles.iter().all(|x| x.caption.is_empty()), "Clear clears all");
    let errors = h.errors().len();
    h.send(Msg::Picked(PickKind::TileFile(increart_core::config::TileFormat::Png), vec![file.clone()]));
    h.settle();
    assert_eq!(h.errors().len(), errors, "{:?}", h.errors());
    // the Generate picker doesn't open on the tiling's inputs
    h.send(Msg::RefPickerSource(RefSource::TileInputs));
    h.send(Msg::RefPicker(false));
    h.send(Msg::RefPicker(true));
    assert_eq!(h.state.ref_picker.as_ref().unwrap().source, RefSource::Inputs(None));
    h.send(Msg::RefPicker(false));
    h.send(Msg::CloseTile);

    // rated together, in one op; the sheet keeps them picked, another tab drops them
    let before: Vec<_> = all.iter().map(|i| h.state.output_link(i).unwrap().verdict).collect();
    let undos = h.state.review_undo.len();
    let fx = h.send(Msg::RatePicked(Verdict::Bad));
    assert_eq!(fx.iter().filter(|f| matches!(f, Effect::Lib(_))).count(), 1, "one op for all");
    h.settle();
    assert!(all.iter().all(|i| h.state.output_link(i).unwrap().verdict == Some(Verdict::Bad)));
    assert_eq!(h.state.picked.len(), 3);
    // one undo takes back the whole rating
    h.send(Msg::UndoReview);
    h.settle();
    let after: Vec<_> = all.iter().map(|i| h.state.output_link(i).unwrap().verdict).collect();
    assert_eq!(after, before, "each back to its own verdict");
    assert_eq!(h.state.review_undo.len(), undos, "one step");
    h.send(Msg::RatePicked(Verdict::Bad));
    h.settle();
    h.send(Msg::Tab(Tab::Projects));
    assert!(h.state.picked.is_empty());

    // a kept image: its run under the project in the Output library
    h.send(Msg::PickSet(vec![all[0].clone()]));
    h.send(Msg::RatePicked(Verdict::Good));
    h.settle();
    let run = h.state.output_link(&all[0]).unwrap().name.run.clone();
    let p = h.state.index.projects.iter().find(|p| p.project.id == project).unwrap();
    let kept: Vec<_> = h.state.kept_runs(p).into_iter().map(|(r, n)| (r.id.clone(), n)).collect();
    assert_eq!(kept, [(run, 1)]);
    // the project tree's runs: hidden (kept in config)
    h.send(Msg::ProjectRuns(false));
    h.settle();
    assert!(h.state.config.hide_project_runs);
}

#[test]
fn image_usage_counts_runs_as_ref_and_as_start() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    let first = Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let imgs: Vec<_> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    let (a, b) = (imgs[0].clone(), imgs[1].clone());
    assert_eq!(h.state.usage(&a), Usage::default());
    assert_eq!(h.state.usage(&a).label(), None);

    // a as <image1>, b as the start image
    h.send(Msg::AddRefs(vec![a.clone(), b.clone()]));
    h.send(Msg::ImageRoles { index: 1, reference: false, start: true });
    let mut f = h.state.generate.clone();
    f.prompt = "the fox from <image1>".into();
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    h.settle();
    assert!(h.errors().is_empty(), "{:?}", h.errors());
    assert_eq!(h.state.usage(&a), Usage { ref_runs: 1, start_runs: 0, images: 0 });
    assert_eq!(h.state.usage(&b), Usage { ref_runs: 0, start_runs: 1, images: 0 });
    assert_eq!(h.state.usage(&a).label().unwrap(), "ref in 1 run · 0 images");
    // a second run with a as a ref again
    h.send(Msg::ImageRoles { index: 1, reference: false, start: false });
    h.send(Msg::QueueGenerate);
    h.settle();
    assert_eq!(h.state.usage(&a).ref_runs, 2);
    assert_eq!(h.state.usage(&b).start_runs, 1);
    assert!(h.state.index.run(&first).is_some());
}

#[test]
fn generate_history_keeps_settings_and_runs() {
    use increart_core::HistoryKey;
    use increart_core::history::HistoryKind;
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let p = h.state.project.clone().unwrap();
    let key = HistoryKey::Project(p.clone());
    let r01 = Library_::steal(&h.root).write_two(&p);
    h.rt.run(Effect::Rescan);
    h.settle();
    let imgs: Vec<_> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    let edit_at = |h: &mut Harness, t: f64, change: &dyn Fn(&mut GenerateForm)| {
        h.send(Msg::Tick(t));
        let mut f = h.state.generate.clone();
        change(&mut f);
        h.send(Msg::Generate(f));
        h.send(Msg::Tick(t + 2.5));
        h.settle();
    };

    // the whole form is kept, and the run it follows
    h.send(Msg::AddRefs(vec![imgs[0].clone()]));
    edit_at(&mut h, 100.0, &|f| {
        f.prompt = "a fox from <image1>".into();
        f.label = "r02".into();
        f.steps = 30;
    });
    let e = history(&mut h, key.clone());
    let form = e[0].form.as_ref().expect("the form");
    assert_eq!((form.steps, form.refs.clone()), (30, vec![imgs[0].clone()]));
    assert_eq!(e[0].after.as_ref(), Some(&r01));

    // queued: the checkpoint names its run
    h.send(Msg::QueueGenerate);
    h.settle();
    let r02 = h.state.project_entry().unwrap().runs.iter().find(|r| r.label == "r02").unwrap().id.clone();
    let e = history(&mut h, key.clone());
    assert_eq!(e[0].kind, HistoryKind::Queued("r02".into()));
    assert_eq!((e[0].runs.clone(), e[0].after.clone()), (vec![r02.clone()], Some(r01.clone())));
    assert_eq!(e[0].form.as_ref().unwrap().steps, 30);

    // a settings change is an entry; another one soon after merges into it
    edit_at(&mut h, 110.0, &|f| f.steps = 40);
    edit_at(&mut h, 120.0, &|f| f.cfg = 3.5);
    h.send(Msg::OpenHistory(key.clone()));
    h.settle();
    let v = h.state.history_view.clone().unwrap();
    // (the edit queued unchanged became the queued checkpoint)
    assert_eq!(v.entries.len(), 2, "{:?}", v.entries.iter().map(|e| &e.kind).collect::<Vec<_>>());
    assert_eq!((v.entries[0].kind.clone(), v.after[0].clone()), (HistoryKind::Edit, Some(r02.clone())));
    assert_eq!(v.changes(0), ["steps 30 → 40", "cfg 2 → 3.5"]);
    h.send(Msg::HistorySettingsOnly(true));
    assert_eq!(h.state.history_view.as_ref().unwrap().shown().len(), 1);
    h.send(Msg::HistorySettingsOnly(false));

    // Restore all: settings and images back; the label stays the next run's
    let label = h.state.generate.label.clone();
    h.send(Msg::RemoveImage(0));
    edit_at(&mut h, 200.0, &|f| f.prompt = "a cat".into());
    assert!(h.state.generate.images.is_empty());
    h.send(Msg::OpenHistory(key.clone()));
    h.settle();
    let i = h.state.history_view.as_ref().unwrap().entries.iter().position(|e| e.runs == [r02.clone()]).unwrap();
    h.send(Msg::HistorySelect(i));
    h.send(Msg::HistoryRestore(Restore::All));
    h.settle();
    let g = &h.state.generate;
    assert_eq!((g.prompt.as_str(), g.steps, g.cfg), ("a fox from <image1>", 30, 2.0));
    assert_eq!(g.refs(), vec![imgs[0].clone()]);
    assert_eq!(g.label, label);

    // Show in history: what led to r02, at its checkpoint
    h.send(Msg::RunHistory(r02.clone()));
    h.settle();
    let v = h.state.history_view.clone().unwrap();
    assert_eq!(v.run_filter, Some((r02.clone(), Some(r01.clone()))));
    assert_eq!(v.runs_of(v.selected), std::slice::from_ref(&r02));
    let shown: Vec<_> = v.shown().iter().map(|r| r.entry.kind.clone()).collect();
    assert_eq!(shown, [HistoryKind::Queued("r02".into())], "not the edits since");
    h.send(Msg::CloseHistory);

    // older entries (prompt only) find their runs by label and time
    let file = h.root.join("projects/new project/prompt-history.jsonl");
    let now = increart_core::id::now();
    std::fs::remove_file(&file).unwrap();
    for (kind, text) in [(HistoryKind::Queued("r02".into()), "x"), (HistoryKind::Edit, "y")] {
        let e = increart_core::history::HistoryEntry::text(now, kind, text.into(), String::new());
        increart_core::history::record(&file, &e, false).unwrap();
    }
    h.send(Msg::OpenHistory(key.clone()));
    h.settle();
    let v = h.state.history_view.clone().unwrap();
    assert_eq!(v.runs, [vec![], vec![r02.clone()]]);
    assert_eq!(v.after[0], Some(r02.clone()), "the newest run before it");
    h.send(Msg::CloseHistory);

    // an image gone from the library is left out of a restore
    std::fs::remove_file(imgs[0].path(&h.root)).unwrap();
    std::fs::remove_file(&file).unwrap();
    h.send(Msg::Tick(300.0));
    h.send(Msg::QueueGenerate);
    h.settle();
    h.send(Msg::OpenHistory(key));
    h.settle();
    h.send(Msg::HistorySelect(0));
    h.send(Msg::HistoryRestore(Restore::All));
    h.settle();
    assert!(h.state.generate.images.is_empty());
    assert!(h.errors().iter().any(|e| e.contains("no longer in the library")), "{:?}", h.errors());
}

fn plays(fx: &[Effect]) -> usize {
    fx.iter().filter(|f| matches!(f, Effect::PlaySound { test: false, .. })).count()
}

fn idle_server(s: &mut AppState, i: usize, idle: bool) {
    update(
        s,
        Msg::Engine(increart_core::engine::Event::Server {
            server: i,
            state: "ready".into(),
            model: None,
            idle,
            runs: vec![],
        }),
    );
}

#[test]
fn prompt_enhance_asking_holds_the_gpu_idle_alert() {
    let mut s = dialog_state(vec![], None);
    let mut n = s.config.notifications.clone();
    assert!(n.pe_counts_as_work, "on by default");
    n.gpu_idle.on = true;
    n.gpu_idle.threshold = 5;
    n.player = "true {file}".into();
    update(&mut s, Msg::SetNotifications(n));
    s.servers = vec![ServerStatus {
        config: None,
        name: "a".into(),
        device: String::new(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    }];
    s.engine_running = true;
    let at = |s: &mut AppState, min: f64| plays(&update(s, Msg::Tick(min * 60.0)));
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 2.0), 0);

    // asking from 2 to 8 min: not idle then, and idle again from 8
    let key = PeKey::Text(1);
    s.pe.jobs.push(PeJobState { id: 1, key, step: PeStep::Queued, asked: 120.0, since: 120.0, rate: None, on: None });
    assert_eq!(at(&mut s, 6.0), 0);
    assert_eq!(at(&mut s, 8.0), 0);
    s.pe.jobs.clear();
    assert_eq!(at(&mut s, 12.0), 0, "4 min since the last ask");
    assert_eq!(at(&mut s, 13.0), 1);

    // off: the GPU's own idle time only
    let mut s2 = s.clone();
    let mut n = s2.config.notifications.clone();
    n.pe_counts_as_work = false;
    update(&mut s2, Msg::SetNotifications(n));
    idle_server(&mut s2, 0, false);
    idle_server(&mut s2, 0, true);
    let key = PeKey::Text(2);
    s2.pe.jobs.push(PeJobState { id: 2, key, step: PeStep::Queued, asked: 0.0, since: 0.0, rate: None, on: None });
    assert_eq!(at(&mut s2, 18.0), 1, "sounds while asking");
}

#[test]
fn gpu_idle_alert_repeats_while_it_lasts() {
    let mut s = dialog_state(vec![], None);
    let mut n = s.config.notifications.clone();
    n.gpu_idle.on = true;
    n.gpu_idle.threshold = 5;
    n.gpu_idle.repeat_minutes = 10;
    n.player = "true {file}".into();
    update(&mut s, Msg::SetNotifications(n));
    let gpu = |name: &str| ServerStatus {
        config: None,
        name: name.into(),
        device: String::new(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    };
    s.servers = vec![gpu("a"), gpu("b")];
    let at = |s: &mut AppState, min: f64| plays(&update(s, Msg::Tick(min * 60.0)));

    // no engine: nothing
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 6.0), 0);
    s.engine_running = true;
    assert_eq!(at(&mut s, 4.0), 0, "under the threshold");
    assert_eq!(at(&mut s, 5.0), 1);
    assert_eq!(s.alerts.idle_text.as_deref(), Some("a idle for 5 min"));
    assert!(s.alerts.attention);
    assert_eq!(at(&mut s, 10.0), 0, "not again before the repeat");
    assert_eq!(at(&mut s, 15.0), 1);

    // work resets it; idle again counts from then
    idle_server(&mut s, 0, false);
    assert_eq!(at(&mut s, 16.0), 0);
    assert!(s.alerts.idle_text.is_none());
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 20.0), 0);
    assert_eq!(at(&mut s, 21.0), 1);

    // "all GPUs": b has to be idle too
    let mut n = s.config.notifications.clone();
    n.all_gpus = true;
    update(&mut s, Msg::SetNotifications(n));
    assert_eq!(at(&mut s, 40.0), 0);
    idle_server(&mut s, 1, true);
    assert_eq!(at(&mut s, 44.0), 0);
    assert_eq!(at(&mut s, 45.0), 1);
    assert_eq!(s.alerts.idle_text.as_deref(), Some("All GPUs idle for 5 min"));

    // snoozed: held back, then played when the snooze ends
    update(&mut s, Msg::Snooze(Some(30.0 * 60.0)));
    assert_eq!(at(&mut s, 60.0), 0);
    assert_eq!(at(&mut s, 75.0), 1);
    update(&mut s, Msg::Snooze(None));
    assert_eq!(at(&mut s, 200.0), 0, "muted until restart");
    update(&mut s, Msg::Unsnooze);
    assert_eq!(at(&mut s, 201.0), 1);

    // only while another window is active
    let mut n = s.config.notifications.clone();
    n.only_unfocused = true;
    update(&mut s, Msg::SetNotifications(n));
    update(&mut s, Msg::Focused(true));
    assert_eq!(at(&mut s, 300.0), 0);
    update(&mut s, Msg::Focused(false));
    assert_eq!(at(&mut s, 301.0), 1);

    // off
    let mut n = s.config.notifications.clone();
    n.gpu_idle.on = false;
    update(&mut s, Msg::SetNotifications(n));
    assert_eq!(at(&mut s, 400.0), 0);
    assert!(s.alerts.idle_text.is_none());
}

#[test]
fn queue_cleared_says_what_the_gpu_did() {
    let mut s = dialog_state(vec![], None);
    let mut n = s.config.notifications.clone();
    n.queue_cleared.on = true;
    n.player = "true {file}".into();
    update(&mut s, Msg::SetNotifications(n));
    s.servers = vec![ServerStatus {
        config: None,
        name: "a".into(),
        device: String::new(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    }];
    s.engine_running = true;
    update(&mut s, Msg::Tick(10.0));
    idle_server(&mut s, 0, false);
    for seed in 0..2 {
        update(
            &mut s,
            Msg::Engine(increart_core::engine::Event::Output {
                run: increart_core::Id::new(increart_core::id::now(), 1),
                rev: 0,
                seed,
                image: increart_core::Id::new(increart_core::id::now(), 2),
                link: PathBuf::new(),
                server: 0,
                elapsed_s: 30.0,
                step_s: 1.5,
            }),
        );
    }
    update(&mut s, Msg::Tick(70.0));
    idle_server(&mut s, 0, true);
    update(&mut s, Msg::Tick(100.0));
    assert_eq!(s.alerts.cleared_text.as_deref(), Some("a finished its queue · 2 image(s) in 1 min 30 s"));
    assert!(s.alerts.busy.is_empty(), "its spell ended");
}

#[test]
fn queue_cleared_alert_sounds_once_when_a_gpu_finishes() {
    let mut s = dialog_state(vec![], None);
    let mut n = s.config.notifications.clone();
    n.queue_cleared.on = true;
    n.player = "true {file}".into();
    update(&mut s, Msg::SetNotifications(n));
    let gpu = |name: &str| ServerStatus {
        config: None,
        name: name.into(),
        device: String::new(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    };
    s.servers = vec![gpu("a"), gpu("b")];
    s.engine_running = true;
    let at = |s: &mut AppState, sec: f64| plays(&update(s, Msg::Tick(sec)));

    // ready from the start: no news
    idle_server(&mut s, 0, true);
    idle_server(&mut s, 1, true);
    assert_eq!(at(&mut s, 1.0), 0);

    // a job, then none: once, at once
    idle_server(&mut s, 0, false);
    idle_server(&mut s, 1, false);
    assert_eq!(at(&mut s, 2.0), 0);
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 3.0), 1);
    assert_eq!(s.alerts.cleared_text.as_deref(), Some("a finished its queue"));
    assert!(s.alerts.attention);
    assert_eq!(at(&mut s, 400.0), 0, "no repeat");
    update(&mut s, Msg::Focused(true));
    assert!(s.alerts.cleared_text.is_none(), "seen");
    update(&mut s, Msg::Focused(false));

    // "all GPUs": only when the last busy one clears
    let mut n = s.config.notifications.clone();
    n.queue_cleared_all = true;
    update(&mut s, Msg::SetNotifications(n));
    idle_server(&mut s, 0, false);
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 401.0), 0, "b is still working");
    idle_server(&mut s, 1, true);
    assert_eq!(at(&mut s, 402.0), 1);
    assert_eq!(s.alerts.cleared_text.as_deref(), Some("All GPUs finished their queues"));

    // snoozed: shown, not played (and not later)
    update(&mut s, Msg::Snooze(Some(60.0)));
    idle_server(&mut s, 0, false);
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 403.0), 0);
    assert_eq!(at(&mut s, 500.0), 0);
    update(&mut s, Msg::Unsnooze);

    // stopped now by the user, who saw it: no alert; Finish & stop, or
    // another run after it, still sounds
    let t0 = increart_core::id::now();
    let (x, y) = (increart_core::Id::new(t0, 1), increart_core::Id::new(t0, 2));
    let mut n = s.config.notifications.clone();
    n.queue_cleared_all = false;
    update(&mut s, Msg::SetNotifications(n));
    let work = |s: &mut AppState, run: &increart_core::Id| {
        let e = increart_core::engine::Event::Server {
            server: 0,
            state: "ready".into(),
            model: None,
            idle: false,
            runs: vec![run.clone()],
        };
        update(s, Msg::Engine(e));
    };
    work(&mut s, &x);
    update(&mut s, Msg::CancelRun(x.clone(), increart_core::engine::Stop::Now));
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 510.0), 0, "stopped by the user");
    work(&mut s, &y);
    update(&mut s, Msg::CancelRun(y.clone(), increart_core::engine::Stop::Finish));
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 520.0), 1, "Finish & stop: its last images done");
    work(&mut s, &y);
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 530.0), 1);
    // one of its runs stopped now, another not: the other finished
    work(&mut s, &x);
    let z = increart_core::Id::new(t0, 3);
    work(&mut s, &z);
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 540.0), 1);

    // off
    let mut n = s.config.notifications.clone();
    n.queue_cleared.on = false;
    update(&mut s, Msg::SetNotifications(n));
    idle_server(&mut s, 0, false);
    idle_server(&mut s, 0, true);
    assert_eq!(at(&mut s, 600.0), 0);
}

#[test]
fn review_backlog_alert_counts_unreviewed_images() {
    let mut h = Harness::new();
    let mut n = h.state.config.notifications.clone();
    n.review_backlog.on = true;
    n.review_backlog.threshold = 1;
    n.player = "true {file}".into();
    h.send(Msg::SetNotifications(n));
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    assert_eq!(plays(&h.send(Msg::Tick(10.0))), 0);
    {
        let lib = Library_::steal(&h.root);
        lib.write_two(&project);
    }
    h.rt.run(Effect::Rescan);
    h.settle();
    assert_eq!(plays(&h.send(Msg::Tick(20.0))), 1, "two over one");
    assert_eq!(h.state.alerts.backlog_text.as_deref(), Some("2 images waiting for review"));
    h.settle();
    assert_eq!(h.state.alerts.sound_error, None, "played with `true`");
    h.send(Msg::ReviewCurrent(Verdict::Good));
    h.settle();
    h.send(Msg::Tick(30.0));
    assert!(h.state.alerts.backlog_text.is_none(), "one isn't more than one");
}

#[test]
fn preview_steps_through_review_and_the_output_library() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    {
        let lib = Library_::steal(&h.root);
        lib.write_two(&project);
    }
    h.rt.run(Effect::Rescan);
    h.settle();

    // Review: the queue, and the cursor follows
    h.send(Msg::Tab(Tab::Review));
    let queue: Vec<increart_core::ImageRef> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    assert_eq!(queue.len(), 2);
    h.send(Msg::Preview(Some(queue[0].clone())));
    assert_eq!(h.state.preview_position(), Some((0, 2)));
    h.send(Msg::PreviewStep(-1));
    assert_eq!(h.state.preview.as_ref(), Some(&queue[0]), "stops at the first");
    h.send(Msg::PreviewStep(1));
    assert_eq!(h.state.preview.as_ref(), Some(&queue[1]));
    assert_eq!(h.state.review_cursor, 1);
    h.send(Msg::PreviewStep(1));
    assert_eq!(h.state.preview.as_ref(), Some(&queue[1]), "stops at the last");
    h.send(Msg::Preview(None));

    // the Output library's grid: its images, and the selection follows
    for _ in 0..2 {
        h.send(Msg::ReviewCurrent(Verdict::Good));
        h.settle();
    }
    h.send(Msg::Tab(Tab::OutputLibrary));
    let shown: Vec<increart_core::ImageRef> = h.state.output_source_images().iter().map(|o| o.image.clone()).collect();
    assert_eq!(shown.len(), 2);
    h.send(Msg::Preview(Some(shown[1].clone())));
    h.send(Msg::PreviewStep(-1));
    assert_eq!(h.state.preview.as_ref(), Some(&shown[0]));
    assert_eq!(h.state.output_selected.as_ref(), Some(&shown[0]));

    // elsewhere (or an image not in the list): nothing to step through
    h.send(Msg::Tab(Tab::Projects));
    assert_eq!(h.state.preview_position(), None);
    h.send(Msg::PreviewStep(1));
    assert_eq!(h.state.preview.as_ref(), Some(&shown[0]));
}

#[test]
fn queue_pressed_twice_before_the_save_lands_saves_once() {
    use increart_core::prompts::SweepVary;
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    h.state.no_server_warned = true;

    // a run: the second Queue comes before the first is saved (a double-click)
    let mut f = h.state.generate.clone();
    f.prompt = "a fox".into();
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    assert!(h.send(Msg::QueueGenerate).is_empty(), "not saved again");
    assert!(h.state.messages.iter().any(|m| m.text.contains("r00 is being queued already")));
    h.settle();
    assert_eq!(h.state.project_entry().unwrap().runs.len(), 1);
    assert!(h.state.queueing.is_empty());

    // a sweep: the same, then the next label queues as usual
    let mut f = h.state.generate.clone();
    f.sweep.on = true;
    f.sweep.vary = SweepVary::Cfg;
    f.sweep.values = "1, 2".into();
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    assert!(h.send(Msg::QueueGenerate).is_empty(), "not saved again");
    h.settle();
    assert!(h.errors().is_empty(), "{:?}", h.errors());
    assert_eq!(h.state.project_entry().unwrap().runs.len(), 3, "the run and the sweep's two");
    assert_eq!(h.state.generate.label, "r02");
    h.send(Msg::QueueGenerate);
    h.settle();
    assert_eq!(h.state.project_entry().unwrap().runs.len(), 5, "queued again under its new label");
}

#[test]
fn setting_sweeps_vary_cfg_steps_or_the_model() {
    use increart_core::prompts::SweepVary;
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let label = h.state.generate.label.clone();

    // CFG: the values in order, without repeats; the rest as the form has it
    let mut f = h.state.generate.clone();
    f.prompt = "a fox".into();
    f.sweep.on = true;
    f.sweep.vary = SweepVary::Cfg;
    f.sweep.values = "3, 1;2 2".into();
    f.seeds = 2;
    f.steps = 12;
    h.send(Msg::Generate(f.clone()));
    let (runs, _) = update::sweep_runs(&h.state).unwrap();
    let got: Vec<_> = runs.iter().map(|(f, t)| (f.label.clone(), f.cfg, f.steps, t.row())).collect();
    assert_eq!(
        got,
        [
            (format!("{label} · cfg 1"), 1.0, 12, "cfg 1".to_string()),
            (format!("{label} · cfg 2"), 2.0, 12, "cfg 2".to_string()),
            (format!("{label} · cfg 3"), 3.0, 12, "cfg 3".to_string()),
        ]
    );

    // what's wrong is said
    let recipes = h.state.config.recipes.clone();
    let err = |f: &increart_gui_core::GenerateForm| update::param_runs(f, &recipes).unwrap_err();
    let mut bad = f.clone();
    bad.sweep.values = "2, x".into();
    assert!(err(&bad).contains("“x” isn't a number"));
    bad.sweep.values = "40".into();
    assert!(err(&bad).contains("CFG goes from 0 to 30"));
    bad.sweep.vary = SweepVary::Steps;
    bad.sweep.values = "8, 12.5".into();
    assert!(err(&bad).contains("whole numbers"));
    bad.sweep.vary = SweepVary::Strength;
    assert!(err(&bad).contains("needs a start image"));
    bad.sweep.vary = SweepVary::Steps;
    bad.sweep.values = "8".into();
    bad.prompt = "a fox, {preset}".into();
    assert!(err(&bad).contains("for style sweeps"));
    bad.prompt = "a {animal}".into();
    assert!(err(&bad).contains("Fill in {animal}"));
    bad.sweep.values = String::new();
    bad.prompt = "a fox".into();
    assert!(err(&bad).contains("Type the values"));

    // defaults around the form's own; a range fills the field
    assert_eq!(update::sweep_defaults(SweepVary::Steps, &f), "6, 9, 12, 18, 24");
    let mut two = f.clone();
    two.cfg = 2.0;
    assert_eq!(update::sweep_defaults(SweepVary::Cfg, &two), "1, 1.5, 2, 3, 4");
    assert_eq!(update::range_values(0.3, 0.9, 0.15), "0.3, 0.45, 0.6, 0.75, 0.9");

    // queued: a run each, tagged with its value; the sheet's rows say it
    h.send(Msg::QueueGenerate);
    h.settle();
    let p = h.state.project_entry().unwrap();
    assert_eq!(p.runs.len(), 3, "{:?}", h.errors());
    let mut runs: Vec<_> = p.runs.clone();
    runs.sort_by(|a, b| a.id.cmp(&b.id));
    assert!(runs.iter().all(|r| r.sweep.as_ref().unwrap().name == label));
    assert_eq!(runs.iter().map(|r| r.sample.cfg).collect::<Vec<_>>(), [1.0, 2.0, 3.0]);
    assert_eq!(increart_gui_core::settings_line(&runs[1]), "12 steps · cfg 2");
    let project = p.project.id.clone();
    let seeds: Vec<u64> = runs[0].all_seeds().map(|(_, s)| s).collect();
    for r in &runs {
        Library_::steal(&h.root).write_seeds(&r.id, &seeds);
    }
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::Tab(Tab::Review));
    h.send(Msg::ReviewSweep(project, label.clone()));
    let sh = h.state.sheet().unwrap();
    let rows: Vec<_> = sh.rows.iter().map(|r| r.run.sweep.as_ref().unwrap().row()).collect();
    assert_eq!(rows, ["cfg 1", "cfg 2", "cfg 3"]);
    // tile captions: the row, the row and seed, the settings
    let img = match &sh.rows[2].cells[1] {
        SheetCell::Image(o) => o.image.clone(),
        c => panic!("{c:?}"),
    };
    use increart_gui_core::CaptionFill;
    assert_eq!(h.state.caption(&img, CaptionFill::Style), "cfg 3");
    assert_eq!(h.state.caption(&img, CaptionFill::StyleSeed), format!("cfg 3 · seed {}", seeds[1]));
    assert_eq!(h.state.caption(&img, CaptionFill::Settings), "12 steps · cfg 3");

    // Model: each model preset with its own steps and CFG
    let mut m = f.clone();
    m.sweep.vary = SweepVary::Model;
    assert!(err(&m).contains("Tick the models"));
    m.sweep.models = recipes.iter().map(|r| r.name.clone()).collect();
    let runs = update::param_runs(&m, &recipes).unwrap();
    assert_eq!(runs.len(), recipes.len());
    for ((r, t), p) in runs.iter().zip(&recipes) {
        assert_eq!(
            (r.preset, r.steps, r.cfg, t.row()),
            (recipes.iter().position(|x| x.name == p.name).unwrap(), p.steps, p.cfg, p.name.clone())
        );
    }
}

#[test]
fn output_library_search_moves_between_images_and_project_names() {
    let mut h = Harness::new();
    let mut f = h.state.output_filter.clone();
    f.text = "fox".into();
    h.send(Msg::OutputFilter(f));
    // to project names: what's typed goes along, the images' filter clears
    h.send(Msg::OutputSearchProjects(true));
    assert!(h.state.output_search_projects);
    assert_eq!(h.state.output_project_search, "fox");
    assert!(h.state.output_filter.text.is_empty());
    h.send(Msg::OutputProjectSearch("cafe".into()));
    h.send(Msg::OutputSearchProjects(true));
    assert_eq!(h.state.output_project_search, "cafe", "choosing it again changes nothing");
    h.send(Msg::OutputSearchProjects(false));
    assert_eq!(h.state.output_filter.text, "cafe");
    assert!(h.state.output_project_search.is_empty());
}

#[test]
fn rating_in_the_preview() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    {
        let lib = Library_::steal(&h.root);
        lib.write_two(&project);
    }
    h.rt.run(Effect::Rescan);
    h.settle();

    // Review's queue: the rated image leaves it, the preview goes on
    h.send(Msg::Tab(Tab::Review));
    let queue: Vec<increart_core::ImageRef> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    h.send(Msg::Preview(Some(queue[0].clone())));
    h.send(Msg::PreviewRate(Verdict::Good));
    assert_eq!(h.state.preview.as_ref(), Some(&queue[1]));
    h.settle();
    assert_eq!(h.state.preview_position(), Some((0, 1)));
    assert_eq!(h.state.review_cursor, 0);
    // the last one closes it; undo brings it back to the queue
    h.send(Msg::PreviewRate(Verdict::Bad));
    assert_eq!(h.state.preview, None);
    h.settle();
    assert!(h.state.review_queue().is_empty());
    h.send(Msg::UndoReview);
    h.settle();
    assert_eq!(h.state.review_queue().len(), 1);

    // the Output library: re-rating keeps the image shown
    h.send(Msg::Tab(Tab::OutputLibrary));
    let shown = h.state.output_source_images().first().map(|o| o.image.clone()).unwrap();
    h.send(Msg::Preview(Some(shown.clone())));
    h.send(Msg::PreviewRate(Verdict::Neutral));
    h.settle();
    assert_eq!(h.state.preview.as_ref(), Some(&shown));
    let o = h.state.index.projects.iter().flat_map(|p| &p.outputs).find(|o| o.image == shown).unwrap();
    assert_eq!(o.verdict, Some(Verdict::Neutral));
}

#[test]
fn selecting_a_project_narrows_the_grid_only_to_a_folder_with_others() {
    let mut h = Harness::new();
    let from_ideas = |h: &mut Harness, name: &str, folder: Option<&str>| {
        h.send(Msg::Draft(IdeaDraft { title: name.into(), text: "x".into(), ..Default::default() }));
        h.send(Msg::AddIdea(AddIdea::NewProject));
        h.send(Msg::FromIdeasParent(PathBuf::new()));
        h.send(Msg::FromIdeasNewFolder(folder.map(String::from)));
        h.send(Msg::ConfirmFromIdeas);
        h.settle();
        h.state.project.clone().unwrap()
    };
    let fox = from_ideas(&mut h, "Fox", Some("Fox"));
    from_ideas(&mut h, "Owl", Some("Owl"));
    let deer = from_ideas(&mut h, "deer tail", Some("Deer"));
    from_ideas(&mut h, "deer river", Some("deer"));
    let top = from_ideas(&mut h, "Top", None);
    let scope = |h: &mut Harness, id: &increart_core::Id| {
        h.send(Msg::SelectProject(id.clone()));
        (h.state.project_scope(), h.state.projects_in_scope().len())
    };
    // with others in its folder it narrows; alone, the grid stays
    h.send(Msg::SelectProjectFolder(Some(PathBuf::new())));
    assert_eq!(scope(&mut h, &fox), (PathBuf::new(), 5), "alone in Fox/: the grid stays");
    assert_eq!(scope(&mut h, &deer), (PathBuf::from("Deer"), 2), "Deer/ has two");
    assert_eq!(scope(&mut h, &top), (PathBuf::new(), 5), "at the top level, with the rest");
    // alone and outside what the grid shows: the folder above its own
    h.send(Msg::SelectProjectFolder(Some("Deer".into())));
    assert_eq!(scope(&mut h, &fox), (PathBuf::new(), 5));
    h.send(Msg::SelectProjectFolder(Some("Fox".into())));
    assert_eq!(h.state.projects_in_scope().len(), 1, "a chosen folder is kept");

    // unticked: a project never narrows it
    h.send(Msg::NarrowToSiblings(false));
    assert!(h.state.config.project_click_keeps_scope);
    h.send(Msg::SelectProjectFolder(Some(PathBuf::new())));
    assert_eq!(scope(&mut h, &deer), (PathBuf::new(), 5));
    h.send(Msg::NarrowToSiblings(true));

    // newest created first, or A–Z (any case), a config setting
    h.send(Msg::SelectProjectFolder(Some(PathBuf::new())));
    let names = |h: &Harness| h.state.projects_in_scope().iter().map(|p| p.name()).collect::<Vec<_>>();
    assert_eq!(names(&h), ["Top", "deer river", "deer tail", "Owl", "Fox"]);
    h.send(Msg::ProjectOrder(TreeOrder::Name));
    assert_eq!(h.state.config.project_order, TreeOrder::Name);
    assert_eq!(names(&h), ["deer river", "deer tail", "Fox", "Owl", "Top"]);

    // folders A–Z, or newest created first by the newest project in them
    let folders = |h: &Harness| {
        h.state.project_folder_children(Path::new("")).iter().map(|f| f.display().to_string()).collect::<Vec<_>>()
    };
    assert_eq!(folders(&h), ["Deer", "Fox", "Owl"]);
    h.send(Msg::FolderOrder(TreeOrder::Created));
    assert_eq!(h.state.config.folder_order.0, TreeOrder::Created);
    assert_eq!(folders(&h), ["Deer", "Owl", "Fox"]);

    // or last run first (never run: by when made), projects and folders alike
    let owl = h.state.index.projects.iter().find(|p| p.name() == "Owl").unwrap().project.id.clone();
    h.send(Msg::SelectProject(owl));
    h.send(Msg::Generate(h.state.generate.clone()));
    h.send(Msg::QueueGenerate);
    h.settle();
    assert!(h.errors().is_empty(), "{:?}", h.errors());
    // queued after the others were made (timestamps are whole seconds)
    let p = h.state.index.projects.iter_mut().find(|p| p.name() == "Owl").unwrap();
    p.runs[0].revisions[0].created += chrono::TimeDelta::seconds(1);
    h.send(Msg::SelectProjectFolder(Some(PathBuf::new())));
    h.send(Msg::ProjectOrder(TreeOrder::LastRun));
    assert_eq!(names(&h), ["Owl", "Top", "deer river", "deer tail", "Fox"]);
    h.send(Msg::FolderOrder(TreeOrder::LastRun));
    assert_eq!(folders(&h), ["Owl", "Deer", "Fox"]);
}

#[test]
fn new_project_from_ideas_in_a_new_or_existing_folder() {
    let mut h = Harness::new();
    h.send(Msg::Draft(IdeaDraft { title: "Fox".into(), text: "a red fox".into(), ..Default::default() }));
    h.send(Msg::AddIdea(AddIdea::NewProject));
    // step 2: a new folder at the top
    h.send(Msg::EditFromIdeasName("Fox cafe".into()));
    h.send(Msg::FromIdeasNewFolder(Some("Fox cafe".into())));
    let d = h.state.from_ideas.clone().unwrap();
    assert_eq!(from_ideas_folder(&h.state, &d), (PathBuf::from("Fox cafe"), false));
    h.send(Msg::ConfirmFromIdeas);
    h.settle();
    assert_eq!(h.state.project_entry().unwrap().dir, Path::new("projects/Fox cafe/Fox cafe"));
    assert!(h.state.index.project_folders.contains(&PathBuf::from("Fox cafe")));

    // the next one starts next to a project's own folder, not in it; in a
    // folder chosen otherwise, where the last went; without a new folder, if
    // unticked
    let fox = h.state.library_ideas()[0].idea.id.clone();
    h.send(Msg::NewProjectFromIdeas(vec![fox.clone()]));
    assert_eq!(h.state.from_ideas.as_ref().unwrap().parent, Path::new(""), "next to the last one's folder");
    h.send(Msg::EditFromIdeasName("Fox bar".into()));
    h.send(Msg::FromIdeasNewFolder(Some("bars".into())));
    h.send(Msg::EditFromIdeasName("Fox pub".into()));
    assert_eq!(
        h.state.from_ideas.as_ref().unwrap().new_folder.as_deref(),
        Some("bars"),
        "a folder named otherwise stays"
    );
    h.send(Msg::ConfirmFromIdeas);
    h.settle();
    assert_eq!(h.state.project_entry().unwrap().dir, Path::new("projects/bars/Fox pub"));
    h.send(Msg::NewProjectFromIdeas(vec![fox.clone()]));
    assert_eq!(h.state.from_ideas.as_ref().unwrap().parent, Path::new("bars"), "where the last one went");
    h.send(Msg::FromIdeasNewFolder(None));
    h.send(Msg::EditFromIdeasName("Fox inn".into()));
    h.send(Msg::ConfirmFromIdeas);
    h.settle();
    assert_eq!(h.state.project_entry().unwrap().dir, Path::new("projects/bars/Fox inn"), "no new folder");

    // a "new" folder that exists (any case) is used; a name taken there is refused
    h.send(Msg::NewProjectFromIdeas(vec![fox]));
    h.send(Msg::FromIdeasParent(PathBuf::new()));
    h.send(Msg::FromIdeasNewFolder(Some("fox cafe".into())));
    h.send(Msg::EditFromIdeasName("Fox cafe".into()));
    let d = h.state.from_ideas.clone().unwrap();
    assert_eq!(from_ideas_folder(&h.state, &d), (PathBuf::from("Fox cafe"), true));
    h.send(Msg::ConfirmFromIdeas);
    assert!(h.state.from_ideas.is_some() && !h.errors().is_empty(), "the name is taken in that folder");
    h.send(Msg::EditFromIdeasName("Fox night".into()));
    h.send(Msg::ConfirmFromIdeas);
    h.settle();
    let p = h.state.project_entry().unwrap();
    assert_eq!(p.dir, Path::new("projects/Fox cafe/Fox night"));
    assert_eq!(h.state.index.project_folders.len(), 2);
}

#[test]
fn project_folders_collapse_and_are_remembered() {
    let mut h = Harness::new();
    h.send(Msg::NewFolder(increart_core::Tree::Projects, PathBuf::new()));
    h.settle();
    let a = h.state.index.project_folders[0].clone();
    h.send(Msg::NewFolder(increart_core::Tree::Projects, a.clone()));
    h.settle();
    let b = h.state.index.project_folders.iter().find(|f| **f != a).unwrap().clone();
    assert!(!h.state.folder_collapsed(&a));

    h.send(Msg::ToggleProjectFolder(a.clone()));
    // remembered per library in the config
    assert_eq!(h.state.config.collapsed_folders.values().collect::<Vec<_>>(), [&vec![a.clone()]]);
    assert!(h.state.folder_collapsed(&a) && !h.state.folder_collapsed(&b));
    h.send(Msg::ToggleProjectFolder(a.clone()));
    assert!(!h.state.folder_collapsed(&a));
    assert!(h.state.config.collapsed_folders.is_empty(), "nothing collapsed, nothing saved");

    h.send(Msg::CollapseProjectFolders(true));
    assert!(h.state.folder_collapsed(&a) && h.state.folder_collapsed(&b));
    // a folder that's gone drops out on the next change
    h.state.index.project_folders.retain(|f| *f != b);
    h.send(Msg::ToggleProjectFolder(a.clone()));
    assert!(h.state.config.collapsed_folders.is_empty());
    h.send(Msg::CollapseProjectFolders(true));
    h.send(Msg::CollapseProjectFolders(false));
    assert!(!h.state.folder_collapsed(&a));
}

#[test]
fn interface_zoom_steps_and_stays_in_range() {
    let mut h = Harness::new();
    assert_eq!(h.state.ui_zoom(), 100);
    h.send(Msg::UiZoomStep(true));
    assert_eq!((h.state.ui_zoom(), h.state.config.ui_zoom), (105, 105));
    // a typed value steps to the next 5% either way
    h.send(Msg::UiZoom { percent: 123, save: true });
    h.send(Msg::UiZoomStep(true));
    assert_eq!(h.state.ui_zoom(), 125);
    h.send(Msg::UiZoom { percent: 123, save: true });
    h.send(Msg::UiZoomStep(false));
    assert_eq!(h.state.ui_zoom(), 120);
    // 70% to 300%
    h.send(Msg::UiZoom { percent: 500, save: true });
    assert_eq!(h.state.ui_zoom(), 300);
    h.send(Msg::UiZoomStep(true));
    assert_eq!(h.state.ui_zoom(), 300);
    h.send(Msg::UiZoom { percent: 70, save: true });
    h.send(Msg::UiZoomStep(false));
    assert_eq!(h.state.ui_zoom(), 70);
    // 100% isn't stored
    h.send(Msg::UiZoom { percent: 100, save: true });
    assert_eq!(h.state.config.ui_zoom, 0);
}

#[test]
fn reference_picker_browses_the_libraries_and_adds_the_chosen() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let imgs: Vec<_> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    h.send(Msg::ReviewLink(h.state.output_link(&imgs[0]).unwrap().path.clone(), Some(Verdict::Good)));
    h.settle();

    // no input images: it opens on the outputs, kept ones by default
    h.send(Msg::RefPicker(true));
    assert_eq!(h.state.ref_picker.as_ref().unwrap().source, RefSource::Outputs(OutputSource::AllProjects));
    let shown = |h: &Harness| h.state.ref_candidates().into_iter().map(|c| c.image).collect::<Vec<_>>();
    assert_eq!(shown(&h), [imgs[0].clone()]);
    h.send(Msg::RefPickerRatings(Ratings::ALL));
    assert_eq!(shown(&h).len(), 2);
    h.send(Msg::RefPickerText("no such prompt".into()));
    assert!(shown(&h).is_empty());
    h.send(Msg::RefPickerText(String::new()));

    // choose both, unchoose one, add
    h.send(Msg::RefPickerToggle(imgs[1].clone()));
    h.send(Msg::RefPickerToggle(imgs[0].clone()));
    h.send(Msg::RefPickerToggle(imgs[1].clone()));
    assert_eq!(h.state.ref_picker.as_ref().unwrap().chosen, [imgs[0].clone()]);
    h.send(Msg::RefPickerAdd);
    assert!(h.state.ref_picker.is_none());
    assert_eq!(h.state.generate.refs(), [imgs[0].clone()]);

    // an added image can't be chosen again; it reopens where it was
    h.send(Msg::RefPicker(true));
    h.send(Msg::RefPickerToggle(imgs[0].clone()));
    assert!(h.state.ref_picker.as_ref().unwrap().chosen.is_empty());
    h.send(Msg::RefPickerSource(RefSource::Inputs(None)));
    assert!(shown(&h).is_empty());
    h.send(Msg::RefPicker(false));
    h.send(Msg::RefPicker(true));
    assert_eq!(h.state.ref_picker.as_ref().unwrap().source, RefSource::Inputs(None));
    assert_eq!(h.state.generate.refs().len(), 1, "closing adds nothing");
}

#[test]
fn output_images_get_titles_found_by_search() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let imgs: Vec<_> = h.state.review_queue().iter().map(|o| o.image.clone()).collect();
    for img in &imgs {
        h.send(Msg::ReviewLink(h.state.output_link(img).unwrap().path.clone(), Some(Verdict::Good)));
        h.settle();
    }

    h.send(Msg::TitleDialog(Some(imgs[0].clone())));
    assert_eq!(h.state.title_edit.as_ref().unwrap().text, "");
    h.send(Msg::EditTitle("  Harbour   at dusk ".into()));
    h.send(Msg::CommitTitle);
    h.settle();
    assert!(h.state.title_edit.is_none());
    assert_eq!(h.state.image_title(&imgs[0]), Some("Harbour at dusk"));
    assert_eq!(h.state.image_title(&imgs[1]), None);
    // kept in project.rsn
    let rsn = std::fs::read_to_string(h.root.join(&h.state.project_entry().unwrap().dir).join("project.rsn")).unwrap();
    assert!(rsn.contains("Harbour at dusk"), "{rsn}");

    // the Output library's search finds it by title (any case)
    h.send(Msg::Tab(Tab::OutputLibrary));
    let mut f = h.state.output_filter.clone();
    f.text = "harbour".into();
    h.send(Msg::OutputFilter(f.clone()));
    let shown: Vec<_> = h.state.output_source_images().iter().map(|o| o.image.clone()).collect();
    assert_eq!(shown, [imgs[0].clone()]);

    // reopening shows it; emptying removes it
    h.send(Msg::TitleDialog(Some(imgs[0].clone())));
    assert_eq!(h.state.title_edit.as_ref().unwrap().text, "Harbour at dusk");
    h.send(Msg::EditTitle(" ".into()));
    h.send(Msg::CommitTitle);
    h.settle();
    assert_eq!(h.state.image_title(&imgs[0]), None);
    assert!(h.state.output_source_images().is_empty());
    assert!(h.errors().is_empty(), "{:?}", h.errors());
}

#[test]
fn runs_finish_and_stop_or_stop_now() {
    use increart_core::engine::{Event, Stop};
    let mut s = dialog_state(vec![], None);
    let t0 = chrono::Utc::now();
    let (a, b) = (increart_core::Id::new(t0, 1), increart_core::Id::new(t0, 2));
    update(&mut s, Msg::RunsQueued(vec![a.clone(), b.clone()]));
    assert_eq!(update(&mut s, Msg::CancelRun(a.clone(), Stop::Finish)), [Effect::CancelRun(a.clone(), Stop::Finish)]);
    assert!(s.finishing.contains(&a));
    // Stop now after Finish & stop: no longer just finishing
    update(&mut s, Msg::CancelRun(a.clone(), Stop::Now));
    assert!(!s.finishing.contains(&a));
    let fx = update(&mut s, Msg::StopAll(Stop::Finish));
    assert_eq!(fx, [Effect::CancelRun(a.clone(), Stop::Finish), Effect::CancelRun(b.clone(), Stop::Finish)]);
    update(&mut s, Msg::Engine(Event::RunDone { run: a.clone() }));
    assert!(!s.finishing.contains(&a) && s.finishing.contains(&b));
    update(&mut s, Msg::RunsQueued(vec![]));
    assert!(s.finishing.is_empty());
}

#[test]
fn status_messages_are_dismissed_one_by_one_or_cleared() {
    let mut h = Harness::new();
    for m in ["one", "two", "three"] {
        h.send(Msg::Info(m.into()));
    }
    let texts = |h: &Harness| h.state.messages.iter().map(|m| m.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts(&h), ["one", "two", "three"]);
    h.send(Msg::DismissMessage(1));
    assert_eq!(texts(&h), ["one", "three"]);
    h.send(Msg::ClearMessages);
    assert!(h.state.messages.is_empty());
}

#[test]
fn prompt_enhance_suggests_values_for_a_sweeps_blanks_looking_at_its_images() {
    use increart_core::pe::{PeRun, PeServer};
    use increart_core::prompts::{PresetKey, PromptPreset, PromptPresets, PromptTask};
    use std::sync::{Arc, Mutex};
    // a chat server that answers with two values (marble and ice, then
    // stone<n> and Marble), keeping what it was asked
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    let asked: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let seen = asked.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let answer = if req.url().ends_with("/v1/models") {
                r#"{"data": []}"#.to_string()
            } else {
                let mut body = String::new();
                req.as_reader().read_to_string(&mut body).unwrap();
                let mut seen = seen.lock().unwrap();
                seen.push(serde_json::from_str(&body).unwrap());
                let values = match seen.len() - 1 {
                    0 => serde_json::json!(["marble", "ice"]),
                    n => serde_json::json!([format!("stone{n}"), "Marble"]),
                };
                serde_json::json!({"choices": [{"message": {"content": format!("Here: {values}")}}]}).to_string()
            };
            let _ = req.respond(tiny_http::Response::from_string(answer));
        }
    });
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let img = h.state.review_queue()[0].image.clone();
    h.send(Msg::AddRefs(vec![img]));
    let preset =
        |name: &str, prompt: &str| PromptPreset { name: name.into(), prompt: prompt.into(), ..Default::default() };
    h.state.prompt_presets = PromptPresets {
        tasks: vec![PromptTask {
            name: "Becomes".into(),
            description: String::new(),
            presets: vec![
                preset("Sculpted", "<image1> sculpted from {material}"),
                preset("Melting", "<image1> melting, made of {material}, in the {era}"),
            ],
        }],
    };
    let mut f = h.state.generate.clone();
    f.prompt = "{preset}".into();
    f.sweep.on = true;
    h.send(Msg::Generate(f));
    h.send(Msg::SweepTicks(vec![(0, 0), (0, 1)], true));

    // no server set up: says where to add one
    let shared = BlankKey { style: None, slot: "material".into() };
    assert!(h.send(Msg::PeSuggest(vec![shared.clone()])).is_empty());
    assert!(h.errors().last().unwrap().contains("Servers…"), "{:?}", h.errors());

    // the shared {material}: one request naming both styles, with the image
    let fake = PeServer { name: "fake".into(), run: PeRun::Connect { url }, ..Default::default() };
    h.state.pe.server = Some(fake);
    h.send(Msg::PeSuggest(vec![shared.clone()]));
    assert!(h.state.pe.blank_job(&shared).is_some());
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !h.state.pe.jobs.is_empty() {
        assert!(std::time::Instant::now() < deadline, "no answer");
        for m in h.rt.wait(Duration::from_millis(100)) {
            h.send(m);
        }
    }
    assert_eq!(h.state.pe.status, PeStatus::Ready);
    assert_eq!(h.state.blank_shown(&shared).unwrap()[..], ["marble", "ice"]);
    let req = asked.lock().unwrap()[0].clone();
    let system = req["messages"][0]["content"].as_str().unwrap();
    assert_eq!(system, increart_core::pe::DEFAULT_FILL_BLANKS, "the built-in system prompt");
    let parts = req["messages"][1]["content"].as_array().unwrap();
    // the image first: asks for other blanks start the same
    let text = parts[1]["text"].as_str().unwrap();
    assert!(text.contains("all 2 of these styles") && text.contains("Suggest 6 values for {material}"), "{text}");
    assert!(parts[0]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"), "the image");
    assert!(text.starts_with("The reference image <image1> is attached.\n\nSubject (template): "), "{text}");
    assert!(!text.contains("Already suggested"), "{text}");
    // livelier than rewrites, kept sensible
    assert_eq!((req["temperature"].as_f64(), req["min_p"].as_f64()), (Some(1.1f32 as f64), Some(0.05f32 as f64)));

    // a style's own {era}, with the server's own system prompt
    h.send(Msg::PeSystemPrompt { job: "fill_blanks".into(), text: "mine".into() });
    let melting = PresetKey { task: "Becomes".into(), name: "Melting".into() };
    let own = BlankKey { style: Some(melting.clone()), slot: "era".into() };
    h.send(Msg::PeSuggest(vec![own.clone()]));
    while !h.state.pe.jobs.is_empty() {
        assert!(std::time::Instant::now() < deadline, "no answer");
        for m in h.rt.wait(Duration::from_millis(100)) {
            h.send(m);
        }
    }
    let req = asked.lock().unwrap()[1].clone();
    assert_eq!(req["messages"][0]["content"], "mine");

    // stopped while queued: never asked
    let sculpted = BlankKey {
        style: Some(PresetKey { task: "Becomes".into(), name: "Sculpted".into() }),
        slot: "material".into(),
    };
    h.send(Msg::PeSuggest(vec![shared.clone(), sculpted.clone()]));
    let id = h.state.pe.blank_job(&sculpted).unwrap().id;
    h.send(Msg::PeStop(vec![id]));
    assert!(h.state.pe.blank_job(&sculpted).is_none());
    while !h.state.pe.jobs.is_empty() {
        assert!(std::time::Instant::now() < deadline, "no answer");
        for m in h.rt.wait(Duration::from_millis(100)) {
            h.send(m);
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(asked.lock().unwrap().len(), 3, "the stopped one wasn't sent");
    assert!(h.state.blank_shown(&sculpted).is_none());
    let text = req["messages"][1]["content"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(text.contains("Style \"Melting\"") && !text.contains("Sculpted"), "{text}");
    let wait = |h: &mut Harness| {
        while !h.state.pe.jobs.is_empty() {
            assert!(std::time::Instant::now() < deadline, "no answer");
            for m in h.rt.wait(Duration::from_millis(100)) {
                h.send(m);
            }
        }
    };
    let text_of = |i: usize| {
        asked.lock().unwrap()[i]["messages"][1]["content"].as_array().unwrap().last().unwrap()["text"]
            .as_str()
            .unwrap()
            .to_string()
    };

    // asked again: told what was offered, and the new values added to them
    // (not Marble, offered before)
    let text = text_of(2);
    assert!(text.contains("not these or close variations of them): marble; ice\n"), "{text}");
    assert_eq!(h.state.blank_shown(&shared).unwrap()[..], ["marble", "ice", "stone2"]);

    // a reroll replaces them, avoiding every value offered before; one
    // offered again anyway is dropped
    h.send(Msg::PeReroll(shared.clone()));
    wait(&mut h);
    assert!(text_of(3).contains("): marble; ice; stone2\n"), "{}", text_of(3));
    assert_eq!(h.state.blank_shown(&shared).unwrap()[..], ["stone3"]);
    assert!(h.state.pe.rerolls.is_empty() && h.state.pe.avoiding.is_empty());

    // Suggest…: instructions, how many, sampling, no images, not avoiding
    h.send(Msg::PeSuggestOpen(shared.clone()));
    let mut d = h.state.pe.suggest.clone().unwrap();
    assert_eq!((d.form.count, d.form.images, d.form.avoid, d.form.replace), (6, true, true, false));
    d.form.how = "only stones".into();
    d.form.count = 3;
    d.form.sampling.temperature = 1.5;
    (d.form.images, d.form.avoid) = (false, false);
    h.send(Msg::PeSuggestDialog(Some(d.clone())));
    let preview = update::suggest_message(&h.state, &d.key, &d.form).unwrap();
    h.send(Msg::PeSuggestAsk);
    wait(&mut h);
    let req = asked.lock().unwrap()[4].clone();
    let text = text_of(4);
    assert_eq!(text, preview, "the dialog shows what's sent");
    assert!(text.ends_with("Instructions: only stones\n\nSuggest 3 values for {material}."), "{text}");
    assert!(!text.contains("Already suggested") && !text.contains("is attached"), "{text}");
    assert_eq!(req["messages"][1]["content"].as_array().unwrap().len(), 1, "no image");
    assert_eq!(req["temperature"].as_f64(), Some(1.5));
    // not avoiding: Marble is back
    assert_eq!(h.state.blank_shown(&shared).unwrap()[..], ["stone3", "stone4", "Marble"]);
    // the settings are the blank's own, as last set; another's are as by default
    h.send(Msg::PeSuggestDialog(None));
    h.send(Msg::PeSuggestOpen(shared.clone()));
    assert_eq!(h.state.pe.suggest.as_ref().unwrap().form, d.form);
    h.send(Msg::PeSuggestOpen(own.clone()));
    assert_eq!(h.state.pe.suggest.as_ref().unwrap().form.how, "");
    h.send(Msg::PeSuggestDialog(None));
    let ask = h.state.generate.sweep.blank_values(None, "material").unwrap().ask.clone().unwrap();
    let hot = increart_core::pe::Sampling { temperature: 1.5, ..h.state.config.pe.sampling("fill_blanks") };
    assert_eq!((ask.how.as_str(), ask.count, ask.sampling), ("only stones", 3, Some(hot)));
    // saved with the project: gone in another, back from disk in it again
    let shown = h.state.blank_shown(&shared).cloned().unwrap();
    let before = h.state.blank_offered(&shared).to_vec();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    assert_ne!(h.state.project.as_ref(), Some(&project));
    assert!(h.state.blank_shown(&shared).is_none() && h.state.pe.suggest.is_none());
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::SelectProject(project.clone()));
    assert_eq!(h.state.blank_shown(&shared), Some(&shown));
    assert_eq!(h.state.blank_offered(&shared), before);
    h.send(Msg::PeSuggestOpen(shared.clone()));
    assert_eq!(h.state.pe.suggest.as_ref().unwrap().form, d.form, "the settings saved too");
    h.send(Msg::PeSuggestDialog(None));

    // leaving the project while asks are waiting asks first: Cancel keeps
    // them; Yes stops them (a folder, another project alike)
    let other = h.state.index.projects.iter().find(|p| p.project.id != project).unwrap().project.id.clone();
    h.send(Msg::PeSuggest(vec![shared.clone()]));
    for leave in [Msg::SelectProjectFolder(None), Msg::SelectProject(other.clone())] {
        h.send(leave.clone());
        let text = &h.state.confirm.as_ref().unwrap().text;
        assert!(text.contains("1 ask(s)") && text.contains("Leaving the project stops them"), "{text}");
        h.send(Msg::ConfirmNo);
        assert_eq!(h.state.project.as_ref(), Some(&project), "{leave:?}");
        assert!(h.state.pe.blank_job(&shared).is_some());
    }
    h.send(Msg::SelectProject(other.clone()));
    h.send(Msg::ConfirmYes);
    assert_eq!(h.state.project.as_ref(), Some(&other));
    assert!(h.state.pe.jobs.is_empty() && h.state.confirm.is_none());
    // nothing waiting: no question
    h.send(Msg::SelectProject(project.clone()));
    assert!(h.state.confirm.is_none());
    assert_eq!(h.state.blank_shown(&shared), Some(&shown));
    // and kept when the form is cleared
    let form = h.state.generate.clone();
    h.send(Msg::ClearGenerate);
    assert_eq!(h.state.blank_shown(&shared), Some(&shown));
    h.send(Msg::Generate(form));
    h.send(Msg::PeSuggestOpen(shared.clone()));
    h.send(Msg::PeSuggestDialog(Some(d.clone())));
    // forgotten: asks may offer them again; Clear empties those shown
    h.send(Msg::PeForgetOffered(shared.clone()));
    h.send(Msg::PeClearSuggestions(shared.clone()));
    assert!(h.state.blank_offered(&shared).is_empty() && h.state.blank_shown(&shared).is_none());
    h.send(Msg::PeSuggestDialog(None));
    assert!(h.state.pe.suggest.is_none());

    // the server's own sampling for a job, and back to the default
    let x = increart_core::pe::Sampling { temperature: 0.7, min_p: 0.1 };
    h.send(Msg::PeSampling { job: "fill_blanks".into(), sampling: Some(x) });
    assert_eq!(h.state.config.pe.sampling("fill_blanks"), x);
    h.send(Msg::PeSampling { job: "fill_blanks".into(), sampling: None });
    assert!(h.state.config.pe.sampling.is_empty());

    // Suggest for blanks…: the empty ones ticked at first, then as last set;
    // one ask for each blank ticked, with the dialog's settings
    h.send(Msg::PeBatchSuggestOpen(vec!["material".into(), "era".into()]));
    let mut b = h.state.pe.batch_suggest.clone().unwrap();
    assert_eq!(b.blanks, ["material", "era"]);
    assert_eq!((b.form.how.as_str(), b.form.count, b.form.avoid), ("", 6, true));
    assert_eq!(b.form.sampling, h.state.config.pe.sampling("fill_blanks"), "the server's");
    b.blanks = vec!["era".into()];
    (b.form.how, b.form.count) = ("1920s only".into(), 2);
    h.send(Msg::PeBatchSuggestDialog(Some(b.clone())));
    let kept = h.state.generate.sweep.batch_ask.clone().unwrap();
    assert_eq!((kept.blanks, kept.ask.how, kept.ask.sampling), (vec!["era".to_string()], "1920s only".into(), None));
    h.send(Msg::PeBatchSuggestAsk);
    assert!(h.state.pe.batch_suggest.is_none(), "closed");
    let era = BlankKey { style: None, slot: "era".into() };
    assert!(h.state.pe.blank_job(&era).is_some() && h.state.pe.blank_job(&shared).is_none());
    assert_eq!(h.state.pe.jobs.len(), 1, "only the one ticked");
    wait(&mut h);
    let last = asked.lock().unwrap().len() - 1;
    let text = text_of(last);
    assert!(text.ends_with("Instructions: 1920s only\n\nSuggest 2 values for {era}."), "{text}");
    assert!(h.state.blank_shown(&era).is_some());
    // Tick only: fewer suggestions than asked for, no value, failed
    let names = ["material".to_string(), "era".into()];
    assert!(h.state.blank_shown(&shared).is_none() && h.state.blank_shown(&era).unwrap().len() < 6);
    assert_eq!(h.state.batch_picks(&names, 6)[0], names, "none and a few: both");
    assert_eq!(h.state.batch_picks(&names, 1)[0], ["material"], "only the one with none");
    let mut f = h.state.generate.clone();
    f.sweep.slots.insert("era".into(), "1920s".into());
    h.send(Msg::Generate(f));
    assert_eq!(h.state.batch_picks(&names, 6)[1], ["material"], "no value");
    assert!(h.state.batch_picks(&names, 6)[2].is_empty(), "none failed");
    let mut f = h.state.generate.clone();
    f.sweep.slots.remove("era");
    h.send(Msg::Generate(f));
    h.send(Msg::PeBatchSuggestOpen(vec![]));
    assert_eq!(h.state.pe.batch_suggest.as_ref().unwrap(), &b, "as last set");
    h.send(Msg::PeBatchSuggestDialog(None));
    // the quick ask (Basic suggest, ✨) keeps to the defaults:
    // neither the batch's settings nor the blank's own
    h.send(Msg::PeSuggest(vec![era.clone(), shared.clone()]));
    wait(&mut h);
    let reqs = asked.lock().unwrap().clone();
    for req in &reqs[reqs.len() - 2..] {
        let text = req["messages"][1]["content"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap();
        assert!(!text.contains("Instructions") && text.contains("Suggest 6 values for {"), "{text}");
        assert_eq!(req["temperature"].as_f64(), Some(h.state.config.pe.sampling("fill_blanks").temperature as f64));
    }

    // a shared value, and a style's own over it: Queue needs every style's
    // blanks filled one way or the other
    let mut f = h.state.generate.clone();
    f.sweep.slots.insert("material".into(), "marble".into());
    h.send(Msg::Generate(f));
    let err = update::sweep_runs(&h.state).unwrap_err();
    assert!(err.contains("{era}"), "{err}");
    let mut f = h.state.generate.clone();
    f.sweep.set_own_slot(&melting, "era", "1920s".into());
    f.sweep.set_own_slot(&melting, "material", "ice".into());
    h.send(Msg::Generate(f));
    let (runs, _) = update::sweep_runs(&h.state).unwrap();
    let prompts: Vec<_> = runs.iter().map(|(f, _)| f.prompt.as_str()).collect();
    assert_eq!(prompts, ["<image1> sculpted from marble", "<image1> melting, made of ice, in the 1920s"]);
    // clearing the last own value drops the style's entry
    let mut f = h.state.generate.clone();
    f.sweep.set_own_slot(&melting, "era", String::new());
    f.sweep.set_own_slot(&melting, "material", String::new());
    assert!(f.sweep.style_slots.is_empty());
}

#[test]
fn a_blank_whose_answer_has_nothing_new_is_asked_again_then_marked() {
    use increart_core::pe::{PeRun, PeServer};
    use increart_core::prompts::{PromptPreset, PromptPresets, PromptTask};
    use std::sync::{Arc, Mutex};
    // a chat server answering from a script, keeping what it was asked
    let script = [r#"["a", "b"]"#, r#"["a", "b"]"#, r#"["A", "b"]"#, r#"["a"]"#, "[]", r#"["c"]"#];
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    let asked: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let seen = asked.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let answer = if req.url().ends_with("/v1/models") {
                r#"{"data": []}"#.to_string()
            } else {
                let mut body = String::new();
                req.as_reader().read_to_string(&mut body).unwrap();
                let mut seen = seen.lock().unwrap();
                seen.push(serde_json::from_str(&body).unwrap());
                let values = script[(seen.len() - 1).min(script.len() - 1)];
                serde_json::json!({"choices": [{"message": {"content": format!("Here: {values}")}}]}).to_string()
            };
            let _ = req.respond(tiny_http::Response::from_string(answer));
        }
    });
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    h.state.prompt_presets = PromptPresets {
        tasks: vec![PromptTask {
            name: "Becomes".into(),
            description: String::new(),
            presets: vec![PromptPreset {
                name: "Sculpted".into(),
                prompt: "sculpted from {material}".into(),
                ..Default::default()
            }],
        }],
    };
    let mut f = h.state.generate.clone();
    f.prompt = "{preset}".into();
    f.sweep.on = true;
    h.send(Msg::Generate(f));
    h.send(Msg::SweepTicks(vec![(0, 0)], true));
    h.state.pe.server = Some(PeServer { name: "fake".into(), run: PeRun::Connect { url }, ..Default::default() });
    let key = BlankKey { style: None, slot: "material".into() };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let wait = |h: &mut Harness| {
        while !h.state.pe.jobs.is_empty() {
            assert!(std::time::Instant::now() < deadline, "no answer");
            for m in h.rt.wait(Duration::from_millis(100)) {
                h.send(m);
            }
        }
    };
    let req = |i: usize| asked.lock().unwrap()[i].clone();
    let text = |r: &serde_json::Value| {
        r["messages"][1]["content"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap().to_string()
    };
    h.send(Msg::PeSuggest(vec![key.clone()]));
    wait(&mut h);
    assert_eq!(h.state.blank_shown(&key).unwrap()[..], ["a", "b"]);

    // only repeats: asked again twice, each hotter and told so, then marked
    h.send(Msg::PeSuggest(vec![key.clone()]));
    wait(&mut h);
    assert_eq!(asked.lock().unwrap().len(), 4, "the first ask and 2 more");
    let temps: Vec<_> = (1..4).map(|i| (req(i)["temperature"].as_f64().unwrap() * 100.0).round()).collect();
    assert_eq!(temps, [110.0, 130.0, 150.0]);
    assert!(!text(&req(1)).contains("nothing new") && text(&req(3)).contains("nothing new"), "{}", text(&req(3)));
    assert_eq!(h.state.pe.blank_batch, 3, "the retries in the batch's count");
    let why = h.state.pe.blank_failed.get(&key).unwrap();
    assert!(why.contains("only repeated") && why.contains("asked 3 times"), "{why}");
    let errors = h.errors();
    assert_eq!(errors.iter().filter(|e| e.contains("{material}")).count(), 1, "one message, at the end: {errors:?}");
    assert!(h.state.pe.blank_tries.is_empty());

    // nothing at all, then a new value: the mark goes when asked again
    h.send(Msg::PeSuggest(vec![key.clone()]));
    assert!(h.state.pe.blank_failed.is_empty());
    wait(&mut h);
    assert_eq!(asked.lock().unwrap().len(), 6);
    assert_eq!(h.state.blank_shown(&key).unwrap()[..], ["a", "b", "c"]);
    assert!(h.state.pe.blank_failed.is_empty());
}

#[test]
fn prompt_enhance_rewrites_prompts_and_ideas_and_describes_images() {
    use increart_core::HistoryKey;
    use increart_core::pe::{DEFAULT_DESCRIBE, DEFAULT_REWRITE, PeRun, PeServer};
    use std::sync::{Arc, Mutex};
    // rewrites as a fox at golden hour; describes as a beach
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    let asked: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let seen = asked.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let answer = if req.url().ends_with("/v1/models") {
                r#"{"data": []}"#.to_string()
            } else {
                let mut body = String::new();
                req.as_reader().read_to_string(&mut body).unwrap();
                let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                let describe = v["messages"][0]["content"] == DEFAULT_DESCRIBE;
                seen.lock().unwrap().push(v);
                if !describe {
                    // streamed, in pieces
                    let mut sse = String::new();
                    for p in ["Rewritten prompt: \"a sleek red fox", " like <image1>, golden hour\""] {
                        let chunk = serde_json::json!({"choices": [{"delta": {"content": p}}]});
                        sse.push_str(&format!("data: {chunk}\n\n"));
                    }
                    sse.push_str("data: [DONE]\n\n");
                    let h = tiny_http::Header::from_bytes("Content-Type", "text/event-stream").unwrap();
                    let _ = req.respond(tiny_http::Response::from_string(sse).with_header(h));
                    continue;
                }
                r#"{"choices": [{"message": {"content": "A fox on a beach."}}]}"#.to_string()
            };
            let _ = req.respond(tiny_http::Response::from_string(answer));
        }
    });
    // waits for the window's answer, returning the steps it went through
    let wait = |h: &mut Harness| {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut steps = vec![];
        while h.state.pe.text.as_ref().is_some_and(|t| t.waiting.is_some()) {
            assert!(std::time::Instant::now() < deadline, "no answer");
            for m in h.rt.wait(Duration::from_millis(100)) {
                if let Msg::PeProgress(_, s) = &m {
                    steps.push(s.clone());
                }
                h.send(m);
            }
        }
        steps
    };
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let img = h.state.review_queue()[0].image.clone();
    h.send(Msg::AddRefs(vec![img.clone()]));
    let mut f = h.state.generate.clone();
    f.prompt = "a fox like <image1>".into();
    h.send(Msg::Generate(f));

    // no server set up
    assert!(h.send(Msg::PeRewrite(PeTarget::Prompt)).is_empty());
    assert!(h.state.pe.text.is_none() && h.errors().last().unwrap().contains("Servers…"));
    let fake = PeServer { name: "fake".into(), run: PeRun::Connect { url }, ..Default::default() };
    h.state.pe.server = Some(fake);

    // the Generate prompt, with its reference image
    h.send(Msg::PeRewrite(PeTarget::Prompt));
    let id = h.state.pe.text.as_ref().unwrap().waiting.unwrap();
    assert_eq!(h.state.pe.job(id).unwrap().step, PeStep::Queued);
    let steps = wait(&mut h);
    assert_eq!(steps[..2], [PeStep::Preparing, PeStep::Reading { images: 1 }]);
    assert!(
        matches!(&steps[2], PeStep::Writing { pieces: 1, text } if text == "Rewritten prompt: \"a sleek red fox"),
        "the answer as it's written: {steps:?}"
    );
    assert!(h.state.pe.jobs.is_empty() && h.state.pe.read_times.contains_key(&1));
    let t = h.state.pe.text.clone().unwrap();
    assert_eq!((t.text.as_str(), t.images), ("a sleek red fox like <image1>, golden hour", 1), "{:?}", t.error);
    let req = asked.lock().unwrap()[0].clone();
    assert_eq!(req["messages"][0]["content"], DEFAULT_REWRITE);
    let text = req["messages"][1]["content"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(text.starts_with("Rewrite this prompt:\n\na fox like <image1>") && text.contains("<image1> is attached"));
    assert!(req["messages"][1]["content"][0]["image_url"].is_object(), "the reference image, first");

    // again with instructions; an answer to the old ask is dropped
    h.send(Msg::PeTextEdit { how: "moodier".into(), text: "edited".into() });
    h.send(Msg::PeTextRetry);
    h.send(Msg::PeAnswer(id, Ok("stale".into())));
    assert!(h.state.pe.text.as_ref().unwrap().waiting.is_some());
    wait(&mut h);
    assert!(
        asked.lock().unwrap()[1]["messages"][1]["content"].as_array().unwrap().last().unwrap()["text"]
            .as_str()
            .unwrap()
            .ends_with("Instructions: moodier")
    );
    h.send(Msg::PeTextClose(PeUse::Use));
    assert!(h.state.pe.text.is_none());
    assert_eq!(h.state.generate.prompt, "a sleek red fox like <image1>, golden hour");
    h.settle();
    let e = history(&mut h, HistoryKey::Project(project));
    assert!(
        e.iter()
            .any(|e| e.kind == increart_core::history::HistoryKind::BeforeRewrite && e.text == "a fox like <image1>")
    );

    // an idea's rewrite kept as a variant, then used as its text
    h.send(Msg::Draft(IdeaDraft { title: "Fox".into(), text: "a fox".into(), ..Default::default() }));
    h.send(Msg::PeRewrite(PeTarget::NewIdea));
    wait(&mut h);
    h.send(Msg::PeTextClose(PeUse::Use));
    assert_eq!(h.state.draft.text, "a sleek red fox like <image1>, golden hour");
    h.send(Msg::AddIdea(AddIdea::Inbox));
    h.settle();
    let id = h.state.unreviewed_ideas()[0].idea.id.clone();
    h.send(Msg::OpenIdea(id.clone()));
    h.send(Msg::PeRewrite(PeTarget::Idea(id.clone())));
    wait(&mut h);
    h.send(Msg::PeTextClose(PeUse::Variant));
    h.settle();
    let i = &h.state.idea(&id).unwrap().idea;
    assert_eq!(i.variants.len(), 1);
    assert!(
        matches!(&i.variants[0].origin, increart_core::model::VariantOrigin::Rewritten { model, .. } if model == "fake")
    );
    h.send(Msg::PeRewrite(PeTarget::Idea(id.clone())));
    wait(&mut h);
    h.send(Msg::PeTextEdit { how: String::new(), text: "a fox, by hand".into() });
    h.send(Msg::PeTextClose(PeUse::Use));
    assert_eq!(h.state.idea_edit.as_ref().unwrap().text, "a fox, by hand", "the edited answer");

    // an image described becomes the Generate prompt
    h.send(Msg::PeRewrite(PeTarget::Describe(img)));
    wait(&mut h);
    let req = asked.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        req["messages"][1]["content"].as_array().unwrap().last().unwrap()["text"],
        "Describe the attached image."
    );
    h.send(Msg::PeTextClose(PeUse::Use));
    assert_eq!(h.state.generate.prompt, "A fox on a beach.");

    // Stop: the window keeps the text and says so; asking again stops the
    // ask before
    h.send(Msg::PeRewrite(PeTarget::Prompt));
    let id = h.state.pe.text.as_ref().unwrap().waiting.unwrap();
    assert_eq!(h.send(Msg::PeStop(vec![id])), [Effect::PeStop(vec![id])]);
    let t = h.state.pe.text.clone().unwrap();
    assert!(t.waiting.is_none() && t.error.as_deref() == Some("Stopped") && h.state.pe.jobs.is_empty());
    h.send(Msg::PeTextRetry);
    let again = h.state.pe.text.as_ref().unwrap().waiting.unwrap();
    assert!(h.send(Msg::PeTextRetry).contains(&Effect::PeStop(vec![again])));
    wait(&mut h);
    h.send(Msg::PeTextClose(PeUse::Discard));

    // discarding changes nothing
    h.send(Msg::PeRewrite(PeTarget::Prompt));
    wait(&mut h);
    h.send(Msg::PeTextClose(PeUse::Discard));
    assert_eq!(h.state.generate.prompt, "A fox on a beach.");
    assert!(h.errors().iter().all(|e| e.contains("Servers…")), "{:?}", h.errors());
}

#[test]
fn presets_window_shows_one_group_of_a_task() {
    let mut h = Harness::new();
    let mut f = h.state.generate.clone();
    f.prompt = "a fox".into();
    h.send(Msg::Generate(f));
    h.send(Msg::PresetsPanel(true));
    let g = h.state.prompt_presets.tasks[0].presets.iter().find(|p| !p.group.is_empty()).unwrap().group.clone();
    h.send(Msg::PresetsShow(PresetsView::Group(0, g.clone())));
    let view = &h.state.presets_panel.as_ref().unwrap().view;
    assert_eq!((view.task(), view.group()), (Some(0), Some(g.as_str())));

    // Save prompt as preset goes into the group shown
    h.send(Msg::OpenSavePreset);
    let d = h.state.save_preset.clone().unwrap();
    assert_eq!((d.task, d.preset.group.as_str()), (0, g.as_str()));

    // the group edited away: its task is shown
    h.send(Msg::OpenPresetEditor(0, None));
    let mut e = h.state.presets_editor.clone().unwrap();
    for p in e.list.tasks[0].presets.iter_mut().filter(|p| p.group == g) {
        p.group = "Renamed".into();
    }
    h.send(Msg::EditPresetEditor(e));
    h.send(Msg::SavePresetEditor);
    assert!(h.state.presets_editor.is_none());
    assert_eq!(h.state.presets_panel.as_ref().unwrap().view, PresetsView::Task(0));
}

#[test]
fn chat_servers_go_with_their_preset() {
    use increart_core::config::{full_binary, full_path};
    use increart_core::pe::{PeDevice, PeRun, PeServer, PeStart};
    // full already, as saving makes them (on Windows with a drive letter)
    let kobold = |port| PeRun::KoboldCpp {
        binary: full_binary("/bin/kcpp"),
        model: full_path("/m/a.gguf"),
        mmproj: String::new(),
        config: String::new(),
        host: "127.0.0.1".into(),
        port,
        device: PeDevice::Cpu { threads: 8 },
        context: 8192,
        args: vec![],
    };
    let a = PeServer { name: "A".into(), run: kobold(1), ..Default::default() };
    let b = PeServer {
        name: "B".into(),
        run: PeRun::Connect { url: "http://x:2".into() },
        start: PeStart::AtLaunch,
        ..Default::default()
    };
    let gpu0 = start("gpu0", 1234);
    let presets = vec![
        ServerPreset { name: "GPU + A".into(), servers: vec![gpu0.clone(), ServerConfig::Chat(a.clone())] },
        ServerPreset { name: "GPU + B".into(), servers: vec![ServerConfig::Chat(b.clone()), gpu0.clone()] },
        ServerPreset { name: "B only".into(), servers: vec![ServerConfig::Chat(b.clone())] },
    ];
    let mut s = dialog_state(presets.clone(), None);
    let asks = |fx: &[Effect]| fx.iter().filter(|f| matches!(f, Effect::PeAsk { jobs, .. } if jobs.is_empty())).count();
    let stops = |fx: &[Effect]| fx.iter().filter(|f| matches!(f, Effect::PeShutdown)).count();

    // A waits to be asked; it isn't an sd-server
    let fx = update(&mut s, Msg::OpenPreset(0));
    assert_eq!(asks(&fx), 0);
    assert!(matches!(fx.last(), Some(Effect::OpenServers { servers, .. }) if *servers == [gpu0.clone()]));
    assert_eq!(s.pe.server.as_ref(), Some(&a));
    assert_eq!(dialog(&s).progress.len(), 1);
    let at = s.open_attempt;
    update(&mut s, Msg::ServerOpened(at, 0, Ok(())));
    assert!(matches!(update(&mut s, Msg::ServersSettled(at))[..], [Effect::StartEngine { .. }]));
    assert_eq!(s.phase, Phase::Main);

    // the same sd-server with B: nothing to open or ask; A stops, B starts
    // with its preset
    update(&mut s, Msg::OpenServerDialog);
    s.pe.status = PeStatus::Ready;
    let fx = update(&mut s, Msg::OpenPreset(1));
    assert!(s.confirm.is_none(), "nothing stops");
    assert!(!fx.iter().any(|f| matches!(f, Effect::OpenServers { .. } | Effect::StopServers)), "{fx:?}");
    assert_eq!((asks(&fx), stops(&fx)), (1, 1));
    let i = fx.iter().position(|f| matches!(f, Effect::PeShutdown)).unwrap();
    let j = fx.iter().position(|f| matches!(f, Effect::PeAsk { servers, .. } if servers[0].name == "B")).unwrap();
    assert!(i < j, "started after the stop");
    assert_eq!((&s.phase, s.servers.len()), (&Phase::Main, 1));

    // B only: asks before stopping gpu0; B runs on as it is
    update(&mut s, Msg::OpenServerDialog);
    update(&mut s, Msg::OpenPreset(2));
    let text = s.confirm.as_ref().unwrap().text.clone();
    assert!(text.starts_with("Stop the servers in use and open B only?"), "{text}");
    let fx = update(&mut s, Msg::ConfirmYes);
    assert_eq!(fx.iter().filter(|f| **f == Effect::StopServers).count(), 1);
    assert_eq!(stops(&fx), 0);
    assert!(s.servers.is_empty() && s.phase == Phase::Main && s.pe.server.as_ref() == Some(&b));

    // continuing without sd-servers: the chat server in use stays, unless
    // another is picked
    let mut s = dialog_state(presets, None);
    update(&mut s, Msg::AskContinueWithoutServers);
    update(&mut s, Msg::WithoutDialog(Some(Without { chat: Some(b.clone()) })));
    let fx = update(&mut s, Msg::ContinueWithoutServers);
    assert_eq!(asks(&fx), 1, "B starts");
    assert_eq!((&s.phase, s.pe.server.as_ref()), (&Phase::Main, Some(&b)));
    s.pe.status = PeStatus::Ready;
    update(&mut s, Msg::OpenServerDialog);
    update(&mut s, Msg::AskContinueWithoutServers);
    assert_eq!(dialog(&s).without.as_ref().unwrap().chat.as_ref(), Some(&b), "the one in use, ticked");
    update(&mut s, Msg::WithoutDialog(Some(Without { chat: None })));
    assert_eq!(stops(&update(&mut s, Msg::ContinueWithoutServers)), 1);
    assert_eq!(s.pe.server, None);
}

#[test]
fn the_chat_server_editor_edits_the_presets_draft() {
    use increart_core::pe::{PeRun, PeServer};
    let a = PeServer { name: "A".into(), run: PeRun::Connect { url: "http://a:1".into() }, ..Default::default() };
    let p = ServerPreset { name: "GPU + A".into(), servers: vec![start("gpu0", 1234), ServerConfig::Chat(a.clone())] };
    let mut s = dialog_state(vec![p, ServerPreset { name: "GPU".into(), servers: vec![start("gpu0", 1234)] }], None);
    // a new one starts like the presets' first; a preset has one
    update(&mut s, Msg::EditPreset(1));
    let fx = update(&mut s, Msg::PeEditorOpen(None));
    assert!(fx.is_empty(), "connected to: no GPUs to list");
    assert_eq!(s.pe.editor.as_ref().unwrap().draft, a);
    // on GPUs: listed by its runtime, and again for another
    let gpus = |runtime| PeServer {
        run: PeRun::KoboldCpp {
            binary: "k".into(),
            model: "m".into(),
            mmproj: String::new(),
            config: String::new(),
            host: "127.0.0.1".into(),
            port: 5001,
            device: increart_core::pe::PeDevice::Gpu { devices: vec![], runtime },
            context: 0,
            args: vec![],
        },
        ..a.clone()
    };
    let fx = update(&mut s, Msg::PeEditorEdit(gpus(GpuRuntime::Vulkan)));
    assert_eq!(fx, [Effect::PeListGpus(GpuRuntime::Vulkan)]);
    update(&mut s, Msg::PeGpus(GpuRuntime::Vulkan, Ok(vec![(1, "R9700".into())])));
    assert!(update(&mut s, Msg::PeEditorEdit(gpus(GpuRuntime::Vulkan))).is_empty(), "listed already");
    let fx = update(&mut s, Msg::PeEditorEdit(gpus(GpuRuntime::Rocm)));
    assert_eq!(fx, [Effect::PeListGpus(GpuRuntime::Rocm)]);
    update(&mut s, Msg::PeGpus(GpuRuntime::Vulkan, Ok(vec![])));
    assert!(s.pe.gpus.is_none(), "a stale list is dropped");
    update(&mut s, Msg::PeEditorEdit(a.clone()));
    let mut d = a.clone();
    (d.name, d.run) = (" C ".into(), PeRun::Connect { url: String::new() });
    update(&mut s, Msg::PeEditorEdit(d.clone()));
    update(&mut s, Msg::PeEditorSave);
    assert!(s.pe.editor.as_ref().unwrap().error.as_deref().unwrap().contains("address"));
    d.run = PeRun::Connect { url: "http://c:3".into() };
    update(&mut s, Msg::PeEditorEdit(d.clone()));
    assert!(update(&mut s, Msg::PeEditorSave).is_empty(), "saved with the preset, not before");
    assert!(s.pe.editor.is_none());
    assert_eq!(dialog(&s).draft.chat().unwrap().name, "C");
    assert!(s.config.server_presets[1].chat().is_none());
    // another: added, the asks shared among them
    update(&mut s, Msg::PeEditorOpen(None));
    update(&mut s, Msg::PeEditorSave);
    assert_eq!(dialog(&s).draft.servers.len(), 3, "added");
    assert_eq!(dialog(&s).draft.servers[2], ServerConfig::Chat(PeServer { name: "A 2".into(), ..a.clone() }));
    update(&mut s, Msg::RemoveServer(1));
    update(&mut s, Msg::PeEditorOpen(Some(1)));
    update(&mut s, Msg::PeEditorEdit(a.clone()));
    update(&mut s, Msg::PeEditorSave);
    assert_eq!(dialog(&s).draft.chat(), Some(&a));
    update(&mut s, Msg::SavePreset);
    assert_eq!(s.config.server_presets[1].chat(), Some(&a));
    // Settings… on it; files picked go into it
    update(&mut s, Msg::EditPreset(0));
    update(&mut s, Msg::PeEditorOpen(Some(1)));
    assert_eq!(s.pe.editor.as_ref().map(|e| (e.index, e.draft.name.as_str())), Some((Some(1), "A")));
    let mut d = s.pe.editor.as_ref().unwrap().draft.clone();
    d.run = PeRun::KoboldCpp {
        binary: "/bin/k".into(),
        model: "/m/a.gguf".into(),
        mmproj: String::new(),
        config: String::new(),
        host: "127.0.0.1".into(),
        port: 5001,
        device: Default::default(),
        context: 0,
        args: vec![],
    };
    update(&mut s, Msg::PeEditorEdit(d));
    update(&mut s, Msg::Picked(PickKind::Pe(PeFile::Mmproj), vec!["/m/mm.gguf".into()]));
    update(&mut s, Msg::PeEditorSave);
    let chat = dialog(&s).draft.chat().unwrap().clone();
    assert!(
        matches!(&chat.run, PeRun::KoboldCpp { mmproj, .. } if *mmproj == increart_core::config::full_path("/m/mm.gguf"))
    );
    assert_eq!(dialog(&s).draft.servers.len(), 2);
    // a settings file stands in for the model
    update(&mut s, Msg::PeEditorOpen(Some(1)));
    let mut d = s.pe.editor.as_ref().unwrap().draft.clone();
    if let PeRun::KoboldCpp { model, .. } = &mut d.run {
        model.clear();
    }
    update(&mut s, Msg::PeEditorEdit(d));
    update(&mut s, Msg::PeEditorSave);
    assert!(s.pe.editor.as_ref().unwrap().error.as_deref().unwrap().contains("a model or a settings file"));
    update(&mut s, Msg::Picked(PickKind::Pe(PeFile::Config), vec!["/m/q.kcpps".into()]));
    update(&mut s, Msg::PeEditorSave);
    assert!(s.pe.editor.is_none());
    let chat = dialog(&s).draft.chat().unwrap().clone();
    assert!(
        matches!(&chat.run, PeRun::KoboldCpp { model, config, .. } if model.is_empty() && *config == increart_core::config::full_path("/m/q.kcpps"))
    );
}

#[test]
fn reopening_keeps_the_sd_servers_still_wanted() {
    let gpu0 = start("gpu0", 1234);
    let gpu1 = start("gpu1", 1235).with_device("ROCm1");
    let presets = vec![
        ServerPreset { name: "One".into(), servers: vec![gpu0.clone()] },
        ServerPreset { name: "Both".into(), servers: vec![gpu0.clone(), gpu1.clone()] },
        ServerPreset { name: "Other".into(), servers: vec![gpu1.clone()] },
    ];
    let mut s = dialog_state(presets, None);
    update(&mut s, Msg::OpenPreset(0));
    let at = s.open_attempt;
    update(&mut s, Msg::ServerOpened(at, 0, Ok(())));
    update(&mut s, Msg::ServersSettled(at));
    // one more: nothing stops, so no question (the runtime keeps gpu0 as it is)
    update(&mut s, Msg::OpenServerDialog);
    let fx = update(&mut s, Msg::OpenPreset(1));
    assert!(s.confirm.is_none() && s.stopping.is_empty());
    assert!(matches!(fx.last(), Some(Effect::OpenServers { servers, .. }) if *servers == [gpu0.clone(), gpu1.clone()]));
    assert!(!fx.contains(&Effect::StopServers));
    let at = s.open_attempt;
    update(&mut s, Msg::ServerOpened(at, 0, Ok(())));
    update(&mut s, Msg::ServerOpened(at, 1, Ok(())));
    update(&mut s, Msg::ServersSettled(at));
    // the same again: nothing at all
    update(&mut s, Msg::OpenServerDialog);
    assert!(update(&mut s, Msg::OpenPreset(1)).iter().all(|f| matches!(f, Effect::SaveConfig(..))));
    assert_eq!(s.phase, Phase::Main);
    // one fewer: asks, naming the one that stops
    update(&mut s, Msg::OpenServerDialog);
    update(&mut s, Msg::OpenPreset(2));
    let text = s.confirm.as_ref().unwrap().text.clone();
    assert!(text.starts_with("Stop ROCm0 and open Other? ROCm1 stay"), "{text}");
    update(&mut s, Msg::ConfirmYes);
    assert_eq!(s.stopping, ["ROCm0"]);
    let at = s.open_attempt;
    update(&mut s, Msg::ServerOpened(at, 0, Ok(())));
    update(&mut s, Msg::ServersSettled(at));
    // the sd-server settings changed: started ones start again
    s.config.programs.sd_server.args = vec!["--x".into()];
    update(&mut s, Msg::OpenServerDialog);
    update(&mut s, Msg::OpenPreset(2));
    assert!(s.confirm.as_ref().unwrap().text.starts_with("Stop the servers in use"));
    let fx = update(&mut s, Msg::ConfirmYes);
    assert!(fx.iter().any(|f| matches!(f, Effect::OpenServers { .. })));
}

#[test]
#[cfg(unix)] // the KoboldCpp that never answers is a shell script
fn pe_server_stopped_while_starting_is_off_not_failed() {
    use increart_core::pe::{PeDevice, PeRun, PeServer};
    use std::os::unix::fs::PermissionsExt;
    let mut h = Harness::new();
    // a KoboldCpp that never answers
    let bin = h.root.join("kcpp.sh");
    std::fs::write(&bin, "#!/bin/sh\nexec sleep 60\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    h.state.pe.server = Some(PeServer {
        name: "slow".into(),
        run: PeRun::KoboldCpp {
            binary: bin.to_string_lossy().into(),
            model: "m.gguf".into(),
            mmproj: String::new(),
            config: String::new(),
            host: "127.0.0.1".into(),
            port,
            device: PeDevice::Gpu { devices: vec![], runtime: Default::default() },
            context: 0,
            args: vec![],
        },
        ..Default::default()
    });
    let wait = |h: &mut Harness, want: PeStatus| {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while h.state.pe.status != want {
            assert!(std::time::Instant::now() < deadline, "still {:?}", h.state.pe.status);
            for m in h.rt.wait(Duration::from_millis(100)) {
                h.send(m);
            }
        }
    };
    h.send(Msg::PeStart);
    wait(&mut h, PeStatus::Starting);
    h.send(Msg::PeShutdown);
    wait(&mut h, PeStatus::Off);
    // and it stays off
    for m in h.rt.wait(Duration::from_millis(600)) {
        h.send(m);
    }
    assert_eq!(h.state.pe.status, PeStatus::Off);
    assert!(h.errors().is_empty(), "{:?}", h.errors());
}

#[test]
fn prompt_enhance_improves_each_styles_prompt() {
    use increart_core::pe::{PeRun, PeServer};
    use increart_core::prompts::{EnhanceAsk, PresetKey, PromptPreset, PromptPresets, PromptTask, StylePrompt};
    use increart_gui_core::update::{StyleRow, enhance_picks, style_row, styles_to_enhance, sweep_runs};
    use std::sync::{Arc, Mutex};
    // a chat server answering "Improved prompt: <n>. <the style's name>",
    // keeping what it was asked
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    let asked: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let seen = asked.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let answer = if req.url().ends_with("/v1/models") {
                r#"{"data": []}"#.to_string()
            } else {
                let mut body = String::new();
                req.as_reader().read_to_string(&mut body).unwrap();
                let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                let text = v["messages"][1]["content"].as_array().unwrap().last().unwrap()["text"]
                    .as_str()
                    .unwrap()
                    .to_string();
                let style = text.split('"').nth(1).unwrap_or("?").to_string();
                let mut seen = seen.lock().unwrap();
                seen.push(v);
                // asked to echo: the prompt given back as it was sent
                let content = match text.contains("Instructions: echo") {
                    true => text.split("Improve this prompt:\n\n").nth(1).unwrap().split("\n\n").next().unwrap().into(),
                    false => format!("Improved prompt: {}. {style} in glass", seen.len()),
                };
                serde_json::json!({"choices": [{"message": {"content": content}}]}).to_string()
            };
            let _ = req.respond(tiny_http::Response::from_string(answer));
        }
    });
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    let img = h.state.review_queue()[0].image.clone();
    h.send(Msg::AddRefs(vec![img]));
    let preset =
        |name: &str, prompt: &str| PromptPreset { name: name.into(), prompt: prompt.into(), ..Default::default() };
    h.state.prompt_presets = PromptPresets {
        tasks: vec![PromptTask {
            name: "Becomes".into(),
            description: String::new(),
            presets: vec![
                preset("Sculpted", "<image1> sculpted from {material}"),
                preset("Plain", "<image1> as a sketch"),
            ],
        }],
    };
    let key = |name: &str| PresetKey { task: "Becomes".into(), name: name.into() };
    let mut f = h.state.generate.clone();
    f.prompt = "a cat, {preset}".into();
    f.sweep.on = true;
    h.send(Msg::Generate(f));
    h.send(Msg::SweepTicks(vec![(0, 0), (0, 1)], true));
    let fake = PeServer { name: "fake".into(), run: PeRun::Connect { url }, ..Default::default() };
    h.state.pe.server = Some(fake);
    let wait = |h: &mut Harness| {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !h.state.pe.jobs.is_empty() {
            assert!(std::time::Instant::now() < deadline, "no answer");
            for m in h.rt.wait(Duration::from_millis(100)) {
                h.send(m);
            }
        }
    };

    // both styles as built: both asked, the blank left for PE to fill
    // asked from Enhance…: both ticked at first; its settings kept with the
    // project, and it closes
    assert_eq!(styles_to_enhance(&h.state).len(), 2);
    h.send(Msg::PeStyleOpen);
    let a = h.state.pe.style_dialog.clone().unwrap();
    assert_eq!(a.styles, [key("Sculpted"), key("Plain")]);
    assert_eq!(enhance_picks(&h.state), [a.styles.clone(), vec![]]);
    let moody = EnhanceAsk { how: "moody".into(), ..a };
    h.send(Msg::PeStyleDialog(Some(moody.clone())));
    assert_eq!(h.state.generate.sweep.enhance_ask.as_ref(), Some(&moody));
    h.send(Msg::PeStyleAsk);
    assert!(h.state.pe.style_dialog.is_none());
    assert_eq!(h.state.pe.styles_waiting(), 2);
    wait(&mut h);
    assert!(h.errors().is_empty(), "{:?}", h.errors());
    let req = asked.lock().unwrap()[0].clone();
    assert_eq!(req["messages"][0]["content"].as_str().unwrap(), increart_core::pe::DEFAULT_STYLE_PROMPT);
    let parts = req["messages"][1]["content"].as_array().unwrap();
    assert!(parts[0]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"), "the image first");
    let text = parts.last().unwrap()["text"].as_str().unwrap();
    assert_eq!(
        text,
        "The reference image <image1> is attached.\n\nTemplate (the subject): a cat, {preset}\n\n\
         Style \"Sculpted\": <image1> sculpted from {material}\n\n\
         Improve this prompt:\n\na cat, <image1> sculpted from {material}\n\n\
         Blanks to fill: {material}\n\nInstructions: moody"
    );
    let sp = h.state.generate.sweep.style_prompt(&key("Sculpted")).unwrap().clone();
    assert!(sp.enhanced && sp.prompt.ends_with("Sculpted in glass"), "{sp:?}");
    assert_eq!(sp.from, "a cat, <image1> sculpted from {material}");
    assert_eq!(
        style_row(&h.state, &key("Sculpted"), "<image1> sculpted from {material}"),
        StyleRow::Enhanced { stale: false }
    );
    assert!(styles_to_enhance(&h.state).is_empty(), "all enhanced");

    // the runs take them (the blank PE filled needs no value), marked
    let (runs, _) = sweep_runs(&h.state).unwrap();
    assert!(runs.iter().all(|(r, t)| t.enhanced && r.prompt.ends_with("in glass")));

    // a value given after: made from older values, asked again; an edit
    // by hand isn't (unless asked for every style)
    let mut f = h.state.generate.clone();
    f.sweep.slots.insert("material".into(), "wax".into());
    let plain = f.sweep.style_prompt(&key("Plain")).unwrap().clone();
    f.sweep.set_style_prompt(&key("Plain"), Some(StylePrompt { prompt: "my own".into(), enhanced: false, ..plain }));
    h.send(Msg::Generate(f));
    assert_eq!(
        style_row(&h.state, &key("Sculpted"), "<image1> sculpted from {material}"),
        StyleRow::Enhanced { stale: true }
    );
    assert_eq!(styles_to_enhance(&h.state), [key("Sculpted")]);
    assert_eq!(enhance_picks(&h.state)[0], [key("Sculpted")]);
    // reopened as left, ticks and all
    h.send(Msg::PeStyleOpen);
    assert_eq!(h.state.pe.style_dialog.as_ref(), Some(&moody));

    // as shown: an edit is improved further
    let shown = EnhanceAsk { from_shown: true, styles: vec![key("Plain")], ..moody.clone() };
    h.send(Msg::PeStyleDialog(Some(shown)));
    h.send(Msg::PeStyleAsk);
    wait(&mut h);
    let text =
        asked.lock().unwrap().last().unwrap()["messages"][1]["content"].as_array().unwrap().last().unwrap()["text"]
            .as_str()
            .unwrap()
            .to_string();
    assert!(text.contains("Improve this prompt:\n\nmy own\n\n") && !text.contains("Blanks"), "{text}");

    // the quick ask (the bar's, or a row's ✨) takes the defaults, not Enhance…'s
    h.send(Msg::PeStyleDialog(None));
    h.send(Msg::PeStyleEnhance(vec![key("Plain")]));
    wait(&mut h);
    let last = asked.lock().unwrap().len() - 1;
    let text = asked.lock().unwrap()[last]["messages"][1]["content"].as_array().unwrap().last().unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!text.contains("Instructions") && !text.contains("my own"), "{text}");

    // the prompt given back unchanged: asked once more, hotter, then marked
    let before = h.state.generate.sweep.style_prompt(&key("Plain")).cloned();
    let n = asked.lock().unwrap().len();
    h.send(Msg::PeStyleDialog(Some(EnhanceAsk { how: "echo".into(), styles: vec![key("Plain")], ..moody })));
    h.send(Msg::PeStyleAsk);
    wait(&mut h);
    let reqs = asked.lock().unwrap()[n..].to_vec();
    assert_eq!(reqs.len(), 2, "asked again");
    assert!(reqs[1]["temperature"].as_f64() > reqs[0]["temperature"].as_f64());
    assert_eq!(h.state.generate.sweep.style_prompt(&key("Plain")).cloned(), before);
    assert_eq!(h.state.pe.style_failed[&key("Plain")], "gave the prompt back unchanged, asked 2 times");
    assert_eq!(enhance_picks(&h.state)[1], [key("Plain")]);
    assert!(h.errors().iter().any(|e| e.contains("unchanged for Plain")), "{:?}", h.errors());
    h.state.messages.clear();
    // asked again and answered: the mark goes
    h.send(Msg::PeStyleEnhance(vec![key("Plain")]));
    wait(&mut h);
    assert!(h.state.pe.style_failed.is_empty());

    // stopped: no answer lands
    h.send(Msg::PeStyleEnhance(vec![key("Sculpted")]));
    let ids: Vec<u64> = h.state.pe.jobs.iter().map(|j| j.id).collect();
    h.send(Msg::PeStop(ids));
    let before = h.state.generate.sweep.style_prompt(&key("Sculpted")).cloned();
    for m in h.rt.wait(Duration::from_millis(500)) {
        h.send(m);
    }
    assert_eq!(h.state.generate.sweep.style_prompt(&key("Sculpted")).cloned(), before);
    assert!(h.state.pe.style_from.is_empty());

    // saved with the project: gone in another, back in it again
    let kept = h.state.generate.sweep.style_prompts.clone();
    assert_eq!(kept.len(), 2);
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    assert!(h.state.generate.sweep.style_prompts.is_empty());
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::SelectProject(project.clone()));
    assert_eq!(h.state.generate.sweep.style_prompts, kept);
}

#[test]
fn styles_own_values_become_their_prompts() {
    use increart_core::prompts::{PresetKey, Sweep};
    let k = |n: &str| PresetKey { task: "T".into(), name: n.into() };
    let mut sw = Sweep::default();
    sw.slots.insert("era".into(), "1920s".into());
    sw.set_own_slot(&k("A"), "era", "1980s".into());
    sw.set_own_slot(&k("B"), "era", "1920s".into());
    assert!(sw.fold_style_slots("x, {preset}", &[(k("A"), "in the {era}"), (k("B"), "in the {era}"), (k("C"), "c")]));
    assert!(sw.style_slots.is_empty());
    // B's own value was the shared one: nothing to keep
    assert_eq!(sw.style_prompts.len(), 1);
    let a = sw.style_prompt(&k("A")).unwrap();
    assert_eq!((a.prompt.as_str(), a.from.as_str(), a.enhanced), ("x, in the 1980s", "x, in the 1920s", false));
    assert_eq!(sw.prompt_for("x, {preset}", &k("A"), "in the {era}"), "x, in the 1980s");
    assert!(!sw.fold_style_slots("x", &[]), "once");
}

#[test]
fn pe_asks_are_batched_by_kind_for_jobs() {
    use increart_core::prompts::PresetKey;
    use increart_gui_core::{BlankKey, PeBatchKind, PeJobState, PeKey, PeState, PeStep};
    let job = |id, key| PeJobState { id, key, step: PeStep::Queued, asked: 0.0, since: 0.0, rate: None, on: None };
    let blank = |s: &str| PeKey::Blank(BlankKey { style: None, slot: s.into() });
    let style = |s: &str| PeKey::Style(PresetKey { task: "Becomes".into(), name: s.into() });
    let mut pe = PeState {
        jobs: vec![job(3, style("A")), job(4, blank("x")), job(5, style("B")), job(6, PeKey::Text(6))],
        style_batch: 5,
        blank_batch: 1,
        ..Default::default()
    };
    // a text ask with no window open isn't shown
    let b = pe.batches();
    assert_eq!(b.len(), 2);
    assert_eq!((&b[0].kind, b[0].ids.clone(), b[0].total, b[0].at()), (&PeBatchKind::Styles, vec![3, 5], 5, 4));
    assert_eq!((&b[1].kind, b[1].ids.clone(), b[1].total, b[1].at()), (&PeBatchKind::Blanks, vec![4], 1, 1));
    pe.jobs.retain(|j| j.id == 4);
    pe.blank_batch = 0;
    assert_eq!(pe.batches()[0].total, 1);
}

#[test]
fn blanks_pe_added_are_asked_about() {
    use increart_core::prompts::PresetKey;
    let mut s = AppState::new(Config::default(), None);
    let key = |name: &str| PresetKey { task: "Becomes".into(), name: name.into() };
    for (id, name) in [(1, "Machine"), (2, "Plain")] {
        let step = PeStep::Queued;
        s.pe.jobs.push(PeJobState {
            id,
            key: PeKey::Style(key(name)),
            step,
            asked: 0.0,
            since: 0.0,
            rate: None,
            on: None,
        });
        s.pe.style_from.insert(key(name), format!("a {{era}} {name}"));
    }
    update(&mut s, Msg::PeAnswer(1, Ok("a {era} robot of {scrap_metal} on a {pole}".into())));
    assert!(!s.pe.added_blanks_ask, "asked once the batch ends");
    update(&mut s, Msg::PeAnswer(2, Ok("a {era} plain thing".into())));
    assert!(s.pe.added_blanks_ask);
    assert_eq!(s.pe.added_blanks, [(key("Machine"), vec!["scrap_metal".to_string(), "pole".into()])]);

    // made words; the blank it was given stays
    update(&mut s, Msg::PeAddedBlanks(false));
    assert!(!s.pe.added_blanks_ask && s.pe.added_blanks.is_empty());
    let own = s.generate.sweep.style_prompt(&key("Machine")).unwrap();
    assert_eq!(own.prompt, "a {era} robot of scrap metal on a pole");
    assert!(own.enhanced);
}

#[test]
fn a_writing_asks_rate_is_worked_out_at_most_twice_a_second() {
    use increart_core::prompts::PresetKey;
    let mut s = AppState::new(Config::default(), None);
    let key = PeKey::Style(PresetKey { task: "T".into(), name: "A".into() });
    s.pe.jobs.push(PeJobState { id: 1, key, step: PeStep::Queued, asked: 0.0, since: 0.0, rate: None, on: None });
    let token = |s: &mut AppState, now: f64, pieces: u32| {
        s.now = now;
        update(s, Msg::PeProgress(1, PeStep::Writing { pieces, text: String::new() }));
        s.pe.jobs[0].rate.map(|(_, r)| r)
    };
    assert_eq!(token(&mut s, 10.0, 1), None);
    assert_eq!(token(&mut s, 10.5, 5), None, "none in the first second");
    assert_eq!(token(&mut s, 11.0, 10), Some(10.0));
    assert_eq!(token(&mut s, 11.2, 14), Some(10.0), "kept");
    assert_eq!(token(&mut s, 11.4, 18), Some(10.0), "kept");
    assert_eq!(token(&mut s, 11.5, 24), Some(16.0), "half a second on");
}

#[test]
fn jobs_group_runs_by_project_and_sweep() {
    use increart_core::prompts::{PromptPreset, PromptPresets, PromptTask};
    use increart_gui_core::{RunItem, runs_progress};
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let mut f = h.state.generate.clone();
    f.prompt = "a fox".into();
    f.seeds = 2;
    h.send(Msg::Generate(f));
    h.send(Msg::QueueGenerate);
    h.settle();
    let preset = |name: &str| PromptPreset { name: name.into(), prompt: format!("as {name}"), ..Default::default() };
    h.state.prompt_presets = PromptPresets {
        tasks: vec![PromptTask {
            name: "Styles".into(),
            description: String::new(),
            presets: vec![preset("Ink"), preset("Clay")],
        }],
    };
    let mut f = h.state.generate.clone();
    f.prompt = "a cat, {preset}".into();
    f.sweep.on = true;
    h.send(Msg::Generate(f));
    h.send(Msg::SweepTicks(vec![(0, 0), (0, 1)], true));
    h.send(Msg::QueueGenerate);
    h.settle();
    let runs: Vec<_> = h.state.project_entry().unwrap().runs.iter().map(|r| r.id.clone()).collect();
    assert_eq!(runs.len(), 3);
    assert!(h.state.run_groups().is_empty(), "nothing in the engine");
    h.state.active_runs.extend(runs.iter().cloned());
    let groups = h.state.run_groups();
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    assert_eq!(g.runs().len(), 3);
    assert!(matches!(&g.items[0], RunItem::Run(r) if r.run == runs[0]));
    let RunItem::Sweep { runs: sw, .. } = &g.items[1] else { panic!("the sweep's runs together") };
    let rows: Vec<_> = sw.iter().map(|r| r.sweep.as_ref().unwrap().1.clone()).collect();
    assert_eq!(rows, ["Ink", "Clay"]);
    let (done, total, fraction, eta) = runs_progress(&g.runs(), 1);
    assert_eq!((done, total, fraction, eta), (0, 6, 0.0, None));
    // a group's stop: each of its runs
    h.send(Msg::CancelRuns(runs[1..].to_vec(), increart_core::engine::Stop::Finish));
    assert!(!h.state.finishing.contains(&runs[0]));
    assert!(runs[1..].iter().all(|r| h.state.finishing.contains(r)));
}

#[test]
fn picked_images_sweep_their_styles_again() {
    use increart_core::HistoryKey;
    use increart_core::prompts::{PresetKey, PromptPreset, PromptPresets, PromptTask};
    use increart_gui_core::NewSweepAsk;
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let preset =
        |name: &str, prompt: &str| PromptPreset { name: name.into(), prompt: prompt.into(), ..Default::default() };
    h.state.prompt_presets = PromptPresets {
        tasks: vec![PromptTask {
            name: "Looks".into(),
            description: String::new(),
            presets: vec![preset("Clay", "claymation"), preset("Ink", "ink in a {mood} mood")],
        }],
    };
    let mut f = h.state.generate.clone();
    f.prompt = "a fox, {preset}".into();
    f.seeds = 2;
    f.sweep.on = true;
    f.sweep.slots.insert("mood".into(), "gloomy".into());
    h.send(Msg::Generate(f));
    h.send(Msg::SweepTicks(vec![(0, 0), (0, 1)], true));
    h.send(Msg::QueueGenerate);
    h.settle();
    let p = h.state.project_entry().unwrap();
    let project = p.project.id.clone();
    let label = p.runs[0].sweep.as_ref().unwrap().name.clone();
    let mut ids: Vec<_> = p.runs.iter().map(|r| r.id.clone()).collect();
    ids.sort();
    let seeds: Vec<u64> = p.runs[0].all_seeds().map(|(_, s)| s).collect();
    for id in &ids {
        Library_::steal(&h.root).write_seeds(id, &seeds);
    }
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::ReviewScope(Some(project.clone())));
    h.send(Msg::ReviewSweep(project.clone(), label.clone()));
    // two of Ink's images picked: one style
    let ink = ids.iter().find(|id| h.state.index.run(id).unwrap().1.sweep.as_ref().unwrap().preset == "Ink").unwrap();
    let images: Vec<_> = h
        .state
        .project_entry()
        .unwrap()
        .outputs
        .iter()
        .filter(|o| &o.name.run == ink)
        .map(|o| o.image.clone())
        .collect();
    assert_eq!(images.len(), 2);
    h.send(Msg::PickSet(images));
    h.send(Msg::SheetNewSweep);
    h.settle();
    let g = &h.state.generate;
    assert_eq!((h.state.tab, h.state.projects_tab), (Tab::Projects, ProjectsTab::Generate));
    assert_eq!(g.prompt, "a fox, {preset}", "the template it was queued with");
    assert!(g.sweep.on);
    assert_eq!(g.sweep.selected, [PresetKey { task: "Looks".into(), name: "Ink".into() }]);
    assert_eq!(g.sweep.slots["mood"], "gloomy");
    assert_ne!(g.label, label, "the next label");
    assert!(g.first_seed > *seeds.last().unwrap());
    // without the history's checkpoint: the run's very prompt
    h.state.new_sweep = Some(NewSweepAsk { project: project.clone(), sweep: label.clone(), runs: vec![ink.clone()] });
    h.send(Msg::HistoryLoaded(HistoryKey::Project(project), vec![]));
    let g = &h.state.generate;
    assert_eq!(g.prompt, "{preset}");
    assert_eq!(g.sweep.style_prompts.len(), 1);
    assert_eq!(g.sweep.style_prompts[0].prompt, "a fox, ink in a gloomy mood");
    let last = h.state.messages.last().map(|m| m.text.clone()).unwrap_or_default();
    assert!(last.contains("wasn't in the prompt history"), "{last}");
}

#[test]
fn review_follow_moves_on_at_its_pace_until_moved_back() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    let run = Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::Tab(Tab::Review));
    let shown = |h: &Harness| h.state.review_queue()[h.state.review_cursor].name.seed;
    h.send(Msg::Tick(100.0));
    h.send(Msg::ReviewFollow(true));
    assert_eq!(shown(&h), 0, "starts on the image shown");
    // nothing made lately: 8 s an image
    h.send(Msg::Tick(105.0));
    assert_eq!(shown(&h), 0);
    let c = h.state.follow_countdown().unwrap();
    assert_eq!((c.left.round(), c.ahead), (3.0, 1));
    h.send(Msg::Tick(108.5));
    assert_eq!(shown(&h), 1);
    // rated just as it moved on: the image it moved from
    h.send(Msg::ReviewCurrent(Verdict::Good));
    h.settle();
    assert_eq!((h.state.review_queue().len(), shown(&h)), (1, 1));

    // caught up: waits on the newest; a new one comes, but paused it stays
    Library_::steal(&h.root).write_seeds(&run, &[2]);
    h.rt.run(Effect::Rescan);
    h.settle();
    assert_eq!(h.state.follow.arrivals.len(), 1);
    h.send(Msg::FollowPause(true));
    h.send(Msg::Tick(200.0));
    assert_eq!(shown(&h), 1);
    // and the paused time isn't counted
    h.send(Msg::FollowPause(false));
    h.send(Msg::Tick(204.0));
    assert_eq!(shown(&h), 1);
    // nor while away from Review
    h.send(Msg::Tab(Tab::Projects));
    h.send(Msg::Tick(300.0));
    h.send(Msg::Tab(Tab::Review));
    h.send(Msg::Tick(301.0));
    assert_eq!(shown(&h), 1);
    h.send(Msg::Tick(305.0));
    assert_eq!(shown(&h), 2);

    // → keeps following; ← (or any pick) stops it, and the cursor stays put
    h.send(Msg::ReviewNext(1));
    assert!(h.state.review_follow);
    h.send(Msg::ReviewNext(-1));
    assert!(!h.state.review_follow);
    h.send(Msg::Tick(400.0));
    assert_eq!(shown(&h), 1);
    // started from the newest, when set so
    h.send(Msg::FollowPace(increart_core::config::FollowPace { start_newest: true, ..Default::default() }));
    h.send(Msg::ReviewFollow(true));
    assert_eq!(shown(&h), 2);
}

#[test]
fn reviews_generate_button_opens_the_shown_images_project() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    let run = Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::Tab(Tab::Review));
    // all projects in scope: the image's
    assert_eq!(h.state.review_scope, None);
    let g = h.state.review_generate().unwrap();
    assert_eq!((&g.project, g.run.as_ref()), (&project, Some(&run)));
    assert_eq!(g.image.as_ref(), Some(&h.state.review_queue()[0].image));
    h.send(Msg::OpenInGenerate(g.project));
    assert_eq!((h.state.tab, h.state.projects_tab), (Tab::Projects, ProjectsTab::Generate));
    assert_eq!(h.state.project.as_ref(), Some(&project));
}

#[test]
fn generates_images_tile_in_their_order_with_the_start_marked() {
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::OpenInGenerate(project));
    h.send(Msg::TileGenerateImages);
    assert!(h.state.tile.is_none(), "no images: nothing to tile");
    let imgs: Vec<_> = h.state.index.projects[0].outputs.iter().map(|o| o.image.clone()).collect();
    h.send(Msg::AddRefs(imgs.clone()));
    h.send(Msg::ImageRoles { index: 1, reference: false, start: true });
    h.send(Msg::ImageRoles { index: 0, reference: false, start: false });
    assert!(!h.state.generate.images[0].reference && !h.state.generate.images[0].start, "parked");
    h.send(Msg::TileGenerateImages);
    let t = h.state.tile.as_ref().unwrap();
    assert_eq!(t.layout.images().cloned().collect::<Vec<_>>(), imgs, "the parked one too");
    assert!(h.state.picked.is_empty(), "the picks aren't touched");
    h.send(Msg::TileCaptionFormat("{kind}".into()));
    let t = h.state.tile.as_ref().unwrap();
    let kinds: Vec<_> = t.layout.tiles.iter().map(|x| h.state.caption_text(&x.caption, x.image(), 1)).collect();
    assert_eq!(kinds, ["output", "start"]);
}

#[test]
fn the_usage_check_is_shown_from_its_start_and_kept_once_done() {
    let mut s = AppState::new(Config::default(), None);
    update(&mut s, Msg::Tick(10.0));
    update(&mut s, Msg::UsageCheck(0, 3));
    assert_eq!((s.usage_check, s.usage_checked), (Some((0, 3)), None));
    update(&mut s, Msg::UsageCheck(3, 3));
    assert_eq!((s.usage_check, s.usage_checked), (None, Some((3, 10.0))));
}

#[test]
fn crop_and_mark_makes_a_hidden_copy_next_to_the_image_and_reopens_it() {
    use increart_core::model::{ImageEdits, Mark, Shape};
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    let project = h.state.project.clone().unwrap();
    Library_::steal(&h.root).write_two(&project);
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::OpenInGenerate(project));
    let imgs: Vec<_> = h.state.index.projects[0].outputs.iter().map(|o| o.image.clone()).collect();
    h.send(Msg::AddRefs(imgs.clone()));
    h.state.generate.prompt = "make <image1> blue, keep <image2>".into();

    // nothing drawn: nothing to save
    h.send(Msg::OpenMarkup(imgs[0].clone(), Some(0)));
    h.send(Msg::MarkupSave(false));
    assert!(h.state.markup.is_some() && h.state.markup.as_ref().unwrap().undo.is_empty());

    // a crop and a circle, one undo step each; undo, redo
    let (w, h_) = image::image_dimensions(imgs[0].path(&h.root)).unwrap();
    let circle = Mark { shape: Shape::Ellipse([1.0, 1.0], [6.0, 6.0]), color: [255, 0, 0, 255], width: 2.0 };
    let cropped = ImageEdits { crop: Some([0, 0, w / 2, h_ / 2]), marks: vec![] };
    let edits = ImageEdits { marks: vec![circle], ..cropped.clone() };
    h.send(Msg::MarkupEdit(cropped.clone()));
    h.send(Msg::MarkupEdit(edits.clone()));
    h.send(Msg::MarkupUndo);
    assert_eq!(h.state.markup.as_ref().unwrap().edits, cropped);
    h.send(Msg::MarkupRedo);
    assert_eq!(h.state.markup.as_ref().unwrap().edits, edits);
    h.send(Msg::MarkupSave(false));
    h.settle();
    assert!(h.state.markup.is_none(), "{:?}", h.errors());

    // the copy: below the image, a reference; the prompt follows the numbers
    let g = &h.state.generate.images;
    assert_eq!((g.len(), &g[0].image, &g[2].image), (3, &imgs[0], &imgs[1]));
    let copy = g[1].image.clone();
    assert!(g[1].reference && !g[1].start);
    assert_eq!(copy.store, increart_core::Store::Input);
    assert_eq!(image::image_dimensions(copy.path(&h.root)).unwrap(), (w / 2, h_ / 2));
    assert_eq!(h.state.retag.as_ref().unwrap().prompt, "make <image1> blue, keep <image3>");
    h.send(Msg::RetagConfirm);
    // hidden: in input/edited/, not in the library's lists unless asked for
    let e = h.state.index.ideas.iter().find(|e| e.idea.images.contains(&copy.id)).unwrap();
    assert_eq!(e.loc, IdeaLoc::Edited);
    assert!(h.state.library_ideas().is_empty());
    h.send(Msg::LibFilter(LibFilter { show_hidden: true, ..Default::default() }));
    assert_eq!(h.state.library_ideas().len(), 1);
    assert_eq!(h.state.edited_copies(&imgs[0]).len(), 1);
    h.send(Msg::RefPicker(true));
    h.send(Msg::RefPickerSource(RefSource::Edited));
    assert!(h.state.ref_candidates().iter().any(|c| c.image == copy));
    h.send(Msg::RefPicker(false));

    // opened again from Generate: the original, as drawn; Save replaces the copy
    h.send(Msg::OpenMarkup(copy.clone(), Some(1)));
    let m = h.state.markup.as_ref().unwrap();
    assert_eq!((&m.image, &m.edits, m.replace), (&imgs[0], &edits, true));
    h.send(Msg::MarkupEdit(ImageEdits { crop: None, ..edits.clone() }));
    h.send(Msg::MarkupSave(false));
    h.settle();
    let g = &h.state.generate.images;
    assert_eq!(g.len(), 3, "replaced, not added");
    assert_ne!(g[1].image, copy);
    assert!(h.state.retag.is_none(), "the same <image2>");
    assert_eq!(image::image_dimensions(g[1].image.path(&h.root)).unwrap(), (w, h_));
    assert_eq!(h.state.edited_copies(&imgs[0]).len(), 2, "the first copy stays");

    // from elsewhere: Save and use in Generate adds it at the end
    h.send(Msg::OpenMarkup(imgs[1].clone(), None));
    h.send(Msg::MarkupEdit(ImageEdits { crop: Some([0, 0, 8, 8]), marks: vec![] }));
    h.send(Msg::MarkupSave(true));
    h.settle();
    let g = &h.state.generate.images;
    assert_eq!(g.len(), 4);
    assert_eq!(h.state.edited_source(&g[3].image).map(|(i, _)| i), Some(&imgs[1]));
}

#[test]
fn queue_asks_about_references_no_prompt_names() {
    use increart_core::model::GenerateImage;
    use increart_core::prompts::{PresetKey, PromptPreset, PromptPresets, PromptTask};
    let mut s = AppState::new(Config::default(), None);
    let now = increart_core::id::now();
    let image = |i| GenerateImage {
        image: increart_core::ImageRef::input(increart_core::Id::new(now, i)),
        reference: true,
        start: false,
    };
    s.generate.images = vec![image(0), image(1), image(2)];
    s.generate.prompt = "<image1> in the style of <image3>".into();
    assert_eq!(update::unnamed_refs(&s), vec![2]);
    // the confirm comes first; Queue anyway goes on to the usual checks
    assert!(update(&mut s, Msg::QueueGenerate).is_empty());
    let c = s.confirm.as_ref().expect("asks");
    assert!(c.text.starts_with("<image2> isn't named"), "{}", c.text);
    assert_eq!(*c.then, Msg::QueueChecked);

    // named only by a style the sweep takes: not unnamed
    s.prompt_presets = PromptPresets {
        tasks: vec![PromptTask {
            name: "Becomes".into(),
            description: String::new(),
            presets: vec![PromptPreset {
                name: "Worn".into(),
                prompt: "<image1> wearing <image2>, {preset}".into(),
                ..Default::default()
            }],
        }],
    };
    s.generate.sweep.on = true;
    s.generate.sweep.selected = vec![PresetKey { task: "Becomes".into(), name: "Worn".into() }];
    assert!(update::unnamed_refs(&s).is_empty());
    s.generate.sweep.selected.clear();
    assert_eq!(update::unnamed_refs(&s), vec![2]);

    // one reference left unnamed is usual: no ask
    s.generate.images.truncate(1);
    s.generate.prompt = "a fox".into();
    assert!(update::unnamed_refs(&s).is_empty());
}

#[test]
fn prompt_enhance_shares_its_asks_among_its_servers() {
    use increart_core::pe::{PeRun, PeServer};
    use increart_gui_core::update::PeJob;
    // chat servers answering with their names, a little slowly
    let fake = |name: &'static str| {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let answer = if req.url().ends_with("/v1/models") {
                    r#"{"data": []}"#.to_string()
                } else {
                    std::thread::sleep(Duration::from_millis(300));
                    serde_json::json!({"choices": [{"message": {"content": name}}]}).to_string()
                };
                let _ = req.respond(tiny_http::Response::from_string(answer));
            }
        });
        PeServer { name: name.into(), run: PeRun::Connect { url }, ..Default::default() }
    };
    let servers = vec![fake("A"), fake("B")];
    let job = |id| PeJob {
        id,
        key: PeKey::Text(id),
        system: "s".into(),
        sampling: increart_core::pe::Sampling { temperature: 0.9, min_p: 0.0 },
        text: "t".into(),
        images: vec![],
        image_size: 0,
        max_tokens: 10,
    };
    let rt = Runtime::new(|| {}).unwrap();
    rt.run(Effect::PeAsk { servers: servers.clone(), jobs: (1..=4).map(job).collect() });
    let (mut on, mut answers, mut ready) = (vec![], vec![], 0);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while answers.len() < 4 {
        assert!(std::time::Instant::now() < deadline, "answered {answers:?}");
        for m in rt.wait(Duration::from_millis(100)) {
            match m {
                Msg::PeOn(id, name) => on.push((id, name)),
                Msg::PeAnswer(id, r) => answers.push((id, r.unwrap())),
                Msg::PeReady(n) => ready = ready.max(n),
                Msg::PeStatus(PeStatus::Failed(e)) => panic!("{e}"),
                _ => {}
            }
        }
    }
    assert_eq!(ready, 2, "both ready");
    for name in ["A", "B"] {
        assert!(answers.iter().any(|(_, a)| a == name), "{name} answered some: {answers:?}");
    }
    for (id, a) in &answers {
        assert!(on.contains(&(*id, a.clone())), "each answer from the server it was on: {on:?}");
    }
    // one dropped: the other answers alone
    rt.run(Effect::PeAsk { servers: servers[1..].to_vec(), jobs: vec![job(5)] });
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(std::time::Instant::now() < deadline, "no answer");
        if let Some(a) = rt.wait(Duration::from_millis(100)).into_iter().find_map(|m| match m {
            Msg::PeAnswer(5, r) => Some(r.unwrap()),
            _ => None,
        }) {
            assert_eq!(a, "B");
            break;
        }
    }
    rt.run(Effect::PeShutdown);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !rt.wait(Duration::from_millis(100)).iter().any(|m| matches!(m, Msg::PeStatus(PeStatus::Off))) {
        assert!(std::time::Instant::now() < deadline, "not off");
    }
}

#[test]
fn prompt_enhance_asks_stopped_are_never_answered() {
    use increart_core::pe::{PeRun, PeServer};
    use increart_gui_core::update::PeJob;
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let answer = if req.url().ends_with("/v1/models") {
                r#"{"data": []}"#.to_string()
            } else {
                std::thread::sleep(Duration::from_millis(400));
                serde_json::json!({"choices": [{"message": {"content": "ok"}}]}).to_string()
            };
            let _ = req.respond(tiny_http::Response::from_string(answer));
        }
    });
    let servers = vec![PeServer { name: "A".into(), run: PeRun::Connect { url }, ..Default::default() }];
    let job = |id| PeJob {
        id,
        key: PeKey::Text(id),
        system: "s".into(),
        sampling: increart_core::pe::Sampling { temperature: 0.9, min_p: 0.0 },
        text: "t".into(),
        images: vec![],
        image_size: 0,
        max_tokens: 10,
    };
    let rt = Runtime::new(|| {}).unwrap();
    // until `want` is answered: what was started and answered
    let until = |want: u64| {
        let (mut on, mut answered) = (vec![], vec![]);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !answered.contains(&want) {
            assert!(std::time::Instant::now() < deadline, "{want} not answered: {answered:?}");
            for m in rt.wait(Duration::from_millis(50)) {
                match m {
                    Msg::PeOn(id, _) => on.push(id),
                    Msg::PeAnswer(id, _) => answered.push(id),
                    _ => {}
                }
            }
        }
        (on, answered)
    };
    rt.run(Effect::PeAsk { servers: servers.clone(), jobs: (1..=3).map(job).collect() });
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !rt.wait(Duration::from_millis(50)).iter().any(|m| matches!(m, Msg::PeOn(1, _))) {
        assert!(std::time::Instant::now() < deadline, "1 not started");
    }
    // the one being answered and one queued
    rt.run(Effect::PeStop(vec![1, 2]));
    let (on, answered) = until(3);
    assert_eq!(answered, [3]);
    assert!(!on.contains(&2), "{on:?}");
    // one stopped before it comes
    rt.run(Effect::PeStop(vec![4]));
    rt.run(Effect::PeAsk { servers, jobs: vec![job(4), job(5)] });
    let (on, answered) = until(5);
    assert_eq!((on, answered), (vec![5], vec![5]));
}

#[test]
fn another_chat_server_gets_its_own_name_and_port() {
    use increart_core::pe::{PeRun, PeServer};
    let kcpp = |port| PeRun::KoboldCpp {
        binary: "k".into(),
        model: "m".into(),
        mmproj: String::new(),
        config: String::new(),
        host: "127.0.0.1".into(),
        port,
        device: Default::default(),
        context: 0,
        args: vec![],
    };
    let first = PeServer { name: "Chat".into(), run: kcpp(5001), ..Default::default() };
    let preset = ServerPreset { name: "P".into(), servers: vec![ServerConfig::Chat(first)] };
    let mut s = dialog_state(vec![preset], None);
    update(&mut s, Msg::EditPreset(0));
    update(&mut s, Msg::PeEditorOpen(None));
    let d = s.pe.editor.as_ref().unwrap().draft.clone();
    assert_eq!(d.name, "Chat 2");
    assert!(matches!(d.run, PeRun::KoboldCpp { port: 5002, .. }), "{:?}", d.run);
    update(&mut s, Msg::PeEditorSave);
    let Phase::Servers(dlg) = &s.phase else { panic!() };
    assert_eq!(dlg.draft.servers.iter().filter(|c| c.is_chat()).count(), 2, "added, not replacing");
    assert_eq!(dlg.draft.chat_pool().len(), 2);
}

#[test]
fn generate_offers_the_model_presets_the_servers_can_run() {
    use increart_core::config::RecipePreset;
    use increart_core::sdserver::Loadable;
    let preset = |name: &str, model: &str| RecipePreset {
        name: name.into(),
        recipe: "qwen-image-21".into(),
        files: [("diffusion_model".to_string(), model.to_string()), ("vae".to_string(), "vae.safetensors".to_string())]
            .into(),
        steps: 20,
        cfg: 2.0,
        flow_shift: None,
        custom_sigmas: vec![],
        loras: vec![],
    };
    let config = Config {
        recipes: vec![preset("Int8", "int8.safetensors"), preset("Q8", "koboldcpp/q8.gguf")],
        ..Default::default()
    };
    let mut s = AppState::new(config, None);
    let server = |name: &str| ServerStatus {
        config: None,
        name: name.into(),
        device: String::new(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    };
    s.servers = vec![server("gpu0"), server("gpu1")];
    // nothing said yet: all of them
    assert_eq!(s.runnable_recipes(), vec![0, 1]);

    // gpu0 has the Q8 files: chosen, as Int8 is on no server
    let files = Loadable::files(["koboldcpp/q8.gguf", "vae.safetensors"].map(String::from));
    update(&mut s, Msg::ServerModels(vec![("gpu0".into(), files)]));
    assert_eq!(s.runnable_recipes(), vec![1]);
    assert_eq!(s.generate.preset, 1);

    // a KoboldCpp started elsewhere runs one diffusion model, matched by name
    update(&mut s, Msg::ServerModels(vec![("gpu1".into(), Loadable::Only(Some("int8.safetensors".into())))]));
    assert_eq!(s.runnable_recipes(), vec![0, 1]);
    assert!(s.models_without_preset().is_empty());
    update(&mut s, Msg::ServerModels(vec![("gpu1".into(), Loadable::Only(Some("other.gguf".into())))]));
    assert_eq!(s.models_without_preset(), vec![("gpu1", "other.gguf")]);

    // a file a server has but can't load says why, not that it isn't listed
    let refusing = Loadable::Files {
        ids: Default::default(),
        refused: [("int8.safetensors".to_string(), "is in sd-server's int8 format".to_string())].into(),
    };
    update(&mut s, Msg::ServerModels(vec![("gpu1".into(), refusing)]));
    assert_eq!(s.why_file_unlisted("int8.safetensors"), "gpu1: is in sd-server's int8 format");
    assert!(s.why_file_unlisted("none.gguf").starts_with("Not in the model directories"));

    // only the open servers count
    s.servers.retain(|x| x.name == "gpu1");
    assert!(s.runnable_recipes().is_empty());
}

#[test]
fn a_model_preset_file_picked_is_named_inside_its_models_folder() {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models");
    let mut config = Config::default();
    config.programs.sd_server.model_dirs = vec![models.to_string_lossy().into()];
    config.recipes = increart_core::config::builtin_recipes();
    let mut s = AppState::new(config, None);
    update(&mut s, Msg::OpenRecipes);
    let vae = increart_core::config::MODEL_SLOTS.iter().position(|n| *n == "vae").unwrap();
    // starts in the model directory (the built-in's vae isn't there)
    let fx = update(&mut s, Msg::Pick(PickKind::RecipeFile(vae)));
    assert!(matches!(&fx[..], [Effect::Pick { start: Some(p), .. }] if *p == models), "{fx:?}");
    update(&mut s, Msg::Picked(PickKind::RecipeFile(vae), vec![models.join("vae").join("mine.safetensors")]));
    let e = s.recipe_editor.clone().unwrap();
    assert_eq!(e.rows[e.selected].preset.files["vae"], "vae/mine.safetensors");
    assert!(e.error.is_none());
    // outside it: said, the file kept
    update(&mut s, Msg::Picked(PickKind::RecipeFile(vae), vec![dir.path().join("other.safetensors")]));
    let e = s.recipe_editor.clone().unwrap();
    assert_eq!(e.rows[e.selected].preset.files["vae"], "vae/mine.safetensors");
    assert!(e.error.unwrap().contains("isn't in a model directory"));
}

#[test]
fn a_model_presets_lora_picked_is_named_inside_the_loras_folder() {
    use increart_core::config::PresetLora;
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models");
    std::fs::create_dir_all(models.join("loras")).unwrap();
    let mut config = Config::default();
    config.programs.sd_server.model_dirs = vec![models.to_string_lossy().into()];
    config.recipes = increart_core::config::builtin_recipes();
    config.recipes[0].loras = vec![PresetLora { file: String::new(), weight: 1.0 }];
    let mut s = AppState::new(config, None);
    update(&mut s, Msg::OpenRecipes);
    // starts in the loras folder
    let fx = update(&mut s, Msg::Pick(PickKind::RecipeLora(0)));
    assert!(matches!(&fx[..], [Effect::Pick { start: Some(p), .. }] if *p == models.join("loras")), "{fx:?}");
    update(&mut s, Msg::Picked(PickKind::RecipeLora(0), vec![models.join("loras").join("style.safetensors")]));
    let e = s.recipe_editor.clone().unwrap();
    assert_eq!(e.rows[e.selected].preset.loras[0].file, "loras/style.safetensors");
    // a model file isn't a LoRA: said, the LoRA kept
    update(&mut s, Msg::Picked(PickKind::RecipeLora(0), vec![models.join("vae").join("v.safetensors")]));
    let e = s.recipe_editor.clone().unwrap();
    assert_eq!(e.rows[e.selected].preset.loras[0].file, "loras/style.safetensors");
    assert!(e.error.unwrap().contains("isn't in the loras folder"));
}

#[test]
fn model_presets_edit_and_generate_keeps_its_model() {
    use increart_core::config::RecipePreset;
    let preset = |name: &str, steps: u32| RecipePreset {
        name: name.into(),
        recipe: "qwen-image-21".into(),
        files: [("diffusion_model".to_string(), format!("{name}.gguf"))].into(),
        steps,
        cfg: 2.0,
        flow_shift: None,
        custom_sigmas: vec![],
        loras: vec![],
    };
    let config = Config { recipes: vec![preset("A", 20), preset("B", 30)], ..Default::default() };
    let mut s = AppState::new(config, None);
    update(&mut s, Msg::SelectRecipe(1));
    s.generate.steps = 25; // tweaked in the form
    s.generate.sweep.models = vec!["A".into(), "B".into()];

    // B moves up and A is renamed: the form keeps B and its own steps
    update(&mut s, Msg::OpenRecipes);
    let mut e = s.recipe_editor.clone().unwrap();
    assert_eq!(e.selected, 1);
    e.rows.swap(0, 1);
    e.rows[1].preset.name = "A2".into();
    update(&mut s, Msg::EditRecipes(e));
    update(&mut s, Msg::SaveRecipes);
    assert!(s.recipe_editor.is_none());
    let names: Vec<_> = s.config.recipes.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["B", "A2"]);
    assert_eq!((s.generate.preset, s.generate.steps), (0, 25));
    assert_eq!(s.generate.sweep.models, ["A2", "B"]);

    // B's steps change: the form takes them; sigmas are parsed
    update(&mut s, Msg::OpenRecipes);
    let mut e = s.recipe_editor.clone().unwrap();
    e.rows[0].preset.steps = 8;
    e.rows[0].sigmas = "1, 0.5 0".into();
    update(&mut s, Msg::EditRecipes(e));
    update(&mut s, Msg::SaveRecipes);
    assert_eq!(s.generate.steps, 8);
    assert_eq!(s.config.recipes[0].custom_sigmas, [1.0, 0.5, 0.0]);

    // a clash, a missing model or bad sigmas keeps the editor open
    update(&mut s, Msg::OpenRecipes);
    let mut e = s.recipe_editor.clone().unwrap();
    e.rows[1].preset.name = "B ".into();
    assert_eq!(e.problem().as_deref(), Some("Two model presets are called B"));
    e.rows[1].preset.name = "C".into();
    e.rows[1].preset.files.clear();
    assert_eq!(e.problem().as_deref(), Some("C has no diffusion model file"));
    e.rows[1].preset.files.insert("model".into(), "c.gguf".into());
    e.rows[1].sigmas = "1, x".into();
    update(&mut s, Msg::EditRecipes(e));
    update(&mut s, Msg::SaveRecipes);
    assert!(s.recipe_editor.is_some());
    assert_eq!(s.config.recipes[1].name, "A2");
    update(&mut s, Msg::CloseRecipes);
    assert!(s.recipe_editor.is_none());

    // deleting the chosen one picks the first
    update(&mut s, Msg::OpenRecipes);
    let mut e = s.recipe_editor.clone().unwrap();
    e.rows.remove(0);
    update(&mut s, Msg::EditRecipes(e));
    update(&mut s, Msg::SaveRecipes);
    assert_eq!(s.config.recipes.len(), 1);
    assert_eq!((s.generate.preset, s.generate.steps), (0, 20));
    assert_eq!(s.generate.sweep.models, ["A2"]);
}

#[test]
fn queueing_a_preset_no_server_runs_asks_first() {
    use increart_core::config::RecipePreset;
    use increart_core::sdserver::Loadable;
    let preset = |name: &str, model: &str| RecipePreset {
        name: name.into(),
        recipe: "qwen-image-21".into(),
        files: [("diffusion_model".to_string(), model.to_string())].into(),
        steps: 20,
        cfg: 2.0,
        flow_shift: None,
        custom_sigmas: vec![],
        loras: vec![],
    };
    let mut h = Harness::new();
    let mut other = preset("Other", "other.gguf");
    other.recipe = "flux".into();
    h.state.config.recipes = vec![preset("Int8", "int8.safetensors"), preset("Q8", "q8.gguf"), other];
    // a draft on Int8, its steps changed
    h.state.generate.preset = 0;
    h.state.generate.steps = 31;
    h.state.servers = vec![ServerStatus {
        config: None,
        name: "kcpp".into(),
        device: String::new(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    }];
    let kobold = Loadable::Files {
        ids: ["q8.gguf".to_string()].into(),
        refused: [("int8.safetensors".to_string(), "is in sd-server's int8 format".to_string())].into(),
    };
    update(&mut h.state, Msg::ServerModels(vec![("kcpp".into(), kobold)]));
    assert_eq!(h.state.runnable_recipes(), vec![1]);
    // the same model in a preset it runs: switched to, the draft's steps kept, said
    assert_eq!((h.state.generate.preset, h.state.generate.steps), (1, 31));
    assert!(h.state.messages.iter().any(|m| m.text.contains("Model preset Q8 instead of Int8")));
    // no other preset of that recipe: nothing to switch to
    assert_eq!(h.state.runnable_alternative(2), None);
    assert_eq!(h.state.runnable_alternative(1), None, "it runs");

    // chosen anyway (e.g. a draft from before): the warning, with the reason
    h.state.generate.preset = 0;
    h.state.generate.prompt = "a fox".into();
    h.state.project = None;
    update(&mut h.state, Msg::QueueGenerate);
    let c = h.state.confirm.clone().expect("asks");
    assert!(c.text.contains("Int8:\n  kcpp: int8.safetensors is in sd-server's int8 format"), "{}", c.text);
    assert_eq!(c.verb, "Queue anyway");
    assert_eq!(*c.then, Msg::QueueModelChecked);
    // the fix offered: that model on a preset the server runs
    let (name, then) = c.fix.clone().unwrap();
    assert_eq!((name.as_str(), *then), ("Use Q8 and queue", Msg::QueueWithPreset(1)));
    update(&mut h.state, Msg::ConfirmFix);
    assert_eq!(h.state.generate.preset, 1);
    assert!(h.state.confirm.as_ref().is_none_or(|c| c.verb != "Queue anyway"), "not asked again");
    // a preset with no runnable twin: no fix
    h.state.confirm = None;
    h.state.generate.preset = 2;
    update(&mut h.state, Msg::QueueGenerate);
    assert!(h.state.confirm.clone().unwrap().fix.is_none());
    h.state.generate.preset = 0;

    // one the server runs: straight on
    h.state.confirm = None;
    h.state.generate.preset = 1;
    update(&mut h.state, Msg::QueueGenerate);
    assert!(h.state.confirm.is_none());
}

/// Opening a library whose generated images have lost their links (an unzip
/// that skipped symlinks) says so, and queues nothing for them.
#[test]
fn opening_a_library_with_lost_output_links_warns() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = increart_core::Library::create(&tmp.path().join("lib"), None).unwrap();
    let idea = lib.new_idea(increart_core::NewIdea::text("A", "")).unwrap();
    let (project, _) = lib.create_project(Path::new(""), None, std::slice::from_ref(&idea)).unwrap();
    let mut cfg = Config::default();
    cfg.normalize();
    let run = increart_core::NewRun {
        sweep: None,
        label: "r01".into(),
        prompt: "x".into(),
        negative_prompt: String::new(),
        alpha: false,
        width: 64,
        height: 64,
        sample: increart_core::model::Sample {
            method: "euler".into(),
            scheduler: String::new(),
            steps: 4,
            cfg: 1.0,
            flow_shift: None,
            custom_sigmas: vec![],
        },
        mode: increart_core::model::Mode::Generate,
        refs: vec![],
        model: increart_core::model::ModelSelection {
            recipe: "qwen-image-21".into(),
            files: Default::default(),
            loras: vec![],
        },
        source_idea: None,
        first_seed: 0,
        seeds: 1,
    };
    let run = lib.create_run(&project, run).unwrap();
    let (_, def) = lib.find_run(&run).unwrap();
    let p = increart_core::request::Provenance { elapsed_s: 30.0, ..Default::default() };
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let (_, at) = lib.write_output(&run, 0, 0, &png, &increart_core::request::recipe(&def, 0, &p)).unwrap();
    std::fs::remove_file(lib.path(&at)).unwrap();
    let ix = lib.scan().unwrap();
    assert_eq!(ix.pending_seeds(), 0);

    let mut s = AppState::new(cfg, None);
    update(&mut s, Msg::LibraryOpened(Ok((lib.root().to_path_buf(), ix))));
    let m = s.messages.last().expect("a warning");
    assert!(m.error && m.text.contains("1 generated image(s) have no output link"), "{}", m.text);
}

/// Save for a bug report: a zip where picked (`.zip` added) of the config
/// and the logs; pressed again while it's written, nothing.
#[test]
fn a_bug_report_is_saved_where_picked() {
    let mut h = Harness::new();
    let tmp = h._tmp.path().to_path_buf();
    std::fs::write(tmp.join("config.rsn"), "Config ()").unwrap();
    h.state.config_path = Some(tmp.join("config.rsn"));
    std::fs::create_dir_all(tmp.join("logs")).unwrap();
    std::fs::write(tmp.join("logs/sd-server-a.log"), "loading\n").unwrap();
    let fx = h.send(Msg::SaveBugReport);
    assert!(matches!(&fx[..], [Effect::Pick { kind: PickKind::BugReport, .. }]));
    let fx = h.send(Msg::Picked(PickKind::BugReport, vec![tmp.join("report")]));
    assert!(matches!(&fx[..], [Effect::SaveBugReport { dest, .. }] if dest.ends_with("report.zip")));
    assert!(h.send(Msg::SaveBugReport).is_empty(), "not again while saving");
    h.settle();
    assert!(!h.state.saving_bug_report);
    let m = h.state.messages.last().unwrap();
    assert!(!m.error && m.text.starts_with("Saved for a bug report: "), "{}", m.text);
    let z = zip::ZipArchive::new(std::fs::File::open(tmp.join("report.zip")).unwrap()).unwrap();
    let names: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
    for n in ["report/config/config.rsn", "report/logs/sd-server-a.log", "report/about.txt"] {
        assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
    }
}

/// Export library writes the archive where picked (its extension added);
/// Unpack a library archive: into a folder picked after it, then opened (from
/// the library menu, and from the start-up library screen).
#[test]
fn a_library_exported_and_opened_again() {
    use increart_core::archive::ArchiveFormat;
    let mut h = Harness::new();
    h.send(Msg::Draft(IdeaDraft { title: "Otter".into(), text: "otter".into(), ..Default::default() }));
    h.send(Msg::AddIdea(AddIdea::Inbox));
    h.settle();
    let fx = h.send(Msg::ExportLibrary(ArchiveFormat::TarGz));
    assert!(matches!(&fx[..], [Effect::Pick { kind: PickKind::LibraryArchiveOut(ArchiveFormat::TarGz), .. }]));
    let away = h._tmp.path().join("away");
    std::fs::create_dir_all(&away).unwrap();
    let fx = h.send(Msg::Picked(PickKind::LibraryArchiveOut(ArchiveFormat::TarGz), vec![away.join("backup")]));
    assert!(matches!(&fx[..], [Effect::ExportLibrary { dest, .. }] if dest.ends_with("backup.tar.gz")));
    h.settle();
    assert!(h.state.archiving.is_none());
    assert!(away.join("backup.tar.gz").is_file(), "{:?}", h.state.messages);
    assert!(h.state.messages.last().unwrap().text.starts_with("Exported the library to"));

    // unpacked into a folder picked after the archive (starting beside it)
    let fx = h.send(Msg::Picked(PickKind::LibraryArchiveIn, vec![away.join("backup.tar.gz")]));
    assert!(matches!(&fx[..], [Effect::Pick { kind: PickKind::LibraryUnpackInto, start: Some(s) }] if *s == away));
    let into = h._tmp.path().join("restored");
    std::fs::create_dir_all(&into).unwrap();
    h.send(Msg::Picked(PickKind::LibraryUnpackInto, vec![into.clone()]));
    h.settle();
    assert!(h.errors().is_empty(), "{:?}", h.errors());
    assert_eq!(h.state.library.as_deref(), Some(into.join("lib").as_path()), "{:?}", h.state.messages);
    assert_eq!(h.state.index.ideas.len(), 1);

    // from the start-up library screen: unpacked and opened there; the
    // folder there already is refused, said on the screen
    h.send(Msg::OtherLibrary(None));
    assert!(matches!(h.state.phase, Phase::Library(_)));
    let fx = h.send(Msg::UnpackLibraryArchive);
    assert!(matches!(&fx[..], [Effect::Pick { kind: PickKind::LibraryArchiveIn, .. }]));
    h.send(Msg::Picked(PickKind::LibraryArchiveIn, vec![away.join("backup.tar.gz")]));
    h.send(Msg::Picked(PickKind::LibraryUnpackInto, vec![into.clone()]));
    h.settle();
    let Phase::Library(d) = &h.state.phase else { panic!("{:?}", h.state.phase) };
    assert!(d.error.as_deref().is_some_and(|e| e.contains("there already")), "{:?}", d.error);
    h.send(Msg::Picked(PickKind::LibraryArchiveIn, vec![away.join("backup.tar.gz")]));
    h.send(Msg::Picked(PickKind::LibraryUnpackInto, vec![away.clone()]));
    h.settle();
    assert_eq!(h.state.phase, Phase::Main);
    assert_eq!(h.state.library.as_deref(), Some(away.join("lib").as_path()), "{:?}", h.state.messages);
}

#[test]
fn timeouts_are_set_at_least_a_second_and_saved() {
    use increart_core::config::Timeouts;
    let mut s = dialog_state(vec![], None);
    s.config_path = Some(PathBuf::from("/nowhere/config.rsn"));
    let t = Timeouts { server_start: 900, request: 0, ..Timeouts::DEFAULT };
    let fx = update(&mut s, Msg::SetTimeouts(t));
    assert!(matches!(fx[..], [Effect::SaveConfig(..)]), "{fx:?}");
    assert_eq!(s.config.timeouts, Timeouts { server_start: 900, request: 1, ..Timeouts::DEFAULT });
    let same = s.config.timeouts;
    assert!(update(&mut s, Msg::SetTimeouts(same)).is_empty(), "unchanged: not saved");
    // in config.rsn only when not the defaults
    let text = increart_core::rsnfmt::to_string(&s.config).unwrap();
    assert!(text.contains("server_start: 900"), "{text}");
    let back: increart_core::config::Config = increart_core::rsnfmt::from_str(&text).unwrap();
    assert_eq!(back.timeouts, s.config.timeouts);
    let text = increart_core::rsnfmt::to_string(&increart_core::config::Config::default()).unwrap();
    assert!(!text.contains("timeouts"));
}

#[test]
fn skipping_the_ram_check_is_saved_and_reaches_the_engine() {
    let mut s = dialog_state(vec![], None);
    assert!(!s.config.skip_ram_check);
    let fx = update(&mut s, Msg::SetSkipRamCheck(true));
    assert!(s.config.skip_ram_check);
    assert!(fx.iter().any(|f| matches!(f, Effect::SetSkipRamCheck(true))));
    assert!(update(&mut s, Msg::SetSkipRamCheck(true)).is_empty());
}

#[test]
fn the_traffic_log_setting_is_saved_and_applied() {
    let mut s = dialog_state(vec![], None);
    assert!(!s.config.log_traffic);
    let fx = update(&mut s, Msg::SetLogTraffic(true));
    assert!(s.config.log_traffic);
    assert!(fx.iter().any(|f| matches!(f, Effect::SetLogTraffic(true))));
    assert!(fx.iter().any(|f| matches!(f, Effect::SaveConfig(..))), "{fx:?}");
    assert!(update(&mut s, Msg::SetLogTraffic(true)).is_empty());
    let text = increart_core::rsnfmt::to_string(&s.config).unwrap();
    assert!(text.contains("log_traffic: true"), "{text}");
}

#[test]
fn a_server_test_shows_its_steps_and_stops() {
    use increart_core::servertest::{Outcome, Step};
    let mut s = dialog_state(vec![], None);
    let proxy =
        ServerConfig::Connect { name: "proxy".into(), url: "http://127.0.0.1:7800/".into(), device: String::new() };
    let fx = update(&mut s, Msg::TestServer(proxy.clone()));
    let [Effect::TestServer { run, generate: Ok(()), .. }] = fx.as_slice() else { panic!("{fx:?}") };
    let run = *run;
    let step = |outcome| Step { title: "Answers".into(), outcome, detail: String::new() };
    update(&mut s, Msg::ServerTestStep(run, 0, step(Outcome::Running)));
    update(&mut s, Msg::ServerTestStep(run, 0, step(Outcome::Ok)));
    // an older test's step is dropped
    update(&mut s, Msg::ServerTestStep(run - 1, 1, step(Outcome::Failed)));
    assert_eq!(s.server_test.as_ref().unwrap().steps, [step(Outcome::Ok)]);
    assert_eq!(update(&mut s, Msg::StopServerTest), [Effect::StopServerTest]);
    update(&mut s, Msg::ServerTestDone(run));
    assert!(update(&mut s, Msg::StopServerTest).is_empty());
    assert!(update(&mut s, Msg::CloseServerTest).is_empty());
    assert!(s.server_test.is_none());
    // one the engine uses doesn't load or make an image
    s.engine_running = true;
    s.servers = vec![ServerStatus {
        config: Some(proxy.clone()),
        name: "proxy".into(),
        device: String::new(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    }];
    let fx = update(&mut s, Msg::TestServer(proxy));
    let [Effect::TestServer { run: next, generate: Err(why), .. }] = fx.as_slice() else { panic!("{fx:?}") };
    assert!(*next > run && why.contains("proxy"), "{why}");
    // closing one still going stops it
    assert_eq!(update(&mut s, Msg::CloseServerTest), [Effect::StopServerTest]);
}

#[test]
fn validate_servers_lists_runs_checked_cases_and_stops() {
    use increart_core::apitest::{Case, Cost, Counts, Event, Outcome};
    let mut s = dialog_state(vec![], None);
    let fx = update(&mut s, Msg::ApiTest(true));
    let [Effect::ApiTestList { run, config }] = fx.as_slice() else { panic!("{fx:?}") };
    assert_eq!(config, Path::new("/nonexistent/config.rsn"));
    let run = *run;
    assert!(s.api_test.as_ref().unwrap().listing);
    let case = |id: &str, na: Option<&str>| Case {
        id: id.into(),
        preset: "Two".into(),
        server: None,
        group: "together".into(),
        title: id.into(),
        cost: Cost::Light,
        basic: false,
        not_applicable: na.map(String::from),
        expects: String::new(),
    };
    let plan = vec![
        case("Two/together.open", None),
        case("Two/a/connect.latency", None),
        case("Two/a/generate.lora", Some("no LoRA")),
    ];
    update(&mut s, Msg::ApiTestEvent(run, Event::Plan { cases: plan.clone() }));
    update(&mut s, Msg::ApiTestEnded(run, None));
    let t = s.api_test.as_ref().unwrap();
    assert!(!t.listing && t.error.is_none());
    // n/a ones start unchecked
    assert_eq!(t.cases.iter().map(|c| c.checked).collect::<Vec<_>>(), [true, true, false]);
    update(&mut s, Msg::ApiTestCheck(vec![0], false));
    let fx = update(&mut s, Msg::ApiTestRun(ApiRun::Checked));
    let [Effect::ApiTestRun { run, cases, .. }] = fx.as_slice() else { panic!("{fx:?}") };
    assert_eq!(cases, &["Two/a/connect.latency"]);
    let run = *run;
    // a second run waits for the first
    assert!(update(&mut s, Msg::ApiTestRun(ApiRun::All)).is_empty());
    // its plan (the cases it runs) keeps the list as it is
    update(&mut s, Msg::ApiTestEvent(run, Event::Plan { cases: plan[..2].to_vec() }));
    assert_eq!(s.api_test.as_ref().unwrap().cases.len(), 3);
    for e in [
        Event::Start { id: "Two/together.open".into() },
        Event::End { id: "Two/together.open".into(), outcome: Outcome::Pass, summary: "a answers".into(), secs: 0.1 },
        Event::Start { id: "Two/a/connect.latency".into() },
        Event::Log { id: Some("Two/a/connect.latency".into()), text: "20 asks".into() },
        Event::Traffic { id: Some("Two/a/connect.latency".into()), text: "GET /sdcpp/v1/capabilities".into() },
    ] {
        update(&mut s, Msg::ApiTestEvent(run, e));
    }
    // an older run's event is dropped
    update(&mut s, Msg::ApiTestEvent(run - 1, Event::Log { id: None, text: "old".into() }));
    let t = s.api_test.as_ref().unwrap();
    assert_eq!(t.selected, Some(1), "it follows the case running");
    assert!(t.cases[1].running && t.cases[1].log == ["20 asks"] && t.cases[1].traffic.len() == 1);
    assert_eq!(t.cases[0].outcome, Some(Outcome::Pass));
    assert!(t.log.is_empty());
    // closing while it runs stops it first, keeping the window
    assert_eq!(update(&mut s, Msg::ApiTest(false)), [Effect::ApiTestStop]);
    assert!(update(&mut s, Msg::ApiTestStop).is_empty());
    assert!(s.api_test.is_some());
    let end = Event::End {
        id: "Two/a/connect.latency".into(),
        outcome: Outcome::Fail,
        summary: "slowest 9 s".into(),
        secs: 9.0,
    };
    update(&mut s, Msg::ApiTestEvent(run, end));
    let mut counts = Counts::default();
    counts.add(Outcome::Pass);
    counts.add(Outcome::Fail);
    update(
        &mut s,
        Msg::ApiTestEvent(run, Event::Done { counts, report: Some("/r".into()), zip: Some("/r.zip".into()) }),
    );
    update(&mut s, Msg::ApiTestEnded(run, None));
    let t = s.api_test.as_ref().unwrap();
    assert!(!t.running && !t.stopping);
    assert_eq!(t.done.as_ref().unwrap().2.as_deref(), Some(Path::new("/r.zip")));
    // Rerun failed: the failed one
    let fx = update(&mut s, Msg::ApiTestRun(ApiRun::Failed));
    let [Effect::ApiTestRun { run, cases, .. }] = fx.as_slice() else { panic!("{fx:?}") };
    assert_eq!(cases, &["Two/a/connect.latency"]);
    // one that ends without a word says why
    update(&mut s, Msg::ApiTestEnded(*run, Some("it ended (signal 9) before finishing".into())));
    assert!(s.api_test.as_ref().unwrap().error.as_deref().unwrap().contains("signal 9"));
    assert!(update(&mut s, Msg::ApiTest(false)).is_empty());
    assert!(s.api_test.is_none());
    // not with servers open: the suite starts its own
    s.servers = vec![ServerStatus {
        config: None,
        name: "a".into(),
        device: String::new(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    }];
    assert!(update(&mut s, Msg::ApiTest(true)).is_empty());
    assert!(s.api_test.is_none());
}

#[test]
fn validate_servers_opens_on_the_last_used_preset_and_shows_one_at_a_time() {
    use increart_core::apitest::{Case, Cost, Event};
    let preset = |name: &str| ServerPreset { name: name.into(), servers: vec![start("gpu0", 1234)] };
    let mut s = dialog_state(vec![preset("Kobold"), preset("Sd")], Some("Sd"));
    let fx = update(&mut s, Msg::ApiTest(true));
    let [Effect::ApiTestList { run, .. }] = fx.as_slice() else { panic!("{fx:?}") };
    let case = |preset: &str, key: &str, na: bool| Case {
        id: format!("{preset}/gpu0/{key}"),
        preset: preset.into(),
        server: Some("gpu0".into()),
        group: "connect".into(),
        title: key.into(),
        cost: Cost::Light,
        basic: false,
        not_applicable: na.then(|| "no LoRA".into()),
        expects: String::new(),
    };
    let cases = vec![case("Kobold", "latency", false), case("Sd", "latency", false), case("Sd", "lora", true)];
    update(&mut s, Msg::ApiTestEvent(*run, Event::Plan { cases }));
    let t = s.api_test.as_ref().unwrap();
    // the last used one, its cases for its setup checked, no other's
    assert_eq!(t.preset.as_deref(), Some("Sd"));
    assert_eq!(t.presets(), ["Kobold", "Sd"]);
    assert_eq!(t.shown_cases(), [1, 2]);
    assert_eq!(t.cases.iter().map(|c| c.checked).collect::<Vec<_>>(), [false, true, false]);
    // Run all: all of the preset shown
    assert_eq!(t.ids(ApiRun::All), ["Sd/gpu0/latency", "Sd/gpu0/lora"]);
    // another: its cases, and only those
    update(&mut s, Msg::ApiTestPreset(Some("Kobold".into())));
    let t = s.api_test.as_ref().unwrap();
    assert_eq!(t.shown_cases(), [0]);
    assert_eq!(t.cases.iter().map(|c| c.checked).collect::<Vec<_>>(), [true, false, false]);
    // all presets: all shown, the checks kept; Run all runs everything
    update(&mut s, Msg::ApiTestPreset(None));
    let t = s.api_test.as_ref().unwrap();
    assert_eq!(t.shown_cases(), [0, 1, 2]);
    assert_eq!(t.cases.iter().map(|c| c.checked).collect::<Vec<_>>(), [true, false, false]);
    assert!(t.ids(ApiRun::All).is_empty());
    // one that isn't there: all presets
    update(&mut s, Msg::ApiTestPreset(Some("Gone".into())));
    assert_eq!(s.api_test.as_ref().unwrap().preset, None);
}

#[test]
fn opening_a_preset_not_validated_asks_once_to_validate_it_first() {
    use increart_core::apitest::{Case, Cost, Counts, Event, Outcome};
    use increart_core::validation::{Stamp, fingerprint};
    let tmp = tempfile::tempdir().unwrap();
    let preset = |name: &str, port| ServerPreset { name: name.into(), servers: vec![start("gpu0", port)] };
    let mut s = dialog_state(vec![preset("Sd", 1234), preset("Checked", 1240)], Some("Sd"));
    // tests have no stamps file: nothing asked
    update(&mut s, Msg::OpenPreset(0));
    assert!(s.confirm.is_none());
    let mut s = dialog_state(vec![preset("Sd", 1234), preset("Checked", 1240)], Some("Sd"));
    let path = tmp.path().join("validated.json");
    s.validations_path = Some(path.clone());
    s.suite_found = true;
    // one validated with the settings it has
    let checked = s.config.server_presets[1].clone();
    s.validations.stamps.push(
        Stamp {
            preset: "Checked".into(),
            fingerprint: fingerprint(&s.config, &checked),
            when: "2026-10-31T17:00:00+01:00".into(),
            outcome: Outcome::Warn,
            counts: Counts::default(),
            servers: vec![],
            suite: String::new(),
            report_sha256: String::new(),
            zip: None,
            seal: String::new(),
        }
        .sealed(),
    );
    update(&mut s, Msg::OpenPreset(1));
    assert!(s.confirm.is_none(), "validated: opened without asking");
    let mut s2 = dialog_state(vec![preset("Sd", 1234)], Some("Sd"));
    s2.validations_path = Some(path.clone());
    s2.suite_found = true;
    let mut s = s2;
    let fx = update(&mut s, Msg::OpenPreset(0));
    let fp = fingerprint(&s.config, &s.config.server_presets[0].clone());
    assert!(fx.contains(&Effect::ValidationAsked(path.clone(), fp)), "{fx:?}");
    let c = s.confirm.clone().expect("asked");
    assert_eq!(c.heading.as_deref(), Some("Validate these servers first?"));
    assert!(c.text.contains("Sd hasn't been validated") && c.text.contains("Strongly recommended"), "{}", c.text);
    // Validate now: the window on it; once listed, its basic cases run
    let fx = update(&mut s, Msg::ConfirmYes);
    let [Effect::ApiTestList { run, .. }] = fx.as_slice() else { panic!("{fx:?}") };
    let run = *run;
    let case = |key: &str, basic: bool| Case {
        id: format!("Sd/gpu0/{key}"),
        preset: "Sd".into(),
        server: Some("gpu0".into()),
        group: "connect".into(),
        title: key.into(),
        cost: Cost::Light,
        basic,
        not_applicable: None,
        expects: String::new(),
    };
    let cases = vec![case("connect.capabilities", true), case("connect.latency", false)];
    update(&mut s, Msg::ApiTestEvent(run, Event::Plan { cases }));
    let fx = update(&mut s, Msg::ApiTestEnded(run, None));
    let [Effect::ApiTestRun { run, cases, .. }] = fx.as_slice() else { panic!("{fx:?}") };
    assert_eq!(cases, &["Sd/gpu0/connect.capabilities"]);
    // its end: the stamps read again
    let fx = update(&mut s, Msg::ApiTestEnded(*run, None));
    assert_eq!(fx, [Effect::LoadValidations(path.clone())]);
    update(&mut s, Msg::ApiTest(false));
    // asked once for these settings: opened now, not asked again
    update(&mut s, Msg::OpenPreset(0));
    assert!(s.confirm.is_none());
}

#[test]
fn a_basic_validation_ends_on_its_summary_and_continues_to_the_preset() {
    use increart_core::apitest::{Case, Cost, Event, Outcome};
    use increart_core::validation::fingerprint;
    let tmp = tempfile::tempdir().unwrap();
    let preset = |name: &str, port| ServerPreset { name: name.into(), servers: vec![start("gpu0", port)] };
    let mut s = dialog_state(vec![preset("Sd", 1234)], Some("Sd"));
    let path = tmp.path().join("validated.json");
    s.validations_path = Some(path.clone());
    s.suite_found = true;
    assert!(!s.api_test_advanced, "the basic view first");
    let fx = update(&mut s, Msg::ValidateBasic("Sd".into()));
    let [Effect::ApiTestList { run, .. }] = fx.as_slice() else { panic!("{fx:?}") };
    let case = |key: &str, basic: bool| Case {
        id: format!("Sd/gpu0/{key}"),
        preset: "Sd".into(),
        server: Some("gpu0".into()),
        group: "connect".into(),
        title: key.into(),
        cost: Cost::Light,
        basic,
        not_applicable: None,
        expects: String::new(),
    };
    let cases = vec![case("connect.capabilities", true), case("models.list", true), case("connect.latency", false)];
    update(&mut s, Msg::ApiTestEvent(*run, Event::Plan { cases }));
    s.now = 10.0;
    let fx = update(&mut s, Msg::ApiTestEnded(*run, None));
    let [Effect::ApiTestRun { run, .. }] = fx.as_slice() else { panic!("{fx:?}") };
    let run = *run;
    let t = s.api_test.as_ref().unwrap();
    assert_eq!(t.ran, [0, 1], "the basic cases");
    assert_eq!(t.basic_cases(), [0, 1]);
    assert!(!t.finished() && t.started == 10.0);
    // Continue does nothing while it runs
    update(&mut s, Msg::ApiTestContinue);
    assert!(s.api_test.is_some());
    for (id, outcome) in [("Sd/gpu0/connect.capabilities", Outcome::Pass), ("Sd/gpu0/models.list", Outcome::Fail)] {
        update(&mut s, Msg::ApiTestEvent(run, Event::End { id: id.into(), outcome, summary: "x".into(), secs: 1.0 }));
    }
    s.now = 75.0;
    update(&mut s, Msg::ApiTestEnded(run, None));
    let t = s.api_test.as_ref().unwrap();
    assert!(t.finished() && !t.stopped && t.ended == Some(75.0), "its summary");
    // the advanced view, kept for the session
    update(&mut s, Msg::ApiTestAdvanced(true));
    assert!(s.api_test_advanced);
    update(&mut s, Msg::ApiTestAdvanced(false));
    // Open anyway: the window closes, the preset opens, not asked again
    let fx = update(&mut s, Msg::ApiTestContinue);
    let fp = fingerprint(&s.config, &s.config.server_presets[0].clone());
    assert!(fx.contains(&Effect::ValidationAsked(path, fp)), "{fx:?}");
    assert!(s.api_test.is_none());
    let Phase::Servers(d) = &s.phase else { panic!() };
    assert!(d.opening, "opening Sd");
}

#[test]
fn a_validation_stopped_says_so() {
    use increart_core::apitest::{Case, Cost, Event};
    let mut s = dialog_state(vec![ServerPreset { name: "Sd".into(), servers: vec![start("gpu0", 1234)] }], Some("Sd"));
    let fx = update(&mut s, Msg::ValidateBasic("Sd".into()));
    let [Effect::ApiTestList { run, .. }] = fx.as_slice() else { panic!("{fx:?}") };
    let cases = vec![Case {
        id: "Sd/gpu0/models.list".into(),
        preset: "Sd".into(),
        server: Some("gpu0".into()),
        group: "models".into(),
        title: "models".into(),
        cost: Cost::Light,
        basic: true,
        not_applicable: None,
        expects: String::new(),
    }];
    update(&mut s, Msg::ApiTestEvent(*run, Event::Plan { cases }));
    let fx = update(&mut s, Msg::ApiTestEnded(*run, None));
    let [Effect::ApiTestRun { run, .. }] = fx.as_slice() else { panic!("{fx:?}") };
    let run = *run;
    update(&mut s, Msg::ApiTestStop);
    update(&mut s, Msg::ApiTestEnded(run, None));
    let t = s.api_test.as_ref().unwrap();
    assert!(t.finished() && t.stopped);
    // a run again starts over
    update(&mut s, Msg::ApiTestRun(increart_gui_core::ApiRun::All));
    let t = s.api_test.as_ref().unwrap();
    assert!(!t.stopped && t.ended.is_none() && !t.finished());
}

#[test]
fn a_server_problem_links_the_servers_log() {
    let mut s = dialog_state(vec![], None);
    let status = |name: &str, config: Option<ServerConfig>| ServerStatus {
        config,
        name: name.into(),
        device: String::new(),
        state: "ready".into(),
        model: None,
        idle_since: None,
        worked: false,
    };
    let connect =
        ServerConfig::Connect { name: "far".into(), url: "http://10.0.0.2:1234".into(), device: String::new() };
    s.servers = vec![
        status("sd-rocm0", Some(start("sd-rocm0", 1234))),
        status("sd-rocm01", Some(start("sd-rocm01", 1235))),
        status("far", Some(connect)),
    ];
    let problem = |s: &mut AppState, text: &str| {
        update(s, Msg::Engine(increart_core::engine::Event::Problem(text.into())));
        s.messages.last().cloned().unwrap()
    };
    let m = problem(&mut s, "sd-rocm0: can't load qwen-image-21: unknown model or incorrect category");
    assert!(m.error);
    assert_eq!(m.log.as_deref(), Some("sd-server-sd-rocm0.log"), "its own, not sd-rocm01's");
    // connected to, not started: the app has no log of it
    assert_eq!(problem(&mut s, "far: loading x failed: oom").log, None);
    assert_eq!(problem(&mut s, "no server can take qwen-image-21").log, None);
}

#[test]
fn home_starts_a_project_and_asks_for_references() {
    let mut h = Harness::new();
    assert_eq!(h.state.tab, Tab::Home, "the app opens on Home");

    // with references from the library: the picker opens on its form
    h.send(Msg::StartProject(true));
    let d = h.state.start_project.clone().unwrap();
    assert_eq!((d.name.as_str(), d.refs), ("new project", StartRefs::Library));
    h.send(Msg::EditStartProject(StartProjectDialog { name: "otters".into(), refs: StartRefs::Library }));
    h.send(Msg::ConfirmStartProject);
    h.settle();
    assert!(h.state.start_project.is_none());
    assert!(h.root.join("projects/otters").is_dir());
    let otters = h.state.index.projects.iter().find(|p| p.name() == "otters").unwrap().project.id.clone();
    assert_eq!(h.state.project.as_ref(), Some(&otters));
    assert_eq!((h.state.tab, h.state.projects_tab), (Tab::Projects, ProjectsTab::Generate));
    assert_eq!(h.state.ref_picker.as_ref().map(|p| p.target.clone()), Some(PickTarget::Generate));
    h.send(Msg::RefPicker(false));

    // an invalid name keeps the dialog open
    h.send(Msg::StartProject(true));
    h.send(Msg::EditStartProject(StartProjectDialog { name: "a/b".into(), refs: StartRefs::None }));
    assert!(h.send(Msg::ConfirmStartProject).is_empty());
    assert!(h.state.start_project.is_some());

    // from files: Generate's Add file… picker
    h.send(Msg::EditStartProject(StartProjectDialog { name: "badgers".into(), refs: StartRefs::Files }));
    h.send(Msg::ConfirmStartProject);
    let mut picked = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while h.state.busy > 0 && std::time::Instant::now() < deadline {
        for m in h.rt.wait(Duration::from_millis(300)) {
            picked |= h.send(m).iter().any(|f| matches!(f, Effect::Pick { kind: PickKind::GenerateRefs, .. }));
        }
    }
    h.settle();
    assert!(picked, "Add file… opened");
    assert!(h.state.ref_picker.is_none());
    assert!(h.root.join("projects/badgers").is_dir());

    // a taken name: the next number; not now: just the form
    h.send(Msg::StartProject(true));
    h.send(Msg::EditStartProject(StartProjectDialog { name: "stoats".into(), refs: StartRefs::None }));
    h.send(Msg::ConfirmStartProject);
    h.settle();
    assert!(h.state.ref_picker.is_none());
    assert_eq!((h.state.tab, h.state.projects_tab), (Tab::Projects, ProjectsTab::Generate));
    h.send(Msg::StartProject(true));
    h.send(Msg::ConfirmStartProject);
    h.settle();
    assert!(h.root.join("projects/new project").is_dir());
    h.send(Msg::RefPicker(false));
    h.send(Msg::StartProject(true));
    assert_eq!(h.state.start_project.as_ref().unwrap().name, "new project 2");
    h.send(Msg::StartProject(false));
    assert!(h.state.start_project.is_none());
    assert!(h.errors().is_empty(), "{:?}", h.errors());
}

#[test]
fn picked_images_make_an_image_sweep_in_a_new_project() {
    use increart_core::model::Mode;
    use increart_core::prompts::{PromptPreset, PromptPresets, PromptTask, SweepVary};
    let mut h = Harness::new();
    h.send(Msg::NewProject(PathBuf::new()));
    h.send(Msg::CommitRename);
    h.settle();
    h.state.no_server_warned = true;
    let preset =
        |name: &str, prompt: &str| PromptPreset { name: name.into(), prompt: prompt.into(), ..Default::default() };
    h.state.prompt_presets = PromptPresets {
        tasks: vec![PromptTask {
            name: "Looks".into(),
            description: String::new(),
            presets: vec![preset("Clay", "claymation"), preset("Ink", "ink")],
        }],
    };
    // a style sweep's outputs to pick from
    let mut f = h.state.generate.clone();
    f.prompt = "a fox, {preset}".into();
    f.sweep.on = true;
    f.steps = 12;
    f.seeds = 1;
    h.send(Msg::Generate(f));
    h.send(Msg::SweepTicks(vec![(0, 0), (0, 1)], true));
    h.send(Msg::QueueGenerate);
    h.settle();
    let p = h.state.project_entry().unwrap();
    for r in &p.runs {
        let seeds: Vec<u64> = r.all_seeds().map(|(_, s)| s).collect();
        Library_::steal(&h.root).write_seeds(&r.id, &seeds);
    }
    h.rt.run(Effect::Rescan);
    h.settle();
    let style_of = |h: &Harness, i: &increart_core::ImageRef| {
        let run = &h.state.output_link(i).unwrap().name.run;
        h.state.index.run(run).unwrap().1.sweep.as_ref().unwrap().preset.clone()
    };
    let mut images: Vec<_> = h.state.project_entry().unwrap().outputs.iter().map(|o| o.image.clone()).collect();
    images.sort_by_key(|i| std::cmp::Reverse(style_of(&h, i)));
    assert_eq!(images.iter().map(|i| style_of(&h, i)).collect::<Vec<_>>(), ["Ink", "Clay"], "picked Ink first");

    // the folder selected in the Projects tree is where it goes
    h.send(Msg::NewFolder(increart_core::Tree::Projects, PathBuf::new()));
    h.settle();
    h.send(Msg::EditRename("dest".into()));
    h.send(Msg::CommitRename);
    h.settle();
    h.send(Msg::SelectProjectFolder(Some("dest".into())));
    h.send(Msg::Tab(Tab::OutputLibrary));
    h.send(Msg::PickSet(images.clone()));
    h.send(Msg::SweepProject(true));
    let d = h.state.sweep_project.clone().expect("the dialog");
    assert_eq!((d.name.as_str(), d.parent.as_path()), ("new project sweep", Path::new("dest")));
    assert_eq!(d.images, images, "in pick order");
    h.send(Msg::ConfirmSweepProject);
    h.settle();
    assert!(h.errors().is_empty(), "{:?}", h.errors());
    assert_eq!(h.state.project_entry().unwrap().dir, Path::new("projects/dest/new project sweep"));
    assert_eq!((h.state.tab, h.state.projects_tab), (Tab::Projects, ProjectsTab::Generate));
    let g = h.state.generate.clone();
    assert!(g.sweep.on && g.sweep.vary == SweepVary::Image);
    assert_eq!(g.sweep.images, images);
    assert_eq!((g.prompt.as_str(), g.steps), ("<image1> ", 12), "the first pick's run's settings");
    assert!(g.images.is_empty());
    // the first swept image stands for them all: <image1>, the form's own after it
    assert_eq!(g.refs(), [images[0].clone()]);
    let mut own = g.clone();
    own.add_reference(images[1].clone());
    assert_eq!((own.tag_of(0), own.refs().len()), (Some(2), 2));
    // a draft comes back without the swept image in the list
    let back = GenerateForm::from_draft(&g.to_draft(&h.state.config), &h.state.config);
    assert!(back.images.is_empty() && back.sweep.images == images);

    // a run per image, named by its style, the image its <image1>
    let (runs, _) = update::sweep_runs(&h.state).unwrap();
    let got: Vec<_> = runs.iter().map(|(f, t)| (f.label.clone(), f.refs(), t.param.clone().unwrap())).collect();
    let label = g.label.clone();
    let param = |name: &str, value: &str| increart_core::model::SweepParam { name: name.into(), value: value.into() };
    assert_eq!(
        got,
        [
            (format!("{label} · Ink"), vec![images[0].clone()], param("image", "Ink")),
            (format!("{label} · Clay"), vec![images[1].clone()], param("image", "Clay")),
        ]
    );
    // or each run's start image
    let mut start = g.clone();
    start.sweep.image_start = true;
    assert_eq!(start.init(), Some(images[0].clone()));
    assert!(start.refs().is_empty());
    let runs = update::image_runs(&start, &h.state.swept_names(&images)).unwrap();
    assert_eq!(runs[1].0.init(), Some(images[1].clone()));
    assert_eq!(runs[1].1.param, Some(param("start_image", "Clay")));
    // names that repeat get the seed
    assert_eq!(h.state.swept_names(&[images[0].clone(), images[0].clone()]), ["Ink · s0000", "Ink · s0000"]);
    let mut none = g.clone();
    none.sweep.images.clear();
    assert!(update::image_runs(&none, &[]).unwrap_err().contains("No images to sweep"));

    // queued: the runs hold their images
    h.send(Msg::QueueGenerate);
    h.settle();
    assert!(h.errors().is_empty(), "{:?}", h.errors());
    let p = h.state.project_entry().unwrap();
    let project = p.project.id.clone();
    let mut queued: Vec<_> = p.runs.clone();
    queued.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[1].refs, [images[1].clone()]);
    assert!(matches!(queued[1].mode, Mode::Generate));
    assert_eq!(queued[1].sweep.as_ref().unwrap().row(), "Clay");

    // its sheet's picks sweep just those images again
    for r in &queued {
        let seeds: Vec<u64> = r.all_seeds().map(|(_, s)| s).collect();
        Library_::steal(&h.root).write_seeds(&r.id, &seeds);
    }
    h.rt.run(Effect::Rescan);
    h.settle();
    h.send(Msg::ReviewScope(Some(project.clone())));
    h.send(Msg::ReviewSweep(project.clone(), label.clone()));
    let clay: Vec<_> = h
        .state
        .project_entry()
        .unwrap()
        .outputs
        .iter()
        .filter(|o| o.name.run == queued[1].id)
        .map(|o| o.image.clone())
        .collect();
    h.send(Msg::PickSet(clay));
    h.send(Msg::SheetNewSweep);
    h.settle();
    let g = &h.state.generate;
    assert!(g.sweep.on && g.sweep.vary == SweepVary::Image);
    assert_eq!(g.sweep.images, [images[1].clone()]);
    assert!(g.images.is_empty(), "the swept image isn't also in the list");
    assert_ne!(g.label, label, "the next label");
}

#[test]
fn a_preview_opened_on_a_list_steps_through_it() {
    let mut h = Harness::new();
    let list: Vec<_> = ["2026.11.01-15.24-0001", "2026.11.01-15.24-0002", "2026.11.01-15.24-0003"]
        .iter()
        .map(|n| increart_core::ImageRef::input(n.parse().unwrap()))
        .collect();
    h.send(Msg::PreviewAmong(list.clone(), list[1].clone()));
    assert_eq!(h.state.preview_position(), Some((1, 3)));
    h.send(Msg::PreviewStep(1));
    h.send(Msg::PreviewStep(1));
    assert_eq!(h.state.preview, Some(list[2].clone()), "stops at the last");
    h.send(Msg::PreviewStep(-1));
    assert_eq!(h.state.preview, Some(list[1].clone()));
    h.send(Msg::Preview(None));
    assert!(h.state.preview_among.is_none());
}
