//! The folder view (DESIGN.md, Folder view): a file manager over the
//! library's on-disk layout, for the Folder subtabs of Projects, the Output
//! library (collections and the virtual smart folders), the Input library
//! and the Idea inbox.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontId, Key, Panel, RichText, ScrollArea, Sense, Stroke, Vec2};
use increart_core::{ImageRef, Verdict};
use increart_gui_core::{AppState, EntryKind, FvEntry, FvMode, FvPlace, Msg, OutputSource, SelectHow, is_smart};

use crate::textures::Textures;
use crate::widgets::{
    ACCENT, BAD, Dragged, GOOD, NEUTRAL, drag_source, drop_target, focus_until_done, ic, icon2, image_cover, image_fit,
    ph,
};

fn glyph(k: &EntryKind) -> &'static str {
    match k {
        EntryKind::Folder => ph::FOLDER,
        EntryKind::Project => ph::ROCKET_LAUNCH,
        EntryKind::Run => ph::FILM_STRIP,
        EntryKind::Image(_) => ph::IMAGE,
        EntryKind::Meta => ph::FILE_TEXT,
        EntryKind::File => ph::FILE,
        EntryKind::Smart => ph::SPARKLE,
        EntryKind::Idea(_) => ph::LIGHTBULB,
    }
}

/// An entry kind's icon colours: (fill, outline).
fn kind_colors(k: &EntryKind) -> (Color32, Color32) {
    match k {
        EntryKind::Folder => (FOLDER_FILL, FOLDER_STROKE),
        EntryKind::Project => (PROJECT_FILL, PROJECT_STROKE),
        EntryKind::Run => (RUN_FILL, RUN_STROKE),
        EntryKind::Smart => (SMART_FILL, SMART_STROKE),
        EntryKind::Idea(_) => (IDEA_FILL, IDEA_STROKE),
        _ => (PLAIN_FILL, PLAIN_STROKE),
    }
}

const FOLDER_FILL: Color32 = Color32::from_rgb(48, 74, 108);
const FOLDER_STROKE: Color32 = Color32::from_rgb(140, 186, 240);
const PROJECT_FILL: Color32 = Color32::from_rgb(92, 62, 24);
const PROJECT_STROKE: Color32 = Color32::from_rgb(240, 180, 90);
const RUN_FILL: Color32 = Color32::from_rgb(68, 48, 98);
const RUN_STROKE: Color32 = Color32::from_rgb(196, 166, 246);
const IDEA_FILL: Color32 = Color32::from_rgb(92, 40, 100);
const IDEA_STROKE: Color32 = Color32::from_rgb(232, 150, 245);
const SMART_FILL: Color32 = Color32::from_rgb(28, 82, 70);
const SMART_STROKE: Color32 = Color32::from_rgb(110, 220, 190);
const PLAIN_FILL: Color32 = Color32::from_gray(52);
const PLAIN_STROKE: Color32 = Color32::from_gray(175);

/// A preview image as a small framed card.
fn card(p: &egui::Painter, rect: egui::Rect, img: &ImageRef, root: &Path, tex: &mut Textures) {
    p.rect_filled(rect.expand(1.5), 3.0, Color32::from_gray(200));
    p.rect_filled(rect, 2.0, Color32::from_gray(30));
    image_fit(p, rect, tex.thumb(root, img).as_ref());
}

/// An entry's icon: an image's thumbnail; a project's rocket over its
/// thumbnail (cropped to fill the tile); for folders and runs, up to three
/// of their images sticking out from behind the icon.
fn draw_icon(p: &egui::Painter, r: egui::Rect, e: &FvEntry, previews: &[ImageRef], root: &Path, tex: &mut Textures) {
    let s = r.width();
    match &e.kind {
        EntryKind::Image(img) => image_fit(p, r, tex.thumb(root, img).as_ref()),
        EntryKind::Folder | EntryKind::Project | EntryKind::Run | EntryKind::Smart | EntryKind::Idea(_) => {
            let (fill, stroke) = kind_colors(&e.kind);
            // smart folders and ideas: a folder, with a badge
            let g = match e.kind {
                EntryKind::Project | EntryKind::Run => glyph(&e.kind),
                _ => ph::FOLDER,
            };
            if e.kind == EntryKind::Project {
                if let Some(img) = previews.first() {
                    image_cover(p, r, tex.thumb(root, img).as_ref());
                }
            } else {
                // back cards first, so the first preview is in front
                for (i, img) in previews.iter().enumerate().rev() {
                    let dx = [0.0, 0.16, -0.16][i] * s;
                    let dy = [0.0, 0.04, 0.06][i] * s;
                    let c = egui::Rect::from_center_size(
                        r.center_top() + Vec2::new(dx, 0.34 * s + dy),
                        Vec2::splat(0.46 * s),
                    );
                    card(p, c, img, root, tex);
                }
            }
            if e.kind == EntryKind::Project && !previews.is_empty() {
                // centred over the thumbnail, 15% transparent so it shows through
                icon2(p, r.center(), g, s * 0.8, fill.gamma_multiply(0.85), stroke.gamma_multiply(0.85));
            } else {
                // folders and runs sit low, below the images sticking out
                let center = r.center() + Vec2::new(0.0, if previews.is_empty() { 0.0 } else { 0.14 * s });
                icon2(p, center, g, s * 0.8, fill, stroke);
                if matches!(e.kind, EntryKind::Smart | EntryKind::Idea(_)) {
                    icon2(p, center + Vec2::new(0.0, 0.06 * s), glyph(&e.kind), s * 0.3, stroke, fill);
                }
            }
        }
        k => icon2(p, r.center(), glyph(k), s * 0.6, PLAIN_FILL, PLAIN_STROKE),
    }
}

fn kind_name(k: &EntryKind) -> &'static str {
    match k {
        EntryKind::Folder => "folder",
        EntryKind::Project => "project",
        EntryKind::Run => "run",
        EntryKind::Image(_) => "image link",
        EntryKind::Meta => "metadata",
        EntryKind::File => "file",
        EntryKind::Smart => "smart folder",
        EntryKind::Idea(_) => "idea",
    }
}

fn size_text(e: &FvEntry) -> String {
    if e.kind.is_dir() {
        format!("{} item{}", e.size, if e.size == 1 { "" } else { "s" })
    } else if e.size >= 1 << 20 {
        format!("{:.1} MB", e.size as f64 / (1 << 20) as f64)
    } else {
        format!("{} KB", e.size.div_ceil(1024))
    }
}

fn select_how(ui: &egui::Ui) -> SelectHow {
    ui.input(|i| {
        if i.modifiers.shift {
            SelectHow::Range
        } else if i.modifiers.command {
            SelectHow::Toggle
        } else {
            SelectHow::Replace
        }
    })
}

/// Copy / cut also put the paths on the system clipboard as text: egui only
/// reports Ctrl+V when the clipboard isn't empty.
fn copy_paths_text(ui: &egui::Ui, st: &AppState) {
    let fv = &st.folder_view;
    let text: Vec<String> = fv.selected.iter().map(|n| fv.path.join(n).display().to_string()).collect();
    ui.ctx().copy_text(text.join("\n"));
}

fn keys(ui: &egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    if ui.ctx().egui_wants_keyboard_input() || st.confirm.is_some() || st.preview.is_some() {
        return;
    }
    let fv = &st.folder_view;
    let mut copy_text = false;
    // read before `ui.input`: the context's lock isn't re-entrant
    let cols = ui.ctx().data(|d| d.get_temp::<usize>(egui::Id::unique("fv_cols"))).unwrap_or(1) as i32;
    ui.input(|i| {
        if i.key_pressed(Key::Backspace) || (i.modifiers.alt && i.key_pressed(Key::ArrowUp)) {
            out.push(Msg::FvUp);
        }
        // past the folder's own history: the app's (the tab before, …)
        if (i.modifiers.alt && i.key_pressed(Key::ArrowLeft)) || i.pointer.button_pressed(egui::PointerButton::Extra1) {
            out.push(if fv.back.is_empty() { Msg::NavBack } else { Msg::FvBack });
        }
        if (i.modifiers.alt && i.key_pressed(Key::ArrowRight)) || i.pointer.button_pressed(egui::PointerButton::Extra2)
        {
            out.push(if fv.forward.is_empty() { Msg::NavForward } else { Msg::FvForward });
        }
        if i.key_pressed(Key::Enter)
            && fv.selected.len() == 1
            && let Some(n) = fv.selected.first()
        {
            out.push(Msg::FvOpen(n.clone()));
        }
        if i.key_pressed(Key::F2) {
            out.push(Msg::FvRename);
        }
        if i.key_pressed(Key::Delete) {
            out.push(Msg::FvDelete);
        }
        if i.key_pressed(Key::Escape) {
            out.push(Msg::FvClearSelection);
        }
        if i.modifiers.command && i.key_pressed(Key::A) {
            out.push(Msg::FvSelectAll);
        }
        if i.modifiers.command && i.key_pressed(Key::H) {
            out.push(Msg::FvShowMeta(!fv.show_meta));
        }
        if i.modifiers.command && i.key_pressed(Key::L) {
            out.push(Msg::FvPathEdit(Some(fv.path.display().to_string())));
        }
        // arrows move the cursor (Shift: select the range); Up / Down by a row
        if !i.modifiers.alt && !i.modifiers.command {
            let row = if fv.mode == FvMode::Details { 1 } else { cols };
            for (key, by) in [
                (Key::ArrowLeft, -1),
                (Key::ArrowRight, 1),
                (Key::ArrowUp, -row),
                (Key::ArrowDown, row),
                (Key::Home, i32::MIN / 2),
                (Key::End, i32::MAX / 2),
            ] {
                if i.key_pressed(key) {
                    out.push(Msg::FvMoveCursor { by, extend: i.modifiers.shift });
                }
            }
        }
        // egui delivers Ctrl+C / X / V as clipboard events, not key presses.
        for e in &i.events {
            match e {
                egui::Event::Copy => {
                    out.push(Msg::FvCopy);
                    copy_text = true;
                }
                egui::Event::Cut => {
                    out.push(Msg::FvCut);
                    copy_text = true;
                }
                egui::Event::Paste(_) => out.push(Msg::FvPaste(None)),
                // typing jumps to the first name starting with the letters
                egui::Event::Text(t) if !i.modifiers.command && !t.trim().is_empty() => {
                    out.push(Msg::FvTypeAhead(t.clone()))
                }
                _ => {}
            }
        }
    });
    if copy_text {
        copy_paths_text(ui, st);
    }
}

fn toolbar(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let fv = &st.folder_view;
    // wraps: the view buttons go under the path when they don't fit beside it
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!fv.back.is_empty(), egui::Button::new(ph::ARROW_LEFT))
            .on_hover_text("back (Alt+←)")
            .clicked()
        {
            out.push(Msg::FvBack);
        }
        if ui
            .add_enabled(!fv.forward.is_empty(), egui::Button::new(ph::ARROW_RIGHT))
            .on_hover_text("forward (Alt+→)")
            .clicked()
        {
            out.push(Msg::FvForward);
        }
        if ui.add_enabled(fv.path != fv.root, egui::Button::new(ph::ARROW_UP)).on_hover_text("up (Backspace)").clicked()
        {
            out.push(Msg::FvUp);
        }
        // path bar: clickable segments, or an editable path (Ctrl+L)
        match &fv.path_edit {
            Some(text) => {
                let mut t = text.clone();
                let r = ui.add(egui::TextEdit::singleline(&mut t).desired_width(420.0));
                focus_until_done(&r);
                if t != *text {
                    out.push(Msg::FvPathEdit(Some(t.clone())));
                }
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    out.push(Msg::FvPathEdit(None));
                } else if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    out.push(Msg::FvGo(PathBuf::from(t.trim())));
                } else if r.lost_focus() {
                    out.push(Msg::FvPathEdit(None));
                }
            }
            None => {
                let mut acc = PathBuf::new();
                for (i, c) in fv.path.components().enumerate() {
                    acc.push(c);
                    if i > 0 {
                        ui.weak("›");
                    }
                    let label = c.as_os_str().to_string_lossy();
                    if acc.starts_with(&fv.root) {
                        let r = ui.link(label);
                        drop_target(ui, &r, &acc, out);
                        if r.clicked() {
                            out.push(Msg::FvGo(acc.clone()));
                        }
                    } else {
                        ui.weak(label);
                    }
                }
                if ui.small_button(ph::PENCIL_SIMPLE).on_hover_text("type a path (Ctrl+L)").clicked() {
                    out.push(Msg::FvPathEdit(Some(fv.path.display().to_string())));
                }
            }
        }
        crate::widgets::right_group(ui, "fv_right", |ui| {
            if fv.mode == FvMode::Icons {
                let mut size = fv.icon_size;
                if ui.add(egui::Slider::new(&mut size, 64.0..=256.0).show_value(false)).changed() {
                    out.push(Msg::FvIconSize(size));
                }
            }
            if ui.selectable_label(fv.mode == FvMode::Icons, ic(ph::SQUARES_FOUR, "Icons")).clicked() {
                out.push(Msg::FvMode(FvMode::Icons));
            }
            if ui.selectable_label(fv.mode == FvMode::Compact, ic(ph::ROWS, "Compact")).clicked() {
                out.push(Msg::FvMode(FvMode::Compact));
            }
            if ui.selectable_label(fv.mode == FvMode::Details, ic(ph::LIST, "Details")).clicked() {
                out.push(Msg::FvMode(FvMode::Details));
            }
            let mut meta = fv.show_meta;
            if ui.checkbox(&mut meta, "metadata").on_hover_text("show project.rsn / run.rsn … (Ctrl+H)").changed() {
                out.push(Msg::FvShowMeta(meta));
            }
            if ui.button(ph::ARROWS_CLOCKWISE).on_hover_text("refresh").clicked() {
                out.push(Msg::FvRefresh);
            }
            if ui.button(ph::FOLDER_OPEN).on_hover_text("open in the system file manager").clicked() {
                out.push(Msg::ShowInFileManager(fv.path.clone()));
            }
        });
    });
}

/// Right-click on an entry: what it offers depends on the tree (projects or
/// collections) and on the virtual `smart/` folders being read-only.
fn entry_menu(ui: &mut egui::Ui, st: &AppState, e: &FvEntry, out: &mut Vec<Msg>) {
    let fv = &st.folder_view;
    let path = fv.path.join(&e.name);
    let smart = is_smart(&path);
    let collections = fv.place == FvPlace::Collections;
    let ideas = matches!(fv.place, FvPlace::InputLibrary | FvPlace::Inbox);
    if !fv.selected.contains(&e.name) {
        out.push(Msg::FvSelect(e.name.clone(), SelectHow::Replace));
    }
    let item = |ui: &mut egui::Ui, out: &mut Vec<Msg>, label: String, msg: Msg| {
        if ui.button(label).clicked() {
            out.push(msg);
            ui.close();
        }
    };
    item(ui, out, ic(ph::ARROW_SQUARE_OUT, "Open"), Msg::FvOpen(e.name.clone()));
    if let (EntryKind::Image(img), Some(root)) = (&e.kind, &st.library) {
        crate::widgets::image_menu_items(ui, root, img, out);
    }
    // on disk: the entry itself; a smart folder's image: its store file
    let on_disk = match &e.kind {
        EntryKind::Image(img) if smart => st.library.as_ref().map(|root| PathBuf::from(img.rel_path(root))),
        _ if smart => None,
        _ => Some(path.clone()),
    };
    if let Some(p) = on_disk {
        item(ui, out, ic(ph::FOLDER_OPEN, "Show in file manager"), Msg::ShowInFileManager(p));
    }
    if matches!(e.kind, EntryKind::Folder | EntryKind::Project | EntryKind::Run)
        && let Some(source) = outputs_of(st, &path)
        && crate::review::show_outputs_button(ui, st, source, out)
    {
        ui.close();
    }
    ui.separator();
    match &e.kind {
        EntryKind::Idea(id) => {
            item(ui, out, ic(ph::PENCIL_SIMPLE, "Edit idea…"), Msg::OpenIdea(id.clone()));
            if fv.place == FvPlace::InputLibrary {
                item(ui, out, ic(ph::COPY, "Copy   Ctrl+C"), Msg::FvCopy);
                item(ui, out, ic(ph::SCISSORS, "Cut   Ctrl+X"), Msg::FvCut);
            }
        }
        EntryKind::Folder if fv.place == FvPlace::Inbox => {}
        EntryKind::Folder | EntryKind::Project if !smart => {
            item(ui, out, ic(ph::PENCIL_SIMPLE, "Rename   F2"), Msg::FvRename);
            if ui.button(ic(ph::COPY, "Copy   Ctrl+C")).clicked() {
                out.push(Msg::FvCopy);
                copy_paths_text(ui, st);
                ui.close();
            }
            if ui.button(ic(ph::SCISSORS, "Cut   Ctrl+X")).clicked() {
                out.push(Msg::FvCut);
                copy_paths_text(ui, st);
                ui.close();
            }
            if e.kind == EntryKind::Folder && st.fv_clipboard.is_some() {
                item(ui, out, ic(ph::CLIPBOARD, "Paste into"), Msg::FvPaste(Some(e.name.clone())));
            }
        }
        EntryKind::Image(_) if ideas => {
            if let Some(Ok(id)) = fv.path.file_name().map(|n| n.to_string_lossy().parse()) {
                item(ui, out, ic(ph::PENCIL_SIMPLE, "Edit idea…"), Msg::OpenIdea(id));
            }
        }
        EntryKind::Image(img) => {
            // rating goes through the image's project link
            let link = if collections { st.output_link(img).map(|o| o.path.clone()) } else { Some(path.clone()) };
            if let Some(link) = link {
                for (label, v, c) in [
                    ("Rate good", Some(Verdict::Good), GOOD),
                    ("Rate neutral", Some(Verdict::Neutral), NEUTRAL),
                    ("Rate bad", Some(Verdict::Bad), BAD),
                    ("Back to unreviewed", None, ACCENT),
                ] {
                    if ui.button(RichText::new(label).color(c)).clicked() {
                        out.push(Msg::ReviewLink(link.clone(), v));
                        ui.close();
                    }
                }
            }
            if collections {
                ui.separator();
                let hint = if smart { "then paste into a collection" } else { "then paste into another collection" };
                if ui.button(ic(ph::COPY, "Copy   Ctrl+C")).on_hover_text(hint).clicked() {
                    out.push(Msg::FvCopy);
                    ui.close();
                }
            }
            if !smart {
                let hint = if collections {
                    "then paste into another collection"
                } else {
                    "then paste into another review folder"
                };
                if ui.button(ic(ph::SCISSORS, "Cut   Ctrl+X")).on_hover_text(hint).clicked() {
                    out.push(Msg::FvCut);
                    ui.close();
                }
            }
            item(ui, out, ic(ph::EXPORT, "Export…"), Msg::OpenExport(vec![img.clone()], e.name.clone()));
        }
        _ => {}
    }
    let deletable = match fv.place {
        _ if smart => false,
        FvPlace::Inbox => false,
        FvPlace::InputLibrary => matches!(e.kind, EntryKind::Folder | EntryKind::Idea(_)),
        _ => matches!(e.kind, EntryKind::Folder | EntryKind::Project | EntryKind::Image(_)),
    };
    if deletable {
        ui.separator();
        let label = match (fv.place, &e.kind) {
            (FvPlace::Collections, EntryKind::Image(_)) => "Remove from collection…   Del",
            (FvPlace::InputLibrary, _) => "Discard…   Del",
            _ => "Delete…   Del",
        };
        item(ui, out, ic(ph::TRASH, label), Msg::FvDelete);
    }
}

/// The kept images a Projects path stands for: a run (`…/runs/<id>`), the
/// project it's in, or a project folder.
fn outputs_of(st: &AppState, path: &Path) -> Option<OutputSource> {
    if st.folder_view.place != FvPlace::Projects {
        return None;
    }
    if let Some(p) = st.index.projects.iter().find(|p| path.starts_with(&p.dir)) {
        let run = path.strip_prefix(p.dir.join("runs")).ok().and_then(|r| r.iter().next());
        if let Some(Ok(id)) = run.map(|r| r.to_string_lossy().parse()) {
            return Some(OutputSource::Run(id));
        }
        return Some(OutputSource::Project(p.project.id.clone()));
    }
    path.strip_prefix("projects").ok().map(|f| OutputSource::ProjectFolder(f.to_path_buf()))
}

/// Right-click on empty space.
fn background_menu(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let fv = &st.folder_view;
    let smart = is_smart(&fv.path);
    let in_project = st.index.projects.iter().any(|p| fv.path.starts_with(&p.dir));
    let in_idea = st.index.ideas.iter().any(|i| fv.path.starts_with(&i.dir));
    if smart {
        ui.weak("Smart folders are read-only");
    } else if fv.place == FvPlace::Inbox {
        if fv.path.starts_with("input/inbox_drop") && ui.button(ic(ph::FILE_ARROW_DOWN, "Import now")).clicked() {
            out.push(Msg::ImportDropNow);
            ui.close();
        }
    } else if !in_project && !in_idea {
        let label = if fv.place == FvPlace::Collections { "New collection" } else { "New folder" };
        if ui.button(ic(ph::FOLDER_PLUS, label)).clicked() {
            out.push(Msg::FvNewFolder);
            ui.close();
        }
        if fv.place == FvPlace::Projects && ui.button(ic(ph::ROCKET_LAUNCH, "New project")).clicked() {
            out.push(Msg::FvNewProject);
            ui.close();
        }
    }
    if !smart && st.fv_clipboard.is_some() && ui.button(ic(ph::CLIPBOARD, "Paste   Ctrl+V")).clicked() {
        out.push(Msg::FvPaste(None));
        ui.close();
    }
    if ui
        .button(ic(ph::SQUARES_FOUR, "Tile the clipboard…"))
        .on_hover_text("its image, or the images it names (copied here or in a file manager)")
        .clicked()
    {
        let tiles = crate::tiles::clipboard_tiles(st, out);
        if !tiles.is_empty() {
            out.push(Msg::TileStart(tiles));
        }
        ui.close();
    }
    if let Some(source) = outputs_of(st, &fv.path)
        && crate::review::show_outputs_button(ui, st, source, out)
    {
        ui.close();
    }
    if !smart && ui.button(ic(ph::FOLDER_OPEN, "Open in file manager")).clicked() {
        out.push(Msg::ShowInFileManager(fv.path.clone()));
        ui.close();
    }
    if ui.button(ic(ph::ARROWS_CLOCKWISE, "Refresh")).clicked() {
        out.push(Msg::FvRefresh);
        ui.close();
    }
}

pub fn view(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let Some(root) = st.library.clone() else { return };
    keys(ui, st, out);
    let fv = &st.folder_view;
    Panel::top("fv_toolbar").show(ui, |ui| toolbar(ui, st, out));
    Panel::bottom("fv_status").show(ui, |ui| {
        ui.horizontal(|ui| {
            let shown = fv.shown();
            ui.weak(format!("{} item(s)", shown.len()));
            if !fv.selected.is_empty() {
                let bytes: u64 =
                    shown.iter().filter(|e| fv.selected.contains(&e.name) && !e.kind.is_dir()).map(|e| e.size).sum();
                ui.weak(format!("· {} selected ({} KB)", fv.selected.len(), bytes.div_ceil(1024)));
            }
            if let Some(c) = &st.fv_clipboard {
                ui.weak(format!("· {} {} on the clipboard", c.paths.len(), if c.cut { "cut" } else { "copied" }));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // cut short (whole on hover) when the panel is narrow
                let text = if is_smart(&fv.path) {
                    format!("{} (virtual: computed from the library, nothing on disk)", fv.path.display())
                } else {
                    root.join(&fv.path).display().to_string()
                };
                ui.add(egui::Label::new(RichText::new(text).weak()).truncate());
            });
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        // Empty space: click clears the selection, drag selects with a
        // rubber band, right-click opens its own menu. Registered first, so
        // the entries drawn after it get their clicks and drags.
        let bg = ui.interact(ui.max_rect(), ui.scope_id().with("fv_bg"), Sense::click_and_drag());
        if bg.clicked() {
            out.push(Msg::FvClearSelection);
        }
        bg.context_menu(|ui| background_menu(ui, st, out));
        if fv.listed.as_ref() != Some(&fv.path) && fv.entries.is_empty() {
            ui.spinner();
            return;
        }
        let shown = fv.shown();
        if shown.is_empty() {
            ui.weak(match fv.place {
                _ if is_smart(&fv.path) => "No images here.",
                FvPlace::Collections => "Empty. Right-click for New collection; paste or drag images here.",
                FvPlace::InputLibrary => "Empty folder. Right-click for New folder; drag ideas here.",
                FvPlace::Inbox => "Nothing here.",
                FvPlace::Projects => "Empty folder. Right-click for New folder / New project.",
            });
            return;
        }
        // scroll to the cursor when the keys moved it
        let last = egui::Id::unique("fv_last_cursor");
        let reveal = fv.cursor.clone().filter(|c| ui.ctx().data(|d| d.get_temp::<String>(last)).as_ref() != Some(c));
        if let Some(c) = &fv.cursor {
            ui.ctx().data_mut(|d| d.insert_temp(last, c.clone()));
        }
        let mut f = Drawn { rects: vec![], reveal };
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| match fv.mode {
            FvMode::Icons => icons(ui, st, &root, tex, &shown, &mut f, out),
            FvMode::Compact => compact(ui, st, &root, tex, &shown, &mut f, out),
            FvMode::Details => details(ui, st, &shown, &mut f, out),
        });
        rubber_band(ui, st, &bg, &f.rects, out);
        // Empty space takes drops into this folder (after the entries, so a
        // drop on a folder entry goes there), when they come from elsewhere.
        let from_elsewhere = egui::DragAndDrop::payload::<Dragged>(ui.ctx())
            .is_some_and(|d| d.0.iter().any(|p| p.parent() != Some(fv.path.as_path())));
        if from_elsewhere {
            drop_target(ui, &bg, &fv.path, out);
        }
    });
}

/// What the entries drawn this frame tell the rest of the view.
struct Drawn {
    /// Where each entry is, for the rubber band.
    rects: Vec<(String, egui::Rect)>,
    /// The entry to scroll to (the cursor, just moved).
    reveal: Option<String>,
}

/// Dragging on empty space selects what the band touches (Ctrl adds to
/// the selection).
fn rubber_band(ui: &egui::Ui, st: &AppState, bg: &egui::Response, rects: &[(String, egui::Rect)], out: &mut Vec<Msg>) {
    type Band = (egui::Pos2, BTreeSet<String>);
    let id = egui::Id::unique("fv_band");
    let fv = &st.folder_view;
    if bg.drag_started()
        && let Some(origin) = bg.interact_pointer_pos()
    {
        let base = if ui.input(|i| i.modifiers.command) { fv.selected.clone() } else { BTreeSet::new() };
        ui.ctx().data_mut(|d| d.insert_temp::<Band>(id, (origin, base)));
    }
    if bg.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Band>(id));
        return;
    }
    let (Some((origin, base)), Some(now)) = (ui.ctx().data(|d| d.get_temp::<Band>(id)), ui.ctx().pointer_latest_pos())
    else {
        return;
    };
    if !bg.dragged() {
        return;
    }
    let band = egui::Rect::from_two_pos(origin, now);
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("fv_band_paint")));
    p.rect_filled(band, 2.0, Color32::from_rgba_unmultiplied(90, 160, 255, 40));
    p.rect_stroke(band, 2.0, Stroke::new(1.0, ACCENT), egui::StrokeKind::Inside);
    let mut sel = base;
    sel.extend(rects.iter().filter(|(_, r)| r.intersects(band)).map(|(n, _)| n.clone()));
    if sel != fv.selected {
        out.push(Msg::FvSelectSet(sel));
    }
}

/// Drag an entry (with the rest of the selection, if it is selected); drop
/// onto a folder entry.
fn entry_dnd(ui: &egui::Ui, st: &AppState, e: &FvEntry, resp: &egui::Response, out: &mut Vec<Msg>) -> egui::Response {
    let fv = &st.folder_view;
    let movable = |k: &EntryKind| {
        matches!(k, EntryKind::Folder | EntryKind::Project | EntryKind::Image(_) | EntryKind::Idea(_))
            && fv.place != FvPlace::Inbox
    };
    let resp = if movable(&e.kind) {
        drag_source(resp, &fv.path.join(&e.name), || {
            if fv.selected.contains(&e.name) {
                fv.shown()
                    .into_iter()
                    .filter(|x| fv.selected.contains(&x.name) && movable(&x.kind))
                    .map(|x| fv.path.join(&x.name))
                    .collect()
            } else {
                out.push(Msg::FvSelect(e.name.clone(), SelectHow::Replace));
                vec![fv.path.join(&e.name)]
            }
        })
    } else {
        resp.clone()
    };
    if e.kind == EntryKind::Folder {
        drop_target(ui, &resp, &fv.path.join(&e.name), out);
    }
    resp
}

/// What every entry does, in any mode: hover text, select, open, menu,
/// drag and drop; and it's remembered for the rubber band.
fn entry_response(ui: &egui::Ui, st: &AppState, e: &FvEntry, resp: egui::Response, f: &mut Drawn, out: &mut Vec<Msg>) {
    let resp = resp.on_hover_text(format!("{}\n{} · {} · {}", e.name, kind_name(&e.kind), e.info, size_text(e)));
    let resp = entry_dnd(ui, st, e, &resp, out);
    if resp.clicked() {
        out.push(Msg::FvSelect(e.name.clone(), select_how(ui)));
    }
    if resp.double_clicked() {
        out.push(Msg::FvOpen(e.name.clone()));
    }
    if f.reveal.as_ref() == Some(&e.name) {
        resp.scroll_to_me(None);
    }
    f.rects.push((e.name.clone(), resp.rect));
    resp.context_menu(|ui| entry_menu(ui, st, e, out));
}

/// Selected / hovered background of an entry.
fn entry_bg(p: &egui::Painter, rect: egui::Rect, selected: bool, hovered: bool) {
    if selected {
        p.rect_filled(rect, 4.0, Color32::from_rgba_unmultiplied(90, 160, 255, 50));
        p.rect_stroke(rect, 4.0, Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
    } else if hovered {
        p.rect_filled(rect, 4.0, Color32::from_gray(40));
    }
}

/// How many entries fit in a row of `item` wide entries (for Up / Down).
fn remember_columns(ui: &egui::Ui, item: f32, spacing: f32) {
    let cols = ((ui.available_width() + spacing) / (item + spacing)).floor().max(1.0) as usize;
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("fv_cols"), cols));
}

#[allow(clippy::too_many_arguments)]
fn icons(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    shown: &[&FvEntry],
    f: &mut Drawn,
    out: &mut Vec<Msg>,
) {
    let fv = &st.folder_view;
    let size = fv.icon_size;
    let mut want = vec![];
    remember_columns(ui, size, 8.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(8.0);
        for e in shown {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(size, size + 36.0), Sense::click());
            let p = ui.painter_at(rect.expand(2.0));
            entry_bg(&p, rect, fv.selected.contains(&e.name), resp.hovered());
            let icon_rect = egui::Rect::from_min_size(rect.min + Vec2::splat(4.0), Vec2::splat(size - 8.0));
            // Thumbnails load only for tiles on screen.
            if ui.is_rect_visible(rect) {
                if e.kind.is_dir() && !fv.previews_asked.contains(&e.name) && !fv.previews.contains_key(&e.name) {
                    want.push(e.name.clone());
                }
                let previews = fv.previews.get(&e.name).map(Vec::as_slice).unwrap_or_default();
                draw_icon(&p, icon_rect, e, previews, root, tex);
            }
            let name_rect =
                egui::Rect::from_min_size(egui::pos2(rect.left(), rect.bottom() - 34.0), Vec2::new(size, 32.0));
            let color = ui.visuals().text_color();
            let mut job =
                egui::text::LayoutJob::simple(e.label().to_string(), FontId::proportional(12.0), color, size - 4.0);
            job.wrap.max_rows = 2;
            job.wrap.break_anywhere = true;
            job.halign = egui::Align::Center;
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            p.galley(egui::pos2(name_rect.center().x, name_rect.top()), galley, color);
            entry_response(ui, st, e, resp, f, out);
        }
    });
    if !want.is_empty() {
        out.push(Msg::FvWantPreviews(want));
    }
}

/// Compact: a small icon (or thumbnail) and the name, in columns.
#[allow(clippy::too_many_arguments)]
fn compact(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    shown: &[&FvEntry],
    f: &mut Drawn,
    out: &mut Vec<Msg>,
) {
    const W: f32 = 240.0;
    const H: f32 = 24.0;
    let fv = &st.folder_view;
    remember_columns(ui, W, 6.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 2.0);
        for e in shown {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(W, H), Sense::click());
            let p = ui.painter_at(rect.expand(2.0));
            entry_bg(&p, rect, fv.selected.contains(&e.name), resp.hovered());
            let icon = egui::Rect::from_min_size(rect.min + Vec2::new(3.0, 2.0), Vec2::splat(H - 4.0));
            if ui.is_rect_visible(rect) {
                match &e.kind {
                    EntryKind::Image(img) => image_fit(&p, icon, tex.thumb(root, img).as_ref()),
                    k => {
                        let (fill, stroke) = kind_colors(k);
                        icon2(&p, icon.center(), glyph(k), H - 6.0, fill, stroke);
                    }
                }
            }
            let name_rect = egui::Rect::from_min_max(egui::pos2(icon.right() + 6.0, rect.top()), rect.max);
            let color = ui.visuals().text_color();
            let mut job =
                egui::text::LayoutJob::simple_singleline(e.label().to_string(), FontId::proportional(13.0), color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(name_rect.width() - 4.0);
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            p.galley(egui::pos2(name_rect.left(), name_rect.center().y - galley.size().y / 2.0), galley, color);
            entry_response(ui, st, e, resp, f, out);
        }
    });
}

fn details(ui: &mut egui::Ui, st: &AppState, shown: &[&FvEntry], f: &mut Drawn, out: &mut Vec<Msg>) {
    let fv = &st.folder_view;
    egui::Grid::new("fv_details").num_columns(5).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
        for h in ["Name", "Kind", "Info", "Size", "Modified"] {
            ui.label(RichText::new(h).strong());
        }
        ui.end_row();
        for e in shown {
            let resp = ui.selectable_label(fv.selected.contains(&e.name), ic(glyph(&e.kind), e.label()));
            entry_response(ui, st, e, resp, f, out);
            ui.weak(kind_name(&e.kind));
            ui.weak(&e.info);
            ui.weak(size_text(e));
            ui.weak(e.modified.map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default());
            ui.end_row();
        }
    });
}
