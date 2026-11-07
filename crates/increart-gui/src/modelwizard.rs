//! **Models**, the servers screen's right half (DESIGN.md, Setting up
//! models): your models and the built-in ones in three read-only lists
//! (yours, built-in installed, built-in not installed) with search, filter
//! and order; **Edit…** opens **Set up models**, a window with the model
//! folders and the built-in models to add (downloading what isn't there).

use eframe::egui::{self, RichText};
use increart_core::config::RecipePreset;
use increart_core::tiers::Gpu;
use increart_gui_core::{
    AppState, ModelColumn, ModelKind, ModelLists, ModelSetupTab, ModelSort, ModelWizard, Msg, PickKind, PresetNeeds,
};

use crate::widgets::{ACCENT, BAD, GOOD, NEUTRAL, dialog_footer, ic, ph};

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1e9)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The GPUs listed, or none while listing (or when they can't be).
fn gpus_of(w: &ModelWizard) -> Vec<Gpu> {
    match &w.gpus {
        Some(Ok(g)) => g.clone(),
        _ => vec![],
    }
}

/// The GPUs on one line ("GPU 0: … (24 GB) · GPU 1: …"), or why not.
fn gpu_line(ui: &mut egui::Ui, w: &ModelWizard) {
    match &w.gpus {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Listing the GPUs…");
            });
        }
        Some(Err(e)) => {
            ui.weak(format!("The GPUs couldn't be listed ({e}), so there's no fit column."));
        }
        Some(Ok(g)) => {
            let list: Vec<String> =
                g.iter().map(|g| format!("GPU {}: {} ({:.0} GB)", g.number, g.name, g.vram_gib)).collect();
            ui.weak(list.join("  ·  "));
        }
    }
}

/// The servers screen's right half: your models and the built-in ones,
/// read only, greyed until there's a server setup. Opens its state (folders
/// found, GPUs, sizes) the first time it's shown.
pub fn panel(ui: &mut egui::Ui, st: &AppState, list_height: f32, out: &mut Vec<Msg>) {
    let Some(old) = &st.model_wizard else {
        out.push(Msg::OpenModelWizard);
        ui.spinner();
        return;
    };
    let mut w = old.clone();
    let servers = !st.config.server_presets.is_empty();
    let r = ui.add_enabled_ui(servers, |ui| overview(ui, st, &mut w, list_height, out));
    if !servers {
        // disabled widgets only fade a little: dim the half too, and say why
        let rect = r.response.rect.expand(6.0);
        ui.painter().rect_filled(rect, 6.0, egui::Color32::from_black_alpha(150));
        let lines = ["Add a server setup first (on the left):", "models are set up for the servers that run them"];
        for (i, line) in lines.into_iter().enumerate() {
            ui.painter().text(
                rect.center() + egui::vec2(0.0, (i as f32 - 0.5) * 22.0),
                egui::Align2::CENTER_CENTER,
                line,
                egui::FontId::proportional(16.0),
                egui::Color32::from_gray(210),
            );
        }
    }
    if w.editing && servers {
        setup_window(ui.ctx(), st, &mut w, out);
    }
    if &w != old {
        out.push(Msg::EditModelWizard(Box::new(w)));
    }
}

fn overview(ui: &mut egui::Ui, st: &AppState, w: &mut ModelWizard, list_height: f32, out: &mut Vec<Msg>) {
    let catalogue = increart_core::config::catalogue();
    let model_dirs = &st.config.programs.model_dirs;
    let lists = ModelLists::of(&st.config.recipes, &catalogue, model_dirs, &w.sizes);
    egui::Sides::new().show(
        ui,
        |ui| ui.heading(ic(ph::CUBE, "Models")),
        |ui| {
            if ui
                .button(RichText::new(ic(ph::PENCIL_SIMPLE, "Edit…")).strong())
                .on_hover_text("Set up models: the model folders, and built-in models to add and download")
                .clicked()
            {
                w.editing = true;
            }
            if ui
                .button(ic(ph::SLIDERS_HORIZONTAL, "Model settings…"))
                .on_hover_text("each of your models' files and settings; make your own; test one on a server")
                .clicked()
            {
                out.push(Msg::OpenRecipes);
            }
        },
    );
    ui.label(
        "The image models the servers can load, each its files and the settings it runs with. Edit… sets up the \
         model folders and installs built-in models (downloading their files).",
    );
    let dirs: Vec<&String> = model_dirs.iter().filter(|d| !d.trim().is_empty()).collect();
    ui.weak(match dirs.as_slice() {
        [] => "No model folders yet: Edit… to choose one.".to_string(),
        [one] => format!("Model folder: {one}"),
        [first, more @ ..] => format!("Model folders: {first} and {} more", more.len()),
    });
    ui.add_space(6.0);
    filters(ui, w, "overview");
    gpu_line(ui, w);
    ui.add_space(4.0);
    let gpus = gpus_of(w);
    let vram = w.ticked_vram(&gpus);
    let sections = [
        (
            "Your custom models",
            "models_custom",
            &lists.custom,
            Section::Custom,
            "None: models you make in Model settings… (not built-in ones) are listed here.",
        ),
        (
            "Built-in models, installed",
            "models_installed",
            &lists.installed,
            Section::Installed,
            "None yet: Edit… → 2. Add built-in models.",
        ),
        (
            "Built-in models, not installed",
            "models_available",
            &lists.available,
            Section::Available,
            "None: every built-in model is installed.",
        ),
    ];
    egui::ScrollArea::vertical()
        .id_salt("models_overview")
        .min_scrolled_height(list_height)
        .max_height(list_height)
        .show(ui, |ui| {
            for (i, (title, id, all, section, none)) in sections.into_iter().enumerate() {
                if i > 0 {
                    ui.add_space(6.0);
                }
                let shown = w.shown(all.clone(), &vram);
                let n = all.len();
                let count = if shown.len() < n { format!("{} of {n} shown", shown.len()) } else { n.to_string() };
                // collapsible, open to start (egui keeps each one's state)
                egui::CollapsingHeader::new(RichText::new(format!("{title}  ({count})")).strong())
                    .id_salt(id)
                    .default_open(true)
                    .show(ui, |ui| {
                        if all.is_empty() {
                            ui.weak(none);
                        } else {
                            list_table(ui, id, &shown, &gpus, section, &mut w.sort);
                        }
                    });
            }
        });
    ui.add_space(6.0);
    download_line(ui, st, out);
}

/// Search, kind, fits a GPU, order: one row, for the lists and the window's.
fn filters(ui: &mut egui::Ui, w: &mut ModelWizard, salt: &str) {
    ui.horizontal(|ui| {
        ui.label(ph::MAGNIFYING_GLASS);
        ui.add(egui::TextEdit::singleline(&mut w.search).desired_width(160.0).hint_text("Search models"));
        if !w.search.is_empty() && ui.small_button(ph::X).on_hover_text("clear the search").clicked() {
            w.search.clear();
        }
        egui::ComboBox::from_id_salt(("model_kind", salt)).selected_text(w.kind.label()).show_ui(ui, |ui| {
            for k in ModelKind::ALL {
                ui.selectable_value(&mut w.kind, k, k.label());
            }
        });
        fits_popover(ui, w, salt);
        if w.filtering()
            && ui.small_button("Reset").on_hover_text("clear the search, all kinds, every GPU ticked").clicked()
        {
            w.search.clear();
            w.kind = ModelKind::All;
            w.gpus_off.clear();
        }
    });
}

/// **Fits GPU ▾**: a checkbox per GPU, all ticked to start; a model is
/// listed when it fits one ticked (none ticked: every size).
fn fits_popover(ui: &mut egui::Ui, w: &mut ModelWizard, salt: &str) {
    let gpus = gpus_of(w);
    let ticked: Vec<u32> = gpus.iter().map(|g| g.number).filter(|n| !w.gpus_off.contains(n)).collect();
    let text = if gpus.is_empty() {
        "Fits GPU".to_string()
    } else if ticked.len() == gpus.len() {
        "Fits GPU: any".to_string()
    } else if ticked.is_empty() {
        "Fits GPU: off".to_string()
    } else {
        format!("Fits GPU {}", ticked.iter().map(u32::to_string).collect::<Vec<_>>().join(", "))
    };
    let r = ui
        .add_enabled(!gpus.is_empty(), egui::Button::new(format!("{text} {}", ph::CARET_DOWN)))
        .on_hover_text("only models that fit in one of the ticked GPUs' memory")
        .on_disabled_hover_text("the GPUs aren't listed (yet)");
    egui::Popup::menu(&r)
        .id(egui::Id::unique(("fits_gpu", salt)))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(400.0);
            ui.weak("Show models that fit in:");
            for g in &gpus {
                let mut on = !w.gpus_off.contains(&g.number);
                let label = format!("GPU {}: {} ({:.0} GB)", g.number, g.name, g.vram_gib);
                if ui.checkbox(&mut on, label).changed() {
                    if on {
                        w.gpus_off.remove(&g.number);
                    } else {
                        w.gpus_off.insert(g.number);
                    }
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.small_button("All").clicked() {
                    w.gpus_off.clear();
                }
                if ui.small_button("None").on_hover_text("every size shown").clicked() {
                    w.gpus_off = gpus.iter().map(|g| g.number).collect();
                }
            });
            ui.weak("None ticked: every size. Sizes not known yet are shown.");
        });
}

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Custom,
    Installed,
    Available,
}

/// What a model's files need, as the Files column says it.
fn status(needs: &PresetNeeds, section: Section) -> RichText {
    if needs.unavailable {
        let t = if section == Section::Custom { "files missing" } else { "not there, no download" };
        return RichText::new(ic(ph::WARNING, t)).color(NEUTRAL);
    }
    if needs.downloads.is_empty() {
        let t = if section == Section::Available { "files here, not installed" } else { "ready" };
        return RichText::new(ic(ph::CHECK, t)).color(GOOD);
    }
    let n = needs.downloads.len();
    let size = needs.download_bytes.map_or(String::new(), |b| format!(" ({})", gb(b)));
    if section == Section::Available {
        RichText::new(ic(ph::DOWNLOAD_SIMPLE, &format!("download {n} file{}{size}", plural(n))))
    } else {
        RichText::new(ic(ph::DOWNLOAD_SIMPLE, &format!("{n} file{} missing{size}", plural(n)))).color(NEUTRAL)
    }
}

const NAME_W: f32 = 230.0;
const SIZE_W: f32 = 62.0;
const FILES_W: f32 = 190.0;

/// The column headings, each a click to sort by it (again: the other way;
/// a third time: the list's order), the order shown by an arrow.
fn heads(h: &mut egui_extras::TableRow, model_tip: &str, sort: &mut ModelSort) {
    let heads = [
        (ModelColumn::Name, "Model", model_tip),
        (ModelColumn::Size, "Size", "all its files together, about the GPU memory it needs"),
        (ModelColumn::Files, "Files", "whether its files are in your model folders, or what's downloaded"),
        (
            ModelColumn::Fits,
            "Fits GPU",
            "by GPU number (listed above): ✓ fits in its memory; ✗ too large, it'd spill to RAM (slow) if it runs \
             at all",
        ),
    ];
    for (column, title, tip) in heads {
        h.col(|ui| {
            let arrow = match sort.by(column) {
                Some(false) => format!(" {}", ph::CARET_UP),
                Some(true) => format!(" {}", ph::CARET_DOWN),
                None => String::new(),
            };
            let r = ui.add(egui::Button::new(RichText::new(format!("{title}{arrow}")).strong()).frame(false));
            let how = match (sort.by(column), column) {
                (Some(true), _) => "click: back to the list's order",
                (_, ModelColumn::Name) => "click: sort A–Z (again: Z–A)",
                (_, ModelColumn::Size) => "click: smallest first (again: largest)",
                (_, ModelColumn::Files) => "click: ready first, then least to download (again: the other way)",
                (_, ModelColumn::Fits) => "click: fitting the most ticked GPUs first (again: the other way)",
            };
            let tip = if tip.is_empty() { how.to_string() } else { format!("{tip}\n{how}") };
            if r.on_hover_text(tip).clicked() {
                sort.click(column);
            }
        });
    }
}

fn size_cell(ui: &mut egui::Ui, needs: &PresetNeeds) {
    match needs.total_bytes {
        Some(b) => ui.label(gb(b)),
        None => ui.weak("…").on_hover_text("asking Hugging Face for the sizes"),
    };
}

fn fits_cell(ui: &mut egui::Ui, needs: &PresetNeeds, gpus: &[Gpu]) {
    if gpus.is_empty() {
        ui.weak("—");
    }
    for g in gpus {
        let label = if gpus.len() > 1 { format!("{} ", g.number) } else { String::new() };
        match needs.fits(g.vram_gib) {
            Some(true) => {
                ui.label(RichText::new(format!("{label}✓")).color(GOOD))
                    .on_hover_text(format!("fits {} ({:.0} GB)", g.name, g.vram_gib));
            }
            Some(false) => {
                ui.label(RichText::new(format!("{label}✗")).color(BAD)).on_hover_text(format!(
                    "larger than {} ({:.0} GB): spills to RAM, slow, if it runs at all",
                    g.name, g.vram_gib
                ));
            }
            None => {
                ui.weak(format!("{label}…"));
            }
        }
    }
}

fn tip(p: &RecipePreset) -> String {
    if p.note.is_empty() { p.recipe.clone() } else { format!("{}\n{}", p.note, p.recipe) }
}

/// One of the read-only lists.
fn list_table(
    ui: &mut egui::Ui,
    id: &str,
    rows: &[(&RecipePreset, PresetNeeds)],
    gpus: &[Gpu],
    section: Section,
    sort: &mut ModelSort,
) {
    use egui_extras::{Column, TableBuilder};
    if rows.is_empty() {
        ui.weak("None match the filter.");
        return;
    }
    TableBuilder::new(ui)
        .id_salt(id)
        .striped(true)
        .vscroll(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(NAME_W).clip(true))
        .column(Column::exact(SIZE_W))
        .column(Column::exact(FILES_W).clip(true))
        .column(Column::remainder().at_least(100.0).clip(true))
        .header(20.0, |mut h| heads(&mut h, "", sort))
        .body(|mut body| {
            for (p, needs) in rows {
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        ui.label(&p.name).on_hover_text(tip(p));
                    });
                    row.col(|ui| size_cell(ui, needs));
                    row.col(|ui| {
                        ui.label(status(needs, section));
                    });
                    row.col(|ui| fits_cell(ui, needs, gpus));
                });
            }
        });
}

/// A download under way: its progress and Cancel.
fn download_line(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    if let Some(d) = &st.preset_download {
        crate::startup::download_progress(ui, &d.files, &d.at);
        if ui.button("Cancel download").clicked() {
            out.push(Msg::CancelPresetDownload);
        }
    }
}

/// **Set up models**: the model folders, and the built-in models to add.
fn setup_window(ctx: &egui::Context, st: &AppState, w: &mut ModelWizard, out: &mut Vec<Msg>) {
    let (mut go, mut close) = (false, false);
    egui::Modal::new(egui::Id::unique("model_setup")).show(ctx, |ui| {
        ui.set_width(920.0);
        ui.heading(ic(ph::CUBE, "Set up models"));
        ui.label(
            "Where the model files are, and the built-in models to install: only the files that aren't already in \
             one of the model folders are downloaded.",
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for (t, label) in
                [(ModelSetupTab::Folders, "1. Model folders"), (ModelSetupTab::Add, "2. Add built-in models")]
            {
                if ui.selectable_label(w.tab == t, RichText::new(label).strong()).clicked() {
                    w.tab = t;
                }
            }
            ui.separator();
            if ui
                .button(ic(ph::SLIDERS_HORIZONTAL, "Model settings…"))
                .on_hover_text("each of your models' files and settings; make your own; test one on a server")
                .clicked()
            {
                out.push(Msg::OpenRecipes);
            }
        });
        ui.separator();
        let height = (ctx.content_rect().height() - 380.0).clamp(200.0, 520.0);
        // pages as tall as each other, so the window doesn't jump
        egui::ScrollArea::vertical().id_salt("model_setup_page").min_scrolled_height(height).max_height(height).show(
            ui,
            |ui| {
                ui.set_min_height(height);
                match w.tab {
                    ModelSetupTab::Folders => folders_page(ui, st, w, out),
                    ModelSetupTab::Add => add_page(ui, st, w),
                }
            },
        );
        ui.add_space(6.0);
        let files = w.downloads(&st.config.programs.model_dirs);
        let picked = w.picked.len();
        download_line(ui, st, out);
        if st.preset_download.is_none() && w.started {
            ui.colored_label(GOOD, ic(ph::CHECK_CIRCLE, "Done: installed, their files in the model folders."));
        }
        dialog_footer(
            ui,
            |ui| {
                let line = if picked > 0 {
                    let known: Option<u64> =
                        files.iter().try_fold(0u64, |n, d| Some(n + w.sizes.get(&d.url).copied().flatten()?));
                    let s = plural(picked);
                    match (files.len(), known) {
                        (0, _) => format!("{picked} model{s}, files already here"),
                        (n, Some(b)) => format!("{picked} model{s} · {n} files to download, {}", gb(b)),
                        (n, None) => format!("{picked} model{s} · {n} files to download"),
                    }
                } else if w.tab == ModelSetupTab::Folders {
                    "Next: 2. Add built-in models".to_string()
                } else {
                    "Tick models to install them.".to_string()
                };
                ui.label(RichText::new(line).color(if picked == 0 { NEUTRAL } else { ACCENT }));
            },
            |ui| {
                close = ui.button("Close   Esc").clicked();
                let label = if files.is_empty() { "Install" } else { "Download and install" };
                let can = picked > 0 && !w.dir.trim().is_empty() && st.preset_download.is_none();
                go = ui
                    .add_enabled(can, egui::Button::new(RichText::new(ic(ph::DOWNLOAD_SIMPLE, label)).strong()))
                    .clicked();
            },
        );
    });
    // Esc: not while the model settings are open over it
    let esc = st.recipe_editor.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if go {
        out.push(Msg::ModelWizardGo);
    } else if close || esc {
        w.editing = false;
    }
}

/// A heading with what it's for.
fn step(ui: &mut egui::Ui, title: &str, what: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong());
        ui.weak(format!("— {what}"));
    });
}

fn folders_page(ui: &mut egui::Ui, st: &AppState, w: &mut ModelWizard, out: &mut Vec<Msg>) {
    let model_dirs = &st.config.programs.model_dirs;
    step(ui, "Download folder", "where new model files are saved");
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut w.dir).desired_width(620.0).hint_text("~/models"));
        if ui.button(ic(ph::FOLDER_OPEN, "Browse…")).clicked() {
            out.push(Msg::Pick(PickKind::ModelWizardDir));
        }
    });
    let dir = w.dir.trim();
    let first = model_dirs.iter().find(|d| !d.trim().is_empty());
    ui.weak(if dir.is_empty() {
        "Give a folder: downloads go in it."
    } else if first.is_some_and(|f| f == dir) {
        "Already your first model folder."
    } else if model_dirs.iter().any(|d| d == dir) {
        "Already one of your model folders."
    } else {
        "Not a model folder yet: it becomes your first one (made if it isn't there) when you install models."
    });
    ui.add_space(12.0);
    step(ui, "Your model folders", "where the servers look for model files, in this order");
    let dirs: Vec<&String> = model_dirs.iter().filter(|d| !d.trim().is_empty()).collect();
    if dirs.is_empty() {
        ui.weak("None yet.");
    }
    for d in dirs {
        ui.label(ic(ph::FOLDER, d));
    }
    ui.horizontal(|ui| {
        ui.weak("To take one out or reorder them:");
        if ui.small_button(ic(ph::GEAR, "Server settings…")).clicked() {
            out.push(Msg::ServerSettings(true));
        }
    });
    ui.add_space(12.0);
    step(
        ui,
        "Folders that already have model files",
        "ticked ones become model folders when you install; their files are used where they are",
    );
    match &w.found {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Looking in the usual places…");
            });
        }
        Some(found) if found.is_empty() => {
            ui.weak(
                "None found (looked for ComfyUI's, stable-diffusion.cpp's, KoboldCpp's and ~/models). That's fine: \
                 models are downloaded into the download folder.",
            );
        }
        Some(found) => {
            for f in found.clone() {
                let already = model_dirs.iter().any(|d| d.trim() == f.path);
                let mut on = already || w.add.contains(&f.path);
                let text = format!("{}   ({} model file{}, {})", f.path, f.files, plural(f.files), gb(f.bytes));
                let r = ui.add_enabled(!already, egui::Checkbox::new(&mut on, text));
                if already {
                    r.on_disabled_hover_text("already one of your model folders");
                } else if r.changed() {
                    if on {
                        w.add.insert(f.path.clone());
                    } else {
                        w.add.remove(&f.path);
                    }
                }
            }
        }
    }
}

fn add_page(ui: &mut egui::Ui, st: &AppState, w: &mut ModelWizard) {
    use egui_extras::{Column, TableBuilder};
    let catalogue = increart_core::config::catalogue();
    let dirs = w.folders(&st.config.programs.model_dirs);
    filters(ui, w, "setup");
    gpu_line(ui, w);
    ui.add_space(4.0);
    let gpus = gpus_of(w);
    let vram = w.ticked_vram(&gpus);
    let rows: Vec<(&RecipePreset, PresetNeeds)> =
        catalogue.iter().map(|p| (p, PresetNeeds::of(p, &dirs, &w.sizes))).collect();
    let all = rows.len();
    let rows = w.shown(rows, &vram);
    if rows.len() < all {
        ui.weak(format!("{} of {all} shown", rows.len()));
    }
    if rows.is_empty() {
        ui.weak("None match the filter.");
        return;
    }
    TableBuilder::new(ui)
        .id_salt("model_setup_add")
        .striped(true)
        .vscroll(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(NAME_W + 60.0).clip(true))
        .column(Column::exact(SIZE_W))
        .column(Column::exact(FILES_W + 40.0).clip(true))
        .column(Column::remainder().at_least(100.0).clip(true))
        .header(22.0, |mut h| {
            heads(
                &mut h,
                "tick to install it; greyed: installed with its files, or it can't be downloaded",
                &mut w.sort,
            )
        })
        .body(|mut body| {
            for (p, needs) in &rows {
                let have = st.config.recipes.iter().any(|r| r.name == p.name);
                let enabled = !needs.unavailable && !(have && needs.downloads.is_empty());
                let mut on = w.picked.contains(&p.name);
                // nothing to do for it: not picked
                if !enabled && on {
                    on = false;
                    w.picked.remove(&p.name);
                }
                body.row(24.0, |mut row| {
                    row.col(|ui| {
                        let mut name = RichText::new(&p.name);
                        if have {
                            name = name.weak();
                        }
                        let r = ui.add_enabled(enabled, egui::Checkbox::new(&mut on, name));
                        let why = if needs.unavailable {
                            "its files aren't in your model folders and there's no download for them"
                        } else {
                            "installed, with its files"
                        };
                        let r = r.on_hover_text(tip(p)).on_disabled_hover_text(format!("{}\n\n{why}", tip(p)));
                        if r.changed() {
                            if on {
                                w.picked.insert(p.name.clone());
                            } else {
                                w.picked.remove(&p.name);
                            }
                        }
                    });
                    row.col(|ui| size_cell(ui, needs));
                    row.col(|ui| {
                        ui.label(status(needs, if have { Section::Installed } else { Section::Available }));
                    });
                    row.col(|ui| fits_cell(ui, needs, &gpus));
                });
            }
        });
}
