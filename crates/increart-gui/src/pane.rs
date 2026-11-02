//! The libraries' left panes, as one: the Output library's (images) and the
//! Input library's (ideas) draw their Collections and Smart folders with
//! these, each its own tree and messages.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use increart_core::Tree;
use increart_core::scan::INPUT_COLLECTIONS;
use increart_gui_core::{AppState, Confirm, Msg, OutputSource};

use crate::widgets::{collection_drop_target, ic, idea_collection_drop_target, ph};

/// "Collections" with + (New collection at the top level).
pub fn collections_heading(ui: &mut egui::Ui, tree: Tree, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Collections").strong());
        if ui.small_button(ph::FOLDER_PLUS).on_hover_text("New collection").clicked() {
            out.push(new_collection(tree, PathBuf::new()));
        }
    });
}

/// All collections (what's in any), then each collection, indented as
/// nested, with how many are in it and in the ones inside it: a click shows
/// it, what's dragged from the grid drops on it, and its menu has New
/// collection inside, Rename, Move to the top level, Open in file manager
/// and Delete…. `tree` is Tree::Collections or Tree::InputCollections.
pub fn collection_rows(ui: &mut egui::Ui, st: &AppState, tree: Tree, out: &mut Vec<Msg>) {
    let any = Path::new("");
    let r = ui.selectable_label(
        selected(st, tree, any),
        ic(ph::FOLDERS, &format!("All collections ({})", count(st, tree, any))),
    );
    let tip =
        if tree == Tree::InputCollections { "the ideas in any collection" } else { "the images in any collection" };
    if r.on_hover_text(tip).clicked() {
        out.push(pick(tree, PathBuf::new()));
    }
    let mut folders = match tree {
        Tree::InputCollections => st.index.input_collection_folders.clone(),
        _ => st.index.collection_folders.clone(),
    };
    folders.sort();
    for f in folders {
        let n = count(st, tree, &f);
        ui.horizontal(|ui| {
            ui.add_space(12.0 * f.components().count() as f32);
            let name = f.file_name().unwrap_or_default().to_string_lossy();
            let r = ui.selectable_label(selected(st, tree, &f), ic(ph::FOLDER, &format!("{name} ({n})")));
            if r.clicked() {
                out.push(pick(tree, f.clone()));
            }
            match tree {
                Tree::InputCollections => idea_collection_drop_target(ui, &r, &f, out),
                _ => collection_drop_target(ui, &r, &f, out),
            };
            r.context_menu(|ui| {
                if ui.button(ic(ph::FOLDER_PLUS, "New collection inside")).clicked() {
                    out.push(new_collection(tree, f.clone()));
                    ui.close();
                }
                if ui.button(ic(ph::PENCIL_SIMPLE, "Rename")).clicked() {
                    out.push(Msg::StartRename(tree, f.clone()));
                    ui.close();
                }
                if f.parent().is_some_and(|p| !p.as_os_str().is_empty())
                    && ui.button(ic(ph::ARROW_UP, "Move to the top level")).clicked()
                {
                    out.push(Msg::MoveNode(tree, f.clone(), PathBuf::new()));
                    ui.close();
                }
                if ui.button(ic(ph::FOLDER_OPEN, "Open in file manager")).clicked() {
                    out.push(Msg::ShowInFileManager(Path::new(root(tree)).join(&f)));
                    ui.close();
                }
                if ui.button(ic(ph::TRASH, "Delete…")).clicked() {
                    let text = format!("Delete collection {name}? {}.", st.delete_summary(tree, &f));
                    out.push(Msg::Ask(Confirm::new(text, "Delete", Msg::DeleteNode(tree, f.clone()))));
                    ui.close();
                }
            });
        });
    }
}

/// "Smart folders", a little apart from the collections above.
pub fn smart_heading(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    ui.label(RichText::new("Smart folders").strong());
}

/// A smart folder that's a list (by date, by tag…): closed at first, open
/// while one of it is shown.
pub fn smart_group<R>(
    ui: &mut egui::Ui,
    title: &str,
    showing: bool,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::CollapsingResponse<R> {
    egui::CollapsingHeader::new(title).default_open(showing).show(ui, body)
}

fn root(tree: Tree) -> &'static str {
    match tree {
        Tree::InputCollections => INPUT_COLLECTIONS,
        _ => "collections",
    }
}

fn new_collection(tree: Tree, parent: PathBuf) -> Msg {
    match tree {
        Tree::InputCollections => Msg::NewFolder(tree, parent),
        _ => Msg::NewCollection(parent, vec![]),
    }
}

fn pick(tree: Tree, f: PathBuf) -> Msg {
    match tree {
        Tree::InputCollections => Msg::LibFolder(Some(f)),
        _ => Msg::OutputSource(OutputSource::Collection(f)),
    }
}

fn selected(st: &AppState, tree: Tree, f: &Path) -> bool {
    match tree {
        Tree::InputCollections => st.lib_folder.as_deref() == Some(f),
        _ => matches!(&st.output_source, OutputSource::Collection(c) if c == f),
    }
}

/// How many are in collection `f` and the ones inside it ("": any): the
/// input library's ideas, or the output links.
fn count(st: &AppState, tree: Tree, f: &Path) -> usize {
    match tree {
        Tree::InputCollections => crate::ideas::lib_count(st, Some(f)),
        _ => st.index.collection_links.iter().filter(|l| l.folder.starts_with(f)).count(),
    }
}
