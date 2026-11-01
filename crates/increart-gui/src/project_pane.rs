//! Projects → Grid and Folder: the details pane on the right. The selected
//! project (its counts, source ideas, latest run, recent runs, best
//! outputs), a folder's totals, a run's details, or an image's lineage.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Panel, RichText, ScrollArea};
use increart_core::ImageRef;
use increart_core::Verdict;
use increart_core::model::{Mode, Run};
use increart_core::scan::{IdeaLoc, ProjectEntry};
use increart_gui_core::{AppState, EntryKind, InfoPanel, LibTab, Msg, OutputSource, ProjectsTab, Tab, counts, cover};

use crate::textures::Textures;
use crate::widgets::{ACCENT, BAD, GOOD, NEUTRAL, ic, ph, short, thumb, when};

/// How many runs "Recent runs" lists, and how many images "Best outputs".
const RECENT: usize = 8;
const BEST: usize = 8;

/// What the pane shows.
enum Shown<'a> {
    Project(&'a ProjectEntry),
    Run(&'a ProjectEntry, &'a Run),
    Image(ImageRef),
    /// A project-tree folder, relative to the library root (`projects/…`).
    Folder(PathBuf),
    /// Several folder-view entries.
    Many(usize),
}

pub fn pane(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let Some(root) = st.library.clone() else { return };
    Panel::right("projects_info").default_size(300.0).show(ui, |ui| match shown(st) {
        Shown::Project(p) => {
            header(ui, "Project", out);
            pinned_actions(ui, |ui| project_actions(ui, st, p, out));
            scroll(ui, |ui| project(ui, st, &root, tex, p, out));
        }
        Shown::Run(p, r) => {
            header(ui, "Run", out);
            pinned_actions(ui, |ui| run_actions(ui, st, r, out));
            scroll(ui, |ui| run(ui, st, &root, tex, p, r, out));
        }
        Shown::Image(i) => {
            scroll(ui, |ui| crate::review::lineage_panel(ui, st, &root, tex, &i, InfoPanel::Projects, out));
        }
        Shown::Folder(dir) => {
            header(ui, "Folder", out);
            pinned_actions(ui, |ui| folder_actions(ui, st, &dir, out));
            scroll(ui, |ui| folder(ui, st, &dir));
        }
        Shown::Many(n) => {
            header(ui, "Selection", out);
            ui.weak(format!("{n} items selected."));
        }
    });
}

/// Grid: the selected project, else the folder it shows. Folder: the
/// selected entry, else where it is (a run, a project, or a folder).
fn shown(st: &AppState) -> Shown<'_> {
    let projects = Path::new(increart_core::scan::PROJECTS);
    if st.projects_tab == ProjectsTab::Grid {
        return match st.project_entry() {
            Some(p) => Shown::Project(p),
            None => Shown::Folder(projects.join(st.project_scope())),
        };
    }
    let fv = &st.folder_view;
    if fv.selected.len() > 1 {
        return Shown::Many(fv.selected.len());
    }
    if let Some(name) = fv.selected.first()
        && let Some(e) = fv.entries.iter().find(|e| &e.name == name)
    {
        let path = fv.path.join(name);
        match &e.kind {
            EntryKind::Image(i) => return Shown::Image(i.clone()),
            EntryKind::Project | EntryKind::Run | EntryKind::Folder => return at(st, path),
            _ => {}
        }
    }
    at(st, fv.path.clone())
}

/// What a project-tree path (relative to the library root) is: in a run's
/// folder, the run; in a project's, the project; else a folder.
fn at(st: &AppState, path: PathBuf) -> Shown<'_> {
    let Some(p) = st.index.projects.iter().find(|p| path.starts_with(&p.dir)) else {
        return Shown::Folder(path);
    };
    let rest = path.strip_prefix(&p.dir).unwrap_or(Path::new(""));
    let mut parts = rest.iter();
    if parts.next().is_some_and(|c| c == "runs")
        && let Some(id) = parts.next()
        && let Some(r) = p.runs.iter().find(|r| r.id.as_str() == id.to_string_lossy())
    {
        return Shown::Run(p, r);
    }
    Shown::Project(p)
}

fn header(ui: &mut egui::Ui, title: &str, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong());
        if ui.small_button(ph::X).on_hover_text("close").clicked() {
            out.push(Msg::ShowInfo(InfoPanel::Projects, false));
        }
    });
}

/// The pane's buttons, pinned to its bottom.
fn pinned_actions(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    Panel::bottom("projects_info_actions")
        .resizable(false)
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 6)))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.horizontal_wrapped(add);
        });
}

fn scroll(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ScrollArea::vertical().auto_shrink([false, false]).id_salt("projects_info_scroll").show(ui, add);
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.separator();
    ui.label(RichText::new(title).strong());
}

/// A project's timing (DESIGN.md, Timing): per image and step, GPU time,
/// each model's when there are several, and its time left while it
/// generates; `None` with no timed images and nothing generating.
fn project_timing(st: &AppState, p: &ProjectEntry) -> Option<Vec<(&'static str, String)>> {
    use increart_gui_core::timing::{Timing, duration, left_text};
    let mut rows = vec![];
    if let Some(t) = Timing::of_runs(&p.runs) {
        rows.push(("per image", duration(t.per_image)));
        rows.push(("per step", t.step_text()));
        rows.push(("GPU time", format!("{} for {} image(s)", duration(t.gpu_s), t.images)));
        let mut models: Vec<&str> = p.runs.iter().map(|r| r.model.recipe.as_str()).collect();
        models.sort();
        models.dedup();
        if models.len() > 1 {
            let each: Vec<String> = models
                .iter()
                .filter_map(|m| {
                    let t = Timing::of_runs(p.runs.iter().filter(|r| r.model.recipe == *m))?;
                    Some(format!("{m}: {}/image · {} ({})", duration(t.per_image), t.step_text(), t.images))
                })
                .collect();
            if each.len() > 1 {
                rows.push(("by model", each.join("\n")));
            }
        }
    }
    let runs: Vec<_> = st.jobs().into_iter().filter(|j| j.project_id == p.project.id).collect();
    if !runs.is_empty() {
        let (done, total, _, eta) = st.group_progress(&runs.iter().collect::<Vec<_>>());
        let mut now = format!("{done} / {total} seeds");
        if let Some(s) = eta.filter(|s| *s > 0.0) {
            now.push_str(&format!(" · {}", left_text(s)));
        }
        rows.push(("now", now));
    }
    (!rows.is_empty()).then_some(rows)
}

fn rows(ui: &mut egui::Ui, id: &str, rows: Vec<(&str, String)>) {
    egui::Grid::new(id).num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
        for (k, v) in rows {
            ui.weak(k);
            ui.add(egui::Label::new(v).wrap());
            ui.end_row();
        }
    });
}

/// good · neutral · bad · new, coloured.
fn verdict_counts(ui: &mut egui::Ui, [g, n, b, u]: [usize; 4]) {
    ui.horizontal(|ui| {
        ui.weak("kept");
        ui.colored_label(GOOD, format!("{g} good"));
        ui.colored_label(NEUTRAL, format!("{n} neutral"));
        ui.colored_label(BAD, format!("{b} bad"));
    });
    ui.horizontal(|ui| {
        ui.weak("to review");
        ui.colored_label(ACCENT, format!("{u} new"));
    });
}

fn queued(p: &ProjectEntry) -> usize {
    p.runs.iter().map(|r| p.pending_seeds(r).count()).sum()
}

fn images(ui: &mut egui::Ui, root: &Path, tex: &mut Textures, imgs: &[ImageRef], size: f32, out: &mut Vec<Msg>) {
    ui.horizontal_wrapped(|ui| {
        for i in imgs {
            let t = tex.thumb(root, i);
            let r = thumb(ui, t.as_ref(), size, false, None)
                .on_hover_text(format!("{}\ndouble-click: full size", tex.dims_text(root, i)));
            if r.double_clicked() {
                out.push(Msg::Preview(Some(i.clone())));
            }
        }
    });
}

/// A prompt, wrapped, with a button that copies it.
fn prompt(ui: &mut egui::Ui, label: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.weak(label);
        if ui.small_button(ph::COPY).on_hover_text("copy it").clicked() {
            ui.ctx().copy_text(text.to_string());
        }
    });
    ui.add(egui::Label::new(if text.trim().is_empty() { "(empty)" } else { text }).wrap());
}

fn project(ui: &mut egui::Ui, st: &AppState, root: &Path, tex: &mut Textures, p: &ProjectEntry, out: &mut Vec<Msg>) {
    let c = counts(p);
    let cov = cover(p).and_then(|c| tex.thumb(root, &c));
    let size = ui.available_width().min(280.0);
    if let Some(img) = cover(p)
        && thumb(ui, cov.as_ref(), size, false, None).on_hover_text("double-click: full size").double_clicked()
    {
        out.push(Msg::Preview(Some(img)));
    }
    ui.label(RichText::new(p.name()).strong().size(16.0));
    let folder = p.dir.parent().and_then(|d| d.strip_prefix(increart_core::scan::PROJECTS).ok());
    let mut info = vec![
        (
            "folder",
            folder.filter(|f| !f.as_os_str().is_empty()).map_or("(top level)".into(), |f| f.display().to_string()),
        ),
        ("created", when(p.project.created)),
    ];
    if !p.runs.is_empty() {
        info.push(("last run", when(p.last_run())));
    }
    rows(ui, "pp_info", info);

    section(ui, "Counts");
    let generated: usize = p.outputs.len();
    rows(ui, "pp_counts", vec![("runs", p.runs.len().to_string()), ("images", generated.to_string())]);
    verdict_counts(ui, c);
    let q = queued(p);
    if q > 0 {
        let running = p.runs.iter().any(|r| st.active_runs.contains(&r.id));
        ui.horizontal(|ui| {
            ui.weak("queued");
            ui.label(format!("{q} seed(s){}", if running { " · generating" } else { "" }));
        });
    }

    if let Some(t) = project_timing(st, p) {
        section(ui, "Timing");
        rows(ui, "pp_timing", t);
    }

    let sources: Vec<_> = p.project.sources.iter().filter_map(|id| st.idea(id)).collect();
    if !sources.is_empty() {
        section(ui, "Source ideas");
        for e in sources {
            ui.horizontal(|ui| {
                let img = e.idea.images.first().map(|i| ImageRef::input(i.clone()));
                let t = img.as_ref().and_then(|i| tex.thumb(root, i));
                let title = if e.idea.title.is_empty() { "(untitled)".to_string() } else { short(&e.idea.title, 30) };
                let in_library = matches!(e.loc, IdeaLoc::Library(_));
                let tip = if in_library { "open it in the Input library" } else { "not in the Input library" };
                let r = thumb(ui, t.as_ref(), 40.0, false, None).on_hover_text(tip);
                let l = ui.add_enabled(in_library, egui::Link::new(title)).on_hover_text(tip);
                if in_library && (r.clicked() || l.clicked()) {
                    out.extend([
                        Msg::Tab(Tab::InputLibrary),
                        Msg::LibTab(LibTab::Grid),
                        Msg::LibFolder(None),
                        Msg::LibSelect { id: e.idea.id.clone(), additive: false },
                        Msg::LibInfo(Some(e.idea.id.clone())),
                    ]);
                }
            });
        }
    }

    if let Some(r) = st.latest_run(p) {
        section(ui, &format!("Latest run · {}", r.label));
        prompt(ui, "prompt", &r.prompt);
        rows(ui, "pp_latest", settings(r));
        let inputs = inputs(r);
        if !inputs.is_empty() {
            ui.weak("input images");
            images(ui, root, tex, &inputs, 56.0, out);
        }
    }

    if !p.runs.is_empty() {
        section(ui, "Recent runs");
        recent_runs(ui, p, out);
    }

    let best = best(p);
    if !best.is_empty() {
        section(ui, "Best outputs");
        ui.horizontal_wrapped(|ui| {
            for o in best {
                let t = tex.thumb(root, &o.image);
                let color = match o.verdict {
                    Some(Verdict::Good) => GOOD,
                    Some(Verdict::Neutral) => NEUTRAL,
                    Some(Verdict::Bad) => BAD,
                    None => ACCENT,
                };
                let r = thumb(ui, t.as_ref(), 64.0, false, Some(color)).on_hover_text("double-click: full size");
                if r.double_clicked() {
                    out.push(Msg::Preview(Some(o.image.clone())));
                }
            }
        });
    }
}

/// Runs, newest first: label, images made, when. A click shows its
/// outputs; the info button, its details.
fn recent_runs(ui: &mut egui::Ui, p: &ProjectEntry, out: &mut Vec<Msg>) {
    let mut runs: Vec<&Run> = p.runs.iter().collect();
    runs.sort_by(|a, b| b.id.cmp(&a.id));
    egui::Grid::new("pp_runs").num_columns(3).spacing([8.0, 3.0]).striped(true).show(ui, |ui| {
        for r in runs.iter().take(RECENT) {
            let n = p.outputs.iter().filter(|o| o.name.run == r.id).count();
            let label = if r.label.is_empty() { "(no label)" } else { &r.label };
            let tip = format!("{}\nclick: show its images", short(&r.prompt, 120));
            if ui.link(short(label, 22)).on_hover_text(tip).clicked() {
                out.push(Msg::ShowOutputs(OutputSource::Run(r.id.clone())));
            }
            ui.weak(format!("{n} img"));
            ui.horizontal(|ui| {
                ui.weak(r.created.format("%Y-%m-%d").to_string());
                if ui.small_button(ph::INFO).on_hover_text("the run's details").clicked() {
                    out.push(Msg::JobInfo(Some(r.id.clone())));
                }
            });
            ui.end_row();
        }
    });
    if runs.len() > RECENT {
        ui.weak(format!("and {} older (in the tree, with “runs” ticked)", runs.len() - RECENT));
    }
}

/// Its best images: good, then neutral, then new; newest first among equals.
fn best(p: &ProjectEntry) -> Vec<&increart_core::scan::OutputLink> {
    let rank = |v: Option<Verdict>| match v {
        Some(Verdict::Good) => 0,
        Some(Verdict::Neutral) => 1,
        None => 2,
        Some(Verdict::Bad) => 3,
    };
    let mut v: Vec<_> = p.outputs.iter().filter(|o| o.verdict != Some(Verdict::Bad)).collect();
    v.sort_by(|a, b| rank(a.verdict).cmp(&rank(b.verdict)).then(b.image.id.cmp(&a.image.id)));
    v.truncate(BEST);
    v
}

/// A run's references, then its start image when that isn't one of them.
fn inputs(r: &Run) -> Vec<ImageRef> {
    let mut v = r.refs.clone();
    if let Mode::Edit { init: Some(i), .. } = &r.mode
        && !v.contains(i)
    {
        v.push(i.clone());
    }
    v
}

fn settings(r: &Run) -> Vec<(&'static str, String)> {
    let mode = match &r.mode {
        Mode::Generate => "generate".to_string(),
        Mode::Edit { strength, .. } => format!("img2img, strength {strength}"),
    };
    vec![
        ("size", crate::widgets::size_info(r.width, r.height)),
        ("model", r.model.recipe.clone()),
        ("steps / cfg", format!("{} / {}", r.sample.steps, r.sample.cfg)),
        ("mode", mode),
    ]
}

fn project_actions(ui: &mut egui::Ui, st: &AppState, p: &ProjectEntry, out: &mut Vec<Msg>) {
    let id = &p.project.id;
    let u = counts(p)[3];
    if ui.button(ic(ph::MAGIC_WAND, "Generate")).on_hover_text("open it in Generate").clicked() {
        out.push(Msg::OpenInGenerate(id.clone()));
    }
    let review = ui.add_enabled(u > 0, egui::Button::new(ic(ph::EYE, &format!("Review {u}"))));
    if review.on_hover_text("review its new images").clicked() {
        out.push(Msg::ReviewScope(Some(id.clone())));
        out.push(Msg::Tab(Tab::Review));
    }
    crate::review::show_outputs_button(ui, st, OutputSource::Project(id.clone()), out);
    if ui.button(ic(ph::FOLDER_OPEN, "Open folder")).on_hover_text("in the file manager").clicked() {
        out.push(Msg::ShowInFileManager(p.dir.clone()));
    }
}

fn run(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    p: &ProjectEntry,
    r: &Run,
    out: &mut Vec<Msg>,
) {
    ui.label(RichText::new(if r.label.is_empty() { "(no label)" } else { &r.label }).strong().size(16.0));
    ui.horizontal(|ui| {
        ui.weak("project");
        if ui.link(ic(ph::ROCKET_LAUNCH, &p.name())).on_hover_text("select it").clicked() {
            out.push(Msg::SelectProject(p.project.id.clone()));
        }
    });
    rows(ui, "pr_when", vec![("created", when(r.created))]);
    ui.separator();
    prompt(ui, "prompt", &r.prompt);
    if !r.negative_prompt.is_empty() {
        prompt(ui, "negative", &r.negative_prompt);
    }
    section(ui, "Settings");
    rows(ui, "pr_settings", settings(r));
    let inputs = inputs(r);
    if !inputs.is_empty() {
        ui.weak("input images");
        images(ui, root, tex, &inputs, 56.0, out);
    }
    section(ui, "Seeds");
    let made = p.outputs.iter().filter(|o| o.name.run == r.id).count();
    let pending = p.pending_seeds(r).count();
    let cancelled: u64 = r.cancelled.iter().map(|c| c.count).sum();
    let mut seeds = vec![("images", made.to_string()), ("queued", pending.to_string())];
    if cancelled > 0 {
        seeds.push(("cancelled", cancelled.to_string()));
    }
    if !r.failed.is_empty() {
        seeds.push(("failed", r.failed.len().to_string()));
    }
    if let Some(t) = increart_gui_core::timing::Timing::of_run(r) {
        seeds.push(("per image", increart_gui_core::timing::duration(t.per_image)));
    }
    if st.active_runs.contains(&r.id) {
        seeds.push(("now", "generating".into()));
    }
    rows(ui, "pr_seeds", seeds);
    if !r.revisions.is_empty() {
        ui.weak("revisions");
        for rev in &r.revisions {
            let s = &rev.seeds;
            let seeds = match s.count {
                1 => format!("seed {}", s.first),
                n => format!("{n} seeds {}–{}", s.first, s.first + n - 1),
            };
            ui.label(format!("rev {} · {seeds} · {}", rev.rev, rev.created.format("%Y-%m-%d %H:%M")));
        }
    }
}

fn run_actions(ui: &mut egui::Ui, st: &AppState, r: &Run, out: &mut Vec<Msg>) {
    let tip = "a new run in Generate with its prompt and settings";
    if ui.button(ic(ph::MAGIC_WAND, "Open in Generate")).on_hover_text(tip).clicked() {
        out.push(Msg::RunInGenerate(r.id.clone()));
    }
    crate::review::show_outputs_button(ui, st, OutputSource::Run(r.id.clone()), out);
    if ui.button(ic(ph::INFO, "Details")).on_hover_text("timing, sampler, seeds").clicked() {
        out.push(Msg::JobInfo(Some(r.id.clone())));
    }
}

/// Totals for every project under a folder (relative to the library root).
fn folder(ui: &mut egui::Ui, st: &AppState, dir: &Path) {
    let rel = dir.strip_prefix(increart_core::scan::PROJECTS).unwrap_or(dir);
    let name = if rel.as_os_str().is_empty() { "All projects".to_string() } else { rel.display().to_string() };
    ui.label(RichText::new(name).strong().size(16.0));
    let ps: Vec<&ProjectEntry> = st.index.projects.iter().filter(|p| p.dir.starts_with(dir)).collect();
    if ps.is_empty() {
        ui.weak("No projects here.");
        return;
    }
    let mut c = [0; 4];
    for p in &ps {
        for (t, n) in c.iter_mut().zip(counts(p)) {
            *t += n;
        }
    }
    let runs: usize = ps.iter().map(|p| p.runs.len()).sum();
    let images: usize = ps.iter().map(|p| p.outputs.len()).sum();
    let q: usize = ps.iter().map(|p| queued(p)).sum();
    let last = ps.iter().filter(|p| !p.runs.is_empty()).map(|p| p.last_run()).max();
    let mut info = vec![("projects", ps.len().to_string()), ("runs", runs.to_string()), ("images", images.to_string())];
    if q > 0 {
        info.push(("queued", format!("{q} seed(s)")));
    }
    if let Some(t) = last {
        info.push(("last run", when(t)));
    }
    rows(ui, "pf_info", info);
    verdict_counts(ui, c);
    let mut new: Vec<_> = ps.iter().filter(|p| counts(p)[3] > 0).collect();
    if !new.is_empty() {
        section(ui, "New to review");
        new.sort_by_key(|p| std::cmp::Reverse(counts(p)[3]));
        for p in new.iter().take(RECENT) {
            ui.horizontal(|ui| {
                ui.label(short(&p.name(), 28));
                ui.colored_label(ACCENT, format!("{} new", counts(p)[3]));
            });
        }
    }
}

fn folder_actions(ui: &mut egui::Ui, st: &AppState, dir: &Path, out: &mut Vec<Msg>) {
    let rel = dir.strip_prefix(increart_core::scan::PROJECTS).map(Path::to_path_buf).unwrap_or_default();
    if ui.button(ic(ph::ROCKET_LAUNCH, "New project")).on_hover_text("in this folder").clicked() {
        out.push(Msg::NewProject(rel.clone()));
    }
    crate::review::show_outputs_button(ui, st, OutputSource::ProjectFolder(rel.clone()), out);
    if ui.button(ic(ph::FOLDER_OPEN, "Open folder")).on_hover_text("in the file manager").clicked() {
        out.push(Msg::ShowInFileManager(dir.to_path_buf()));
    }
}
