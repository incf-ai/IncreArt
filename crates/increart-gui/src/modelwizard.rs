//! **Set up models** (DESIGN.md, Setting up models): the folder the model
//! files go in, folders found with some, and the built-in presets to
//! download, each with its size and whether it fits each GPU.

use eframe::egui::{self, RichText};
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
        ui.label(
            "Gets image models onto this machine for the servers to use. Pick where downloads go, add folders \
             that already have model files (from ComfyUI, stable-diffusion.cpp, KoboldCpp…), then tick the models \
             you want: each becomes a model preset (Model presets…), and only the files that aren't already in \
             one of the folders are downloaded.",
        );
        ui.add_space(10.0);
        // 1: the folder
        step(ui, "1", "Download folder", "where new model files are saved");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut w.dir).desired_width(520.0).hint_text("~/models"));
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
        step(
            ui,
            "2",
            "Folders that already have model files",
            "ticked ones are added as model folders; their files are used where they are, not copied",
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
                    "None found (looked for ComfyUI's, stable-diffusion.cpp's, KoboldCpp's and ~/models). That's \
                     fine: the models below are downloaded into the folder above.",
                );
            }
            Some(found) => {
                for f in found.clone() {
                    let mut on = w.add.contains(&f.path);
                    let s = if f.files == 1 { "" } else { "s" };
                    let already = model_dirs.iter().any(|d| d.trim() == f.path);
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
        step(ui, "3", "Models to add", "tested built-in models; tick the ones you want");
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
        let height = (ctx.content_rect().height() - 520.0).clamp(150.0, 420.0);
        models_table(ui, st, &catalogue, &dirs, &gpus, height, &mut w);
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
                let label = if picked == 0 {
                    "Add".to_string()
                } else if files.is_empty() {
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
        .column(Column::exact(300.0).clip(true))
        .column(Column::exact(70.0))
        .column(Column::exact(250.0).clip(true))
        .column(Column::remainder().at_least(120.0).clip(true))
        .header(22.0, |mut h| {
            let heads = [
                (
                    "Model",
                    "tick to add it as a model preset; greyed: nothing to do (already set up) or can't be fetched",
                ),
                ("Size", "all its files together, about the GPU memory it needs"),
                ("Files", "whether its files are already in your model folders, or what's downloaded"),
                (
                    "Fits your GPUs",
                    "✓: fits in that GPU's memory; ✗: too large, it'd spill to RAM (slow) if it runs at all",
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
                            "already in your model presets, files there"
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
                            let t = if have { "set up (preset and files)" } else { "all there" };
                            RichText::new(ic(ph::CHECK, t)).color(GOOD)
                        } else {
                            let n = needs.downloads.len();
                            let s = if n == 1 { "" } else { "s" };
                            let size = needs.download_bytes.map_or(String::new(), |b| format!(" ({})", gb(b)));
                            let have = if have { ", preset there" } else { "" };
                            RichText::new(ic(ph::DOWNLOAD_SIMPLE, &format!("download {n} file{s}{size}{have}")))
                        };
                        ui.label(status);
                    });
                    row.col(|ui| {
                        if gpus.is_empty() {
                            ui.weak("—");
                        }
                        for g in gpus {
                            let label = if gpus.len() > 1 { format!("GPU {} ", g.number) } else { String::new() };
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
