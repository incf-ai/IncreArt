//! Picking several images (Review, the Output library's Grid) and the Tile
//! window that lays them out (DESIGN.md, Tiling).

use eframe::egui::{self, Align2, Color32, FontId, Key, Rect, RichText, Sense, Stroke, Vec2};
use increart_core::config::{TextSize, TileFormat, TileSettings};
use increart_core::{ImageRef, Verdict};
use std::path::PathBuf;
use std::sync::Arc;

use increart_gui_core::tile::{Geometry, NO_IMAGES, Tile, TileContent, TileImage, TileLayout, caption_fit, text_lines};
use increart_gui_core::{AppState, CaptionFill, Msg, PickKind, SelectHow, TileDialog, TileTarget};

use crate::textures::Textures;
use crate::widgets::{ACCENT, BAD, GOOD, NEUTRAL, checkerboard, dialog_buttons, ic, image_cover, image_fit_bg, ph};

/// The empty space of a pickable view: a click there clears the picks, a
/// drag draws the rubber band. Register it before the images (it covers
/// the space left, under what comes after it).
pub fn background(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) -> egui::Response {
    let bg = ui.interact(ui.available_rect_before_wrap(), ui.scope_id().with("pick_bg"), Sense::click_and_drag());
    if bg.clicked() && !st.picked.is_empty() {
        out.push(Msg::PickSet(vec![]));
    }
    bg
}

/// An image's click: with Ctrl or Shift it picks (true: the view's own
/// click is skipped); a plain click drops the picks.
pub fn click(ui: &egui::Ui, st: &AppState, resp: &egui::Response, image: &ImageRef, out: &mut Vec<Msg>) -> bool {
    if !resp.clicked() {
        return false;
    }
    let m = ui.input(|i| i.modifiers);
    let how = if m.shift {
        SelectHow::Range
    } else if m.command {
        SelectHow::Toggle
    } else {
        if !st.picked.is_empty() {
            out.push(Msg::PickImage(image.clone(), SelectHow::Replace));
        }
        return false;
    };
    out.push(Msg::PickImage(image.clone(), how));
    true
}

/// Whether the cursor is drawn selected: not while anything is picked (the
/// picks are the selection then).
pub fn cursor(st: &AppState, on_it: bool) -> bool {
    on_it && st.picked.is_empty()
}

/// A picked image: outlined, with its number in the pick order.
pub fn mark(ui: &egui::Ui, st: &AppState, rect: Rect, image: &ImageRef) {
    let Some(n) = st.picked.iter().position(|i| i == image) else { return };
    mark_n(ui, rect, n);
}

/// Dragging on empty space picks what the band touches, in the view's
/// order (Ctrl adds to the picks).
pub fn band(ui: &egui::Ui, st: &AppState, bg: &egui::Response, rects: &[(ImageRef, Rect)], out: &mut Vec<Msg>) {
    // Ctrl adds, starting from the cursor as Ctrl+click does; else the
    // band replaces the picks (and the cursor's selection, see `cursor`)
    let base = |add: bool| match add {
        true if st.picked.is_empty() => st.current_image().into_iter().collect(),
        true => st.picked.clone(),
        false => vec![],
    };
    if let Some(picked) = band_of(ui, bg, &st.picked, rects, base) {
        out.push(Msg::PickSet(picked));
    }
}

/// The rubber band over `bg`, for any view: the picks it makes when they
/// differ from `now`; `base` gives what it starts from (true: Ctrl held,
/// adding).
pub fn band_of<T: Clone + PartialEq + Send + Sync + 'static>(
    ui: &egui::Ui,
    bg: &egui::Response,
    now: &[T],
    rects: &[(T, Rect)],
    base: impl FnOnce(bool) -> Vec<T>,
) -> Option<Vec<T>> {
    type Band<T> = (egui::Pos2, Vec<T>);
    let id = bg.id.with("band");
    if bg.drag_started()
        && let Some(origin) = bg.interact_pointer_pos()
    {
        let base = base(ui.input(|i| i.modifiers.command));
        ui.ctx().data_mut(|d| d.insert_temp::<Band<T>>(id, (origin, base)));
    }
    if bg.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Band<T>>(id));
        return None;
    }
    let (Some((origin, base)), Some(pos)) =
        (ui.ctx().data(|d| d.get_temp::<Band<T>>(id)), ui.ctx().pointer_latest_pos())
    else {
        return None;
    };
    if !bg.dragged() {
        return None;
    }
    let r = Rect::from_two_pos(origin, pos);
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, id.with("paint")));
    p.rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(90, 160, 255, 40));
    p.rect_stroke(r, 2.0, Stroke::new(1.0, ACCENT), egui::StrokeKind::Inside);
    let mut picked = base;
    for (x, _) in rects.iter().filter(|(_, x)| x.intersects(r)) {
        if !picked.contains(x) {
            picked.push(x.clone());
        }
    }
    (picked != now).then_some(picked)
}

/// A picked thing's outline and its number in the pick order (`n`, from 0).
pub fn mark_n(ui: &egui::Ui, rect: Rect, n: usize) {
    let p = ui.painter_at(rect.expand(2.0));
    p.rect_stroke(rect, 2.0, Stroke::new(3.0, ACCENT), egui::StrokeKind::Inside);
    let c = rect.left_top() + Vec2::splat(12.0);
    p.circle_filled(c, 10.0, ACCENT);
    p.text(c, Align2::CENTER_CENTER, (n + 1).to_string(), FontId::proportional(12.0), Color32::BLACK);
}

/// The top of a pickable image's right-click menu: for a picked image,
/// what to do with all the picks; else picking it. Ends with a separator.
pub fn menu(ui: &mut egui::Ui, st: &AppState, image: &ImageRef, out: &mut Vec<Msg>) {
    if !st.picked.contains(image) {
        if ui.button(ic(ph::CHECK_SQUARE, "Pick")).on_hover_text("also Ctrl+click").clicked() {
            let mut picked = st.picked.clone();
            picked.push(image.clone());
            out.push(Msg::PickSet(picked));
            ui.close();
        }
        ui.separator();
        return;
    }
    let n = st.picked.len();
    ui.weak(format!("the {n} picked"));
    ui.horizontal(|ui| {
        for (label, v, c) in
            [("Good", Verdict::Good, GOOD), ("Neutral", Verdict::Neutral, NEUTRAL), ("Bad", Verdict::Bad, BAD)]
        {
            if ui.button(RichText::new(label).color(c)).clicked() {
                out.push(Msg::RatePicked(v));
                ui.close();
            }
        }
    });
    if ui.button(ic(ph::SQUARES_FOUR, &format!("Tile the {n} picked…"))).clicked() {
        out.push(Msg::OpenTile);
        ui.close();
    }
    if new_sweep_button(ui, st, out) {
        ui.close();
    }
    crate::review::collection_menu(ui, st, &st.picked, out);
    if ui.button(ic(ph::EXPORT, &format!("Export the {n} picked…"))).clicked() {
        out.push(Msg::OpenExport(st.picked.clone(), format!("{n} images")));
        ui.close();
    }
    if ui.button(ic(ph::X, "Unpick this one")).clicked() {
        out.push(Msg::PickSet(st.picked.iter().filter(|i| *i != image).cloned().collect()));
        ui.close();
    }
    ui.separator();
    ui.weak("this image");
}

/// Ctrl+A picks everything shown, Esc drops the picks, T tiles them; with
/// picks, 1/2/3 (G/N/B) rate them all. True when the rating keys were
/// taken.
pub fn keys(ui: &egui::Ui, st: &AppState, out: &mut Vec<Msg>) -> bool {
    if ui.ctx().egui_wants_keyboard_input() || st.preview.is_some() || st.tile.is_some() {
        return false;
    }
    ui.input(|i| {
        if i.modifiers.command && i.key_pressed(Key::A) {
            out.push(Msg::PickSet(st.pickable()));
        }
        if st.picked.is_empty() {
            return false;
        }
        if i.key_pressed(Key::Escape) {
            out.push(Msg::PickSet(vec![]));
        }
        if i.key_pressed(Key::T) && !i.modifiers.command {
            out.push(Msg::OpenTile);
        }
        for (keys, v) in [
            ([Key::Num1, Key::G], Verdict::Good),
            ([Key::Num2, Key::N], Verdict::Neutral),
            ([Key::Num3, Key::B], Verdict::Bad),
        ] {
            if keys.iter().any(|k| i.key_pressed(*k)) {
                out.push(Msg::RatePicked(v));
            }
        }
        true
    })
}

/// "3 picked · Good Neutral Bad · Tile… · Clear", while anything is picked.
pub fn bar(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    if st.picked.is_empty() {
        return;
    }
    egui::Panel::top("picked_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} picked", st.picked.len())).color(ACCENT).strong());
            ui.weak("rate them:");
            for (label, v, c) in
                [("Good", Verdict::Good, GOOD), ("Neutral", Verdict::Neutral, NEUTRAL), ("Bad", Verdict::Bad, BAD)]
            {
                if ui.button(RichText::new(label).color(c)).clicked() {
                    out.push(Msg::RatePicked(v));
                }
            }
            ui.separator();
            if ui.button(ic(ph::SQUARES_FOUR, "Tile…")).on_hover_text("lay them out as one image (T)").clicked() {
                out.push(Msg::OpenTile);
            }
            new_sweep_button(ui, st, out);
            if ui.button(ic(ph::X, "Clear")).on_hover_text("Esc").clicked() {
                out.push(Msg::PickSet(vec![]));
            }
            ui.weak("Ctrl+click: pick · Shift+click: range · drag: box · Ctrl+A: all");
        });
    });
}

/// In Review's sweep scope: **New sweep from their K styles** (or values),
/// the picked images' runs swept again in Generate (the picks bar and the
/// picked images' menu). Whether it was clicked.
fn new_sweep_button(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) -> bool {
    let Some(sweep) = st.review_sweep.as_ref().filter(|_| st.tab == increart_gui_core::Tab::Review) else {
        return false;
    };
    let mut runs: Vec<&increart_core::model::Run> = vec![];
    for i in &st.picked {
        let run = st.output_link(i).and_then(|o| st.index.run(&o.name.run)).map(|(_, r)| r);
        if let Some(r) = run.filter(|r| r.sweep.as_ref().is_some_and(|s| &s.name == sweep))
            && !runs.iter().any(|x| x.id == r.id)
        {
            runs.push(r);
        }
    }
    if runs.is_empty() {
        return false;
    }
    let setting = runs[0].sweep.as_ref().is_some_and(|s| s.param.is_some());
    let what = match (runs.len(), setting) {
        (1, false) => "1 style".to_string(),
        (1, true) => "1 value".to_string(),
        (n, false) => format!("{n} styles"),
        (n, true) => format!("{n} values"),
    };
    let names: Vec<String> = runs.iter().filter_map(|r| r.sweep.as_ref()).map(|s| s.row()).collect();
    let tip = format!(
        "{sweep} again with only these, in Generate (to change and queue): {}\n\
         its template, values and settings as it was queued, the next label and seeds",
        names.join(", ")
    );
    let clicked = ui.button(ic(ph::STACK, &format!("New sweep from their {what}…"))).on_hover_text(tip).clicked();
    if clicked {
        out.push(Msg::SheetNewSweep);
    }
    clicked
}

// ---------------------------------------------------------------- the window

const BACKGROUNDS: [(&str, [u8; 4]); 4] = [
    ("transparent", [0, 0, 0, 0]),
    ("dark", [30, 30, 30, 255]),
    ("white", [255, 255, 255, 255]),
    ("black", [0, 0, 0, 255]),
];

pub fn window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let (Some(d), Some(root)) = (&st.tile, st.library.clone()) else { return };
    let n = d.layout.tiles.iter().filter(|t| !matches!(t.content, TileContent::Text { .. })).count();
    // a modal (the rest dims), as large as fits; only Close or Esc closes
    // it (a click outside would lose the layout)
    let size = egui::vec2(1100.0, 760.0).min(ctx.content_rect().size() * 0.95 - egui::Vec2::splat(16.0));
    let m = egui::Modal::new(egui::Id::unique("tile_window")).show(ctx, |ui| {
        ui.set_width(size.x);
        ui.set_height(size.y);
        ui.heading(ic(ph::SQUARES_FOUR, &format!("Tile {n} images")));
        egui::Panel::right("tile_settings").default_size(290.0).resizable(false).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                settings(ui, st, d, &root, tex, out);
            });
        });
        egui::Panel::bottom("tile_actions").show(ui, |ui| {
            ui.add_space(4.0);
            dialog_buttons(ui, |ui| {
                let idle = d.working == 0;
                if ui.add_enabled(idle, egui::Button::new(ic(ph::COPY, "Copy"))).clicked() {
                    out.push(Msg::TileOut(TileTarget::Clipboard));
                }
                let format = st.config.tile.save_format.usable();
                let save = egui::Button::new(ic(ph::FLOPPY_DISK, &format!("Save {}…", format.name())));
                if ui.add_enabled(idle, save).on_hover_text("the format is set under Save in the settings").clicked() {
                    out.push(Msg::Pick(PickKind::TileFile(format)));
                }
                let lib = egui::Button::new(ic(ph::BOOKS, "Add to input library"));
                if ui.add_enabled(idle, lib).on_hover_text("as a new idea, made of these images").clicked() {
                    out.push(Msg::TileOut(TileTarget::Library));
                }
                if ui.button("Close   Esc").clicked() {
                    out.push(Msg::CloseTile);
                }
                if !idle {
                    ui.spinner();
                    ui.weak("composing…");
                }
            });
            ui.add_space(2.0);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button(ic(ph::IMAGES, "+ Images…"))
                    .on_hover_text("from the libraries, after the last tile")
                    .clicked()
                {
                    out.push(Msg::TilePicker(None));
                }
                let inputs = d.layout.images().any(|i| !st.image_inputs(i).is_empty());
                ui.add_enabled_ui(inputs, |ui| {
                    ui.menu_button(ic(ph::ARROW_SQUARE_IN, &format!("+ Inputs {}", ph::CARET_DOWN)), |ui| {
                        if ui.button("as a column").on_hover_text("each row's inputs at its left").clicked() {
                            out.push(Msg::TileInputs(true));
                            ui.close();
                        }
                        if ui.button("before each image").on_hover_text("input → output pairs").clicked() {
                            out.push(Msg::TileInputs(false));
                            ui.close();
                        }
                    })
                })
                .response
                .on_hover_text("the images these were made from (start image and references)")
                .on_disabled_hover_text("these images were made from text only");
                if ui
                    .button(ic(ph::CLIPBOARD, "Paste"))
                    .on_hover_text("an image (or an image file) from the clipboard, after the last tile   Ctrl+V")
                    .clicked()
                {
                    paste(None, out);
                }
            });
            ui.weak(
                "Drag a tile onto another to swap them, or onto an empty cell; click one to edit it. \
                     Right-click an empty cell to add text or images there; paste (Ctrl+V) or drop image files \
                     on a cell.",
            );
            preview(ui, st, d, &root, tex, out);
        });
    });
    let esc = || ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if m.is_top_modal && !m.any_popup_open && esc() {
        out.push(Msg::CloseTile);
    }
}

/// The largest image's size (the cell), as the runs asked for.
fn largest(st: &AppState, l: &TileLayout, root: &std::path::Path, tex: &mut Textures) -> (u32, u32) {
    // as composing: the outputs size the cells, else the other images
    let outputs: Vec<(u32, u32)> = l.images().filter_map(|i| st.output_size(i)).collect();
    let sizes = if !outputs.is_empty() {
        outputs
    } else {
        l.tiles
            .iter()
            .filter_map(|t| match &t.content {
                TileContent::Image(i) => tex.dims(root, i).map(|[w, h]| (w, h)),
                TileContent::Pasted(p) => Some(p.0.dimensions()),
                TileContent::Text { .. } => None,
            })
            .collect()
    };
    sizes
        .into_iter()
        .fold(None, |a: Option<(u32, u32)>, (w, h)| Some(a.map_or((w, h), |(aw, ah)| (aw.max(w), ah.max(h)))))
        .unwrap_or(NO_IMAGES)
}

fn color(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// Text as composing lays it out (`text_lines` at `px` output pixels, `w`
/// wide), centred both ways in `r`, at `k` points per output pixel.
fn paint_text(p: &egui::Painter, r: Rect, text: &str, (px, w): (u32, u32), k: f32, c: Color32) {
    let font = FontId::proportional((px as f32 * k).max(4.0));
    let galleys: Vec<_> =
        text_lines(text, px, w).into_iter().map(|line| p.layout_no_wrap(line, font.clone(), c)).collect();
    let height: f32 = galleys.iter().map(|g| g.size().y).sum();
    let mut y = r.top() + ((r.height() - height) / 2.0).max(0.0);
    let p = p.with_clip_rect(r.intersect(p.clip_rect()));
    for g in galleys {
        let h = g.size().y;
        p.galley(egui::pos2(r.center().x - g.size().x / 2.0, y), g, c);
        y += h;
    }
}

/// Whether no tile is being dragged.
fn held_none(ui: &egui::Ui, dragging: egui::Id) -> bool {
    ui.ctx().data(|m| m.get_temp::<usize>(dragging)).is_none()
}

/// The layout at the window's scale: drag tiles between cells; an extra
/// row below takes drops; Delete (or Backspace) removes the selected tile.
fn preview(
    ui: &mut egui::Ui,
    st: &AppState,
    d: &TileDialog,
    root: &std::path::Path,
    tex: &mut Textures,
    out: &mut Vec<Msg>,
) {
    let s = &st.config.tile;
    let l = &d.layout;
    if let Some(i) = d.selected
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|x| x.key_pressed(Key::Delete) || x.key_pressed(Key::Backspace))
    {
        out.push(Msg::TileRemove(i));
    }
    let g = Geometry::new(l, s, largest(st, l, root, tex));
    // one more row to drop into
    let rows = l.rows() + 1;
    let full = (g.size.0 as f32, (g.size.1 + g.cell.1 + g.caption + g.gap) as f32);
    let avail = ui.available_size() - Vec2::splat(8.0);
    let k = (avail.x / full.0).min(avail.y / full.1).min(1.0);
    let (area, _) = ui.allocate_exact_size(Vec2::new(full.0, full.1) * k, Sense::hover());
    let p = ui.painter_at(area);
    let out_rect = Rect::from_min_size(area.min, Vec2::new(g.size.0 as f32, g.size.1 as f32) * k);
    if s.background[3] == 0 {
        checkerboard(&p, out_rect);
    } else {
        p.rect_filled(out_rect, 0.0, color(s.background));
    }
    let to_screen = |(x, y, w, h): (u32, u32, u32, u32)| {
        Rect::from_min_size(area.min + Vec2::new(x as f32, y as f32) * k, Vec2::new(w as f32, h as f32) * k)
    };
    let cell_rect = |c: u32, r: u32| to_screen(g.cell_rect(c, r));
    let [tr, sr] = g.title_rects();
    for (text, px, r) in [(&l.title, g.title_px, tr), (&l.subtitle, g.title_px / 2, sr)] {
        if r.3 > 0 {
            paint_text(&p, to_screen(r), text, (px, r.2), k, color(s.caption_color));
        }
    }
    // empty cells: right-click (or double-click) adds a text block
    for r in 0..rows {
        for c in 0..l.cols {
            if l.tile_at(c, r).is_some() {
                continue;
            }
            let rect = cell_rect(c, r);
            let resp = ui.interact(rect, egui::Id::unique(("tile_cell", c, r)), Sense::click());
            let gray = if resp.hovered() { 140 } else { 90 };
            p.rect_stroke(rect.shrink(1.0), 2.0, Stroke::new(1.0, Color32::from_gray(gray)), egui::StrokeKind::Inside);
            if resp.double_clicked() {
                out.push(Msg::TileAddText(c, r));
            } else if resp.clicked() {
                out.push(Msg::TileSelect(None));
            }
            resp.context_menu(|ui| {
                if ui.button(ic(ph::TEXT_T, "Add text here")).clicked() {
                    out.push(Msg::TileAddText(c, r));
                    ui.close();
                }
                if ui.button(ic(ph::IMAGES, "Add images…")).on_hover_text("from the libraries, from here on").clicked()
                {
                    out.push(Msg::TilePicker(Some((c, r))));
                    ui.close();
                }
                if ui.button(ic(ph::CLIPBOARD, "Paste image here   Ctrl+V")).clicked() {
                    paste(Some((c, r)), out);
                    ui.close();
                }
            });
        }
    }
    let dragging = egui::Id::unique("tile_drag");
    let mut dropped = None;
    for (i, t) in l.tiles.iter().enumerate() {
        let r = to_screen(g.image_rect(t));
        let resp = ui.interact(r, egui::Id::unique(("tile", i)), Sense::click_and_drag());
        match &t.content {
            TileContent::Text { text, .. } => {
                paint_text(&p, r, text, (g.text_px(t), g.image_rect(t).2), k, color(s.caption_color));
                p.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.0, Color32::from_gray(70)), egui::StrokeKind::Inside);
            }
            TileContent::Image(_) | TileContent::Pasted(_) => {
                let tx = match &t.content {
                    TileContent::Pasted(img) => Some(pasted_texture(ui.ctx(), img)),
                    TileContent::Image(image) => tex.thumb(root, image),
                    TileContent::Text { .. } => None,
                };
                if s.fill {
                    image_cover(&p, r, tx.as_ref());
                } else {
                    image_fit_bg(&p, r, tx.as_ref(), false);
                    if let Some(tx) = &tx {
                        // the letterbox shows the background, as in the output
                        let sz = tx.size_vec2();
                        let f = (r.width() / sz.x).min(r.height() / sz.y);
                        let img = Rect::from_center_size(r.center(), sz * f);
                        for side in [
                            Rect::from_min_max(r.min, egui::pos2(r.max.x, img.min.y)),
                            Rect::from_min_max(egui::pos2(r.min.x, img.max.y), r.max),
                            Rect::from_min_max(egui::pos2(r.min.x, img.min.y), egui::pos2(img.min.x, img.max.y)),
                            Rect::from_min_max(egui::pos2(img.max.x, img.min.y), egui::pos2(r.max.x, img.max.y)),
                        ] {
                            if s.background[3] == 0 {
                                checkerboard(&p, side);
                            } else {
                                p.rect_filled(side, 0.0, color(s.background));
                            }
                        }
                    }
                }
            }
        }
        if g.caption > 0 && !t.caption.trim().is_empty() {
            let strip = Rect::from_min_size(r.left_bottom(), Vec2::new(r.width(), g.caption as f32 * k));
            let caption = st.caption_text(t.caption.trim(), t.image(), i + 1);
            let (caption, px) = caption_fit(&caption, g.image_rect(t).2, g.caption_px);
            let font = FontId::proportional((px as f32 * k).max(4.0));
            let text = ui.painter().layout_no_wrap(caption, font, color(s.caption_color));
            p.with_clip_rect(strip).galley(strip.center() - text.size() / 2.0, text, color(s.caption_color));
        }
        let sel = d.selected == Some(i);
        let stroke = if sel {
            Stroke::new(3.0, ACCENT)
        } else if resp.hovered() {
            Stroke::new(1.0, Color32::GRAY)
        } else {
            Stroke::NONE
        };
        p.rect_stroke(r, 2.0, stroke, egui::StrokeKind::Inside);
        // × in the corner, while hovered or selected: out of the tiling
        // (the image itself stays where it is)
        let x = Rect::from_min_size(r.right_top() + Vec2::new(-26.0, 4.0), Vec2::splat(22.0));
        let mut removed = false;
        if (sel || ui.rect_contains_pointer(r)) && r.width() > 40.0 && held_none(ui, dragging) {
            let b = ui.interact(x, egui::Id::unique(("tile_x", i)), Sense::click());
            let fill = if b.hovered() { Color32::from_rgb(170, 50, 50) } else { Color32::from_black_alpha(170) };
            p.rect_filled(x, 4.0, fill);
            p.text(x.center(), Align2::CENTER_CENTER, ph::X, FontId::proportional(14.0), Color32::WHITE);
            if b.on_hover_text("Remove from the tiling   Delete").clicked() {
                out.push(Msg::TileRemove(i));
                removed = true;
            }
        }
        if resp.clicked() && !removed {
            out.push(Msg::TileSelect(Some(i)));
        }
        resp.context_menu(|ui| tile_menu(ui, st, d, &g, i, out));
        if resp.drag_started() {
            ui.ctx().data_mut(|m| m.insert_temp(dragging, i));
        }
        if resp.drag_stopped() {
            dropped = Some(i);
        }
    }
    // pasted (Ctrl+V) or dropped files: at the cell under the pointer
    let pos = ui.ctx().pointer_latest_pos().filter(|p| area.contains(*p));
    let under = pos.and_then(|pos| {
        (0..rows).flat_map(|r| (0..l.cols).map(move |c| (c, r))).find(|&(c, r)| cell_rect(c, r).contains(pos))
    });
    let (events, hovering, dropped_files) =
        ui.ctx().input(|i| (i.events.clone(), !i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone()));
    if hovering && let Some((c, r)) = under {
        p.rect_stroke(cell_rect(c, r), 2.0, Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
    }
    let picker = st.ref_picker.is_some() || ui.ctx().memory(|m| m.focused().is_some());
    if !picker {
        for e in events {
            match e {
                egui::Event::PasteImage(img) => {
                    let rgba = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
                    let [w, h] = img.size;
                    if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
                        out.push(Msg::TileAdd(vec![TileContent::Pasted(TileImage(Arc::new(buf)))], under));
                    }
                }
                // text: image files' paths (copied in a file manager)
                egui::Event::Paste(text) => {
                    add_files(paths_in(&text), under, out);
                }
                _ => {}
            }
        }
    }
    let files: Vec<PathBuf> = dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| p.is_file()).collect();
    if !files.is_empty() && pos.is_some() {
        add_files(files, under, out);
    }
    // the tile being dragged follows the pointer
    let held = ui.ctx().data(|m| m.get_temp::<usize>(dragging));
    if let (Some(i), Some(pos)) = (held, ui.ctx().pointer_latest_pos()) {
        let cell =
            (0..rows).flat_map(|r| (0..l.cols).map(move |c| (c, r))).find(|&(c, r)| cell_rect(c, r).contains(pos));
        if let Some((c, r)) = cell {
            p.rect_stroke(cell_rect(c, r), 2.0, Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
        }
        if let Some(t) = l.tiles.get(i) {
            let ghost = Rect::from_center_size(pos, Vec2::splat(64.0));
            let fg = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, dragging));
            match &t.content {
                TileContent::Image(image) => image_fit_bg(&fg, ghost, tex.thumb(root, image).as_ref(), false),
                TileContent::Pasted(img) => image_fit_bg(&fg, ghost, Some(&pasted_texture(ui.ctx(), img)), false),
                TileContent::Text { text, .. } => {
                    fg.rect_filled(ghost, 2.0, Color32::from_gray(40));
                    paint_text(&fg, ghost, text, (11, 64), 1.0, Color32::from_gray(220));
                }
            }
        }
        if dropped.is_some() {
            ui.ctx().data_mut(|m| m.remove::<usize>(dragging));
            if let Some((c, r)) = cell
                && l.tiles.get(i).is_some_and(|t| (t.col, t.row) != (c, r))
            {
                out.push(Msg::TileMove(i, c, r));
            }
        }
    }
}

/// The clipboard's image into the tiling (at a cell): image data, else
/// image files' paths or `file://` URIs as text.
fn paste(at: Option<(u32, u32)>, out: &mut Vec<Msg>) {
    let mut cb = match arboard::Clipboard::new() {
        Ok(cb) => cb,
        Err(e) => return out.push(Msg::Problem(format!("Clipboard: {e}"))),
    };
    if let Ok(img) = cb.get_image()
        && let Some(buf) = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
    {
        return out.push(Msg::TileAdd(vec![TileContent::Pasted(TileImage(Arc::new(buf)))], at));
    }
    let files = paths_in(&cb.get_text().unwrap_or_default());
    if files.is_empty() {
        out.push(Msg::Info("No image on the clipboard".into()));
    }
    add_files(files, at, out);
}

/// Paths in pasted text, one a line (`file://` URIs decoded); only files.
fn paths_in(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| match l.strip_prefix("file://") {
            Some(rest) => PathBuf::from(percent_decode(rest)),
            None => PathBuf::from(l),
        })
        .filter(|p| p.is_absolute() && p.is_file())
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut v = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (b[i], b.get(i + 1).and_then(|&c| hex(c)), b.get(i + 2).and_then(|&c| hex(c))) {
            (b'%', Some(h), Some(l)) => {
                v.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, _, _) => {
                v.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&v).into_owned()
}

/// Image files read into the tiling; those that aren't images are named.
fn add_files(files: Vec<PathBuf>, at: Option<(u32, u32)>, out: &mut Vec<Msg>) {
    let mut images = vec![];
    for f in files {
        match image::open(&f) {
            Ok(i) => images.push(TileContent::Pasted(TileImage(Arc::new(i.to_rgba8())))),
            Err(e) => out.push(Msg::Problem(format!("{}: {e}", f.display()))),
        }
    }
    if !images.is_empty() {
        out.push(Msg::TileAdd(images, at));
    }
}

/// A pasted image's texture, made once (at most 1024 px).
fn pasted_texture(ctx: &egui::Context, img: &TileImage) -> egui::TextureHandle {
    let id = egui::Id::unique("tile_pasted").with(Arc::as_ptr(&img.0) as usize);
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let k = (1024.0 / img.0.width().max(img.0.height()) as f32).min(1.0);
    let (w, h) = ((img.0.width() as f32 * k).max(1.0) as u32, (img.0.height() as f32 * k).max(1.0) as u32);
    let small = image::imageops::thumbnail(&*img.0, w, h);
    let ci =
        egui::ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw());
    let t = ctx.load_texture("tile_pasted", ci, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// A tile's right-click menu: caption (an image's) or text, span, remove.
fn tile_menu(ui: &mut egui::Ui, st: &AppState, d: &TileDialog, g: &Geometry, i: usize, out: &mut Vec<Msg>) {
    let Some(t) = d.layout.tiles.get(i) else { return };
    match &t.content {
        TileContent::Image(_) | TileContent::Pasted(_) => {
            let mut c = t.caption.clone();
            let r =
                ui.add(egui::TextEdit::singleline(&mut c).hint_text("caption, e.g. cfg {cfg}").desired_width(200.0));
            if r.changed() {
                out.push(Msg::TileCaption(i, c));
            }
            if r.lost_focus() && ui.input(|x| x.key_pressed(Key::Enter)) {
                ui.close();
            }
        }
        TileContent::Text { .. } => text_edit(ui, t, g, i, 200.0, out),
    }
    ui.horizontal(|ui| {
        ui.weak("span");
        for (w, h) in [(1, 1), (2, 1), (1, 2), (2, 2), (3, 3)] {
            let fits = w <= d.layout.cols;
            if ui.add_enabled(fits, egui::Button::selectable((t.w, t.h) == (w, h), format!("{w}×{h}"))).clicked() {
                out.push(Msg::TileSpan(i, w, h));
                ui.close();
            }
        }
    });
    if let Some(image) = t.image() {
        let n = st.image_inputs(image).len();
        let add = egui::Button::new(ic(ph::ARROW_SQUARE_IN, "Add its inputs here"));
        if ui
            .add_enabled(n > 0, add)
            .on_hover_text(format!("its start image and references ({n}), just left of it"))
            .on_disabled_hover_text("made from text only")
            .clicked()
        {
            out.push(Msg::TileAddInputs(i));
            ui.close();
        }
    }
    if ui.button(ic(ph::PENCIL_SIMPLE, "Edit in the panel")).clicked() {
        out.push(Msg::TileSelect(Some(i)));
        ui.close();
    }
    if ui
        .button(ic(ph::MINUS_CIRCLE, "Remove from the tiling"))
        .on_hover_text("Delete · the image itself stays")
        .clicked()
    {
        out.push(Msg::TileRemove(i));
        ui.close();
    }
}

/// A text block's text (several lines) and size.
fn text_edit(ui: &mut egui::Ui, t: &Tile, g: &Geometry, i: usize, width: f32, out: &mut Vec<Msg>) {
    let TileContent::Text { text, size } = &t.content else { return };
    let mut x = text.clone();
    let edit = egui::TextEdit::multiline(&mut x).hint_text("text").desired_rows(3).desired_width(width);
    if ui.add(edit).changed() {
        out.push(Msg::TileText(i, x));
    }
    let mut size = *size;
    let auto = "as large as fits its cells, up to the title's size";
    if size_edit(ui, &mut size, g.cell.1, g.text_px(t), auto) {
        out.push(Msg::TileTextSize(i, size));
    }
}

/// A text size: auto, a share of the cell's height, or pixels; `now` is
/// what it comes to (a switch keeps it). True when changed.
fn size_edit(ui: &mut egui::Ui, size: &mut TextSize, cell_h: u32, now: u32, auto: &str) -> bool {
    let before = *size;
    ui.horizontal(|ui| {
        ui.label("size");
        if ui.selectable_label(*size == TextSize::Auto, "auto").on_hover_text(auto).clicked() {
            *size = TextSize::Auto;
        }
        let cell = matches!(size, TextSize::Cell(_));
        if ui.selectable_label(cell, "% of cell").on_hover_text("a share of the cell's height").clicked() && !cell {
            *size = TextSize::Cell(((now as f32 * 1000.0 / cell_h.max(1) as f32).round() / 10.0).max(0.1));
        }
        let px = matches!(size, TextSize::Px(_));
        if ui.selectable_label(px, "px").on_hover_text("exact output pixels").clicked() && !px {
            *size = TextSize::Px(now);
        }
        match size {
            TextSize::Cell(pc) => {
                ui.add(egui::DragValue::new(pc).range(0.5..=100.0).speed(0.1).suffix(" %"));
            }
            TextSize::Px(n) => {
                ui.add(egui::DragValue::new(n).range(6..=2000).suffix(" px"));
            }
            TextSize::Auto => {}
        }
        if !matches!(size, TextSize::Px(_)) {
            ui.weak(format!("{now} px"));
        }
    });
    *size != before
}

/// What a caption's `{placeholders}` can be.
fn placeholders(ui: &mut egui::Ui) {
    ui.label("Any caption can hold placeholders, filled per image:");
    egui::Grid::new("caption_fields").num_columns(2).show(ui, |ui| {
        for (key, what) in increart_gui_core::CAPTION_FIELDS {
            ui.monospace(format!("{{{key}}}"));
            ui.weak(what);
            ui.end_row();
        }
    });
}

fn settings(
    ui: &mut egui::Ui,
    st: &AppState,
    d: &TileDialog,
    root: &std::path::Path,
    tex: &mut Textures,
    out: &mut Vec<Msg>,
) {
    let mut s: TileSettings = st.config.tile.clone();
    let l = &d.layout;
    ui.label(RichText::new("Layout").strong());
    ui.horizontal(|ui| {
        let mut cols = l.cols;
        ui.label("columns");
        if ui.add(egui::DragValue::new(&mut cols).range(1..=increart_gui_core::tile::MAX_COLS)).changed() {
            out.push(Msg::TileCols(cols));
        }
        if ui.button("Reflow").on_hover_text("pack the tiles in order, no holes").clicked() {
            out.push(Msg::TileReflow);
        }
    });
    ui.horizontal(|ui| {
        ui.label("gap");
        ui.add(egui::DragValue::new(&mut s.gap).range(0..=512).suffix(" px"));
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut s.fill, false, "fit").on_hover_text("the whole image, letterboxed");
        ui.radio_value(&mut s.fill, true, "fill").on_hover_text("cropped to fill the cell");
    });
    ui.label("background");
    ui.horizontal_wrapped(|ui| {
        for (name, c) in BACKGROUNDS {
            if ui.selectable_label(s.background == c, name).clicked() {
                s.background = c;
            }
        }
        ui.color_edit_button_srgba_unmultiplied(&mut s.background);
    });
    ui.horizontal(|ui| {
        ui.label("scale");
        for pc in [25, 50, 100] {
            if ui.selectable_label(s.scale == pc, format!("{pc}%")).clicked() {
                s.scale = pc;
            }
        }
        // any other scale: a plain number, so it doesn't read as a fourth preset
        ui.separator();
        ui.weak("custom");
        ui.spacing_mut().interact_size.x = 32.0;
        ui.add(egui::DragValue::new(&mut s.scale).range(1..=100)).on_hover_text("drag, or double-click to type");
        ui.weak("%");
    });
    let big = largest(st, l, root, tex);
    ui.horizontal(|ui| {
        ui.label("cell");
        let mut auto = s.cell.is_none();
        if ui
            .checkbox(&mut auto, "auto")
            .on_hover_text(format!("the largest image's size, {} × {}", big.0, big.1))
            .changed()
        {
            s.cell = if auto { None } else { Some([big.0, big.1]) };
        }
        if let Some([w, h]) = &mut s.cell {
            let max = increart_gui_core::tile::MAX_CELL;
            ui.spacing_mut().interact_size.x = 44.0;
            ui.add(egui::DragValue::new(w).range(16..=max).speed(4.0));
            ui.weak("×");
            ui.add(egui::DragValue::new(h).range(16..=max).speed(4.0));
            ui.weak("px");
        }
    })
    .response
    .on_hover_text("each cell's size at 100 %; images fit (or fill) inside it");
    let g = Geometry::new(l, &s, big);
    ui.weak(format!("{} × {} px · {} cells of {} × {}", g.size.0, g.size.1, l.cols * l.rows(), g.cell.0, g.cell.1));

    ui.add_space(8.0);
    ui.label(RichText::new("Save").strong());
    ui.horizontal_wrapped(|ui| {
        let tips = |f: TileFormat| match f {
            TileFormat::Png => "lossless, readable everywhere",
            TileFormat::Webp => "lossless, about half PNG's size",
            TileFormat::Jpeg => "lossy and small; transparency becomes white",
            TileFormat::Avif => "lossy, smaller than JPEG at the same quality; slow to write when large",
        };
        let current = s.save_format.usable();
        for &f in TileFormat::available() {
            if ui.selectable_label(current == f, f.name()).on_hover_text(tips(f)).clicked() {
                s.save_format = f;
            }
        }
    });
    if s.save_format.usable().lossy() {
        ui.horizontal(|ui| {
            ui.label("quality");
            ui.add(egui::Slider::new(&mut s.save_quality, 1..=100).suffix(" %"));
        });
    }

    ui.add_space(8.0);
    ui.label(RichText::new("Title").strong());
    let (mut title, mut subtitle) = (l.title.clone(), l.subtitle.clone());
    let a = ui.add(egui::TextEdit::singleline(&mut title).hint_text("title").desired_width(f32::INFINITY));
    let b = ui.add(egui::TextEdit::singleline(&mut subtitle).hint_text("subtitle").desired_width(f32::INFINITY));
    if a.changed() || b.changed() {
        out.push(Msg::TileTitle(title, subtitle));
    }
    let auto = "4 % of the image's width, at least 10 % of the cell's height; the subtitle is half";
    size_edit(ui, &mut s.title_text, g.cell.1, g.title_px, auto);

    ui.add_space(8.0);
    ui.label(RichText::new("Captions").strong());
    ui.horizontal_wrapped(|ui| {
        ui.weak("all from:").on_hover_text("generated images only: the others keep their own captions");
        for f in CaptionFill::EVERY {
            if ui.small_button(f.label()).clicked() {
                out.push(Msg::TileCaptions(f));
            }
        }
    });
    ui.horizontal(|ui| {
        ui.weak("format");
        let apply = ui
            .button("Apply")
            .on_hover_text("to every generated image's caption (pasted and input images, marked-up copies included, keep their own: they have no seed or run)");
        let edit = egui::TextEdit::singleline(&mut s.caption_format).desired_width(f32::INFINITY);
        let r = ui.add(edit).on_hover_ui(placeholders);
        if apply.clicked() || (r.lost_focus() && ui.input(|x| x.key_pressed(Key::Enter))) {
            out.push(Msg::TileCaptionFormat(s.caption_format.clone()));
        }
    });
    let auto = "5 % of the cell's height; a caption too wide shrinks (to 60 %) before it's cut short";
    size_edit(ui, &mut s.caption_text, g.cell.1, g.caption_px, auto);
    ui.horizontal(|ui| {
        ui.label("colour");
        ui.color_edit_button_srgba_unmultiplied(&mut s.caption_color);
    });

    ui.add_space(8.0);
    ui.label(RichText::new("Tile").strong());
    match d.selected.and_then(|i| l.tiles.get(i).map(|t| (i, t))) {
        None => {
            ui.weak("Click a tile to edit its caption and size.");
        }
        Some((i, t)) if t.image().is_none() => {
            ui.weak(format!("#{} · text · column {}, row {}", i + 1, t.col + 1, t.row + 1));
            text_edit(ui, t, &g, i, f32::INFINITY, out);
            ui.horizontal(|ui| {
                let (mut w, mut h) = (t.w, t.h);
                ui.label("spans");
                let a = ui.add(egui::DragValue::new(&mut w).range(1..=l.cols));
                ui.label("×");
                let b = ui.add(egui::DragValue::new(&mut h).range(1..=16));
                if a.changed() || b.changed() {
                    out.push(Msg::TileSpan(i, w, h));
                }
            });
            if ui
                .button(ic(ph::MINUS_CIRCLE, "Remove from the tiling"))
                .on_hover_text("Delete · the image itself stays")
                .clicked()
            {
                out.push(Msg::TileRemove(i));
            }
        }
        Some((i, t)) => {
            ui.horizontal(|ui| {
                let tx = t.image().and_then(|im| tex.thumb(root, im));
                let (r, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
                image_fit_bg(&ui.painter_at(r), r, tx.as_ref(), true);
                ui.vertical(|ui| {
                    ui.weak(format!("#{} · column {}, row {}", i + 1, t.col + 1, t.row + 1));
                    ui.horizontal(|ui| {
                        let (mut w, mut h) = (t.w, t.h);
                        ui.label("spans");
                        let a = ui.add(egui::DragValue::new(&mut w).range(1..=l.cols));
                        ui.label("×");
                        let b = ui.add(egui::DragValue::new(&mut h).range(1..=16));
                        if a.changed() || b.changed() {
                            out.push(Msg::TileSpan(i, w, h));
                        }
                    });
                });
            });
            let mut c = t.caption.clone();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut c)
                        .hint_text("caption, e.g. cfg {cfg}")
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                out.push(Msg::TileCaption(i, c));
            }
            if ui
                .button(ic(ph::MINUS_CIRCLE, "Remove from the tiling"))
                .on_hover_text("Delete · the image itself stays")
                .clicked()
            {
                out.push(Msg::TileRemove(i));
            }
        }
    }
    if s != st.config.tile {
        out.push(Msg::TileSettings(s));
    }
}
