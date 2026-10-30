//! Small drawing helpers shared by the views.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, TextureHandle, Vec2};
use increart_gui_core::Msg;

pub use egui_phosphor::regular as ph;

/// Drag and drop payload: folders, projects or image links, as paths
/// relative to the library root.
pub struct Dragged(pub Vec<PathBuf>);

/// Megapixels, e.g. "1.05 MP".
pub fn mp(w: u32, h: u32) -> String {
    format!("{:.2} MP", w as f64 * h as f64 / 1e6)
}

/// Aspect ratio as the short side over the long one, with the orientation:
/// "67% landscape" (3:2), "56% portrait" (9:16), "square".
pub fn aspect(w: u32, h: u32) -> String {
    let (w, h) = (w.max(1), h.max(1));
    if w == h {
        return "square".into();
    }
    let pct = (w.min(h) as f64 / w.max(h) as f64 * 100.0).round();
    format!("{pct}% {}", if w > h { "landscape" } else { "portrait" })
}

/// "1344 × 896 · 67% landscape · 1.20 MP"
pub fn size_info(w: u32, h: u32) -> String {
    format!("{w} × {h} · {} · {}", aspect(w, h), mp(w, h))
}

/// Puts a library image, full size, on the system clipboard. Decoded on a
/// thread so a large image doesn't stall the UI.
pub fn copy_image(ctx: &egui::Context, root: &Path, img: &increart_core::ImageRef) {
    let (ctx, path) = (ctx.clone(), img.path(root));
    std::thread::spawn(move || {
        if let Ok(decoded) = image::open(&path) {
            let rgba = decoded.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            ctx.copy_image(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()));
            ctx.request_repaint();
        }
    });
}

/// The "Copy image" and "Describe with PE…" menu items, for any image's
/// right-click menu.
pub fn image_menu_items(ui: &mut egui::Ui, root: &Path, img: &increart_core::ImageRef, out: &mut Vec<Msg>) {
    copy_image_item(ui, root, img, out);
    markup_item(ui, img, None, out);
    describe_item(ui, img, "Describe with PE…", out);
}

/// **Crop / mark…**: the editor on the image; `index`: its place in
/// Generate, where the copy goes.
pub fn markup_item(ui: &mut egui::Ui, img: &increart_core::ImageRef, index: Option<usize>, out: &mut Vec<Msg>) {
    let tip = match index {
        Some(_) => "crop it, circle what to change: the marked copy goes in below it",
        None => "crop it, circle what to change: saved as a copy (hidden in the input library)",
    };
    if ui.button(ic(ph::PENCIL_SIMPLE, "Crop / mark…")).on_hover_text(tip).clicked() {
        out.push(Msg::OpenMarkup(img.clone(), index));
        ui.close();
    }
}

/// **Copy image**: full size, to the clipboard.
pub fn copy_image_item(ui: &mut egui::Ui, root: &Path, img: &increart_core::ImageRef, out: &mut Vec<Msg>) {
    if ui.button(ic(ph::COPY, "Copy image")).on_hover_text("to the clipboard, full size").clicked() {
        copy_image(ui.ctx(), root, img);
        out.push(Msg::Info("Image copied to the clipboard".into()));
        ui.close();
    }
}

/// **Describe with PE…**: the prompt-enhance server describes it as a prompt.
pub fn describe_item(ui: &mut egui::Ui, img: &increart_core::ImageRef, label: &str, out: &mut Vec<Msg>) {
    if ui
        .button(ic(ph::MAGIC_WAND, label))
        .on_hover_text("the prompt-enhance server describes it as a prompt")
        .clicked()
    {
        out.push(Msg::PeRewrite(increart_gui_core::PeTarget::Describe(img.clone())));
        ui.close();
    }
}

/// Scrolls the cursor's cell into view when the cursor has moved (keys,
/// Follow, a rating), not every frame, so scrolling by hand stays.
pub fn reveal_cursor<T: PartialEq + Clone + Send + Sync + 'static>(
    ui: &egui::Ui,
    salt: &str,
    at: T,
    r: &egui::Response,
) {
    let id = egui::Id::unique("cursor_seen").with(salt);
    if ui.data(|d| d.get_temp::<T>(id)).as_ref() != Some(&at) {
        ui.data_mut(|d| d.insert_temp(id, at));
        r.scroll_to_me(None);
    }
}

/// Where the drag source / drop target for a path was drawn this frame, for
/// the tour's drag checks.
pub fn dnd_rect_id(target: bool, path: &Path) -> egui::Id {
    egui::Id::unique(("dnd_rect", target, path))
}

/// Makes `resp` (showing `path`) draggable; `paths` is called when a drag
/// starts. Returns the response with drag sensing added (use it for clicks
/// too).
pub fn drag_source(resp: &egui::Response, path: &Path, paths: impl FnOnce() -> Vec<PathBuf>) -> egui::Response {
    let r = resp.interact(Sense::drag());
    r.ctx.data_mut(|d| d.insert_temp(dnd_rect_id(false, path), r.rect));
    if r.drag_started() {
        egui::DragAndDrop::set_payload(&r.ctx, Dragged(paths()));
    }
    r
}

/// Makes `resp` a drop target for the folder `dest` (relative to the library
/// root): outlined while something hovers it, `DropInto` on release (Ctrl
/// copies). True if something was dropped.
pub fn drop_target(ui: &egui::Ui, resp: &egui::Response, dest: &Path, out: &mut Vec<Msg>) -> bool {
    ui.ctx().data_mut(|d| d.insert_temp(dnd_rect_id(true, dest), resp.rect));
    if resp.dnd_hover_payload::<Dragged>().is_some() {
        ui.painter().rect_stroke(resp.rect.expand(1.0), 3.0, Stroke::new(2.0, ACCENT), egui::StrokeKind::Outside);
    }
    let Some(d) = resp.dnd_release_payload::<Dragged>() else { return false };
    let copy = ui.input(|i| i.modifiers.command);
    out.push(Msg::DropInto { paths: d.0.clone(), dest: dest.to_path_buf(), copy });
    true
}

/// While dragging: what will happen, next to the pointer.
pub fn drag_overlay(ctx: &egui::Context) {
    let Some(d) = egui::DragAndDrop::payload::<Dragged>(ctx) else { return };
    let Some(pos) = ctx.pointer_latest_pos() else { return };
    let what = match d.0.as_slice() {
        [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        many => format!("{} items", many.len()),
    };
    let verb = if ctx.input(|i| i.modifiers.command) { "Copy" } else { "Move" };
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("drag_overlay")));
    let text = format!("{verb} {what}   (Ctrl: copy)");
    let galley = p.layout_no_wrap(text, FontId::proportional(13.0), Color32::WHITE);
    let rect = Rect::from_min_size(pos + Vec2::new(14.0, 10.0), galley.size()).expand(5.0);
    p.rect_filled(rect, 4.0, Color32::from_black_alpha(220));
    p.galley(rect.min + Vec2::splat(5.0), galley, Color32::WHITE);
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
}

/// Focuses an inline text field until it loses focus. It must not grab it
/// back in the frame it lets go (Enter, a click elsewhere), or `lost_focus`
/// never fires and the edit can't be committed.
pub fn focus_until_done(resp: &egui::Response) {
    if !resp.has_focus() && !resp.lost_focus() {
        resp.request_focus();
    }
}

/// The font family holding Phosphor's filled glyphs.
pub const ICON_FILL: &str = "phosphor-fill";

/// A two-tone icon: the filled glyph in `fill`, the outline over it in `stroke`.
pub fn icon2(p: &egui::Painter, center: egui::Pos2, glyph: &str, size: f32, fill: Color32, stroke: Color32) {
    let fill_font = FontId::new(size, egui::FontFamily::Name(ICON_FILL.into()));
    p.text(center, Align2::CENTER_CENTER, glyph, fill_font, fill);
    p.text(center, Align2::CENTER_CENTER, glyph, FontId::proportional(size), stroke);
}

pub const ACCENT: Color32 = Color32::from_rgb(90, 160, 255);
/// The accent, desaturated and darker: tags such as "last used".
pub const ACCENT_MUTED: Color32 = Color32::from_rgb(105, 135, 180);
pub const GOOD: Color32 = Color32::from_rgb(90, 190, 100);
pub const NEUTRAL: Color32 = Color32::from_rgb(200, 170, 70);
pub const BAD: Color32 = Color32::from_rgb(210, 80, 80);

/// Button/menu text with a leading icon.
pub fn ic(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

pub fn checkerboard(p: &egui::Painter, rect: Rect) {
    let s = 10.0;
    p.rect_filled(rect, 0.0, Color32::from_gray(150));
    let (nx, ny) = ((rect.width() / s).ceil() as i32, (rect.height() / s).ceil() as i32);
    for y in 0..ny {
        for x in 0..nx {
            if (x + y) % 2 == 0 {
                let min = rect.min + Vec2::new(x as f32 * s, y as f32 * s);
                let r = Rect::from_min_size(min, Vec2::splat(s)).intersect(rect);
                p.rect_filled(r, 0.0, Color32::from_gray(110));
            }
        }
    }
}

/// Draws `tex` fitted into `rect` (keeping its aspect), over a checkerboard.
pub fn image_fit(p: &egui::Painter, rect: Rect, tex: Option<&TextureHandle>) {
    image_fit_bg(p, rect, tex, true)
}

/// The image cropped to fill `rect` (centred), over a checkerboard.
pub fn image_cover(p: &egui::Painter, rect: Rect, tex: Option<&TextureHandle>) {
    let Some(t) = tex else { return };
    let size = t.size_vec2();
    let scale = (rect.width() / size.x).max(rect.height() / size.y);
    // the part of the image that fits, in UV coordinates
    let half = Vec2::new(rect.width() / (size.x * scale), rect.height() / (size.y * scale)) / 2.0;
    let uv = Rect::from_min_max(egui::pos2(0.5 - half.x, 0.5 - half.y), egui::pos2(0.5 + half.x, 0.5 + half.y));
    checkerboard(p, rect);
    p.image(t.id(), rect, uv, Color32::WHITE);
}

/// As [`image_fit`], over a checkerboard or a flat dark background.
pub fn image_fit_bg(p: &egui::Painter, rect: Rect, tex: Option<&TextureHandle>, checker: bool) {
    match tex {
        Some(t) => {
            let size = t.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let r = Rect::from_center_size(rect.center(), size * scale);
            if checker {
                checkerboard(p, r);
            } else {
                p.rect_filled(r, 0.0, Color32::from_gray(30));
            }
            p.image(t.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            p.rect_filled(rect, 4.0, Color32::from_gray(45));
            p.text(rect.center(), Align2::CENTER_CENTER, ph::IMAGE, FontId::proportional(20.0), Color32::from_gray(90));
        }
    }
}

/// A clickable square thumbnail with optional selection outline and badge.
pub fn thumb(
    ui: &mut egui::Ui,
    tex: Option<&TextureHandle>,
    size: f32,
    selected: bool,
    badge: Option<Color32>,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, Color32::from_gray(35));
    image_fit(&p, rect, tex);
    if let Some(c) = badge {
        p.circle_filled(rect.right_top() + Vec2::new(-9.0, 9.0), 6.0, c);
    }
    let stroke = if selected {
        Stroke::new(3.0, ACCENT)
    } else if resp.hovered() {
        Stroke::new(1.0, Color32::GRAY)
    } else {
        Stroke::NONE
    };
    p.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
    resp
}

/// A fixed-size card in a wrapped grid; the whole card is clickable.
/// A plain click that isn't the start of a double-click: true a moment
/// after it, once no second click came. For clicks that open a side panel,
/// so a double-click doesn't (nor shift what's under the second click).
pub fn single_click(ui: &egui::Ui, resp: &egui::Response) -> bool {
    let id = resp.id.with("single_click");
    let delay = ui.ctx().options(|o| o.input_options.max_double_click_delay) + 0.05;
    let now = ui.input(|i| i.time);
    if resp.double_clicked() {
        ui.ctx().data_mut(|d| d.remove::<f64>(id));
        return false;
    }
    if resp.clicked() {
        if !ui.input(|i| i.modifiers.any()) {
            ui.ctx().data_mut(|d| d.insert_temp(id, now));
            ui.ctx().request_repaint_after_secs(delay as f32);
        }
        return false;
    }
    let Some(t) = ui.ctx().data(|d| d.get_temp::<f64>(id)) else { return false };
    if now - t < delay {
        ui.ctx().request_repaint_after_secs((delay - (now - t)) as f32);
        return false;
    }
    ui.ctx().data_mut(|d| d.remove::<f64>(id));
    // long gone (scrolled away meanwhile): dropped
    now - t < delay * 3.0
}

/// A widget drawn at `rect` on top of others, taking no space in `ui`'s
/// layout (`ui.put` would: in the wrapped grid that pushes the next tiles
/// along when a hovered tile shows its buttons, so they jump around).
pub fn overlay(ui: &mut egui::Ui, rect: egui::Rect, w: impl egui::Widget) -> egui::Response {
    let layout = egui::Layout::centered_and_justified(egui::Direction::LeftToRight);
    ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout)).add(w)
}

/// A button over an image (a tile's actions): dark, so it reads on any
/// picture.
pub fn overlay_button(b: egui::Button<'static>) -> egui::Button<'static> {
    b.fill(Color32::from_rgba_unmultiplied(20, 20, 20, 220))
}

/// Two buttons along the bottom of a tile's `cover` (its picture), each
/// (icon, label, hover): labelled when there's room, else icons with the
/// label on hover. Returns which was clicked.
pub fn cover_buttons(ui: &mut egui::Ui, cover: egui::Rect, buttons: &[(&str, &str, &str)]) -> Option<usize> {
    let n = buttons.len().max(1) as f32;
    let w = (cover.width() - 12.0 - 4.0 * (n - 1.0)) / n;
    let labelled = w >= 70.0;
    let mut clicked = None;
    for (i, (icon, label, hover)) in buttons.iter().enumerate() {
        let at = egui::pos2(cover.left() + 6.0 + i as f32 * (w + 4.0), cover.bottom() - 34.0);
        let text = if labelled { ic(icon, label) } else { icon.to_string() };
        let hover = if labelled { hover.to_string() } else { format!("{label}: {hover}") };
        let r =
            overlay(ui, egui::Rect::from_min_size(at, egui::vec2(w, 28.0)), overlay_button(egui::Button::new(text)));
        if r.on_hover_text(hover).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

pub fn card(ui: &mut egui::Ui, selected: bool, w: f32, h: f32, add: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    let stroke = if selected { Stroke::new(2.0, ACCENT) } else { ui.visuals().widgets.noninteractive.bg_stroke };
    let r = ui.allocate_ui_with_layout(Vec2::new(w + 14.0, h + 14.0), egui::Layout::top_down(egui::Align::Min), |ui| {
        egui::Frame::group(ui.style()).stroke(stroke).show(ui, |ui| {
            ui.set_width(w);
            ui.set_height(h);
            add(ui);
        })
    });
    ui.interact(r.response.rect, r.response.id.with("card"), Sense::click())
}

/// One line, cut with "…" to fit `chars`.
pub fn short(s: &str, chars: usize) -> String {
    let one = s.lines().next().unwrap_or("");
    if one.chars().count() > chars || s.lines().count() > 1 {
        format!("{}…", one.chars().take(chars).collect::<String>())
    } else {
        one.to_string()
    }
}

pub fn verdict_color(v: Option<increart_core::Verdict>) -> Color32 {
    match v {
        Some(increart_core::Verdict::Good) => GOOD,
        Some(increart_core::Verdict::Neutral) => NEUTRAL,
        Some(increart_core::Verdict::Bad) => BAD,
        None => ACCENT,
    }
}

/// One unit of a wrapping row (`horizontal_wrapped`): it goes to the next
/// line whole when it doesn't fit (by its width last frame), so a label
/// stays with its control.
pub fn group<R>(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let id = ui.scope_id().with(("group", id));
    let width = ui.ctx().data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
    let at_start = ui.cursor().min.x <= ui.max_rect().min.x + 1.0;
    if !at_start && width > ui.available_size_before_wrap().x {
        ui.end_row();
    }
    let r = ui.horizontal(|ui| {
        let inner = add(ui);
        (inner, ui.min_rect().width())
    });
    let (inner, w) = r.inner;
    ui.ctx().data_mut(|d| d.insert_temp(id, w));
    inner
}

/// The last unit of a wrapping row: at the right edge while it fits on
/// the line, else on the next line like the others.
pub fn right_group<R>(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let key = ui.scope_id().with(("group", id));
    let width = ui.ctx().data(|d| d.get_temp::<f32>(key)).unwrap_or(0.0);
    let room = ui.available_size_before_wrap().x - ui.spacing().item_spacing.x;
    if width > 0.0 && width <= room {
        ui.add_space(room - width);
    }
    group(ui, id, add)
}

/// The tab bar's **Details** toggle (put it in the bar's
/// [`right_group`]): shows or hides the tab's right-side panel.
pub fn details_toggle(
    ui: &mut egui::Ui,
    st: &increart_gui_core::AppState,
    panel: increart_gui_core::InfoPanel,
    out: &mut Vec<increart_gui_core::Msg>,
) {
    let on = !st.info_hidden.contains(&panel);
    let tip = if on { "hide the details panel on the right" } else { "show the details panel on the right" };
    if ui.selectable_label(on, ic(ph::INFO, "Details")).on_hover_text(tip).clicked() {
        out.push(increart_gui_core::Msg::ShowInfo(panel, !on));
    }
}

/// A details panel's placeholder while nothing is selected.
pub fn nothing_selected(ui: &mut egui::Ui, title: &str, text: &str) {
    ui.label(egui::RichText::new(title).strong());
    ui.add_space(6.0);
    ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap());
}

/// "2026-10-21 11:00 UTC · 6 days ago".
pub fn when(t: increart_core::id::Timestamp) -> String {
    let secs = (increart_core::id::now() - t).num_seconds().max(0);
    let ago = match secs {
        0..60 => "just now".to_string(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        86_400..172_800 => "yesterday".to_string(),
        _ => format!("{} days ago", secs / 86_400),
    };
    format!("{} · {ago}", t.format("%Y-%m-%d %H:%M UTC"))
}

/// `name` cut to `max` characters, "…" in place of the rest: a name in a
/// button (the whole of it on hover).
pub fn short_name(name: &str, max: usize) -> String {
    match name.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", name[..at].trim_end()),
        None => name.to_string(),
    }
}

/// A dialog's button row (DESIGN.md, Dialog buttons): `tools` (buttons
/// that don't finish it, and status) on the left, `actions` on the right,
/// laid out right to left: add the main action first, it goes in the corner,
/// and Cancel / Close last. When both don't fit across, the actions keep
/// the row and the tools go on one of their own under it (nothing clipped).
pub fn dialog_footer<L, R>(
    ui: &mut egui::Ui,
    tools: impl FnOnce(&mut egui::Ui) -> L,
    actions: impl FnOnce(&mut egui::Ui) -> R,
) -> (L, R) {
    use egui::{Align, Layout};
    const GAP: f32 = 16.0;
    // the two sides' widths as last drawn decide this frame's layout
    let id = ui.next_auto_id().with("dialog_footer");
    let (was_tools, was_actions) = ui.data(|d| d.get_temp::<(f32, f32)>(id)).unwrap_or_default();
    let width = ui.available_width();
    let fits = |t: f32, a: f32| t == 0.0 || t + GAP + a <= width;
    let across = fits(was_tools, was_actions);
    let ((l, wt), (r, wa)) = if across {
        ui.horizontal(|ui| {
            let left = ui.horizontal(|ui| (tools(ui), ui.min_rect().width())).inner;
            let right = ui.with_layout(Layout::right_to_left(Align::Center), |ui| (actions(ui), ui.min_rect().width()));
            (left, right.inner)
        })
        .inner
    } else {
        // in a row, as across: alone in the column it would fill its height
        let right = ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| (actions(ui), ui.min_rect().width())).inner
        });
        let left = ui.horizontal(|ui| (tools(ui), ui.min_rect().width())).inner;
        (left, right.inner)
    };
    ui.data_mut(|d| d.insert_temp(id, (wt, wa)));
    if fits(wt, wa) != across {
        ui.ctx().request_discard("dialog footer layout");
    }
    (l, r)
}

/// A dialog's button row with only actions ([`dialog_footer`]).
pub fn dialog_buttons<R>(ui: &mut egui::Ui, actions: impl FnOnce(&mut egui::Ui) -> R) -> R {
    dialog_footer(ui, |_| (), actions).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cut_for_a_button() {
        assert_eq!(short_name("KoboldCpp Vulkan2 + KoboldCpp Vulkan1", 24), "KoboldCpp Vulkan2 + Kobo…");
        assert_eq!(short_name("lib", 24), "lib");
        assert_eq!(short_name("ab cd", 3), "ab…", "no space before the …");
        assert_eq!(short_name("ééééé", 4), "éééé…");
    }

    #[test]
    fn aspect_ratios() {
        let a = |w, h| aspect(w, h);
        assert_eq!(
            [a(1024, 1024), a(1344, 896), a(896, 1344), a(1664, 928), a(1024, 1008)],
            ["square", "67% landscape", "67% portrait", "56% landscape", "98% landscape"]
        );
        assert_eq!(size_info(1344, 896), "1344 × 896 · 67% landscape · 1.20 MP");
    }
}
