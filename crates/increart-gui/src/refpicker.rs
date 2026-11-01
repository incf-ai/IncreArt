//! Generate's From library… picker: a window over nearly all of the app
//! that browses the input library and the output library like their own
//! screens (a source tree, a text filter, ratings, a thumbnail grid) to
//! choose references, tiles, or the one image Compare with… compares
//! against.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontId, Key, RichText, ScrollArea, Sense, Stroke, Vec2};
use increart_core::{IdeaLoc, ImageRef};
use increart_gui_core::{AppState, MAX_REFS, Msg, OutputSource, PickTarget, Ratings, RefSource, Smart};

use crate::textures::Textures;
use crate::widgets::{ACCENT, BAD, dialog_footer, ic, image_fit_bg, ph, short, verdict_color};

const SIDE: f32 = 250.0;
const FOOT: f32 = 40.0;

pub fn window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let (Some(p), Some(root)) = (&st.ref_picker, &st.library) else { return };
    let size = ctx.content_rect().size() * 0.95 - Vec2::splat(16.0);
    let cell_id = egui::Id::unique("ref_picker_cell");
    let pending_id = egui::Id::unique("ref_picker_pending_click");
    let mut cell = ctx.data(|d| d.get_temp::<f32>(cell_id)).unwrap_or(150.0);
    let candidates = st.ref_candidates();
    // already there: Generate's references (not again), or the tiling's
    // images (again if wanted)
    let tile = matches!(p.target, PickTarget::Tile(_));
    let compare = p.target == PickTarget::Compare;
    let sweep = p.target == PickTarget::Sweep;
    let refs: Vec<ImageRef> = match p.target {
        PickTarget::Tile(_) => st.tile.iter().flat_map(|t| t.layout.images()).cloned().collect(),
        PickTarget::Generate => st.generate.refs(),
        PickTarget::Sweep => st.generate.sweep.images.clone(),
        PickTarget::Compare => vec![],
    };
    let m = egui::Modal::new(egui::Id::unique("ref_picker")).show(ctx, |ui| {
        ui.set_width(size.x);
        ui.set_height(size.y);
        let heading = match p.target {
            PickTarget::Tile(_) => "Add images to the tiling",
            PickTarget::Generate => "Add references from the library",
            PickTarget::Sweep => "Add images to the sweep",
            PickTarget::Compare => "Compare with…",
        };
        ui.heading(ic(ph::IMAGES, heading));
        ui.separator();
        let body = size.y - (ui.cursor().min.y - ui.min_rect().top()) - FOOT - 8.0;
        // the sources on the left, the grid on the right, at full height
        let area = egui::Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), body));
        let left = egui::Rect::from_min_size(area.min, Vec2::new(SIDE, body));
        let right = egui::Rect::from_min_max(egui::pos2(left.right() + 12.0, area.top()), area.max);
        ui.allocate_rect(area, Sense::hover());
        let x = left.right() + 6.0;
        ui.painter().vline(x, area.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
        let down = egui::Layout::top_down(egui::Align::Min);
        {
            ui.scope_builder(egui::UiBuilder::new().max_rect(left).layout(down), |ui| {
                ScrollArea::vertical()
                    .id_salt("ref_picker_sources")
                    .auto_shrink([false, false])
                    .show(ui, |ui| sources(ui, st, &p.source, p.ratings, &p.target, out));
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(right).layout(down), |ui| {
                ui.horizontal(|ui| {
                    let mut text = p.text.clone();
                    let hint = match p.source {
                        RefSource::Inputs(_) | RefSource::Inbox | RefSource::Edited => "title, text or tag",
                        RefSource::Outputs(_) => "prompt text",
                        RefSource::TileInputs | RefSource::ImageInputs(_) | RefSource::ProjectInputs(_) => {
                            "title, text or prompt"
                        }
                    };
                    let r = ui.add(egui::TextEdit::singleline(&mut text).hint_text(hint).desired_width(220.0));
                    if r.changed() {
                        out.push(Msg::RefPickerText(text));
                    }
                    if matches!(p.source, RefSource::Outputs(_)) {
                        ui.separator();
                        for (label, r) in [
                            ("Kept", Ratings::KEPT),
                            ("Good", Ratings { neutral: false, ..Ratings::KEPT }),
                            ("All", Ratings::ALL),
                        ] {
                            if ui.selectable_label(p.ratings == r, label).clicked() {
                                out.push(Msg::RefPickerRatings(r));
                            }
                        }
                    }
                    ui.separator();
                    ui.weak(format!("{} image(s)", candidates.len()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Slider::new(&mut cell, 80.0..=320.0).show_value(false))
                            .on_hover_text("thumbnail size");
                        ui.label(ph::MAGNIFYING_GLASS);
                    });
                });
                ui.add_space(4.0);
                if candidates.is_empty() {
                    ui.weak(match p.source {
                        RefSource::Inputs(_) | RefSource::Inbox => "No input images here.",
                        RefSource::Edited => "No edited copies yet: Crop / mark… on an image makes one.",
                        RefSource::Outputs(_) => "No images here with these ratings.",
                        RefSource::TileInputs => "These images were made from text only.",
                        RefSource::ImageInputs(_) => "This image was made from text only.",
                        RefSource::ProjectInputs(_) => "This project's runs were made from text only.",
                    });
                }
                ScrollArea::vertical().id_salt("ref_picker_grid").auto_shrink([false, false]).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for c in &candidates {
                            let added = refs.contains(&c.image);
                            let chosen = p.chosen.iter().position(|x| *x == c.image);
                            let (rect, r) = ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                            if !ui.is_rect_visible(rect) {
                                continue;
                            }
                            let paint = ui.painter_at(rect);
                            paint.rect_filled(rect, 2.0, Color32::from_gray(35));
                            image_fit_bg(&paint, rect, tex.thumb(root, &c.image).as_ref(), true);
                            if let Some(v) = c.verdict {
                                paint.circle_filled(rect.right_top() + Vec2::new(-9.0, 9.0), 6.0, verdict_color(v));
                            }
                            // what it was to the image: start, or ref N
                            if let RefSource::ImageInputs(of) = &p.source
                                && let Some(role) = st.input_role(of, &c.image)
                            {
                                let text = role.map_or("start".to_string(), |n| format!("ref {n}"));
                                let at = rect.left_bottom() + Vec2::new(6.0, -6.0);
                                let g = paint.layout_no_wrap(text, FontId::proportional(12.0), Color32::WHITE);
                                let bg = egui::Rect::from_min_size(at - Vec2::new(0.0, g.size().y), g.size())
                                    .expand2(Vec2::new(4.0, 1.0));
                                paint.rect_filled(bg, 3.0, ACCENT);
                                paint.galley(bg.min + Vec2::new(4.0, 1.0), g, Color32::WHITE);
                            }
                            if added && tile {
                                let at = rect.right_bottom() - Vec2::splat(12.0);
                                paint.circle_filled(at, 9.0, Color32::from_black_alpha(180));
                                paint.text(
                                    at,
                                    egui::Align2::CENTER_CENTER,
                                    ph::CHECK,
                                    FontId::proportional(12.0),
                                    Color32::from_gray(220),
                                );
                            } else if added {
                                paint.rect_filled(rect, 2.0, Color32::from_black_alpha(150));
                                paint.text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    ic(ph::CHECK, "added"),
                                    FontId::proportional(14.0),
                                    Color32::from_gray(220),
                                );
                            }
                            // chosen: the <imageN> it will be
                            if let Some(i) = chosen {
                                let at = rect.left_top() + Vec2::splat(14.0);
                                paint.circle_filled(at, 11.0, ACCENT);
                                paint.text(
                                    at,
                                    egui::Align2::CENTER_CENTER,
                                    (if tile { i + 1 } else { refs.len() + i + 1 }).to_string(),
                                    FontId::proportional(13.0),
                                    Color32::WHITE,
                                );
                            }
                            let stroke = match (chosen, r.hovered()) {
                                (Some(_), _) => Stroke::new(3.0, ACCENT),
                                (None, true) => Stroke::new(1.0, Color32::GRAY),
                                _ => Stroke::NONE,
                            };
                            paint.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
                            let what = if compare {
                                "click: compare against it (A)"
                            } else if added && tile && chosen.is_none() {
                                "already in the tiling · click: choose again"
                            } else if added && sweep {
                                "already in the sweep"
                            } else if added && !tile {
                                "already a reference"
                            } else if chosen.is_some() {
                                "click: unchoose"
                            } else {
                                "click: choose"
                            };
                            let tip = format!("{}\n{}\n{what} · double-click: full size", c.title, short(&c.text, 120));
                            let r = r.on_hover_text(tip);
                            // a click chooses once it can't be the first of a
                            // double-click, which only shows full size
                            if r.double_clicked() {
                                ctx.data_mut(|d| d.remove::<(ImageRef, f64)>(pending_id));
                                out.push(Msg::Preview(Some(c.image.clone())));
                            } else if r.clicked() && (tile || !added) {
                                let now = ctx.input(|i| i.time);
                                ctx.data_mut(|d| d.insert_temp(pending_id, (c.image.clone(), now)));
                            }
                        }
                    });
                });
            });
        }
        ui.separator();
        if compare {
            dialog_footer(
                ui,
                |ui| {
                    ui.weak("Click an image: it becomes A, beside the image you are on (B).");
                },
                |ui| {
                    if ui.button("Cancel   Esc").clicked() {
                        out.push(Msg::RefPicker(false));
                    }
                },
            );
        } else {
            footer(ui, st, p.chosen.len(), refs.len(), &p.target, out);
        }
    });
    ctx.data_mut(|d| d.insert_temp(cell_id, cell));
    if let Some((image, at)) = ctx.data(|d| d.get_temp::<(ImageRef, f64)>(pending_id)) {
        let wait = ctx.options(|o| o.input_options.max_double_click_delay);
        let left = at + wait - ctx.input(|i| i.time);
        if left <= 0.0 {
            ctx.data_mut(|d| d.remove::<(ImageRef, f64)>(pending_id));
            out.push(Msg::RefPickerToggle(image));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
    }
    // keys, unless the full-size preview is over it
    if st.preview.is_none() {
        let (enter, esc) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        let typing = ctx.memory(|m| m.focused().is_some());
        if enter && !typing && !p.chosen.is_empty() {
            out.push(Msg::RefPickerAdd);
        } else if m.should_close() || esc {
            out.push(Msg::RefPicker(false));
        }
    }
}

fn footer(ui: &mut egui::Ui, st: &AppState, chosen: usize, refs: usize, target: &PickTarget, out: &mut Vec<Msg>) {
    let mut clear = vec![];
    let tile = matches!(target, PickTarget::Tile(_));
    let sweep = *target == PickTarget::Sweep;
    dialog_footer(
        ui,
        |ui| {
            let after = refs + chosen;
            let text = if tile {
                format!("{chosen} chosen · added in the order chosen")
            } else if sweep {
                format!("{chosen} chosen · {after} images in the sweep after adding, a run each")
            } else {
                format!("{chosen} chosen · {after} / {MAX_REFS} references after adding")
            };
            if !tile && !sweep && after > MAX_REFS {
                ui.colored_label(BAD, format!("{text}: more than Qwen-Image 2.1 takes"));
            } else {
                ui.weak(text);
            }
            if chosen > 0 && ui.small_button("Clear").clicked() {
                for img in st.ref_picker.iter().flat_map(|p| &p.chosen) {
                    clear.push(Msg::RefPickerToggle(img.clone()));
                }
            }
        },
        |ui| {
            let what = match target {
                PickTarget::Tile(_) => "image(s) to the tiling",
                PickTarget::Sweep => "image(s) to the sweep",
                _ => "reference(s)",
            };
            let add = egui::Button::new(RichText::new(ic(ph::PLUS, &format!("Add {chosen} {what}   Enter"))).strong());
            if ui.add_enabled(chosen > 0, add).clicked() {
                out.push(Msg::RefPickerAdd);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::RefPicker(false));
            }
        },
    );
    out.extend(clear);
}

/// The source tree: the input library and its folders; the output library
/// with its collections and smart folders.
fn sources(
    ui: &mut egui::Ui,
    st: &AppState,
    at: &RefSource,
    ratings: Ratings,
    target: &PickTarget,
    out: &mut Vec<Msg>,
) {
    let pick = |ui: &mut egui::Ui, out: &mut Vec<Msg>, s: RefSource, label: String| {
        if ui.selectable_label(*at == s, label).clicked() {
            out.push(Msg::RefPickerSource(s));
        }
    };
    let inputs_in = |folder: Option<&Path>| -> usize {
        st.index
            .ideas
            .iter()
            .filter(|e| match (&e.loc, folder) {
                (IdeaLoc::Library(_), None) => true,
                (IdeaLoc::Library(f), Some(sel)) => f.starts_with(sel),
                _ => false,
            })
            .map(|e| e.idea.images.len())
            .sum()
    };
    let outputs_in = |s: &OutputSource| st.source_images_in(s, ratings).len();

    if *target == PickTarget::Compare
        && let Some(b) = st.compare_b()
    {
        let n = st.image_inputs(&b).len();
        ui.label(RichText::new("This image").strong());
        pick(ui, out, RefSource::ImageInputs(b.clone()), ic(ph::ARROW_SQUARE_IN, &format!("Its references ({n})")));
        let project = st.output_link(&b).and_then(|o| st.index.run(&o.name.run)).map(|(p, _)| p);
        if let Some(p) = project {
            let n = st.project_inputs(&p.project.id).len();
            let label = ic(ph::ROCKET_LAUNCH, &format!("{}'s references ({n})", p.name()));
            pick(ui, out, RefSource::ProjectInputs(p.project.id.clone()), label);
        }
        ui.add_space(8.0);
    }
    if matches!(target, PickTarget::Tile(_)) {
        let n: usize = st.tile.iter().flat_map(|t| t.layout.images()).map(|i| st.image_inputs(i).len()).sum();
        ui.label(RichText::new("This tiling").strong());
        pick(ui, out, RefSource::TileInputs, ic(ph::ARROW_SQUARE_IN, &format!("Inputs of these images ({n})")));
        ui.add_space(8.0);
    }
    let inbox: usize =
        st.index.ideas.iter().filter(|e| matches!(e.loc, IdeaLoc::Unreviewed)).map(|e| e.idea.images.len()).sum();
    ui.label(RichText::new("Idea inbox").strong());
    pick(ui, out, RefSource::Inbox, ic(ph::TRAY, &format!("Inbox ({inbox})")));
    ui.add_space(8.0);
    ui.label(RichText::new("Input library").strong());
    pick(ui, out, RefSource::Inputs(None), ic(ph::BOOKS, &format!("All ideas ({})", inputs_in(None))));
    folder_tree(ui, &st.index.library_folders, Path::new(""), "ref_inputs", &mut |ui, f| {
        let n = inputs_in(Some(f));
        let name = f.file_name().unwrap_or_default().to_string_lossy();
        pick(ui, out, RefSource::Inputs(Some(f.to_path_buf())), ic(ph::FOLDER, &format!("{name} ({n})")));
    });
    let edited = st.index.ideas.iter().filter(|e| e.loc == IdeaLoc::Edited).count();
    pick(ui, out, RefSource::Edited, ic(ph::PENCIL_SIMPLE, &format!("Edited copies, hidden ({edited})")));

    ui.add_space(8.0);
    ui.label(RichText::new("Output library").strong());
    let all = OutputSource::AllProjects;
    pick(ui, out, RefSource::Outputs(all.clone()), ic(ph::IMAGES, &format!("All projects ({})", outputs_in(&all))));
    egui::CollapsingHeader::new("Collections").id_salt("ref_collections").show(ui, |ui| {
        let all = OutputSource::Collection(PathBuf::new());
        let label = ic(ph::FOLDERS, &format!("All collections ({})", outputs_in(&all)));
        pick(ui, out, RefSource::Outputs(all), label);
        folder_tree(ui, &st.index.collection_folders, Path::new(""), "ref_coll", &mut |ui, f| {
            let s = OutputSource::Collection(f.to_path_buf());
            let name = f.file_name().unwrap_or_default().to_string_lossy();
            let label = ic(ph::FOLDER, &format!("{name} ({})", outputs_in(&s)));
            pick(ui, out, RefSource::Outputs(s), label);
        });
    });
    egui::CollapsingHeader::new("by project").id_salt("ref_by_project").default_open(true).show(ui, |ui| {
        by_project(ui, st, Path::new(""), &mut |ui, s, label| pick(ui, out, RefSource::Outputs(s), label), &outputs_in);
    });
    egui::CollapsingHeader::new("by date").id_salt("ref_by_date").show(ui, |ui| {
        for (day, _) in st.kept_days() {
            let s = OutputSource::Smart(Smart::Day(day.clone()));
            let label = format!("{day} ({})", outputs_in(&s));
            pick(ui, out, RefSource::Outputs(s), label);
        }
    });
    egui::CollapsingHeader::new("by source idea").id_salt("ref_by_idea").show(ui, |ui| {
        for (id, _) in st.kept_source_ideas() {
            let title =
                st.idea(&id).map(|e| e.idea.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| id.to_string());
            let s = OutputSource::Smart(Smart::Idea(id));
            let label = format!("{} ({})", short(&title, 26), outputs_in(&s));
            pick(ui, out, RefSource::Outputs(s), label);
        }
    });
    let s = OutputSource::Smart(Smart::Transparent);
    let label = ic(ph::CHECKERBOARD, &format!("transparent ({})", outputs_in(&s)));
    pick(ui, out, RefSource::Outputs(s), label);
}

/// A folder tree (paths relative to its root), each folder a row from
/// `row`, subfolders under a caret.
fn folder_tree(
    ui: &mut egui::Ui,
    folders: &[PathBuf],
    parent: &Path,
    salt: &str,
    row: &mut dyn FnMut(&mut egui::Ui, &Path),
) {
    let mut children: Vec<&PathBuf> = folders.iter().filter(|f| f.parent() == Some(parent)).collect();
    children.sort_by_key(|f| f.to_string_lossy().to_lowercase());
    for f in children {
        if folders.iter().any(|g| g.parent() == Some(f.as_path())) {
            let id = ui.make_persistent_id((salt, f));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| row(ui, f))
                .body(|ui| folder_tree(ui, folders, f, salt, row));
        } else {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                row(ui, f);
            });
        }
    }
}

/// The project tree, as the Output library's "by project".
fn by_project(
    ui: &mut egui::Ui,
    st: &AppState,
    folder: &Path,
    pick: &mut dyn FnMut(&mut egui::Ui, OutputSource, String),
    count: &dyn Fn(&OutputSource) -> usize,
) {
    let (folders, projects) = st.kept_project_children(folder);
    for f in folders {
        let s = OutputSource::ProjectFolder(f.clone());
        let name = f.file_name().unwrap_or_default().to_string_lossy();
        let label = ic(ph::FOLDER, &format!("{name} ({})", count(&s)));
        let id = ui.make_persistent_id(("ref_by_project", f));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
            .show_header(ui, |ui| pick(ui, s, label))
            .body(|ui| by_project(ui, st, f, pick, count));
    }
    for p in projects {
        let s = OutputSource::Project(p.project.id.clone());
        let label = ic(ph::ROCKET_LAUNCH, &format!("{} ({})", p.name(), count(&s)));
        pick(ui, s, label);
    }
}
