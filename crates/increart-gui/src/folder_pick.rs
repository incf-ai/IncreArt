//! Choosing a folder in a dialog's left column: a filter, the tree, and New
//! folder under it (Save to input library, New project from ideas).

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, ScrollArea};

use crate::widgets::{ic, ph};

/// What the column shows.
pub struct Picker<'a> {
    /// Names the dialog: its focus flags, the tour's handle on the filter.
    pub id: &'static str,
    /// The folders, relative to the root.
    pub folders: &'a [PathBuf],
    /// The root's icon and name ("input library", "projects").
    pub root: (&'static str, &'static str),
    pub selected: &'a Path,
    pub filter: &'a str,
    /// The new folder to make in `selected` (`None`: none).
    pub new_folder: Option<&'a str>,
    /// New folder's name to start with.
    pub new_name: &'a str,
    /// What its folders are called: "folder", "collection".
    pub noun: &'static str,
}

/// What changed this frame.
#[derive(Default)]
pub struct Picked {
    pub filter: Option<String>,
    pub select: Option<PathBuf>,
    /// The new folder (`Some(None)`: none any more).
    pub new_folder: Option<Option<String>>,
    /// Enter left one of its fields (it doesn't confirm the dialog).
    pub field_enter: bool,
}

/// The folder column, `height` tall for the tree. The new folder's name
/// takes the focus once, as New folder is clicked.
pub fn show(ui: &mut egui::Ui, p: &Picker, height: f32) -> Picked {
    let mut out = Picked::default();
    let asked = egui::Id::unique(("folder_pick_new_asked", p.id));
    let mut heading = p.noun.to_string();
    heading[..1].make_ascii_uppercase();
    ui.label(RichText::new(heading).strong());
    let mut filter = p.filter.to_string();
    ui.horizontal(|ui| {
        ui.label(ph::MAGNIFYING_GLASS);
        let r =
            ui.add(egui::TextEdit::singleline(&mut filter).hint_text(format!("filter {}s", p.noun)).desired_width(f32::INFINITY));
        out.field_enter |= r.lost_focus();
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(("tour_filter", p.id)), r.rect));
    });
    if filter != p.filter {
        out.filter = Some(filter.clone());
    }
    let words: Vec<String> = filter.to_lowercase().split_whitespace().map(String::from).collect();
    // shown: matches and their ancestors (the selected folder always)
    let matches = |f: &Path| words.iter().all(|w| f.to_string_lossy().to_lowercase().contains(w));
    let shown =
        |f: &Path| words.is_empty() || f == p.selected || p.folders.iter().any(|g| g.starts_with(f) && matches(g));
    // the first frame shown (none the frame before): the folder it starts
    // on is scrolled to, down a list longer than the column
    let pass = ui.ctx().cumulative_pass_nr();
    let last = egui::Id::unique(("folder_pick_pass", p.id));
    let before: Option<u64> = ui.ctx().data_mut(|d| d.get_temp(last));
    ui.ctx().data_mut(|d| d.insert_temp(last, pass));
    let opening = before.is_none_or(|was| was + 1 < pass);
    let mut row = |ui: &mut egui::Ui, path: &Path, label: &str, depth: usize| {
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 16.0);
            let selected = p.selected == path;
            let icon = match depth {
                0 => p.root.0,
                _ if selected => ph::FOLDER_OPEN,
                _ => ph::FOLDER,
            };
            let r = ui.selectable_label(selected, ic(icon, label));
            if selected && opening {
                r.scroll_to_me(Some(egui::Align::Center));
            }
            if r.clicked() {
                out.select = Some(path.to_path_buf());
            }
        });
    };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ScrollArea::vertical().min_scrolled_height(height).max_height(height).auto_shrink([false, false]).show(
            ui,
            |ui| {
                row(ui, Path::new(""), p.root.1, 0);
                let mut sorted: Vec<&PathBuf> = p.folders.iter().filter(|f| shown(f)).collect();
                sorted.sort_by_key(|f| f.to_string_lossy().to_lowercase());
                for f in sorted {
                    let label = f.file_name().unwrap_or_default().to_string_lossy();
                    row(ui, f, &label, f.components().count());
                }
                if !words.is_empty() && !p.folders.iter().any(|g| matches(g)) {
                    ui.weak("No folder matches.");
                }
            },
        );
    });
    let here = match p.selected.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => p.root.1.to_string(),
    };
    match p.new_folder {
        None => {
            if ui
                .button(ic(ph::FOLDER_PLUS, &format!("New {}", p.noun)))
                .on_hover_text(format!("in “{here}”"))
                .clicked()
            {
                ui.ctx().data_mut(|m| m.insert_temp(asked, ()));
                out.new_folder = Some(Some(p.new_name.to_string()));
            }
        }
        Some(new) => {
            ui.weak(format!("New {} in “{here}”", p.noun));
            ui.horizontal(|ui| {
                ui.label(ph::FOLDER_PLUS);
                let mut name = new.to_string();
                let r = ui.add(
                    egui::TextEdit::singleline(&mut name).hint_text(format!("{} name", p.noun)).desired_width(220.0),
                );
                if ui.ctx().data_mut(|m| m.remove_temp::<()>(asked)).is_some() {
                    r.request_focus();
                }
                out.field_enter |= r.lost_focus();
                if name != new {
                    out.new_folder = Some(Some(name));
                }
                if ui.small_button(ph::X).on_hover_text("no new folder").clicked() {
                    out.new_folder = Some(None);
                }
            });
        }
    }
    out
}

/// True on the first frame a dialog is shown (`open`), once: what takes the
/// focus as it opens. Call it every frame, open or not.
pub fn opening(ctx: &egui::Context, id: &'static str, open: bool) -> bool {
    let key = egui::Id::unique(("dialog_opened", id));
    ctx.data_mut(|m| {
        if !open {
            m.remove::<()>(key);
            return false;
        }
        let first = m.get_temp::<()>(key).is_none();
        m.insert_temp(key, ());
        first
    })
}

/// A line between a dialog's columns, `height` tall (a separator there
/// grows with the modal, which grows with it).
pub fn column_line(ui: &mut egui::Ui, height: f32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, height), egui::Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
}
