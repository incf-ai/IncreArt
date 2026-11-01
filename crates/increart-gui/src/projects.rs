//! Projects: the tree, Grid (one tile per project), Generate and Jobs.

use std::path::{Path, PathBuf};

use eframe::egui::{self, ComboBox, Panel, RichText, ScrollArea};
use increart_core::config::TreeOrder;
use increart_core::engine::Stop;
use increart_core::{Tree, Verdict};
use increart_gui_core::{AppState, Confirm, Msg, OutputSource, PickKind, ProjectsTab, Tab, UseTarget, counts, cover};

use crate::folder_pick;
use crate::textures::Textures;
use crate::tree;
use crate::widgets::{ACCENT, BAD, GOOD, NEUTRAL, card, dialog_buttons, dialog_footer, ic, ph, short, thumb};

pub fn view(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    Panel::left("proj_tree").default_size(250.0).show(ui, |ui| {
        Panel::bottom("proj_keys").show(ui, |ui| key_hints(ui, st.projects_tab));
        // right to left: the clear button, then the field takes what's left
        let row = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut q = st.project_search.clone();
            if !q.is_empty() && ui.small_button(ph::X).on_hover_text("clear").clicked() {
                out.push(Msg::ProjectSearch(String::new()));
            }
            let field = egui::TextEdit::singleline(&mut q)
                .hint_text(ic(ph::MAGNIFYING_GLASS, "Search projects"))
                .desired_width(ui.available_width());
            let r = ui.add(field);
            if r.changed() {
                out.push(Msg::ProjectSearch(q.clone()));
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                out.push(Msg::ProjectSearch(String::new()));
            }
        });
        ui.horizontal_wrapped(|ui| {
            let mut show = !st.config.hide_project_runs;
            if ui.checkbox(&mut show, "runs").on_hover_text("list the selected project's runs under it").changed() {
                out.push(Msg::ProjectRuns(show));
            }
            let mut narrow = !st.config.project_click_keeps_scope;
            let tip = "selecting a project shows its folder in the grid, when that holds other projects \
                       (else the grid stays as it is)";
            if ui.checkbox(&mut narrow, "narrow to siblings").on_hover_text(tip).changed() {
                out.push(Msg::NarrowToSiblings(narrow));
            }
        });
        ui.horizontal_wrapped(|ui| {
            crate::widgets::group(ui, "sort", |ui| {
                ui.label(ic(ph::SORT_ASCENDING, "Sort"));
                let tip = "the order of projects in the tree and the grid: Last run goes by when seeds were \
                           last queued (a project that has never run, by when it was made)";
                if let Some(o) = order_combo(ui, "project_order", "projects", st.config.project_order, tip) {
                    out.push(Msg::ProjectOrder(o));
                }
            });
            let tip = "the order of folders in the tree: Created and Last run go by the newest project in them";
            if let Some(o) = order_combo(ui, "folder_order", "folders", st.config.folder_order.0, tip) {
                out.push(Msg::FolderOrder(o));
            }
        });
        ui.horizontal(|ui| {
            // at the top level; a folder's right-click menu makes them in it
            let tip = "at the top level (right-click a folder to make one in it)";
            if ui.button(ic(ph::ROCKET_LAUNCH, "New project")).on_hover_text(tip).clicked() {
                out.push(Msg::NewProject(PathBuf::new()));
            }
            if ui.button(ic(ph::FOLDER_PLUS, "New folder")).on_hover_text(tip).clicked() {
                out.push(Msg::NewFolder(Tree::Projects, PathBuf::new()));
            }
        });
        ui.separator();
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| tree::projects(ui, st, out));
    });
    Panel::top("proj_tabs").show(ui, |ui| {
        ui.horizontal(|ui| {
            for (t, label) in [
                (ProjectsTab::Grid, ic(ph::SQUARES_FOUR, "Grid")),
                (ProjectsTab::Folder, ic(ph::FOLDER, "Folder")),
                (ProjectsTab::Generate, ic(ph::MAGIC_WAND, "Generate")),
                (ProjectsTab::Jobs, ic(ph::QUEUE, &format!("Jobs ({})", st.active_runs.len()))),
                (ProjectsTab::Discarded, ic(ph::TRASH, &format!("Discarded ({})", st.index.discarded_projects.len()))),
            ] {
                if ui.selectable_label(st.projects_tab == t, label).clicked() {
                    out.push(Msg::ProjectsTab(t));
                }
            }
            if matches!(st.projects_tab, ProjectsTab::Grid | ProjectsTab::Folder) {
                crate::widgets::right_group(ui, "right", |ui| {
                    crate::widgets::details_toggle(ui, st, increart_gui_core::InfoPanel::Projects, out)
                });
            }
        });
    });
    if matches!(st.projects_tab, ProjectsTab::Grid | ProjectsTab::Folder)
        && !st.info_hidden.contains(&increart_gui_core::InfoPanel::Projects)
    {
        crate::project_pane::pane(ui, st, tex, out);
    }
    egui::CentralPanel::default().show(ui, |ui| match st.projects_tab {
        ProjectsTab::Grid => grid(ui, st, tex, out),
        ProjectsTab::Folder => crate::folderview::view(ui, st, tex, out),
        ProjectsTab::Generate => generate(ui, st, tex, out),
        ProjectsTab::Jobs => jobs(ui, st, out),
        ProjectsTab::Discarded => discarded(ui, st, tex, out),
    });
}

/// Projects → Discarded: discarded projects, newest first, each with where
/// it was; Restore, Delete permanently, and Empty discarded.
fn discarded(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let Some(root) = st.library.clone() else { return };
    let ds = &st.index.discarded_projects;
    if ds.is_empty() {
        ui.weak("No discarded projects. A project's Discard… (right-click it) puts it here, to restore later.");
        return;
    }
    ui.horizontal(|ui| {
        ui.weak("Restore puts a project back in its folder (the top level if that's gone); its pending seeds generate again.");
        crate::widgets::right_group(ui, "empty_discarded", |ui| {
            if ui.button(ic(ph::TRASH, "Empty discarded…")).clicked() {
                let images: usize = ds.iter().map(|d| d.entry.outputs.len()).sum();
                let text = format!(
                    "Delete the {} discarded project(s) permanently? Their folders go to the trash with {images} image link(s); the images stay in the store until Clean up.",
                    ds.len()
                );
                out.push(Msg::Ask(Confirm::new(text, "Delete", Msg::EmptyDiscardedProjects)));
            }
        });
    });
    ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for d in ds {
                let p = &d.entry;
                let id = &p.project.id;
                let [g, n, b, u] = counts(p);
                let cov = cover(p).and_then(|c| tex.thumb(&root, &c));
                let from = Path::new("projects").join(&d.discarded.from);
                let r = card(ui, false, 200.0, 290.0, |ui| {
                    let r = thumb(ui, cov.as_ref(), 200.0, false, None);
                    if cov.is_none() {
                        scattered_inputs(ui, r.rect, id, &root, tex, p);
                    }
                    ui.label(RichText::new(short(&d.discarded.name, 26)).strong());
                    ui.horizontal(|ui| {
                        ui.colored_label(GOOD, g.to_string());
                        ui.colored_label(NEUTRAL, n.to_string());
                        ui.colored_label(BAD, b.to_string());
                        ui.colored_label(ACCENT, format!("{u} new"));
                    });
                    ui.weak(format!("from {}", from.display()));
                    ui.weak(format!("discarded {}", crate::widgets::when(d.discarded.discarded)));
                });
                let restore = || Msg::RestoreProject(id.clone());
                let delete = || {
                    let text = format!(
                        "Delete project {} permanently? Its folder goes to the trash with {} image link(s); the images stay in the store until Clean up.",
                        d.discarded.name,
                        p.outputs.len()
                    );
                    Msg::Ask(Confirm::new(text, "Delete", Msg::DeleteDiscardedProject(id.clone())))
                };
                if ui.rect_contains_pointer(r.rect) {
                    let cover = egui::Rect::from_min_size(r.rect.min + egui::vec2(7.0, 7.0), egui::vec2(200.0, 200.0));
                    let buttons = [
                        (ph::ARROW_COUNTER_CLOCKWISE, "Restore", "back to its folder"),
                        (ph::TRASH, "Delete", "permanently (asks first)"),
                    ];
                    match crate::widgets::cover_buttons(ui, cover, &buttons) {
                        Some(0) => out.push(restore()),
                        Some(_) => out.push(delete()),
                        None => {}
                    }
                }
                r.context_menu(|ui| {
                    if ui.button(ic(ph::ARROW_COUNTER_CLOCKWISE, "Restore")).clicked() {
                        out.push(restore());
                        ui.close();
                    }
                    if ui.button(ic(ph::FOLDER_OPEN, "Open in file manager")).clicked() {
                        out.push(Msg::ShowInFileManager(p.dir.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(ic(ph::TRASH, "Delete permanently…")).clicked() {
                        out.push(delete());
                        ui.close();
                    }
                });
            }
        });
    });
}

/// The Projects tab's keys, folded away at the bottom of the project tree;
/// the Folder tab's own, else those of an opened image.
fn key_hints(ui: &mut egui::Ui, tab: ProjectsTab) {
    const FOLDER: &[(&str, &str)] = &[
        ("Enter", "open"),
        ("Backspace", "up a folder"),
        ("Alt+← →", "back / forward"),
        ("arrows", "move (Shift: select)"),
        ("Ctrl+A", "select all"),
        ("Ctrl+C X V", "copy / cut / paste"),
        ("F2", "rename"),
        ("Delete", "discard (projects) / trash"),
        ("Ctrl+H", "show .rsn files"),
        ("Ctrl+L", "type a path"),
        ("Esc", "clear the selection"),
    ];
    const IMAGE: &[(&str, &str)] = &[
        ("← →", "previous / next"),
        ("1 / G", "good"),
        ("2 / N", "neutral"),
        ("3 / B", "bad"),
        ("4 / Ctrl+Z", "undo"),
        ("E", "edit this image"),
        ("D", "duplicate its project"),
        ("R", "→ input library"),
        ("I", "→ new idea"),
        ("+ − 0", "zoom / fit"),
        ("Esc", "close"),
    ];
    // (the folder's Alt+← → are its own back / forward)
    const ANYWHERE: &[(&str, &str)] = &[("Alt+← →", "back / forward"), ("Ctrl+N", "new idea")];
    const FOLDER_ANYWHERE: &[(&str, &str)] = &[("Ctrl+N", "new idea")];
    let groups = match tab {
        ProjectsTab::Folder => [("In the folder", FOLDER), ("Anywhere", FOLDER_ANYWHERE)],
        _ => [("In an opened image", IMAGE), ("Anywhere", ANYWHERE)],
    };
    egui::CollapsingHeader::new(RichText::new(ic(ph::KEYBOARD, "Keys")).strong())
        .id_salt("proj_keys")
        .default_open(true)
        .show(ui, |ui| {
            for (heading, keys) in groups {
                ui.weak(RichText::new(heading).small());
                for &(k, what) in keys {
                    ui.horizontal(|ui| {
                        ui.monospace(k);
                        ui.weak(what);
                    });
                }
            }
        });
}

/// A cover with no output image yet: the project's references and start
/// images as small thumbnails scattered around it, clear of the empty-image
/// icon in the middle. Each project keeps its own scatter.
fn scattered_inputs(
    ui: &egui::Ui,
    cover: egui::Rect,
    id: &increart_core::Id,
    root: &Path,
    tex: &mut Textures,
    p: &increart_core::scan::ProjectEntry,
) {
    const SIZE: f32 = 50.0;
    // top-left corners, in a 200 × 200 cover; the middle 60 × 60 stays clear
    const SLOTS: [(f32, f32); 8] = [
        (12.0, 12.0),
        (138.0, 14.0),
        (14.0, 138.0),
        (136.0, 136.0),
        (75.0, 8.0),
        (8.0, 75.0),
        (142.0, 75.0),
        (75.0, 142.0),
    ];
    let scale = cover.width() / 200.0;
    let painter = ui.painter_at(cover);
    for (i, (image, (x, y))) in increart_gui_core::project_inputs(p).iter().zip(SLOTS).enumerate() {
        // a small nudge per project and slot, so it looks scattered
        let h = id
            .as_str()
            .bytes()
            .chain(image.id.as_str().bytes())
            .fold(i as u32 * 7919, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        let jitter = |k: u32| ((h >> k) % 9) as f32 - 4.0;
        let at = cover.min + egui::vec2(x + jitter(0), y + jitter(8)) * scale;
        let rect = egui::Rect::from_min_size(at, egui::Vec2::splat(SIZE * scale));
        painter.rect_filled(rect.translate(egui::vec2(2.0, 2.0)), 3.0, egui::Color32::from_black_alpha(90));
        painter.rect_filled(rect, 3.0, egui::Color32::from_gray(55));
        crate::widgets::image_cover(&painter, rect.shrink(2.0), tex.thumb(root, image).as_ref());
    }
}

fn grid(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let Some(root) = st.library.clone() else { return };
    breadcrumb(ui, st, out);
    let ps = st.projects_in_scope();
    if ps.is_empty() {
        ui.weak("No projects here. Make one from ideas, or right-click the tree → New project.");
        return;
    }
    ui.weak("Click to select (it becomes the current project); Review / Generate on the tile; double-click: review, or generate if nothing is new; right-click for more.");
    ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for p in ps {
                let selected = st.project.as_ref() == Some(&p.project.id);
                let [g, n, b, u] = counts(p);
                let cov = cover(p).and_then(|c| tex.thumb(&root, &c));
                let r = card(ui, selected, 200.0, 270.0, |ui| {
                    let badge = (u > 0).then_some(ACCENT);
                    let r = thumb(ui, cov.as_ref(), 200.0, false, badge);
                    if cov.is_none() {
                        scattered_inputs(ui, r.rect, &p.project.id, &root, tex, p);
                    }
                    ui.label(RichText::new(short(&p.name(), 26)).strong());
                    ui.horizontal(|ui| {
                        ui.colored_label(GOOD, g.to_string());
                        ui.colored_label(NEUTRAL, n.to_string());
                        ui.colored_label(BAD, b.to_string());
                        ui.colored_label(ACCENT, format!("{u} new"));
                        if p.runs.iter().any(|r| st.active_runs.contains(&r.id)) {
                            ui.label(ph::SPARKLE);
                        }
                    });
                    ui.weak(p.dir.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default());
                });
                // drag onto a folder in the tree to move it
                let r = crate::widgets::drag_source(&r, &p.dir, || vec![p.dir.clone()]);
                if r.clicked() {
                    out.push(Msg::SelectProject(p.project.id.clone()));
                }
                // Double-click: Review if there is something to review, else Generate.
                if r.double_clicked() {
                    if u > 0 {
                        out.push(Msg::ReviewScope(Some(p.project.id.clone())));
                        out.push(Msg::Tab(Tab::Review));
                    } else {
                        out.push(Msg::OpenInGenerate(p.project.id.clone()));
                    }
                }
                // The selected (or hovered) tile shows its two actions on the
                // cover. Placed after the tile's click area, so they're on top
                // and get the clicks.
                if selected || ui.rect_contains_pointer(r.rect) {
                    let cover = egui::Rect::from_min_size(r.rect.min + egui::vec2(7.0, 7.0), egui::vec2(200.0, 200.0));
                    let y = cover.bottom() - 34.0;
                    let review = egui::Rect::from_min_size(egui::pos2(cover.left() + 6.0, y), egui::vec2(92.0, 28.0));
                    let generate =
                        egui::Rect::from_min_size(egui::pos2(cover.left() + 102.0, y), egui::vec2(92.0, 28.0));
                    let fill = crate::widgets::overlay_button;
                    let rv = crate::widgets::overlay(
                        ui,
                        review,
                        fill(egui::Button::new(ic(ph::EYE, &format!("Review {u}")))),
                    );
                    if rv.on_hover_text("review this project's new images").clicked() {
                        out.push(Msg::ReviewScope(Some(p.project.id.clone())));
                        out.push(Msg::Tab(Tab::Review));
                    }
                    if selected {
                        // where the button is, for the screenshot tour's click check
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("tour_generate_button"), generate));
                    }
                    if crate::widgets::overlay(ui, generate, fill(egui::Button::new(ic(ph::MAGIC_WAND, "Generate"))))
                        .clicked()
                    {
                        out.push(Msg::OpenInGenerate(p.project.id.clone()));
                    }
                    // top left: the top right has the "new images" dot
                    let outputs = egui::Rect::from_min_size(cover.min + egui::vec2(6.0, 6.0), egui::vec2(28.0, 28.0));
                    let ov = crate::widgets::overlay(ui, outputs, fill(egui::Button::new(ph::IMAGES)));
                    if ov.on_hover_text(format!("show its outputs ({} kept)", g + n)).clicked() {
                        out.push(Msg::ShowOutputs(OutputSource::Project(p.project.id.clone())));
                    }
                }
                r.context_menu(|ui| project_menu(ui, st, p, out));
            }
        });
    });
}

/// Right-click on a project tile: generate more of its latest run, open it,
/// review it, and the tree's actions.
fn project_menu(ui: &mut egui::Ui, st: &AppState, p: &increart_core::scan::ProjectEntry, out: &mut Vec<Msg>) {
    let id = &p.project.id;
    if let Some(run) = st.latest_run(p) {
        ui.label(RichText::new(format!("Generate more of {}", run.label)).strong());
        ui.weak(short(&run.prompt, 50));
        ui.horizontal(|ui| {
            for n in [4, 8, 16] {
                if ui.button(format!("+{n} seeds")).clicked() {
                    out.push(Msg::MoreSeeds(run.id.clone(), n));
                    ui.close();
                }
            }
        });
        if !st.engine_running {
            ui.weak("no servers: they run once servers are connected");
        }
        ui.separator();
    }
    if ui.button(ic(ph::MAGIC_WAND, "Open in Generate")).clicked() {
        out.push(Msg::OpenInGenerate(id.clone()));
        ui.close();
    }
    if counts(p)[3] > 0 && ui.button(ic(ph::EYE, "Review its images")).clicked() {
        out.push(Msg::ReviewScope(Some(id.clone())));
        out.push(Msg::Tab(Tab::Review));
        ui.close();
    }
    if crate::review::show_outputs_button(ui, st, OutputSource::Project(id.clone()), out) {
        ui.close();
    }
    ui.separator();
    let rel = p.dir.strip_prefix("projects").map(std::path::Path::to_path_buf).unwrap_or_default();
    if ui.button(ic(ph::COPY, "Duplicate")).clicked() {
        out.push(Msg::DuplicateProject(rel.clone()));
        ui.close();
    }
    if ui.button(ic(ph::PENCIL_SIMPLE, "Rename")).clicked() {
        out.push(Msg::SelectProject(id.clone()));
        out.push(Msg::StartRename(Tree::Projects, rel.clone()));
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
        out.push(Msg::Ask(Confirm::new(text, "Discard", Msg::DeleteNode(Tree::Projects, rel))));
        ui.close();
    }
}

/// "All projects › clients › acme": the Grid's scope; click a part to go up.
fn breadcrumb(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let scope = st.project_scope();
    ui.horizontal(|ui| {
        if ui.link(ic(ph::FOLDERS, "All projects")).clicked() {
            out.push(Msg::SelectProjectFolder(Some(Default::default())));
        }
        let mut acc = std::path::PathBuf::new();
        for c in scope.components() {
            acc.push(c);
            ui.weak("›");
            if ui.link(c.as_os_str().to_string_lossy()).clicked() {
                out.push(Msg::SelectProjectFolder(Some(acc.clone())));
            }
        }
        ui.weak(format!("· {} project(s) here and below", st.projects_in_scope().len()));
        crate::review::show_outputs_button(ui, st, OutputSource::ProjectFolder(scope.clone()), out);
    });
}

/// The Generate form's images: one list, each image a reference
/// (`<imageN>`, numbered in list order), the img2img start image (at most
/// one), both, or parked (neither: kept, not sent).
fn images_pane(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &std::path::Path,
    tex: &mut Textures,
    f: &mut increart_gui_core::GenerateForm,
    out: &mut Vec<Msg>,
) {
    let refs = f.refs();
    ui.horizontal(|ui| {
        ui.heading(ic(ph::IMAGES, "Images"));
        let text = format!("{} / {} references", refs.len(), increart_gui_core::MAX_REFS);
        if refs.len() > increart_gui_core::MAX_REFS {
            ui.colored_label(BAD, format!("{text}: more than Qwen-Image 2.1 takes"));
        } else {
            ui.weak(text);
        }
    });
    ui.weak("<imageN>: a reference, numbered in this order. start: the img2img start image (one). Neither: parked, not sent.");
    ui.horizontal(|ui| {
        if ui.button(ic(ph::FILE_PLUS, "Add file…")).clicked() {
            out.push(Msg::Pick(PickKind::GenerateRefs));
        }
        if ui.button(ic(ph::IMAGES, "From library…")).on_hover_text("browse the input and output libraries").clicked()
        {
            out.push(Msg::RefPicker(true));
        }
        if ui
            .add_enabled(!f.images.is_empty(), egui::Button::new(ic(ph::SQUARES_FOUR, "Tile…")))
            .on_hover_text("all these images (parked ones too), tiled in this order")
            .on_disabled_hover_text("no images in Generate")
            .clicked()
        {
            out.push(Msg::TileGenerateImages);
        }
    });
    ui.weak("or drop image files here");
    // what the prompt names vs what's there
    let tags: std::collections::BTreeSet<usize> =
        increart_gui_core::image_tags(&f.prompt).into_iter().map(|(_, n)| n).collect();
    for n in tags.iter().filter(|n| **n == 0 || **n > refs.len()) {
        let why = if refs.is_empty() && f.init().is_some() {
            " (the start image isn't a reference: turn on its <imageN>)"
        } else {
            ""
        };
        ui.colored_label(BAD, format!("The prompt names <image{n}>, but there are {} reference(s){why}", refs.len()));
    }
    // a sweep's prompts can name what the prompt doesn't
    let named = increart_gui_core::update::named_image_tags(st);
    ui.separator();
    if f.images.is_empty() && f.swept().is_empty() {
        ui.weak("No images: the run is text-to-image.");
    }
    swept_placeholder(ui, st, root, tex, f, out);
    let n = f.images.len();
    ScrollArea::vertical().auto_shrink([false, true]).id_salt("gen_images_scroll").show(ui, |ui| {
        for i in 0..n {
            let x = f.images[i].clone();
            let tag = f.tag_of(i);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let t = tex.thumb(root, &x.image);
                    let resp = thumb(ui, t.as_ref(), 84.0, x.start, None);
                    if !x.reference && !x.start {
                        ui.painter().rect_filled(resp.rect, 2.0, egui::Color32::from_black_alpha(150));
                    }
                    if tag == Some(1) {
                        // where it is, for the screenshot tour's double-click check
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("tour_first_ref"), resp.rect));
                    }
                    let resp = resp.on_hover_text("double-click to view · right-click to copy");
                    if resp.double_clicked() {
                        out.push(Msg::Preview(Some(x.image.clone())));
                    }
                    resp.context_menu(|ui| {
                        crate::widgets::copy_image_item(ui, root, &x.image, out);
                        crate::widgets::markup_item(ui, &x.image, Some(i), out);
                        crate::widgets::describe_item(ui, &x.image, "Describe with PE…", out);
                        if ui.button(ic(ph::ARROWS_OUT, "View large")).clicked() {
                            out.push(Msg::Preview(Some(x.image.clone())));
                            ui.close();
                        }
                    });
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.weak(tex.dims_text(root, &x.image));
                            if let Some([w, h]) = tex.dims(root, &x.image) {
                                let fit = increart_gui_core::fit_size(w, h);
                                let tip = if fit == (w, h) {
                                    format!("set the output size to {} × {}", fit.0, fit.1)
                                } else {
                                    format!(
                                        "set the output size to {} × {}: this image's aspect, \
                                         in multiples of 32 within 256–2048",
                                        fit.0, fit.1
                                    )
                                };
                                let r = ui.add_enabled(
                                    fit != (f.width, f.height),
                                    egui::Button::new(ic(ph::ARROWS_OUT_SIMPLE, "use size")).small(),
                                );
                                if r.on_hover_text(tip)
                                    .on_disabled_hover_text("the output size already matches")
                                    .clicked()
                                {
                                    (f.width, f.height) = fit;
                                }
                            }
                        });
                        if let Some(u) = st.usage(&x.image).label() {
                            ui.weak(RichText::new(u).small());
                        }
                        ui.horizontal(|ui| {
                            let label = tag.map_or("<imageN>".to_string(), |k| format!("<image{k}>"));
                            if ui
                                .selectable_label(x.reference, label)
                                .on_hover_text("a reference the prompt can name (Qwen-Image's editing)")
                                .clicked()
                            {
                                out.push(Msg::ImageRoles { index: i, reference: !x.reference, start: x.start });
                            }
                            if ui
                                .selectable_label(x.start, "start")
                                .on_hover_text("the img2img start image: the run starts from it, keeping its layout")
                                .clicked()
                            {
                                out.push(Msg::ImageRoles { index: i, reference: x.reference, start: !x.start });
                            }
                        });
                        ui.horizontal(|ui| {
                            if let Some(k) = tag
                                && ui
                                    .small_button(ph::TEXT_T)
                                    .on_hover_text(format!("insert <image{k}> into the prompt"))
                                    .clicked()
                            {
                                let (g, end) = with_tag(f, &format!("<image{k}>"), crate::presets::selection(ui.ctx()));
                                *f = g;
                                crate::presets::focus_prompt_at(ui.ctx(), end);
                            }
                            let tip = if st.edited_source(&x.image).is_some() {
                                "crop / mark… again: the copy is replaced"
                            } else {
                                "crop / mark…: the marked copy goes in below"
                            };
                            if ui.small_button(ph::PENCIL_SIMPLE).on_hover_text(tip).clicked() {
                                out.push(Msg::OpenMarkup(x.image.clone(), Some(i)));
                            }
                            if ui
                                .add_enabled(i > 0, egui::Button::new(ph::ARROW_UP).small())
                                .on_hover_text("move up")
                                .clicked()
                            {
                                out.push(Msg::MoveImage { from: i, to: i - 1 });
                            }
                            if ui
                                .add_enabled(i + 1 < n, egui::Button::new(ph::ARROW_DOWN).small())
                                .on_hover_text("move down")
                                .clicked()
                            {
                                out.push(Msg::MoveImage { from: i, to: i + 1 });
                            }
                            if ui.small_button(ph::X).on_hover_text("remove from the list").clicked() {
                                out.push(Msg::RemoveImage(i));
                            }
                        });
                        if x.reference && !named.contains(&tag.unwrap_or(0)) {
                            ui.weak(if f.sweep.on {
                                "not named in the prompt or the sweep"
                            } else {
                                "not named in the prompt"
                            });
                        }
                        if !x.reference && !x.start {
                            ui.weak("parked: not sent");
                        }
                    });
                });
                if x.start {
                    // shown as how much of the start image to keep; sd-server
                    // gets strength = 1 − keep (how much is redrawn)
                    let mut keep = ((1.0 - f.strength) * 100.0).round();
                    let r = ui
                        .add(egui::Slider::new(&mut keep, 0.0..=95.0).step_by(5.0).suffix(" %").text("keep"))
                        .on_hover_text(
                            "How much of the start image the result keeps.\n\
                             90 %: barely changed · 50 %: same layout, details redrawn · 0 %: start image ignored\n\
                             (sent to sd-server as strength = 1 − keep)",
                        );
                    if r.changed() {
                        f.strength = 1.0 - keep / 100.0;
                    }
                    // measured: the same image as start + <imageN> comes back nearly unchanged
                    // whatever the prompt asks, even at strength 0.75
                    ui.colored_label(
                        NEUTRAL,
                        "With a start image the result stays close to it, whatever the prompt asks. \
                         For instruction edits (remove, change, add …) untick start and keep <imageN>.",
                    );
                    if let Some([w, h]) = tex.dims(root, &x.image)
                        && (w, h) != (f.width, f.height)
                    {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(format!("resized to {} × {}", f.width, f.height));
                            let fit = increart_gui_core::fit_size(w, h);
                            if fit != (f.width, f.height)
                                && ui
                                    .small_button(format!("Use {} × {}", fit.0, fit.1))
                                    .on_hover_text("output size with the start image's aspect (multiples of 32)")
                                    .clicked()
                            {
                                (f.width, f.height) = fit;
                            }
                        });
                    }
                }
            });
        }
    });
}

/// An image sweep's images, first in the Images pane as one: a stacked
/// thumbnail, "swept · K images", `<image1>` or start (DESIGN.md, Image
/// sweeps). A double-click shows them all in a modal.
fn swept_placeholder(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &std::path::Path,
    tex: &mut Textures,
    f: &mut increart_gui_core::GenerateForm,
    out: &mut Vec<Msg>,
) {
    let swept = f.swept().to_vec();
    let Some(first) = swept.first() else { return };
    let names = st.swept_names(&swept);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let t = tex.thumb(root, first);
            let resp = crate::widgets::stacked_thumb(ui, t.as_ref(), 84.0);
            let resp = resp.on_hover_text("double-click: all the sweep's images");
            let open = egui::Id::unique(SWEPT_MODAL);
            if resp.double_clicked() {
                ui.ctx().data_mut(|d| d.insert_temp(open, true));
            }
            if ui.ctx().data(|d| d.get_temp::<bool>(open)).is_some() {
                swept_modal(ui.ctx(), st.preview.is_some(), root, tex, &swept, &names, out);
            }
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(ic(ph::STACK, &format!("swept · {} images", swept.len()))).strong());
                });
                // one size for all: use size as for any image
                let dims: Vec<Option<[u32; 2]>> = swept.iter().map(|i| tex.dims(root, i)).collect();
                match dims.first().copied().flatten().filter(|d| dims.iter().all(|x| *x == Some(*d))) {
                    Some([w, h]) => {
                        ui.horizontal(|ui| {
                            ui.weak(format!("{w} × {h}"));
                            let fit = increart_gui_core::fit_size(w, h);
                            let r = ui.add_enabled(
                                fit != (f.width, f.height),
                                egui::Button::new(ic(ph::ARROWS_OUT_SIMPLE, "use size")).small(),
                            );
                            if r.on_hover_text(format!("set the output size to {} × {}", fit.0, fit.1))
                                .on_disabled_hover_text("the output size already matches")
                                .clicked()
                            {
                                (f.width, f.height) = fit;
                            }
                        });
                    }
                    None => {
                        ui.weak("sizes differ");
                    }
                }
                ui.horizontal(|ui| {
                    let start = f.sweep.image_start;
                    if ui
                        .selectable_label(!start, "<image1>")
                        .on_hover_text("each run's image is its <image1>, the form's references after it")
                        .clicked()
                    {
                        f.sweep.image_start = false;
                    }
                    if ui
                        .selectable_label(start, "start")
                        .on_hover_text("each run's image is its img2img start image")
                        .clicked()
                    {
                        f.sweep.image_start = true;
                    }
                });
                if f.sweep.image_start && f.images.iter().any(|i| i.start) {
                    ui.weak("the list's own start image isn't used");
                }
                ui.weak("Vary something else to take it out");
            });
        });
        if f.sweep.image_start {
            // as for a start image: how much of it to keep
            let mut keep = ((1.0 - f.strength) * 100.0).round();
            let r = ui
                .add(egui::Slider::new(&mut keep, 0.0..=95.0).step_by(5.0).suffix(" %").text("keep"))
                .on_hover_text("How much of each start image the result keeps (strength = 1 − keep)");
            if r.changed() {
                f.strength = 1.0 - keep / 100.0;
            }
        }
    });
}

/// Whether the swept images' modal is open (egui temp data).
pub const SWEPT_MODAL: &str = "swept_images_modal";

/// An image sweep's images, all of them, larger and named (the swept
/// placeholder double-clicked). Double-click one for full size.
fn swept_modal(
    ctx: &egui::Context,
    previewing: bool,
    root: &std::path::Path,
    tex: &mut Textures,
    swept: &[increart_core::ImageRef],
    names: &[String],
    out: &mut Vec<Msg>,
) {
    // cards of a 3:2 image box over the name; as many columns as fit
    const CELL: f32 = 210.0;
    const GAP: f32 = 12.0;
    let open = egui::Id::unique(SWEPT_MODAL);
    let mut close = false;
    let room = ctx.content_rect().size() * 0.9 - egui::Vec2::splat(32.0);
    let fit = (((room.x + GAP) / (CELL + GAP)).floor() as usize).max(1);
    let cols = fit.min(swept.len().max(1)).min(5);
    let width = (cols as f32 * (CELL + GAP) - GAP).max(420.0);
    let m = egui::Modal::new(egui::Id::unique("swept_images")).show(ctx, |ui| {
        ui.set_width(width);
        ui.set_max_width(width);
        ui.heading(ic(ph::STACK, &format!("The sweep's {} images", swept.len())));
        ui.weak("A run each, in this order · double-click one for full size, ← / → there for the others");
        ui.add_space(6.0);
        ScrollArea::vertical().max_height((room.y - 120.0).max(200.0)).auto_shrink([false, true]).show(ui, |ui| {
            for (row, line) in swept.iter().zip(names).enumerate().collect::<Vec<_>>().chunks(cols).enumerate() {
                if row > 0 {
                    ui.add_space(GAP);
                }
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for (i, (image, name)) in line {
                        ui.vertical(|ui| {
                            ui.set_width(CELL);
                            let (rect, r) =
                                ui.allocate_exact_size(egui::vec2(CELL, CELL * 2.0 / 3.0), egui::Sense::click());
                            let p = ui.painter_at(rect);
                            p.rect_filled(rect, 3.0, egui::Color32::from_gray(30));
                            let t = tex.thumb(root, image);
                            crate::widgets::image_fit(&p, rect, t.as_ref());
                            if r.hovered() {
                                p.rect_stroke(rect, 3.0, (1.0, egui::Color32::GRAY), egui::StrokeKind::Inside);
                            }
                            if r.on_hover_text(name.as_str()).double_clicked() {
                                out.push(Msg::PreviewAmong(swept.to_vec(), (*image).clone()));
                            }
                            ui.horizontal(|ui| {
                                ui.weak(format!("{}", i + 1));
                                ui.add(egui::Label::new(RichText::new(name.as_str()).strong()).truncate());
                            });
                            if let Some([w, h]) = tex.dims(root, image) {
                                ui.weak(RichText::new(format!("{w} × {h}")).small());
                            }
                        });
                    }
                });
            }
        });
        ui.add_space(6.0);
        dialog_buttons(ui, |ui| {
            if ui.button(RichText::new("Close   Esc").strong()).clicked() {
                close = true;
            }
        });
    });
    // Esc closing the full-size preview over it leaves it open
    if close || (m.should_close() && !previewing) {
        ctx.data_mut(|d| d.remove::<bool>(open));
    }
}

/// Names the project "New project from this image" or "Duplicate project
/// with this image" makes.
pub fn new_project_window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let (Some(d), Some(root)) = (&st.new_project, &st.library) else { return };
    let duplicate = d.target == increart_gui_core::UseTarget::DuplicateProject;
    let source = st.output_link(&d.image).and_then(|o| st.project_of_link(o));
    let m = egui::Modal::new(egui::Id::unique("new_project_name")).show(ctx, |ui| {
        ui.set_width(440.0);
        let title = if duplicate { "Duplicate project with this image" } else { "New project from this image" };
        ui.heading(ic(if duplicate { ph::COPY } else { ph::ROCKET_LAUNCH }, title));
        ui.horizontal(|ui| {
            let t = tex.thumb(root, &d.image);
            thumb(ui, t.as_ref(), 72.0, false, None);
            ui.vertical(|ui| {
                if let Some(p) = source {
                    let parent = p.dir.parent().map(|x| x.display().to_string()).unwrap_or_default();
                    ui.weak(if duplicate {
                        format!("A copy of {} (settings, sources, draft; no images), in {parent}/", p.name())
                    } else {
                        format!("A new project next to {}, in {parent}/", p.name())
                    });
                }
                ui.label("Name");
                let mut name = d.name.clone();
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(300.0));
                crate::widgets::focus_until_done(&r);
                if name != d.name {
                    out.push(Msg::EditNewProjectName(name.clone()));
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    out.push(Msg::ConfirmNewProject);
                }
                if let Err(e) = increart_core::library::validate_name(name.trim()) {
                    ui.colored_label(BAD, e.to_string());
                }
            });
        });
        dialog_buttons(ui, |ui| {
            if ui.button(RichText::new(ic(ph::CHECK, "Create   Enter")).strong()).clicked() {
                out.push(Msg::ConfirmNewProject);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::CancelNewProject);
            }
        });
    });
    if m.should_close() {
        out.push(Msg::CancelNewProject);
    }
}

/// New project with these as a sweep: the picked images, the project's
/// name and where it goes (DESIGN.md, Image sweeps).
pub fn sweep_project_window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let (Some(d), Some(root)) = (&st.sweep_project, &st.library) else { return };
    let n = d.images.len();
    let m = egui::Modal::new(egui::Id::unique("sweep_project")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading(ic(ph::STACK, "New project with these as a sweep"));
        ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for image in &d.images {
                    let t = tex.thumb(root, image);
                    thumb(ui, t.as_ref(), 56.0, false, None);
                }
            });
        });
        let folder = if d.parent.as_os_str().is_empty() {
            "projects/".to_string()
        } else {
            format!("projects/{}/", d.parent.display())
        };
        ui.weak(format!(
            "An image sweep of these {n} images, in pick order: a run each, the image its <image1>.              Made in {folder} and opened in Generate (nothing queued)."
        ));
        if n as u64 >= increart_gui_core::update::BIG_SWEEP {
            ui.colored_label(NEUTRAL, format!("{n} images: {n} runs per seed"));
        }
        ui.label("Name");
        let mut name = d.name.clone();
        let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
        crate::widgets::focus_until_done(&r);
        if name != d.name {
            out.push(Msg::EditSweepProjectName(name.clone()));
        }
        let problem = match increart_core::library::validate_name(name.trim()) {
            Err(e) => Some(e.to_string()),
            Ok(()) if st.project_name_taken(&d.parent.join("_"), name.trim()) => {
                Some("already exists here".to_string())
            }
            Ok(()) => None,
        };
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && problem.is_none() {
            out.push(Msg::ConfirmSweepProject);
        }
        if let Some(e) = &problem {
            ui.colored_label(BAD, e);
        }
        ui.add_space(4.0);
        dialog_buttons(ui, |ui| {
            let create = egui::Button::new(RichText::new(ic(ph::CHECK, "Create   Enter")).strong());
            if ui.add_enabled(problem.is_none(), create).clicked() {
                out.push(Msg::ConfirmSweepProject);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::SweepProject(false));
            }
        });
    });
    if m.should_close() {
        out.push(Msg::SweepProject(false));
    }
}

/// Names the copy a project's Duplicate makes.
pub fn duplicate_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let Some(d) = &st.duplicate else { return };
    let m = egui::Modal::new(egui::Id::unique("duplicate_project")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.heading(ic(ph::COPY, "Duplicate project"));
        let source = d.path.file_name().unwrap_or_default().to_string_lossy();
        let parent = d.path.parent().map(|x| x.display().to_string()).unwrap_or_default();
        ui.weak(format!("A copy of {source} (settings, sources, draft; no images), in projects/{parent}"));
        ui.label("Name");
        let mut name = d.name.clone();
        let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
        crate::widgets::focus_until_done(&r);
        if name != d.name {
            out.push(Msg::EditDuplicateName(name.clone()));
        }
        let problem = match increart_core::library::validate_name(name.trim()) {
            Err(e) => Some(e.to_string()),
            Ok(()) if st.project_name_taken(&d.path, name.trim()) => Some("already exists here".to_string()),
            Ok(()) => None,
        };
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && problem.is_none() {
            out.push(Msg::ConfirmDuplicate);
        }
        if let Some(e) = &problem {
            ui.colored_label(BAD, e);
        }
        ui.add_space(4.0);
        dialog_buttons(ui, |ui| {
            let create = egui::Button::new(RichText::new(ic(ph::CHECK, "Duplicate   Enter")).strong());
            if ui.add_enabled(problem.is_none(), create).clicked() {
                out.push(Msg::ConfirmDuplicate);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::CancelDuplicate);
            }
        });
    });
    if m.should_close() {
        out.push(Msg::CancelDuplicate);
    }
}

/// New project from ideas: the project folders on the left (a filter, the
/// tree, New folder under it: to start with, one named after the project,
/// following its name), the project's name on the right, where it goes,
/// Create.
pub fn from_ideas_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    // the name takes the focus as the dialog opens: once, not back from a
    // field clicked since
    let opening = folder_pick::opening(ctx, "from_ideas", st.from_ideas.is_some());
    let Some(d) = &st.from_ideas else { return };
    let m = egui::Modal::new(egui::Id::unique("new_project_from_ideas")).show(ctx, |ui| {
        ui.set_width(760.0);
        let title = match d.ideas.len() {
            0 | 1 => "New project from idea".to_string(),
            n => format!("New project from the {n} ideas"),
        };
        ui.heading(ic(ph::ROCKET_LAUNCH, &title));
        ui.add_space(4.0);
        // the dialog as edited this frame
        let mut d = d.clone();
        // Enter in the filter or the new folder's name only leaves the field
        let mut field_enter = false;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(300.0);
                let picker = folder_pick::Picker {
                    id: "from_ideas",
                    folders: &st.index.project_folders,
                    root: (ph::FOLDERS, "projects"),
                    selected: &d.parent,
                    filter: &d.filter,
                    new_folder: d.new_folder.as_deref(),
                    new_name: d.name.trim(),
                };
                let picked = folder_pick::show(ui, &picker, 280.0);
                field_enter |= picked.field_enter;
                if let Some(f) = picked.filter {
                    out.push(Msg::FromIdeasFilter(f));
                }
                if let Some(f) = picked.select {
                    out.push(Msg::FromIdeasParent(f.clone()));
                    d.parent = f;
                }
                if let Some(f) = picked.new_folder {
                    out.push(Msg::FromIdeasNewFolder(f.clone()));
                    d.new_folder = f;
                }
            });
            folder_pick::column_line(ui, 340.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("Project name").strong());
                let mut name = d.name.clone();
                let r = ui
                    .add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY).hint_text("project name"));
                if opening {
                    r.request_focus();
                }
                if name != d.name {
                    out.push(Msg::EditFromIdeasName(name.clone()));
                    if d.own_folder() {
                        d.new_folder = Some(name.trim().to_string());
                    }
                    d.name = name;
                }
                ui.weak(match d.ideas.len() {
                    0 => "The new idea goes to the input library and into the project.".to_string(),
                    1 => "The idea goes into the project.".to_string(),
                    n => format!("The {n} ideas go into the project."),
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Made in").strong());
                let (at, exists) = increart_gui_core::from_ideas_folder(st, &d);
                let shown = if d.name.trim().is_empty() { "…" } else { d.name.trim() };
                ui.horizontal_wrapped(|ui| {
                    let path = Path::new("projects").join(&at).join(shown);
                    ui.label(RichText::new(path.display().to_string()).monospace());
                    if !exists {
                        ui.weak("(new folder)");
                    }
                });
                if let Some(e) = increart_gui_core::from_ideas_problem(st, &d)
                    && !d.name.trim().is_empty()
                {
                    ui.add_space(4.0);
                    ui.colored_label(BAD, e);
                }
            });
        });
        ui.add_space(4.0);
        let problem = increart_gui_core::from_ideas_problem(st, &d);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        dialog_buttons(ui, |ui| {
            let create = egui::Button::new(RichText::new(ic(ph::CHECK, "Create   Enter")).strong());
            if ui.add_enabled(problem.is_none(), create).clicked() || enter && !field_enter && problem.is_none() {
                out.push(Msg::ConfirmFromIdeas);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::CancelFromIdeas);
            }
        });
    });
    if m.should_close() {
        out.push(Msg::CancelFromIdeas);
    }
}

/// Asks before renumbering the prompt's `<imageN>` tags after the image
/// list changed.
pub fn retag_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let Some(r) = &st.retag else { return };
    let m = egui::Modal::new(egui::Id::unique("retag")).show(ctx, |ui| {
        ui.set_max_width(520.0);
        ui.heading(ic(ph::TEXT_T, "Update the prompt's image tags?"));
        ui.label("The image list changed the numbers of images the prompt names:");
        for (old, new) in &r.changes {
            match new {
                Some(n) => ui.label(format!("    <image{old}>  →  <image{n}>")),
                None => ui.label(format!("    <image{old}>  →  [removed image]   (no longer a reference)")),
            };
        }
        ui.add_space(6.0);
        ui.weak("The current prompt is saved to its history first.");
        // leaving it is the default: renumbering rewrites what was typed
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        dialog_buttons(ui, |ui| {
            if ui.button(RichText::new("Leave the prompt as it is   Enter").strong()).clicked() || enter {
                out.push(Msg::RetagDismiss);
            }
            if ui.button(ic(ph::CHECK, "Update prompt")).clicked() {
                out.push(Msg::RetagConfirm);
            }
        });
    });
    if m.should_close() {
        out.push(Msg::RetagDismiss);
    }
}

/// A reference or start image shown large, in a fixed box nearly the
/// window's size (hairline border, over a dark backdrop), scaled to fit.
/// The wheel, `+` / `-` and the bar below zoom (the wheel at the pointer),
/// dragging pans, `0` fits and `1` is 100%; ← / → step through the list it
/// was opened from (Review, its contact sheet, the Output library's grid).
/// There the image can be rated as in Review — 1 / G, 2 / N, 3 / B (so `1`
/// no longer zooms), 4 or Ctrl+Z (undo), the bar's Good · Neutral · Bad · undo, or the
/// right-click menu — and used (E, D, R, I, the menu). A click (when not
/// zoomed in), Esc or a click outside closes it.
/// `keys_open` opens the shortcuts popover (for the tour).
pub fn preview_window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, keys_open: bool, out: &mut Vec<Msg>) {
    let (Some(img), Some(root)) = (&st.preview, &st.library) else { return };
    // the large version once it's loaded, the thumbnail until then
    let t = tex.full(root, img).or_else(|| tex.thumb(root, img));
    let [w, h] = tex.dims(root, img).unwrap_or([1024, 1024]);
    let full = egui::vec2(w as f32, h as f32);
    const BAR: f32 = 30.0;
    let room = ctx.content_rect().size() * 0.95 - egui::vec2(0.0, BAR);
    let fit = (room.x / full.x).min(room.y / full.y);
    let view_id = egui::Id::unique("image_preview_view");
    let mut v = ctx
        .data(|d| d.get_temp::<PreviewView>(view_id))
        .filter(|v| v.image == *img)
        .unwrap_or_else(|| PreviewView::new(img.clone(), full, fit));
    v.fit = fit;
    // ← / →: the previous / next image where it was opened from, an output
    // there, so it can be rated (its verdict) and used
    let at = st.preview_position();
    let verdict = at.filter(|_| st.preview_among.is_none()).and_then(|(i, _)| Some(st.preview_list()?.get(i)?.verdict));
    let rates = verdict.is_some();
    // opened from the contact sheet: ↑ / ↓ move between its style rows
    let rows = at.is_some() && st.tab == Tab::Review && st.review_view == increart_gui_core::ReviewView::Sheet;
    let keys = ctx.input(|i| {
        let k = |key| i.key_pressed(key);
        let actual = k(egui::Key::Num1) && !rates;
        (k(egui::Key::Plus) || k(egui::Key::Equals), k(egui::Key::Minus), k(egui::Key::Num0), actual)
    });
    let used = out.len();
    if at.is_some() {
        ctx.input(|i| {
            if i.key_pressed(egui::Key::ArrowLeft) {
                out.push(Msg::PreviewStep(-1));
            }
            if i.key_pressed(egui::Key::ArrowRight) {
                out.push(Msg::PreviewStep(1));
            }
            if rows {
                if i.key_pressed(egui::Key::ArrowUp) {
                    out.push(Msg::PreviewRow(-1));
                }
                if i.key_pressed(egui::Key::ArrowDown) {
                    out.push(Msg::PreviewRow(1));
                }
            }
            if !rates {
                return;
            }
            for (keys, v) in [
                ([egui::Key::Num1, egui::Key::G], Verdict::Good),
                ([egui::Key::Num2, egui::Key::N], Verdict::Neutral),
                ([egui::Key::Num3, egui::Key::B], Verdict::Bad),
            ] {
                if !i.modifiers.command && keys.iter().any(|k| i.key_pressed(*k)) {
                    out.push(Msg::PreviewRate(v));
                }
            }
            if i.key_pressed(egui::Key::Num4) || (i.modifiers.command && i.key_pressed(egui::Key::Z)) {
                out.push(Msg::UndoReview);
            }
            if !i.modifiers.command {
                for (key, msg) in [
                    (egui::Key::E, Msg::UseImage(img.clone(), UseTarget::Edit)),
                    (egui::Key::D, Msg::UseImage(img.clone(), UseTarget::DuplicateProject)),
                    (egui::Key::R, Msg::ToInputLibrary(img.clone())),
                    (egui::Key::I, Msg::NewIdeaFromImage(img.clone())),
                ] {
                    if i.key_pressed(key) {
                        out.push(msg);
                    }
                }
            }
        });
    }
    let frame = egui::Frame::NONE
        .fill(egui::Color32::from_gray(16))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(90)))
        .inner_margin(1.0);
    let m = egui::Modal::new(egui::Id::unique("image_preview"))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(200))
        .show(ctx, |ui| {
            let (zoom_in, zoom_out, fit_key, actual) = keys;
            if zoom_in {
                v.zoom(v.scale() * 1.25, egui::Vec2::ZERO);
            }
            if zoom_out {
                v.zoom(v.scale() / 1.25, egui::Vec2::ZERO);
            }
            if fit_key {
                v.reset();
            }
            if actual {
                v.zoom(1.0, egui::Vec2::ZERO);
            }

            // a fixed box, nearly the whole window: zooming changes only the image
            let view = room;
            let (rect, resp) = ui.allocate_exact_size(view, egui::Sense::click_and_drag());
            if let Some(p) = resp.hover_pos() {
                let (wheel, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
                let f = pinch * (wheel / 200.0).exp();
                if f != 1.0 {
                    v.zoom(v.scale() * f, p - rect.center());
                }
            }
            if resp.dragged() {
                v.center -= resp.drag_delta() / v.scale();
            }
            let zoomed_in = v.scale() > fit * 1.001;
            if resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if resp.hovered() && zoomed_in {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            v.clamp(view);
            let scale = v.scale();
            let shown = egui::Rect::from_min_size(rect.center() - v.center * scale, full * scale);
            crate::widgets::image_fit(&ui.painter_at(rect), shown, t.as_ref());
            let arrows = match (at.is_some(), rows) {
                (true, true) => " · ← → previous / next · ↑ ↓ style above / below",
                (true, false) => " · ← → previous / next",
                _ => "",
            };
            let close = if zoomed_in { "drag to move · Esc to close" } else { "click or Esc to close" };
            let (hundred, menu) = if rates { ("", "rate and use") } else { (", 1 100%", "copy") };
            let rate = if rates { " · 1 / 2 / 3 Good / Neutral / Bad, 4 or Ctrl+Z undo" } else { "" };
            let resp = resp.on_hover_text(format!(
                "{}\n{:.0}% · wheel or + / − zoom, 0 fit{hundred}{arrows}{rate} · {close} · right-click to {menu}",
                crate::widgets::size_info(w, h),
                scale * 100.0
            ));
            resp.context_menu(|ui| {
                if !rates {
                    return crate::widgets::image_menu_items(ui, root, img, out);
                }
                if ui.horizontal(|ui| verdict_buttons(ui, verdict.flatten(), false, out)).inner {
                    ui.close();
                }
                ui.separator();
                crate::review::use_menu(ui, st, root, img, out);
            });

            let mut close_clicked = false;
            // the bar's choices (both its sides can't borrow `out`)
            let mut picked = vec![];
            ui.allocate_ui_with_layout(
                egui::vec2(view.x, BAR - 2.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    // right to left: drawn from the close button back
                    let bar = |ui: &mut egui::Ui| {
                        close_clicked = ui.small_button(ph::X).on_hover_text("close (Esc)").clicked();
                        let keys = ui.small_button(ph::KEYBOARD).on_hover_text("keyboard shortcuts");
                        let mut popup = egui::Popup::from_toggle_button_response(&keys);
                        if keys_open {
                            popup = popup.open_memory(egui::SetOpenCommand::Bool(true));
                        }
                        popup
                            .align(egui::RectAlign::TOP_END)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .show(|ui| preview_keys(ui, at.is_some(), rows, rates));
                        ui.separator();
                        if rates {
                            // right to left, so Good · Neutral · Bad read left to right
                            ui.menu_button(ph::DOTS_THREE, |ui| {
                                crate::review::use_menu(ui, st, root, img, &mut picked)
                            })
                            .response
                            .on_hover_text("use this image");
                            verdict_buttons(ui, verdict.flatten(), true, &mut picked);
                            let undo = ui.add_enabled(
                                !st.review_undo.is_empty(),
                                egui::Button::new(ph::ARROW_COUNTER_CLOCKWISE),
                            );
                            if undo.on_hover_text("undo the last rating (4 or Ctrl+Z)").clicked() {
                                picked.push(Msg::UndoReview);
                            }
                            ui.separator();
                        }
                        let one = (scale - 1.0).abs() < 1e-3;
                        let hint = if rates {
                            "one image pixel per screen pixel"
                        } else {
                            "one image pixel per screen pixel (1)"
                        };
                        if ui.selectable_label(one, "100%").on_hover_text(hint).clicked() {
                            v.zoom(1.0, egui::Vec2::ZERO);
                        }
                        if ui.selectable_label(v.zoom.is_none(), "Fit").on_hover_text("fit the window (0)").clicked() {
                            v.reset();
                        }
                        if ui.small_button(ph::MAGNIFYING_GLASS_PLUS).on_hover_text("zoom in (+)").clicked() {
                            v.zoom(scale * 1.25, egui::Vec2::ZERO);
                        }
                        ui.label(RichText::new(format!("{:.0}%", scale * 100.0)).monospace());
                        if ui.small_button(ph::MAGNIFYING_GLASS_MINUS).on_hover_text("zoom out (−)").clicked() {
                            v.zoom(scale / 1.25, egui::Vec2::ZERO);
                        }
                    };
                    let left = |ui: &mut egui::Ui| {
                        if let Some((i, n)) = at {
                            let prev = ui.add_enabled(i > 0, egui::Button::new(ph::CARET_LEFT).small());
                            if prev.on_hover_text("previous image (←)").clicked() {
                                out.push(Msg::PreviewStep(-1));
                            }
                            ui.label(RichText::new(format!("{} / {n}", i + 1)).monospace());
                            let next = ui.add_enabled(i + 1 < n, egui::Button::new(ph::CARET_RIGHT).small());
                            if next.on_hover_text("next image (→)").clicked() {
                                out.push(Msg::PreviewStep(1));
                            }
                            ui.separator();
                        }
                        ui.weak(crate::widgets::size_info(w, h));
                    };
                    egui::Sides::new().show(ui, left, bar);
                },
            );
            out.append(&mut picked);
            close_clicked || resp.clicked() && !zoomed_in
        });
    // a use that opens another tab leaves the preview
    let left = out[used..].iter().any(|m| matches!(m, Msg::UseImage(..)));
    if m.inner || m.should_close() || left {
        ctx.data_mut(|d| d.remove::<PreviewView>(view_id));
        out.push(Msg::Preview(None));
    } else {
        ctx.data_mut(|d| d.insert_temp(view_id, v));
    }
}

/// The large preview's keyboard shortcuts, those that apply: stepping when
/// it was opened from a list (between style rows on the contact sheet),
/// rating and using when that list is outputs.
fn preview_keys(ui: &mut egui::Ui, steps: bool, rows: bool, rates: bool) {
    let mut groups: Vec<(&str, Vec<(&str, &str)>)> = vec![];
    if steps {
        let mut nav = vec![("← / →", "previous / next image")];
        if rows {
            nav.push(("↑ / ↓", "same seed, style above / below"));
        }
        groups.push(("Navigate", nav));
    }
    if rates {
        groups.push((
            "Rate",
            vec![("1 / G", "Good"), ("2 / N", "Neutral"), ("3 / B", "Bad"), ("4 / Ctrl+Z", "undo the last rating")],
        ));
        groups.push((
            "Use",
            vec![
                ("E", "edit this image"),
                ("D", "duplicate the project with this image"),
                ("R", "to the input library as a ref"),
                ("I", "new idea from this image"),
            ],
        ));
    }
    let mut zoom = vec![("wheel  + / −", "zoom in / out"), ("0", "fit the window")];
    if !rates {
        zoom.push(("1", "100%"));
    }
    zoom.push(("drag", "move, when zoomed in"));
    groups.push(("Zoom", zoom));
    groups.push(("Close", vec![("Esc", "close"), ("click", "close, when not zoomed in")]));
    egui::Grid::new("preview_keys").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        for (i, (title, rows)) in groups.into_iter().enumerate() {
            if i > 0 {
                ui.end_row();
            }
            ui.label(RichText::new(title).strong());
            ui.end_row();
            for (key, what) in rows {
                ui.label(RichText::new(key).monospace());
                ui.label(what);
                ui.end_row();
            }
        }
    });
}

/// Good · Neutral · Bad for the preview's image, the `current` verdict
/// marked (drawn in reverse for a right-to-left bar). Whether one was clicked.
fn verdict_buttons(ui: &mut egui::Ui, current: Option<Verdict>, rtl: bool, out: &mut Vec<Msg>) -> bool {
    let mut clicked = false;
    let mut all: Vec<_> = crate::review::VERDICTS.iter().zip(1..).collect();
    if rtl {
        all.reverse();
    }
    for ((label, v, color), key) in all {
        let b = egui::Button::new(RichText::new(*label).color(*color).strong()).selected(current == Some(*v));
        let hint = format!("{label} ({key} / {})", &label[..1]);
        if ui.add(b).on_hover_text(hint).clicked() {
            out.push(Msg::PreviewRate(*v));
            clicked = true;
        }
    }
    clicked
}

/// The large preview's zoom and position, kept while it shows one image.
#[derive(Clone)]
struct PreviewView {
    image: increart_core::ImageRef,
    full: egui::Vec2,
    /// The scale that fits the window (it follows the window's size).
    fit: f32,
    /// `None`: fit.
    zoom: Option<f32>,
    /// The image point (in image pixels) at the middle of the view.
    center: egui::Vec2,
}

impl PreviewView {
    fn new(image: increart_core::ImageRef, full: egui::Vec2, fit: f32) -> Self {
        PreviewView { image, full, fit, zoom: None, center: full / 2.0 }
    }

    fn scale(&self) -> f32 {
        self.zoom.unwrap_or(self.fit)
    }

    fn reset(&mut self) {
        (self.zoom, self.center) = (None, self.full / 2.0);
    }

    /// To scale `s` (from a quarter of fit or 100% to 800%), keeping the
    /// image point `at` (from the view's middle) where it is.
    fn zoom(&mut self, s: f32, at: egui::Vec2) {
        let s = s.clamp(self.fit.min(1.0) * 0.25, self.fit.max(8.0));
        self.center += at / self.scale() - at / s;
        self.zoom = Some(s);
    }

    /// No blank edges: an axis smaller than the view stays centred.
    fn clamp(&mut self, view: egui::Vec2) {
        let (s, full) = (self.scale(), self.full);
        let half = view / 2.0 / s;
        let axis = |c: f32, f: f32, v: f32, h: f32| if f * s <= v { f / 2.0 } else { c.clamp(h, f - h) };
        self.center =
            egui::vec2(axis(self.center.x, full.x, view.x, half.x), axis(self.center.y, full.y, view.y, half.y));
    }
}

/// The Scale dialog: a new size with the same aspect, from megapixels or
/// one side, rounded to multiples of 32.
pub fn scale_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    use increart_gui_core::ScaleBy;
    let Some(d) = &st.scale else { return };
    let mut e = d.clone();
    let (lo, hi) = e.limits();
    let mut apply = false;
    let m = egui::Modal::new(egui::Id::unique("scale_dialog")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.heading(ic(ph::ARROWS_OUT, "Scale"));
        ui.weak(format!("From {}", crate::widgets::size_info(d.width, d.height)));
        ui.add_space(6.0);
        egui::Grid::new("scale_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.radio_value(&mut e.by, ScaleBy::Megapixels, "Megapixels");
            let max_mp = hi as f32 * hi as f32 / 1e6;
            let r = ui.add(egui::DragValue::new(&mut e.megapixels).range(0.05..=max_mp).speed(0.01).fixed_decimals(2));
            if r.changed() {
                e.by = ScaleBy::Megapixels;
            }
            ui.end_row();
            for (by, name, current) in [(ScaleBy::Width, "Width", d.width), (ScaleBy::Height, "Height", d.height)] {
                if ui.radio(e.by == by, name).clicked() && e.by != by {
                    (e.by, e.side) = (by, current);
                }
                let mut v = if e.by == by { e.side } else { current };
                let r = ui.add(egui::DragValue::new(&mut v).range(lo..=hi).speed(32).update_while_editing(false));
                if r.changed() {
                    (e.by, e.side) = (by, v);
                }
                ui.end_row();
            }
        });
        ui.separator();
        let (w, h) = e.result();
        ui.label(RichText::new(format!("→ {}", crate::widgets::size_info(w, h))).strong());
        ui.weak(format!("Same aspect, both sides rounded to multiples of 32, within {lo}–{hi}."));
        ui.add_space(6.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        dialog_buttons(ui, |ui| {
            apply = ui.button(RichText::new(ic(ph::CHECK, "Apply   Enter")).strong()).clicked() || enter;
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::CloseScale);
            }
        });
    });
    if e != *d {
        out.push(Msg::EditScale(e));
    }
    if apply {
        out.push(Msg::ApplyScale);
    } else if m.should_close() {
        out.push(Msg::CloseScale);
    }
}

/// The resolutions editor (DESIGN.md, Resolutions): name, size, aspect,
/// megapixels, a time estimate; default, duplicate, swap, reorder, delete.
/// A modal that only Escape or Cancel closes: a stray click outside would
/// lose the edits.
pub fn resolutions_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let Some(list) = &st.res_editor else { return };
    let mut l = list.clone();
    let (mut save, mut cancel) = (false, false);
    egui::Modal::new(egui::Id::unique("resolutions")).show(ctx, |ui| {
        ui.heading(ic(ph::RESIZE, "Resolutions"));
        ui.weak("Sizes snap to multiples of 32 when saved; one is the default. Saved to config.rsn.");
        let mut action: Option<(usize, &str)> = None;
        egui::Grid::new("res").num_columns(8).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
            for h in ["default", "name", "width", "height", "aspect", "MP", "≈ per image", ""] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for (i, r) in l.iter_mut().enumerate() {
                if ui.radio(r.default, "").clicked() {
                    action = Some((i, "default"));
                }
                ui.add_sized([150.0, 20.0], egui::TextEdit::singleline(&mut r.name));
                for v in [&mut r.width, &mut r.height] {
                    let d = egui::DragValue::new(v).range(32..=4096).speed(32).update_while_editing(false);
                    if ui.add(d).changed() {
                        *v = increart_gui_core::round32(*v).min(4096);
                    }
                }
                ui.label(crate::widgets::aspect(r.width, r.height));
                let mp = r.width as f32 * r.height as f32 / 1e6;
                ui.label(format!("{mp:.2}"));
                let mut f = st.generate.clone();
                (f.width, f.height) = (r.width, r.height);
                ui.label(fmt_secs(estimate(st, &f)));
                ui.horizontal(|ui| {
                    for (icon, tip, what) in [
                        (ph::COPY, "duplicate", "dup"),
                        (ph::ARROWS_OUT, "scale… (megapixels, width or height)", "scale"),
                        (ph::ARROWS_LEFT_RIGHT, "swap width and height", "swap"),
                        (ph::ARROW_UP, "move up", "up"),
                        (ph::ARROW_DOWN, "move down", "down"),
                        (ph::TRASH, "delete", "del"),
                    ] {
                        if ui.small_button(icon).on_hover_text(tip).clicked() {
                            action = Some((i, what));
                        }
                    }
                });
                if r.width % 32 != 0 || r.height % 32 != 0 {
                    ui.colored_label(BAD, "not a multiple of 32: snaps on save");
                }
                if !(256..=2048).contains(&r.width) || !(256..=2048).contains(&r.height) {
                    ui.colored_label(NEUTRAL, "outside 256–2048");
                }
                ui.end_row();
            }
        });
        match action {
            Some((i, "default")) => l.iter_mut().enumerate().for_each(|(j, r)| r.default = i == j),
            Some((i, "dup")) => {
                let mut r = l[i].clone();
                r.default = false;
                l.insert(i + 1, r);
            }
            Some((i, "scale")) => out.push(Msg::OpenScale(increart_gui_core::ScaleTarget::Resolution(i))),
            Some((i, "swap")) => (l[i].width, l[i].height) = (l[i].height, l[i].width),
            Some((i, "up")) if i > 0 => l.swap(i, i - 1),
            Some((i, "down")) if i + 1 < l.len() => l.swap(i, i + 1),
            Some((i, "del")) if l.len() > 1 => {
                l.remove(i);
            }
            _ => {}
        }
        ui.add_space(6.0);
        dialog_footer(
            ui,
            |ui| {
                if ui.button(ic(ph::PLUS, "Add")).clicked() {
                    l.push(increart_core::config::Resolution::new("", st.generate.width, st.generate.height));
                }
                if ui.button("Reset to built-ins").clicked() {
                    l = increart_core::config::builtin_resolutions();
                }
            },
            |ui| {
                save = ui.button(RichText::new(ic(ph::FLOPPY_DISK, "Save")).strong()).clicked();
                cancel = ui.button("Cancel   Esc").clicked();
            },
        );
    });
    if &l != list {
        out.push(Msg::EditResolutions(l));
    }
    let esc = st.scale.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if save {
        out.push(Msg::SaveResolutions);
    } else if cancel || esc {
        out.push(Msg::CloseResolutions);
    }
}

/// The model presets editor (DESIGN.md, Model presets): the list, with
/// which open servers run each, and the selected one's recipe, files and
/// settings. A modal, like the resolutions editor.
pub fn recipes_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    use increart_core::config::{MODEL_SLOTS, RecipePreset};
    let Some(ed) = &st.recipe_editor else { return };
    let mut e = ed.clone();
    let (mut save, mut cancel) = (false, false);
    let files = st.server_files();
    // file pickers and the file manager, asked from inside the window
    let mut picks: Vec<Msg> = vec![];
    egui::Modal::new(egui::Id::unique("recipes")).show(ctx, |ui| {
        ui.set_width(820.0);
        ui.heading(ic(ph::CUBE, "Model presets"));
        ui.weak("Generate's Model list: what each loads and its default settings. Saved to config.rsn.");
        ui.add_space(4.0);
        let mut action: Option<(usize, &str)> = None;
        ui.horizontal_top(|ui| {
            // the list
            ui.vertical(|ui| {
                ui.set_width(220.0);
                for (i, r) in e.rows.iter().enumerate() {
                    let on = st.servers_running(&r.preset.files);
                    let mark = if on.is_empty() {
                        RichText::new(ph::CIRCLE).weak()
                    } else {
                        RichText::new(ph::CHECK_CIRCLE).color(GOOD)
                    };
                    ui.horizontal(|ui| {
                        ui.label(mark).on_hover_text(if on.is_empty() {
                            match st.why_no_server_runs(&r.preset.files) {
                                Some(why) => format!("no open server can run it:\n{}", why.join("\n")),
                                None => "no open server has said what it can load".to_string(),
                            }
                        } else {
                            format!("runs on {}", on.join(", "))
                        });
                        let name = if r.preset.name.trim().is_empty() { "(no name)" } else { r.preset.name.as_str() };
                        if ui.selectable_label(i == e.selected, name).clicked() {
                            action = Some((i, "select"));
                        }
                    });
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let i = e.selected;
                    for (icon, tip, what) in [
                        (ph::PLUS, "new (a copy of the selected one's files)", "new"),
                        (ph::COPY, "duplicate", "dup"),
                        (ph::ARROW_UP, "move up", "up"),
                        (ph::ARROW_DOWN, "move down", "down"),
                        (ph::TRASH, "delete", "del"),
                    ] {
                        if ui.small_button(icon).on_hover_text(tip).clicked() {
                            action = Some((i, what));
                        }
                    }
                });
            });
            ui.add_space(12.0);
            // the selected one
            let Some(r) = e.rows.get_mut(e.selected) else { return };
            ui.vertical(|ui| {
                let p = &mut r.preset;
                egui::Grid::new("recipe").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(320.0));
                    ui.end_row();
                    ui.label("Recipe").on_hover_text("sd-server's recipe for these files, e.g. qwen-image-21");
                    ui.add(egui::TextEdit::singleline(&mut p.recipe).desired_width(320.0).hint_text("qwen-image-21"));
                    ui.end_row();
                    ui.label("Steps");
                    ui.add(egui::DragValue::new(&mut p.steps).range(1..=200));
                    ui.end_row();
                    ui.label("CFG");
                    ui.add(egui::DragValue::new(&mut p.cfg).range(0.0..=30.0).speed(0.1).max_decimals(2));
                    ui.end_row();
                    ui.label("Flow shift");
                    ui.horizontal(|ui| {
                        let mut on = p.flow_shift.is_some();
                        if ui.checkbox(&mut on, "").on_hover_text("off: the recipe's").changed() {
                            p.flow_shift = on.then_some(3.0);
                        }
                        if let Some(v) = &mut p.flow_shift {
                            ui.add(egui::DragValue::new(v).range(0.01..=20.0).speed(0.05).max_decimals(2));
                        } else {
                            ui.weak("the recipe's");
                        }
                    });
                    ui.end_row();
                    ui.label("Sigmas").on_hover_text("a custom schedule, comma-separated; empty: the recipe's");
                    ui.add(egui::TextEdit::singleline(&mut r.sigmas).desired_width(320.0).hint_text("the recipe's"));
                    ui.end_row();
                });
                let p = &mut r.preset;
                ui.add_space(6.0);
                crate::startup::models_list(
                    ui,
                    st,
                    "A preset's files are found in these, searched in this order. Each is written \
                     relative to its folder (vae/x.safetensors), so a preset keeps working wherever the \
                     folders are; a path in more than one folder is the first one's. LoRAs are in each \
                     folder's loras/. The list is Server settings' own, for every preset and both servers, \
                     and saved as it's changed (Cancel keeps it); a running sd-server sees a change once \
                     it's restarted.",
                    &mut picks,
                );
                ui.add_space(6.0);
                ui.label(RichText::new("Files").strong());
                let dirs = &st.config.programs.sd_server.model_dirs;
                if dirs.iter().all(|d| d.trim().is_empty()) {
                    ui.colored_label(NEUTRAL, "No model directory yet: add the one holding the model files.");
                }
                if let Some(why) = &e.error {
                    ui.add(egui::Label::new(RichText::new(why).color(BAD)).wrap());
                }
                let mut remove = None;
                let slots: Vec<(String, String)> = p.files.clone().into_iter().collect();
                egui::Grid::new("recipe_files").num_columns(6).spacing([6.0, 4.0]).show(ui, |ui| {
                    for (slot, file) in &slots {
                        let mut s = slot.clone();
                        ComboBox::from_id_salt(("slot", slot)).selected_text(&s).width(120.0).show_ui(ui, |ui| {
                            for name in MODEL_SLOTS {
                                if name == slot || !p.files.contains_key(name) {
                                    ui.selectable_value(&mut s, name.to_string(), name);
                                }
                            }
                        });
                        let mut f = file.clone();
                        let known = files.contains(f.as_str()) || std::path::Path::new(&f).is_absolute();
                        let edit = egui::TextEdit::singleline(&mut f);
                        let r = if known || files.is_empty() {
                            ui.add_sized([300.0, 20.0], edit)
                        } else {
                            ui.add_sized([300.0, 20.0], edit.text_color(NEUTRAL))
                                .on_hover_text(st.why_file_unlisted(&f))
                        };
                        // a full path inside a model directory: written relative to it
                        if r.lost_focus()
                            && Path::new(&f).is_absolute()
                            && let Ok(id) = increart_core::config::model_file_id(Path::new(&f), dirs)
                        {
                            f = id;
                        }
                        if files.is_empty() {
                            ui.label("");
                        } else {
                            ui.menu_button(ph::CARET_DOWN, |ui| {
                                ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                    for id in &files {
                                        if ui.selectable_label(*id == f, *id).clicked() {
                                            f = id.to_string();
                                            ui.close();
                                        }
                                    }
                                });
                            })
                            .response
                            .on_hover_text("the files the open servers list");
                        }
                        let at = MODEL_SLOTS.iter().position(|n| *n == s.as_str()).unwrap_or(0);
                        if ui.small_button(ph::FOLDER_OPEN).on_hover_text("Browse… (in a model directory)").clicked()
                        {
                            picks.push(Msg::Pick(PickKind::RecipeFile(at)));
                        }
                        let on_disk = increart_core::config::model_file_on_disk(&f, dirs);
                        let tip = match &on_disk {
                            Some(p) => format!("Found at {}: show it in the file manager", p.display()),
                            None => "Not found in any model directory".into(),
                        };
                        let show = ui.add_enabled(on_disk.is_some(), egui::Button::new(ph::ARROW_SQUARE_OUT).small());
                        if show.on_hover_text(&tip).on_disabled_hover_text(&tip).clicked()
                            && let Some(p) = on_disk
                        {
                            picks.push(Msg::ShowInFileManager(p));
                        }
                        if ui.small_button(ph::TRASH).on_hover_text("remove").clicked() {
                            remove = Some(slot.clone());
                        }
                        ui.end_row();
                        if s != *slot {
                            p.files.remove(slot);
                            p.files.insert(s, f);
                        } else if f != *file {
                            p.files.insert(s, f);
                        }
                    }
                });
                if let Some(slot) = remove {
                    p.files.remove(&slot);
                }
                if let Some(free) = MODEL_SLOTS.iter().find(|s| !p.files.contains_key(**s))
                    && ui.small_button(ic(ph::PLUS, "File")).clicked()
                {
                    p.files.insert(free.to_string(), String::new());
                }
                ui.add_space(6.0);
                ui.label(RichText::new("LoRAs").strong());
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "Applied to every image at their weight, with no reload. They go in a model \
                             directory's loras folder (loras/x.safetensors), any of the folders above; KoboldCpp sees \
                             the ones there when it loads the model, at most one folder down.",
                        )
                        .weak(),
                    )
                    .wrap(),
                );
                let lora_dirs = &dirs[..dirs.len().min(1)];
                let mut remove = None;
                egui::Grid::new("recipe_loras").num_columns(5).spacing([6.0, 4.0]).show(ui, |ui| {
                    for (i, l) in p.loras.iter_mut().enumerate() {
                        let r = ui.add_sized(
                            [426.0, 20.0],
                            egui::TextEdit::singleline(&mut l.file).hint_text("loras/style.safetensors"),
                        );
                        if r.lost_focus()
                            && Path::new(&l.file).is_absolute()
                            && let Ok(id) = increart_core::config::lora_file_id(Path::new(&l.file), dirs)
                        {
                            l.file = id;
                        }
                        ui.add(egui::DragValue::new(&mut l.weight).range(-4.0..=4.0).speed(0.01).max_decimals(2))
                            .on_hover_text("weight: 1 as trained, 0 off, below 0 against it");
                        if ui.small_button(ph::FOLDER_OPEN).on_hover_text("Browse… (in the loras folder)").clicked() {
                            picks.push(Msg::Pick(PickKind::RecipeLora(i)));
                        }
                        let on_disk = increart_core::config::model_file_on_disk(&l.file, lora_dirs);
                        let tip = if on_disk.is_some() {
                            "Show it in the file manager"
                        } else {
                            "Not found in the loras folder"
                        };
                        let show = ui.add_enabled(on_disk.is_some(), egui::Button::new(ph::ARROW_SQUARE_OUT).small());
                        if show.on_hover_text(tip).on_disabled_hover_text(tip).clicked()
                            && let Some(p) = on_disk
                        {
                            picks.push(Msg::ShowInFileManager(p));
                        }
                        if ui.small_button(ph::TRASH).on_hover_text("remove").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
                if let Some(i) = remove {
                    p.loras.remove(i);
                }
                if ui.small_button(ic(ph::PLUS, "LoRA")).clicked() {
                    p.loras.push(increart_core::config::PresetLora { file: String::new(), weight: 1.0 });
                }
            });
        });
        match action {
            Some((i, "select")) => {
                e.selected = i;
                e.error = None;
            }
            Some((i, "new")) => {
                let files = e.rows.get(i).map(|r| r.preset.files.clone()).unwrap_or_default();
                let preset = RecipePreset {
                    name: String::new(),
                    recipe: e.rows.get(i).map_or("qwen-image-21".into(), |r| r.preset.recipe.clone()),
                    files,
                    steps: 20,
                    cfg: 2.0,
                    flow_shift: None,
                    custom_sigmas: vec![],
                    loras: e.rows.get(i).map(|r| r.preset.loras.clone()).unwrap_or_default(),
                };
                e.rows.push(increart_gui_core::RecipeRow { preset, origin: None, sigmas: String::new() });
                e.selected = e.rows.len() - 1;
            }
            Some((i, "dup")) if i < e.rows.len() => {
                let mut r = e.rows[i].clone();
                r.origin = None;
                r.preset.name = format!("{} copy", r.preset.name);
                e.rows.insert(i + 1, r);
                e.selected = i + 1;
            }
            Some((i, "up")) if i > 0 && i < e.rows.len() => {
                e.rows.swap(i, i - 1);
                e.selected = i - 1;
            }
            Some((i, "down")) if i + 1 < e.rows.len() => {
                e.rows.swap(i, i + 1);
                e.selected = i + 1;
            }
            Some((i, "del")) if e.rows.len() > 1 && i < e.rows.len() => {
                e.rows.remove(i);
                e.selected = i.min(e.rows.len() - 1);
            }
            _ => {}
        }
        ui.add_space(6.0);
        let problem = e.problem();
        dialog_footer(
            ui,
            |ui| {
                if ui.button("Reset to built-ins").on_hover_text("replaces the whole list").clicked() {
                    e = increart_gui_core::RecipeEditor::new(&increart_core::config::builtin_recipes(), 0);
                    e.rows.iter_mut().for_each(|r| r.origin = None);
                }
                if let Some(p) = &problem {
                    ui.colored_label(BAD, p);
                }
            },
            |ui| {
                let b = egui::Button::new(RichText::new(ic(ph::FLOPPY_DISK, "Save")).strong());
                save = ui.add_enabled(problem.is_none(), b).clicked();
                cancel = ui.button("Cancel   Esc").clicked();
            },
        );
    });
    if &e != ed {
        out.push(Msg::EditRecipes(e));
    }
    out.extend(picks);
    if save {
        out.push(Msg::SaveRecipes);
    } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        out.push(Msg::CloseRecipes);
    }
}

fn generate(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let Some(p) = st.project_entry() else {
        ui.weak("Select a project in the tree (Generate needs a single project).");
        return;
    };
    let Some(root) = st.library.clone() else { return };
    // Files dragged from the desktop onto Generate become references.
    let (hovering, dropped) = ui.ctx().input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone()));
    if !dropped.is_empty() {
        out.push(Msg::DropRefs(dropped.iter().map(|f| f.path().to_path_buf()).collect()));
    }
    if hovering {
        let painter =
            ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("refs_drop")));
        let r = ui.max_rect().shrink(12.0);
        painter.rect(
            r,
            12.0,
            egui::Color32::from_rgba_unmultiplied(40, 90, 160, 120),
            egui::Stroke::new(3.0, ACCENT),
            egui::StrokeKind::Inside,
        );
        painter.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            ic(ph::IMAGES, "Drop to add as references"),
            egui::FontId::proportional(28.0),
            egui::Color32::WHITE,
        );
    }
    let mut f = st.generate.clone();
    Panel::right("gen_images")
        .default_size(330.0)
        .resizable(true)
        .show(ui, |ui| images_pane(ui, st, &root, tex, &mut f, out));
    // the rest of the space, so the form stays left of the images pane
    egui::CentralPanel::default().show(ui, |ui| {
        // Queue stays in sight however long the form (a sweep's prompts)
        let strip = Panel::bottom("gen_queue").show(ui, |ui| queue_strip(ui, st, &f, out)).response.rect;
        let scroll = ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            ui.heading(ic(ph::MAGIC_WAND, &format!("Generate in {}", p.name())));
            ui.horizontal(|ui| {
                ui.label("Label");
                ui.add(egui::TextEdit::singleline(&mut f.label).desired_width(140.0));
                ui.label("Model preset");
                model_choice(ui, st, f.preset, out);
                if ui.small_button(ph::GEAR).on_hover_text("Edit the model presets…").clicked() {
                    out.push(Msg::OpenRecipes);
                }
            });

            ui.horizontal(|ui| {
                ui.label("Mode");
                ui.selectable_value(&mut f.sweep.on, false, ic(ph::SQUARE, "Single prompt"))
                    .on_hover_text("one run: this prompt × the seeds");
                ui.selectable_value(&mut f.sweep.on, true, ic(ph::SQUARES_FOUR, "Sweep"))
                    .on_hover_text("a run per style (prompt preset) or per value of a setting, all with the same seeds");
            });
            ui.horizontal(|ui| {
                let template = f.sweep.on && f.sweep.vary == increart_core::prompts::SweepVary::Style;
                ui.label(if template { "Template" } else { "Prompt" });
                if ui
                    .small_button(ic(ph::CLOCK_COUNTER_CLOCKWISE, "History…"))
                    .on_hover_text("past prompts of this project")
                    .clicked()
                {
                    out.push(Msg::OpenHistory(increart_core::HistoryKey::Project(p.project.id.clone())));
                }
                let open = st.presets_panel.is_some();
                if ui
                    .selectable_label(open, ic(ph::SWATCHES, "Presets…"))
                    .on_hover_text("ready-made prompts by task: styles, background removal, repair…")
                    .clicked()
                {
                    out.push(Msg::PresetsPanel(!open));
                }
                if ui
                    .small_button(ic(ph::FLOPPY_DISK, "Save as preset…"))
                    .on_hover_text("keep this prompt as a preset (all libraries)")
                    .clicked()
                {
                    out.push(Msg::OpenSavePreset);
                }
            });
            let hint = if f.sweep.on {
                "Optional: your text with {preset} where each preset's prompt goes, e.g. \
                 \"the fox from <image1> reading a newspaper, {preset}\". \
                 Empty or without {preset}: each preset's own prompt."
            } else {
                ""
            };
            let r = ui.add(
                egui::TextEdit::multiline(&mut f.prompt)
                    .id(crate::presets::prompt_id())
                    .hint_text(hint)
                    .desired_rows(if f.sweep.on { 3 } else { 6 })
                    .desired_width(f32::INFINITY),
            );
            crate::presets::select_in_prompt(ui, st, &r, out);
            r.context_menu(|ui| {
                if ui.button(ic(ph::SWATCHES, "Presets…")).clicked() {
                    out.push(Msg::PresetsPanel(true));
                    ui.close();
                }
                if ui.button(ic(ph::FLOPPY_DISK, "Save as preset…")).clicked() {
                    out.push(Msg::OpenSavePreset);
                    ui.close();
                }
            });
            let slots = increart_core::prompts::slots(&f.prompt);
            if f.sweep.on
                && f.sweep.vary == increart_core::prompts::SweepVary::Style
                && !f.prompt.trim().is_empty()
                && !f.prompt.contains(increart_core::prompts::PRESET_SLOT)
            {
                ui.colored_label(NEUTRAL, "No {preset} in the template: each run uses its preset's prompt alone.");
            }
            ui.horizontal(|ui| {
                crate::rewrite::button(ui, st, increart_gui_core::PeTarget::Prompt, "Rewrite", out);
                ui.weak("For a transparent background, ask for it in the prompt (e.g. \"make the background transparent\").");
            });
            let style_sweep = f.sweep.on && f.sweep.vary == increart_core::prompts::SweepVary::Style;
            if !style_sweep && !slots.is_empty() {
                ui.add_space(4.0);
                crate::presets::prompt_blanks(ui, st, &mut f, out);
            }
            ui.label("Negative prompt");
            ui.add(egui::TextEdit::singleline(&mut f.negative).desired_width(f32::INFINITY));
            if f.sweep.on {
                ui.add_space(6.0);
                crate::presets::sweep_panel(ui, st, &root, tex, &mut f, out);
            }

            ui.add_space(6.0);
            // Size on its own row (it wraps when narrow), the rest in a grid
            // narrow enough for the space left of the Images pane
            ui.horizontal_wrapped(|ui| {
                ui.label("Size");
                let label = st
                    .config
                    .resolutions
                    .iter()
                    .find(|r| (r.width, r.height) == (f.width, f.height))
                    .map_or(format!("{} × {}  custom", f.width, f.height), |r| r.label());
                let label = format!(
                    "{label}   {} · {}",
                    crate::widgets::aspect(f.width, f.height),
                    crate::widgets::mp(f.width, f.height)
                );
                ComboBox::from_id_salt("size").selected_text(label).width(330.0).show_ui(ui, |ui| {
                    for r in &st.config.resolutions {
                        let (a, m) = (crate::widgets::aspect(r.width, r.height), crate::widgets::mp(r.width, r.height));
                        let text = format!("{}   {a} · {m}", r.label());
                        if ui.selectable_label((r.width, r.height) == (f.width, f.height), text).clicked() {
                            (f.width, f.height) = (r.width, r.height);
                        }
                    }
                });
                // typed or dragged sizes snap to multiples of 32 when committed
                for (i, v) in [&mut f.width, &mut f.height].into_iter().enumerate() {
                    if i == 1 {
                        ui.label("×");
                    }
                    let r = ui.add(egui::DragValue::new(v).range(256..=2048).speed(32).update_while_editing(false));
                    if r.changed() {
                        *v = increart_gui_core::round32(*v).clamp(256, 2048);
                    }
                    if i == 0 {
                        // where it is, for the screenshot tour's rounding check
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("tour_width_field"), r.rect));
                    }
                }
                if ui
                    .small_button(ic(ph::ARROWS_OUT, "Scale…"))
                    .on_hover_text("to a number of megapixels, or a width / height, keeping the aspect")
                    .clicked()
                {
                    out.push(Msg::OpenScale(increart_gui_core::ScaleTarget::Generate));
                }
                aspect_menu(ui, &root, tex, &mut f);
                if ui.small_button("Edit…").on_hover_text("the resolution list").clicked() {
                    out.push(Msg::OpenResolutions);
                }
            });
            egui::Grid::new("gen_params").num_columns(4).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Steps");
                ui.add(egui::DragValue::new(&mut f.steps).range(1..=100));
                ui.label("CFG");
                ui.add(egui::DragValue::new(&mut f.cfg).range(1.0..=20.0).speed(0.1).fixed_decimals(1));
                ui.end_row();
                ui.label("Flow shift");
                ui.horizontal(|ui| {
                    let mut on = f.flow_shift.is_some();
                    ui.checkbox(&mut on, "");
                    let mut v = f.flow_shift.unwrap_or(3.0);
                    ui.add_enabled(on, egui::DragValue::new(&mut v).range(0.0..=20.0).speed(0.1).fixed_decimals(1));
                    f.flow_shift = on.then_some(v);
                    if !on {
                        ui.weak("recipe schedule");
                    }
                });
                ui.end_row();
                ui.label("Seeds");
                ui.add(egui::DragValue::new(&mut f.seeds).range(1..=999));
                ui.label("First seed");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut f.first_seed));
                    let next = st.next_seed();
                    if f.first_seed != 0
                        && ui
                            .small_button("0")
                            .on_hover_text("start at seed 0, so seeds line up with earlier runs for A/B comparison")
                            .clicked()
                    {
                        f.first_seed = 0;
                    }
                    if f.first_seed != next
                        && ui
                            .small_button(format!("{next}"))
                            .on_hover_text("continue after this project's last seed")
                            .clicked()
                    {
                        f.first_seed = next;
                    }
                });
                ui.end_row();
            });
            if !f.width.is_multiple_of(32) || !f.height.is_multiple_of(32) {
                ui.colored_label(BAD, "Width and height must be multiples of 32.");
            }
            // scrolled to the end, the form stops well clear of the Queue strip
            ui.add_space(64.0);
        });
        debug_layout(ui, strip, &scroll);
    });
    if f != st.generate {
        out.push(Msg::Generate(f));
    }
}

/// `INCREART_DEBUG_LAYOUT`: the Generate form's layout on stderr, each
/// frame it differs from the one before (a jump for a frame shows as two).
fn debug_layout<R>(ui: &egui::Ui, strip: egui::Rect, scroll: &egui::scroll_area::ScrollAreaOutput<R>) {
    if std::env::var_os("INCREART_DEBUG_LAYOUT").is_none() {
        return;
    }
    let now = format!(
        "strip {:?} view {:?} content {:?} offset {:?}",
        strip,
        scroll.inner_rect,
        scroll.content_size,
        scroll.state.unclamped_offset()
    );
    let id = egui::Id::unique("debug_layout");
    if ui.data(|d| d.get_temp::<String>(id)).as_deref() != Some(now.as_str()) {
        let (frame, discard) = (ui.ctx().cumulative_pass_nr(), ui.ctx().will_discard());
        eprintln!("[layout] pass {frame}{} {now}", if discard { " (discarded)" } else { "" });
        ui.data_mut(|d| d.insert_temp(id, now));
    }
}

/// Under the Generate form, laid out as a dialog's button row: **Clear**,
/// **Review** and the draft's state on the left, **Queue** (how many runs
/// and seeds) in the corner with the time per image beside it; below, why
/// Queue can't go yet.
/// The model presets the open servers can run (the others need files no
/// server has), and what a KoboldCpp started elsewhere runs without a preset.
fn model_choice(ui: &mut egui::Ui, st: &AppState, chosen: usize, out: &mut Vec<Msg>) {
    let recipes = &st.config.recipes;
    let runnable = st.runnable_recipes();
    let hidden: Vec<&str> =
        (0..recipes.len()).filter(|i| !runnable.contains(i)).map(|i| recipes[i].name.as_str()).collect();
    let name = recipes.get(chosen).map_or("?", |r| r.name.as_str());
    let shown = if runnable.contains(&chosen) { RichText::new(name) } else { RichText::new(name).color(BAD) };
    let r = ComboBox::from_id_salt("preset").selected_text(shown).show_ui(ui, |ui| {
        for &i in &runnable {
            if ui.selectable_label(i == chosen, &recipes[i].name).clicked() {
                out.push(Msg::SelectRecipe(i));
            }
        }
        if runnable.is_empty() {
            ui.weak("No server has the files of any model preset");
        }
        if !hidden.is_empty() {
            ui.separator();
            let why: Vec<String> = recipes
                .iter()
                .filter(|r| hidden.contains(&r.name.as_str()))
                .filter_map(|r| st.why_no_server_runs(&r.files).map(|w| format!("{}:\n  {}", r.name, w.join("\n  "))))
                .collect();
            let l = ui.weak(format!("Not on these servers: {}", hidden.join(", ")));
            if !why.is_empty() {
                l.on_hover_text(why.join("\n\n"));
            }
        }
        for (server, file) in st.models_without_preset() {
            ui.weak(format!("{server} runs {file}, which no model preset uses"));
        }
    });
    if !runnable.contains(&chosen) {
        r.response.on_hover_text("No open server has this preset's model files");
    }
}

fn queue_strip(ui: &mut egui::Ui, st: &AppState, f: &increart_gui_core::GenerateForm, out: &mut Vec<Msg>) {
    ui.add_space(4.0);
    let runs = f.sweep.on.then(|| increart_gui_core::update::sweep_runs(st));
    let (_, queue) = dialog_footer(
        ui,
        |ui| {
            if ui.button(ic(ph::ERASER, "Clear")).on_hover_text("reset the form (and this project's draft)").clicked() {
                out.push(Msg::ClearGenerate);
            }
            review_button(ui, st, out);
            crate::app::draft_note(ui, st, st.generate_dirty);
        },
        |ui| {
            let label = match &runs {
                Some(r) => {
                    let n = r.as_ref().map_or(0, |(r, _)| r.len());
                    format!("Queue sweep: {n} runs × {} seed(s)", f.seeds)
                }
                None => format!("Queue {} seed(s)", f.seeds),
            };
            let queue = ui.button(RichText::new(ic(ph::PLAY, &label)).strong()).clicked();
            let (per, basis) = st.image_s_for(&increart_gui_core::forecast::Shape::of_form(f, &st.config.recipes));
            ui.weak(format!("≈ {} per image", fmt_secs(per as f32))).on_hover_text(basis.text());
            queue
        },
    );
    if queue {
        out.push(Msg::QueueGenerate);
    }
    queue_estimate(ui, st, f);
    if let Some(Err(e)) = &runs {
        ui.colored_label(crate::widgets::NEUTRAL, e);
    }
    if !st.engine_running {
        ui.weak("No servers: the run is saved and generated once servers are connected.");
    }
    ui.add_space(2.0);
}

/// **Review N new**: Review on the open project's unreviewed images (every
/// run and sweep in it); greyed while there are none, so the strip stays put.
fn review_button(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let Some(p) = st.project.as_ref().and_then(|id| st.index.project(id)) else { return };
    ui.separator();
    let n = counts(p)[3];
    let generating = st
        .progress
        .keys()
        .filter(|(run, _)| st.index.run(run).is_some_and(|(q, _)| q.project.id == p.project.id))
        .count();
    let more = match generating {
        0 => String::new(),
        g => format!("; {g} more generating"),
    };
    let label = if n > 0 { format!("Review {n} new") } else { "Review".into() };
    let r = ui.add_enabled(n > 0, egui::Button::new(ic(ph::EYE, &label)));
    let r = r
        .on_hover_text(format!("review this project's new images, every run and sweep, oldest first{more}"))
        .on_disabled_hover_text(format!("nothing to review yet{more}"));
    if r.clicked() {
        out.push(Msg::ReviewScope(Some(p.project.id.clone())));
        out.push(Msg::Tab(Tab::Review));
    }
}

/// **Aspect**: common ratios (landscape and portrait), a swap, and the
/// aspect of each reference, at about the same megapixels.
fn aspect_menu(ui: &mut egui::Ui, root: &std::path::Path, tex: &mut Textures, f: &mut increart_gui_core::GenerateForm) {
    use increart_gui_core::{ScaleBy, scaled};
    let mp = f.width as f64 * f.height as f64 / 1e6;
    let set = |f: &mut increart_gui_core::GenerateForm, w: u32, h: u32| {
        (f.width, f.height) = scaled(w, h, ScaleBy::Megapixels, mp, (256, 2048));
    };
    ui.menu_button(ic(ph::FRAME_CORNERS, "Aspect"), |ui| {
        ui.weak(format!("at about {mp:.2} MP"));
        egui::Grid::new("aspects").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
            for (w, h) in [(1, 1), (5, 4), (4, 3), (3, 2), (16, 10), (16, 9), (2, 1), (21, 9)] {
                if ui.button(format!("{w}:{h}")).clicked() {
                    set(f, w, h);
                    ui.close();
                }
                if w != h && ui.button(format!("{h}:{w}")).clicked() {
                    set(f, h, w);
                    ui.close();
                }
                ui.end_row();
            }
        });
        ui.separator();
        if ui.button(ic(ph::ARROWS_LEFT_RIGHT, "Swap width and height")).clicked() {
            (f.width, f.height) = (f.height, f.width);
            ui.close();
        }
        for (i, img) in f.refs().iter().enumerate() {
            let Some([w, h]) = tex.dims(root, img) else { continue };
            let text = format!("Like <image{}>  ({})", i + 1, crate::widgets::aspect(w, h));
            if ui.button(text).clicked() {
                set(f, w, h);
                ui.close();
            }
        }
    });
}

/// The form with `tag` in its prompt: in place of the selection (characters;
/// at the end if the prompt was never focused), spaced from the words
/// around it. Also the character just after the tag, for the cursor.
fn with_tag(
    f: &increart_gui_core::GenerateForm,
    tag: &str,
    sel: Option<std::ops::Range<usize>>,
) -> (increart_gui_core::GenerateForm, usize) {
    let mut f = f.clone();
    let p = &mut f.prompt;
    let n = p.chars().count();
    let sel = sel.unwrap_or(n..n);
    let byte = |c: usize| p.char_indices().nth(c).map_or(p.len(), |(i, _)| i);
    let (a, b) = (byte(sel.start.min(n)), byte(sel.end.min(n)));
    // no space after an opening bracket or before closing punctuation
    let before = if a > 0 && !p[..a].ends_with(|c: char| c.is_whitespace() || "([{".contains(c)) { " " } else { "" };
    let after = if b < p.len() && !p[b..].starts_with(|c: char| c.is_whitespace() || ",.;:!?)]}".contains(c)) {
        " "
    } else {
        ""
    };
    let insert = format!("{before}{tag}{after}");
    let end = p[..a].chars().count() + before.chars().count() + tag.chars().count();
    p.replace_range(a..b, &insert);
    (f, end)
}

/// Rough time per image (DESIGN.md, Measurements): ~1.57 s per step per
/// model pass at 1024², CFG > 1 doubles the passes, attention grows with
/// target × (target + ref) tokens.
/// Seconds per image of what the form would make ([`AppState::image_s_for`]).
pub fn estimate(st: &AppState, f: &increart_gui_core::GenerateForm) -> f32 {
    st.image_s_for(&increart_gui_core::forecast::Shape::of_form(f, &st.config.recipes)).0 as f32
}

/// Under the Generate form, one line always (the form mustn't change
/// height): how long its seeds would take on the servers working, and
/// when they'd be done with the runs in the engine. Made again only when
/// the form's runs or the forecast change.
fn queue_estimate(ui: &mut egui::Ui, st: &AppState, f: &increart_gui_core::GenerateForm) {
    use increart_gui_core::forecast::Shape;
    use increart_gui_core::timing::{clock, coarse, duration};
    use std::hash::{Hash, Hasher};
    let shapes: Vec<(Shape, u64)> = if f.sweep.on {
        match increart_gui_core::update::sweep_runs(st) {
            Ok((runs, _)) => runs.iter().map(|(r, _)| (Shape::of_form(r, &st.config.recipes), r.seeds)).collect(),
            Err(_) => vec![],
        }
    } else {
        vec![(Shape::of_form(f, &st.config.recipes), f.seeds)]
    };
    let images: u64 = shapes.iter().map(|(_, n)| n).sum();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{shapes:?}").hash(&mut h);
    (st.forecast.made.to_bits(), st.forecast.key, st.working_servers(), st.servers.len()).hash(&mut h);
    let id = egui::Id::unique("queue_estimate");
    let key = h.finish();
    let cached = ui.data(|d| d.get_temp::<(u64, Option<increart_gui_core::NewEstimate>)>(id));
    let est = match cached {
        Some((k, e)) if k == key => e,
        _ => {
            let e = st.estimate_new(&shapes, f.sweep.on);
            ui.data_mut(|d| d.insert_temp(id, (key, e)));
            e
        }
    };
    let text = match est {
        _ if images == 0 => String::new(),
        Some(e) => {
            let on = if e.servers == 1 { "1 server".to_string() } else { format!("{} servers", e.servers) };
            let mut t = format!("{images} image(s) ≈ {} on {on}", coarse(e.alone_s));
            match e.with_queue_s {
                Some(after) => {
                    t.push_str(&format!(" · with what's queued, done in ~{}", coarse(after)));
                    if let Some(c) = clock(after) {
                        t.push_str(&format!(" ({c})"));
                    }
                }
                None => {
                    if let Some(c) = clock(e.alone_s) {
                        t.push_str(&format!(" · done {c} if queued now"));
                    }
                }
            }
            t
        }
        // no server working: one image after another
        None => {
            let per: f64 = shapes.iter().map(|(s, n)| st.image_s_for(s).0 * *n as f64).sum();
            format!("{images} image(s) ≈ {} of GPU time", duration(per))
        }
    };
    let r = ui.add(egui::Label::new(RichText::new(if text.is_empty() { " ".into() } else { text }).weak()).truncate());
    r.on_hover_text(
        "the engine's scheduler on a simulated clock, each image as long as images like it took on each server \
         (model loads too, as long as they took)",
    );
}

pub fn fmt_secs(s: f32) -> String {
    if s < 90.0 { format!("{s:.0} s") } else { format!("{:.1} min", s / 60.0) }
}

fn jobs(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    ui.heading(ic(ph::HARD_DRIVES, "Servers"));
    server_list(ui, st, out);
    if ui.button(ic(ph::GEAR, "Change servers…")).clicked() {
        out.push(Msg::OpenServerDialog);
    }
    if st.pe.server.is_some() || !st.pe.jobs.is_empty() {
        ui.separator();
        ui.heading(ic(ph::MAGIC_WAND, "Prompt enhance"));
        pe_list(ui, st, out);
    }
    ui.separator();
    ui.heading(ic(ph::QUEUE, "Runs"));
    order_toggle(ui, st, out);
    stop_all_row(ui, st, out);
    // the servers stay in view; the runs scroll
    ScrollArea::vertical().auto_shrink([false, false]).id_salt("jobs_runs").show(ui, |ui| run_list(ui, st, out));
}

/// "what [Created ▾]": a sort order dropdown; the new choice when it
/// changes.
fn order_combo(ui: &mut egui::Ui, id: &str, what: &str, order: TreeOrder, tip: &str) -> Option<TreeOrder> {
    let mut picked = None;
    // one group: the label wraps with its dropdown
    crate::widgets::group(ui, id, |ui| {
        ui.weak(what);
        egui::ComboBox::from_id_salt(id)
            .selected_text(order.name())
            .width(72.0)
            .show_ui(ui, |ui| {
                for o in TreeOrder::EVERY {
                    if ui.selectable_label(order == o, o.name()).clicked() && order != o {
                        picked = Some(o);
                    }
                }
            })
            .response
            .on_hover_text(tip);
    });
    picked
}

/// Spread all · Sweep by sweep · One at a time: how the queued runs share
/// the servers.
pub fn order_toggle(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    use increart_core::scheduler::Order;
    ui.horizontal(|ui| {
        ui.weak("Order");
        for (o, label, tip) in [
            (
                Order::Spread,
                ic(ph::SHUFFLE_ANGULAR, "Spread all"),
                "every run gets a seed in turn: all runs move along together (style sweeps fill row by row)",
            ),
            (
                Order::BySweep,
                ic(ph::STACK, "Sweep by sweep"),
                "the oldest sweep's runs get a seed in turn (its contact sheet fills row by row) and it \
                 finishes before the next sweep starts; a lone run counts as a sweep of one",
            ),
            (
                Order::Sequential,
                ic(ph::LIST_NUMBERS, "One at a time"),
                "each run finishes before the next starts, in queue order",
            ),
        ] {
            if ui.selectable_label(st.config.job_order == o, label).on_hover_text(tip).clicked() {
                out.push(Msg::JobOrder(o));
            }
        }
    });
}

/// Queueing a sweep while the jobs go one at a time: sweep by sweep instead?
pub fn sweep_order_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    if st.sweep_order_ask.is_none() {
        return;
    }
    let m = egui::Modal::new(egui::Id::unique("sweep_order")).show(ctx, |ui| {
        ui.set_max_width(440.0);
        ui.heading(ic(ph::LIST_NUMBERS, "Jobs run one at a time"));
        ui.label(
            "A style sweep is best spread: each style gets a seed in turn, so the contact sheet \
             fills row by row and you can compare styles early. Sweep by sweep does that, one sweep \
             after another. One at a time finishes each style (and anything queued before it) before \
             the next starts.",
        );
        ui.add_space(8.0);
        dialog_buttons(ui, |ui| {
            let spread = RichText::new(ic(ph::STACK, "Sweep by sweep & queue   Enter")).strong();
            if ui.button(spread).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                out.push(Msg::SweepOrderAnswer(Some(true)));
            }
            if ui.button("Keep one at a time & queue").clicked() {
                out.push(Msg::SweepOrderAnswer(Some(false)));
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::SweepOrderAnswer(None));
            }
        });
    });
    if m.should_close() {
        out.push(Msg::SweepOrderAnswer(None));
    }
}

/// The PE server's device for the servers list: "GPU 1", "CPU", "chat"
/// (connected to, or Auto).
fn pe_device(st: &AppState) -> String {
    use increart_core::pe::{PeDevice, PeRun};
    match st.pe.server.as_ref().map(|s| &s.run) {
        Some(PeRun::KoboldCpp { device: PeDevice::Gpu { devices, .. }, .. }) if !devices.is_empty() => {
            let ids: Vec<String> = devices.iter().map(u32::to_string).collect();
            format!("GPU {}", ids.join(", "))
        }
        Some(PeRun::KoboldCpp { device: PeDevice::Gpu { .. }, .. }) => "GPU".into(),
        Some(PeRun::KoboldCpp { device: PeDevice::Cpu { .. }, .. }) => "CPU".into(),
        _ => "chat".into(),
    }
}

/// One row per server: name, device, state, loaded model; a row saying so
/// when no GPU server is connected, and the PE server's row last.
pub fn server_list(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let weak = ui.visuals().weak_text_color();
    egui::Grid::new(ui.scope_id().with("servers")).num_columns(5).spacing([16.0, 4.0]).show(ui, |ui| {
        if st.servers.is_empty() {
            ui.label(RichText::new("sd-server").strong());
            ui.label("GPU");
            ui.colored_label(weak, "○ none connected");
            ui.label("");
            ui.label("");
            ui.end_row();
        }
        for s in &st.servers {
            ui.label(RichText::new(&s.name).strong());
            ui.label(&s.device);
            let c = if s.state == "ready" {
                GOOD
            } else if s.state.contains("retrying") {
                BAD
            } else {
                NEUTRAL
            };
            state_cell(ui, RichText::new(&s.state).color(c));
            ui.label(s.model.clone().unwrap_or_default());
            let log = s.config.as_ref().map(|c| increart_core::launcher::log_path(Path::new(""), c));
            log_button(
                ui,
                log.map_or_else(|| increart_core::launcher::log_name(&s.name), |p| p.display().to_string()),
                out,
            );
            ui.end_row();
            // its speed this session, under its state
            if let Some(speed) = st.server_speed(&s.name) {
                ui.label("");
                ui.label("");
                state_cell(ui, RichText::new(speed.text()).weak().small())
                    .on_hover_text(format!("its latest {} image(s) this session", speed.images));
                ui.label("");
                ui.label("");
                ui.end_row();
            }
        }
        ui.label(RichText::new("PE").strong()).on_hover_text("Prompt enhance");
        ui.label(pe_device(st))
            .on_hover_text("the chat server's device; on a GPU an sd-server also uses, its asks slow the images down");
        let (dot, state, color) = crate::app::pe_state(ui, st);
        state_cell(ui, RichText::new(format!("{dot} {state}")).color(color));
        ui.label("");
        match &st.pe.server {
            Some(s) => log_button(ui, increart_core::pe::log_name(&s.name), out),
            None => {
                ui.label("");
            }
        }
        ui.end_row();
    });
}

/// A server's log in the log viewer, when the app started it (only those
/// have a log).
pub fn log_button(ui: &mut egui::Ui, file: String, out: &mut Vec<Msg>) {
    if !increart_gui_core::runtime::log_dir().join(&file).is_file() {
        ui.label("");
        return;
    }
    if ui.small_button(ph::SCROLL).on_hover_text(format!("its log ({file})")).clicked() {
        out.push(Msg::OpenLogs(Some(file)));
    }
}

/// A server's state in a fixed width, cut with "…" (whole on hover): it
/// changes many times a second, and the list mustn't change width with it.
fn state_cell(ui: &mut egui::Ui, text: RichText) -> egui::Response {
    const W: f32 = 240.0;
    let h = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(egui::vec2(W, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_width(W);
        // egui shows a cut label whole on hover
        ui.add(egui::Label::new(text).truncate())
    })
    .inner
}

/// The prompt-enhance asks waiting, a block per batch: Stop, what it's for
/// (a click goes where it was asked), "4 of 41", where the ask being worked
/// on is, and a bar over the batch; each ask in the tooltip.
pub fn pe_list(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    use increart_gui_core::{PeBatchKind, PeTarget};
    let batches = st.pe.batches();
    if batches.is_empty() {
        ui.weak("Nothing asked.");
        return;
    }
    let project = st.project.as_ref().and_then(|p| st.index.project(p)).map(|p| p.name()).unwrap_or_default();
    for b in &batches {
        let Some(first) = st.pe.job(b.ids[0]) else { continue };
        let (title, place, go) = match &b.kind {
            PeBatchKind::Blanks => ("Suggest values", project.clone(), "go to the Generate form they're for"),
            PeBatchKind::Styles => ("Enhance style prompts", project.clone(), "go to the Generate form they're for"),
            PeBatchKind::Text(t) => match t {
                PeTarget::Prompt => ("Rewrite the prompt", project.clone(), "show the rewrite window"),
                PeTarget::NewIdea => ("Rewrite the idea", "new idea".into(), "show the rewrite window"),
                PeTarget::Idea(id) => {
                    let name = st.idea(id).map(|e| e.idea.title.clone()).filter(|t| !t.is_empty());
                    ("Rewrite the idea", name.unwrap_or_else(|| "(untitled)".into()), "show the rewrite window")
                }
                PeTarget::Describe(_) => ("Describe the image", String::new(), "show the describe window"),
            },
        };
        let mut tip = String::new();
        for (i, id) in b.ids.iter().enumerate() {
            if let Some(j) = st.pe.job(*id) {
                let n = b.at() + i;
                tip.push_str(&format!(
                    "{n}. {}: {}\n",
                    crate::rewrite::ask_name(st, j),
                    crate::rewrite::progress(st, j)
                ));
            }
        }
        tip.push_str(go);
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            // Stop first: the text after it changes as the asks go
            ui.horizontal(|ui| {
                ui.spinner();
                if ui.small_button(ic(ph::STOP, "Stop")).on_hover_text("stop these asks").clicked() {
                    out.push(Msg::PeStop(b.ids.clone()));
                }
                if ui.link(RichText::new(title).strong()).on_hover_text(&tip).clicked() {
                    match &b.kind {
                        PeBatchKind::Blanks | PeBatchKind::Styles | PeBatchKind::Text(PeTarget::Prompt) => {
                            out.push(Msg::Tab(Tab::Projects));
                            out.push(Msg::ProjectsTab(ProjectsTab::Generate));
                        }
                        PeBatchKind::Text(_) => {}
                    }
                    if matches!(b.kind, PeBatchKind::Text(_)) {
                        let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::unique("pe_text"));
                        ui.ctx().move_to_top(layer);
                    }
                    egui::Popup::close_all(ui.ctx());
                }
                if !place.is_empty() {
                    ui.weak(place);
                }
                if b.total > 1 {
                    ui.label(format!("{} of {}", b.at(), b.total));
                }
            });
            let step = RichText::new(crate::rewrite::progress(st, first)).weak();
            ui.add(egui::Label::new(step).truncate()).on_hover_text(&tip);
            if b.total > 1 {
                let done = (b.at() - 1) as f32 / b.total as f32;
                ui.add(egui::ProgressBar::new(done).desired_height(6.0));
            }
        });
    }
}

/// **Finish & stop all** / **Stop all now**, above the runs (not scrolled
/// away with them), greyed out while nothing generates; on the right, in
/// the order of each project's buttons below (Stop now in the corner).
pub fn stop_all_row(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    dialog_buttons(ui, |ui| {
        let all = st.active_runs.len();
        let unfinishing = st.active_runs.iter().any(|r| !st.finishing.contains(r));
        if ui
            .add_enabled(all > 0, egui::Button::new(RichText::new(ic(ph::STOP, "Stop all now")).color(BAD)))
            .on_hover_text("everything stops; images being generated are thrown away")
            .clicked()
        {
            let runs = if all == 1 { "the run".to_string() } else { format!("all {all} runs") };
            let text = format!("Stop {runs} now? The images being generated are thrown away.");
            out.push(Msg::Ask(Confirm::new(text, "Stop now", Msg::StopAll(Stop::Now))));
        }
        if ui
            .add_enabled(unfinishing, egui::Button::new(ic(ph::STOP_CIRCLE, "Finish & stop all")))
            .on_hover_text("the images being generated finish; nothing new starts")
            .clicked()
        {
            out.push(Msg::StopAll(Stop::Finish));
        }
    });
}

/// The runs in the engine by project, folded (for the session): a header
/// with its seeds, time left and stop buttons, and under it a bar; open,
/// its runs, a sweep's together under one more such header, a line each.
pub fn run_list(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    use increart_gui_core::RunItem;
    let groups = st.run_groups();
    if groups.is_empty() {
        ui.weak("Nothing generating.");
        return;
    }
    for g in &groups {
        let runs = g.runs();
        let p = st.group_progress(&runs);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let id = egui::Id::unique(("jobs_project", &g.project));
            let n = if runs.len() == 1 { "1 run".to_string() } else { format!("{} runs", runs.len()) };
            let open = fold(ui, id, |ui| {
                let title = RichText::new(&g.name).strong();
                group_stops(ui, st, &runs, &format!("{n} of {}", g.name), (title, group_text(&n, p)), out);
            });
            ui.add(egui::ProgressBar::new(p.2).desired_height(6.0));
            if !open {
                return;
            }
            for item in &g.items {
                match item {
                    RunItem::Run(j) => {
                        let finishing = st.finishing.contains(&j.run);
                        // the whole block opens the details; its own buttons come first
                        let block = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
                            job_block(ui, st, j, finishing, out);
                        });
                        let r = block
                            .response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("click for details");
                        if r.clicked() {
                            out.push(Msg::JobInfo(Some(j.run.clone())));
                        }
                    }
                    RunItem::Sweep { name, runs } => {
                        let runs: Vec<_> = runs.iter().collect();
                        sweep_block(ui, st, &g.project, &g.name, name, &runs, out);
                    }
                }
            }
        });
    }
}

/// A sweep's runs in the engine: its header (what's sampling now among its
/// seeds) and a bar; open, a line per run.
fn sweep_block(
    ui: &mut egui::Ui,
    st: &AppState,
    project: &increart_core::Id,
    project_name: &str,
    name: &str,
    runs: &[&increart_gui_core::RunProgress],
    out: &mut Vec<Msg>,
) {
    let p = st.group_progress(runs);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let id = egui::Id::unique(("jobs_sweep", project, name));
        let n = format!("{} runs", runs.len());
        let open = fold(ui, id, |ui| {
            let title = RichText::new(format!("{name} · sweep")).strong();
            let what = format!("the {n} of {project_name} {name}");
            group_stops(ui, st, runs, &what, (title, group_text(&n, p)), out);
        });
        let now: Vec<String> = runs
            .iter()
            .flat_map(|r| {
                let row = r.sweep.as_ref().map_or(r.label.as_str(), |(_, row)| row.as_str());
                r.sampling.iter().map(move |(seed, step, n)| format!("{row} s{seed} {step}/{n}"))
            })
            .collect();
        if !now.is_empty() {
            let text = RichText::new(format!("now: {}", now.join(", "))).weak().small();
            ui.add(egui::Label::new(text).truncate());
        }
        ui.add(egui::ProgressBar::new(p.2).desired_height(6.0));
        if open {
            for r in runs {
                run_line(ui, st, r, out);
            }
        }
    });
}

/// One of a sweep's runs on a line: its row (the style, "cfg 3"), a bar
/// over its seeds, what it's sampling, and its stop buttons; a click opens
/// the details.
fn run_line(ui: &mut egui::Ui, st: &AppState, r: &increart_gui_core::RunProgress, out: &mut Vec<Msg>) {
    let finishing = st.finishing.contains(&r.run);
    let line = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
        ui.horizontal(|ui| {
            let row = r.sweep.as_ref().map_or(r.label.clone(), |(_, row)| row.clone());
            let h = ui.spacing().interact_size.y;
            ui.allocate_ui_with_layout(egui::vec2(150.0, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_width(150.0);
                ui.add(egui::Label::new(row).truncate());
            });
            let bar =
                egui::ProgressBar::new(r.fraction()).desired_width(110.0).text(format!("{} / {}", r.done, r.total));
            ui.add(bar);
            let now: Vec<String> = r.sampling.iter().map(|(seed, step, n)| format!("s{seed} {step}/{n}")).collect();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                stop_buttons(ui, &r.run, finishing, r.sampling.len(), true, out);
                // its time left, without the clock: the line is short
                if let Some(s) = st.run_starts_in(&r.run) {
                    let text = format!("starts in ~{}", increart_gui_core::timing::duration(s));
                    ui.weak(RichText::new(text).small());
                } else if let Some(s) = st.run_left(&r.run).or(r.eta_s()).filter(|s| *s > 0.0) {
                    ui.weak(RichText::new(format!("~{}", increart_gui_core::timing::duration(s))).small());
                }
                ui.add(egui::Label::new(RichText::new(now.join(", ")).weak().small()).truncate());
            });
        });
    });
    let resp = line.response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("click for details");
    if resp.clicked() {
        out.push(Msg::JobInfo(Some(r.run.clone())));
    }
}

/// A foldable header: ▸/▾, then `header`; folded until opened (for the
/// session). Whether it's open.
fn fold(ui: &mut egui::Ui, id: egui::Id, header: impl FnOnce(&mut egui::Ui)) -> bool {
    let open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let flip = ui
        .horizontal(|ui| {
            let caret = if open { ph::CARET_DOWN } else { ph::CARET_RIGHT };
            let r = ui.add(egui::Button::new(caret).frame(false)).on_hover_text(if open { "fold" } else { "unfold" });
            header(ui);
            r.clicked()
        })
        .inner;
    if flip {
        ui.ctx().data_mut(|d| d.insert_temp(id, !open));
    }
    open != flip
}

/// "3 runs · 46 / 120 seeds · ~12 min left".
fn group_text(runs: &str, (done, total, _, eta): (u64, u64, f32, Option<f64>)) -> String {
    let mut t = format!("{runs} · {done} / {total} seeds");
    if let Some(s) = eta.filter(|s| *s > 0.0) {
        t.push_str(&format!(" · {}", increart_gui_core::timing::left_text(s)));
    }
    t
}

/// Every run together, for the top of Jobs: "46 / 120 seeds · ~38 min
/// left · ≈15:12 · 205 images/h"; `None` with nothing generating.
pub fn jobs_total(st: &AppState) -> Option<String> {
    let sum = st.job_summary();
    if sum.runs == 0 {
        return None;
    }
    let mut parts = vec![format!("{} / {} seeds", sum.done, sum.total)];
    if let Some(s) = sum.eta_s.filter(|s| *s > 0.0) {
        parts.push(increart_gui_core::timing::left_text(s));
    }
    if sum.per_hour > 0.0 {
        parts.push(format!("{:.0} images/h", sum.per_hour));
    }
    Some(parts.join(" · "))
}

/// A group's Finish & stop / Stop now (asked first), at the header's right;
/// its `title` and summary in the space left, cut short when it's narrow.
fn group_stops(
    ui: &mut egui::Ui,
    st: &AppState,
    runs: &[&increart_gui_core::RunProgress],
    what: &str,
    (title, summary): (RichText, String),
    out: &mut Vec<Msg>,
) {
    let ids: Vec<increart_core::Id> = runs.iter().map(|r| r.run.clone()).collect();
    let unfinishing: Vec<increart_core::Id> = ids.iter().filter(|r| !st.finishing.contains(*r)).cloned().collect();
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // right to left: Stop now, then Finish & stop
        if ui
            .add(egui::Button::new(RichText::new(ic(ph::STOP, "Stop now")).color(BAD)).small())
            .on_hover_text("stop these at once: images being generated are thrown away")
            .clicked()
        {
            let text = format!("Stop {what} now? The images being generated are thrown away.");
            out.push(Msg::Ask(Confirm::new(text, "Stop now", Msg::CancelRuns(ids.clone(), Stop::Now))));
        }
        if unfinishing.is_empty() {
            ui.weak("finishing…");
        } else if ui
            .add(egui::Button::new(RichText::new(ic(ph::STOP_CIRCLE, "Finish & stop")).strong()).small())
            .on_hover_text("the images being generated finish and are kept; the rest are cancelled")
            .clicked()
        {
            out.push(Msg::CancelRuns(unfinishing, Stop::Finish));
        }
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(title).truncate());
            ui.add(egui::Label::new(RichText::new(summary).weak()).truncate());
        });
    });
}

fn job_block(
    ui: &mut egui::Ui,
    st: &AppState,
    j: &increart_gui_core::RunProgress,
    finishing: bool,
    out: &mut Vec<Msg>,
) {
    {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    stop_buttons(ui, &j.run, finishing, j.sampling.len(), true, out);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(&j.label).strong()).truncate());
                    });
                });
            });
            ui.weak(short(&j.prompt, 70));
            ui.add(egui::ProgressBar::new(j.fraction()).text(format!("{} / {} seeds", j.done, j.total)));
            sampling_bars(ui, j);
            if let Some(text) = live_timing(st, j) {
                ui.weak(RichText::new(text).small());
            }
        });
    }
}

/// A running job's timing line: "1.71 s/it · 34.2 s/image · 4 done in
/// 2 min 16 s · ~2 min 30 s left".
fn live_timing(st: &AppState, j: &increart_gui_core::RunProgress) -> Option<String> {
    use increart_gui_core::timing::duration;
    let mut parts = vec![];
    match (j.step_s, &j.timing) {
        (Some(s), _) => parts.push(format!("{s:.2} s/it")),
        (None, Some(t)) => parts.push(t.step_text()),
        (None, None) => {}
    }
    if let Some(s) = j.image_s() {
        parts.push(format!("{}/image", duration(s)));
    }
    if let Some(t) = &j.timing {
        parts.push(format!("{} done in {}", t.images, duration(t.wall_s)));
    }
    if let Some(s) = st.run_starts_in(&j.run) {
        parts.push(format!("starts in ~{}", increart_gui_core::timing::duration(s)));
    }
    if let Some(s) = st.run_left(&j.run).or(j.eta_s()).filter(|s| *s > 0.0) {
        parts.push(increart_gui_core::timing::left_text(s));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Timing statistics as grid rows (DESIGN.md, Timing).
fn timing_rows(t: &increart_gui_core::timing::Timing) -> Vec<(&'static str, String)> {
    use increart_gui_core::timing::duration;
    let mut rows = vec![
        (
            "per image",
            if t.images > 1 {
                format!("{} (fastest {}, slowest {})", duration(t.per_image), duration(t.fastest), duration(t.slowest))
            } else {
                duration(t.per_image)
            },
        ),
        ("per step", t.step_text()),
        ("GPU time", format!("{} for {} image(s)", duration(t.gpu_s), t.images)),
        ("wall clock", duration(t.wall_s)),
    ];
    if t.queued_s >= 0.5 {
        rows.push(("server queue", format!("{} per image", duration(t.queued_s))));
    }
    if t.devices.len() > 1 || t.devices.first().is_some_and(|(d, ..)| !d.is_empty()) {
        let per: Vec<String> = t
            .devices
            .iter()
            .map(|(d, n, s)| format!("{} {}× {}", if d.is_empty() { "?" } else { d }, n, duration(*s)))
            .collect();
        rows.push(("devices", per.join(" · ")));
    }
    rows
}

/// A run's **Finish & stop** (the images being generated finish, nothing
/// new starts) and **Stop now**; while finishing, a note and Stop now.
/// `small`: the Jobs list's, in a right-to-left layout.
fn stop_buttons(
    ui: &mut egui::Ui,
    run: &increart_core::Id,
    finishing: bool,
    sampling: usize,
    small: bool,
    out: &mut Vec<Msg>,
) {
    let button = |text: RichText| if small { egui::Button::new(text).small() } else { egui::Button::new(text) };
    let finish = |ui: &mut egui::Ui, out: &mut Vec<Msg>| {
        if finishing {
            let n = match sampling {
                0 => "finishing…".to_string(),
                1 => "finishing 1 image…".to_string(),
                n => format!("finishing {n} images…"),
            };
            ui.weak(n);
        } else if ui
            .add(button(RichText::new(ic(ph::STOP_CIRCLE, "Finish & stop")).strong()))
            .on_hover_text("the images being generated finish and are kept; the rest are cancelled")
            .clicked()
        {
            out.push(Msg::CancelRun(run.clone(), Stop::Finish));
        }
    };
    let now = |ui: &mut egui::Ui, out: &mut Vec<Msg>| {
        if ui
            .add(button(RichText::new(ic(ph::STOP, "Stop now")).color(BAD)))
            .on_hover_text("stop at once: images being generated are thrown away")
            .clicked()
        {
            out.push(Msg::CancelRun(run.clone(), Stop::Now));
        }
    };
    // reading order either way: Finish & stop, then Stop now
    if small {
        now(ui, out);
        finish(ui, out);
    } else {
        finish(ui, out);
        now(ui, out);
    }
}

/// A bar per seed being sampled now.
fn sampling_bars(ui: &mut egui::Ui, j: &increart_gui_core::RunProgress) {
    for (seed, step, n) in &j.sampling {
        ui.add(
            egui::ProgressBar::new(*step as f32 / (*n).max(1) as f32)
                .desired_height(12.0)
                .text(RichText::new(format!("seed {seed}: step {step}/{n}")).small()),
        );
    }
}

/// A run's generation log: what was asked of which server and what it
/// answered, for finding out why a run fails or waits.
fn job_log(ui: &mut egui::Ui, st: &AppState, file: &std::path::Path, out: &mut Vec<Msg>) {
    // paths as their file names, unless asked for in full
    let full_id = egui::Id::unique("job_log_full_paths");
    let mut full = ui.data(|d| d.get_temp::<bool>(full_id)).unwrap_or(false);
    ui.horizontal(|ui| {
        ui.label(RichText::new(ic(ph::SCROLL, "Generation log")).strong());
        if st.job_log.is_empty() {
            return;
        }
        if ui.checkbox(&mut full, "full paths").on_hover_text("the model files' folders too").changed() {
            ui.data_mut(|d| d.insert_temp(full_id, full));
        }
        if ui.small_button(ic(ph::COPY, "Copy")).on_hover_text("copy the log, e.g. into a bug report").clicked() {
            ui.ctx().copy_text(st.job_log.join("\n"));
        }
        if ui.small_button(ic(ph::FOLDER_OPEN, "Show file")).on_hover_text(file.display().to_string()).clicked() {
            out.push(Msg::ShowInFileManager(file.to_path_buf()));
        }
    });
    if st.job_log.is_empty() {
        ui.weak("Nothing logged yet: a run is logged from when it is queued.");
        return;
    }
    egui::ScrollArea::vertical().id_salt("job_log").max_height(220.0).stick_to_bottom(true).show(ui, |ui| {
        for line in &st.job_log {
            ui.add(egui::Label::new(crate::logs::line_job(ui, line, "", false, !full)).wrap());
        }
    });
}

/// A job's details: progress, the prompt, the settings, and the images it
/// was given (click one to see it large).
pub fn job_window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    use increart_core::model::Mode;
    let Some(id) = &st.job_info else { return };
    let (Some((p, run)), Some(root)) = (st.index.run(id), &st.library) else { return };
    let progress = st.run_progress(id);
    let active = st.active_runs.contains(id);
    let m = egui::Modal::new(egui::Id::unique("job_info")).show(ctx, |ui| {
        ui.set_width(640.0);
        ui.horizontal(|ui| {
            ui.heading(ic(ph::QUEUE, &format!("{} · {}", p.name(), run.label)));
        });
        if let Some(j) = &progress {
            // generating only once a server has one of its seeds
            let on: Vec<&str> = st
                .server_jobs
                .iter()
                .filter(|(_, runs)| runs.contains(id))
                .filter_map(|(i, _)| st.servers.get(*i).map(|s| s.name.as_str()))
                .collect();
            let sampling = st.progress.keys().any(|(r, _)| r == id);
            let state = match (active, on.is_empty()) {
                (false, _) => "not queued".to_string(),
                (true, true) if sampling => "generating".to_string(),
                (true, true) => "waiting for a server (see the log)".to_string(),
                (true, false) => format!("generating on {}", on.join(", ")),
            };
            ui.add(egui::ProgressBar::new(j.fraction()).text(format!("{} / {} seeds · {state}", j.done, j.total)));
            sampling_bars(ui, j);
        }
        ui.add_space(4.0);
        egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.7).show(ui, |ui| {
            ui.weak("Prompt");
            ui.add(egui::Label::new(&run.prompt).wrap());
            if !run.negative_prompt.is_empty() {
                ui.weak("Negative prompt");
                ui.add(egui::Label::new(&run.negative_prompt).wrap());
            }
            ui.add_space(6.0);
            // the images it was given: references by their tag, the start image
            let init = match &run.mode {
                Mode::Edit { init, .. } => init.as_ref(),
                Mode::Generate => None,
            };
            let mut images: Vec<(String, &increart_core::ImageRef)> = run
                .refs
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let start = if Some(r) == init { " · start" } else { "" };
                    (format!("<image{}>{start}", i + 1), r)
                })
                .collect();
            if let Some(i) = init.filter(|i| !run.refs.contains(i)) {
                images.push(("start image".into(), i));
            }
            if images.is_empty() {
                ui.weak("No input images (text to image).");
            } else {
                ui.weak("Input images · click to view");
                ui.horizontal_wrapped(|ui| {
                    for (label, img) in &images {
                        ui.vertical(|ui| {
                            let t = tex.thumb(root, img);
                            let r = thumb(ui, t.as_ref(), 120.0, false, None)
                                .on_hover_cursor(egui::CursorIcon::ZoomIn)
                                .on_hover_text(tex.dims_text(root, img));
                            if r.clicked() {
                                out.push(Msg::Preview(Some((*img).clone())));
                            }
                            ui.label(RichText::new(label).monospace());
                        });
                    }
                });
            }
            ui.add_space(6.0);
            let mode = match &run.mode {
                Mode::Generate => "generate".to_string(),
                Mode::Edit { strength, .. } => format!("img2img, strength {strength}"),
            };
            let s = &run.sample;
            let seeds: Vec<String> = run
                .revisions
                .iter()
                .map(|r| match r.seeds.count {
                    1 => format!("rev {}: seed {}", r.rev, r.seeds.first),
                    n => format!("rev {}: seeds {}–{}", r.rev, r.seeds.first, r.seeds.first + n - 1),
                })
                .collect();
            let mut rows: Vec<(&str, String)> = vec![
                ("created", run.created.format("%Y-%m-%d %H:%M").to_string()),
                ("mode", mode),
                ("model", run.model.recipe.clone()),
                ("size", crate::widgets::size_info(run.width, run.height)),
                ("steps / cfg", format!("{} / {}", s.steps, s.cfg)),
                (
                    "sampler",
                    if s.scheduler.is_empty() { s.method.clone() } else { format!("{} · {}", s.method, s.scheduler) },
                ),
                ("flow shift", s.flow_shift.map_or("recipe schedule".into(), |f| f.to_string())),
                ("alpha", if run.alpha { "transparent background".into() } else { "no".into() }),
                ("seeds", if seeds.is_empty() { "none queued".into() } else { seeds.join(" · ") }),
            ];
            if let Some(l) = run.model.loras_text() {
                rows.insert(3, ("LoRAs", l));
            }
            if let Some(t) = &run.sweep {
                rows.push(match &t.param {
                    Some(p) => ("sweep", format!("{} · {}", t.name, p.label())),
                    None => ("sweep", format!("{} · {} → {}", t.name, t.task, t.preset)),
                });
            }
            if let Some(idea) = run.source_idea.as_ref().and_then(|i| st.index.idea(i)) {
                let t = &idea.idea.title;
                rows.push(("from idea", if t.is_empty() { "(untitled)".into() } else { t.clone() }));
            }
            let cancelled: u64 = run.cancelled.iter().map(|c| c.count).sum();
            if cancelled > 0 {
                rows.push(("cancelled", format!("{cancelled} seed(s)")));
            }
            egui::Grid::new("job_info_grid").num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
                for (k, v) in rows {
                    ui.weak(k);
                    ui.label(v);
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new(ic(ph::TIMER, "Timing")).strong());
            match increart_gui_core::timing::Timing::of_run(run) {
                Some(t) => {
                    let mut rows = timing_rows(&t);
                    if let Some(text) = progress.as_ref().filter(|_| active).and_then(|j| live_timing(st, j)) {
                        rows.insert(0, ("now", text));
                    }
                    if let Some(all) =
                        increart_gui_core::timing::Timing::of_runs(&p.runs).filter(|a| a.images > t.images)
                    {
                        rows.push((
                            "project",
                            format!(
                                "{} image(s) · {}/image · {} · GPU time {}",
                                all.images,
                                increart_gui_core::timing::duration(all.per_image),
                                all.step_text(),
                                increart_gui_core::timing::duration(all.gpu_s)
                            ),
                        ));
                    }
                    egui::Grid::new("job_timing_grid").num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
                        for (k, v) in rows {
                            ui.weak(k);
                            ui.label(v);
                            ui.end_row();
                        }
                    });
                }
                None => {
                    ui.weak("No timed images yet (images are timed as they are made).");
                    if let Some(text) = progress.as_ref().filter(|_| active).and_then(|j| live_timing(st, j)) {
                        ui.label(format!("now: {text}"));
                    }
                }
            }
            if !run.failed.is_empty() {
                ui.add_space(6.0);
                ui.colored_label(BAD, format!("{} seed(s) failed", run.failed.len()));
                for f in run.failed.iter().take(5) {
                    ui.add(egui::Label::new(RichText::new(format!("seed {}: {}", f.seed, f.error)).small()).wrap());
                }
            }
            ui.add_space(6.0);
            job_log(ui, st, &increart_core::engine::run_log(&root.join(&p.dir), id), out);
        });
        ui.separator();
        if active {
            ui.horizontal(|ui| {
                let finishing = st.finishing.contains(id);
                let sampling = st.progress.keys().filter(|(r, _)| r == id).count();
                stop_buttons(ui, id, finishing, sampling, false, out);
            });
        }
        dialog_footer(
            ui,
            |ui| {
                if ui.button(ic(ph::COPY, "Copy prompt")).clicked() {
                    ui.ctx().copy_text(run.prompt.clone());
                }
            },
            |ui| {
                if ui
                    .button(ic(ph::ARROW_SQUARE_OUT, "Open in Generate"))
                    .on_hover_text("this run's settings in the Generate form")
                    .clicked()
                {
                    out.push(Msg::JobInfo(None));
                    out.push(Msg::RunInGenerate(id.clone()));
                }
                if ui
                    .button(ic(ph::CLOCK_COUNTER_CLOCKWISE, "Show in history"))
                    .on_hover_text("the Generate history that led to this run, at its queued checkpoint")
                    .clicked()
                {
                    out.push(Msg::JobInfo(None));
                    out.push(Msg::RunHistory(id.clone()));
                }
                ui.button("Close   Esc").clicked()
            },
        )
    });
    if m.inner.1 || m.should_close() {
        out.push(Msg::JobInfo(None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tags_go_in_at_the_cursor() {
        let mut f = increart_gui_core::GenerateForm::new(&increart_core::config::Config::default());
        let tag = |f: &increart_gui_core::GenerateForm, sel| {
            let (g, end) = with_tag(f, "<image1>", sel);
            (g.prompt, end)
        };
        f.prompt = "a fox, café style".into();
        // never focused: at the end
        assert_eq!(tag(&f, None), ("a fox, café style <image1>".into(), 26));
        // mid-word cursor: spaced on both sides; counted in characters (é)
        assert_eq!(tag(&f, Some(2..2)), ("a <image1> fox, café style".into(), 10));
        assert_eq!(tag(&f, Some(11..11)), ("a fox, café <image1> style".into(), 20));
        assert_eq!(tag(&f, Some(10..10)), ("a fox, caf <image1> é style".into(), 19));
        // a selection is replaced
        assert_eq!(tag(&f, Some(2..5)), ("a <image1>, café style".into(), 10));
        f.prompt = "a fox (in ) here".into();
        assert_eq!(tag(&f, Some(10..10)), ("a fox (in <image1>) here".into(), 18));
        f.prompt.clear();
        assert_eq!(tag(&f, Some(0..0)), ("<image1>".into(), 8));
    }
}
