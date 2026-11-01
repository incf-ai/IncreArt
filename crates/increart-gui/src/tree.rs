//! Folder trees with right-click actions (DESIGN.md,
//! Project folders, Input library folders).

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use increart_core::Tree;
use increart_gui_core::{AppState, Confirm, Msg, OutputSource, ProjectsTab, ReviewRun, Tab, counts};

use crate::widgets::{ACCENT, BAD, dialog_buttons, drag_source, drop_target, focus_until_done, ic, ph};

fn children<'a>(folders: &'a [PathBuf], parent: &Path) -> Vec<&'a PathBuf> {
    let mut v: Vec<_> = folders.iter().filter(|f| f.parent() == Some(parent)).collect();
    v.sort_by_key(|f| f.to_string_lossy().to_lowercase());
    v
}

/// Names a folder, project or collection being renamed (from a tree, a
/// folder view, F2, or right after New folder / New project).
pub fn rename_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let Some(r) = &st.renaming else { return };
    let project =
        r.creating || r.tree == Tree::Projects && st.index.projects.iter().any(|p| p.dir == tree_dir(r.tree, &r.path));
    let what = match r.tree {
        _ if project => "project",
        Tree::Collections => "collection",
        _ => "folder",
    };
    let m = egui::Modal::new(egui::Id::unique("rename")).show(ctx, |ui| {
        ui.set_width(400.0);
        let creating = r.creating;
        if creating {
            ui.heading(ic(ph::PLUS, "New project"));
        } else {
            ui.heading(ic(ph::PENCIL_SIMPLE, &format!("Rename {what}")));
        }
        let parent = tree_dir(r.tree, r.path.parent().unwrap_or(Path::new("")));
        ui.weak(format!("In {}/", parent.display().to_string().trim_end_matches('/')));
        let mut name = r.name.clone();
        let resp = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
        focus_until_done(&resp);
        if name != r.name {
            out.push(Msg::EditRename(name.clone()));
        }
        let problem = match increart_core::library::validate_name(name.trim()) {
            Err(e) => Some(e.to_string()),
            Ok(()) if creating && st.project_name_taken(&r.path, name.trim()) => Some("already exists here".into()),
            Ok(()) => None,
        };
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && problem.is_none() {
            out.push(Msg::CommitRename);
        }
        if let Some(e) = &problem {
            ui.colored_label(BAD, e);
        }
        ui.add_space(4.0);
        dialog_buttons(ui, |ui| {
            let ok = egui::Button::new(
                RichText::new(ic(ph::CHECK, if creating { "Create   Enter" } else { "Rename   Enter" })).strong(),
            );
            if ui.add_enabled(problem.is_none(), ok).clicked() {
                out.push(Msg::CommitRename);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::CancelRename);
            }
        });
    });
    if m.should_close() {
        out.push(Msg::CancelRename);
    }
}

/// Where a tree's folder is, relative to the library root.
fn tree_dir(tree: Tree, path: &Path) -> PathBuf {
    Path::new(tree.base()).join(path)
}

fn folder_menu(ui: &mut egui::Ui, st: &AppState, tree: Tree, path: &Path, out: &mut Vec<Msg>) {
    if ui.button(ic(ph::FOLDER_OPEN, "Open in file manager")).clicked() {
        out.push(Msg::ShowInFileManager(tree_dir(tree, path)));
        ui.close();
    }
    if ui.button(ic(ph::FOLDER_PLUS, "New folder")).clicked() {
        out.push(Msg::NewFolder(tree, path.to_path_buf()));
        ui.close();
    }
    if tree == Tree::Projects && ui.button(ic(ph::ROCKET_LAUNCH, "New project")).clicked() {
        out.push(Msg::NewProject(path.to_path_buf()));
        ui.close();
    }
    if tree == Tree::Projects
        && crate::review::show_outputs_button(ui, st, OutputSource::ProjectFolder(path.to_path_buf()), out)
    {
        ui.close();
    }
    if !path.as_os_str().is_empty() {
        if ui.button(ic(ph::PENCIL_SIMPLE, "Rename")).clicked() {
            out.push(Msg::StartRename(tree, path.to_path_buf()));
            ui.close();
        }
        // a project folder asks where; the other trees' to the top level
        if tree == Tree::Projects {
            if ui.button(ic(ph::FOLDER_SIMPLE_DASHED, "Move…")).on_hover_text("to another folder").clicked() {
                out.push(Msg::MoveProject(path.to_path_buf()));
                ui.close();
            }
        } else if path.parent().is_some_and(|p| !p.as_os_str().is_empty())
            && ui.button(ic(ph::ARROW_UP, "Move to the top level")).clicked()
        {
            out.push(Msg::MoveNode(tree, path.to_path_buf(), PathBuf::new()));
            ui.close();
        }
        let what = match tree {
            Tree::InputLibrary => "Delete… (discards its ideas)",
            Tree::Projects => "Delete… (discards its projects)",
            Tree::Collections => "Delete… (to the trash)",
        };
        if ui.button(ic(ph::TRASH, what)).clicked() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let text = format!("Delete folder {name}? {}.", st.delete_summary(tree, path));
            out.push(Msg::Ask(Confirm::new(text, "Delete", Msg::DeleteNode(tree, path.to_path_buf()))));
            ui.close();
        }
    }
}

/// A tree of folders only (the input library: ideas show in the grid).
pub fn folders(
    ui: &mut egui::Ui,
    st: &AppState,
    tree: Tree,
    all: &[PathBuf],
    selected: Option<&Path>,
    out: &mut Vec<Msg>,
    select: impl Fn(&Path) -> Msg + Copy,
) {
    #[allow(clippy::too_many_arguments)]
    fn rec(
        ui: &mut egui::Ui,
        st: &AppState,
        tree: Tree,
        all: &[PathBuf],
        parent: &Path,
        selected: Option<&Path>,
        out: &mut Vec<Msg>,
        select: impl Fn(&Path) -> Msg + Copy,
    ) {
        for f in children(all, parent) {
            ui.horizontal(|ui| {
                ui.add_space(14.0 * (f.components().count() - 1) as f32);
                let name = f.file_name().unwrap_or_default().to_string_lossy();
                let r = ui.selectable_label(selected == Some(f.as_path()), ic(ph::FOLDER, &name));
                if r.clicked() {
                    out.push(select(f));
                }
                r.context_menu(|ui| folder_menu(ui, st, tree, f, out));
            });
            rec(ui, st, tree, all, f, selected, out, select);
        }
    }
    rec(ui, st, tree, all, Path::new(""), selected, out, select);
}

/// The project tree: folders and projects (with unreviewed counts).
pub fn projects(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let r = ui.selectable_label(
        st.proj_folder.as_deref() == Some(Path::new("")),
        ic(ph::FOLDERS, &format!("All projects ({})", st.index.projects.len())),
    );
    drop_target(ui, &r, Path::new("projects"), out);
    if r.clicked() {
        out.push(Msg::SelectProjectFolder(Some(PathBuf::new())));
    }
    r.context_menu(|ui| {
        folder_menu(ui, st, Tree::Projects, Path::new(""), out);
        if !st.index.project_folders.is_empty() {
            ui.separator();
            if ui.button(ic(ph::CARET_RIGHT, "Collapse all folders")).clicked() {
                out.push(Msg::CollapseProjectFolders(true));
                ui.close();
            }
            if ui.button(ic(ph::CARET_DOWN, "Expand all folders")).clicked() {
                out.push(Msg::CollapseProjectFolders(false));
                ui.close();
            }
        }
    });
    let q = st.project_search.trim().to_lowercase();
    if !q.is_empty() && !folder_matches(st, Path::new(""), &q) {
        ui.weak("No project or folder matches.");
    }
    rec(ui, st, Path::new(""), &q, out);

    /// Whether the folder's name or anything under it matches the search
    /// (lowercase).
    fn folder_matches(st: &AppState, f: &Path, q: &str) -> bool {
        let hit = |name: &str| name.to_lowercase().contains(q);
        let dir = Path::new("projects").join(f);
        (!f.as_os_str().is_empty() && hit(&f.file_name().unwrap_or_default().to_string_lossy()))
            || st.index.projects.iter().any(|p| p.dir.starts_with(&dir) && hit(&p.name()))
            || st
                .index
                .project_folders
                .iter()
                .any(|g| g != f && g.starts_with(f) && hit(&g.file_name().unwrap_or_default().to_string_lossy()))
    }

    /// `q`: the search (lowercase); a matching folder shows all it holds.
    /// Collapsed folders hide what they hold, except while searching.
    fn rec(ui: &mut egui::Ui, st: &AppState, parent: &Path, q: &str, out: &mut Vec<Msg>) {
        let depth = parent.components().count() as f32 + 1.0;
        for f in st.project_folder_children(parent) {
            if !q.is_empty() && !folder_matches(st, f, q) {
                continue;
            }
            let own = f.file_name().unwrap_or_default().to_string_lossy().to_lowercase().contains(q);
            let dir = Path::new("projects").join(f);
            let inside: Vec<_> = st.index.projects.iter().filter(|p| p.dir.starts_with(&dir)).collect();
            let has_any = !inside.is_empty() || st.index.project_folders.iter().any(|g| g != f && g.starts_with(f));
            let collapsed = q.is_empty() && has_any && st.folder_collapsed(f);
            ui.horizontal(|ui| {
                // the caret sits in the indent, so names line up with projects
                // (never a negative space: egui carries it on to the next row,
                // each top-level folder a little further left)
                ui.add_space(14.0 * depth - 14.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                if has_any {
                    let caret = if collapsed { ph::CARET_RIGHT } else { ph::CARET_DOWN };
                    let tip = if collapsed { "expand" } else { "collapse" };
                    let b = egui::Button::new(RichText::new(caret).weak()).frame(false).small();
                    if ui.add_sized([14.0, 18.0], b).on_hover_text(tip).clicked() && q.is_empty() {
                        out.push(Msg::ToggleProjectFolder(f.clone()));
                    }
                } else {
                    ui.add_space(14.0);
                }
                ui.spacing_mut().item_spacing.x = 6.0;
                let name = f.file_name().unwrap_or_default().to_string_lossy();
                let icon = if collapsed { ph::FOLDER } else { ph::FOLDER_OPEN };
                let mut label = ic(if has_any { icon } else { ph::FOLDER }, &name);
                // collapsed: what's hidden inside, in short
                let holds_selected = inside.iter().any(|p| st.project.as_ref() == Some(&p.project.id));
                if collapsed {
                    let unreviewed: usize = inside.iter().map(|p| counts(p)[3]).sum();
                    if unreviewed > 0 {
                        label = format!("{label}  ({unreviewed})");
                    }
                    if inside.iter().any(|p| p.runs.iter().any(|r| st.active_runs.contains(&r.id))) {
                        label = format!("{label}  {}", ph::SPARKLE);
                    }
                }
                let text =
                    if collapsed && holds_selected { RichText::new(label).color(ACCENT) } else { RichText::new(label) };
                let r = ui.selectable_label(st.proj_folder.as_deref() == Some(f.as_path()), text);
                let r = if collapsed && holds_selected { r.on_hover_text("the open project is in here") } else { r };
                let r = drag_source(&r, &dir, || vec![dir.clone()]);
                drop_target(ui, &r, &dir, out);
                if r.clicked() {
                    out.push(Msg::SelectProjectFolder(Some(f.clone())));
                }
                if r.double_clicked() && q.is_empty() && has_any {
                    out.push(Msg::ToggleProjectFolder(f.clone()));
                }
                r.context_menu(|ui| folder_menu(ui, st, Tree::Projects, f, out));
            });
            if !collapsed {
                rec(ui, st, f, if own { "" } else { q }, out);
            }
        }
        let dir = Path::new("projects").join(parent);
        let mut ps: Vec<_> = st
            .index
            .projects
            .iter()
            .filter(|p| p.dir.parent() == Some(dir.as_path()) && p.name().to_lowercase().contains(q))
            .collect();
        // newest first or by name (folders stay by name)
        st.sort_projects(&mut ps);
        for p in ps {
            let Ok(rel) = p.dir.strip_prefix("projects").map(Path::to_path_buf) else { continue };
            ui.horizontal(|ui| {
                ui.add_space(14.0 * depth + 2.0);
                let [_, _, _, unreviewed] = counts(p);
                let busy = p.runs.iter().any(|r| st.active_runs.contains(&r.id));
                let mut label = ic(ph::ROCKET_LAUNCH, &p.name());
                if unreviewed > 0 {
                    label = format!("{label}  ({unreviewed})");
                }
                if busy {
                    label = format!("{label}  {}", ph::SPARKLE);
                }
                let selected = st.project.as_ref() == Some(&p.project.id);
                let text = if selected { RichText::new(label).color(ACCENT) } else { RichText::new(label) };
                let r = ui.selectable_label(selected, text);
                let r = drag_source(&r, &p.dir, || vec![p.dir.clone()]);
                if r.clicked() {
                    out.push(Msg::SelectProject(p.project.id.clone()));
                }
                r.context_menu(|ui| {
                    if ui.button(ic(ph::MAGIC_WAND, "Generate")).on_hover_text("its Generate tab").clicked() {
                        out.push(Msg::SelectProject(p.project.id.clone()));
                        out.push(Msg::Tab(Tab::Projects));
                        out.push(Msg::ProjectsTab(ProjectsTab::Generate));
                        ui.close();
                    }
                    if crate::review::show_outputs_button(ui, st, OutputSource::Project(p.project.id.clone()), out) {
                        ui.close();
                    }
                    if ui.button(ic(ph::PENCIL_SIMPLE, "Rename")).clicked() {
                        out.push(Msg::StartRename(Tree::Projects, rel.clone()));
                        ui.close();
                    }
                    if ui
                        .button(ic(ph::COPY, "Duplicate"))
                        .on_hover_text("settings, sources and draft; no images")
                        .clicked()
                    {
                        out.push(Msg::DuplicateProject(rel.clone()));
                        ui.close();
                    }
                    if ui.button(ic(ph::FOLDER_SIMPLE_DASHED, "Move…")).on_hover_text("to another folder").clicked() {
                        out.push(Msg::MoveProject(rel.clone()));
                        ui.close();
                    }
                    if ui.button(ic(ph::FOLDER_OPEN, "Open in file manager")).clicked() {
                        out.push(Msg::ShowInFileManager(p.dir.clone()));
                        ui.close();
                    }
                    if ui.button(ic(ph::TRASH, "Discard…")).clicked() {
                        let text = format!(
                            "Discard project {}? {}. Restore it from Projects → Discarded.",
                            p.name(),
                            st.delete_summary(Tree::Projects, &rel)
                        );
                        out.push(Msg::Ask(Confirm::new(text, "Discard", Msg::DeleteNode(Tree::Projects, rel.clone()))));
                        ui.close();
                    }
                });
            });
            if st.project.as_ref() == Some(&p.project.id) && !st.config.hide_project_runs {
                runs(ui, st, p, depth + 1.0, out);
            }
        }
    }
}

/// The selected project's runs, newest first, a sweep once (as in Review's
/// scope): click for the run's details (a sweep's newest run), right-click
/// for Review, its kept images, Generate, history.
fn runs(ui: &mut egui::Ui, st: &AppState, p: &increart_core::scan::ProjectEntry, depth: f32, out: &mut Vec<Msg>) {
    let rows = st.review_runs(p);
    if rows.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(14.0 * depth);
            ui.weak("no runs yet");
        });
    }
    for row in rows {
        let (run, label, unreviewed, review) = match row {
            ReviewRun::Single(r) => {
                let label = if r.label.is_empty() { r.id.to_string() } else { r.label.clone() };
                let review = Msg::ReviewRun(p.project.id.clone(), r.id.clone());
                (Some(r), ic(ph::IMAGES, &label), st.run_unreviewed(p, &r.id), review)
            }
            ReviewRun::Sweep(name) => {
                let review = Msg::ReviewSweep(p.project.id.clone(), name.to_string());
                (
                    st.sweep_newest(p, name),
                    ic(ph::TABLE, &format!("sweep {name}")),
                    st.sweep_unreviewed(p, name),
                    review,
                )
            }
        };
        let Some(run) = run else { continue };
        let sweep = matches!(row, ReviewRun::Sweep(_));
        let ids: Vec<_> = match row {
            ReviewRun::Single(r) => vec![r.id.clone()],
            ReviewRun::Sweep(name) => p
                .runs
                .iter()
                .filter(|r| r.sweep.as_ref().is_some_and(|s| s.name == name))
                .map(|r| r.id.clone())
                .collect(),
        };
        ui.horizontal(|ui| {
            ui.add_space(14.0 * depth);
            let mut label = label;
            if unreviewed > 0 {
                label = format!("{label}  ({unreviewed})");
            }
            if ids.iter().any(|id| st.active_runs.contains(id)) {
                label = format!("{label}  {}", ph::SPARKLE);
            }
            let open = st.job_info.as_ref().is_some_and(|id| ids.contains(id));
            let text = if unreviewed == 0 { RichText::new(label).weak() } else { RichText::new(label) };
            let tip = if sweep { "the sweep's newest run's details" } else { "the run's details" };
            let r = ui.selectable_label(open, text).on_hover_text(tip);
            if r.clicked() {
                out.push(Msg::JobInfo(Some(run.id.clone())));
            }
            r.context_menu(|ui| {
                let what = if sweep { "Review the contact sheet" } else { "Review this run" };
                if ui.button(ic(ph::THUMBS_UP, what)).clicked() {
                    out.push(Msg::Tab(Tab::Review));
                    out.push(review.clone());
                    ui.close();
                }
                if !sweep && crate::review::show_outputs_button(ui, st, OutputSource::Run(run.id.clone()), out) {
                    ui.close();
                }
                if ui.button(ic(ph::INFO, "Details")).clicked() {
                    out.push(Msg::JobInfo(Some(run.id.clone())));
                    ui.close();
                }
                if ui.button(ic(ph::ARROW_SQUARE_OUT, "Open in Generate")).clicked() {
                    out.push(Msg::RunInGenerate(run.id.clone()));
                    ui.close();
                }
                if ui.button(ic(ph::CLOCK_COUNTER_CLOCKWISE, "Show in history")).clicked() {
                    out.push(Msg::RunHistory(run.id.clone()));
                    ui.close();
                }
            });
        });
    }
}
