//! Crop / mark… (DESIGN.md, Crop and mark): crop an image and draw over
//! it (pencil, rectangle, ellipse) to show the edit model what to change.
//! What is drawn stays vectors in the original's pixels until Save makes
//! the copy.

use eframe::egui::{self, Color32, Key, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use increart_core::markup::{distance, outline};
use increart_core::model::{ImageEdits, Mark, Shape};
use increart_gui_core::{AppState, CropAspect, MarkTool, MarkTools, Msg};

use crate::textures::Textures;
use crate::widgets::{ACCENT, dialog_footer, ic, ph};

const FOOT: f32 = 40.0;
const SWATCHES: [[u8; 3]; 6] =
    [[230, 30, 30], [255, 210, 0], [40, 200, 60], [30, 140, 255], [255, 255, 255], [0, 0, 0]];
/// The opacity steps, as alpha.
const OPACITY: [(u8, &str); 4] = [(64, "25%"), (128, "50%"), (191, "75%"), (255, "100%")];

/// A drag in progress, in image pixels.
#[derive(Clone, Debug)]
enum Drag {
    Line(Vec<[f32; 2]>),
    Shape([f32; 2], [f32; 2]),
    /// A new crop from this corner.
    NewCrop([f32; 2]),
    /// The crop moved: where it was, where the drag started.
    MoveCrop([f32; 4], [f32; 2]),
    /// A crop corner dragged: the opposite corner stays.
    Corner([f32; 2]),
    /// The view panned.
    Pan,
}

/// Zoom (1: fit) and pan (screen points) of the canvas.
#[derive(Clone, Copy, Debug)]
struct View {
    zoom: f32,
    pan: Vec2,
}

/// Image pixels ↔ screen points.
struct Map {
    origin: Pos2,
    scale: f32,
}

impl Map {
    fn screen(&self, p: [f32; 2]) -> Pos2 {
        self.origin + Vec2::new(p[0], p[1]) * self.scale
    }
    fn image(&self, p: Pos2) -> [f32; 2] {
        let v = (p - self.origin) / self.scale;
        [v.x, v.y]
    }
}

fn color32(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

pub fn window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let (Some(m), Some(root)) = (&st.markup, &st.library) else { return };
    let size = ctx.content_rect().size() * 0.95 - Vec2::splat(16.0);
    let tools = st.mark_tools;
    let dims = tex.dims(root, &m.image);
    let texture = tex.full(root, &m.image);
    let mut t = tools;
    let mut edits = m.edits.clone();
    let (mut save, mut use_it, mut cancel) = (false, false, false);
    egui::Modal::new(egui::Id::unique("markup")).show(ctx, |ui| {
        ui.set_width(size.x);
        ui.set_height(size.y);
        ui.horizontal(|ui| {
            ui.heading(ic(ph::PENCIL_SIMPLE, "Crop / mark"));
            if let Some([w, h]) = dims {
                let now = edits.crop.map_or(format!("{w} × {h}"), |[_, _, cw, ch]| format!("{w} × {h} → {cw} × {ch}"));
                ui.weak(now);
            }
        });
        toolbar(ui, st, &mut t, out);
        ui.separator();
        let body = size.y - (ui.cursor().min.y - ui.min_rect().top()) - FOOT;
        let area = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), body.max(100.0)));
        ui.allocate_rect(area, Sense::hover());
        match dims {
            Some([w, h]) => canvas(ui, st, area, (w, h), texture.as_ref(), &t, &mut edits),
            None => {
                ui.painter().text(
                    area.center(),
                    egui::Align2::CENTER_CENTER,
                    "The image can't be read.",
                    egui::FontId::proportional(16.0),
                    ui.visuals().weak_text_color(),
                );
            }
        }
        ui.separator();
        dialog_footer(
            ui,
            |ui| {
                if ui.add_enabled(!edits.marks.is_empty(), egui::Button::new("Clear marks")).clicked() {
                    edits.marks.clear();
                }
                if ui.add_enabled(edits.crop.is_some(), egui::Button::new("Reset crop")).clicked() {
                    edits.crop = None;
                }
            },
            |ui| {
                let (label, tip) = match (m.slot.is_some(), m.replace) {
                    (true, true) => ("Save   Enter", "replaces the edited copy in Generate"),
                    (true, false) => ("Save   Enter", "the marked copy goes into Generate below the image"),
                    (false, _) => ("Save copy   Enter", "a copy, hidden in the input library (Show edited copies)"),
                };
                let b = egui::Button::new(RichText::new(ic(ph::FLOPPY_DISK, label)).strong());
                save = ui.add_enabled(!m.saving, b).on_hover_text(tip).clicked();
                if m.slot.is_none() {
                    let b = egui::Button::new(ic(ph::ARROW_SQUARE_OUT, "Save and use in Generate"));
                    use_it = ui
                        .add_enabled(!m.saving, b)
                        .on_hover_text("the copy, also as a reference in the selected project's Generate form")
                        .clicked();
                }
                cancel = ui.button("Cancel   Esc").clicked();
                if m.saving {
                    ui.spinner();
                }
            },
        );
    });
    if t != tools {
        out.push(Msg::MarkTools(t));
    }
    if edits != m.edits {
        out.push(Msg::MarkupEdit(edits));
    }
    keys(ctx, st, &mut t, out);
    let (enter, esc) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    // Escape or Cancel closes; a click outside doesn't (it would lose the marks)
    if save || use_it || enter {
        out.push(Msg::MarkupSave(use_it));
    } else if cancel || esc {
        out.push(Msg::CloseMarkup);
    }
}

fn toolbar(ui: &mut egui::Ui, st: &AppState, t: &mut MarkTools, out: &mut Vec<Msg>) {
    let Some(m) = &st.markup else { return };
    ui.horizontal_wrapped(|ui| {
        for (tool, icon, name, key) in [
            (MarkTool::Crop, ph::CROP, "Crop", "C"),
            (MarkTool::Pencil, ph::PENCIL_SIMPLE, "Pencil", "P"),
            (MarkTool::Rect, ph::RECTANGLE, "Rectangle", "R"),
            (MarkTool::Ellipse, ph::CIRCLE, "Ellipse", "O"),
            (MarkTool::Eraser, ph::ERASER, "Eraser", "E"),
        ] {
            let tip = match tool {
                MarkTool::Crop => "drag a crop; drag inside it to move it, a corner to resize",
                MarkTool::Rect | MarkTool::Ellipse => "drag to draw · Shift: square / circle",
                MarkTool::Eraser => "click or drag over a mark to remove it",
                MarkTool::Pencil => "draw freehand",
            };
            if ui.selectable_label(t.tool == tool, ic(icon, name)).on_hover_text(format!("{tip}  ({key})")).clicked() {
                t.tool = tool;
            }
        }
        ui.separator();
        if t.tool == MarkTool::Crop {
            ui.label("aspect");
            for (a, name, tip) in [
                (CropAspect::Free, "free", "any shape"),
                (CropAspect::Original, "original", "the image's own aspect"),
                (CropAspect::Generate, "Generate size", "the aspect of Generate's output size"),
            ] {
                if ui.selectable_label(t.aspect == a, name).on_hover_text(tip).clicked() {
                    t.aspect = a;
                }
            }
            ui.checkbox(&mut t.snap32, "multiples of 32").on_hover_text("round the crop's sides when the drag ends");
        } else {
            // the swatches and the picker set the colour; the opacity stays
            let [r0, g0, b0, a] = t.color;
            for [r, g, b] in SWATCHES {
                let (resp, p) = ui.allocate_painter(Vec2::splat(18.0), Sense::click());
                p.circle_filled(resp.rect.center(), 7.0, Color32::from_rgb(r, g, b));
                if [r0, g0, b0] == [r, g, b] {
                    p.circle_stroke(resp.rect.center(), 8.5, Stroke::new(1.5, ACCENT));
                }
                if resp.clicked() {
                    t.color = [r, g, b, a];
                }
            }
            let mut rgb = [r0, g0, b0];
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                t.color = [rgb[0], rgb[1], rgb[2], a];
            }
            ui.label("opacity");
            for (alpha, name) in OPACITY {
                if ui.selectable_label(t.color[3] == alpha, name).clicked() {
                    t.color[3] = alpha;
                }
            }
            ui.label("width");
            ui.add(egui::Slider::new(&mut t.width, 1.0..=30.0).show_value(false))
                .on_hover_text("in thousandths of the image's longer side");
        }
        ui.separator();
        let undo = egui::Button::new(ph::ARROW_COUNTER_CLOCKWISE);
        if ui.add_enabled(!m.undo.is_empty(), undo).on_hover_text("undo (Ctrl+Z)").clicked() {
            out.push(Msg::MarkupUndo);
        }
        let redo = egui::Button::new(ph::ARROW_CLOCKWISE);
        if ui.add_enabled(!m.redo.is_empty(), redo).on_hover_text("redo (Ctrl+Shift+Z)").clicked() {
            out.push(Msg::MarkupRedo);
        }
        ui.weak(ph::INFO).on_hover_text("scroll: zoom · right-drag: pan · 0: fit");
    });
}

fn keys(ctx: &egui::Context, st: &AppState, t: &mut MarkTools, out: &mut Vec<Msg>) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let tools = st.mark_tools;
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Z)
            || i.consume_key(egui::Modifiers::COMMAND, Key::Y)
        {
            out.push(Msg::MarkupRedo);
        }
        if i.consume_key(egui::Modifiers::COMMAND, Key::Z) {
            out.push(Msg::MarkupUndo);
        }
        for (key, tool) in [
            (Key::C, MarkTool::Crop),
            (Key::P, MarkTool::Pencil),
            (Key::R, MarkTool::Rect),
            (Key::O, MarkTool::Ellipse),
            (Key::E, MarkTool::Eraser),
        ] {
            if i.consume_key(egui::Modifiers::NONE, key) {
                t.tool = tool;
            }
        }
    });
    if *t != tools {
        out.push(Msg::MarkTools(*t));
    }
}

/// The crop's aspect (width / height), if it is held.
fn aspect(st: &AppState, t: &MarkTools, (w, h): (u32, u32)) -> Option<f32> {
    match t.aspect {
        CropAspect::Free => None,
        CropAspect::Original => Some(w as f32 / h as f32),
        CropAspect::Generate => Some(st.generate.width as f32 / st.generate.height.max(1) as f32),
    }
}

/// A crop from a fixed corner to the pointer: held to `ratio`, inside the
/// image.
fn crop_from(fixed: [f32; 2], to: [f32; 2], ratio: Option<f32>, (w, h): (u32, u32)) -> [f32; 4] {
    let to = [to[0].clamp(0.0, w as f32), to[1].clamp(0.0, h as f32)];
    let (mut dx, mut dy) = (to[0] - fixed[0], to[1] - fixed[1]);
    if let Some(r) = ratio {
        // the larger side leads; then shrink to fit inside the image
        if dx.abs() / r > dy.abs() {
            dy = dx.abs() / r * dy.signum();
        } else {
            dx = dy.abs() * r * dx.signum();
        }
        let room_x = if dx < 0.0 { fixed[0] } else { w as f32 - fixed[0] };
        let room_y = if dy < 0.0 { fixed[1] } else { h as f32 - fixed[1] };
        let k = (room_x / dx.abs().max(1e-3)).min(room_y / dy.abs().max(1e-3)).min(1.0);
        (dx, dy) = (dx * k, dy * k);
    }
    let x = fixed[0].min(fixed[0] + dx);
    let y = fixed[1].min(fixed[1] + dy);
    [x, y, dx.abs(), dy.abs()]
}

/// The crop in whole pixels, at least 8 × 8, rounded to multiples of 32
/// when asked, inside the image.
fn finish_crop(c: [f32; 4], snap32: bool, (w, h): (u32, u32)) -> Option<[u32; 4]> {
    if c[2] < 8.0 || c[3] < 8.0 {
        return None;
    }
    let (mut cw, mut ch) = (c[2].round() as u32, c[3].round() as u32);
    if snap32 {
        let snap = |v: u32, max: u32| ((v + 16) / 32 * 32).clamp(32.min(max), max / 32 * 32);
        (cw, ch) = (snap(cw, w), snap(ch, h));
    }
    let x = (c[0].round() as u32).min(w.saturating_sub(cw));
    let y = (c[1].round() as u32).min(h.saturating_sub(ch));
    Some(increart_core::markup::clamp_crop([x, y, cw, ch], w, h))
}

fn canvas(
    ui: &mut egui::Ui,
    st: &AppState,
    area: Rect,
    (w, h): (u32, u32),
    texture: Option<&egui::TextureHandle>,
    t: &MarkTools,
    edits: &mut ImageEdits,
) {
    let Some(m) = &st.markup else { return };
    let view_id = egui::Id::unique(("markup_view", &m.image));
    let drag_id = egui::Id::unique("markup_drag");
    let mut view = ui.data(|d| d.get_temp::<View>(view_id)).unwrap_or(View { zoom: 1.0, pan: Vec2::ZERO });
    let resp = ui.interact(area, egui::Id::unique("markup_canvas"), Sense::click_and_drag());
    let pointer = ui.input(|i| i.pointer.hover_pos());

    // zoom about the pointer, pan with the right or middle button
    let fit = (area.width() / w as f32).min(area.height() / h as f32);
    let scroll = if resp.hovered() { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
    if scroll != 0.0
        && let Some(p) = pointer
    {
        let before = Map { origin: origin(area, (w, h), fit, view), scale: fit * view.zoom }.image(p);
        view.zoom = (view.zoom * (scroll / 200.0).exp()).clamp(0.5, 16.0);
        let after = Map { origin: origin(area, (w, h), fit, view), scale: fit * view.zoom }.screen(before);
        view.pan += p - after;
    }
    if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) {
        view.pan += resp.drag_delta();
    }
    if !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(Key::Num0)) {
        view = View { zoom: 1.0, pan: Vec2::ZERO };
    }
    ui.data_mut(|d| d.insert_temp(view_id, view));
    let map = Map { origin: origin(area, (w, h), fit, view), scale: fit * view.zoom };
    let px = |v: f32| v / map.scale;
    let long = w.max(h) as f32;
    let width = t.width / 1000.0 * long;

    // the drag
    let shift = ui.input(|i| i.modifiers.shift);
    let mut drag = ui.data(|d| d.get_temp::<Option<Drag>>(drag_id)).flatten();
    let at = pointer.map(|p| map.image(p));
    let crop_f = edits.crop.map(|c| c.map(|v| v as f32));
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(p) = resp.interact_pointer_pos().map(|p| map.image(p))
    {
        drag = Some(match t.tool {
            MarkTool::Pencil => Drag::Line(vec![p]),
            MarkTool::Rect | MarkTool::Ellipse => Drag::Shape(p, p),
            MarkTool::Eraser => Drag::Pan, // erasing happens below; nothing to keep
            MarkTool::Crop => match crop_f {
                Some(c) => {
                    let corners = [[c[0], c[1]], [c[0] + c[2], c[1]], [c[0], c[1] + c[3]], [c[0] + c[2], c[1] + c[3]]];
                    let near = corners.iter().position(|k| map.screen(*k).distance(map.screen(p)) < 12.0);
                    match near {
                        Some(i) => Drag::Corner(corners[3 - i]),
                        None if p[0] >= c[0] && p[0] <= c[0] + c[2] && p[1] >= c[1] && p[1] <= c[1] + c[3] => {
                            Drag::MoveCrop(c, p)
                        }
                        None => Drag::NewCrop(p),
                    }
                }
                None => Drag::NewCrop(p),
            },
        });
    }
    let ratio = aspect(st, t, (w, h));
    let square = |a: [f32; 2], b: [f32; 2]| {
        if !shift {
            return b;
        }
        let s = (b[0] - a[0]).abs().max((b[1] - a[1]).abs());
        [a[0] + s * (b[0] - a[0]).signum(), a[1] + s * (b[1] - a[1]).signum()]
    };
    let mut live_crop = None;
    if let (Some(d), Some(p)) = (&mut drag, at) {
        match d {
            Drag::Line(points) => {
                if points.last().is_none_or(|&last| map.screen(last).distance(map.screen(p)) > 1.5) {
                    points.push(p);
                }
            }
            Drag::Shape(a, b) => *b = square(*a, p),
            Drag::NewCrop(a) | Drag::Corner(a) => live_crop = Some(crop_from(*a, p, ratio, (w, h))),
            Drag::MoveCrop(c, from) => {
                let x = (c[0] + p[0] - from[0]).clamp(0.0, w as f32 - c[2]);
                let y = (c[1] + p[1] - from[1]).clamp(0.0, h as f32 - c[3]);
                live_crop = Some([x, y, c[2], c[3]]);
            }
            Drag::Pan => {}
        }
    }
    if t.tool == MarkTool::Eraser
        && (resp.clicked() || resp.dragged_by(egui::PointerButton::Primary))
        && let Some(p) = at
    {
        let reach = px(6.0);
        edits.marks.retain(|mk| {
            let pts = outline(&mk.shape);
            let r = mk.width / 2.0 + reach;
            let hit = if pts.len() == 1 {
                distance(p, pts[0], pts[0]) <= r
            } else {
                pts.windows(2).any(|s| distance(p, s[0], s[1]) <= r)
            };
            !hit
        });
    }
    if t.tool == MarkTool::Pencil
        && resp.clicked()
        && let Some(p) = at
    {
        edits.marks.push(Mark { shape: Shape::Pencil(vec![p]), color: t.color, width });
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        match drag.take() {
            Some(Drag::Line(points)) => edits.marks.push(Mark { shape: Shape::Pencil(points), color: t.color, width }),
            Some(Drag::Shape(a, b)) if map.screen(a).distance(map.screen(b)) > 4.0 => {
                let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
                let shape = if t.tool == MarkTool::Ellipse { Shape::Ellipse(lo, hi) } else { Shape::Rect(lo, hi) };
                edits.marks.push(Mark { shape, color: t.color, width });
            }
            Some(Drag::NewCrop(_) | Drag::Corner(_) | Drag::MoveCrop(..)) => {
                if let Some(c) = live_crop.take().and_then(|c| finish_crop(c, t.snap32, (w, h))) {
                    edits.crop = Some(c);
                }
            }
            _ => {}
        }
    }
    ui.data_mut(|d| d.insert_temp(drag_id, drag.clone()));

    // the picture
    let p = ui.painter_at(area);
    p.rect_filled(area, 0.0, ui.visuals().extreme_bg_color);
    let img_rect = Rect::from_min_max(map.screen([0.0, 0.0]), map.screen([w as f32, h as f32]));
    crate::widgets::checkerboard(&p, img_rect.intersect(area));
    match texture {
        Some(t) => {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            p.image(t.id(), img_rect, uv, Color32::WHITE);
        }
        None => {
            p.text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                "loading…",
                egui::FontId::proportional(14.0),
                Color32::GRAY,
            );
        }
    }
    for mk in &edits.marks {
        paint(&p, &map, mk);
    }
    match &drag {
        Some(Drag::Line(points)) => {
            paint(&p, &map, &Mark { shape: Shape::Pencil(points.clone()), color: t.color, width });
        }
        Some(Drag::Shape(a, b)) => {
            let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
            let shape = if t.tool == MarkTool::Ellipse { Shape::Ellipse(lo, hi) } else { Shape::Rect(lo, hi) };
            paint(&p, &map, &Mark { shape, color: t.color, width });
        }
        _ => {}
    }
    // the crop: dimmed outside, its corners to drag
    let crop = live_crop.or(crop_f);
    if let Some(c) = crop {
        let r = Rect::from_min_max(map.screen([c[0], c[1]]), map.screen([c[0] + c[2], c[1] + c[3]]));
        let dim = Color32::from_black_alpha(150);
        // a pixel past the image, so its edge doesn't show through
        let img_rect = img_rect.expand(1.0);
        for outside in [
            Rect::from_min_max(img_rect.min, Pos2::new(img_rect.max.x, r.min.y)),
            Rect::from_min_max(Pos2::new(img_rect.min.x, r.max.y), img_rect.max),
            Rect::from_min_max(Pos2::new(img_rect.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)),
            Rect::from_min_max(Pos2::new(r.max.x, r.min.y), Pos2::new(img_rect.max.x, r.max.y)),
        ] {
            p.rect_filled(outside, 0.0, dim);
        }
        p.rect_stroke(r, 0.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Outside);
        if t.tool == MarkTool::Crop {
            for k in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
                p.rect_filled(Rect::from_center_size(k, Vec2::splat(9.0)), 1.0, Color32::WHITE);
            }
        }
        if live_crop.is_some() {
            let text = format!("{} × {}", c[2].round(), c[3].round());
            p.text(
                r.left_top() + Vec2::new(4.0, -4.0),
                egui::Align2::LEFT_BOTTOM,
                text,
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );
        }
    }
    // the brush, where it would draw
    if let (Some(pos), true) = (pointer, area.contains(pointer.unwrap_or_default())) {
        match t.tool {
            MarkTool::Pencil | MarkTool::Rect | MarkTool::Ellipse => {
                p.circle_stroke(pos, (width * map.scale / 2.0).max(2.0), Stroke::new(1.0, color32(t.color)));
            }
            MarkTool::Eraser => {
                p.circle_stroke(pos, 6.0, Stroke::new(1.0, Color32::WHITE));
            }
            MarkTool::Crop => {}
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(match t.tool {
                MarkTool::Crop => egui::CursorIcon::Crosshair,
                _ => egui::CursorIcon::None,
            });
        }
    }
}

/// Where the image's top-left corner is on screen.
fn origin(area: Rect, (w, h): (u32, u32), fit: f32, view: View) -> Pos2 {
    let s = fit * view.zoom;
    area.center() - Vec2::new(w as f32, h as f32) * s / 2.0 + view.pan
}

/// The half disc of radius `r` past `end`, away from `from`.
fn cap(p: &egui::Painter, end: Pos2, from: Pos2, r: f32, color: Color32) {
    let dir = (end - from).normalized();
    if !dir.is_finite() || r < 1.0 {
        return;
    }
    let base = dir.y.atan2(dir.x);
    let points = (0..=12)
        .map(|i| {
            let a = base - std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 12.0;
            end + Vec2::new(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
}

fn paint(p: &egui::Painter, map: &Map, mk: &Mark) {
    let stroke = Stroke::new((mk.width * map.scale).max(1.0), color32(mk.color));
    let points: Vec<Pos2> = outline(&mk.shape).into_iter().map(|q| map.screen(q)).collect();
    match points.as_slice() {
        [] => {}
        [one] => {
            p.circle_filled(*one, stroke.width / 2.0, stroke.color);
        }
        _ => {
            // round ends, as the copy draws them: half discs past each end, so
            // a see-through line isn't darker where they'd overlap it
            p.add(egui::Shape::line(points.clone(), stroke));
            let n = points.len();
            for (end, from) in [(points[0], points[1]), (points[n - 1], points[n - 2])] {
                cap(p, end, from, stroke.width / 2.0, stroke.color);
            }
        }
    }
}
