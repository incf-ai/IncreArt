//! **Models**, the servers screen's right half (DESIGN.md, Setting up
//! models): your models and the built-in ones in three read-only lists
//! (yours, built-in installed, built-in not installed) with search, filter
//! and order; **Manage models…** (or a row) opens **Models**, a window with
//! your models' settings, the built-in models to install (downloading what
//! isn't there) and the model folders.

use eframe::egui::{self, RichText};
use increart_core::config::RecipePreset;
use increart_core::tiers::Gpu;
use increart_gui_core::{
    AppState, ModelColumn, ModelKind, ModelLists, ModelSetupTab, ModelSort, ModelWizard, Msg, PickKind, PresetNeeds,
    RecipeEditor,
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
                .button(RichText::new(ic(ph::SLIDERS_HORIZONTAL, "Manage models…")).strong())
                .on_hover_text("the built-in models to install, each model's files and settings, the model folders")
                .clicked()
            {
                // on the servers screen it opens on the built-in models
                w.editing = true;
                w.tab = ModelSetupTab::Add;
            }
        },
    );
    ui.label(
        "The image models the servers can load, each its files and the settings it runs with. Click one to open \
         it: yours to edit it, a built-in one not installed to install it.",
    );
    let dirs: Vec<&String> = model_dirs.iter().filter(|d| !d.trim().is_empty()).collect();
    ui.weak(match dirs.as_slice() {
        [] => "No model folders yet: Manage models… → Folders to choose one.".to_string(),
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
            "None yet. The built-in models below cover most needs; a custom model is for your own files.",
        ),
        (
            "Built-in models, installed",
            "models_installed",
            &lists.installed,
            Section::Installed,
            "None yet: click one below to install it.",
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
                        } else if let Some(name) = list_table(ui, id, &shown, &gpus, section, &mut w.sort) {
                            open_row(ui, st, w, &name, section, out);
                        }
                        if section == Section::Custom {
                            custom_button(ui, st, w, out);
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
) -> Option<String> {
    use egui_extras::{Column, TableBuilder};
    if rows.is_empty() {
        ui.weak("None match the filter.");
        return None;
    }
    let mut clicked = None;
    let what = if section == Section::Available { "click to install it" } else { "click to open it" };
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
                        let link = ui.add(egui::Link::new(&p.name)).on_hover_text(format!("{}\n\n{what}", tip(p)));
                        if link.clicked() {
                            clicked = Some(p.name.clone());
                        }
                    });
                    row.col(|ui| size_cell(ui, needs));
                    row.col(|ui| {
                        ui.label(status(needs, section));
                    });
                    row.col(|ui| fits_cell(ui, needs, gpus));
                });
            }
        });
    clicked
}

/// **Custom model… (advanced)**, under your custom models: a new model in
/// Your models, its files the selected one's to start from.
fn custom_button(ui: &mut egui::Ui, st: &AppState, w: &mut ModelWizard, out: &mut Vec<Msg>) {
    if ui
        .small_button(ic(ph::PLUS, "Custom model… (advanced)"))
        .on_hover_text(
            "your own files: pick the recipe and each file yourself (starting from a copy of the selected \
             model's); the built-in models cover most needs",
        )
        .clicked()
    {
        let mut e = editor_on(st, None);
        e.add_new();
        ui.memory_mut(|m| m.data.insert_temp(crate::projects::scroll_id(), true));
        out.push(Msg::EditRecipes(e));
        w.editing = true;
        w.tab = ModelSetupTab::Yours;
    }
}

/// A row of the lists clicked: yours and installed ones open in Your
/// models, one not installed in Install built-in, ticked.
fn open_row(ui: &mut egui::Ui, st: &AppState, w: &mut ModelWizard, name: &str, section: Section, out: &mut Vec<Msg>) {
    w.editing = true;
    if section == Section::Available {
        w.tab = ModelSetupTab::Add;
        let dirs = w.folders(&st.config.programs.model_dirs);
        let catalogue = increart_core::config::catalogue();
        if let Some(p) = catalogue.iter().find(|p| p.name == name)
            && !PresetNeeds::of(p, &dirs, &w.sizes).unavailable
        {
            w.picked.insert(name.to_string());
        }
        ui.memory_mut(|m| m.data.insert_temp(add_scroll_id(), name.to_string()));
    } else {
        w.tab = ModelSetupTab::Yours;
        ui.memory_mut(|m| m.data.insert_temp(crate::projects::scroll_id(), true));
        out.push(Msg::EditRecipes(editor_on(st, Some(name))));
    }
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

/// The model whose row was last clicked in the Add tab, to scroll to.
fn add_scroll_id() -> egui::Id {
    egui::Id::unique("model_add_scroll_to")
}

/// The model selected in Your models, kept for when it's opened again
/// (after Save, the editor is new).
fn selected_id() -> egui::Id {
    egui::Id::unique("models_window_selected")
}

/// The editor on `name`, the open one if it's open (keeping its changes).
fn editor_on(st: &AppState, name: Option<&str>) -> RecipeEditor {
    let mut e = st.recipe_editor.clone().unwrap_or_else(|| RecipeEditor::new(&st.config.recipes, st.generate.preset));
    if let Some(name) = name {
        e.select_named(name);
    }
    e
}

/// **Models** (DESIGN.md, Setting up models): one window, three tabs. **Your
/// models**, each model's files and settings (the editor); **Install
/// built-in**, the built-in models to add, downloading what isn't there;
/// **Folders**, where the files are. Open while the Models half says
/// (`editing`) or the editor is (`OpenRecipes`, from Generate and the
/// coverage line).
pub fn window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let editing = st.model_wizard.as_ref().is_some_and(|w| w.editing);
    if !editing && st.recipe_editor.is_none() {
        return;
    }
    let Some(old) = &st.model_wizard else {
        out.push(Msg::OpenModelWizard);
        return;
    };
    let mut w = old.clone();
    if !w.editing {
        // opened as the editor
        w.editing = true;
        w.tab = ModelSetupTab::Yours;
    }
    let mut ed = st.recipe_editor.clone();
    if w.tab == ModelSetupTab::Yours && ed.is_none() {
        let name: Option<String> = ctx.data(|d| d.get_temp(selected_id()));
        ed = Some(editor_on(st, name.as_deref()));
    }
    let (mut go, mut close, mut save) = (false, false, false);
    egui::Modal::new(egui::Id::unique("models_window")).show(ctx, |ui| {
        ui.set_width(960.0);
        ui.horizontal(|ui| {
            ui.heading(ic(ph::CUBE, "Models"));
            ui.add_space(16.0);
            for (t, label) in [
                (ModelSetupTab::Add, "Install built-in"),
                (ModelSetupTab::Yours, "Your models"),
                (ModelSetupTab::Folders, "Folders"),
            ] {
                if ui.selectable_label(w.tab == t, RichText::new(label).strong()).clicked() {
                    w.tab = t;
                }
            }
        });
        ui.weak(match w.tab {
            ModelSetupTab::Yours => {
                "Generate's Model list: what each model loads and the settings it runs with. Saved to config.rsn."
            }
            ModelSetupTab::Add => {
                "Built-in models, tested on sd-server and KoboldCpp: tick to install. Only the files that aren't \
                 already in a model folder are downloaded."
            }
            ModelSetupTab::Folders => "Where the model files are found, and where new ones are saved.",
        });
        ui.separator();
        let height = (ctx.content_rect().height() - 300.0).clamp(240.0, 640.0);
        // pages as tall as each other, so the window doesn't jump
        if let (ModelSetupTab::Yours, Some(e)) = (w.tab, &mut ed) {
            ui.vertical(|ui| {
                ui.set_min_height(height);
                // the row's item spacing after the selected model would widen the
                // window past the other tabs
                ui.set_max_width(ui.available_width() - ui.spacing().item_spacing.x);
                crate::projects::recipes_page(ui, st, e, height, out);
            });
        } else {
            egui::ScrollArea::vertical()
                .id_salt("models_window_page")
                .min_scrolled_height(height)
                .max_height(height)
                .show(ui, |ui| {
                    ui.set_min_height(height);
                    ui.set_width(ui.available_width());
                    match w.tab {
                        ModelSetupTab::Folders => folders_page(ui, st, &mut w, out),
                        _ => add_page(ui, st, &mut w),
                    }
                });
        }
        ui.add_space(6.0);
        download_line(ui, st, out);
        if st.preset_download.is_none() && w.started {
            ui.colored_label(GOOD, ic(ph::CHECK_CIRCLE, "Done: installed, their files in the model folders."));
        }
        let files = w.downloads(&st.config.programs.model_dirs);
        let picked = w.picked.len();
        match (w.tab, &mut ed) {
            (ModelSetupTab::Yours, Some(e)) => {
                let problem = e.problem();
                let changed = e.changed(&st.config.recipes);
                dialog_footer(
                    ui,
                    |ui| {
                        if ui.button("Reset to built-ins").on_hover_text("replaces the whole list").clicked() {
                            *e = RecipeEditor::new(&increart_core::config::builtin_recipes(), 0);
                            e.rows.iter_mut().for_each(|r| r.origin = None);
                        }
                        if let Some(p) = &problem {
                            ui.colored_label(BAD, p);
                        } else if changed {
                            ui.colored_label(ACCENT, "Not saved yet: Close drops the changes");
                        }
                    },
                    |ui| {
                        close = ui.button("Close   Esc").clicked();
                        let b = egui::Button::new(RichText::new(ic(ph::FLOPPY_DISK, "Save")).strong());
                        save = ui.add_enabled(problem.is_none() && changed, b).clicked();
                    },
                );
            }
            _ => {
                dialog_footer(
                    ui,
                    |ui| {
                        let line = if picked > 0 {
                            let known: Option<u64> =
                                files.iter().try_fold(0u64, |n, d| Some(n + w.sizes.get(&d.url).copied().flatten()?));
                            let s = plural(picked);
                            match (files.len(), known) {
                                (0, _) => format!("{picked} model{s} ticked, files already here"),
                                (n, Some(b)) => format!("{picked} model{s} ticked · {n} files to download, {}", gb(b)),
                                (n, None) => format!("{picked} model{s} ticked · {n} files to download"),
                            }
                        } else if w.tab == ModelSetupTab::Folders {
                            "Then: Install built-in, to add models".to_string()
                        } else {
                            "Tick models to install them.".to_string()
                        };
                        ui.label(RichText::new(line).color(if picked == 0 { NEUTRAL } else { ACCENT }));
                    },
                    |ui| {
                        close = ui.button("Close   Esc").clicked();
                        if w.tab == ModelSetupTab::Add || picked > 0 {
                            let label = if files.is_empty() { "Install" } else { "Download and install" };
                            let can = picked > 0 && !w.dir.trim().is_empty() && st.preset_download.is_none();
                            go = ui
                                .add_enabled(
                                    can,
                                    egui::Button::new(RichText::new(ic(ph::DOWNLOAD_SIMPLE, label)).strong()),
                                )
                                .clicked();
                        }
                    },
                );
            }
        }
    });
    if let Some(e) = &ed {
        let name = e.rows.get(e.selected).map(|r| r.preset.name.clone()).unwrap_or_default();
        ctx.data_mut(|d| d.insert_temp(selected_id(), name));
    }
    let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if close || esc {
        out.push(Msg::CloseModelWizard);
        if st.recipe_editor.is_some() {
            out.push(Msg::CloseRecipes);
        }
        return;
    }
    if &w != old {
        out.push(Msg::EditModelWizard(Box::new(w)));
    }
    if let Some(e) = ed
        && st.recipe_editor.as_ref() != Some(&e)
    {
        out.push(Msg::EditRecipes(e));
    }
    if save {
        out.push(Msg::SaveRecipes);
    }
    if go {
        out.push(Msg::ModelWizardGo);
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
                        let to: Option<String> = ui.memory(|m| m.data.get_temp(add_scroll_id()));
                        if to.as_deref() == Some(p.name.as_str()) {
                            r.scroll_to_me(Some(egui::Align::Center));
                            ui.memory_mut(|m| m.data.remove::<String>(add_scroll_id()));
                        }
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
