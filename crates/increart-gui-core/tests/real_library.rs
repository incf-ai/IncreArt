//! Against a real library on disk (ignored by default): how long Save in
//! Crop / mark takes shortly after the library opens, while the startup
//! work (usage check, watcher rescans) runs.
//!
//! cargo test --release -p increart-gui-core --test real_library -- --ignored --nocapture
//!
//! INCREART_REAL_LIBRARY: the library (default /mnt/storage/increart_library)
//! INCREART_SAVE_AFTER: seconds after opening before Save (default 3)
//!
//! Each Save writes one hidden edited copy (`input/edited/`), as in the app.
//! Drafts, history and settings aren't written. The engine starts as it
//! does once servers are up, but with none: nothing is generated.

use std::time::{Duration, Instant};

use increart_core::config::Config;
use increart_core::model::{ImageEdits, Mark, Shape};
use increart_gui_core::*;

struct Real {
    state: AppState,
    rt: Runtime,
    t0: Instant,
}

impl Real {
    fn send(&mut self, msg: Msg) {
        for f in update(&mut self.state, msg) {
            match f {
                // leave the library's drafts, history and the settings as they are
                Effect::SaveDraft(_) | Effect::RecordHistory { .. } | Effect::SaveConfig(..) => {}
                // nothing on the GPUs
                Effect::OpenServers { .. } | Effect::PeAsk { .. } => {}
                Effect::PlaySound { .. } => {}
                // never on the desktop
                Effect::Pick { .. } | Effect::OpenInEditor { .. } => {}
                f => self.rt.run(f),
            }
        }
    }

    fn at(&self) -> String {
        format!("{:7.2}s", self.t0.elapsed().as_secs_f64())
    }

    /// Feeds the runtime's messages back for `d` (or until `done`).
    fn pump(&mut self, d: Duration, done: impl Fn(&AppState) -> bool) -> bool {
        let end = Instant::now() + d;
        while Instant::now() < end {
            for m in self.rt.wait(Duration::from_millis(20)) {
                match &m {
                    Msg::UsageCheck(i, n) if i == n => eprintln!("{} usage check done ({n} images)", self.at()),
                    Msg::OpDone(..) => eprintln!("{} OpDone", self.at()),
                    Msg::Scanned(_) => eprintln!("{} Scanned (a watcher rescan)", self.at()),
                    _ => {}
                }
                self.send(m);
            }
            if done(&self.state) {
                return true;
            }
        }
        false
    }

    /// Opens the first project whose form has a reference, marks it, saves;
    /// how long until the editor closes.
    fn annotate_and_save(&mut self) -> Duration {
        let (project, i) = self
            .state
            .index
            .projects
            .iter()
            .find_map(|p| {
                let d = p.draft.as_ref()?;
                Some((p.project.id.clone(), d.images.iter().position(|x| x.reference)?))
            })
            .expect("a project whose Generate form has a reference");
        self.send(Msg::Tab(Tab::Projects));
        self.send(Msg::ProjectsTab(ProjectsTab::Generate));
        self.send(Msg::SelectProject(project));
        let image = self.state.generate.images[i].image.clone();
        eprintln!("{} editing {:?} of {}", self.at(), image, self.state.project_entry().unwrap().name());
        self.send(Msg::OpenMarkup(image, Some(i)));
        let line = (0..20).map(|k| [40.0 + k as f32 * 10.0, 60.0 + (k % 3) as f32 * 15.0]).collect();
        let edits = ImageEdits {
            crop: None,
            marks: vec![Mark { shape: Shape::Pencil(line), color: MarkTools::default().color, width: 6.0 }],
        };
        self.send(Msg::MarkupEdit(edits));
        let t = Instant::now();
        eprintln!("{} Save", self.at());
        self.send(Msg::MarkupSave(false));
        assert!(self.pump(Duration::from_secs(120), |s| s.markup.is_none()), "the editor never closed");
        let took = t.elapsed();
        eprintln!("{} editor closed: Save took {:.2}s", self.at(), took.as_secs_f64());
        let errors: Vec<_> = self.state.messages.iter().filter(|m| m.error).map(|m| &m.text).collect();
        assert!(errors.is_empty(), "{errors:?}");
        took
    }
}

#[test]
#[ignore = "uses a real library on disk; writes an edited copy into it"]
fn save_soon_after_opening() {
    let path = std::env::var("INCREART_REAL_LIBRARY").unwrap_or("/mnt/storage/increart_library".into());
    let after: f64 = std::env::var("INCREART_SAVE_AFTER").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let mut config = Config::default();
    config.normalize();
    let mut r = Real { state: AppState::new(config, None), rt: Runtime::new(|| {}).unwrap(), t0: Instant::now() };
    r.send(Msg::SetLibraryPath(path.clone()));
    r.send(Msg::OpenLibrary);
    assert!(r.pump(Duration::from_secs(120), |s| s.library.is_some()), "{path} didn't open");
    eprintln!("{} opened {path}", r.at());
    r.send(Msg::ContinueWithoutServers);
    r.state.no_server_warned = true;
    // as when the servers are up: the engine finds the runs with seeds left
    let order = r.state.config.job_order;
    r.rt.run(Effect::StartEngine { shared: r.state.config.sdserver.clone(), order, skip_ram_check: false });
    r.pump(Duration::from_secs_f64(after), |_| false);
    let first = r.annotate_and_save();

    // the same again once the startup work is over, to compare
    r.pump(Duration::from_secs(600), |s| s.usage_check.is_none_or(|(i, n)| i == n));
    r.pump(Duration::from_secs(2), |_| false);
    let later = r.annotate_and_save();
    eprintln!("Save: {:.2}s soon after opening, {:.2}s later", first.as_secs_f64(), later.as_secs_f64());
}
