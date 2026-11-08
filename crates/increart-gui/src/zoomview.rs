//! Zoom and pan over an image: the large preview, Review's single image and
//! A/B compare's two sides, which show the same part of both images when
//! linked. A view is kept relative to its image (the middle as a fraction
//! of its width and height, the zoom as a multiple of the scale that fits),
//! so linked images of different sizes (an input and an output, an upscale
//! and its source) show the same part of the picture.

use eframe::egui::{self, Align2, Color32, Key, Rect, RichText, TextureHandle, Vec2};

use crate::widgets::ph;

/// Where a view is, relative to its image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Times the scale that fits the box; `None`: fit.
    pub zoom: Option<f32>,
    /// The image point at the middle of the box, as a fraction of its
    /// width and height.
    pub center: Vec2,
}

impl Default for View {
    fn default() -> Self {
        View { zoom: None, center: Vec2::splat(0.5) }
    }
}

/// An image in a box: what a view's scales are measured against.
#[derive(Clone, Copy, Debug)]
pub struct Geom {
    /// The box, in points.
    pub size: Vec2,
    /// The image, in pixels.
    pub full: Vec2,
    /// Screen pixels per point (the interface zoom, the display's scale).
    pub ppp: f32,
}

impl Geom {
    pub fn new(ctx: &egui::Context, size: Vec2, full: Vec2) -> Self {
        Geom { size, full: full.max(Vec2::splat(1.0)), ppp: ctx.pixels_per_point() }
    }

    /// Points per image pixel that fit the image in the box.
    pub fn fit(&self) -> f32 {
        (self.size.x / self.full.x).min(self.size.y / self.full.y).max(1e-6)
    }

    /// Points per image pixel for one screen pixel per image pixel (100%).
    pub fn actual(&self) -> f32 {
        1.0 / self.ppp
    }

    /// From a quarter of fit or 100%, whichever is smaller, to 800% or fit.
    fn limits(&self) -> (f32, f32) {
        (self.fit().min(self.actual()) * 0.25, self.fit().max(self.actual() * 8.0))
    }
}

impl View {
    /// Points per image pixel.
    pub fn scale(&self, g: Geom) -> f32 {
        g.fit() * self.zoom.unwrap_or(1.0)
    }

    /// The zoom as it's said: 100% one image pixel per screen pixel.
    pub fn percent(&self, g: Geom) -> f32 {
        self.scale(g) / g.actual() * 100.0
    }

    pub fn reset(&mut self) {
        *self = View::default();
    }

    pub fn zoomed_in(&self) -> bool {
        self.zoom.is_some_and(|z| z > 1.001)
    }

    pub fn is_actual(&self, g: Geom) -> bool {
        (self.scale(g) / g.actual() - 1.0).abs() < 1e-3
    }

    /// To scale `s` (points per image pixel), keeping the image point `at`
    /// (points from the box's middle) where it is.
    pub fn zoom_to(&mut self, s: f32, at: Vec2, g: Geom) {
        let (lo, hi) = g.limits();
        let s = s.clamp(lo, hi);
        self.center += (at / self.scale(g) - at / s) / g.full;
        self.zoom = Some(s / g.fit());
    }

    /// 100%, about the box's middle.
    pub fn actual_size(&mut self, g: Geom) {
        self.zoom_to(g.actual(), Vec2::ZERO, g);
    }

    /// In or out a step (× 1.25), about the box's middle.
    pub fn step(&mut self, zoom_in: bool, g: Geom) {
        let f = if zoom_in { 1.25 } else { 1.0 / 1.25 };
        self.zoom_to(self.scale(g) * f, Vec2::ZERO, g);
    }

    /// Moved by a drag of `delta` points.
    pub fn pan(&mut self, delta: Vec2, g: Geom) {
        self.center -= delta / self.scale(g) / g.full;
    }

    /// No blank edges: an axis smaller than the box stays centred.
    pub fn clamped(mut self, g: Geom) -> View {
        let s = self.scale(g);
        let axis = |c: f32, full: f32, size: f32| {
            let shown = full * s;
            if shown <= size {
                0.5
            } else {
                let half = size / 2.0 / shown;
                c.clamp(half, 1.0 - half)
            }
        };
        self.center = egui::vec2(axis(self.center.x, g.full.x, g.size.x), axis(self.center.y, g.full.y, g.size.y));
        self
    }

    /// Where the whole image goes, for a box at `rect`.
    pub fn shown(&self, rect: Rect, g: Geom) -> Rect {
        let size = g.full * self.scale(g);
        Rect::from_min_size(rect.center() - self.center * size, size)
    }
}

/// The wheel and a pinch zoom at the pointer, a drag (left or middle
/// button) pans; the grab cursors. `resp` is the box's (its middle the
/// box's). Whether the view changed.
pub fn interact(ui: &egui::Ui, resp: &egui::Response, v: &mut View, g: Geom) -> bool {
    let before = *v;
    if let Some(p) = resp.hover_pos() {
        let (wheel, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let f = pinch * (wheel / 200.0).exp();
        if f != 1.0 {
            v.zoom_to(v.scale(g) * f, p - resp.rect.center(), g);
        }
    }
    if resp.dragged_by(egui::PointerButton::Primary) || resp.dragged_by(egui::PointerButton::Middle) {
        v.pan(resp.drag_delta(), g);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if resp.hovered() && v.zoomed_in() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    *v = v.clamped(g);
    *v != before
}

/// The view `v` of `tex` in the box `rect`, over a checkerboard (or a flat
/// dark background) where the image is; `p` clipped to the box.
pub fn draw(p: &egui::Painter, rect: Rect, v: View, g: Geom, tex: Option<&TextureHandle>, checker: bool) {
    let Some(t) = tex else {
        return crate::widgets::image_fit_bg(p, rect, None, checker);
    };
    let shown = v.shown(rect, g);
    let visible = shown.intersect(rect);
    if checker {
        crate::widgets::checkerboard(p, visible);
    } else {
        p.rect_filled(visible, 0.0, Color32::from_gray(30));
    }
    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    p.image(t.id(), shown, uv, Color32::WHITE);
}

/// `+` / `−` / `0` (no Ctrl: that's the interface zoom).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyZoom {
    In,
    Out,
    Fit,
}

pub fn key(ctx: &egui::Context) -> Option<KeyZoom> {
    ctx.input(|i| {
        if i.modifiers.command || i.modifiers.alt {
            None
        } else if i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals) {
            Some(KeyZoom::In)
        } else if i.key_pressed(Key::Minus) {
            Some(KeyZoom::Out)
        } else if i.key_pressed(Key::Num0) {
            Some(KeyZoom::Fit)
        } else {
            None
        }
    })
}

impl KeyZoom {
    pub fn apply(self, v: &mut View, g: Geom) {
        match self {
            KeyZoom::In => v.step(true, g),
            KeyZoom::Out => v.step(false, g),
            KeyZoom::Fit => v.reset(),
        }
    }
}

/// An image a pane shows: its box, the box's response (clicks and drags),
/// the texture and the image's size in pixels. `side` names its own view
/// when the panes aren't linked ("a", "b").
pub struct Pane {
    pub rect: Rect,
    pub resp: egui::Response,
    pub tex: Option<TextureHandle>,
    pub full: Vec2,
    pub side: &'static str,
}

/// The image's size in pixels: as read from its file, else the texture's.
pub fn full_size(dims: Option<[u32; 2]>, tex: Option<&TextureHandle>) -> Vec2 {
    match (dims, tex) {
        (Some([w, h]), _) => egui::vec2(w as f32, h as f32),
        (None, Some(t)) => t.size_vec2(),
        (None, None) => Vec2::splat(1.0),
    }
}

/// `panes` drawn zoomed and panned, after their input. `linked`: one view
/// for all (the same part of each image), kept under `base`; else each
/// its own, the shared one following the last moved (what linking again
/// takes). The view stays as the images change (stepping through them).
/// `keys`: `+` / `−` / `0` apply, to the pane under the pointer (or all).
pub fn show(ui: &egui::Ui, base: egui::Id, linked: bool, keys: bool, checker: bool, panes: &[Pane]) {
    let ctx = ui.ctx().clone();
    let shared_id = base.with("shared");
    let id = |p: &Pane| if linked { shared_id } else { base.with(p.side) };
    let shared = ctx.data(|d| d.get_temp::<View>(shared_id)).unwrap_or_default();
    let mut views: Vec<View> =
        panes.iter().map(|p| ctx.data(|d| d.get_temp::<View>(id(p))).unwrap_or(shared)).collect();
    let geoms: Vec<Geom> = panes.iter().map(|p| Geom::new(&ctx, p.rect.size(), p.full)).collect();
    let key = if keys { key(&ctx) } else { None };
    let pointer = ctx.pointer_hover_pos();
    let over = |r: Rect| pointer.is_some_and(|p| r.contains(p));
    let none_over = !panes.iter().any(|p| over(p.rect));
    let mut moved = None;
    for (i, (p, &g)) in panes.iter().zip(&geoms).enumerate() {
        let mut v = views[i].clamped(g);
        let mut changed = interact(ui, &p.resp, &mut v, g);
        // linked, the keys once: at the pane under the pointer, else the first
        if let Some(k) = key
            && (over(p.rect) || none_over && (!linked || i == 0))
        {
            k.apply(&mut v, g);
            v = v.clamped(g);
            changed = true;
        }
        changed |= controls(&ctx, base.with(("controls", i)), p.rect, &mut v, g, over(p.rect));
        if changed {
            moved = Some(i);
            if linked {
                views.fill(v);
            } else {
                views[i] = v;
            }
        }
    }
    for ((p, &g), v) in panes.iter().zip(&geoms).zip(&views) {
        draw(&ui.painter_at(p.rect), p.rect, v.clamped(g), g, p.tex.as_ref(), checker);
    }
    ctx.data_mut(|d| {
        for (p, v) in panes.iter().zip(&views) {
            d.insert_temp(id(p), *v);
        }
        match (linked, views.first(), moved) {
            // unlinking starts both sides where they were
            (true, Some(v), _) => {
                for side in ["a", "b"] {
                    d.insert_temp(base.with(side), *v);
                }
            }
            (false, _, Some(i)) => {
                d.insert_temp(shared_id, views[i]);
            }
            _ => {}
        }
    });
}

/// Fit · 100% · − n% +, small, at the bottom right of the box: while the
/// pointer is over it (`hovered`) or it's zoomed. Whether one was clicked.
fn controls(ctx: &egui::Context, id: egui::Id, rect: Rect, v: &mut View, g: Geom, hovered: bool) -> bool {
    if !hovered && v.zoom.is_none() {
        return false;
    }
    let mut changed = false;
    egui::Area::new(id)
        .order(egui::Order::Middle)
        .pivot(Align2::RIGHT_BOTTOM)
        .fixed_pos(rect.right_bottom() - egui::vec2(6.0, 6.0))
        .constrain_to(rect)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_black_alpha(170))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(4, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.weak(ph::INFO).on_hover_text(
                            "wheel or + / −: zoom (at the pointer) · drag: move · 0: fit\n\
                             double-click: full size",
                        );
                        if ui.selectable_label(v.zoom.is_none(), "Fit").on_hover_text("fit the image   0").clicked() {
                            v.reset();
                            changed = true;
                        }
                        let hint = "one image pixel per screen pixel";
                        if ui.selectable_label(v.is_actual(g), "100%").on_hover_text(hint).clicked() {
                            v.actual_size(g);
                            changed = true;
                        }
                        if ui.small_button(ph::MAGNIFYING_GLASS_MINUS).on_hover_text("zoom out   −").clicked() {
                            v.step(false, g);
                            changed = true;
                        }
                        ui.label(RichText::new(format!("{:.0}%", v.percent(g))).monospace());
                        if ui.small_button(ph::MAGNIFYING_GLASS_PLUS).on_hover_text("zoom in   +").clicked() {
                            v.step(true, g);
                            changed = true;
                        }
                    });
                });
        });
    if changed {
        *v = v.clamped(g);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(size: (f32, f32), full: (f32, f32)) -> Geom {
        Geom { size: egui::vec2(size.0, size.1), full: egui::vec2(full.0, full.1), ppp: 1.0 }
    }

    #[test]
    fn zooming_at_a_point_keeps_it_under_the_pointer() {
        let g = geom((500.0, 500.0), (1000.0, 1000.0));
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), g.size);
        let mut v = View::default();
        let at = egui::vec2(100.0, -50.0);
        let image_point = |v: &View| (rect.center() + at - v.shown(rect, g).min) / v.scale(g);
        let before = image_point(&v);
        v.zoom_to(2.0, at, g);
        assert!((image_point(&v) - before).length() < 1e-3);
        assert_eq!(v.percent(g), 200.0);
    }

    #[test]
    fn linked_views_show_the_same_part_of_images_of_other_sizes() {
        // an image and its 2x upscale, in boxes of the same size
        let (small, large) = (geom((400.0, 400.0), (512.0, 512.0)), geom((400.0, 400.0), (1024.0, 1024.0)));
        let mut v = View::default();
        v.zoom_to(small.fit() * 3.0, egui::vec2(120.0, 80.0), small);
        v.pan(egui::vec2(-30.0, 10.0), small);
        let v = v.clamped(small);
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), small.size);
        // the box's corners, as fractions of each image
        let part = |g: Geom| {
            let s = v.clamped(g).shown(rect, g);
            ((rect.min - s.min) / s.size(), (rect.max - s.min) / s.size())
        };
        let (a, b) = (part(small), part(large));
        assert!((a.0 - b.0).length() < 1e-4 && (a.1 - b.1).length() < 1e-4, "{a:?} {b:?}");
    }

    #[test]
    fn no_blank_edges_and_a_small_image_stays_centred() {
        let g = geom((400.0, 400.0), (800.0, 400.0));
        let mut v = View { zoom: Some(4.0), center: egui::vec2(0.0, 1.0) };
        v = v.clamped(g);
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), g.size);
        let s = v.shown(rect, g);
        assert!(s.min.x <= 0.0 && s.min.y <= 0.0 && s.max.y >= 400.0);
        // fitted, the short side is centred whatever the centre was
        let v = View { zoom: None, center: egui::vec2(0.9, 0.1) }.clamped(g);
        assert_eq!(v.center, egui::vec2(0.5, 0.5));
    }

    #[test]
    fn hundred_percent_counts_screen_pixels() {
        let mut g = geom((500.0, 500.0), (1000.0, 1000.0));
        g.ppp = 1.5;
        let mut v = View::default();
        v.actual_size(g);
        assert!(v.is_actual(g));
        assert!((v.scale(g) - 1.0 / 1.5).abs() < 1e-6);
    }
}
