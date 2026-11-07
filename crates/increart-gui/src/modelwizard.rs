//! **Set up models** (DESIGN.md, Setting up models): the folder the model
//! files go in, folders found with some, and the built-in presets to
//! download, each with its size and whether it fits each GPU.

use eframe::egui::{self, RichText, ScrollArea};
use increart_gui_core::{AppState, Msg, PickKind, PresetNeeds};

use crate::widgets::{ACCENT, BAD, GOOD, NEUTRAL, dialog_footer, ic, ph};

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1e9)
}

pub fn window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let Some(old) = &st.model_wizard else { return };
    let mut w = old.clone();
    let (mut go, mut close) = (false, false);
    let catalogue = increart_core::config::catalogue();
    let model_dirs = &st.config.programs.model_dirs;
    let dirs = w.folders(model_dirs);
    egui::Modal::new(egui::Id::unique("model_wizard")).show(ctx, |ui| {
        ui.set_width(880.0);
        ui.heading(ic(ph::CUBE, "Set up models"));
        ui.weak(
            "Where the model files go, folders that already have some, and the models to download: each becomes a \
             model preset. Both servers find their files in the model folders.",
        );
        ui.add_space(8.0);
        // the folder
        ui.label(RichText::new("Folder").strong());
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut w.dir).desired_width(520.0).hint_text("~/models"));
            if ui.small_button(ph::FOLDER_OPEN).on_hover_text("Browse…").clicked() {
                out.push(Msg::Pick(PickKind::ModelWizardDir));
            }
        });
        let dir = w.dir.trim();
        let first = model_dirs.iter().find(|d| !d.trim().is_empty());
        ui.weak(if dir.is_empty() {
            "Downloads go in this folder."
        } else if first.is_some_and(|f| f == dir) {
            "Your first model folder: downloads go here."
        } else if model_dirs.iter().any(|d| d == dir) {
            "One of your model folders: downloads go here."
        } else {
            "Downloads go here; it's added as your first model folder (made if it isn't there)."
        });
        ui.add_space(4.0);
        match &w.found {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Looking for folders with model files…");
                });
            }
            Some(found) if found.is_empty() => {
                ui.weak(
                    "No other folders with model files found (ComfyUI's, stable-diffusion.cpp's, KoboldCpp's, \
                     ~/models).",
                );
            }
            Some(found) => {
                ui.label("Found model files in (ticked: added as model folders, their files used where they are):");
                for f in found.clone() {
                    let mut on = w.add.contains(&f.path);
                    let s = if f.files == 1 { "" } else { "s" };
                    let text = format!("{} — {} file{s}, {}", f.path, f.files, gb(f.bytes));
                    if ui.checkbox(&mut on, text).changed() {
                        if on {
                            w.add.insert(f.path.clone());
                        } else {
                            w.add.remove(&f.path);
                        }
                    }
                }
            }
        }
        ui.add_space(8.0);
        // the models
        ui.label(RichText::new("Models").strong());
        let gpus: Vec<increart_core::tiers::Gpu> = match &w.gpus {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Listing the GPUs…");
                });
                vec![]
            }
            Some(Err(e)) => {
                ui.weak(format!("The GPUs couldn't be listed ({e}): sizes only."));
                vec![]
            }
            Some(Ok(g)) => {
                let list: Vec<String> =
                    g.iter().map(|g| format!("{}: {} ({:.0} GB)", g.number, g.name, g.vram_gib)).collect();
                ui.weak(format!(
                    "GPUs: {}. A model fits one when its files are a little under its memory.",
                    list.join(" · ")
                ));
                g.clone()
            }
        };
        let max = (ctx.content_rect().height() - 420.0).max(160.0);
        ScrollArea::vertical().id_salt("model_wizard_list").max_height(max).show(ui, |ui| {
            egui::Grid::new("model_wizard_grid").num_columns(4).spacing([12.0, 4.0]).striped(true).show(ui, |ui| {
                for p in &catalogue {
                    let needs = PresetNeeds::of(p, &dirs, &w.sizes);
                    let have = st.config.recipes.iter().any(|r| r.name == p.name);
                    let mut on = w.picked.contains(&p.name);
                    let mut name = RichText::new(&p.name);
                    if have {
                        name = name.weak();
                    }
                    let tip = if p.note.is_empty() { p.recipe.clone() } else { p.note.clone() };
                    let enabled = !needs.unavailable && !(have && needs.downloads.is_empty());
                    // nothing to do for it: not picked
                    if !enabled && on {
                        on = false;
                        w.picked.remove(&p.name);
                    }
                    let r = ui.add_enabled(enabled, egui::Checkbox::new(&mut on, name)).on_hover_text(&tip);
                    if r.changed() {
                        if on {
                            w.picked.insert(p.name.clone());
                        } else {
                            w.picked.remove(&p.name);
                        }
                    }
                    ui.label(match needs.total_bytes {
                        Some(b) => gb(b),
                        None => "…".into(),
                    })
                    .on_hover_text("its files together, about the memory it takes");
                    let status = if needs.unavailable {
                        RichText::new("files not there, no download").color(NEUTRAL)
                    } else if needs.downloads.is_empty() {
                        let t = if have { "in your presets, files there" } else { "files there" };
                        RichText::new(t).color(GOOD)
                    } else {
                        let n = needs.downloads.len();
                        let s = if n == 1 { "" } else { "s" };
                        let size = needs.download_bytes.map_or(String::new(), |b| format!(", {}", gb(b)));
                        let have = if have { " (in your presets)" } else { "" };
                        RichText::new(format!("{n} file{s} to download{size}{have}"))
                    };
                    ui.label(status);
                    ui.horizontal(|ui| {
                        for g in &gpus {
                            match needs.fits(g.vram_gib) {
                                Some(true) => ui.label(RichText::new(format!("{:.0} GB ✓", g.vram_gib)).color(GOOD)),
                                Some(false) => ui
                                    .label(RichText::new(format!("{:.0} GB ✗", g.vram_gib)).color(BAD))
                                    .on_hover_text("larger than this GPU's memory: offloaded to RAM, if at all"),
                                None => ui.weak("…"),
                            };
                        }
                    });
                    ui.end_row();
                }
            });
        });
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
            ui.colored_label(
                GOOD,
                ic(ph::CHECK_CIRCLE, "Done: the presets are in Model presets, their files in the folders."),
            );
        }
        dialog_footer(
            ui,
            |ui| {
                if picked > 0 {
                    let known: Option<u64> =
                        files.iter().try_fold(0u64, |n, d| Some(n + w.sizes.get(&d.url).copied().flatten()?));
                    let s = if picked == 1 { "" } else { "s" };
                    let line = match (files.len(), known) {
                        (0, _) => format!("{picked} preset{s}, their files there"),
                        (n, Some(b)) => format!("{picked} preset{s} · {n} files to download, {}", gb(b)),
                        (n, None) => format!("{picked} preset{s} · {n} files to download"),
                    };
                    ui.label(RichText::new(line).color(ACCENT));
                }
            },
            |ui| {
                let label = if files.is_empty() {
                    format!("Add {picked} preset{}", if picked == 1 { "" } else { "s" })
                } else {
                    "Download and add".to_string()
                };
                let can = picked > 0 && !w.dir.trim().is_empty() && st.preset_download.is_none();
                go = ui
                    .add_enabled(can, egui::Button::new(RichText::new(ic(ph::DOWNLOAD_SIMPLE, &label)).strong()))
                    .clicked();
                close = ui.button("Close   Esc").clicked();
            },
        );
    });
    if &w != old {
        out.push(Msg::EditModelWizard(Box::new(w)));
    }
    if go {
        out.push(Msg::ModelWizardGo);
    } else if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        out.push(Msg::CloseModelWizard);
    }
}
