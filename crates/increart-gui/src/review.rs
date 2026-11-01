//! Review (the output queue: Single · Grid · A/B compare) and the Output
//! library (Grid with its Kept · Good · Rejected · Unreviewed · All tabs ·
//! Folder).

use std::path::{Path, PathBuf};

use eframe::egui::{self, Key, Panel, RichText, ScrollArea, Sense, Vec2};
use increart_core::Tree;
use increart_core::Verdict;
use increart_core::config::ReviewOrder;
use increart_core::scan::OutputLink;
use increart_gui_core::{
    AppState, Confirm, GRID_CELL_MAX, GRID_CELL_MIN, InfoPanel, Msg, OutputPreset, OutputSource, OutputTab, Ratings,
    ReviewNode, ReviewRun, ReviewView, Smart, Tab, UseTarget, ZoomGrid, counts,
};

use crate::textures::Textures;
use crate::widgets::{
    BAD, GOOD, NEUTRAL, ic, image_fit_bg, image_menu_items, ph, short, single_click, thumb, verdict_color,
};

pub const VERDICTS: [(&str, Verdict, egui::Color32); 3] =
    [("Good", Verdict::Good, GOOD), ("Neutral", Verdict::Neutral, NEUTRAL), ("Bad", Verdict::Bad, BAD)];

fn keys(ui: &egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    // the full-size preview or the picker is up: its keys are its own
    if ui.ctx().egui_wants_keyboard_input()
        || st.preview.is_some()
        || st.ref_picker.is_some()
        || st.collect.is_some()
        || st.tagging.is_some()
    {
        return;
    }
    // Ctrl+G: Generate (taken first, so G doesn't rate too)
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::G))
        && let Some(g) = st.review_generate()
    {
        out.push(Msg::OpenInGenerate(g.project));
    }
    // with picks, the rating keys rate them all
    let picking = crate::tiles::keys(ui, st, out);
    if st.review_view == ReviewView::Sheet {
        return sheet_keys(ui, st, picking, out);
    }
    ui.input(|i| {
        for (keys, v) in [
            ([Key::Num1, Key::G], Verdict::Good),
            ([Key::Num2, Key::N], Verdict::Neutral),
            ([Key::Num3, Key::B], Verdict::Bad),
        ] {
            if !picking && keys.iter().any(|k| i.key_pressed(*k)) {
                out.push(Msg::ReviewCurrent(v));
            }
        }
        if i.key_pressed(Key::ArrowRight) {
            out.push(Msg::ReviewNext(1));
        }
        if i.key_pressed(Key::ArrowLeft) {
            out.push(Msg::ReviewNext(-1));
        }
        // the oldest / newest in the queue
        if i.key_pressed(Key::Home) {
            out.push(Msg::ReviewCursor(0));
        }
        if i.key_pressed(Key::End) {
            out.push(Msg::ReviewNewest);
        }
        if i.key_pressed(Key::Space) {
            out.push(Msg::ToggleReviewGrid);
        }
        if i.key_pressed(Key::C) && !i.modifiers.command {
            out.push(if i.modifiers.shift { Msg::CompareWith(None) } else { Msg::Compare });
        }
        if i.key_pressed(Key::Num4) || (i.modifiers.command && i.key_pressed(Key::Z)) {
            out.push(Msg::UndoReview);
        }
        if !i.modifiers.command
            && !picking
            && let Some(o) = st.review_queue().get(st.review_cursor)
        {
            tag_keys(i, vec![o.image.clone()], out);
            for (key, msg) in [
                (Key::E, Msg::UseImage(o.image.clone(), UseTarget::Edit)),
                (Key::N, Msg::UseImage(o.image.clone(), UseTarget::NewProject)),
                (Key::D, Msg::UseImage(o.image.clone(), UseTarget::DuplicateProject)),
                (Key::R, Msg::ToInputLibrary(o.image.clone())),
                (Key::I, Msg::NewIdeaFromImage(o.image.clone())),
            ] {
                if i.key_pressed(key) {
                    out.push(msg);
                }
            }
        }
    });
    let _ = st;
}

/// "fox-cafe · r02 cfg6 · rev 0 · seed 3".
fn describe(st: &AppState, o: &OutputLink) -> String {
    let what = describe_run(st, o);
    match st.image_title(&o.image) {
        Some(t) => format!("“{t}”\n{what}"),
        None => what,
    }
}

fn describe_run(st: &AppState, o: &OutputLink) -> String {
    match st.index.run(&o.name.run) {
        Some((p, run)) => {
            let what = format!("{} · {} · rev {} · seed {}", p.name(), run.label, o.name.rev, o.name.seed);
            match run.timings.iter().find(|t| t.seed == o.name.seed) {
                Some(t) => format!("{what}\n{}", increart_gui_core::timing::image_text(t, run.sample.steps)),
                None => what,
            }
        }
        None => format!("seed {}", o.name.seed),
    }
}

pub fn view(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let Some(root) = st.library.clone() else { return };
    keys(ui, st, out);
    let queue = st.review_queue();
    Panel::left("review_scope").default_size(230.0).show(ui, |ui| scope_panel(ui, st, out));
    Panel::top("review_tabs").show(ui, |ui| {
        // groups wrap whole when the bar is short of room
        use crate::widgets::{group, right_group};
        ui.horizontal_wrapped(|ui| {
            group(ui, "views", |ui| {
                for (v, label) in [
                    (ReviewView::Single, ic(ph::SQUARE, "Single")),
                    (ReviewView::Grid, ic(ph::SQUARES_FOUR, "Grid")),
                    (ReviewView::Compare, ic(ph::COLUMNS, "A/B compare")),
                    (ReviewView::Sheet, ic(ph::TABLE, "Contact sheet")),
                ] {
                    if v == ReviewView::Sheet && st.review_sweep.is_none() {
                        continue;
                    }
                    if ui.selectable_label(st.review_view == v, label).clicked() && st.review_view != v {
                        out.push(Msg::ReviewView(v));
                    }
                }
                if ui
                    .add_enabled(!queue.is_empty(), egui::Button::new(ic(ph::IMAGES, "Compare with…")))
                    .on_hover_text("compare this image (B) with one from its references or the libraries (A)   Shift+C")
                    .clicked()
                {
                    out.push(Msg::CompareWith(None));
                }
            });
            group(ui, "queue", |ui| {
                ui.separator();
                let generating = st.progress.len();
                ui.weak(format!("{} unreviewed · oldest first", queue.len()));
                if generating > 0 {
                    ui.weak(format!("· {generating} generating, new images join the end"));
                }
                if ui.small_button("newest ›").on_hover_text("jump to the newest unreviewed image").clicked() {
                    out.push(Msg::ReviewNewest);
                }
                follow_controls(ui, st, out);
            });
            // the current image is one style of a sweep: its contact sheet
            let sweep = queue
                .get(st.review_cursor)
                .filter(|_| st.review_view != ReviewView::Sheet)
                .and_then(|o| st.index.run(&o.name.run))
                .and_then(|(p, r)| Some((p.project.id.clone(), r.sweep.as_ref()?)));
            if let Some((project, tag)) = sweep {
                group(ui, "sweep", |ui| {
                    ui.separator();
                    let label = RichText::new(ic(ph::TABLE, &format!("Sweep {} contact sheet", tag.name)))
                        .color(crate::widgets::ACCENT);
                    if ui
                        .button(label)
                        .on_hover_text(format!(
                            "this image is the {} row of sweep {}: see every row side by side",
                            tag.row(),
                            tag.name
                        ))
                        .clicked()
                    {
                        out.push(Msg::ReviewSweep(project, tag.name.clone()));
                    }
                });
            }
            group(ui, "generate", |ui| {
                ui.separator();
                generate_button(ui, st, out);
            });
            if st.review_view == ReviewView::Grid && !queue.is_empty() {
                group(ui, "zoom", |ui| {
                    ui.separator();
                    grid_zoom(ui, st, ZoomGrid::Review, out);
                });
            }
            right_group(ui, "right", |ui| {
                if st.review_view == ReviewView::Grid {
                    crate::widgets::details_toggle(ui, st, InfoPanel::Review, out);
                }
                let mut on = st.checkerboard;
                if ui.checkbox(&mut on, "checkerboard behind alpha").changed() {
                    out.push(Msg::Checkerboard(on));
                }
            });
        });
    });
    crate::tiles::bar(ui, st, out);
    if queue.is_empty() && !matches!(st.review_view, ReviewView::Compare | ReviewView::Sheet) {
        egui::CentralPanel::default().show(ui, |ui| {
            let more = if st.active_runs.is_empty() { "" } else { " Images are still being generated." };
            ui.weak(format!("Nothing to review in this scope.{more}"));
        });
        return;
    }
    match st.review_view {
        ReviewView::Single => single(ui, st, &root, tex, out),
        ReviewView::Grid => grid(ui, st, &root, tex, out),
        ReviewView::Compare => compare(ui, st, &root, tex, out),
        ReviewView::Sheet => sheet(ui, st, &root, tex, out),
    }
}

/// **Generate ›**: the shown image's project in Generate as it is; ▾ with
/// its run's settings, or its recipe & seed.
fn generate_button(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let g = st.review_generate();
    let tip = match &g {
        Some(g) => format!("open {} in Generate   Ctrl+G", g.name),
        None => "pick a project, or an image, to open its Generate".to_string(),
    };
    ui.spacing_mut().item_spacing.x = 1.0;
    let open = ui.add_enabled(g.is_some(), egui::Button::new(ic(ph::ROCKET_LAUNCH, "Generate ›")));
    if open.on_hover_text(tip).on_disabled_hover_text("pick a project, or an image, to open its Generate").clicked()
        && let Some(g) = &g
    {
        out.push(Msg::OpenInGenerate(g.project.clone()));
    }
    let more = g.as_ref().is_some_and(|g| g.run.is_some() || g.image.is_some());
    ui.add_enabled_ui(more, |ui| {
        ui.menu_button(ph::CARET_DOWN, |ui| {
            let Some(g) = &g else { return };
            if let Some(run) = &g.run
                && ui.button(ic(ph::ARROW_SQUARE_OUT, "…with this run's settings")).clicked()
            {
                out.push(Msg::RunInGenerate(run.clone()));
                ui.close();
            }
            if let Some(image) = &g.image
                && ui.button(ic(ph::REPEAT_ONCE, "…with this image's recipe & seed")).clicked()
            {
                out.push(Msg::RecipeInGenerate(image.clone()));
                ui.close();
            }
        });
    });
}

/// Follow's toggle, its countdown ring (click: pause) and its pace.
fn follow_controls(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let follow = ui
        .selectable_label(st.review_follow, ic(ph::BROADCAST, "Follow"))
        .on_hover_text("move on through the unreviewed images by itself, up to the newest; going back turns it off");
    if follow.clicked() {
        out.push(Msg::ReviewFollow(!st.review_follow));
    }
    if let Some(c) = st.follow_countdown() {
        let (rect, ring) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
        let p = ui.painter_at(rect.expand(2.0));
        let (center, radius) = (rect.center(), 7.0);
        let weak = ui.visuals().weak_text_color();
        p.circle_stroke(center, radius, egui::Stroke::new(2.0, weak.gamma_multiply(0.4)));
        let waiting = c.ahead == 0;
        let color = if c.paused || c.held || waiting { weak } else { crate::widgets::ACCENT };
        // the time left, clockwise from the top
        let left = if waiting { 1.0 } else { (c.left / c.secs.max(0.01)).clamp(0.0, 1.0) as f32 };
        let n = (48.0 * left).ceil().max(1.0) as usize;
        let arc = (0..=n)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * left * i as f32 / n as f32;
                center + radius * Vec2::angled(a)
            })
            .collect();
        p.add(egui::Shape::line(arc, egui::Stroke::new(2.0, color)));
        if c.paused {
            for dx in [-1.75, 1.75] {
                let x = center.x + dx;
                p.line_segment([egui::pos2(x, center.y - 3.0), egui::pos2(x, center.y + 3.0)], (1.5, weak));
            }
        }
        let tip = if c.paused {
            "paused — click to go on".to_string()
        } else if c.held {
            "held: the pointer is over the image (or Review isn't showing the queue)".to_string()
        } else if waiting {
            "caught up — waiting for the next image".to_string()
        } else {
            format!("next in {:.0} s · {} to go · {:.0} s an image — click to pause", c.left.ceil(), c.ahead, c.secs)
        };
        if ring.on_hover_text(tip).clicked() {
            out.push(Msg::FollowPause(!c.paused));
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    ui.menu_button(ph::GEAR_SIX, |ui| follow_pace(ui, st, out)).response.on_hover_text("Follow's pace");
}

fn secs(v: &mut f32) -> egui::DragValue<'_> {
    egui::DragValue::new(v).range(0.5..=3600.0).speed(0.5).suffix(" s")
}

fn follow_pace(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let mut p = st.config.review_follow.clone();
    ui.strong("Follow's pace");
    ui.radio_value(&mut p.auto, true, "Auto: as fast as images are made")
        .on_hover_text("each image as long as one takes to make lately, quicker when it falls behind");
    ui.radio_value(&mut p.auto, false, "Fixed time an image");
    egui::Grid::new("follow_pace").num_columns(2).show(ui, |ui| {
        ui.label(if p.auto { "while nothing is made" } else { "each image" });
        ui.add(secs(&mut p.fixed_secs));
        ui.end_row();
        ui.add_enabled_ui(p.auto, |ui| ui.label("keep this many ahead"));
        ui.add_enabled(p.auto, egui::DragValue::new(&mut p.lag).range(0..=100))
            .on_hover_text("ahead of it: quicker, to catch up; short of it: slower");
        ui.end_row();
        ui.add_enabled_ui(p.auto, |ui| ui.label("shortest"));
        ui.add_enabled(p.auto, secs(&mut p.min_secs));
        ui.end_row();
        ui.add_enabled_ui(p.auto, |ui| ui.label("longest"));
        ui.add_enabled(p.auto, secs(&mut p.max_secs));
        ui.end_row();
    });
    ui.checkbox(&mut p.start_newest, "start at the newest image").on_hover_text("else at the image shown");
    ui.checkbox(&mut p.pause_on_hover, "hold while the pointer is over the image");
    if ui.add_enabled(p != Default::default(), egui::Button::new("Defaults")).clicked() {
        p = Default::default();
    }
    if p != st.config.review_follow {
        out.push(Msg::FollowPace(p));
    }
}

/// Follow waits while the pointer is over the image, when set so.
fn follow_hold(ui: &egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let held = st.review_follow && st.config.review_follow.pause_on_hover && ui.ui_contains_pointer();
    if held != st.follow.held {
        out.push(Msg::FollowHold(held));
    }
}

fn scope_panel(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let sheet = st.review_view == ReviewView::Sheet;
    Panel::bottom("review_keys").show(ui, |ui| key_hints(ui, sheet));
    // right to left: the clear button, then the field takes what's left
    let row = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let mut q = st.review_search.clone();
        if !q.is_empty() && ui.small_button(ph::X).on_hover_text("clear").clicked() {
            out.push(Msg::ReviewSearch(String::new()));
        }
        let field = egui::TextEdit::singleline(&mut q)
            .hint_text(ic(ph::MAGNIFYING_GLASS, "Search projects"))
            .desired_width(ui.available_width());
        let r = ui.add(field);
        if r.changed() {
            out.push(Msg::ReviewSearch(q.clone()));
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            out.push(Msg::ReviewSearch(String::new()));
        }
    });
    ui.label(RichText::new("Scope").strong());
    ui.weak("independent of the Projects tab");
    let mut all_runs = st.review_all_runs;
    let tip = "list every project's runs (else only the selected project's)";
    if ui.checkbox(&mut all_runs, "runs of all projects").on_hover_text(tip).changed() {
        out.push(Msg::ReviewAllRuns(all_runs));
    }
    ui.horizontal_wrapped(|ui| {
        // one group: the label wraps with its dropdown
        crate::widgets::group(ui, "sort", |ui| {
            ui.label(ic(ph::SORT_ASCENDING, "Sort"));
            let order = st.config.review_order;
            egui::ComboBox::from_id_salt("review_order")
                .selected_text(order.name())
                .show_ui(ui, |ui| {
                    for o in ReviewOrder::EVERY {
                        if ui.selectable_label(order == o, o.name()).clicked() && order != o {
                            out.push(Msg::ReviewOrder(o));
                        }
                    }
                })
                .response
                .on_hover_text(
                    "folders and projects: newest created first, last run first, A–Z, or the oldest projects \
                     with images to review first, then those with none (a folder goes by the projects in it); \
                     runs and sweeps stay newest first",
                );
        });
        let mut hide = !st.config.review_show_reviewed;
        let tip = "hide folders, projects, runs and sweeps with nothing to review (the selected one stays)";
        if ui.checkbox(&mut hide, "hide reviewed").on_hover_text(tip).changed() {
            out.push(Msg::ReviewHideReviewed(hide));
        }
    });
    let all = st.review_scope.is_none() && st.review_folder.is_none();
    if ui.selectable_label(all, format!("All projects ({})", st.unreviewed_outputs())).clicked() {
        out.push(Msg::ReviewScope(None));
    }
    /// Whether the folder's name or anything under it matches the search
    /// (lowercase).
    fn folder_matches(st: &AppState, f: &Path, q: &str) -> bool {
        let hit = |name: &str| name.to_lowercase().contains(q);
        let dir = Path::new("projects").join(f);
        hit(&f.file_name().unwrap_or_default().to_string_lossy())
            || st.index.projects.iter().any(|p| p.dir.starts_with(&dir) && hit(&p.name()))
            || st
                .index
                .project_folders
                .iter()
                .any(|g| g != f && g.starts_with(f) && hit(&g.file_name().unwrap_or_default().to_string_lossy()))
    }
    /// `q`: the search (lowercase); a matching folder shows all it holds.
    fn rec(ui: &mut egui::Ui, st: &AppState, parent: &Path, q: &str, out: &mut Vec<Msg>) {
        let depth = parent.components().count() as f32 + 1.0;
        for node in st.review_tree_nodes(parent) {
            match node {
                ReviewNode::Folder(f) => {
                    if !q.is_empty() && !folder_matches(st, f, q) {
                        continue;
                    }
                    let own = f.file_name().unwrap_or_default().to_string_lossy().to_lowercase().contains(q);
                    let n = st.unreviewed_under(f);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0 * depth);
                        let name = f.file_name().unwrap_or_default().to_string_lossy();
                        let sel = st.review_folder.as_deref() == Some(f.as_path());
                        if ui.selectable_label(sel, ic(ph::FOLDER, &format!("{name} ({n})"))).clicked() {
                            out.push(Msg::ReviewScopeFolder(f.clone()));
                        }
                    });
                    rec(ui, st, f, if own { "" } else { q }, out);
                }
                ReviewNode::Project(p) => {
                    if !q.is_empty() && !p.name().to_lowercase().contains(q) {
                        continue;
                    }
                    let u = counts(p)[3];
                    ui.horizontal(|ui| {
                        ui.add_space(12.0 * depth);
                        // the whole project; a run or sweep of it is selected below
                        let sel = st.review_scope.as_ref() == Some(&p.project.id)
                            && st.review_sweep.is_none()
                            && st.review_run.is_none();
                        let label = ic(ph::ROCKET_LAUNCH, &format!("{} ({u})", p.name()));
                        let text = if u == 0 { RichText::new(label).weak() } else { RichText::new(label) };
                        if ui.selectable_label(sel, text).clicked() {
                            out.push(Msg::ReviewScope(Some(p.project.id.clone())));
                        }
                    });
                    let selected = st.review_scope.as_ref() == Some(&p.project.id);
                    if !selected && !st.review_all_runs {
                        continue;
                    }
                    for row in st.review_runs(p) {
                        let (sel, label, tip, u, msg) = match row {
                            ReviewRun::Single(r) => {
                                let label = if r.label.is_empty() { r.id.to_string() } else { r.label.clone() };
                                let sel = selected && st.review_run.as_ref() == Some(&r.id);
                                let u = st.run_unreviewed(p, &r.id);
                                let msg = Msg::ReviewRun(p.project.id.clone(), r.id.clone());
                                (sel, ic(ph::IMAGES, &format!("{label} ({u})")), "this run's images", u, msg)
                            }
                            ReviewRun::Sweep(name) => {
                                let sel = selected && st.review_sweep.as_deref() == Some(name);
                                let u = st.sweep_unreviewed(p, name);
                                let msg = Msg::ReviewSweep(p.project.id.clone(), name.to_string());
                                (
                                    sel,
                                    ic(ph::TABLE, &format!("sweep {name} ({u})")),
                                    "this style sweep's contact sheet",
                                    u,
                                    msg,
                                )
                            }
                        };
                        if u == 0 && !sel && !st.config.review_show_reviewed {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            ui.add_space(12.0 * depth + 18.0);
                            let text = if u == 0 { RichText::new(label).weak() } else { RichText::new(label) };
                            if ui.selectable_label(sel, text).on_hover_text(tip).clicked() {
                                out.push(msg);
                            }
                        });
                    }
                }
            }
        }
    }
    let q = st.review_search.trim().to_lowercase();
    ScrollArea::vertical().auto_shrink([false, true]).id_salt("review_scope_tree").show(ui, |ui| {
        if !q.is_empty() && !folder_matches(st, Path::new(""), &q) {
            ui.weak("No project or folder matches.");
        } else if q.is_empty() && st.review_tree_children(Path::new("")) == (vec![], vec![]) {
            ui.weak("Nothing to review. Untick hide reviewed to see every project.");
        }
        rec(ui, st, Path::new(""), &q, out);
    });
}

/// Review's keys, folded away at the bottom of the scope panel; the contact
/// sheet has fewer.
fn key_hints(ui: &mut egui::Ui, sheet: bool) {
    const SHEET: &[(&str, &str)] = &[
        ("1 / G", "good"),
        ("2 / N", "neutral"),
        ("3 / B", "bad"),
        ("← → ↑ ↓", "move"),
        ("Home / End", "row's first / last"),
        ("Ctrl+Home / End", "sheet's first / last"),
        ("L", "tag…"),
        ("Shift+L", "tag again"),
        ("Ctrl+G", "open in Generate"),
        ("4 / Ctrl+Z", "undo"),
    ];
    egui::CollapsingHeader::new(RichText::new(ic(ph::KEYBOARD, "Keys")).strong())
        .id_salt("review_keys")
        .default_open(true)
        .show(ui, |ui| {
            let all: &[(&str, &str)] = &[
                ("1 / G", "good"),
                ("2 / N", "neutral"),
                ("3 / B", "bad"),
                ("← →", "previous / next"),
                ("Home / End", "oldest / newest"),
                ("Space", "grid / single"),
                ("C", "A/B compare"),
                ("Shift+C", "compare with…"),
                ("E", "edit this image"),
                ("N", "new project from it"),
                ("D", "duplicate its project"),
                ("R", "→ input library"),
                ("I", "→ new idea"),
                ("L", "tag…"),
                ("Shift+L", "tag again"),
                ("Ctrl+G", "open in Generate"),
                ("4 / Ctrl+Z", "undo"),
            ];
            for &(k, what) in if sheet { SHEET } else { all } {
                ui.horizontal(|ui| {
                    ui.monospace(k);
                    ui.weak(what);
                });
            }
        });
}

fn verdict_buttons(ui: &mut egui::Ui, out: &mut Vec<Msg>, msg: impl Fn(Verdict) -> Msg) {
    ui.horizontal(|ui| {
        for (label, v, c) in VERDICTS {
            if ui.button(RichText::new(label).color(c)).clicked() {
                out.push(msg(v));
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn info_panel(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    item: &OutputLink,
    pos: usize,
    n: usize,
    out: &mut Vec<Msg>,
) {
    ui.label(RichText::new(format!("{} / {n}", pos + 1)).strong());
    ui.label(describe(st, item));
    tag_chips(ui, st, &item.image, out);
    recipe_grid(ui, st, root, tex, &item.image, out);
}

/// The recipe of an output (loaded on demand), as a two-column grid.
pub fn recipe_grid(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    image: &increart_core::ImageRef,
    out: &mut Vec<Msg>,
) {
    let Some(loaded) = st.recipes.get(&image.id) else {
        out.push(Msg::NeedRecipe(image.id.clone()));
        ui.spinner();
        return;
    };
    let Some(r) = loaded else {
        ui.weak("no recipe in this image");
        return;
    };
    let link = st.output_link(image);
    let project = link.and_then(|o| st.project_of_link(o)).map(|p| p.name()).unwrap_or_default();
    let run_entry = link.and_then(|o| st.index.run(&o.name.run)).map(|(_, r)| r);
    let run = run_entry.map(|r| r.label.clone()).unwrap_or_default();
    // queued with its revision; generated when stored (older runs have no
    // timing: the image ID's minute)
    let queued = link
        .zip(run_entry)
        .and_then(|(o, r)| r.revisions.iter().find(|v| v.rev == o.name.rev))
        .map(|v| crate::widgets::when(v.created));
    let timing =
        link.zip(run_entry).and_then(|(o, r)| r.timings.iter().find(|t| t.rev == o.name.rev && t.seed == o.name.seed));
    let generated = match timing {
        Some(t) => Some(crate::widgets::when(t.done)),
        None => image.id.time().map(|t| format!("{} (from its ID)", crate::widgets::when(t))),
    };
    let mode = match &r.mode {
        increart_core::model::Mode::Generate => "generate".to_string(),
        increart_core::model::Mode::Edit { strength, init } => {
            let from = init.as_ref().map(|i| tex.dims_text(root, i)).filter(|d| !d.is_empty());
            let keep = ((1.0 - strength) * 100.0).round();
            let from = from.map(|d| format!(", from {d}")).unwrap_or_default();
            format!("img2img, kept {keep:.0} % (strength {strength}){from}")
        }
    };
    egui::Grid::new(ui.scope_id().with("recipe")).num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
        let mut rows: Vec<(&str, String)> = vec![("project", project), ("run", run)];
        rows.extend(queued.map(|q| ("queued", q)));
        rows.extend(generated.map(|g| ("generated", g)));
        rows.extend([("seed", r.seed.to_string()), ("recipe", r.model.recipe.clone())]);
        rows.extend(r.model.loras_text().map(|l| ("LoRAs", l)));
        rows.extend([
            ("size", crate::widgets::size_info(r.width, r.height)),
            ("steps / cfg", format!("{} / {}", r.sample.steps, r.sample.cfg)),
            ("mode", mode),
            ("alpha", r.alpha.to_string()),
            ("refs", {
                let sizes: Vec<String> = r.refs.iter().map(|i| tex.dims_text(root, i)).collect();
                if sizes.is_empty() { "0".into() } else { format!("{}: {}", sizes.len(), sizes.join(", ")) }
            }),
            ("parent", r.parent.as_ref().map(|p| p.id.to_string()).unwrap_or_else(|| "—".into())),
            ("GPU", r.device.clone()),
            ("elapsed", format!("{:.1} s", r.elapsed_s)),
            ("sd.cpp", r.sdcpp.clone()),
        ]);
        for (k, v) in rows {
            ui.weak(k);
            ui.label(v);
            ui.end_row();
        }
    });
    if !r.inputs.is_empty() {
        ui.weak("inputs");
        egui::Grid::new(ui.scope_id().with("inputs")).num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
            for i in &r.inputs {
                ui.weak(match i.role {
                    increart_core::model::InputRole::Ref(n) => format!("<image{n}>"),
                    increart_core::model::InputRole::Start => "start".into(),
                });
                let name = i.original_name.clone().unwrap_or_else(|| i.file.clone());
                ui.label(RichText::new(short(&name, 34)).small())
                    .on_hover_text(format!("{}\nBLAKE3 {}", i.file, i.blake3));
                ui.end_row();
            }
        });
    }
    ui.weak("prompt_sent");
    ui.add(egui::Label::new(r.prompt_sent.as_str()).wrap());
    if !r.negative_prompt.is_empty() {
        ui.weak("negative");
        ui.add(egui::Label::new(r.negative_prompt.as_str()).wrap());
    }
}

fn single(ui: &mut egui::Ui, st: &AppState, root: &Path, tex: &mut Textures, out: &mut Vec<Msg>) {
    let queue = st.review_queue();
    let cur = st.review_cursor.min(queue.len() - 1);
    let item = queue[cur];
    Panel::bottom("review_strip").default_size(110.0).show(ui, |ui| {
        let bg = crate::tiles::background(ui, st, out);
        let mut rects = vec![];
        ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, o) in queue.iter().enumerate().take(cur + 40).skip(cur.saturating_sub(10)) {
                    let t = tex.thumb(root, &o.image);
                    let r = thumb(ui, t.as_ref(), 90.0, crate::tiles::cursor(st, i == cur), None);
                    if i == cur {
                        crate::widgets::reveal_cursor(ui, "strip", o.image.clone(), &r);
                    }
                    crate::tiles::mark(ui, st, r.rect, &o.image);
                    rects.push((o.image.clone(), r.rect));
                    if !crate::tiles::click(ui, st, &r, &o.image, out) && r.clicked() {
                        out.push(Msg::ReviewCursor(i));
                    }
                    r.context_menu(|ui| {
                        crate::tiles::menu(ui, st, &o.image, out);
                        use_menu(ui, st, root, &o.image, out);
                        ui.separator();
                        collect_button(ui, st, &o.image, out);
                        tag_button(ui, st, &o.image, out);
                    });
                }
            });
        });
        crate::tiles::band(ui, st, &bg, &rects, out);
    });
    Panel::right("review_info").default_size(300.0).show(ui, |ui| {
        // the buttons stay at the bottom, wherever the info ends
        Panel::bottom("review_actions").show(ui, |ui| {
            ui.add_space(4.0);
            verdict_buttons(ui, out, Msg::ReviewCurrent);
            ui.horizontal(|ui| {
                if ui.button(ph::CARET_LEFT).clicked() {
                    out.push(Msg::ReviewNext(-1));
                }
                if ui.button(ph::CARET_RIGHT).clicked() {
                    out.push(Msg::ReviewNext(1));
                }
                if ui.button(ic(ph::PUSH_PIN, "Pin & compare")).clicked() {
                    out.push(Msg::Compare);
                }
                if ui
                    .add_enabled(!st.review_undo.is_empty(), egui::Button::new(ic(ph::ARROW_COUNTER_CLOCKWISE, "Undo")))
                    .on_hover_text("undo the last rating (4 or Ctrl+Z)")
                    .clicked()
                {
                    out.push(Msg::UndoReview);
                }
            });
            ui.add_space(2.0);
        });
        ScrollArea::vertical().auto_shrink([false, false]).id_salt("review_info_scroll").show(ui, |ui| {
            info_panel(ui, st, root, tex, item, cur, queue.len(), out);
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        follow_hold(ui, st, out);
        let t = tex.full(root, &item.image);
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
        image_fit_bg(&ui.painter_at(rect), rect.shrink(4.0), t.as_ref(), st.checkerboard);
        if resp.double_clicked() {
            out.push(Msg::Preview(Some(item.image.clone())));
        }
        resp.context_menu(|ui| {
            crate::tiles::menu(ui, st, &item.image, out);
            use_menu(ui, st, root, &item.image, out);
            ui.separator();
            collect_button(ui, st, &item.image, out);
            tag_button(ui, st, &item.image, out);
            if ui.button(ic(ph::FOLDER_OPEN, "Show in file manager")).clicked() {
                out.push(Msg::ShowInFileManager(item.path.clone()));
                ui.close();
            }
        });
    });
}

/// "Use this image" actions: into the current, its own, a new or a
/// duplicated project, or to the input side (DESIGN.md, Outputs as inputs).
pub fn use_menu(ui: &mut egui::Ui, st: &AppState, root: &Path, image: &increart_core::ImageRef, out: &mut Vec<Msg>) {
    image_menu_items(ui, root, image, out);
    if matches!(st.tab, Tab::Review | Tab::OutputLibrary) && ui.button(ic(ph::COLUMNS, "Compare with…")).clicked() {
        out.push(Msg::CompareWith(Some(image.clone())));
        ui.close();
    }
    if ui.button(ic(ph::REPEAT_ONCE, "Same recipe & seed in Generate")).clicked() {
        out.push(Msg::RecipeInGenerate(image.clone()));
        ui.close();
    }
    ui.separator();
    let current = st.project_entry().map(|p| p.name());
    let label = format!("Use as a reference in {}", current.as_deref().unwrap_or("the current project"));
    if ui
        .add_enabled(current.is_some(), egui::Button::new(ic(ph::PLUS, &label)))
        .on_disabled_hover_text("Select a project first (Projects tab)")
        .clicked()
    {
        out.push(Msg::UseImage(image.clone(), UseTarget::CurrentProject));
        ui.close();
    }
    for (label, target) in [
        (ic(ph::PENCIL_SIMPLE, "Edit this image   E"), UseTarget::Edit),
        (ic(ph::ROCKET_LAUNCH, "New project from this image   N"), UseTarget::NewProject),
        (ic(ph::COPY, "Duplicate project with this image   D"), UseTarget::DuplicateProject),
    ] {
        if ui.button(label).clicked() {
            out.push(Msg::UseImage(image.clone(), target));
            ui.close();
        }
    }
    ui.separator();
    if ui.button(ic(ph::BOOKS, "To the input library as a ref   R")).clicked() {
        out.push(Msg::ToInputLibrary(image.clone()));
        ui.close();
    }
    if ui.button(ic(ph::LIGHTBULB, "New idea from this image   I")).clicked() {
        out.push(Msg::NewIdeaFromImage(image.clone()));
        ui.close();
    }
}

fn grid(ui: &mut egui::Ui, st: &AppState, root: &Path, tex: &mut Textures, out: &mut Vec<Msg>) {
    let queue = st.review_queue();
    if !st.info_hidden.contains(&InfoPanel::Review) {
        let cur = st.review_cursor.min(queue.len() - 1);
        Panel::right("review_info").default_size(300.0).show(ui, |ui| {
            Panel::bottom("review_grid_actions").show(ui, |ui| {
                ui.add_space(4.0);
                verdict_buttons(ui, out, Msg::ReviewCurrent);
                ui.add_space(2.0);
            });
            ScrollArea::vertical().auto_shrink([false, false]).id_salt("review_info_scroll").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Details").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(ph::X).on_hover_text("hide the details panel").clicked() {
                            out.push(Msg::ShowInfo(InfoPanel::Review, false));
                        }
                    });
                });
                info_panel(ui, st, root, tex, queue[cur], cur, queue.len(), out);
            });
        });
    }
    let size = st.grid_cell_size(ZoomGrid::Review) as f32;
    egui::CentralPanel::default().show(ui, |ui| {
        follow_hold(ui, st, out);
        let bg = crate::tiles::background(ui, st, out);
        let mut rects = vec![];
        grid_ctrl_scroll(ui, st, ZoomGrid::Review, out);
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(6.0);
                for (i, o) in queue.iter().enumerate() {
                    let t = tex.thumb(root, &o.image);
                    let r = thumb(ui, t.as_ref(), size, crate::tiles::cursor(st, i == st.review_cursor), None)
                        .on_hover_text(describe(st, o));
                    if i == st.review_cursor {
                        crate::widgets::reveal_cursor(ui, "grid", o.image.clone(), &r);
                    }
                    crate::tiles::mark(ui, st, r.rect, &o.image);
                    rects.push((o.image.clone(), r.rect));
                    if !crate::tiles::click(ui, st, &r, &o.image, out) && r.clicked() {
                        out.push(Msg::ReviewCursor(i));
                    }
                    if r.double_clicked() {
                        out.push(Msg::ReviewCursor(i));
                        out.push(Msg::ReviewView(ReviewView::Single));
                    }
                    r.context_menu(|ui| {
                        crate::tiles::menu(ui, st, &o.image, out);
                        for (label, v, c) in VERDICTS {
                            if ui.button(RichText::new(label).color(c)).clicked() {
                                out.push(Msg::ReviewLink(o.path.clone(), Some(v)));
                                ui.close();
                            }
                        }
                        collect_button(ui, st, &o.image, out);
                        tag_button(ui, st, &o.image, out);
                        ui.separator();
                        use_menu(ui, st, root, &o.image, out);
                    });
                }
            });
        });
        crate::tiles::band(ui, st, &bg, &rects, out);
    });
}

fn sheet_keys(ui: &egui::Ui, st: &AppState, picking: bool, out: &mut Vec<Msg>) {
    let last = st.sheet().map(|sh| (sh.rows.len().saturating_sub(1), sh.seeds.len().saturating_sub(1)));
    ui.input(|i| {
        // Home / End: the row's first / last seed; with Ctrl the sheet's
        if let Some((rows, cols)) = last {
            let r = st.sheet_cell.0;
            if i.key_pressed(Key::Home) {
                out.push(if i.modifiers.command { Msg::SheetCell(0, 0) } else { Msg::SheetCell(r, 0) });
            }
            if i.key_pressed(Key::End) {
                out.push(if i.modifiers.command { Msg::SheetCell(rows, cols) } else { Msg::SheetCell(r, cols) });
            }
        }
        for (keys, v) in [
            ([Key::Num1, Key::G], Verdict::Good),
            ([Key::Num2, Key::N], Verdict::Neutral),
            ([Key::Num3, Key::B], Verdict::Bad),
        ] {
            if !picking && keys.iter().any(|k| i.key_pressed(*k)) {
                out.push(Msg::SheetRate(v));
            }
        }
        for (k, dr, dc) in
            [(Key::ArrowRight, 0, 1), (Key::ArrowLeft, 0, -1), (Key::ArrowDown, 1, 0), (Key::ArrowUp, -1, 0)]
        {
            if i.key_pressed(k) {
                out.push(Msg::SheetMove(dr, dc));
            }
        }
        if i.key_pressed(Key::Num4) || (i.modifiers.command && i.key_pressed(Key::Z)) {
            out.push(Msg::UndoReview);
        }
        let (r, c) = st.sheet_cell;
        let cell = st.sheet().and_then(|sh| match sh.rows.get(r)?.cells.get(c)? {
            increart_gui_core::SheetCell::Image(o) => Some(o.image.clone()),
            _ => None,
        });
        if !picking && let Some(img) = cell {
            tag_keys(i, vec![img], out);
        }
    });
}

/// A style sweep as a contact sheet: a row per style, a column per seed.
fn sheet(ui: &mut egui::Ui, st: &AppState, root: &Path, tex: &mut Textures, out: &mut Vec<Msg>) {
    use increart_core::config::SheetRows;
    use increart_gui_core::SheetCell;
    let Some(sh) = st.sheet() else { return };
    let size = st.sheet_cell_size() as f32;
    egui::CentralPanel::default().show(ui, |ui| {
        let bg = crate::tiles::background(ui, st, out);
        let mut rects = vec![];
        // as many rows side by side as fit (room left for the scroll bar,
        // so it can't flip the count); Auto's headers go compact to fold
        let cells_w = sh.seeds.len().max(1) as f32 * (size + SHEET_GAP) - SHEET_GAP;
        let avail = ui.available_width() - 16.0;
        let fit = |w: f32| (((avail + SHEET_BAND_GAP) / (w + SHEET_BAND_GAP)) as usize).clamp(1, sh.rows.len().max(1));
        let compact_w = cells_w.max(SHEET_CAPTION_MIN);
        let compact = match st.config.sheet_rows {
            SheetRows::Auto => fit(compact_w) > 1,
            SheetRows::Full => false,
            SheetRows::Compact => true,
        };
        let band_w = if compact { compact_w } else { SHEET_HEAD + SHEET_GAP + cells_w };
        let k = fit(band_w);
        if k != st.sheet_bands {
            out.push(Msg::SheetBands(k));
        }
        // one seed side by side: its number up here, not over every band
        let seed_heads = !(k > 1 && sh.seeds.len() == 1);
        ui.horizontal(|ui| {
            let mut f = st.sheet_filter.clone();
            let r = ui.add(
                egui::TextEdit::singleline(&mut f)
                    .hint_text(format!("{}  filter styles by name", ph::MAGNIFYING_GLASS))
                    .desired_width(220.0),
            );
            if r.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                f.clear();
            }
            if !st.sheet_filter.is_empty() {
                if ui.small_button(ph::X).on_hover_text("clear the filter").clicked() {
                    f.clear();
                }
                let shown = sh.rows.len();
                let all = shown + sh.hidden;
                ui.weak(format!("{shown} of {all} styles"));
            }
            if f != st.sheet_filter {
                out.push(Msg::SheetFilter(f));
            }
            ui.separator();
            zoom(ui, st, out);
            let mode = st.config.sheet_rows;
            if ui
                .button(ic(ph::ROWS, mode.name()))
                .on_hover_text(
                    "Row headers: Full (the style, counts and buttons beside the row), Compact (the style above; \
                     right-click it to rate the row or use the style) or Auto (compact once rows sit side by side). \
                     Click for the next.",
                )
                .clicked()
            {
                out.push(Msg::SheetRows(mode.next()));
            }
            if !seed_heads && let Some(s) = sh.seeds.first() {
                ui.separator();
                ui.weak(format!("seed {s}"));
            }
            ui.separator();
            ui.weak(ph::INFO).on_hover_text(
                "Click selects, double-click opens; keys rate, arrows move. Row buttons rate unreviewed images only. \
                 Ctrl+click, Shift+click or drag a box to pick images: to rate them together, Tile… them, or \
                 sweep their styles again (New sweep from their styles…).",
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let kept = sh
                    .rows
                    .iter()
                    .filter(|r| {
                        r.cells.iter().any(|c| matches!(c, SheetCell::Image(o) if o.verdict == Some(Verdict::Good)))
                    })
                    .count();
                let n = sh.seeds.len().max(1) as u64;
                let styles = if kept == 1 { "style" } else { "styles" };
                let label = ic(ph::PLUS, &format!("{n} more seeds for {kept} kept {styles}"));
                if ui
                    .add_enabled(kept > 0, egui::Button::new(label))
                    .on_hover_text("styles with a Good image get more seeds")
                    .clicked()
                {
                    out.push(Msg::SheetMoreSeeds(n));
                }
            });
        });
        if sh.rows.is_empty() && sh.hidden > 0 {
            ui.add_space(12.0);
            ui.weak(format!("No style's name has “{}”.", st.sheet_filter.trim()));
            return;
        }
        // Ctrl+scroll over the sheet zooms it
        let over = ui.rect_contains_pointer(ui.available_rect_before_wrap());
        let zoom = ui.input(|i| i.zoom_delta());
        if over && zoom != 1.0 {
            out.push(Msg::SheetZoom { size: (size * zoom).round() as u32, save: true });
        }
        ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(SHEET_GAP);
            if seed_heads {
                ui.horizontal(|ui| {
                    for b in 0..k {
                        if b > 0 {
                            ui.add_space(SHEET_BAND_GAP - SHEET_GAP);
                        }
                        if !compact {
                            ui.add_space(SHEET_HEAD + SHEET_GAP);
                        }
                        ui.horizontal(|ui| {
                            ui.set_width(if compact { band_w } else { cells_w });
                            let font = egui::TextStyle::Body.resolve(ui.style());
                            for s in &sh.seeds {
                                let (rect, _) =
                                    ui.allocate_exact_size(Vec2::new(size, font.size + 4.0), Sense::hover());
                                ui.painter().text(
                                    rect.left_center(),
                                    egui::Align2::LEFT_CENTER,
                                    format!("seed {s}"),
                                    font.clone(),
                                    ui.visuals().weak_text_color(),
                                );
                            }
                        });
                    }
                });
            }
            for (l, line) in sh.rows.chunks(k).enumerate() {
                ui.horizontal_top(|ui| {
                    for (b, row) in line.iter().enumerate() {
                        let r = l * k + b;
                        if b > 0 {
                            ui.add_space(SHEET_BAND_GAP - SHEET_GAP);
                        }
                        if compact {
                            ui.vertical(|ui| {
                                ui.set_width(band_w);
                                sheet_caption(ui, root, tex, row, r, out);
                                ui.horizontal(|ui| sheet_row(ui, st, root, tex, row, r, size, &mut rects, out));
                            });
                        } else {
                            sheet_head(ui, root, tex, row, r, out);
                            sheet_row(ui, st, root, tex, row, r, size, &mut rects, out);
                        }
                    }
                });
            }
        });
        crate::tiles::band(ui, st, &bg, &rects, out);
    });
}

/// The contact sheet's spacing, its full row header's width, and the least
/// width a compact row's caption gets (cells smaller than that leave room).
const SHEET_GAP: f32 = 6.0;
const SHEET_BAND_GAP: f32 = 24.0;
const SHEET_HEAD: f32 = 170.0;
const SHEET_CAPTION_MIN: f32 = 110.0;

/// A sheet row's (good, bad, unreviewed) images.
fn sheet_counts(row: &increart_gui_core::SheetRow) -> [usize; 3] {
    let mut n = [0; 3];
    for c in &row.cells {
        if let increart_gui_core::SheetCell::Image(o) = c {
            match o.verdict {
                Some(Verdict::Good) => n[0] += 1,
                Some(Verdict::Bad) => n[1] += 1,
                None => n[2] += 1,
                _ => {}
            }
        }
    }
    n
}

/// A sheet row's style, and its CFG when that is 1 or below (the negative
/// prompt unused).
fn sheet_style(row: &increart_gui_core::SheetRow) -> (String, Option<String>) {
    let style = row.run.sweep.as_ref().map_or(row.run.label.clone(), |s| s.row());
    let param = row.run.sweep.as_ref().and_then(|s| s.param.as_ref());
    let low_cfg = param.filter(|p| p.name == "cfg" && p.value.parse().is_ok_and(|c: f32| c <= 1.0)).map(|p| p.label());
    (style, low_cfg)
}

fn sheet_use_tip(row: &increart_gui_core::SheetRow) -> &'static str {
    match row.run.sweep.as_ref().and_then(|s| s.param.as_ref()) {
        Some(p) if p.image().is_some() => "Use this image: open its run in Generate",
        Some(_) => "Use these settings: open its run in Generate",
        None => "Use this style: open its run in Generate",
    }
}

/// An image sweep's row's image: its run's start image, else its `<image1>`.
fn sheet_image(row: &increart_gui_core::SheetRow) -> Option<increart_core::ImageRef> {
    let start = row.run.sweep.as_ref()?.param.as_ref()?.image()?;
    match &row.run.mode {
        increart_core::model::Mode::Edit { init: Some(i), .. } if start => Some(i.clone()),
        _ if !start => row.run.refs.first().cloned(),
        _ => None,
    }
}

/// A row's image (an image sweep's), as a small thumbnail.
fn sheet_image_thumb(
    ui: &mut egui::Ui,
    root: &std::path::Path,
    tex: &mut Textures,
    row: &increart_gui_core::SheetRow,
    size: f32,
    out: &mut Vec<Msg>,
) {
    let Some(image) = sheet_image(row) else { return };
    let t = tex.thumb(root, &image);
    let r = crate::widgets::thumb(ui, t.as_ref(), size, false, None)
        .on_hover_text("the row's image · double-click: full size");
    if r.double_clicked() {
        out.push(Msg::Preview(Some(image)));
    }
}

/// A full row header beside the row: the style, its counts, G N B for its
/// unreviewed images and Use.
fn sheet_head(
    ui: &mut egui::Ui,
    root: &std::path::Path,
    tex: &mut Textures,
    row: &increart_gui_core::SheetRow,
    r: usize,
    out: &mut Vec<Msg>,
) {
    ui.vertical(|ui| {
        ui.set_width(SHEET_HEAD);
        sheet_image_thumb(ui, root, tex, row, 64.0, out);
        let (style, low_cfg) = sheet_style(row);
        ui.label(RichText::new(style).strong()).on_hover_text(&row.run.label);
        if let Some(cfg) = low_cfg {
            ui.colored_label(crate::widgets::NEUTRAL, "no negative prompt")
                .on_hover_text(format!("at {cfg} the negative prompt isn't used"));
        }
        let [g, b, u] = sheet_counts(row);
        ui.weak(format!("{g} good · {b} bad · {u} new"));
        ui.horizontal(|ui| {
            for (label, v, c) in VERDICTS {
                let b = egui::Button::new(RichText::new(&label[..1]).color(c));
                if ui.add_enabled(u > 0, b).on_hover_text(format!("{label} for this row's unreviewed images")).clicked()
                {
                    out.push(Msg::SheetRow(r, v));
                }
            }
            if ui.small_button(ph::ARROW_SQUARE_OUT).on_hover_text(sheet_use_tip(row)).clicked() {
                out.push(Msg::RunInGenerate(row.run.id.clone()));
            }
        });
    });
}

/// A compact row header: the style as a caption above the row; its menu
/// rates the row's unreviewed images or uses the style.
fn sheet_caption(
    ui: &mut egui::Ui,
    root: &std::path::Path,
    tex: &mut Textures,
    row: &increart_gui_core::SheetRow,
    r: usize,
    out: &mut Vec<Msg>,
) {
    let (style, low_cfg) = sheet_style(row);
    ui.horizontal(|ui| {
        sheet_image_thumb(ui, root, tex, row, 22.0, out);
        if let Some(cfg) = &low_cfg {
            ui.colored_label(crate::widgets::NEUTRAL, ph::WARNING)
                .on_hover_text(format!("no negative prompt: at {cfg} it isn't used"));
        }
        let resp = ui
            .add(egui::Label::new(RichText::new(style).strong()).truncate().sense(Sense::click()))
            .on_hover_text(format!("{}\nRight-click: rate the row, or use it", row.run.label));
        resp.context_menu(|ui| {
            let u = sheet_counts(row)[2];
            for (label, v, c) in VERDICTS {
                let b = egui::Button::new(RichText::new(format!("{label} for the unreviewed")).color(c));
                if ui.add_enabled(u > 0, b).clicked() {
                    out.push(Msg::SheetRow(r, v));
                    ui.close();
                }
            }
            ui.separator();
            let label = sheet_use_tip(row).split(':').next().unwrap_or_default();
            if ui.button(ic(ph::ARROW_SQUARE_OUT, label)).clicked() {
                out.push(Msg::RunInGenerate(row.run.id.clone()));
                ui.close();
            }
        });
    });
}

/// A sheet row's cells, a seed each.
#[allow(clippy::too_many_arguments)]
fn sheet_row(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    row: &increart_gui_core::SheetRow,
    r: usize,
    size: f32,
    rects: &mut Vec<(increart_core::ImageRef, egui::Rect)>,
    out: &mut Vec<Msg>,
) {
    use increart_gui_core::SheetCell;
    for (c, cell) in row.cells.iter().enumerate() {
        let sel = crate::tiles::cursor(st, st.sheet_cell == (r, c));
        match cell {
            SheetCell::Image(o) => {
                let t = tex.thumb(root, &o.image);
                let resp = thumb(ui, t.as_ref(), size, sel, o.verdict.map(|v| verdict_color(Some(v))))
                    .on_hover_text(describe(st, o));
                if st.sheet_cell == (r, c) {
                    crate::widgets::reveal_cursor(ui, "sheet", (r, c), &resp);
                }
                crate::tiles::mark(ui, st, resp.rect, &o.image);
                rects.push((o.image.clone(), resp.rect));
                if !crate::tiles::click(ui, st, &resp, &o.image, out) && resp.clicked() {
                    out.push(Msg::SheetCell(r, c));
                }
                if resp.double_clicked() {
                    out.push(Msg::Preview(Some(o.image.clone())));
                }
                resp.context_menu(|ui| {
                    crate::tiles::menu(ui, st, &o.image, out);
                    for (label, v, col) in VERDICTS {
                        if ui.button(RichText::new(label).color(col)).clicked() {
                            out.push(Msg::ReviewLink(o.path.clone(), Some(v)));
                            ui.close();
                        }
                    }
                    collect_button(ui, st, &o.image, out);
                    tag_button(ui, st, &o.image, out);
                    ui.separator();
                    use_menu(ui, st, root, &o.image, out);
                });
            }
            SheetCell::Pending | SheetCell::Empty => {
                let (text, tip) = match cell {
                    SheetCell::Pending => ("…", "still to be generated"),
                    _ => ("–", "not generated (cancelled or failed)"),
                };
                let resp = empty_cell(ui, size, sel, text).on_hover_text(tip);
                if st.sheet_cell == (r, c) {
                    crate::widgets::reveal_cursor(ui, "sheet", (r, c), &resp);
                }
                if resp.clicked() {
                    out.push(Msg::SheetCell(r, c));
                }
            }
        }
    }
}

/// The contact sheet's cell size: a slider; also Ctrl+scroll over the sheet.
fn zoom(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    use increart_gui_core::{SHEET_CELL_DEFAULT, SHEET_CELL_MAX, SHEET_CELL_MIN};
    let range = (SHEET_CELL_MIN, SHEET_CELL_MAX, SHEET_CELL_DEFAULT);
    if let Some((size, save)) = zoom_control(ui, st.sheet_cell_size(), range, "cell size", "the sheet") {
        out.push(Msg::SheetZoom { size, save });
    }
}

/// An image grid's tile size: a slider; also Ctrl+scroll over the grid.
pub fn grid_zoom(ui: &mut egui::Ui, st: &AppState, grid: ZoomGrid, out: &mut Vec<Msg>) {
    let range = (GRID_CELL_MIN, GRID_CELL_MAX, grid.default_size());
    if let Some((size, save)) = zoom_control(ui, st.grid_cell_size(grid), range, "tile size", "the grid") {
        out.push(Msg::GridZoom { grid, size, save });
    }
}

/// Ctrl+scroll over the rest of `ui`: an image grid's tile size.
pub fn grid_ctrl_scroll(ui: &egui::Ui, st: &AppState, grid: ZoomGrid, out: &mut Vec<Msg>) {
    let over = ui.rect_contains_pointer(ui.available_rect_before_wrap());
    let zoom = ui.input(|i| i.zoom_delta());
    if over && zoom != 1.0 {
        let size = (st.grid_cell_size(grid) as f32 * zoom).round() as u32;
        out.push(Msg::GridZoom { grid, size, save: true });
    }
}

/// − slider +: a size within (min, max, default; double-click the slider
/// for the default), and whether to save it (not while dragging).
fn zoom_control(ui: &mut egui::Ui, now: u32, range: (u32, u32, u32), what: &str, over: &str) -> Option<(u32, bool)> {
    let (min, max, default) = range;
    let step = |ui: &mut egui::Ui, icon: &str, tip: &str, to: u32| {
        ui.add_enabled(to != now, egui::Button::new(icon).small()).on_hover_text(tip).clicked().then_some(to)
    };
    let mut to = step(ui, ph::MAGNIFYING_GLASS_MINUS, "smaller", now.saturating_sub(32).max(min));
    let mut size = now;
    let r = ui
        .add(egui::Slider::new(&mut size, min..=max).show_value(false))
        .on_hover_text(format!("{what} {now} (Ctrl+scroll over {over}; double-click: {default})"));
    if r.double_clicked() {
        size = default;
    }
    let mut changed = (size != now || r.drag_stopped()).then_some((size, !r.dragged()));
    to = to.or(step(ui, ph::MAGNIFYING_GLASS_PLUS, "larger", (now + 32).min(max)));
    if let Some(size) = to {
        changed = Some((size, true));
    }
    changed
}

/// A contact sheet cell without an image: `…` pending, `–` none.
fn empty_cell(ui: &mut egui::Ui, size: f32, selected: bool, text: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, egui::Color32::from_gray(35));
    p.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(24.0), egui::Color32::GRAY);
    let stroke = if selected {
        egui::Stroke::new(3.0, crate::widgets::ACCENT)
    } else if resp.hovered() {
        egui::Stroke::new(1.0, egui::Color32::GRAY)
    } else {
        egui::Stroke::NONE
    };
    p.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
    resp
}

fn compare(ui: &mut egui::Ui, st: &AppState, root: &Path, tex: &mut Textures, out: &mut Vec<Msg>) {
    let queue = st.review_queue();
    let a = st.review_pinned.as_ref();
    let b = queue.get(st.review_cursor.min(queue.len().saturating_sub(1))).copied();
    Panel::bottom("compare_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            let tip = "the pinned image and the one that steps change sides";
            if ui.button(ic(ph::ARROWS_LEFT_RIGHT, "Swap sides")).on_hover_text(tip).clicked() {
                out.push(Msg::SwapCompare);
            }
            if ui.button(ph::CARET_LEFT).on_hover_text("previous B").clicked() {
                out.push(Msg::ReviewNext(-1));
            }
            if ui.button(ph::CARET_RIGHT).on_hover_text("next B").clicked() {
                out.push(Msg::ReviewNext(1));
            }
            if ui.button(ic(ph::IMAGES, "Compare with…")).on_hover_text("another image as A   Shift+C").clicked() {
                out.push(Msg::CompareWith(None));
            }
            if ui.button(ic(ph::X, "Leave compare")).clicked() {
                out.push(Msg::Compare);
            }
            let a_rated = a.and_then(|a| st.output_link(a)).is_some();
            ui.weak(if a_rated { "Keys rate B; A has its own buttons." } else { "Keys rate B." });
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        follow_hold(ui, st, out);
        compare_sides(
            ui,
            st,
            root,
            tex,
            a,
            b,
            &|is_a, _, v| if is_a { Msg::ReviewPinned(v) } else { Msg::ReviewCurrent(v) },
            out,
        );
    });
}

/// A and B side by side, each over its own rating buttons when it's an
/// output; A may be an input image (Compare with…), shown with its idea's
/// title and no rating. `rate`: the message rating A (true) or B, given
/// its link.
#[allow(clippy::too_many_arguments)]
fn compare_sides(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    a: Option<&increart_core::ImageRef>,
    b: Option<&OutputLink>,
    rate: &dyn Fn(bool, &OutputLink, Verdict) -> Msg,
    out: &mut Vec<Msg>,
) {
    let b = b.map(|o| &o.image);
    // what A was to B: its start image or a reference
    let role = a.zip(b).and_then(|(a, b)| st.input_role(b, a)).map(|r| match r {
        None => "B's start image".to_string(),
        Some(n) => format!("B's reference {n} · <image{n}>"),
    });
    // B, the one the arrows change, on the left unless swapped
    let sides = if st.compare_flipped { [(a, true), (b, false)] } else { [(b, false), (a, true)] };
    ui.columns(2, |cols| {
        for (ui, (image, is_a)) in cols.iter_mut().zip(sides) {
            let link = image.and_then(|i| st.output_link(i));
            let title = if is_a { "A (pinned)" } else { "B (← →)" };
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong());
                if is_a && let Some(r) = &role {
                    egui::Frame::new()
                        .fill(crate::widgets::ACCENT)
                        .corner_radius(3.0)
                        .inner_margin(egui::Margin::symmetric(5, 1))
                        .show(ui, |ui| ui.label(RichText::new(r).color(egui::Color32::WHITE)));
                }
                match (link, image) {
                    (Some(o), _) => {
                        ui.weak(describe(st, o));
                        if is_a && o.verdict.is_some() {
                            ui.colored_label(verdict_color(o.verdict), "rated");
                        }
                    }
                    (None, Some(i)) => {
                        let t = st.input_title(i).filter(|t| !t.is_empty()).unwrap_or("(untitled)");
                        ui.weak(format!("“{t}” · input image"));
                    }
                    (None, None) => {}
                }
            });
            let Some(image) = image else {
                ui.weak(if is_a { "Nothing pinned." } else { "No more unreviewed images." });
                continue;
            };
            // bottom up: the buttons at the bottom of the side, the image above
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                if let Some(o) = link {
                    verdict_buttons(ui, out, |v| rate(is_a, o, v));
                }
                let t = tex.full(root, image);
                let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
                image_fit_bg(&ui.painter_at(rect), rect.shrink(4.0), t.as_ref(), st.checkerboard);
                if resp.double_clicked() {
                    out.push(Msg::Preview(Some(image.clone())));
                }
                resp.context_menu(|ui| {
                    if is_a {
                        if ui.button(ic(ph::IMAGES, "Replace A with…")).clicked() {
                            out.push(Msg::CompareWith(None));
                            ui.close();
                        }
                        ui.separator();
                    }
                    match link {
                        Some(_) => use_menu(ui, st, root, image, out),
                        None => image_menu_items(ui, root, image, out),
                    }
                });
            });
        }
    });
}

// ---------------------------------------------------------------- output library

/// The Output library Grid's **Keys**, pinned under its sources.
fn output_keys_hint(ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(RichText::new(ic(ph::KEYBOARD, "Keys")).strong())
        .id_salt("out_keys")
        .default_open(true)
        .show(ui, |ui| {
            for (k, what) in [
                ("← →", "previous / next"),
                ("Home / End", "first / last"),
                ("F2", "title the selected image"),
                ("Ctrl+click", "pick"),
                ("Shift+click", "pick a range"),
                ("Ctrl+A", "pick all"),
                ("1 2 3 / G N B", "rate the picked"),
                ("T", "tile the picked"),
                ("L", "tag the selected / picked…"),
                ("Shift+L", "tag them again"),
                ("Esc", "clear the picks"),
                ("Double-click", "full size"),
            ] {
                ui.horizontal(|ui| {
                    ui.monospace(k);
                    ui.weak(what);
                });
            }
        });
}

pub fn output_library(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let Some(root) = st.library.clone() else { return };
    // Discarded lists every project's, whatever the source and filters
    let sources = st.output_tab != OutputTab::Discarded;
    if sources {
        Panel::left("out_sources").default_size(250.0).show(ui, |ui| {
            if st.output_tab == OutputTab::Grid {
                Panel::bottom("out_keys").show(ui, output_keys_hint);
            }
            ScrollArea::vertical()
                .auto_shrink([false, true])
                .id_salt("out_sources_scroll")
                .show(ui, |ui| sources_panel(ui, st, out));
            if st.output_reveal {
                reveal_done(ui, out);
            }
        });
    }
    let images = st.output_source_images();
    // F2: the selected image's title
    if st.output_tab == OutputTab::Grid
        && st.preview.is_none()
        && st.title_edit.is_none()
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(Key::F2))
        && let Some(img) = &st.output_selected
    {
        out.push(Msg::TitleDialog(Some(img.clone())));
    }
    // L / Shift+L: the selected image (the picks: tiles::keys)
    if st.output_tab == OutputTab::Grid
        && st.preview.is_none()
        && st.title_edit.is_none()
        && st.tagging.is_none()
        && st.picked.is_empty()
        && !ui.ctx().egui_wants_keyboard_input()
        && let Some(img) = &st.output_selected
    {
        ui.input(|i| tag_keys(i, vec![img.clone()], out));
    }
    // the arrows and Home / End: the image selected, shown in the lineage panel
    if st.output_tab == OutputTab::Grid
        && st.preview.is_none()
        && st.title_edit.is_none()
        && st.ref_picker.is_none()
        && st.tagging.is_none()
        && !ui.ctx().egui_wants_keyboard_input()
        && let Some(last) = images.len().checked_sub(1)
    {
        let at = st.output_selected.as_ref().and_then(|s| images.iter().position(|o| &o.image == s));
        let to = ui.input(|i| {
            if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown) {
                Some(at.map_or(0, |a| (a + 1).min(last)))
            } else if i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowUp) {
                Some(at.map_or(0, |a| a.saturating_sub(1)))
            } else if i.key_pressed(Key::Home) {
                Some(0)
            } else if i.key_pressed(Key::End) {
                Some(last)
            } else {
                None
            }
        });
        if let Some(to) = to {
            out.push(Msg::OutputSelect(Some(images[to].image.clone())));
        }
    }
    Panel::top("out_tabs").show(ui, |ui| {
        // groups wrap whole when the bar is short of room
        use crate::widgets::{group, right_group};
        ui.horizontal_wrapped(|ui| {
            group(ui, "tabs", |ui| {
                if ui.selectable_label(st.output_tab == OutputTab::Grid, ic(ph::SQUARES_FOUR, "Grid")).clicked() {
                    out.push(Msg::OutputTab(OutputTab::Grid));
                }
                if ui.selectable_label(st.output_tab == OutputTab::Folder, ic(ph::FOLDER, "Folder")).clicked() {
                    out.push(Msg::OutputTab(OutputTab::Folder));
                }
                let n = st.index.projects.iter().map(|p| p.discarded.len()).sum::<usize>();
                let label = if n == 0 { "Discarded".to_string() } else { format!("Discarded ({n})") };
                if ui.selectable_label(st.output_tab == OutputTab::Discarded, ic(ph::TRASH, &label)).clicked() {
                    out.push(Msg::OutputTab(OutputTab::Discarded));
                }
            });
            if st.output_tab == OutputTab::Discarded {
                right_group(ui, "right", |ui| crate::widgets::details_toggle(ui, st, InfoPanel::Output, out));
                return;
            }
            group(ui, "export", |ui| {
                ui.separator();
                let shown = &images;
                if ui
                    .add_enabled(
                        !shown.is_empty(),
                        egui::Button::new(ic(ph::EXPORT, &format!("Export these {}…", shown.len()))),
                    )
                    .clicked()
                {
                    out.push(Msg::OpenExport(shown.iter().map(|o| o.image.clone()).collect(), source_name(st)));
                }
            });
            group(ui, "source", |ui| {
                if st.output_tab == OutputTab::Grid {
                    ui.weak(format!("· {}", source_name(st)));
                }
                if let OutputSource::Collection(f) = &st.output_source
                    && ui.button(ic(ph::FOLDER_OPEN, "Open in file manager")).clicked()
                {
                    out.push(Msg::ShowInFileManager(Path::new("collections").join(f)));
                }
            });
            if st.output_tab == OutputTab::Grid {
                group(ui, "zoom", |ui| {
                    ui.separator();
                    grid_zoom(ui, st, ZoomGrid::Output, out);
                });
            }
            right_group(ui, "right", |ui| crate::widgets::details_toggle(ui, st, InfoPanel::Output, out));
        });
    });
    if st.output_tab == OutputTab::Grid {
        Panel::top("out_presets").show(ui, |ui| preset_tabs(ui, st, out));
        crate::tiles::bar(ui, st, out);
        crate::tiles::keys(ui, st, out);
    }
    if !st.info_hidden.contains(&InfoPanel::Output) {
        Panel::right("out_lineage").default_size(300.0).show(ui, |ui| {
            ScrollArea::vertical().auto_shrink([false, true]).id_salt("lineage_scroll").show(ui, |ui| {
                match st.output_selected.clone() {
                    Some(sel) => lineage_panel(ui, st, &root, tex, &sel, InfoPanel::Output, out),
                    None => crate::widgets::nothing_selected(
                        ui,
                        "Lineage",
                        "No image selected: click one to see where it came from, its recipe and what was made from it.",
                    ),
                }
            });
        });
    }
    egui::CentralPanel::default().show(ui, |ui| match st.output_tab {
        OutputTab::Folder => crate::folderview::view(ui, st, tex, out),
        OutputTab::Discarded => discarded_outputs(ui, st, &root, tex, out),
        OutputTab::Grid if st.output_compare.is_some() => output_compare(ui, st, &root, tex, &images, out),
        OutputTab::Grid => {
            let noticed = st.output_notice && output_notice(ui, st, images.len(), out);
            let r = st.output_filter.ratings;
            let rejected = r.bad && !r.good && !r.neutral && !r.unreviewed;
            if rejected && !images.is_empty() {
                ui.horizontal(|ui| {
                    ui.weak("Images rated bad. Right-click to re-rate or delete.");
                    let n = images.len();
                    if ui.button(ic(ph::TRASH, &format!("Delete these {n} rejected…"))).clicked() {
                        let text = format!(
                            "Delete these {n} rejected images ({})? They go to Discarded (restore them from Output \
                             library → Discarded); their seeds aren't generated again.",
                            source_name(st)
                        );
                        out.push(Msg::Ask(Confirm::new(text, "Delete", Msg::DeleteShownRejected)));
                    }
                });
            }
            if images.is_empty() {
                if !noticed {
                    ui.weak(match st.output_preset {
                        OutputPreset::Rejected if !st.output_preset_modified() => "No rejected images here.",
                        OutputPreset::Unreviewed if !st.output_preset_modified() => "Nothing waiting for review here.",
                        _ => "Nothing here: rate images in Review, pick another tab, or loosen the filters.",
                    });
                }
                return;
            }
            image_grid(ui, st, &root, tex, out, &images, rejected);
        }
    });
}

/// The Grid's compare: Compare with…'s image (A) beside the selected one
/// (B), which the arrows move through the images shown.
fn output_compare(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    images: &[&OutputLink],
    out: &mut Vec<Msg>,
) {
    let at = st.output_selected.as_ref().and_then(|s| images.iter().position(|o| &o.image == s));
    let b = st.output_selected.as_ref().and_then(|s| st.output_link(s));
    let esc = st.preview.is_none() && st.ref_picker.is_none() && ui.input(|i| i.key_pressed(Key::Escape));
    if esc {
        out.push(Msg::OutputCompare(None));
    }
    ui.horizontal(|ui| {
        let step = |d: isize| at.and_then(|i| i.checked_add_signed(d)).and_then(|i| images.get(i));
        if ui
            .add_enabled(step(-1).is_some(), egui::Button::new(ph::CARET_LEFT))
            .on_hover_text("previous B   ←")
            .clicked()
            && let Some(o) = step(-1)
        {
            out.push(Msg::OutputSelect(Some(o.image.clone())));
        }
        if ui.add_enabled(step(1).is_some(), egui::Button::new(ph::CARET_RIGHT)).on_hover_text("next B   →").clicked()
            && let Some(o) = step(1)
        {
            out.push(Msg::OutputSelect(Some(o.image.clone())));
        }
        if ui
            .button(ic(ph::ARROWS_LEFT_RIGHT, "Swap sides"))
            .on_hover_text("the pinned image and the one that steps change sides")
            .clicked()
        {
            out.push(Msg::SwapCompare);
        }
        if ui.button(ic(ph::IMAGES, "Compare with…")).on_hover_text("another image as A").clicked() {
            out.push(Msg::CompareWith(None));
        }
        if ui.button(ic(ph::X, "Leave compare   Esc")).clicked() {
            out.push(Msg::OutputCompare(None));
        }
        if let Some(i) = at {
            ui.weak(format!("B: {} / {}", i + 1, images.len()));
        }
    });
    let rate = |_, o: &OutputLink, v| Msg::ReviewLink(o.path.clone(), Some(v));
    compare_sides(ui, st, root, tex, st.output_compare.as_ref(), b, &rate, out);
}

/// Kept · Good · Rejected · Unreviewed · All, with the current source's
/// counts. A click replaces the filters; changed filters mark the tab "•".
fn preset_tabs(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        for p in OutputPreset::EVERY {
            let n = st.source_images_in(&st.output_source, p.ratings()).len();
            let on = st.output_preset == p;
            let dot = if on && st.output_preset_modified() { " •" } else { "" };
            let tip = if dot.is_empty() { "Show these" } else { "Filters changed: click to reset them" };
            if ui.selectable_label(on, format!("{} ({n}){dot}", p.label())).on_hover_text(tip).clicked() {
                out.push(Msg::OutputPreset(p));
            }
        }
        if st.output_preset == OutputPreset::Unreviewed && st.source_unreviewed(&st.output_source) > 0 {
            ui.separator();
            review_them(ui, st, out);
        }
    });
}

/// Jumps to Review for the source's unreviewed images.
fn review_them(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    if ui.button(ic(ph::EYE, "Review them")).clicked() {
        match &st.output_source {
            OutputSource::Project(id) => out.push(Msg::ReviewScope(Some(id.clone()))),
            OutputSource::ProjectFolder(f) => out.push(Msg::ReviewScopeFolder(f.clone())),
            _ => out.push(Msg::ReviewScope(None)),
        }
        out.push(Msg::Tab(Tab::Review));
    }
}

/// "Show outputs (12 kept)": a project's or project folder's kept images in
/// the Output library Grid.
pub fn show_outputs_button(ui: &mut egui::Ui, st: &AppState, source: OutputSource, out: &mut Vec<Msg>) -> bool {
    let n = st.source_images(&source).len();
    let r = ui.button(ic(ph::IMAGES, &format!("Show outputs ({n} kept)")));
    let clicked = r.on_hover_text("its good and neutral images, in Output library → Grid").clicked();
    if clicked {
        out.push(Msg::ShowOutputs(source));
    }
    clicked
}

/// After Show outputs: the source has no kept images (offering the All
/// tab and Review), or filters besides the ratings hide some. Returns
/// whether it showed.
fn output_notice(ui: &mut egui::Ui, st: &AppState, shown: usize, out: &mut Vec<Msg>) -> bool {
    let f = &st.output_filter;
    let all = st.source_images_in(&st.output_source, f.ratings).len();
    if all > 0 && !f.narrows() {
        return false;
    }
    egui::Frame::group(ui.style()).stroke(egui::Stroke::new(1.0, NEUTRAL)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            if all == 0 {
                ui.colored_label(NEUTRAL, ic(ph::WARNING, &format!("No kept images in {} yet.", source_name(st))));
                let any = st.source_images_in(&st.output_source, Ratings::ALL).len();
                if f.ratings != Ratings::ALL
                    && any > 0
                    && ui.button(ic(ph::IMAGES, &format!("Show all {any}, bad included"))).clicked()
                {
                    out.push(Msg::OutputPreset(OutputPreset::All));
                }
                let waiting = st.source_unreviewed(&st.output_source);
                if waiting > 0 {
                    ui.label(format!("{waiting} waiting in Review."));
                    review_them(ui, st, out);
                }
            } else {
                let hidden = all - shown;
                ui.colored_label(
                    NEUTRAL,
                    ic(ph::FUNNEL, &format!("Filters are on: showing {shown} of {all} ({hidden} hidden).")),
                );
                if ui.button(ic(ph::FUNNEL_X, "Reset filters")).clicked() {
                    out.push(Msg::ResetOutputFilters);
                }
            }
            if ui.small_button(ph::X).on_hover_text("dismiss").clicked() {
                out.push(Msg::DismissOutputNotice);
            }
        });
    });
    ui.add_space(6.0);
    true
}

/// "collection animals", "smart: 2026.10.24", "all kept images".
fn source_name(st: &AppState) -> String {
    match &st.output_source {
        OutputSource::AllProjects => "all projects".into(),
        OutputSource::Project(id) => st.index.project(id).map(|p| p.name()).unwrap_or_default(),
        OutputSource::ProjectFolder(f) if f.as_os_str().is_empty() => "all projects".into(),
        OutputSource::ProjectFolder(f) => format!("project folder {}", f.display()),
        OutputSource::Collection(f) if f.as_os_str().is_empty() => "all collections".into(),
        OutputSource::Collection(f) => format!("collection {}", f.display()),
        OutputSource::Smart(s) => format!("smart folder {s:?}"),
        OutputSource::Run(id) => match st.index.run(id) {
            Some((p, r)) => format!("run {} of {}", r.label, p.name()),
            None => format!("run {id}"),
        },
    }
}

fn pick(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>, src: OutputSource, label: String) {
    let selected = st.output_source == src;
    let r = ui.selectable_label(selected, label);
    // after Show outputs: scroll it into view
    if selected {
        if st.output_reveal {
            r.scroll_to_me(Some(egui::Align::Center));
        }
        // for the tour's reveal check: where it is, what's visible, which frame
        let seen = (r.rect, ui.clip_rect(), ui.ctx().cumulative_pass_nr());
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("tour_selected_source"), seen));
    }
    if r.clicked() {
        out.push(Msg::OutputSource(src));
    }
}

/// The "by project" smart folder: the project tree (what has kept images),
/// each folder a source of its own. A folder holding the selected source
/// starts open; Show outputs starts the tree over, only the way to its
/// source open. `q` (lower case) is the project-name search: only matching
/// projects and folders (a matching folder with all it holds), every folder
/// open.
fn by_project(ui: &mut egui::Ui, st: &AppState, folder: &Path, q: &str, out: &mut Vec<Msg>) {
    let (folders, projects) = st.kept_project_children(folder);
    let selected_dir = match &st.output_source {
        OutputSource::ProjectFolder(f) => Some(Path::new("projects").join(f)),
        OutputSource::Project(id) => st.index.projects.iter().find(|p| &p.project.id == id).map(|p| p.dir.clone()),
        OutputSource::Run(id) => st.index.run(id).map(|(p, _)| p.dir.clone()),
        _ => None,
    };
    let searching = !q.is_empty();
    for f in folders {
        let name = f.file_name().unwrap_or_default().to_string_lossy();
        let own = searching && name.to_lowercase().contains(q);
        if searching && !own && !kept_match(st, f, q) {
            continue;
        }
        let dir = Path::new("projects").join(f);
        let inside = searching || selected_dir.as_ref().is_some_and(|d| d.starts_with(&dir) && *d != dir);
        let src = OutputSource::ProjectFolder(f.clone());
        let n = st.source_images(&src).len();
        // while searching, open state of its own (so the search leaves the tree as it was)
        let id = ui.make_persistent_id(("by project", f, searching, st.output_reveals));
        // a folder whose name matches shows all it holds
        let q = if own { "" } else { q };
        let (toggle, ..) = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, inside)
            .show_header(ui, |ui| pick(ui, st, out, src, ic(ph::FOLDER, &format!("{name} ({n})"))))
            .body(|ui| by_project(ui, st, f, q, out));
        // for the tour's reveal check: the open/close arrow
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(("tour_folder_toggle", &**f)), toggle.rect));
    }
    for p in projects {
        if searching && !p.name().to_lowercase().contains(q) {
            continue;
        }
        let src = OutputSource::Project(p.project.id.clone());
        let n = st.source_images(&src).len();
        let label = ic(ph::ROCKET_LAUNCH, &format!("{} ({n})", p.name()));
        // its runs with kept images, newest first; open while one is shown
        let runs = st.kept_runs(p);
        let run_shown = matches!(&st.output_source, OutputSource::Run(id) if runs.iter().any(|(r, _)| &r.id == id));
        let id = ui.make_persistent_id(("by project runs", &p.project.id, st.output_reveals));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, run_shown)
            .show_header(ui, |ui| pick(ui, st, out, src, label))
            .body(|ui| {
                for (r, n) in runs {
                    let name = if r.label.is_empty() { r.id.to_string() } else { r.label.clone() };
                    let icon = if r.sweep.is_some() { ph::TABLE } else { ph::IMAGES };
                    pick(ui, st, out, OutputSource::Run(r.id.clone()), ic(icon, &format!("{name} ({n})")));
                }
            });
        // for the tour's reveal check: drawn this frame
        let pass = ui.ctx().cumulative_pass_nr();
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(("tour_project_row", p.name())), pass));
    }
}

/// Whether anything in the kept project tree under `folder` has `q`
/// (lower case) in its name.
fn kept_match(st: &AppState, folder: &Path, q: &str) -> bool {
    let (folders, projects) = st.kept_project_children(folder);
    projects.iter().any(|p| p.name().to_lowercase().contains(q))
        || folders.iter().any(|f| {
            f.file_name().unwrap_or_default().to_string_lossy().to_lowercase().contains(q) || kept_match(st, f, q)
        })
}

/// The search box (images' titles and prompts, or project names) and the
/// filters, at the top of the sources panel.
fn search_panel(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let mut f = st.output_filter.clone();
    let projects = st.output_search_projects;
    // right to left: the clear button, then the field takes what's left
    let row = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let mut q = if projects { st.output_project_search.clone() } else { f.text.clone() };
        let mut clear = !q.is_empty() && ui.small_button(ph::X).on_hover_text("clear").clicked();
        let hint = if projects { "Search project names" } else { "Search titles, tags and prompts" };
        let r = ui.add(
            egui::TextEdit::singleline(&mut q)
                .hint_text(ic(ph::MAGNIFYING_GLASS, hint))
                .desired_width(ui.available_width()),
        );
        clear |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape));
        if clear {
            q.clear();
        }
        if projects && q != st.output_project_search {
            out.push(Msg::OutputProjectSearch(q));
        } else if !projects {
            f.text = q;
        }
    });
    ui.horizontal(|ui| {
        ui.weak("in");
        let images =
            ui.selectable_label(!projects, "Images").on_hover_text("the images' titles and tags, their runs' prompts");
        if images.clicked() {
            out.push(Msg::OutputSearchProjects(false));
        }
        let names = ui.selectable_label(projects, "Project names").on_hover_text("narrow the by-project tree below");
        if names.clicked() {
            out.push(Msg::OutputSearchProjects(true));
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut f.has_refs, "has references");
        ui.checkbox(&mut f.transparent, "transparent");
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut f.ratings.good, "good");
        ui.checkbox(&mut f.ratings.neutral, "neutral");
        ui.checkbox(&mut f.ratings.bad, "bad");
        ui.checkbox(&mut f.ratings.unreviewed, "unreviewed");
    });
    tag_filter(ui, st, &mut f);
    if f != st.output_filter {
        out.push(Msg::OutputFilter(f));
    }
}

/// The source's tags (with the ratings picked) and their counts: a click
/// shows only the images with it (several: all of them), again lets go.
fn tag_filter(ui: &mut egui::Ui, st: &AppState, f: &mut increart_gui_core::OutputFilter) {
    let images = st.source_images_in(&st.output_source, f.ratings);
    let mut tags = st.tag_counts(images.iter().map(|o| &o.image));
    // one picked that none of these have stays, to let go of it
    for t in &f.tags {
        if !tags.iter().any(|(x, _)| x.to_lowercase() == t.to_lowercase()) {
            tags.push((t.clone(), 0));
        }
    }
    ui.horizontal(|ui| {
        ui.label(ic(ph::TAG, "Tags"));
        if !f.tags.is_empty() && ui.small_button("clear").on_hover_text("show every tag's images").clicked() {
            f.tags.clear();
        }
    });
    if tags.is_empty() {
        ui.weak("No tags here yet: tag images with L.");
        return;
    }
    ScrollArea::vertical().id_salt("tag_filter").max_height(110.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (t, n) in tags {
                let at = f.tags.iter().position(|x| x.to_lowercase() == t.to_lowercase());
                let r = ui.selectable_label(at.is_some(), format!("{t} ({n})"));
                if r.on_hover_text("only images with this tag (several: all of them)").clicked() {
                    match at {
                        Some(i) => {
                            f.tags.remove(i);
                        }
                        None => f.tags.push(t),
                    }
                }
            }
        });
    });
}

/// Search and filters, then the sources: collections (All projects first,
/// as a virtual one), then the smart folders.
/// Show outputs' reveal lasts while the folders it opens animate open (so
/// the scroll lands on the row where it ends up), then ends.
fn reveal_done(ui: &egui::Ui, out: &mut Vec<Msg>) {
    let (ctx, id) = (ui.ctx(), egui::Id::unique("output reveal start"));
    let now = ctx.input(|i| i.time);
    let start = ctx.data_mut(|d| *d.get_temp_mut_or(id, now));
    if now - start > f64::from(ui.style().animation_time) * 2.0 + 0.1 {
        ctx.data_mut(|d| d.remove::<f64>(id));
        out.push(Msg::OutputRevealed);
    } else {
        ctx.request_repaint();
    }
}

fn sources_panel(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let kept = st.kept_outputs();
    search_panel(ui, st, out);
    ui.separator();

    ui.horizontal(|ui| {
        ui.label(RichText::new("Collections").strong());
        if ui.small_button(ph::FOLDER_PLUS).on_hover_text("New collection").clicked() {
            out.push(Msg::NewCollection(Default::default(), vec![]));
        }
    });
    let n = st.source_images_in(&OutputSource::AllProjects, st.output_filter.ratings).len();
    let all = ui.selectable_label(
        st.output_source == OutputSource::AllProjects,
        ic(ph::IMAGES, &format!("All projects ({n})")),
    );
    if all.on_hover_text("every project's images (a collection of everything)").clicked() {
        out.push(Msg::OutputSource(OutputSource::AllProjects));
    }
    let n_all = st.index.collection_links.len();
    pick(
        ui,
        st,
        out,
        OutputSource::Collection(Default::default()),
        ic(ph::FOLDERS, &format!("All collections ({n_all})")),
    );
    let mut folders = st.index.collection_folders.clone();
    folders.sort();
    for f in folders {
        let n = st.index.collection_links.iter().filter(|l| l.folder.starts_with(&f)).count();
        ui.horizontal(|ui| {
            ui.add_space(12.0 * f.components().count() as f32);
            let name = f.file_name().unwrap_or_default().to_string_lossy();
            let src = OutputSource::Collection(f.clone());
            let r = ui.selectable_label(st.output_source == src, ic(ph::FOLDER, &format!("{name} ({n})")));
            if r.clicked() {
                out.push(Msg::OutputSource(src));
            }
            crate::widgets::collection_drop_target(ui, &r, &f, out);
            r.context_menu(|ui| {
                if ui.button(ic(ph::FOLDER_PLUS, "New collection inside")).clicked() {
                    out.push(Msg::NewCollection(f.clone(), vec![]));
                    ui.close();
                }
                if ui.button(ic(ph::PENCIL_SIMPLE, "Rename")).clicked() {
                    out.push(Msg::StartRename(Tree::Collections, f.clone()));
                    ui.close();
                }
                if ui.button(ic(ph::FOLDER_OPEN, "Open in file manager")).clicked() {
                    out.push(Msg::ShowInFileManager(Path::new("collections").join(&f)));
                    ui.close();
                }
                if ui.button(ic(ph::TRASH, "Delete…")).clicked() {
                    let text = format!("Delete collection {name}? {}.", st.delete_summary(Tree::Collections, &f));
                    out.push(Msg::Ask(Confirm::new(text, "Delete", Msg::DeleteNode(Tree::Collections, f.clone()))));
                    ui.close();
                }
            });
        });
    }

    ui.add_space(6.0);
    ui.label(RichText::new("Smart folders").strong());
    ui.collapsing("by date", |ui| {
        for (day, n) in st.kept_days() {
            pick(ui, st, out, OutputSource::Smart(Smart::Day(day.clone())), format!("{day} ({n})"));
        }
    });
    ui.collapsing("by source idea", |ui| {
        for (id, n) in st.kept_source_ideas() {
            let title =
                st.idea(&id).map(|e| e.idea.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| id.to_string());
            pick(ui, st, out, OutputSource::Smart(Smart::Idea(id)), format!("{} ({n})", short(&title, 26)));
        }
    });
    let q = if st.output_search_projects { st.output_project_search.trim().to_lowercase() } else { String::new() };
    let by_projects =
        matches!(st.output_source, OutputSource::Project(_) | OutputSource::ProjectFolder(_) | OutputSource::Run(_));
    let mut header = egui::CollapsingHeader::new("by project").default_open(by_projects);
    if !q.is_empty() || (st.output_reveal && by_projects) {
        header = header.open(Some(true));
    }
    let shown = header.show(ui, |ui| {
        let top = OutputSource::ProjectFolder(PathBuf::new());
        if st.output_source == top {
            let n = st.source_images(&top).len();
            pick(ui, st, out, top, ic(ph::FOLDER, &format!("All projects ({n})")));
        }
        if !q.is_empty() && !kept_match(st, Path::new(""), &q) {
            ui.weak(format!("No project name has “{q}”."));
        }
        by_project(ui, st, Path::new(""), &q, out);
    });
    let at = shown.header_response.rect;
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("tour_by_project_header"), at));
    let by_tag = matches!(st.output_source, OutputSource::Smart(Smart::Tag(_)));
    egui::CollapsingHeader::new("by tag").default_open(by_tag).show(ui, |ui| {
        let tags = st.kept_tags();
        if tags.is_empty() {
            ui.weak("No kept image has a tag yet.");
        }
        for (t, n) in tags {
            pick(
                ui,
                st,
                out,
                OutputSource::Smart(Smart::Tag(t.clone())),
                ic(ph::TAG, &format!("{} ({n})", short(&t, 26))),
            );
        }
    });
    let n_alpha = kept.iter().filter(|o| st.index.transparent(&o.image)).count();
    pick(
        ui,
        st,
        out,
        OutputSource::Smart(Smart::Transparent),
        ic(ph::CHECKERBOARD, &format!("transparent ({n_alpha})")),
    );
}

fn lineage_row(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    out: &mut Vec<Msg>,
    label: &str,
    imgs: Vec<increart_core::ImageRef>,
) {
    if imgs.is_empty() {
        return;
    }
    ui.weak(label);
    ui.horizontal_wrapped(|ui| {
        for i in imgs {
            let t = tex.thumb(root, &i);
            let r = thumb(ui, t.as_ref(), 64.0, false, None).on_hover_text(format!(
                "{}{}\ndouble-click: full size",
                tex.dims_text(root, &i),
                st.usage(&i).label().map(|u| format!("\nused: {u}")).unwrap_or_default()
            ));
            r.context_menu(|ui| image_menu_items(ui, root, &i, out));
            if r.double_clicked() {
                out.push(Msg::Preview(Some(i.clone())));
            }
            if r.clicked() && i.store == increart_core::Store::Output {
                out.push(Msg::OutputSelect(Some(i)));
            }
        }
    });
}

/// Source idea → parent ← image → children, and the recipe.
/// `panel`: the details panel it is in (its × hides that one).
pub fn lineage_panel(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    image: &increart_core::ImageRef,
    panel: InfoPanel,
    out: &mut Vec<Msg>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Lineage").strong());
        if ui.small_button(ph::X).on_hover_text("close").clicked() {
            out.push(Msg::ShowInfo(panel, false));
        }
    });
    let link = st.output_link(image);
    if link.is_some() {
        ui.horizontal(|ui| {
            ui.weak("title");
            match st.image_title(image) {
                Some(t) => {
                    ui.label(RichText::new(t).strong());
                }
                None => {
                    ui.weak("none");
                }
            }
            if ui.small_button(ph::PENCIL_SIMPLE).on_hover_text("give it a title   F2").clicked() {
                out.push(Msg::TitleDialog(Some(image.clone())));
            }
        });
        tag_chips(ui, st, image, out);
    }
    if let Some(p) = link.and_then(|o| st.project_of_link(o)) {
        ui.horizontal(|ui| {
            ui.weak("project");
            if ui.link(ic(ph::ROCKET_LAUNCH, &p.name())).on_hover_text("open it in Projects").clicked() {
                out.push(Msg::SelectProject(p.project.id.clone()));
                out.push(Msg::Tab(Tab::Projects));
            }
        });
    }
    let run = link.and_then(|o| st.index.run(&o.name.run)).map(|(_, r)| r.clone());
    if let Some(r) = &run {
        ui.horizontal(|ui| {
            ui.weak("run");
            if ui.link(&r.label).on_hover_text("the run's details").clicked() {
                out.push(Msg::JobInfo(Some(r.id.clone())));
            }
            if ui
                .small_button(ic(ph::CLOCK_COUNTER_CLOCKWISE, "history"))
                .on_hover_text("the Generate history that led to this run")
                .clicked()
            {
                out.push(Msg::RunHistory(r.id.clone()));
            }
        });
        if let Some(t) = link.and_then(|o| r.timings.iter().find(|t| t.seed == o.name.seed)) {
            ui.horizontal_wrapped(|ui| {
                ui.weak(ph::TIMER);
                ui.weak(increart_gui_core::timing::image_text(t, r.sample.steps));
            });
        }
    }
    if let Some(idea) = run.as_ref().and_then(|r| r.source_idea.as_ref()).and_then(|id| st.idea(id)) {
        ui.weak("source idea");
        ui.label(if idea.idea.title.is_empty() { "(untitled)".to_string() } else { idea.idea.title.clone() });
    }
    let (parent, children) = st.lineage(image);
    if let Some(r) = &run {
        lineage_row(ui, st, root, tex, out, "references", r.refs.clone());
    }
    lineage_row(ui, st, root, tex, out, "edited from (parent)", parent.into_iter().collect());
    ui.weak("this image");
    let t = tex.thumb(root, image);
    if thumb(ui, t.as_ref(), 160.0, true, None).on_hover_text("double-click: full size").double_clicked() {
        out.push(Msg::Preview(Some(image.clone())));
    }
    if let Some(u) = st.usage(image).label() {
        ui.weak(format!("used: {u}"));
    }
    lineage_row(ui, st, root, tex, out, "used by (children)", children);
    ui.separator();
    ui.label(RichText::new("Recipe").strong());
    recipe_grid(ui, st, root, tex, image, out);
}

/// Review's "Add to collection…" for one image (picked, the picks menu has
/// it for them all).
fn collect_button(ui: &mut egui::Ui, st: &AppState, image: &increart_core::ImageRef, out: &mut Vec<Msg>) {
    if !st.picked.contains(image) && ui.button(ic(ph::FOLDER_PLUS, "Add to collection…")).clicked() {
        out.push(Msg::Collect(vec![image.clone()]));
        ui.close();
    }
}

/// Add to collection for these images: Review's modal, else the submenu.
pub fn add_to_collection(ui: &mut egui::Ui, st: &AppState, images: &[increart_core::ImageRef], out: &mut Vec<Msg>) {
    if st.tab != Tab::Review {
        return collection_menu(ui, st, images, out);
    }
    if ui.button(ic(ph::FOLDER_PLUS, "Add to collection…")).clicked() {
        out.push(Msg::Collect(images.to_vec()));
        ui.close();
    }
}

/// Review's Add to collection… modal (DESIGN.md, Review): a collection
/// (filtered, or a new one made in the one picked) and the rating that goes
/// with it. ↑ / ↓ move through the collections, Enter adds, a double-click
/// adds to that one.
pub fn collect_window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let (Some(d0), Some(root)) = (&st.collect, &st.library) else { return };
    let Some(first) = d0.images.first() else { return };
    let mut d = d0.clone();
    let (mut confirm, mut cancel) = (false, false);
    let name_id = egui::Id::unique("collect_new_name");
    let mut folders = st.index.collection_folders.clone();
    folders.sort();
    let mut counts = std::collections::HashMap::<&Path, usize>::new();
    for l in &st.index.collection_links {
        *counts.entry(l.folder.as_path()).or_default() += 1;
    }
    let needle = d.filter.trim().to_lowercase();
    let shown: Vec<&PathBuf> =
        folders.iter().filter(|f| needle.is_empty() || f.to_string_lossy().to_lowercase().contains(&needle)).collect();
    // the one picked filtered out: the first shown instead
    if d.new_name.is_none() && d.folder.as_ref().is_none_or(|f| !shown.contains(&f)) {
        d.folder = shown.first().map(|f| (*f).clone());
    }
    // ↑ / ↓ (before the filter box takes them)
    let step = ctx.input_mut(|i| {
        let down = i.consume_key(egui::Modifiers::NONE, Key::ArrowDown);
        let up = i.consume_key(egui::Modifiers::NONE, Key::ArrowUp);
        down as isize - up as isize
    });
    if step != 0 && !shown.is_empty() {
        let at = d.folder.as_ref().and_then(|f| shown.iter().position(|s| *s == f));
        let i = match at {
            Some(i) => (i as isize + step).clamp(0, shown.len() as isize - 1) as usize,
            None => 0,
        };
        d.folder = Some(shown[i].clone());
    }
    let enter = ctx.input(|i| i.key_pressed(Key::Enter));
    let m = egui::Modal::new(egui::Id::unique("collect")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading(ic(ph::FOLDER_PLUS, "Add to collection"));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let n = d.images.len();
            let t = tex.thumb(root, first);
            if n > 1 {
                crate::widgets::stacked_thumb(ui, t.as_ref(), 64.0);
            } else {
                thumb(ui, t.as_ref(), 64.0, false, None);
            }
            ui.vertical(|ui| {
                ui.label(RichText::new(if n == 1 { "1 image".into() } else { format!("{n} images") }).strong());
                match st.output_link(first).filter(|_| n == 1) {
                    Some(o) => {
                        let project = st.index.run(&o.name.run).map(|(p, _)| p.name()).unwrap_or_default();
                        ui.weak(format!("{project} · seed {}", o.name.seed));
                    }
                    None => {
                        ui.weak("the picked");
                    }
                }
            });
        });
        ui.add_space(6.0);
        let filter = ui.add(
            egui::TextEdit::singleline(&mut d.filter)
                .hint_text(format!("{} Filter collections", ph::MAGNIFYING_GLASS))
                .desired_width(f32::INFINITY),
        );
        if d.new_name.is_none() && ui.memory(|m| m.focused().is_none()) {
            filter.request_focus();
        }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                if d.new_name.is_some() {
                    // a new one can go at the top level too
                    let r = ui.selectable_label(d.folder.is_none(), ic(ph::FOLDERS, "collections (the top level)"));
                    if r.clicked() {
                        d.folder = None;
                    }
                }
                if folders.is_empty() {
                    ui.weak("No collections yet: make one with New collection…");
                } else if shown.is_empty() {
                    ui.weak("No collection's name has that in it.");
                }
                for f in &shown {
                    let depth = f.components().count().saturating_sub(1);
                    // filtered: whole paths, a flat list
                    let name = if needle.is_empty() {
                        f.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
                    } else {
                        f.display().to_string()
                    };
                    ui.horizontal(|ui| {
                        if needle.is_empty() {
                            ui.add_space(depth as f32 * 18.0);
                        }
                        let picked = d.folder.as_ref() == Some(*f);
                        let r = ui.selectable_label(picked, ic(ph::FOLDER, &name));
                        if picked && step != 0 {
                            r.scroll_to_me(None);
                        }
                        if r.clicked() {
                            d.folder = Some((*f).clone());
                        }
                        if r.double_clicked() && d.new_name.is_none() {
                            confirm = true;
                        }
                        ui.weak(counts.get(f.as_path()).copied().unwrap_or(0).to_string());
                        if st.collect_last.0.as_ref() == Some(*f) {
                            ui.weak("· last used");
                        }
                    });
                }
            });
        });
        let parent = d.folder.clone().unwrap_or_default();
        let mut problem = None;
        if let Some(name) = &mut d.new_name {
            let mut drop = false;
            ui.horizontal(|ui| {
                ui.label(ic(ph::PLUS, "New collection in"));
                ui.strong(if parent.as_os_str().is_empty() {
                    "the top level".into()
                } else {
                    parent.display().to_string()
                });
                ui.add(egui::TextEdit::singleline(name).id(name_id).hint_text("its name").desired_width(180.0));
                drop = ui.small_button(ph::X).on_hover_text("not a new one: add to the one picked").clicked();
            });
            if name.trim().is_empty() {
                problem = Some("Name the new collection".to_string());
            } else if let Some(p) = increart_gui_core::update::collection_name_problem(st, &parent, name) {
                ui.colored_label(BAD, &p);
                problem = Some(p);
            }
            if drop {
                d.new_name = None;
                problem = None;
            }
        }
        if d.target().is_none() {
            problem = Some("Pick a collection, or make one with New collection…".into());
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Rate").strong());
            ui.add_space(6.0);
            if ui.selectable_label(d.verdict.is_none(), "don't rate").clicked() {
                d.verdict = None;
            }
            for (label, v, c) in VERDICTS {
                if ui.selectable_label(d.verdict == Some(v), RichText::new(label.to_lowercase()).color(c)).clicked() {
                    d.verdict = Some(v);
                }
            }
        });
        ui.weak(match d.verdict {
            Some(_) if st.review_view == ReviewView::Sheet => "Rated, it stays on the sheet with its new colour.",
            Some(_) => "Rated, it leaves Review's queue and the next image shows. Undo (4 / Ctrl+Z) takes both back.",
            None => "It stays in Review's queue, unrated.",
        });
        ui.add_space(8.0);
        let what = d.target().map(|t| t.display().to_string()).unwrap_or_default();
        let rate = d.verdict.and_then(|v| VERDICTS.iter().find(|x| x.1 == v)).map(|x| x.0.to_lowercase());
        let label = match (d.new_name.is_some(), rate) {
            (false, None) => format!("Add to {what}"),
            (false, Some(r)) => format!("Add to {what} and rate {r}"),
            (true, None) => format!("Make {what} and add"),
            (true, Some(r)) => format!("Make {what}, add and rate {r}"),
        };
        crate::widgets::dialog_footer(
            ui,
            |ui| {
                if d.new_name.is_none()
                    && ui.button(ic(ph::PLUS, "New collection…")).on_hover_text("in the collection picked").clicked()
                {
                    d.new_name = Some(String::new());
                    ui.memory_mut(|m| m.request_focus(name_id));
                }
            },
            |ui| {
                let r = ui.add_enabled(
                    problem.is_none(),
                    egui::Button::new(RichText::new(ic(ph::FOLDER_PLUS, &format!("{label}   Enter"))).strong()),
                );
                if r.on_disabled_hover_text(problem.clone().unwrap_or_default()).clicked() {
                    confirm = true;
                }
                if ui.button("Cancel   Esc").clicked() {
                    cancel = true;
                }
            },
        );
        if enter && problem.is_none() {
            confirm = true;
        }
    });
    if m.should_close() {
        cancel = true;
    }
    if d != *d0 {
        out.push(Msg::EditCollect(d));
    }
    if cancel {
        out.push(Msg::CancelCollect);
    } else if confirm {
        out.push(Msg::ConfirmCollect);
    }
}

/// "Tag…" for one image (picked, the picks menu has it for them all).
pub fn tag_button(ui: &mut egui::Ui, st: &AppState, image: &increart_core::ImageRef, out: &mut Vec<Msg>) {
    if !st.picked.contains(image) && st.output_link(image).is_some() && ui.button(ic(ph::TAG, "Tag…   L")).clicked() {
        out.push(Msg::TagImages(vec![image.clone()]));
        ui.close();
    }
}

/// An output image's tags as chips (× takes one off), and + for the Tag…
/// modal.
pub fn tag_chips(ui: &mut egui::Ui, st: &AppState, image: &increart_core::ImageRef, out: &mut Vec<Msg>) {
    if st.output_link(image).is_none() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.weak("tags");
        for t in st.image_tags(image) {
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .stroke(egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color))
                .corner_radius(8.0)
                .inner_margin(egui::Margin { left: 6, right: 2, top: 0, bottom: 0 })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.label(t);
                    let x = ui.add(egui::Button::new(RichText::new(ph::X).small()).frame(false));
                    if x.on_hover_text(format!("take “{t}” off (undo: 4 / Ctrl+Z)")).clicked() {
                        out.push(Msg::RemoveTag(image.clone(), t.clone()));
                    }
                });
        }
        if st.image_tags(image).is_empty() {
            ui.weak("none");
        }
        if ui.small_button(ph::PLUS).on_hover_text("tag it…   L").clicked() {
            out.push(Msg::TagImages(vec![image.clone()]));
        }
    });
}

/// L / Shift+L on these images: the Tag… modal, or the last tags again.
pub fn tag_keys(i: &egui::InputState, images: Vec<increart_core::ImageRef>, out: &mut Vec<Msg>) {
    if images.is_empty() || i.modifiers.command || !i.key_pressed(Key::L) {
        return;
    }
    out.push(if i.modifiers.shift { Msg::TagAgain(images) } else { Msg::TagImages(images) });
}

/// "bad hands, sky", or "3 tags" when there are more.
fn tag_list(tags: &[String]) -> String {
    match tags {
        [a] => format!("“{a}”"),
        [a, b] => format!("“{a}”, “{b}”"),
        _ => format!("{} tags", tags.len()),
    }
}

/// The Tag… modal (DESIGN.md, Output library tags): the image or a stack of
/// the picks, a field that adds what's typed and narrows the list, the
/// tags with three-state ticks (on all, on none, on some: "2 / 5"), and in
/// Review the rating that goes with it. ↑ / ↓ move through the list, Space
/// or Enter ticks the row, Enter adds what's typed, Enter with nothing
/// typed applies.
pub fn tag_window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let (Some(d0), Some(root)) = (&st.tagging, &st.library) else { return };
    let Some(first) = d0.images.first() else { return };
    let mut d = d0.clone();
    let (mut confirm, mut cancel) = (false, false);
    let known = st.known_tags();
    let rows = st.tag_rows(&d);
    let n = d.images.len();
    let field_id = egui::Id::unique("tag_field");
    if d.cursor.as_ref().is_some_and(|c| !rows.iter().any(|r| &r.tag == c)) {
        d.cursor = None;
    }
    // ↑ / ↓ (before the field takes them)
    let step = ctx.input_mut(|i| {
        let down = i.consume_key(egui::Modifiers::NONE, Key::ArrowDown);
        let up = i.consume_key(egui::Modifiers::NONE, Key::ArrowUp);
        down as isize - up as isize
    });
    if step != 0 && !rows.is_empty() {
        let at = d.cursor.as_ref().and_then(|c| rows.iter().position(|r| &r.tag == c));
        let i = match at {
            Some(i) => (i as isize + step).clamp(0, rows.len() as isize - 1) as usize,
            None => 0,
        };
        d.cursor = Some(rows[i].tag.clone());
    }
    let at_row =
        |d: &increart_gui_core::TagDialog| d.cursor.as_ref().and_then(|c| rows.iter().find(|r| &r.tag == c)).cloned();
    // Space ticks the row the arrows are on, while nothing is typed
    let space =
        d.field.is_empty() && d.cursor.is_some() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Space));
    if space && let Some(r) = at_row(&d) {
        d.toggle(&r);
    }
    let enter = ctx.input(|i| i.key_pressed(Key::Enter));
    let m = egui::Modal::new(egui::Id::unique("tag_images")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading(ic(ph::TAG, if n == 1 { "Tag 1 image".into() } else { format!("Tag {n} images") }.as_str()));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let t = tex.thumb(root, first);
            if n > 1 {
                crate::widgets::stacked_thumb(ui, t.as_ref(), 64.0);
            } else {
                thumb(ui, t.as_ref(), 64.0, false, None);
            }
            ui.vertical(|ui| match st.output_link(first).filter(|_| n == 1) {
                Some(o) => {
                    let project = st.index.run(&o.name.run).map(|(p, _)| p.name()).unwrap_or_default();
                    ui.weak(format!("{project} · seed {}", o.name.seed));
                }
                None if d.images == st.picked => {
                    ui.weak(format!("the {n} picked"));
                }
                None => {}
            });
        });
        ui.add_space(6.0);
        let before = d.field.clone();
        let field = ui.add(
            egui::TextEdit::singleline(&mut d.field)
                .id(field_id)
                .hint_text(format!("{} Add a tag, or find one", ph::TAG))
                .desired_width(f32::INFINITY),
        );
        if space {
            // the space typed into the field too
            d.field = before.clone();
        }
        if d.field != before {
            d.cursor = None;
        }
        if ui.memory(|m| m.focused().is_none()) {
            field.request_focus();
        }
        ui.weak("Enter adds what's typed (commas for several) · ↑ ↓ then Space ticks a row");
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical().max_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
                if rows.is_empty() {
                    ui.weak(if d.field.trim().is_empty() {
                        "No tags yet: type one above.".to_string()
                    } else {
                        format!("No tag has that in it: Enter makes “{}”.", d.field.trim())
                    });
                }
                for r in &rows {
                    // what Apply makes it: on all, on none, on some
                    let all = r.change == Some(true) || (r.change.is_none() && r.have == n);
                    let some = r.change.is_none() && r.have > 0 && r.have < n;
                    let tick = if all {
                        ph::CHECK_SQUARE
                    } else if some {
                        ph::MINUS_SQUARE
                    } else {
                        ph::SQUARE
                    };
                    ui.horizontal(|ui| {
                        let mut text = RichText::new(format!("{tick}  {}", r.tag));
                        if r.change.is_some() {
                            text = text.color(crate::widgets::ACCENT);
                        }
                        let on = d.cursor.as_ref() == Some(&r.tag);
                        let row = ui.selectable_label(on, text);
                        if on && step != 0 {
                            row.scroll_to_me(None);
                        }
                        if row.clicked() {
                            d.toggle(r);
                            d.cursor = Some(r.tag.clone());
                        }
                        if some {
                            ui.weak(format!("{} / {n}", r.have));
                        }
                        match r.change {
                            Some(true) if r.have < n => {
                                ui.weak(if r.have == 0 && !known.iter().any(|(k, _)| k == &r.tag) {
                                    "new"
                                } else {
                                    "added"
                                });
                            }
                            Some(false) => {
                                ui.weak("taken off");
                            }
                            _ => {}
                        }
                    });
                }
            });
        });
        if d.rate {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Rate").strong());
                ui.add_space(6.0);
                if ui.selectable_label(d.verdict.is_none(), "don't rate").clicked() {
                    d.verdict = None;
                }
                for (label, v, c) in VERDICTS {
                    if ui.selectable_label(d.verdict == Some(v), RichText::new(label.to_lowercase()).color(c)).clicked()
                    {
                        d.verdict = Some(v);
                    }
                }
            });
            ui.weak(match d.verdict {
                Some(_) if st.review_view == ReviewView::Sheet => "Rated, it stays on the sheet with its new colour.",
                Some(_) => "Rated, it leaves Review's queue. Undo (4 / Ctrl+Z) takes back the tags and the rating.",
                None => "It stays in Review's queue, unrated. Undo (4 / Ctrl+Z) takes back the tags.",
            });
        }
        ui.add_space(8.0);
        let c = d.change(&known);
        let mut what = vec![];
        if !c.add.is_empty() {
            what.push(format!("Add {}", tag_list(&c.add)));
        }
        if !c.remove.is_empty() {
            what.push(format!("{} {}", if what.is_empty() { "Take off" } else { "take off" }, tag_list(&c.remove)));
        }
        let rate = d.verdict.and_then(|v| VERDICTS.iter().find(|x| x.1 == v)).map(|x| x.0.to_lowercase());
        let on = if n == 1 { "this image".to_string() } else { format!("{n} images") };
        let label = match (what.is_empty(), rate.clone()) {
            (true, None) => "Apply".to_string(),
            (true, Some(r)) => format!("Rate {r}, no tags changed"),
            (false, None) => format!("{} · {on}", what.join(", ")),
            (false, Some(r)) => format!("{} · {on} · rate {r}", what.join(", ")),
        };
        let ready = !what.is_empty() || rate.is_some();
        crate::widgets::dialog_footer(
            ui,
            |_| {},
            |ui| {
                let r = ui.add_enabled(ready, egui::Button::new(RichText::new(ic(ph::TAG, &label)).strong()));
                if r.on_disabled_hover_text("Tick, untick or type a tag first").clicked() {
                    confirm = true;
                }
                if ui.button("Cancel   Esc").clicked() {
                    cancel = true;
                }
            },
        );
        // Enter: tick the row the arrows are on, else add what's typed,
        // else apply
        if enter {
            if let Some(r) = at_row(&d) {
                d.toggle(&r);
            } else if !d.field.trim().is_empty() {
                d.add_typed(&known);
            } else if ready {
                confirm = true;
            }
            ui.memory_mut(|m| m.request_focus(field_id));
        }
    });
    if m.should_close() {
        cancel = true;
    }
    if d != *d0 {
        out.push(Msg::EditTags(d));
    }
    if cancel {
        out.push(Msg::CancelTags);
    } else if confirm {
        out.push(Msg::ConfirmTags);
    }
}

/// "Add to collection ▸": existing collections and a new one.
pub fn collection_menu(ui: &mut egui::Ui, st: &AppState, images: &[increart_core::ImageRef], out: &mut Vec<Msg>) {
    ui.menu_button(ic(ph::FOLDER_PLUS, "Add to collection"), |ui| {
        let mut folders = st.index.collection_folders.clone();
        folders.sort();
        for f in folders {
            if ui.button(f.display().to_string()).clicked() {
                out.push(Msg::AddToCollection(f.clone(), images.to_vec()));
                ui.close();
            }
        }
        ui.separator();
        if ui.button(ic(ph::PLUS, "New collection…")).clicked() {
            out.push(Msg::NewCollection(Default::default(), images.to_vec()));
            ui.close();
        }
    });
}

/// The Discarded tab: the images deleted from projects, newest first, to
/// restore to where they were or delete for good.
fn discarded_outputs(ui: &mut egui::Ui, st: &AppState, root: &Path, tex: &mut Textures, out: &mut Vec<Msg>) {
    let images = st.discarded_outputs();
    if images.is_empty() {
        ui.weak("No discarded images. Deleting a rejected image (Delete… on its right-click) puts it here, to restore later.");
        return;
    }
    ui.horizontal(|ui| {
        ui.weak("Restore puts an image back in its project, rated as it was. Images here aren't generated again.");
        crate::widgets::right_group(ui, "empty_discarded", |ui| {
            if ui.button(ic(ph::TRASH, "Empty discarded…")).clicked() {
                let text = format!(
                    "Delete the {} discarded image(s) permanently? Their links go to the trash; the images stay in the store until Clean up.",
                    images.len()
                );
                out.push(Msg::Ask(Confirm::new(text, "Delete", Msg::EmptyDiscardedOutputs)));
            }
        });
    });
    let size = st.grid_cell_size(ZoomGrid::Output) as f32;
    grid_ctrl_scroll(ui, st, ZoomGrid::Output, out);
    ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(6.0);
            for o in images {
                let t = tex.thumb(root, &o.image);
                let tip = format!("{}\ndouble-click: full size", describe(st, o));
                let selected = st.output_selected.as_ref() == Some(&o.image);
                let r = thumb(ui, t.as_ref(), size, selected, Some(verdict_color(o.verdict))).on_hover_text(tip);
                if single_click(ui, &r) || (st.output_selected.is_some() && r.clicked()) {
                    out.push(Msg::OutputSelect(Some(o.image.clone())));
                }
                if r.double_clicked() {
                    out.push(Msg::Preview(Some(o.image.clone())));
                }
                let restore = || Msg::RestoreOutput(o.path.clone());
                let delete = || {
                    let text = "Delete this image permanently? Its link goes to the trash; the image stays in the store until Clean up.";
                    Msg::Ask(Confirm::new(text, "Delete", Msg::DeleteDiscardedOutput(o.path.clone())))
                };
                if ui.rect_contains_pointer(r.rect) {
                    let buttons = [
                        (ph::ARROW_COUNTER_CLOCKWISE, "Restore", "back to its project"),
                        (ph::TRASH, "Delete", "permanently (asks first)"),
                    ];
                    match crate::widgets::cover_buttons(ui, r.rect, &buttons) {
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
                    if ui.button(ic(ph::FOLDER_OPEN, "Show in file manager")).clicked() {
                        out.push(Msg::ShowInFileManager(o.path.clone()));
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

fn image_grid(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    out: &mut Vec<Msg>,
    images: &[&OutputLink],
    rejected: bool,
) {
    let bg = crate::tiles::background(ui, st, out);
    let mut rects = vec![];
    let size = st.grid_cell_size(ZoomGrid::Output) as f32;
    grid_ctrl_scroll(ui, st, ZoomGrid::Output, out);
    ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(6.0);
            for o in images {
                let t = tex.thumb(root, &o.image);
                let tip = st
                    .index
                    .run(&o.name.run)
                    .map(|(_, run)| format!("{}\n{}\ndouble-click: full size", describe(st, o), short(&run.prompt, 80)))
                    .unwrap_or_else(|| "double-click: full size".into());
                let selected = crate::tiles::cursor(st, st.output_selected.as_ref() == Some(&o.image));
                let r = thumb(ui, t.as_ref(), size, selected, Some(verdict_color(o.verdict))).on_hover_text(tip);
                // dragged onto a collection in the side panel: the picks
                // when it's one of them, else just this one
                let r = crate::widgets::image_drag_source(&r, &o.path, || match st.picked.contains(&o.image) {
                    true => st.picked.clone(),
                    false => vec![o.image.clone()],
                });
                if st.output_selected.as_ref() == Some(&o.image) {
                    crate::widgets::reveal_cursor(ui, "output", o.image.clone(), &r);
                }
                if rects.is_empty() {
                    // where it is, for the screenshot tour's double-click check
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("tour_first_output"), r.rect));
                }
                crate::tiles::mark(ui, st, r.rect, &o.image);
                rects.push((o.image.clone(), r.rect));
                // the lineage panel opens on a single click only (a double-click
                // previews); while it's open, a click shows the image in it at once
                let picked = crate::tiles::click(ui, st, &r, &o.image, out);
                let open = if st.output_selected.is_some() { r.clicked() } else { single_click(ui, &r) };
                if !picked && open {
                    out.push(Msg::OutputSelect(Some(o.image.clone())));
                }
                if r.double_clicked() {
                    out.push(Msg::Preview(Some(o.image.clone())));
                }
                r.context_menu(|ui| {
                    crate::tiles::menu(ui, st, &o.image, out);
                    if ui.button(ic(ph::ARROW_COUNTER_CLOCKWISE, "Back to Review")).clicked() {
                        out.push(Msg::ReviewLink(o.path.clone(), None));
                        ui.close();
                    }
                    for (label, v, c) in VERDICTS {
                        if o.verdict != Some(v)
                            && ui.button(RichText::new(format!("Rate {}", label.to_lowercase())).color(c)).clicked()
                        {
                            out.push(Msg::ReviewLink(o.path.clone(), Some(v)));
                            ui.close();
                        }
                    }
                    ui.separator();
                    tag_button(ui, st, &o.image, out);
                    collection_menu(ui, st, std::slice::from_ref(&o.image), out);
                    if let OutputSource::Collection(f) = &st.output_source
                        && let Some(link) = st.index.collection_links.iter().find(|l| l.image == o.image && l.folder.starts_with(f))
                        && ui.button(ic(ph::MINUS_CIRCLE, "Remove from this collection")).clicked()
                    {
                        let rel = link.path.strip_prefix("collections").map(Path::to_path_buf).unwrap_or_default();
                        out.push(Msg::DeleteNode(Tree::Collections, rel));
                        ui.close();
                    }
                    let title = if st.image_title(&o.image).is_some() { "Edit title…" } else { "Set title…" };
                    if ui.button(ic(ph::TEXT_AA, title)).clicked() {
                        out.push(Msg::TitleDialog(Some(o.image.clone())));
                        ui.close();
                    }
                    if let Some(p) = st.project_of_link(o)
                        && ui.button(ic(ph::IMAGE_SQUARE, "Use as project cover")).clicked()
                    {
                        out.push(Msg::SetCover(p.project.id.clone(), Some(o.image.clone())));
                        ui.close();
                    }
                    if ui.button(ic(ph::EXPORT, "Export…")).clicked() {
                        out.push(Msg::OpenExport(vec![o.image.clone()], "1 image".into()));
                        ui.close();
                    }
                    if ui.button(ic(ph::FOLDER_OPEN, "Show in file manager")).clicked() {
                        out.push(Msg::ShowInFileManager(o.path.clone()));
                        ui.close();
                    }
                    if ui.button(ic(ph::COPY, "Copy path")).clicked() {
                        let p = st.library.as_ref().map(|r| r.join(&o.path)).unwrap_or_else(|| o.path.clone());
                        ui.ctx().copy_text(p.display().to_string());
                        ui.close();
                    }
                    if rejected && ui.button(ic(ph::TRASH, "Delete…")).clicked() {
                        let text = "Delete this image? It goes to Discarded (restore it from Output library → Discarded); its seed isn't generated again.";
                        out.push(Msg::Ask(Confirm::new(text, "Delete", Msg::DeleteOutput(o.path.clone()))));
                        ui.close();
                    }
                    ui.separator();
                    use_menu(ui, st, root, &o.image, out);
                });
            }
        });
    });
    crate::tiles::band(ui, st, &bg, &rects, out);
}
