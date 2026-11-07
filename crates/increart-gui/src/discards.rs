//! The Discarded views' selection (discarded images, projects, ideas):
//! click one, Ctrl+click more, Shift+click a range, drag a box on the empty
//! space, Ctrl+A all, Esc none; the bar then restores or deletes them all.
//! Kept in egui's memory per view (it's only what's highlighted).

use crate::widgets::{ACCENT, ic, ph};
use eframe::egui::{self, Key, Rect, RichText, Sense};

/// What the bar asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarAction {
    Restore,
    Delete,
    /// Every discarded one, not only the selected.
    Empty,
}

/// One view's selection: the items picked, in the order picked.
pub struct Picks<T> {
    id: egui::Id,
    pub sel: Vec<T>,
    /// The last one clicked, for Shift+click.
    anchor: Option<T>,
    rects: Vec<(T, Rect)>,
}

impl<T: Clone + PartialEq + Send + Sync + 'static> Picks<T> {
    /// The view's selection, less what's no longer shown (`all`).
    pub fn load(ui: &egui::Ui, key: &str, all: &[T]) -> Picks<T> {
        let id = egui::Id::unique(("discard_picks", key));
        let (mut sel, anchor): (Vec<T>, Option<T>) = ui.data(|d| d.get_temp(id)).unwrap_or_default();
        sel.retain(|x| all.contains(x));
        Picks { id, sel, anchor: anchor.filter(|a| all.contains(a)), rects: vec![] }
    }

    pub fn has(&self, x: &T) -> bool {
        self.sel.contains(x)
    }

    /// The empty space under the items: a click clears, a drag draws the
    /// box. Before the items.
    pub fn background(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let bg = ui.interact(ui.available_rect_before_wrap(), self.id.with("bg"), Sense::click_and_drag());
        if bg.clicked() {
            self.sel.clear();
        }
        bg
    }

    /// An item's click (and its place, for the box); `all` in view order.
    /// A right-click on one not selected selects it alone, so its menu acts
    /// on what's highlighted.
    pub fn item(&mut self, ui: &egui::Ui, resp: &egui::Response, x: &T, all: &[T]) {
        self.rects.push((x.clone(), resp.rect));
        if resp.secondary_clicked() && !self.has(x) {
            self.sel = vec![x.clone()];
            self.anchor = Some(x.clone());
        }
        if !resp.clicked() {
            return;
        }
        let m = ui.input(|i| i.modifiers);
        if m.shift
            && let Some(a) = &self.anchor
            && let (Some(i), Some(j)) = (all.iter().position(|y| y == a), all.iter().position(|y| y == x))
        {
            let (lo, hi) = (i.min(j), i.max(j));
            if !m.command {
                self.sel.clear();
            }
            for y in &all[lo..=hi] {
                if !self.sel.contains(y) {
                    self.sel.push(y.clone());
                }
            }
            return;
        }
        if m.command {
            if !self.sel.contains(x) {
                self.sel.push(x.clone());
            } else {
                self.sel.retain(|y| y != x);
            }
        } else {
            self.sel = vec![x.clone()];
        }
        self.anchor = Some(x.clone());
    }

    /// A selected item's outline and number.
    pub fn mark(&self, ui: &egui::Ui, rect: Rect, x: &T) {
        if let Some(n) = self.sel.iter().position(|y| y == x) {
            crate::tiles::mark_n(ui, rect, n);
        }
    }

    /// The box, Ctrl+A / Esc, and keeps the selection. `Del` asks for the
    /// selected to be deleted (returned as the bar's Delete).
    pub fn finish(mut self, ui: &egui::Ui, bg: &egui::Response, all: &[T]) -> Option<BarAction> {
        let base = |add: bool| if add { self.sel.clone() } else { vec![] };
        if let Some(s) = crate::tiles::band_of(ui, bg, &self.sel, &self.rects, base) {
            self.sel = s;
        }
        let mut action = None;
        if !ui.ctx().egui_wants_keyboard_input() {
            ui.input(|i| {
                if i.modifiers.command && i.key_pressed(Key::A) {
                    self.sel = all.to_vec();
                }
                if i.key_pressed(Key::Escape) {
                    self.sel.clear();
                }
                if i.key_pressed(Key::Delete) && !self.sel.is_empty() {
                    action = Some(BarAction::Delete);
                }
            });
        }
        ui.data_mut(|d| d.insert_temp(self.id, (self.sel, self.anchor)));
        action
    }
}

/// The top of a Discarded view: what it is, "N selected · Restore · Delete
/// permanently… · Clear" while any are, and **Empty discarded…** (`what`:
/// "image", "project", "idea").
pub fn bar(ui: &mut egui::Ui, key: &str, hint: &str, selected: usize, total: usize, what: &str) -> Option<BarAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        if selected == 0 {
            ui.weak(hint);
            ui.weak("· click, Ctrl+click, Shift+click or drag a box to select");
        } else {
            let s = if selected == 1 { "" } else { "s" };
            ui.label(RichText::new(format!("{selected} {what}{s} selected")).color(ACCENT).strong());
            if ui.button(ic(ph::ARROW_COUNTER_CLOCKWISE, "Restore")).clicked() {
                action = Some(BarAction::Restore);
            }
            if ui.button(ic(ph::TRASH, "Delete permanently…")).on_hover_text("Del").clicked() {
                action = Some(BarAction::Delete);
            }
            ui.weak("Ctrl+A: all · Esc: none");
        }
        crate::widgets::right_group(ui, &format!("{key}_empty"), |ui| {
            let s = if total == 1 { "" } else { "s" };
            if ui
                .button(ic(ph::TRASH, "Empty discarded…"))
                .on_hover_text(format!("delete all {total} discarded {what}{s} permanently (asks first)"))
                .clicked()
            {
                action = Some(BarAction::Empty);
            }
        });
    });
    action
}
