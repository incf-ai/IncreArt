//! **Models**, the servers screen's right half (DESIGN.md, Setting up
//! models): the folder the model files go in, folders found with some, and
//! the built-in models to add, each with its size and whether it fits each
//! GPU.

use eframe::egui::{self, RichText};
use increart_gui_core::{AppState, Msg, PickKind, PresetNeeds};

use crate::widgets::{ACCENT, BAD, GOOD, NEUTRAL, ic, ph};

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1e9)
}

/// The servers screen's right half: the models, where their files go, and
/// built-in ones to add (downloading what isn't there). Opens its state
/// (folders found, GPUs, sizes) the first time it's shown.
pub fn panel(ui: &mut egui::Ui, st: &AppState, table_height: f32, out: &mut Vec<Msg>) {
    let Some(old) = &st.model_wizard else {
        out.push(Msg::OpenModelWizard);
        ui.spinner();
        return;
    };
    let mut w = old.clone();
    let mut go = false;
    let catalogue = increart_core::config::catalogue();
    let model_dirs = &st.config.programs.model_dirs;
    let dirs = w.folders(model_dirs);
    let n = st.config.recipes.len();
    egui::Sides::new().show(
        ui,
        |ui| ui.heading(ic(ph::CUBE, "Models")),
        |ui| {
            if ui
                .button(ic(ph::PENCIL_SIMPLE, &format!("Edit your models ({n})…")))
                .on_hover_text("each model's files and the settings it runs with; test one on a server")
                .clicked()
            {
                out.push(Msg::OpenRecipes);
            }
        },
    );
    ui.label(
        "The image models the servers can load: each is its files and the settings it runs with. Add built-in \
         ones below; only files that aren't already on this machine are downloaded.",
    );
    ui.add_space(10.0);
    // 1: the folder
    step(ui, "1", "Download folder", "where new model files are saved");
    ui.horizontal(|ui| {
        let width = ui.available_width() - 110.0;
        ui.add(egui::TextEdit::singleline(&mut w.dir).desired_width(width).hint_text("~/models"));
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
        "Not a model folder yet: it's added as your first one (and made if it isn't there)."
    });
    ui.add_space(10.0);
    // 2: folders found
    step(ui, "2", "Folders that already have model files", "ticked: used where they are, not copied");
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
                 models are downloaded into the folder above.",
            );
        }
        Some(found) => {
            for f in found.clone() {
                let already = model_dirs.iter().any(|d| d.trim() == f.path);
                let mut on = already || w.add.contains(&f.path);
                let s = if f.files == 1 { "" } else { "s" };
                let text = format!("{}   ({} model file{s}, {})", f.path, f.files, gb(f.bytes));
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
    ui.add_space(10.0);
    // 3: the models
    step(ui, "3", "Add built-in models", "tested on both servers; tick the ones you want");
    let gpus: Vec<increart_core::tiers::Gpu> = match &w.gpus {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Listing the GPUs…");
            });
            vec![]
        }
        Some(Err(e)) => {
            ui.weak(format!("The GPUs couldn't be listed ({e}), so there's no fit column."));
            vec![]
        }
        Some(Ok(g)) => {
            let list: Vec<String> =
                g.iter().map(|g| format!("GPU {}: {} ({:.0} GB)", g.number, g.name, g.vram_gib)).collect();
            ui.weak(list.join("  ·  "));
            g.clone()
        }
    };
    ui.add_space(4.0);
    models_table(ui, st, &catalogue, &dirs, &gpus, table_height, &mut w);
    ui.add_space(6.0);
    // what it'll do
    let files = w.downloads(model_dirs);
    let picked = w.picked.len();
    if let Some(d) = &st.preset_download {
        crate::startup::download_progress(ui, &d.files, &d.at);
        if ui.button("Cancel download").clicked() {
            out.push(Msg::CancelPresetDownload);
        }
    } else if w.started {
        ui.colored_label(GOOD, ic(ph::CHECK_CIRCLE, "Done: added to your models, their files in the folders."));
    }
    egui::Sides::new().show(
        ui,
        |ui| {
            let s = if picked == 1 { "" } else { "s" };
            let line = if picked == 0 {
                "Tick models to add them.".to_string()
            } else {
                let known: Option<u64> =
                    files.iter().try_fold(0u64, |n, d| Some(n + w.sizes.get(&d.url).copied().flatten()?));
                match (files.len(), known) {
                    (0, _) => format!("{picked} model{s}, files already here"),
                    (n, Some(b)) => format!("{picked} model{s} · {n} files to download, {}", gb(b)),
                    (n, None) => format!("{picked} model{s} · {n} files to download"),
                }
            };
            ui.label(RichText::new(line).color(if picked == 0 { NEUTRAL } else { ACCENT }));
        },
        |ui| {
            let label = if files.is_empty() { "Add to your models" } else { "Download and add" };
            let can = picked > 0 && !w.dir.trim().is_empty() && st.preset_download.is_none();
            go = ui
                .add_enabled(can, egui::Button::new(RichText::new(ic(ph::DOWNLOAD_SIMPLE, label)).strong()))
                .clicked();
        },
    );
    if &w != old {
        out.push(Msg::EditModelWizard(Box::new(w)));
    }
    if go {
        out.push(Msg::ModelWizardGo);
    }
}

/// A numbered section heading with what it's for.
fn step(ui: &mut egui::Ui, n: &str, title: &str, what: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{n}.")).strong().color(ACCENT));
        ui.label(RichText::new(title).strong());
        ui.weak(format!("— {what}"));
    });
}

/// The built-in presets: a row each with a header that stays put, at a
/// fixed height so the window doesn't jump as sizes come in.
fn models_table(
    ui: &mut egui::Ui,
    st: &AppState,
    catalogue: &[increart_core::config::RecipePreset],
    dirs: &[String],
    gpus: &[increart_core::tiers::Gpu],
    height: f32,
    w: &mut increart_gui_core::ModelWizard,
) {
    use egui_extras::{Column, TableBuilder};
    TableBuilder::new(ui)
        .id_salt("model_wizard_table")
        .striped(true)
        .vscroll(true)
        .min_scrolled_height(height)
        .max_scroll_height(height)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(210.0).clip(true))
        .column(Column::exact(62.0))
        .column(Column::exact(190.0).clip(true))
        .column(Column::remainder().at_least(120.0).clip(true))
        .header(22.0, |mut h| {
            let heads = [
                (
                    "Model",
                    "tick to add it to your models; greyed: already added with its files, or it can't be downloaded",
                ),
                ("Size", "all its files together, about the GPU memory it needs"),
                ("Files", "whether its files are already in your model folders, or what's downloaded"),
                (
                    "Fits GPU",
                    "by GPU number (listed above): ✓ fits in its memory; ✗: too large, it'd spill to RAM (slow) if it runs at all",
                ),
            ];
            for (title, tip) in heads {
                h.col(|ui| {
                    ui.strong(title).on_hover_text(tip);
                });
            }
        })
        .body(|mut body| {
            for p in catalogue {
                let needs = PresetNeeds::of(p, dirs, &w.sizes);
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
                        let tip = if p.note.is_empty() { p.recipe.clone() } else { p.note.clone() };
                        let r = ui.add_enabled(enabled, egui::Checkbox::new(&mut on, name));
                        let why = if needs.unavailable {
                            "its files aren't in your model folders and there's no download for them"
                        } else {
                            "already in your models, with its files"
                        };
                        let r = r.on_hover_text(&tip).on_disabled_hover_text(format!("{tip}\n\n{why}"));
                        if r.changed() {
                            if on {
                                w.picked.insert(p.name.clone());
                            } else {
                                w.picked.remove(&p.name);
                            }
                        }
                    });
                    row.col(|ui| {
                        match needs.total_bytes {
                            Some(b) => ui.label(gb(b)),
                            None => ui.weak("…").on_hover_text("asking Hugging Face for the sizes"),
                        };
                    });
                    row.col(|ui| {
                        let status = if needs.unavailable {
                            RichText::new("not there, no download").color(NEUTRAL)
                        } else if needs.downloads.is_empty() {
                            let t = if have { "added, files here" } else { "files here" };
                            RichText::new(ic(ph::CHECK, t)).color(GOOD)
                        } else {
                            let n = needs.downloads.len();
                            let s = if n == 1 { "" } else { "s" };
                            let size = needs.download_bytes.map_or(String::new(), |b| format!(" ({})", gb(b)));
                            let have = if have { ", added" } else { "" };
                            RichText::new(ic(ph::DOWNLOAD_SIMPLE, &format!("download {n} file{s}{size}{have}")))
                        };
                        ui.label(status);
                    });
                    row.col(|ui| {
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
                    });
                });
            }
        });
}
