//! Idea inbox (New idea · Grid · Discarded) and Input library (Grid with the
//! folder tree).

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, Color32, FontId, Key, Panel, RichText, ScrollArea, Sense, Stroke};
use increart_core::scan::IdeaEntry;
use increart_core::{IdeaLoc, ImageRef, Tree, Verdict};
use increart_gui_core::{
    AddIdea, AppState, Confirm, IdeaAge, IdeaOrigin, InboxFilter, InboxTab, InfoPanel, LibTab, Msg, PickKind, Tab,
    ZoomGrid,
};

use crate::folder_pick;
use crate::textures::Textures;
use crate::widgets::{ACCENT, NEUTRAL, card, cover_buttons, dialog_footer, ic, ph, short, single_click, thumb};

/// Idea cards (Idea inbox and Input library): all one size (the Input
/// library's zooms), the first image square across the full width, the
/// text cut to fit.
const CARD_W: f32 = 210.0;
/// Below the image: title, facts, two lines of text and the tags.
const CARD_TEXT_H: f32 = 92.0;

/// An idea's card. Double-click: its first image full size (the editor
/// when it has none); the pencil on the image (on hover, or selected): the
/// editor.
#[allow(clippy::too_many_arguments)]
/// What an idea card's picture offers while hovered or selected, besides
/// ✎ Edit.
#[derive(Clone, Copy, PartialEq)]
enum CardButtons {
    /// The Idea inbox: Save to input library… and Discard.
    Triage,
    /// The Input library: New project, and Add as refs when it has images.
    Library,
    None,
}

#[allow(clippy::too_many_arguments)]
fn idea_card(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    e: &IdeaEntry,
    w: f32,
    selected: bool,
    buttons: CardButtons,
    out: &mut Vec<Msg>,
) -> egui::Response {
    let r = idea_card_body(ui, st, root, tex, e, w, selected);
    // over the card, so it gets the click rather than the card
    let edit = egui::Rect::from_min_size(r.rect.left_top() + egui::vec2(w - 26.0, 13.0), egui::vec2(28.0, 26.0));
    if selected || ui.rect_contains_pointer(r.rect) {
        let b = ui.interact(edit, r.id.with("edit"), egui::Sense::click());
        let fill = if b.hovered() { Color32::from_black_alpha(220) } else { Color32::from_black_alpha(150) };
        ui.painter().rect_filled(edit, 4.0, fill);
        let color = if b.hovered() { Color32::WHITE } else { Color32::from_gray(210) };
        ui.painter().text(edit.center(), Align2::CENTER_CENTER, ph::PENCIL_SIMPLE, FontId::proportional(16.0), color);
        let tip = if st.tab == Tab::IdeaInbox { "Edit idea   Enter" } else { "Edit idea" };
        if b.on_hover_text(tip).clicked() {
            out.push(Msg::OpenIdea(e.idea.id.clone()));
        }
        // along the bottom of the picture, as on project tiles
        let cover = egui::Rect::from_min_size(r.rect.min + egui::vec2(7.0, 7.0), egui::vec2(w, w));
        let id = &e.idea.id;
        if selected {
            // where its buttons are, for the screenshot tour's click checks
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("tour_idea_cover"), cover));
        }
        match buttons {
            CardButtons::Triage => {
                let picked = cover_buttons(
                    ui,
                    cover,
                    &[
                        (ph::BOOKMARK_SIMPLE, "Save", "to the input library (1 / 2 / S)"),
                        (ph::TRASH, "Discard", "can be restored from Discarded (3 / B)"),
                    ],
                );
                match picked {
                    Some(0) => out.push(Msg::SaveIdeas(vec![id.clone()])),
                    Some(_) => out.push(Msg::ReviewIdea(id.clone(), Verdict::Bad)),
                    None => {}
                }
            }
            CardButtons::Library => {
                let new_project = (ph::ROCKET_LAUNCH, "New project", "from this idea (P / Space)");
                let refs = (ph::IMAGES, "Add as refs", "its images to the current project's Generate panel");
                let list: &[_] = if e.idea.images.is_empty() { &[new_project] } else { &[new_project, refs] };
                match cover_buttons(ui, cover, list) {
                    Some(0) => out.push(Msg::NewProjectFromIdeas(vec![id.clone()])),
                    Some(_) => out.push(Msg::AddRefs(e.idea.images.iter().cloned().map(ImageRef::input).collect())),
                    None => {}
                }
            }
            CardButtons::None => {}
        }
    }
    if r.double_clicked() {
        match e.idea.images.first() {
            Some(img) => out.push(Msg::Preview(Some(ImageRef::input(img.clone())))),
            None => out.push(Msg::OpenIdea(e.idea.id.clone())),
        }
    }
    r
}

/// An idea's menu: **Rewrite with PE…** in the Edit window, so Use there
/// changes its text.
fn rewrite_item(ui: &mut egui::Ui, st: &AppState, id: &increart_core::Id, out: &mut Vec<Msg>) {
    let r = ui.add_enabled(st.pe.server.is_some(), egui::Button::new(ic(ph::MAGIC_WAND, "Rewrite with PE…")));
    if r.on_hover_text("open it to edit, and ask the prompt-enhance server for better text")
        .on_disabled_hover_text("needs a prompt-enhance server")
        .clicked()
    {
        out.push(Msg::OpenIdea(id.clone()));
        out.push(Msg::PeRewrite(increart_gui_core::PeTarget::Idea(id.clone())));
        ui.close();
    }
}

/// An idea's menu: **Copy text**, then for its first image **Copy image** and
/// **Describe image with PE…**.
fn copy_items(ui: &mut egui::Ui, root: &Path, e: &IdeaEntry, out: &mut Vec<Msg>) {
    if ui.button(ic(ph::COPY, "Copy text")).clicked() {
        ui.ctx().copy_text(e.idea.text.clone());
        ui.close();
    }
    if let Some(img) = e.idea.images.first() {
        let img = ImageRef::input(img.clone());
        crate::widgets::copy_image_item(ui, root, &img, out);
        crate::widgets::describe_item(ui, &img, "Describe image with PE…", out);
    }
}

fn folder_item(ui: &mut egui::Ui, e: &IdeaEntry, out: &mut Vec<Msg>) {
    if ui.button(ic(ph::FOLDER_OPEN, "Open folder in file manager")).clicked() {
        out.push(Msg::ShowInFileManager(e.dir.clone()));
        ui.close();
    }
}

fn idea_card_body(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    e: &IdeaEntry,
    w: f32,
    selected: bool,
) -> egui::Response {
    let usage = st.idea_usage(e);
    card(ui, selected, w, w + CARD_TEXT_H, |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        match e.idea.images.first().map(|id| tex.thumb(root, &ImageRef::input(id.clone()))) {
            Some(t) => {
                thumb(ui, t.as_ref(), w, false, None);
            }
            None => {
                let (r, _) = ui.allocate_exact_size(egui::vec2(w, w), egui::Sense::hover());
                ui.painter().rect_filled(r, 2.0, Color32::from_gray(28));
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    ph::TEXT_T,
                    FontId::proportional(56.0),
                    Color32::from_gray(90),
                );
            }
        }
        // what it has, on one line
        let mut facts = vec![];
        if e.idea.images.len() > 1 {
            facts.push(format!("+{} images", e.idea.images.len() - 1));
        }
        if !e.idea.variants.is_empty() {
            facts.push(format!("{} variants", e.idea.variants.len()));
        }
        if !e.idea.projects.is_empty() {
            facts.push(ic(ph::ROCKET_LAUNCH, &e.idea.projects.len().to_string()));
        }
        if usage.label().is_some() {
            facts.push(ic(ph::ARROW_BEND_DOWN_RIGHT, &format!("{} ref · {} start", usage.ref_runs, usage.start_runs)));
        }
        let title = if e.idea.title.is_empty() { "(untitled)" } else { &e.idea.title };
        ui.horizontal(|ui| {
            if e.idea.starred {
                ui.label(RichText::new(ph::STAR).color(Color32::GOLD));
            }
            ui.add(egui::Label::new(RichText::new(title).strong()).truncate());
        });
        let facts = ui.add(egui::Label::new(RichText::new(facts.join("  ")).weak().small()).truncate());
        if let Some(text) = usage.label() {
            facts.on_hover_text(format!("its images: {text}"));
        }
        // the text: two lines at most
        let color = ui.visuals().text_color();
        let mut job = egui::text::LayoutJob::simple(e.idea.text.clone(), FontId::proportional(11.0), color, w);
        job.wrap.max_rows = 2;
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        ui.add(egui::Label::new(galley).selectable(false));
        if !e.idea.tags.is_empty() {
            ui.add(egui::Label::new(RichText::new(e.idea.tags.join(", ")).weak()).truncate());
        }
    })
}

/// Files dragged from the desktop onto the Idea inbox: highlight while
/// hovering, import on drop.
fn desktop_drop(ui: &mut egui::Ui, out: &mut Vec<Msg>) {
    let ctx = ui.ctx().clone();
    let (hovering, dropped) = ctx.input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone()));
    if !dropped.is_empty() {
        out.push(Msg::DropFiles(dropped.iter().map(|f| f.path().to_path_buf()).collect()));
    }
    if hovering {
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("inbox_drop_overlay")));
        let r = ui.max_rect().shrink(12.0);
        p.rect(
            r,
            12.0,
            Color32::from_rgba_unmultiplied(40, 90, 160, 120),
            Stroke::new(3.0, ACCENT),
            egui::StrokeKind::Inside,
        );
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            ic(ph::TRAY, "Drop to import into the Idea inbox"),
            FontId::proportional(28.0),
            Color32::WHITE,
        );
    }
}

pub fn inbox(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    desktop_drop(ui, out);
    let n = st.unreviewed_ideas().len();
    Panel::top("inbox_tabs").show(ui, |ui| {
        // groups wrap whole when the bar is short of room
        use crate::widgets::group;
        ui.horizontal_wrapped(|ui| {
            group(ui, "tabs", |ui| {
                for (t, label) in [
                    (InboxTab::NewIdea, ic(ph::PLUS, "New idea")),
                    (InboxTab::Grid, ic(ph::SQUARES_FOUR, &format!("Grid ({n})"))),
                    (InboxTab::Folder, ic(ph::FOLDER, "Folder")),
                    (InboxTab::Import, ic(ph::FILE_ARROW_DOWN, &format!("Import ({})", st.index.inbox_drop.len()))),
                    (InboxTab::Discarded, ic(ph::TRASH, &format!("Discarded ({})", st.discarded_ideas().len()))),
                ] {
                    if ui.selectable_label(st.inbox_tab == t, label).clicked() {
                        out.push(Msg::InboxTab(t));
                    }
                }
            });
            group(ui, "import", |ui| {
                ui.separator();
                if ui
                    .button(ic(ph::FILE_ARROW_DOWN, "Import files…"))
                    .on_hover_text("images and .txt files become new ideas")
                    .clicked()
                {
                    out.push(Msg::Pick(PickKind::ImportFiles));
                }
            });
            if st.inbox_tab == InboxTab::Grid {
                group(ui, "zoom", |ui| {
                    ui.separator();
                    crate::review::grid_zoom(ui, st, ZoomGrid::Inbox, out);
                });
            }
        });
    });
    match st.inbox_tab {
        InboxTab::NewIdea => new_idea(ui, st, tex, out),
        InboxTab::Grid => inbox_grid(ui, st, tex, out),
        InboxTab::Folder => {
            egui::CentralPanel::default().show(ui, |ui| crate::folderview::view(ui, st, tex, out));
        }
        InboxTab::Import => import(ui, st, tex, out),
        InboxTab::Discarded => discarded(ui, st, tex, out),
    }
}

/// The Import tab: files waiting in `inbox_drop/`, recent imports with what
/// changed on import.
fn import(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    if st.imports.is_none() {
        out.push(Msg::LoadImports);
    }
    egui::CentralPanel::default().show(ui, |ui| {
        let Some(root) = st.library.as_deref() else { return };
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(ic(ph::TRAY_ARROW_DOWN, "Drop folder"));
                if ui.button(ic(ph::FOLDER_OPEN, "Open in file manager")).clicked() {
                    out.push(Msg::ShowInFileManager("input/inbox_drop".into()));
                }
            });
            ui.weak("Anything put in input/inbox_drop/ becomes an idea by itself, once it has finished copying: images as image ideas, .txt files as text. Originals move to imported/.");
            if !st.watch_status.is_empty() {
                ui.weak(format!("Library: {}", st.watch_status));
            }
            if st.index.inbox_drop.is_empty() {
                ui.weak("No files waiting.");
            } else {
                for f in &st.index.inbox_drop {
                    ui.monospace(f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                }
                if ui.button(RichText::new(ic(ph::DOWNLOAD_SIMPLE, &format!("Import now ({})", st.index.inbox_drop.len()))).strong()).clicked() {
                    out.push(Msg::ImportDropNow);
                }
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.heading(ic(ph::CLOCK_COUNTER_CLOCKWISE, "Recent imports"));
                if ui.small_button(ph::ARROWS_CLOCKWISE).on_hover_text("refresh").clicked() {
                    out.push(Msg::LoadImports);
                }
            });
            match &st.imports {
                None => {
                    ui.spinner();
                }
                Some(list) if list.is_empty() => {
                    ui.weak("Nothing imported yet.");
                }
                Some(list) => {
                    egui::Grid::new("imports").num_columns(4).spacing([12.0, 6.0]).striped(true).show(ui, |ui| {
                        for (id, src) in list {
                            let t = tex.thumb(root, &ImageRef::input(id.clone()));
                            let r = thumb(ui, t.as_ref(), 48.0, false, None).on_hover_text("double-click to view");
                            if r.double_clicked() {
                                out.push(Msg::Preview(Some(ImageRef::input(id.clone()))));
                            }
                            r.context_menu(|ui| crate::widgets::image_menu_items(ui, root, &ImageRef::input(id.clone()), out));
                            ui.vertical(|ui| {
                                ui.set_width(360.0);
                                ui.add(egui::Label::new(src.original_name.clone().unwrap_or_else(|| "(clipboard)".into())).truncate());
                                ui.add(egui::Label::new(RichText::new(src.original_path.clone().unwrap_or_default()).weak().small()).truncate());
                            });
                            ui.label(format!(
                                "{} · {} × {} · {} KB",
                                src.original_format,
                                src.original_width,
                                src.original_height,
                                src.original_size / 1024
                            ));
                            ui.weak(if src.conversions.is_empty() { "unchanged".into() } else { src.conversions.join(", ") });
                            ui.end_row();
                        }
                    });
                }
            }
        });
    });
}

/// The Edit idea window: title, tags, text (with history), images, variants.
pub fn edit_window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let Some(ed) = &st.idea_edit else { return };
    let Some(root) = st.library.as_deref() else { return };
    let idea = st.idea(&ed.id).map(|e| e.idea.clone());
    let mut open = true;
    egui::Window::new(ic(ph::PENCIL_SIMPLE, "Edit idea")).open(&mut open).default_width(620.0).collapsible(false).show(
        ctx,
        |ui| {
            let mut e = ed.clone();
            ui.label("Title");
            ui.add(egui::TextEdit::singleline(&mut e.title).desired_width(f32::INFINITY));
            ui.label("Tags (comma-separated)");
            ui.add(egui::TextEdit::singleline(&mut e.tags).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                ui.label("Text");
                if ui.small_button(ic(ph::CLOCK_COUNTER_CLOCKWISE, "History…")).clicked() {
                    out.push(Msg::OpenHistory(increart_core::HistoryKey::Idea(e.id.clone())));
                }
            });
            ui.add(egui::TextEdit::multiline(&mut e.text).desired_rows(8).desired_width(f32::INFINITY));
            ui.label("Images");
            ui.horizontal_wrapped(|ui| {
                let mut remove = None;
                for (i, id) in e.images.iter().enumerate() {
                    ui.vertical(|ui| {
                        let t = tex.thumb(root, &ImageRef::input(id.clone()));
                        let r = thumb(ui, t.as_ref(), 96.0, false, None).on_hover_text("double-click to view");
                        r.context_menu(|ui| {
                            crate::widgets::image_menu_items(ui, root, &ImageRef::input(id.clone()), out)
                        });
                        if r.double_clicked() {
                            out.push(Msg::Preview(Some(ImageRef::input(id.clone()))));
                        }
                        if ui.small_button(ph::X).on_hover_text("remove from the idea").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    e.images.remove(i);
                }
                if e.images.is_empty() {
                    ui.weak("none");
                }
            });
            if let Some(i) = &idea
                && !i.variants.is_empty()
            {
                ui.label(format!("Variants ({})", i.variants.len()));
                for v in &i.variants {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(RichText::new(short(&v.text, 90)).small()).wrap());
                        if ui.small_button("use").on_hover_text("make this the text").clicked() {
                            e.text = v.text.clone();
                        }
                    });
                }
            }
            if &e != ed {
                out.push(Msg::EditIdea(e.clone()));
            }
            ui.separator();
            let mut tools = vec![];
            dialog_footer(
                ui,
                |ui| {
                    if ui.button(ic(ph::FOLDER_OPEN, "Open folder")).clicked()
                        && let Some(entry) = st.idea(&ed.id)
                    {
                        tools.push(Msg::ShowInFileManager(entry.dir.clone()));
                    }
                    let target = increart_gui_core::PeTarget::Idea(ed.id.clone());
                    crate::rewrite::button(ui, st, target, "Rewrite with PE", &mut tools);
                },
                |ui| {
                    if ui.button(RichText::new(ic(ph::FLOPPY_DISK, "Save")).strong()).clicked() {
                        out.push(Msg::SaveIdea);
                    }
                    if ui.button("Cancel").clicked() {
                        out.push(Msg::CloseIdea);
                    }
                },
            );
            out.extend(tools);
        },
    );
    if !open {
        out.push(Msg::CloseIdea);
    }
}

fn new_idea(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.set_max_width(760.0);
        ui.heading("New idea");
        let mut d = st.draft.clone();
        ui.label("Title");
        ui.add(egui::TextEdit::singleline(&mut d.title).desired_width(f32::INFINITY));
        ui.label("Text: prompt draft, notes, or both");
        ui.add(egui::TextEdit::multiline(&mut d.text).desired_rows(8).desired_width(f32::INFINITY));
        ui.label("Tags (comma-separated)");
        ui.add(egui::TextEdit::singleline(&mut d.tags).desired_width(f32::INFINITY));
        if d != st.draft {
            out.push(Msg::Draft(d));
        }
        ui.add_space(6.0);
        ui.label("Images");
        ui.horizontal_wrapped(|ui| {
            if let Some(root) = st.library.as_deref() {
                for (i, id) in st.draft.images.iter().enumerate() {
                    ui.vertical(|ui| {
                        let t = tex.thumb(root, &ImageRef::input(id.clone()));
                        let resp = thumb(ui, t.as_ref(), 96.0, false, None);
                        if i == 0 {
                            // where it is, for the screenshot tour's double-click check
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("tour_first_draft_image"), resp.rect));
                        }
                        let resp = resp.on_hover_text("double-click to view");
                        if resp.double_clicked() {
                            out.push(Msg::Preview(Some(ImageRef::input(id.clone()))));
                        }
                        resp.context_menu(|ui| {
                            crate::widgets::image_menu_items(ui, root, &ImageRef::input(id.clone()), out)
                        });
                        if ui.small_button(ph::X).on_hover_text("remove").clicked() {
                            out.push(Msg::RemoveDraftImage(i));
                        }
                    });
                }
            }
            if ui.button(ic(ph::IMAGE, "Add images…")).clicked() {
                out.push(Msg::Pick(PickKind::IdeaImages));
            }
        });
        ui.weak("Files dropped on this window become ideas of their own.");
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(ic(ph::TRAY, "Add to inbox")).clicked() {
                out.push(Msg::AddIdea(AddIdea::Inbox));
            }
            if ui.button(ic(ph::BOOKMARK_SIMPLE, "Add & save to input library…")).clicked() {
                out.push(Msg::AddIdea(AddIdea::Keep));
            }
            if ui.button(ic(ph::ROCKET_LAUNCH, "Add & new project…")).clicked() {
                out.push(Msg::AddIdea(AddIdea::NewProject));
            }
            crate::rewrite::button(ui, st, increart_gui_core::PeTarget::NewIdea, "Rewrite with PE", out);
            if ui.button(ic(ph::CLOCK_COUNTER_CLOCKWISE, "History…")).clicked() {
                out.push(Msg::OpenHistory(increart_core::HistoryKey::NewIdea));
            }
            crate::app::draft_note(ui, st, st.idea_dirty);
        });
    });
}

fn inbox_grid(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let ideas = st.inbox_queue();
    let current = ideas.get(st.inbox_cursor).map(|e| e.idea.id.clone());
    let picks = &st.inbox_picked;
    let modal = st.preview.is_some() || st.tile.is_some() || st.save_idea.is_some() || st.from_ideas.is_some();
    if !ui.ctx().egui_wants_keyboard_input() && !modal {
        ui.input(|i| {
            // with picks, they all go
            if [Key::Num1, Key::Num2, Key::S].iter().any(|k| i.key_pressed(*k)) {
                let ids = if picks.is_empty() { current.clone().into_iter().collect() } else { picks.clone() };
                out.push(Msg::SaveIdeas(ids));
            }
            if [Key::Num3, Key::B].iter().any(|k| i.key_pressed(*k)) {
                let v = Verdict::Bad;
                out.push(if picks.is_empty() { Msg::ReviewCurrentIdea(v) } else { Msg::ReviewPickedIdeas(v) });
            }
            if i.modifiers.command && i.key_pressed(Key::A) {
                out.push(Msg::InboxPick(ideas.iter().map(|e| e.idea.id.clone()).collect()));
            }
            if !picks.is_empty() {
                if i.key_pressed(Key::Escape) {
                    out.push(Msg::InboxPick(vec![]));
                }
                if i.key_pressed(Key::P) || i.key_pressed(Key::Space) {
                    out.push(Msg::NewProjectFromIdeas(picks.clone()));
                }
                if i.key_pressed(Key::T) && !i.modifiers.command {
                    out.push(Msg::TileIdeas(picks.clone()));
                }
                return;
            }
            if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown) {
                out.push(Msg::InboxCursor(st.inbox_cursor + 1));
            }
            if i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowUp) {
                out.push(Msg::InboxCursor(st.inbox_cursor.saturating_sub(1)));
            }
            // the first / last (the cursor is kept to the queue)
            if i.key_pressed(Key::Home) {
                out.push(Msg::InboxCursor(0));
            }
            if i.key_pressed(Key::End) {
                out.push(Msg::InboxCursor(usize::MAX));
            }
            if let Some(id) = &current
                && (i.key_pressed(Key::P) || i.key_pressed(Key::Space))
            {
                out.push(Msg::NewProjectFromIdeas(vec![id.clone()]));
            }
            if let Some(id) = &current
                && (i.key_pressed(Key::Enter) || i.key_pressed(Key::T))
            {
                out.push(Msg::OpenIdea(id.clone()));
            }
        });
    }
    Panel::left("inbox_side").default_size(230.0).show(ui, |ui| inbox_side(ui, st, out));
    // always shown: triage reads the idea there
    if let Some(root) = st.library.as_deref() {
        Panel::right("inbox_info").default_size(300.0).show(ui, |ui| match ideas.get(st.inbox_cursor) {
            Some(e) => {
                panel_header(ui, out, None);
                pinned_actions(ui, "inbox_info_actions", |ui| inbox_actions(ui, e, out));
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .id_salt("inbox_info_scroll")
                    .show(ui, |ui| idea_details(ui, st, root, tex, e, out));
            }
            None => crate::widgets::nothing_selected(
                ui,
                "Idea",
                "No idea here: the inbox is empty, or the filters hide them all.",
            ),
        });
    }
    egui::CentralPanel::default().show(ui, |ui| {
        let Some(root) = st.library.as_deref() else { return };
        ui.horizontal_wrapped(|ui| {
            let dir = root.join("input").join("inbox_drop");
            ui.weak(ic(ph::TRAY_ARROW_DOWN, "Files saved to"));
            ui.label(RichText::new(dir.display().to_string()).monospace().weak())
                .on_hover_text("images and .txt files saved here become ideas by themselves");
            ui.weak("(e.g. Save as… from a browser) show up here as new ideas.");
            if ui.small_button(ic(ph::FOLDER_OPEN, "Open")).clicked() {
                out.push(Msg::ShowInFileManager("input/inbox_drop".into()));
            }
        });
        ui.add_space(4.0);
        let all = st.unreviewed_ideas().len();
        if all == 0 {
            ui.weak("Nothing to review. Add ideas with New idea, or drop files here.");
            return;
        }
        if st.inbox_filter.narrows() {
            ui.horizontal_wrapped(|ui| {
                let (shown, hidden) = (ideas.len(), all - ideas.len());
                let text = format!("Filters are on: showing {shown} of {all} ({hidden} hidden).");
                ui.colored_label(NEUTRAL, ic(ph::FUNNEL, &text));
                if ui.button(ic(ph::FUNNEL_X, "Clear filters")).clicked() {
                    let oldest_first = st.inbox_filter.oldest_first;
                    out.push(Msg::InboxFilter(InboxFilter { oldest_first, ..Default::default() }));
                }
            });
            ui.add_space(6.0);
        }
        picks_bar(ui, st, picks, true, out);
        crate::review::grid_ctrl_scroll(ui, st, ZoomGrid::Inbox, out);
        let w = st.grid_cell_size(ZoomGrid::Inbox) as f32;
        let bg = pick_background(ui, st, picks, Msg::InboxPick(vec![]), out);
        let mut rects = vec![];
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (i, e) in ideas.iter().enumerate() {
                    let on = i == st.inbox_cursor && picks.is_empty();
                    let r = idea_card(ui, st, root, tex, e, w, on, CardButtons::Triage, out);
                    let id = &e.idea.id;
                    rects.push((id.clone(), r.rect));
                    if let Some(n) = picks.iter().position(|p| p == id) {
                        crate::tiles::mark_n(ui, r.rect, n);
                    }
                    if i == st.inbox_cursor {
                        crate::widgets::reveal_cursor(ui, "inbox", id.clone(), &r);
                    }
                    if r.clicked() {
                        let view: Vec<_> = ideas.iter().map(|e| &e.idea.id).collect();
                        match pick_click(ui, picks, current.as_ref(), &view, id) {
                            Some(p) => out.push(Msg::InboxPick(p)),
                            None => {
                                if !picks.is_empty() {
                                    out.push(Msg::InboxPick(vec![]));
                                }
                                out.push(Msg::InboxCursor(i));
                            }
                        }
                    }
                    r.context_menu(|ui| {
                        if picks.contains(id) {
                            picks_menu(ui, st, picks, true, out);
                            ui.separator();
                            ui.weak("this idea");
                        } else if ui.button(ic(ph::CHECK_SQUARE, "Pick")).on_hover_text("also Ctrl+click").clicked() {
                            let mut p = picks.clone();
                            if p.is_empty() {
                                p.extend(current.clone().filter(|c| c != id));
                            }
                            p.push(id.clone());
                            out.push(Msg::InboxPick(p));
                            ui.close();
                        }
                        let item = |ui: &mut egui::Ui, label: String, key: &str| {
                            ui.add(egui::Button::new(label).shortcut_text(key)).clicked()
                        };
                        if !e.idea.images.is_empty() && ui.button(ic(ph::SQUARES_FOUR, "Tile its images…")).clicked()
                        {
                            out.push(Msg::TileIdeas(vec![id.clone()]));
                            ui.close();
                        }
                        ui.separator();
                        if item(ui, ic(ph::ROCKET_LAUNCH, "New project from idea…"), "P") {
                            out.push(Msg::NewProjectFromIdeas(vec![id.clone()]));
                            ui.close();
                        }
                        if item(ui, ic(ph::PENCIL_SIMPLE, "Edit…"), "Enter") {
                            out.push(Msg::OpenIdea(id.clone()));
                            ui.close();
                        }
                        rewrite_item(ui, st, id, out);
                        ui.separator();
                        if item(ui, ic(ph::BOOKMARK_SIMPLE, "Save to input library…"), "S") {
                            out.push(Msg::SaveIdeas(vec![id.clone()]));
                            ui.close();
                        }
                        if item(ui, ic(ph::TRASH, "Discard"), "3") {
                            out.push(Msg::ReviewIdea(id.clone(), Verdict::Bad));
                            ui.close();
                        }
                        ui.separator();
                        copy_items(ui, root, e, out);
                        ui.separator();
                        folder_item(ui, e, out);
                    });
                }
            });
        });
        if let Some(p) = crate::tiles::band_of(ui, &bg, picks, &rects, |add| match add {
            true if picks.is_empty() => current.clone().into_iter().collect(),
            true => picks.clone(),
            false => vec![],
        }) {
            out.push(Msg::InboxPick(p));
        }
    });
}

/// The empty space of an idea grid: a click there drops the picks
/// (`clear`), a drag draws the rubber band. Before the cards.
fn pick_background(
    ui: &mut egui::Ui,
    st: &AppState,
    picks: &[increart_core::Id],
    clear: Msg,
    out: &mut Vec<Msg>,
) -> egui::Response {
    let bg = ui.interact(ui.available_rect_before_wrap(), ui.scope_id().with("idea_pick_bg"), Sense::click_and_drag());
    if bg.clicked() && !picks.is_empty() {
        out.push(clear);
    }
    bg.context_menu(|ui| {
        let n = picks.len();
        if n > 0 && ui.button(ic(ph::SQUARES_FOUR, &format!("Tile the {n} picked…   T"))).clicked() {
            out.push(Msg::TileIdeas(picks.to_vec()));
            ui.close();
        }
        crate::tiles::clipboard_item(ui, st, out);
    });
    bg
}

/// A card's click with Ctrl (toggle) or Shift (the range from the last
/// picked, or the cursor): the new picks. None for a plain click.
fn pick_click(
    ui: &egui::Ui,
    picks: &[increart_core::Id],
    cursor: Option<&increart_core::Id>,
    view: &[&increart_core::Id],
    id: &increart_core::Id,
) -> Option<Vec<increart_core::Id>> {
    let m = ui.input(|i| i.modifiers);
    if !m.command && !m.shift {
        return None;
    }
    // picking starts from the idea the cursor is on
    let mut p = picks.to_vec();
    if p.is_empty()
        && let Some(c) = cursor
    {
        p.push(c.clone());
    }
    if m.shift {
        let to = view.iter().position(|x| *x == id)?;
        let from = p.last().and_then(|a| view.iter().position(|x| *x == a)).unwrap_or(to);
        let range: Vec<_> =
            if from <= to { view[from..=to].to_vec() } else { view[to..=from].iter().rev().copied().collect() };
        for x in range {
            if !p.contains(x) {
                p.push(x.clone());
            }
        }
    } else if let Some(at) = p.iter().position(|x| x == id) {
        p.remove(at);
    } else {
        p.push(id.clone());
    }
    Some(p)
}

/// What to do with the picked ideas (`picks`): tile their images, make a project,
/// add as refs; in the Idea inbox (`inbox`) save or discard them all too.
fn picks_menu(ui: &mut egui::Ui, st: &AppState, picks: &[increart_core::Id], inbox: bool, out: &mut Vec<Msg>) {
    let n = picks.len();
    let images: Vec<ImageRef> = picks
        .iter()
        .filter_map(|id| st.idea(id))
        .flat_map(|e| e.idea.images.iter().cloned().map(ImageRef::input))
        .collect();
    ui.weak(format!("the {n} picked"));
    let tile = egui::Button::new(ic(ph::SQUARES_FOUR, &format!("Tile their {} images…", images.len())));
    if ui.add_enabled(!images.is_empty(), tile).on_disabled_hover_text("they have no images").clicked() {
        out.push(Msg::TileIdeas(picks.to_vec()));
        ui.close();
    }
    if ui.button(ic(ph::ROCKET_LAUNCH, &format!("New project from the {n} ideas…"))).clicked() {
        out.push(Msg::NewProjectFromIdeas(picks.to_vec()));
        ui.close();
    }
    if ui
        .add_enabled(!images.is_empty(), egui::Button::new(ic(ph::IMAGES, "Add their images as refs")))
        .on_hover_text("to the Generate panel of the current project")
        .clicked()
    {
        out.push(Msg::AddRefs(images));
        ui.close();
    }
    if !inbox {
        collection_items(ui, st, picks, out);
    }
    if inbox {
        if ui.button(ic(ph::BOOKMARK_SIMPLE, "Save them to input library…")).clicked() {
            out.push(Msg::SaveIdeas(picks.to_vec()));
            ui.close();
        }
        if ui.button(ic(ph::TRASH, "Discard them")).clicked() {
            out.push(Msg::ReviewPickedIdeas(Verdict::Bad));
            ui.close();
        }
    }
}

/// Add to collection… and, in a collection's view, Remove from this
/// collection (when any of the ideas is in it or one inside it).
fn collection_items(ui: &mut egui::Ui, st: &AppState, ids: &[increart_core::Id], out: &mut Vec<Msg>) {
    if ui.button(ic(ph::FOLDER_PLUS, "Add to collection…")).clicked() {
        out.push(Msg::CollectIdeasAsk(ids.to_vec()));
        ui.close();
    }
    if let Some(f) = st.lib_folder.as_ref().filter(|f| !f.as_os_str().is_empty()) {
        let inside = st.index.ideas_in_collection(Some(f));
        if ids.iter().any(|id| inside.contains(id)) {
            let name = f.file_name().unwrap_or_default().to_string_lossy();
            if ui
                .button(ic(ph::MINUS_CIRCLE, "Remove from this collection"))
                .on_hover_text(format!("from {name} (and the collections inside it); the ideas stay"))
                .clicked()
            {
                out.push(Msg::UncollectIdeas(f.clone(), ids.to_vec()));
                ui.close();
            }
        }
    }
}

/// "3 picked · Tile… · New project… · Save to input library · Discard · Clear", over an idea
/// grid while anything is picked.
fn picks_bar(ui: &mut egui::Ui, st: &AppState, picks: &[increart_core::Id], inbox: bool, out: &mut Vec<Msg>) {
    if picks.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{} picked", picks.len())).color(ACCENT).strong());
        let images: usize = picks.iter().filter_map(|id| st.idea(id)).map(|e| e.idea.images.len()).sum();
        if ui
            .add_enabled(images > 0, egui::Button::new(ic(ph::SQUARES_FOUR, "Tile…")))
            .on_hover_text(format!("their {images} images in the Tile window   T"))
            .clicked()
        {
            out.push(Msg::TileIdeas(picks.to_vec()));
        }
        if ui.button(ic(ph::ROCKET_LAUNCH, "New project…")).on_hover_text("from the picked ideas (P)").clicked() {
            out.push(Msg::NewProjectFromIdeas(picks.to_vec()));
        }
        if !inbox && ui.button(ic(ph::FOLDER_PLUS, "Add to collection…")).clicked() {
            out.push(Msg::CollectIdeasAsk(picks.to_vec()));
        }
        if inbox {
            if ui.button(ic(ph::BOOKMARK_SIMPLE, "Save to input library…")).on_hover_text("1 / 2 / S").clicked() {
                out.push(Msg::SaveIdeas(picks.to_vec()));
            }
            if ui.button(ic(ph::TRASH, "Discard")).on_hover_text("3 / B").clicked() {
                out.push(Msg::ReviewPickedIdeas(Verdict::Bad));
            }
        }
        let clear = if inbox { Msg::InboxPick(vec![]) } else { Msg::LibClearSelection };
        if ui.button(ic(ph::X, "Clear")).on_hover_text("Esc").clicked() {
            out.push(clear);
        }
        ui.weak("Ctrl+click: pick · Shift+click: range · drag: box · Ctrl+A: all");
    });
    ui.add_space(4.0);
}

/// The Idea inbox Grid's sidebar: search, order, filters (each with how
/// many inbox ideas it holds) and tags; the keys at the bottom.
fn inbox_side(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    Panel::bottom("inbox_keys").show(ui, |ui| {
        egui::CollapsingHeader::new(RichText::new(ic(ph::KEYBOARD, "Keys")).strong())
            .id_salt("inbox_keys")
            .default_open(true)
            .show(ui, |ui| {
                for (k, what) in [
                    ("1 / 2 / S", "save…"),
                    ("3 / B", "discard"),
                    ("P / Space", "new project"),
                    ("Enter / T", "edit"),
                    ("Ctrl/Shift+click", "pick / range"),
                    ("drag", "pick a box"),
                    ("Ctrl+A", "pick all"),
                    ("T (picks)", "tile their images"),
                    ("← →", "previous / next"),
                    ("Home / End", "first / last"),
                    ("Ctrl+N", "new idea"),
                ] {
                    ui.horizontal(|ui| {
                        ui.monospace(k);
                        ui.weak(what);
                    });
                }
                let note = "Saved ideas go to the input library; discarded ones can be restored.";
                ui.add(egui::Label::new(RichText::new(note).weak()).wrap());
            });
    });
    let mut f = st.inbox_filter.clone();
    let inbox = st.unreviewed_ideas();
    // right to left: the clear button, then the field takes what's left
    let row = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if !f.text.is_empty() && ui.small_button(ph::X).on_hover_text("clear").clicked() {
            f.text.clear();
        }
        let r = ui.add(
            egui::TextEdit::singleline(&mut f.text)
                .hint_text(ic(ph::MAGNIFYING_GLASS, "Search ideas"))
                .desired_width(ui.available_width()),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            f.text.clear();
        }
    });
    ui.horizontal(|ui| {
        for (oldest, label) in [(false, "newest first"), (true, "oldest first")] {
            if ui.selectable_label(f.oldest_first == oldest, label).clicked() {
                f.oldest_first = oldest;
            }
        }
    });
    ScrollArea::vertical().auto_shrink([false, true]).id_salt("inbox_side").show(ui, |ui| {
        ui.add_space(4.0);
        ui.label(RichText::new("Images").strong());
        ui.horizontal(|ui| {
            for (v, l) in [(None, "any"), (Some(true), "with images"), (Some(false), "text only")] {
                if ui.selectable_label(f.images == v, l).clicked() {
                    f.images = v;
                }
            }
        });
        ui.add_space(4.0);
        ui.label(RichText::new("Source").strong());
        if ui.selectable_label(f.origin.is_none(), format!("any ({})", inbox.len())).clicked() {
            f.origin = None;
        }
        for o in IdeaOrigin::ALL {
            let n = inbox.iter().filter(|e| IdeaOrigin::of(&e.idea.source) == o).count();
            if n == 0 && f.origin != Some(o) {
                continue;
            }
            if ui.selectable_label(f.origin == Some(o), format!("{} ({n})", o.label())).clicked() {
                f.origin = Some(o);
            }
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Added").strong());
        if ui.selectable_label(f.age.is_none(), "any time").clicked() {
            f.age = None;
        }
        let now = increart_core::id::now();
        for a in IdeaAge::ALL {
            let n = inbox.iter().filter(|e| a.holds(e.idea.created, now)).count();
            if ui.selectable_label(f.age == Some(a), format!("{} ({n})", a.label())).clicked() {
                f.age = Some(a);
            }
        }
        // the inbox's tags, most used first; a click picks one, again lets go
        let mut tags: std::collections::BTreeMap<&str, usize> = Default::default();
        for t in inbox.iter().flat_map(|e| &e.idea.tags) {
            *tags.entry(t.as_str()).or_default() += 1;
        }
        let mut tags: Vec<_> = tags.into_iter().collect();
        tags.sort_by_key(|&(t, n)| (std::cmp::Reverse(n), t));
        ui.add_space(4.0);
        ui.label(RichText::new("Tags").strong());
        if tags.is_empty() {
            ui.weak("No tags in the inbox.");
        }
        ui.horizontal_wrapped(|ui| {
            for (t, n) in tags {
                let on = f.tag.as_deref() == Some(t);
                if ui.selectable_label(on, format!("{t} ({n})")).clicked() {
                    f.tag = if on { None } else { Some(t.to_string()) };
                }
            }
        });
    });
    if f != st.inbox_filter {
        out.push(Msg::InboxFilter(f));
    }
}

fn discarded(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    egui::CentralPanel::default().show(ui, |ui| {
        let Some(root) = st.library.as_deref() else { return };
        let ideas = st.discarded_ideas();
        if ideas.is_empty() {
            ui.weak("No discarded ideas.");
            return;
        }
        ui.weak("Right-click to restore or delete.");
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for e in ideas {
                    let r = idea_card(ui, st, root, tex, e, CARD_W, false, CardButtons::None, out);
                    r.context_menu(|ui| {
                        let id = &e.idea.id;
                        if ui.button(ic(ph::TRAY, "Restore to the Idea inbox")).clicked() {
                            out.push(Msg::Restore(id.clone(), IdeaLoc::Unreviewed));
                            ui.close();
                        }
                        if ui.button(ic(ph::BOOKS, "Restore to the input library")).clicked() {
                            out.push(Msg::Restore(id.clone(), IdeaLoc::Library));
                            ui.close();
                        }
                        ui.separator();
                        copy_items(ui, root, e, out);
                        ui.separator();
                        if ui.button(ic(ph::TRASH, "Delete permanently")).clicked() {
                            out.push(Msg::DeleteDiscarded(id.clone()));
                            ui.close();
                        }
                    });
                }
            });
        });
    });
}

pub fn input_library(ui: &mut egui::Ui, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let Some(root) = st.library.clone() else { return };
    if st.lib_tab == LibTab::Grid {
        library_keys(ui, st, out);
    }
    Panel::left("lib_tree").default_size(240.0).show(ui, |ui| {
        Panel::bottom("lib_keys").show(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new(ic(ph::KEYBOARD, "Keys")).strong())
                .id_salt("lib_keys")
                .default_open(true)
                .show(ui, |ui| {
                    for (k, what) in [
                        ("← →", "previous / next"),
                        ("Home / End", "first / last"),
                        ("Ctrl+click", "select more"),
                        ("Shift+click", "select a range"),
                        ("drag", "select a box"),
                        ("Ctrl+A", "select all"),
                        ("P / Space", "new project from selected"),
                        ("T", "tile the selected (several)"),
                        ("Enter / T", "edit"),
                        ("Double-click", "full size"),
                        ("Ctrl+N", "new idea"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.monospace(k);
                            ui.weak(what);
                        });
                    }
                });
        });
        // above the keys; scrolls when the collections and smart folders are many
        ScrollArea::vertical().auto_shrink([false, true]).id_salt("lib_side_scroll").show(ui, |ui| {
            let mut f = st.lib_filter.clone();
            crate::widgets::search_field(ui, &mut f.text, "Search titles, text and tags");
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut f.starred_only, ic(ph::STAR, "starred"));
                ui.checkbox(&mut f.with_images_only, ic(ph::IMAGE, "with images"));
                ui.checkbox(&mut f.show_hidden, ic(ph::PENCIL_SIMPLE, "edited copies")).on_hover_text(
                    "the cropped and marked copies (Crop / mark…), hidden otherwise; with no folder selected",
                );
            });
            ui.horizontal(|ui| {
                ui.label("used in a project:");
                for (v, l) in [(None, "any"), (Some(true), "yes"), (Some(false), "no")] {
                    if ui.selectable_label(f.used_in_project == v, l).clicked() {
                        f.used_in_project = v;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("order:");
                for (v, l, tip) in [
                    (false, "newest", "newest first"),
                    (true, "most used", "most runs using its images (as ref or start image) first"),
                ] {
                    if ui.selectable_label(f.most_used == v, l).on_hover_text(tip).clicked() {
                        f.most_used = v;
                    }
                }
            });
            let tags = st.idea_tag_counts(st.lib_source_ideas());
            let (none, tip) =
                ("No tags here yet: Edit an idea to tag it.", "only ideas with this tag (several: all of them)");
            crate::widgets::tag_chips(ui, "lib_tag_filter", tags, &mut f.tags, none, tip);
            if f != st.lib_filter {
                out.push(Msg::LibFilter(f));
            }
            ui.separator();
            collections_panel(ui, st, out);
            ui.separator();
            let sel: Vec<_> = st.lib_selected.iter().cloned().collect();
            ui.label(format!("{} selected", sel.len()));
            ui.add_enabled_ui(!sel.is_empty(), |ui| {
                if ui.button(ic(ph::ROCKET_LAUNCH, "New project from selection…")).clicked() {
                    out.push(Msg::NewProjectFromIdeas(sel.clone()));
                }
                if ui
                    .button(ic(ph::IMAGES, "Add images as refs"))
                    .on_hover_text("to the Generate panel of the current project")
                    .clicked()
                {
                    let refs = sel
                        .iter()
                        .filter_map(|id| st.idea(id))
                        .flat_map(|e| e.idea.images.iter().cloned().map(ImageRef::input))
                        .collect();
                    out.push(Msg::AddRefs(refs));
                }
                if ui.button(ic(ph::FOLDER_PLUS, "Add to collection…")).clicked() {
                    out.push(Msg::CollectIdeasAsk(sel.clone()));
                }
                if ui
                    .button(ic(ph::SQUARES_FOUR, "Tile their images…"))
                    .on_hover_text("T, with several selected")
                    .clicked()
                {
                    let view: Vec<_> = st.library_ideas().iter().map(|e| e.idea.id.clone()).collect();
                    out.push(Msg::TileIdeas(view.into_iter().filter(|id| st.lib_selected.contains(id)).collect()));
                }
                if ui.button(ic(ph::X, "Clear selection")).clicked() {
                    out.push(Msg::LibClearSelection);
                }
            });
        });
    });
    Panel::top("lib_tabs").show(ui, |ui| {
        // groups wrap whole when the bar is short of room
        use crate::widgets::{group, right_group};
        ui.horizontal_wrapped(|ui| {
            group(ui, "tabs", |ui| {
                if ui.selectable_label(st.lib_tab == LibTab::Grid, ic(ph::SQUARES_FOUR, "Grid")).clicked() {
                    out.push(Msg::LibTab(LibTab::Grid));
                }
                if ui.selectable_label(st.lib_tab == LibTab::Folder, ic(ph::FOLDER, "Folder")).clicked() {
                    out.push(Msg::LibTab(LibTab::Folder));
                }
                let n = st.discarded_ideas().len();
                if ui
                    .selectable_label(st.lib_tab == LibTab::Discarded, ic(ph::TRASH, &format!("Discarded ({n})")))
                    .clicked()
                {
                    out.push(Msg::LibTab(LibTab::Discarded));
                }
            });
            group(ui, "open", |ui| {
                ui.separator();
                if ui.button(ic(ph::FOLDER_OPEN, "Open in file manager")).clicked() {
                    let dir = std::path::Path::new(increart_core::scan::INPUT_COLLECTIONS)
                        .join(st.lib_folder.clone().unwrap_or_default());
                    out.push(Msg::ShowInFileManager(dir));
                }
            });
            if st.lib_tab == LibTab::Grid {
                group(ui, "zoom", |ui| {
                    ui.separator();
                    crate::review::grid_zoom(ui, st, ZoomGrid::Library, out);
                });
                right_group(ui, "right", |ui| crate::widgets::details_toggle(ui, st, InfoPanel::Library, out));
            }
        });
    });
    if st.lib_tab == LibTab::Discarded {
        return discarded(ui, st, tex, out);
    }
    if st.lib_tab == LibTab::Folder {
        egui::CentralPanel::default().show(ui, |ui| crate::folderview::view(ui, st, tex, out));
        return;
    }
    if !st.info_hidden.contains(&InfoPanel::Library) {
        Panel::right("lib_info").default_size(300.0).show(ui, |ui| {
            match st.lib_info.as_ref().and_then(|id| st.idea(id)) {
                Some(e) => {
                    panel_header(ui, out, Some(Msg::ShowInfo(InfoPanel::Library, false)));
                    pinned_actions(ui, "lib_info_actions", |ui| library_actions(ui, e, out));
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .id_salt("lib_info_scroll")
                        .show(ui, |ui| idea_details(ui, st, &root, tex, e, out));
                }
                None => {
                    crate::widgets::nothing_selected(ui, "Idea", "No idea selected: click one to see it in full here.")
                }
            }
        });
    }
    egui::CentralPanel::default().show(ui, |ui| {
        let ideas = st.library_ideas();
        if ideas.is_empty() {
            let filtered = st.lib_filter != increart_gui_core::LibFilter::default();
            ui.weak(if filtered {
                "No ideas match the filters."
            } else {
                "No ideas here. Save ideas to it from the Idea inbox (1 / 2 / S)."
            });
            return;
        }
        // the selection in the grid's order
        let view: Vec<_> = ideas.iter().map(|e| &e.idea.id).collect();
        let sel: Vec<_> = view.iter().filter(|id| st.lib_selected.contains(*id)).map(|id| (*id).clone()).collect();
        let current = lib_current(st);
        if sel.len() > 1 {
            picks_bar(ui, st, &sel, false, out);
        } else {
            ui.weak("Click to select and see it on the right, Ctrl+click to add to the selection, Shift+click: a range, drag: a box; double-click: full size; right-click for actions.");
        }
        crate::review::grid_ctrl_scroll(ui, st, ZoomGrid::Library, out);
        let w = st.grid_cell_size(ZoomGrid::Library) as f32;
        let bg = pick_background(ui, st, &sel, Msg::LibClearSelection, out);
        let mut rects = vec![];
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (i, e) in ideas.iter().enumerate() {
                    let id = e.idea.id.clone();
                    let r = idea_card(ui, st, &root, tex, e, w, st.lib_selected.contains(&id), CardButtons::Library, out);
                    // dragged onto a collection: the selection when it's in
                    // it, else just this one
                    let r = crate::widgets::idea_drag_source(&r, &e.dir, || match sel.contains(&id) {
                        true => sel.clone(),
                        false => vec![id.clone()],
                    });
                    rects.push((id.clone(), r.rect));
                    if lib_current(st).as_ref() == Some(&id) {
                        crate::widgets::reveal_cursor(ui, "library", id.clone(), &r);
                    }
                    if i == 0 {
                        // where it is, for the screenshot tour's click check
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("tour_first_idea"), r.rect));
                    }
                    if r.clicked() {
                        if ui.input(|i| i.modifiers.shift) {
                            let p = pick_click(ui, &sel, current.as_ref(), &view, &id).unwrap_or_default();
                            out.push(Msg::LibSelectSet(p));
                        } else {
                            out.push(Msg::LibSelect { id: id.clone(), additive: ui.input(|i| i.modifiers.command) });
                        }
                    }
                    // the side panel opens on a single click only (a double-click
                    // previews); while it's open, a click shows the idea in it at once
                    let plain = r.clicked() && !ui.input(|i| i.modifiers.any());
                    if if st.lib_info.is_some() { plain } else { single_click(ui, &r) } {
                        out.push(Msg::LibInfo(Some(id.clone())));
                    }
                    r.context_menu(|ui| {
                        if sel.len() > 1 && sel.contains(&id) {
                            picks_menu(ui, st, &sel, false, out);
                            ui.separator();
                            ui.weak("this idea");
                        }
                        if !e.idea.images.is_empty() && ui.button(ic(ph::SQUARES_FOUR, "Tile its images…")).clicked() {
                            out.push(Msg::TileIdeas(vec![id.clone()]));
                            ui.close();
                        }
                        if ui.button(ic(ph::ROCKET_LAUNCH, "New project from idea…")).clicked() {
                            out.push(Msg::NewProjectFromIdeas(vec![id.clone()]));
                            ui.close();
                        }
                        if ui.button(ic(ph::PENCIL_SIMPLE, "Edit…")).clicked() {
                            out.push(Msg::OpenIdea(id.clone()));
                            ui.close();
                        }
                        rewrite_item(ui, st, &id, out);
                        if !e.idea.images.is_empty() && ui.button(ic(ph::IMAGES, "Add images as refs")).clicked() {
                            out.push(Msg::AddRefs(e.idea.images.iter().cloned().map(ImageRef::input).collect()));
                            ui.close();
                        }
                        let star = if e.idea.starred { ic(ph::STAR, "Unstar") } else { ic(ph::STAR, "Star") };
                        if ui.button(star).clicked() {
                            out.push(Msg::SetStar(id.clone(), !e.idea.starred));
                            ui.close();
                        }
                        if !(sel.len() > 1 && sel.contains(&id)) {
                            collection_items(ui, st, std::slice::from_ref(&id), out);
                        }
                        ui.separator();
                        if ui.button(ic(ph::TRAY, "Back to the Idea inbox")).clicked() {
                            out.push(Msg::Restore(id.clone(), IdeaLoc::Unreviewed));
                            ui.close();
                        }
                        if ui.button(ic(ph::TRASH, "Discard")).clicked() {
                            out.push(Msg::Restore(id.clone(), IdeaLoc::Discarded));
                            ui.close();
                        }
                        ui.separator();
                        copy_items(ui, &root, e, out);
                        ui.separator();
                        folder_item(ui, e, out);
                    });
                }
            });
        });
        if let Some(p) = crate::tiles::band_of(ui, &bg, &sel, &rects, |add| if add { sel.clone() } else { vec![] }) {
            out.push(Msg::LibSelectSet(p));
        }
    });
}

/// The Input library Grid's idea the keys move from: the one in the side
/// panel, else the first selected.
fn lib_current(st: &AppState) -> Option<increart_core::Id> {
    let ideas = st.library_ideas();
    let shown = |id: &increart_core::Id| ideas.iter().any(|e| &e.idea.id == id);
    st.lib_info
        .clone()
        .filter(shown)
        .or_else(|| ideas.iter().map(|e| e.idea.id.clone()).find(|id| st.lib_selected.contains(id)))
}

/// The Input library Grid's keys: move (selecting the idea and showing it
/// in the side panel), select all, new project, edit.
fn library_keys(ui: &egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    if ui.ctx().egui_wants_keyboard_input() || st.preview.is_some() || st.collect_ideas.is_some() {
        return;
    }
    let ideas: Vec<_> = st.library_ideas().into_iter().map(|e| e.idea.id.clone()).collect();
    let Some(last) = ideas.len().checked_sub(1) else { return };
    let current = lib_current(st);
    let at = current.as_ref().and_then(|c| ideas.iter().position(|id| id == c));
    ui.input(|i| {
        let to = if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown) {
            Some(at.map_or(0, |a| (a + 1).min(last)))
        } else if i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowUp) {
            Some(at.map_or(0, |a| a.saturating_sub(1)))
        } else if i.key_pressed(Key::Home) {
            Some(0)
        } else if i.key_pressed(Key::End) {
            Some(last)
        } else {
            None
        };
        if let Some(to) = to {
            out.push(Msg::LibSelect { id: ideas[to].clone(), additive: false });
            out.push(Msg::LibInfo(Some(ideas[to].clone())));
        }
        if i.modifiers.command && i.key_pressed(Key::A) {
            for id in ideas.iter().filter(|id| !st.lib_selected.contains(*id)) {
                out.push(Msg::LibSelect { id: id.clone(), additive: true });
            }
        }
        if !i.modifiers.command && (i.key_pressed(Key::P) || i.key_pressed(Key::Space)) {
            let sel: Vec<_> = ideas.iter().filter(|id| st.lib_selected.contains(*id)).cloned().collect();
            if !sel.is_empty() {
                out.push(Msg::NewProjectFromIdeas(sel));
            }
        }
        let sel: Vec<_> = ideas.iter().filter(|id| st.lib_selected.contains(*id)).cloned().collect();
        if sel.len() > 1 {
            if i.key_pressed(Key::T) && !i.modifiers.command {
                out.push(Msg::TileIdeas(sel));
            }
            if i.key_pressed(Key::Escape) {
                out.push(Msg::LibClearSelection);
            }
        } else if let Some(id) = &current
            && (i.key_pressed(Key::Enter) || i.key_pressed(Key::T))
        {
            out.push(Msg::OpenIdea(id.clone()));
        }
    });
}

/// A side panel's buttons, pinned to its bottom (the details scroll above).
fn pinned_actions(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    Panel::bottom(id).resizable(false).frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 6))).show(
        ui,
        |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            add(ui);
        },
    );
}

/// An Input library idea's buttons, under its details in the side panel.
fn library_actions(ui: &mut egui::Ui, e: &IdeaEntry, out: &mut Vec<Msg>) {
    let id = e.idea.id.clone();
    ui.horizontal_wrapped(|ui| {
        if ui.button(ic(ph::PENCIL_SIMPLE, "Edit…")).clicked() {
            out.push(Msg::OpenIdea(id.clone()));
        }
        if ui.button(ic(ph::ROCKET_LAUNCH, "New project")).on_hover_text("from this idea").clicked() {
            out.push(Msg::NewProjectFromIdeas(vec![id.clone()]));
        }
        if !e.idea.images.is_empty()
            && ui
                .button(ic(ph::IMAGES, "Add as refs"))
                .on_hover_text("to the current project's Generate panel")
                .clicked()
        {
            out.push(Msg::AddRefs(e.idea.images.iter().cloned().map(ImageRef::input).collect()));
        }
        if ui.button(ic(ph::FOLDER_PLUS, "Add to collection…")).clicked() {
            out.push(Msg::CollectIdeasAsk(vec![id.clone()]));
        }
        if ui.button(ic(ph::COPY, "Copy text")).clicked() {
            ui.ctx().copy_text(e.idea.text.clone());
        }
        if ui.button(ic(ph::FOLDER_OPEN, "Open folder")).clicked() {
            out.push(Msg::ShowInFileManager(e.dir.clone()));
        }
    });
}

/// The Idea inbox's current idea's buttons: the triage actions (the keys,
/// as buttons), then the rest.
fn inbox_actions(ui: &mut egui::Ui, e: &IdeaEntry, out: &mut Vec<Msg>) {
    let id = e.idea.id.clone();
    ui.horizontal_wrapped(|ui| {
        if ui.button(ic(ph::BOOKMARK_SIMPLE, "Save to input library…")).on_hover_text("1 / 2 / S").clicked() {
            out.push(Msg::SaveIdeas(vec![id.clone()]));
        }
        if ui.button(ic(ph::TRASH, "Discard")).on_hover_text("can be restored from Discarded (3 / B)").clicked() {
            out.push(Msg::ReviewIdea(id.clone(), Verdict::Bad));
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button(ic(ph::ROCKET_LAUNCH, "New project")).on_hover_text("from this idea (P / Space)").clicked() {
            out.push(Msg::NewProjectFromIdeas(vec![id.clone()]));
        }
        if ui.button(ic(ph::PENCIL_SIMPLE, "Edit…")).on_hover_text("Enter / T").clicked() {
            out.push(Msg::OpenIdea(id.clone()));
        }
        if ui.button(ic(ph::COPY, "Copy text")).clicked() {
            ui.ctx().copy_text(e.idea.text.clone());
        }
        if ui.button(ic(ph::FOLDER_OPEN, "Open folder")).clicked() {
            out.push(Msg::ShowInFileManager(e.dir.clone()));
        }
    });
}

fn panel_header(ui: &mut egui::Ui, out: &mut Vec<Msg>, close: Option<Msg>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Idea").strong());
        if let Some(close) = close
            && ui.small_button(ph::X).on_hover_text("close").clicked()
        {
            out.push(close);
        }
    });
}

/// An idea's title, star, images, text and tags; then its sections: the
/// idea (when, where, size), its source, each image's file metadata, its
/// prompt variants, and (when there is any) its images' use and projects.
fn idea_details(ui: &mut egui::Ui, st: &AppState, root: &Path, tex: &mut Textures, e: &IdeaEntry, out: &mut Vec<Msg>) {
    let id = e.idea.id.clone();
    ui.horizontal(|ui| {
        let star =
            if e.idea.starred { RichText::new(ph::STAR).color(Color32::GOLD) } else { RichText::new(ph::STAR).weak() };
        if ui.add(egui::Button::new(star).frame(false)).on_hover_text("star").clicked() {
            out.push(Msg::SetStar(id.clone(), !e.idea.starred));
        }
        let title = if e.idea.title.is_empty() { "(untitled)" } else { &e.idea.title };
        ui.add(egui::Label::new(RichText::new(title).strong()).wrap());
    });
    if !e.idea.images.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let size = if e.idea.images.len() == 1 { ui.available_width().min(280.0) } else { 86.0 };
            for img in &e.idea.images {
                let image = ImageRef::input(img.clone());
                let t = tex.thumb(root, &image);
                let tip = format!("{}\ndouble-click: full size", tex.dims_text(root, &image));
                let r = thumb(ui, t.as_ref(), size, false, None).on_hover_text(tip);
                if r.double_clicked() {
                    out.push(Msg::Preview(Some(image.clone())));
                }
                r.context_menu(|ui| crate::widgets::image_menu_items(ui, root, &image, out));
            }
        });
    }
    if !e.idea.text.is_empty() {
        ui.add_space(4.0);
        ui.add(egui::Label::new(&e.idea.text).wrap());
    }
    if !e.idea.tags.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.weak("tags");
            ui.label(e.idea.tags.join(", "));
        });
    }
    ui.add_space(4.0);
    section(ui, "idea_sec_idea", ic(ph::LIGHTBULB, "Idea"), true, |ui| {
        kv_grid(ui, "idea_kv", |ui| {
            kv(ui, "added", &crate::widgets::when(e.idea.created));
            let place = match &e.loc {
                IdeaLoc::Unreviewed => "Idea inbox".to_string(),
                IdeaLoc::Discarded => "Discarded".to_string(),
                IdeaLoc::Edited => "Edited copies (hidden)".to_string(),
                IdeaLoc::Library => "Input library".to_string(),
            };
            let place = match st.index.idea_collections(&e.idea.id).as_slice() {
                [] => place,
                cs => {
                    let cs: Vec<String> = cs.iter().map(|c| c.display().to_string()).collect();
                    format!("{place} · in {}", cs.join(", "))
                }
            };
            kv(ui, "where", &place);
            ui.weak("folder");
            let dir =
                egui::Label::new(RichText::new(e.dir.display().to_string()).small()).truncate().sense(Sense::click());
            if ui.add(dir).on_hover_text(format!("{}\nclick: open in the file manager", e.dir.display())).clicked() {
                out.push(Msg::ShowInFileManager(e.dir.clone()));
            }
            ui.end_row();
            ui.weak("id");
            ui.horizontal(|ui| {
                ui.label(RichText::new(id.to_string()).monospace().small());
                if ui.small_button(ph::COPY).on_hover_text("copy the id").clicked() {
                    ui.ctx().copy_text(id.to_string());
                }
            });
            ui.end_row();
            let text = &e.idea.text;
            if !text.is_empty() {
                let words = text.split_whitespace().count();
                kv(ui, "text", &format!("{words} words · {} characters", text.chars().count()));
            }
            let tags = image_tags(text);
            if !tags.is_empty() {
                kv(ui, "refs", &tags.join(" "));
            }
        });
    });
    section(ui, "idea_sec_source", ic(ph::ARROW_SQUARE_IN, "Source"), true, |ui| {
        source_section(ui, st, root, tex, &e.idea.source, out)
    });
    let copies: Vec<_> = e.idea.images.iter().flat_map(|i| st.edited_copies(&ImageRef::input(i.clone()))).collect();
    if !copies.is_empty() {
        let title = ic(ph::PENCIL_SIMPLE, &format!("Edited copies ({})", copies.len()));
        section(ui, "idea_sec_edited", title, true, |ui| {
            ui.horizontal_wrapped(|ui| {
                for c in copies.iter().flat_map(|c| &c.idea.images) {
                    let image = ImageRef::input(c.clone());
                    let t = tex.thumb(root, &image);
                    let tip = "double-click: full size · right-click: Crop / mark… again, use it";
                    let r = thumb(ui, t.as_ref(), 72.0, false, None).on_hover_text(tip);
                    if r.double_clicked() {
                        out.push(Msg::Preview(Some(image.clone())));
                    }
                    r.context_menu(|ui| crate::widgets::image_menu_items(ui, root, &image, out));
                }
            });
        });
    }
    if !e.idea.images.is_empty() {
        let missing: Vec<_> = e.idea.images.iter().filter(|i| !st.input_meta.contains_key(*i)).cloned().collect();
        if !missing.is_empty() {
            out.push(Msg::NeedInputMeta(missing));
        }
        let n = e.idea.images.len();
        let title = ic(ph::IMAGES, &format!("Images ({n})"));
        section(ui, ("idea_sec_images", n == 1), title, n == 1, |ui| {
            for (k, img) in e.idea.images.iter().enumerate() {
                if k > 0 {
                    ui.separator();
                }
                image_meta(ui, st, root, tex, img, k);
            }
        });
    }
    if !e.idea.variants.is_empty() {
        let title = ic(ph::LIST_BULLETS, &format!("Variants ({})", e.idea.variants.len()));
        section(ui, "idea_sec_variants", title, false, |ui| {
            for v in &e.idea.variants {
                ui.horizontal(|ui| {
                    let origin = match &v.origin {
                        increart_core::model::VariantOrigin::Manual => "by hand".to_string(),
                        increart_core::model::VariantOrigin::Rewritten { model, .. } => format!("rewritten by {model}"),
                    };
                    ui.weak(origin);
                    if ui.small_button(ph::COPY).on_hover_text("copy this variant").clicked() {
                        ui.ctx().copy_text(v.text.clone());
                    }
                });
                ui.add(egui::Label::new(&v.text).wrap());
                ui.add_space(4.0);
            }
        });
    }
    let usage = st.idea_usage(e).label();
    if usage.is_some() || !e.idea.projects.is_empty() {
        section(ui, "idea_sec_use", ic(ph::ROCKET_LAUNCH, "Use & projects"), true, |ui| {
            if let Some(u) = usage {
                ui.horizontal_wrapped(|ui| {
                    ui.weak("its images");
                    ui.label(u);
                });
            }
            if !e.idea.projects.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.weak("used in");
                    for p in &e.idea.projects {
                        let name = st.index.projects.iter().find(|x| &x.project.id == p).map(|x| x.name());
                        match name {
                            Some(name) => {
                                let r = ui.link(ic(ph::ROCKET_LAUNCH, &name)).on_hover_text("select it in Projects");
                                if r.clicked() {
                                    out.push(Msg::Tab(Tab::Projects));
                                    out.push(Msg::SelectProject(p.clone()));
                                }
                            }
                            None => {
                                ui.weak(format!("{p} (gone)"));
                            }
                        }
                    }
                });
            }
        });
    }
}

/// A side panel section, folded or not (remembered per `id`).
fn section(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: String,
    open: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::CollapsingHeader::new(RichText::new(title).strong()).id_salt(id).default_open(open).show(ui, body);
}

fn kv_grid(ui: &mut egui::Ui, id: &str, rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([10.0, 3.0]).show(ui, rows);
}

/// A key (weak) and its value, wrapped, as a grid row.
fn kv(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.weak(key);
    ui.add(egui::Label::new(value).wrap());
    ui.end_row();
}

/// The `<imageN>` tags in a prompt, each once, in order.
fn image_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = vec![];
    for (i, _) in text.match_indices("<image") {
        let rest = &text[i + 6..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && rest[digits.len()..].starts_with('>') {
            let tag = format!("<image{digits}>");
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    tags
}

/// How the idea came: typed or pasted, the dropped file's path, or the
/// output(s) it was made from (a click goes to them).
fn source_section(
    ui: &mut egui::Ui,
    st: &AppState,
    root: &Path,
    tex: &mut Textures,
    source: &increart_core::model::IdeaSource,
    out: &mut Vec<Msg>,
) {
    use increart_core::model::IdeaSource;
    if let IdeaSource::Edited { image, edits } = source {
        ui.label("cropped and marked from");
        ui.horizontal(|ui| {
            let t = tex.thumb(root, image);
            let r = thumb(ui, t.as_ref(), 56.0, false, None).on_hover_text("double-click: full size");
            if r.double_clicked() {
                out.push(Msg::Preview(Some(image.clone())));
            }
            r.context_menu(|ui| crate::widgets::image_menu_items(ui, root, image, out));
            ui.vertical(|ui| {
                let title = match image.store {
                    increart_core::Store::Input => st.input_title(image).filter(|t| !t.is_empty()).map(str::to_string),
                    increart_core::Store::Output => st
                        .output_link(image)
                        .and_then(|l| st.project_of_link(l))
                        .map(|p| format!("an output of {}", p.name())),
                };
                ui.label(title.unwrap_or_else(|| "(untitled)".into()));
                let crop = edits.crop.map(|[_, _, w, h]| format!("cropped to {w} × {h}"));
                let marks = (!edits.marks.is_empty()).then(|| format!("{} mark(s)", edits.marks.len()));
                ui.weak(crop.into_iter().chain(marks).collect::<Vec<_>>().join(" · "));
            });
        });
        return;
    }
    ui.label(IdeaOrigin::of(source).label());
    let outputs = match source {
        IdeaSource::DroppedFile { original_path } => {
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new(original_path).small()).wrap());
                if ui.small_button(ph::COPY).on_hover_text("copy the path").clicked() {
                    ui.ctx().copy_text(original_path.clone());
                }
            });
            return;
        }
        IdeaSource::FromOutput { image } => vec![image.clone()],
        IdeaSource::Tiled { images } => images.clone(),
        _ => return,
    };
    for image in outputs {
        ui.horizontal(|ui| {
            let t = tex.thumb(root, &image);
            let r = thumb(ui, t.as_ref(), 56.0, false, None).on_hover_text("double-click: full size");
            if r.double_clicked() {
                out.push(Msg::Preview(Some(image.clone())));
            }
            ui.vertical(|ui| match st.output_link(&image) {
                Some(link) => {
                    if let Some(p) = st.project_of_link(link) {
                        ui.label(ic(ph::ROCKET_LAUNCH, &p.name()));
                    }
                    let run = st.index.run(&link.name.run).map(|(_, r)| r);
                    let label = run.map(|r| r.label.clone()).filter(|l| !l.is_empty());
                    let run = label.unwrap_or_else(|| link.name.run.to_string());
                    ui.weak(format!("{run} · rev {} · seed {}", link.name.rev, link.name.seed));
                    let to = if link.verdict.is_none() { "Show in Review" } else { "Show in Output Library" };
                    if ui.link(to).clicked() {
                        out.push(Msg::GoToOutput(image.clone()));
                    }
                }
                None => {
                    ui.weak("the output is gone");
                }
            });
        });
    }
}

/// One idea image's facts: size, transparency, the file it was imported
/// from, its use and pixel hash (the file's metadata loads on first view).
fn image_meta(ui: &mut egui::Ui, st: &AppState, root: &Path, tex: &mut Textures, img: &increart_core::Id, k: usize) {
    let image = ImageRef::input(img.clone());
    kv_grid(ui, &format!("idea_img_kv{k}"), |ui| {
        let dims = tex.dims_text(root, &image);
        if !dims.is_empty() {
            kv(ui, "size", &dims);
        }
        kv(ui, "alpha", if st.index.transparent(&image) { "transparent" } else { "opaque" });
        kv(ui, "used", &st.usage(&image).label().unwrap_or_else(|| "not yet".into()));
        match st.input_meta.get(img) {
            None | Some(None) => {
                ui.weak("file");
                ui.spinner();
                ui.end_row();
            }
            Some(Some(m)) => {
                match &m.source {
                    Some(s) => {
                        kv(ui, "file", s.original_name.as_deref().unwrap_or("(clipboard)"));
                        if let Some(p) = &s.original_path {
                            kv(ui, "from", p);
                        }
                        let kb = s.original_size.div_ceil(1024);
                        let orig =
                            format!("{} · {} × {} · {kb} KB", s.original_format, s.original_width, s.original_height);
                        kv(ui, "original", &orig);
                        if let Some(t) = s.original_created {
                            kv(ui, "created", &crate::widgets::when(t));
                        }
                        if let Some(t) = s.original_mtime {
                            kv(ui, "modified", &crate::widgets::when(t));
                        }
                        if let Some(t) = s.imported {
                            kv(ui, "imported", &crate::widgets::when(t));
                        }
                        let conv =
                            if s.conversions.is_empty() { "unchanged".to_string() } else { s.conversions.join(", ") };
                        kv(ui, "on import", &conv);
                    }
                    None => kv(ui, "file", "no import record"),
                }
                if let Some(h) = &m.blake3 {
                    ui.weak("hash");
                    let short = h.get(..12).unwrap_or(h);
                    if ui
                        .add(egui::Label::new(RichText::new(short).monospace().small()).sense(Sense::click()))
                        .on_hover_text(format!("BLAKE3 of the pixels\n{h}\nclick: copy"))
                        .clicked()
                    {
                        ui.ctx().copy_text(h.clone());
                    }
                    ui.end_row();
                }
            }
        }
    });
}

/// The input library's smart folders, as the Output library's: by date,
/// by source, by project, by tag (each a list, open while one of it is
/// shown), then with images, starred, not in a collection.
fn smart_panel(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    use increart_gui_core::IdeaSmart;
    ui.add_space(6.0);
    ui.label(RichText::new("Smart folders").strong());
    let pick = |ui: &mut egui::Ui, out: &mut Vec<Msg>, s: IdeaSmart, label: String| {
        if ui.selectable_label(st.lib_smart.as_ref() == Some(&s), label).clicked() {
            out.push(Msg::LibSmart(s));
        }
    };
    let open = |f: fn(&IdeaSmart) -> bool| st.lib_smart.as_ref().is_some_and(f);
    egui::CollapsingHeader::new("by date").default_open(open(|s| matches!(s, IdeaSmart::Day(_)))).show(ui, |ui| {
        for (day, n) in st.idea_days() {
            pick(ui, out, IdeaSmart::Day(day.clone()), format!("{day} ({n})"));
        }
    });
    egui::CollapsingHeader::new("by source").default_open(open(|s| matches!(s, IdeaSmart::Origin(_)))).show(ui, |ui| {
        for (o, n) in st.idea_origins() {
            pick(ui, out, IdeaSmart::Origin(o), format!("{} ({n})", o.label()));
        }
    });
    egui::CollapsingHeader::new("by project").default_open(open(|s| matches!(s, IdeaSmart::Project(_)))).show(
        ui,
        |ui| {
            let projects = st.idea_projects();
            if projects.is_empty() {
                ui.weak("No idea has been used in a project yet.");
            }
            for (p, n) in projects {
                let name = st.index.project(&p).map(|e| e.name()).unwrap_or_default();
                pick(ui, out, IdeaSmart::Project(p), ic(ph::ROCKET_LAUNCH, &format!("{} ({n})", short(&name, 26))));
            }
        },
    );
    egui::CollapsingHeader::new("by tag").default_open(open(|s| matches!(s, IdeaSmart::Tag(_)))).show(ui, |ui| {
        let tags = st.idea_tags();
        if tags.is_empty() {
            ui.weak("No idea has a tag yet.");
        }
        for (t, n) in tags {
            pick(ui, out, IdeaSmart::Tag(t.clone()), ic(ph::TAG, &format!("{} ({n})", short(&t, 26))));
        }
    });
    for (s, name) in IdeaSmart::FLAT {
        let n = st.smart_ideas(&s).len();
        pick(ui, out, s, format!("{name} ({n})"));
    }
}

/// The input collections, as the Output library's: New collection, All
/// ideas, All collections (the ideas in any), then each collection with
/// how many ideas are in it and in the ones inside it; idea cards drop
/// onto them.
fn collections_panel(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Collections").strong());
        if ui.small_button(ph::FOLDER_PLUS).on_hover_text("New collection").clicked() {
            out.push(Msg::NewFolder(Tree::InputCollections, PathBuf::new()));
        }
    });
    let all = ui.selectable_label(
        st.lib_folder.is_none() && st.lib_smart.is_none(),
        ic(ph::BOOKS, &format!("All ideas ({})", lib_count(st, None))),
    );
    if all.on_hover_text("every idea in the input library").clicked() {
        out.push(Msg::LibFolder(None));
    }
    let any = Path::new("");
    let r = ui.selectable_label(
        st.lib_folder.as_deref() == Some(any),
        ic(ph::FOLDERS, &format!("All collections ({})", lib_count(st, Some(any)))),
    );
    if r.on_hover_text("the ideas in any collection").clicked() {
        out.push(Msg::LibFolder(Some(PathBuf::new())));
    }
    let mut folders = st.index.input_collection_folders.clone();
    folders.sort();
    for f in folders {
        let n = lib_count(st, Some(&f));
        ui.horizontal(|ui| {
            ui.add_space(12.0 * f.components().count() as f32);
            let name = f.file_name().unwrap_or_default().to_string_lossy();
            let r = ui.selectable_label(st.lib_folder.as_ref() == Some(&f), ic(ph::FOLDER, &format!("{name} ({n})")));
            if r.clicked() {
                out.push(Msg::LibFolder(Some(f.clone())));
            }
            crate::widgets::idea_collection_drop_target(ui, &r, &f, out);
            r.context_menu(|ui| {
                if ui.button(ic(ph::FOLDER_PLUS, "New collection inside")).clicked() {
                    out.push(Msg::NewFolder(Tree::InputCollections, f.clone()));
                    ui.close();
                }
                if ui.button(ic(ph::PENCIL_SIMPLE, "Rename")).clicked() {
                    out.push(Msg::StartRename(Tree::InputCollections, f.clone()));
                    ui.close();
                }
                if f.parent().is_some_and(|p| !p.as_os_str().is_empty())
                    && ui.button(ic(ph::ARROW_UP, "Move to the top level")).clicked()
                {
                    out.push(Msg::MoveNode(Tree::InputCollections, f.clone(), PathBuf::new()));
                    ui.close();
                }
                if ui.button(ic(ph::FOLDER_OPEN, "Open in file manager")).clicked() {
                    out.push(Msg::ShowInFileManager(Path::new(increart_core::scan::INPUT_COLLECTIONS).join(&f)));
                    ui.close();
                }
                if ui.button(ic(ph::TRASH, "Delete…")).clicked() {
                    let text = format!("Delete collection {name}? {}.", st.delete_summary(Tree::InputCollections, &f));
                    out.push(Msg::Ask(Confirm::new(
                        text,
                        "Delete",
                        Msg::DeleteNode(Tree::InputCollections, f.clone()),
                    )));
                    ui.close();
                }
            });
        });
    }
    smart_panel(ui, st, out);
}

fn lib_count(st: &AppState, folder: Option<&Path>) -> usize {
    let lib = st.index.ideas.iter().filter(|e| e.loc == IdeaLoc::Library);
    match folder {
        None => lib.count(),
        Some(f) => {
            let ids = st.index.ideas_in_collection(Some(f));
            lib.filter(|e| ids.contains(&e.idea.id)).count()
        }
    }
}

/// Ctrl+N from anywhere: the New idea form.
pub fn global_keys(ctx: &egui::Context, out: &mut Vec<Msg>) {
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::N)) {
        out.push(Msg::Tab(Tab::IdeaInbox));
        out.push(Msg::InboxTab(InboxTab::NewIdea));
    }
}

/// Save to input library, from the Idea inbox or the New idea form: the
/// input collections on the left (a filter, the tree, New collection under it),
/// the idea's name on the right (one idea or the draft; several keep their
/// titles), where it goes, Save and Save & new project.
pub fn save_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    // the name takes the focus as the dialog opens: once, not back from a
    // field clicked since
    let opening = folder_pick::opening(ctx, "save_idea", st.save_idea.is_some());
    let Some(d) = &st.save_idea else { return };
    let m = egui::Modal::new(egui::Id::unique("save_idea")).show(ctx, |ui| {
        ui.set_width(760.0);
        ui.heading(ic(ph::BOOKMARK_SIMPLE, "Save to input library"));
        ui.add_space(4.0);
        // the dialog as edited this frame
        let mut d = d.clone();
        // Enter in the filter or the new folder's name only leaves the field
        let mut field_enter = false;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(300.0);
                let picker = folder_pick::Picker {
                    id: "save_idea",
                    folders: &st.index.input_collection_folders,
                    root: (ph::BOOKS, "input library"),
                    selected: &d.folder,
                    filter: &d.filter,
                    new_folder: d.new_folder.as_deref(),
                    new_name: "",
                    noun: "collection",
                };
                let picked = folder_pick::show(ui, &picker, 280.0);
                field_enter |= picked.field_enter;
                if let Some(f) = picked.filter {
                    out.push(Msg::SaveIdeaFilter(f));
                }
                if let Some(f) = picked.select {
                    out.push(Msg::SaveIdeaFolder(f));
                }
                if let Some(f) = picked.new_folder {
                    out.push(Msg::SaveIdeaNewFolder(f.clone()));
                    d.new_folder = f;
                }
            });
            folder_pick::column_line(ui, 340.0);
            ui.vertical(|ui| {
                if d.ideas.len() <= 1 {
                    ui.label(RichText::new("Name").strong());
                    let mut name = d.name.clone();
                    let r = ui
                        .add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY).hint_text("idea name"));
                    if opening {
                        r.request_focus();
                    }
                    if name != d.name {
                        out.push(Msg::EditSaveIdeaName(name.clone()));
                        d.name = name;
                    }
                    let old = match d.ideas.first() {
                        Some(id) => st.idea(id).map(|e| e.idea.title.trim().to_string()).unwrap_or_default(),
                        None => st.draft.title.trim().to_string(),
                    };
                    if !old.is_empty() && old != d.name.trim() {
                        ui.weak(format!("was “{old}”"));
                    }
                } else {
                    ui.weak(format!("The {} ideas keep their names.", d.ideas.len()));
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Saved in").strong());
                let (at, exists) = increart_gui_core::save_idea_folder(st, &d);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(Path::new("input/library").display().to_string()).monospace());
                    if !at.as_os_str().is_empty() {
                        ui.label("in the collection");
                        ui.label(RichText::new(at.display().to_string()).monospace());
                    }
                    if !exists {
                        ui.weak("(new collection)");
                    }
                });
                if let Some(e) = increart_gui_core::save_idea_problem(&d) {
                    ui.add_space(4.0);
                    ui.colored_label(crate::widgets::BAD, e);
                }
            });
        });
        ui.add_space(4.0);
        let problem = increart_gui_core::save_idea_problem(&d);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        crate::widgets::dialog_buttons(ui, |ui| {
            let save = egui::Button::new(RichText::new(ic(ph::CHECK, "Save   Enter")).strong());
            if ui.add_enabled(problem.is_none(), save).clicked() || enter && !field_enter && problem.is_none() {
                out.push(Msg::ConfirmSaveIdea);
            }
            let new_project = egui::Button::new(ic(ph::ROCKET_LAUNCH, "Save & new project"));
            let what = if d.ideas.len() > 1 { "them" } else { "it" };
            if ui
                .add_enabled(problem.is_none(), new_project)
                .on_hover_text(format!(
                    "saves {what}, then makes a project from {what}, named as the idea, where the last \
                     project made from ideas went (asked when that name won't do)"
                ))
                .clicked()
            {
                out.push(Msg::SaveIdeaNewProject);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::CancelSaveIdea);
            }
        });
    });
    if m.should_close() {
        out.push(Msg::CancelSaveIdea);
    }
}

/// Add to collection… for input library ideas: the input collections (a
/// filter, the tree, New collection under it), what goes where, Add.
pub fn collect_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    folder_pick::opening(ctx, "collect_ideas", st.collect_ideas.is_some());
    let Some(d0) = &st.collect_ideas else { return };
    let mut d = d0.clone();
    let mut field_enter = false;
    let m = egui::Modal::new(egui::Id::unique("collect_ideas")).show(ctx, |ui| {
        ui.set_width(420.0);
        let n = d.ideas.len();
        let what = match d.ideas.as_slice() {
            [one] => st.idea(one).map(|e| e.idea.title.clone()).filter(|t| !t.is_empty()).unwrap_or("1 idea".into()),
            _ => format!("{n} ideas"),
        };
        ui.heading(ic(ph::FOLDER_PLUS, "Add to collection"));
        ui.weak(short(&what, 60));
        ui.add_space(4.0);
        let picker = folder_pick::Picker {
            id: "collect_ideas",
            folders: &st.index.input_collection_folders,
            root: (ph::FOLDERS, "the top level"),
            selected: &d.folder,
            filter: &d.filter,
            new_folder: d.new_folder.as_deref(),
            new_name: "",
            noun: "collection",
        };
        let picked = folder_pick::show(ui, &picker, 240.0);
        field_enter = picked.field_enter;
        if let Some(f) = picked.filter {
            d.filter = f;
        }
        if let Some(f) = picked.select {
            d.folder = f;
        }
        if let Some(f) = picked.new_folder {
            d.new_folder = f;
        }
        ui.add_space(6.0);
        let problem = d.problem(st);
        match (&problem, d.target(st)) {
            (Some(p), _) => {
                ui.colored_label(crate::widgets::BAD, p);
            }
            (None, Some((to, exists))) => {
                let already = st.index.ideas_in_collection(Some(&to));
                let there = d.ideas.iter().filter(|id| already.contains(id)).count();
                let mut s = format!("Into {}", to.display());
                if !exists {
                    s.push_str(" (new collection)");
                }
                if there > 0 {
                    s.push_str(&format!("; {there} of them in it already"));
                }
                ui.label(s);
            }
            (None, None) => {}
        }
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        crate::widgets::dialog_buttons(ui, |ui| {
            let add = egui::Button::new(RichText::new(ic(ph::FOLDER_PLUS, "Add   Enter")).strong());
            if ui.add_enabled(problem.is_none(), add).clicked() || enter && !field_enter && problem.is_none() {
                out.push(Msg::EditCollectIdeas(d.clone()));
                out.push(Msg::ConfirmCollectIdeas);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::CancelCollectIdeas);
            }
        });
    });
    if m.should_close() {
        out.push(Msg::CancelCollectIdeas);
    } else if d != *d0 && !out.iter().any(|m| matches!(m, Msg::ConfirmCollectIdeas)) {
        out.push(Msg::EditCollectIdeas(d));
    }
}
