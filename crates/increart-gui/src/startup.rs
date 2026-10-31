//! Start-up: open or create a library, then choose servers (DESIGN.md,
//! Startup: servers).

use eframe::egui::{self, Color32, RichText};
use increart_core::config::{self, ServerConfig};
use increart_core::kobold::{KoboldChat, KoboldRun, KoboldServer};
use increart_core::pe::PeDevice;
use increart_core::tiers::{self, Gpu, Layout};
use increart_gui_core::{
    AppState, Confirm, Editing, KoboldWizard, LibraryDialog, LibraryWizard, Msg, PeFile, PickKind, ServerDialog,
    ServerProgress, Without, WizardDownload, WizardGpu,
};

use crate::widgets::{ACCENT, ACCENT_MUTED, BAD, GOOD, NEUTRAL, dialog_buttons, dialog_footer, ic, ph, short_name};

/// How much of a library's or preset's name the main button shows.
const MAIN_NAME: usize = 24;

/// A start-up screen: a framed box in the middle of the window. Its height
/// is only known after drawing it, so it's kept per `key` and the frame is
/// redone when it changes.
fn centered(ui: &mut egui::Ui, key: &str, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let id = egui::Id::unique(("startup_height", key));
    let last: Option<f32> = ui.ctx().data(|d| d.get_temp(id));
    ui.vertical_centered(|ui| {
        ui.add_space(((ui.available_height() - last.unwrap_or(0.0)) / 2.0).max(16.0));
        let h = egui::Frame::window(ui.style())
            .inner_margin(20.0)
            .show(ui, |ui| {
                ui.set_width(width);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), add);
            })
            .response
            .rect
            .height();
        if last.is_none_or(|l| (l - h).abs() > 0.5) {
            ui.ctx().data_mut(|d| d.insert_temp(id, h));
            ui.ctx().request_discard("centring a start-up screen");
        }
    });
}

/// `back`: switching from this open library (offers a way back to it).
pub fn library(
    ui: &mut egui::Ui,
    d: &LibraryDialog,
    recent: &[String],
    back: Option<&std::path::Path>,
    archiving: Option<&increart_gui_core::Archiving>,
    out: &mut Vec<Msg>,
) {
    centered(ui, "library", 780.0, |ui| {
        ui.heading(ic(ph::BOOKS, if back.is_some() { "Switch library" } else { "Open a library" }));
        if let Some(b) = back {
            ui.horizontal(|ui| {
                let name = b.file_name().map_or(b.display().to_string(), |n| n.to_string_lossy().into());
                if ui
                    .add_enabled(!d.busy, egui::Button::new(ic(ph::ARROW_LEFT, &format!("Back to {name}   Esc"))))
                    .clicked()
                    || (d.wizard.is_none() && !d.busy && ui.input(|i| i.key_pressed(egui::Key::Escape)))
                {
                    out.push(Msg::CancelSwitch);
                }
                ui.weak("The servers stay connected.");
            });
        }
        if back.is_none() {
            // what a library is, for a first start
            ui.label(
                "A library is a folder on your disk that holds all your work: ideas and reference images, \
                 projects and their runs, and every image generated, each with the recipe that made it.",
            );
            ui.weak(
                "IncreArt works inside one library at a time, so open one to start. Keep several (one per \
                 theme or client, say) and switch between them from the top bar; copy or back up the folder \
                 to keep everything in it.",
            );
            ui.add_space(4.0);
        }
        ui.add_space(6.0);
        // at least the label and three rows tall, so a short list (or none)
        // doesn't leave a wide, flat dialog
        let row = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
        ui.vertical(|ui| {
            ui.set_min_height(4.0 * row);
            if recent.is_empty() {
                ui.label("No libraries yet: create a new one, or open one you already have.");
            } else {
                ui.label(RichText::new("Recent").strong());
                let example = increart_core::config::bundled_example_library();
                ui.add_enabled_ui(!d.busy, |ui| {
                    for (i, path) in recent.iter().enumerate() {
                        let (open, forget) = egui::Sides::new().shrink_left().truncate().spacing(16.0).show(
                            ui,
                            |ui| {
                                let label = RichText::new(ic(ph::BOOKS, path));
                                ui.selectable_label(false, label).on_hover_text(format!("Open {path}")).clicked()
                            },
                            |ui| {
                                // right to left: ×, then the tag (the list is most recent first: the
                                // last used one tagged, in a muted accent), so a long path can't hide it
                                let open_now = back.is_some_and(|b| b == std::path::Path::new(path));
                                let forget = !open_now
                                    && ui
                                        .small_button(ph::X)
                                        .on_hover_text("Remove from the list (the folder stays)")
                                        .clicked();
                                if open_now {
                                    ui.label(RichText::new("open now").color(ACCENT_MUTED));
                                } else if example.as_deref() == Some(std::path::Path::new(path)) {
                                    // shipped beside the app (maybe never opened yet)
                                    ui.label(RichText::new("example").color(ACCENT_MUTED));
                                } else if i == 0 {
                                    ui.label(RichText::new("last used").color(ACCENT_MUTED));
                                }
                                forget
                            },
                        );
                        if open {
                            out.push(Msg::OpenRecentLibrary(i));
                        }
                        if forget {
                            out.push(Msg::ForgetLibrary(i));
                        }
                    }
                });
            }
        });
        ui.separator();
        ui.add_space(6.0);
        // the main action on the right: the last used library (Enter), or
        // with none yet, creating one; the others grouped on the left
        // (switching: the last used other than the open one)
        let main = recent.iter().position(|p| back.is_none_or(|b| b != std::path::Path::new(p)));
        let create = || {
            let first = suggested_libraries().into_iter().next().map(|(_, p)| p).unwrap_or_default();
            Msg::LibraryWizard(Some((LibraryWizard::Create, first)))
        };
        let create_hover = "A new library: an empty folder for your ideas, projects and images";
        let (tool, action) = ui
            .add_enabled_ui(!d.busy, |ui| {
                dialog_footer(
                    ui,
                    |ui| {
                        let mut msg = None;
                        // with none yet, it is the main action (on the right)
                        if main.is_some()
                            && ui.button(ic(ph::FOLDER_PLUS, "New library…")).on_hover_text(create_hover).clicked()
                        {
                            msg = Some(create());
                        }
                        let open = ui
                            .button(ic(ph::FOLDER_OPEN, "Open existing…"))
                            .on_hover_text("A library folder that isn't in the list");
                        if open.clicked() {
                            let first = found_libraries(recent).into_iter().next().map(|(_, p)| p).unwrap_or_default();
                            msg = Some(Msg::LibraryWizard(Some((LibraryWizard::Open, first))));
                        }
                        let unpack = egui::Button::new(ic(ph::PACKAGE, "Unpack archive…"));
                        let unpack = ui.add_enabled(archiving.is_none(), unpack);
                        if unpack.on_hover_text(crate::app::UNPACK_ARCHIVE_HOVER).clicked() {
                            msg = Some(Msg::UnpackLibraryArchive);
                        }
                        msg
                    },
                    |ui| match main.map(|i| (i, &recent[i])) {
                        Some((i, path)) => {
                            let name = std::path::Path::new(path)
                                .file_name()
                                .map_or(path.clone(), |n| n.to_string_lossy().into());
                            let open =
                                RichText::new(ic(ph::BOOKS, &format!("Open {}   ⏎", short_name(&name, MAIN_NAME))))
                                    .strong()
                                    .color(ACCENT);
                            let enter = d.wizard.is_none()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                && !ui.ctx().egui_wants_keyboard_input();
                            (ui.button(open).on_hover_text(path).clicked() || enter)
                                .then_some(Msg::OpenRecentLibrary(i))
                        }
                        None => {
                            let new =
                                RichText::new(ic(ph::FOLDER_PLUS, "Create a new library…")).strong().color(ACCENT);
                            ui.button(new).on_hover_text(create_hover).clicked().then(create)
                        }
                    },
                )
            })
            .inner;
        out.extend(tool.into_iter().chain(action));
        if let Some(a) = archiving {
            ui.add_space(8.0);
            crate::app::archive_progress(ui, a, out);
        }
        let loading = egui::Id::unique("library_loading_since");
        if !d.busy {
            ui.ctx().data_mut(|m| m.remove::<f64>(loading));
        } else if d.wizard.is_none() {
            ui.add_space(8.0);
            // reading every project, run and idea: under a second, longer on a cold disk
            ui.add(
                egui::ProgressBar::new(loading_fraction(ui, loading)).text("Opening the library…").desired_width(360.0),
            );
            ui.ctx().request_repaint();
        }
        if let Some(e) = &d.error
            && d.wizard.is_none()
        {
            ui.add_space(8.0);
            ui.colored_label(BAD, e);
        }
    });
    if let Some(w) = d.wizard {
        library_wizard(ui.ctx(), d, w, recent, out);
    }
}

/// Where a new library could go: `~/IncreArt`, and `IncreArt` in the
/// current folder when that is somewhere else.
pub fn suggested_libraries() -> Vec<(&'static str, String)> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let cwd = std::env::current_dir().ok();
    let mut v = vec![];
    if let Some(h) = &home {
        v.push(("In your home folder", h.join("IncreArt").to_string_lossy().into_owned()));
    }
    if let Some(c) = cwd.filter(|c| Some(c) != home.as_ref()) {
        v.push(("In the current folder", c.join("IncreArt").to_string_lossy().into_owned()));
    }
    v
}

/// Libraries in the suggested places that aren't in the recent list.
pub fn found_libraries(recent: &[String]) -> Vec<(&'static str, String)> {
    suggested_libraries()
        .into_iter()
        .filter(|(_, p)| folder_state(p) == FolderState::Library && !recent.contains(p))
        .map(|(_, p)| ("Found", p))
        .collect()
}

#[derive(PartialEq)]
pub enum FolderState {
    Missing,
    Empty,
    Library,
    /// Has files, but no `library.rsn`.
    Other,
}

pub fn folder_state(path: &str) -> FolderState {
    let path = std::path::Path::new(path.trim());
    if path.join("library.rsn").exists() {
        return FolderState::Library;
    }
    match std::fs::read_dir(path).map(|mut r| r.next().is_none()) {
        Err(_) => FolderState::Missing,
        Ok(true) => FolderState::Empty,
        Ok(false) => FolderState::Other,
    }
}

/// The library wizards: pick a folder, then create a library there or open
/// the one there. Each offers the other when the folder calls for it.
fn library_wizard(ctx: &egui::Context, d: &LibraryDialog, w: LibraryWizard, recent: &[String], out: &mut Vec<Msg>) {
    let create = w == LibraryWizard::Create;
    let choices = if create { suggested_libraries() } else { found_libraries(recent) };
    let state = folder_state(&d.path);
    let path = d.path.trim().to_string();
    let m = egui::Modal::new(egui::Id::unique("library_wizard")).show(ctx, |ui| {
        ui.set_width(520.0);
        if create {
            ui.heading(ic(ph::FOLDER_PLUS, "Create a new library"));
            ui.label("Where should it live? The folder is created if it doesn't exist.");
        } else {
            ui.heading(ic(ph::FOLDER_OPEN, "Open an existing library"));
            ui.label("Pick the library's folder: the one with library.rsn in it.");
        }
        ui.add_space(8.0);
        ui.add_enabled_ui(!d.busy, |ui| {
            let mut other = true;
            for (label, p) in &choices {
                let on = path == *p;
                other &= !on;
                if ui.radio(on, format!("{label}: {p}")).clicked() {
                    out.push(Msg::SetLibraryPath(p.clone()));
                }
            }
            ui.horizontal(|ui| {
                if !choices.is_empty() && ui.radio(other, "Somewhere else:").clicked() && !other {
                    out.push(Msg::Pick(PickKind::LibraryFolder));
                }
                // empty while a listed folder is picked; typing here picks this option
                let shown = if other { d.path.clone() } else { String::new() };
                let mut p = shown.clone();
                let r = ui.add(
                    egui::TextEdit::singleline(&mut p).desired_width(ui.available_width() - 100.0).hint_text("/path"),
                );
                if choices.is_empty() {
                    crate::widgets::focus_until_done(&r);
                }
                if p != shown {
                    out.push(Msg::SetLibraryPath(p));
                }
                if ui.button(ic(ph::FOLDER_OPEN, "Browse…")).clicked() {
                    out.push(Msg::Pick(PickKind::LibraryFolder));
                }
            });
        });
        ui.add_space(6.0);
        // what's there, and the other wizard when it fits better
        let switch = |ui: &mut egui::Ui, out: &mut Vec<Msg>, text: &str, to: LibraryWizard| {
            if ui.add_enabled(!d.busy, egui::Button::new(text).small()).clicked() {
                out.push(Msg::LibraryWizard(Some((to, path.clone()))));
            }
        };
        if !path.is_empty() {
            ui.horizontal(|ui| match (create, &state) {
                (true, FolderState::Library) => {
                    ui.colored_label(BAD, "There is a library here already.");
                    switch(ui, out, "Open it instead", LibraryWizard::Open);
                }
                (true, FolderState::Other) => {
                    ui.colored_label(BAD, "This folder isn't empty: choose another one.");
                }
                (true, _) => {}
                (false, FolderState::Library) => {
                    ui.colored_label(GOOD, ic(ph::CHECK_CIRCLE, "An IncreArt library"));
                }
                (false, FolderState::Other) => {
                    ui.colored_label(BAD, "Not an IncreArt library (no library.rsn).");
                }
                (false, FolderState::Missing | FolderState::Empty) => {
                    let what = if state == FolderState::Missing { "No such folder." } else { "This folder is empty." };
                    ui.colored_label(BAD, what);
                    switch(ui, out, "Create a library here instead", LibraryWizard::Create);
                }
            });
        }
        if let Some(e) = &d.error {
            ui.colored_label(BAD, e);
        }
        ui.add_space(8.0);
        dialog_buttons(ui, |ui| {
            let (text, msg, ok) = if create {
                let ok = matches!(state, FolderState::Missing | FolderState::Empty);
                (ic(ph::FOLDER_PLUS, "Create library"), Msg::CreateLibrary, ok)
            } else {
                (ic(ph::FOLDER_OPEN, "Open library"), Msg::OpenLibrary, state == FolderState::Library)
            };
            let ok = ok && !d.busy && !path.is_empty();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let text = format!("{text}   Enter");
            if ui.add_enabled(ok, egui::Button::new(RichText::new(text).strong())).clicked() || (ok && enter) {
                out.push(msg);
            }
            if ui.add_enabled(!d.busy, egui::Button::new("Cancel   Esc")).clicked() {
                out.push(Msg::LibraryWizard(None));
            }
            if d.busy {
                ui.spinner();
            }
        });
    });
    if m.should_close() && !d.busy {
        out.push(Msg::LibraryWizard(None));
    }
}

/// "Start sd-server on ROCm0 :1234" / "Connect to sd-server at http://…" /
/// "Chat: Qwen3-VL" / "Start KoboldCpp on :5001".
pub fn describe(s: &ServerConfig) -> String {
    match s {
        ServerConfig::Start { device, port, .. } => format!("Start sd-server on {device} :{port}"),
        ServerConfig::Connect { url, .. } => format!("Connect to sd-server at {url}"),
        ServerConfig::Chat(c) => format!("Chat: {}", c.name),
        ServerConfig::Kobold(k) => {
            let chat = if k.chat.is_some() { " (and chat)" } else { "" };
            match &k.run {
                KoboldRun::Start { port, .. } => format!("Start KoboldCpp on :{port}{chat}"),
                KoboldRun::Connect { url, .. } => format!("Connect to KoboldCpp at {url}{chat}"),
            }
        }
    }
}

/// A started server's log file name.
fn log_file(s: &ServerConfig) -> String {
    increart_core::launcher::log_path(std::path::Path::new(""), s).display().to_string()
}

/// A path's text box, with a Browse… button picking it.
/// The label column of the wizard's KoboldCpp rows.
const LABEL_W: f32 = 76.0;

/// An example model directory, as this OS writes one (`~` works too).
fn models_hint() -> &'static str {
    if cfg!(windows) { r"C:\models" } else { "~/models" }
}

/// What a path field holds, for the note under it.
#[derive(Clone, Copy, PartialEq)]
enum PathKind {
    /// A file, a name on `PATH` or a command line: not checked (its
    /// `--version` or the server's start says).
    Binary,
    File,
    Folder,
    /// A folder made when it's needed.
    NewFolder,
}

fn path_kind(pick: PickKind) -> PathKind {
    match pick {
        PickKind::WizardBinary
        | PickKind::KoboldBinary
        | PickKind::SdServerBinary
        | PickKind::Kobold(_, PeFile::Binary)
        | PickKind::Pe(PeFile::Binary) => PathKind::Binary,
        PickKind::WizardModels => PathKind::NewFolder,
        PickKind::ModelDir(_) => PathKind::Folder,
        _ => PathKind::File,
    }
}

/// Opens `path`'s folder (a file's: the folder it's in) in the file
/// manager; greyed while there's none.
fn show_button(ui: &mut egui::Ui, path: &str, kind: PathKind, out: &mut Vec<Msg>) {
    let p = std::path::Path::new(path);
    // (a bare binary name or a command line: no folder)
    let named = p.is_absolute() && (kind != PathKind::Binary || p.exists());
    let dir = if kind == PathKind::Folder || kind == PathKind::NewFolder { Some(p) } else { p.parent() };
    let there = named && dir.is_some_and(|d| d.is_dir());
    let tip = if there {
        if dir == Some(p) { "Open the folder in the file manager" } else { "Show its folder in the file manager" }
    } else if kind == PathKind::NewFolder {
        "Not made yet: it is when the models download"
    } else {
        "Not there"
    };
    let b = ui.add_enabled(there, egui::Button::new(ph::ARROW_SQUARE_OUT));
    if b.on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
        out.push(Msg::ShowInFileManager(p.to_path_buf()));
    }
}

/// A path field and its Browse… and Show buttons, made a full path when left; its
/// text box's response. Under it: while typing, the full path it becomes; once left, a
/// file or folder that isn't there.
pub(crate) fn path_edit(
    ui: &mut egui::Ui,
    s: &mut String,
    hint: &str,
    pick: PickKind,
    out: &mut Vec<Msg>,
) -> egui::Response {
    let kind = path_kind(pick);
    let full = |s: &str| if kind == PathKind::Binary { config::full_binary(s) } else { config::full_path(s) };
    ui.vertical(|ui| {
        let r = ui
            .horizontal(|ui| {
                let r =
                    ui.add(egui::TextEdit::singleline(s).desired_width(ui.available_width() - 68.0).hint_text(hint));
                if ui.button(ph::FOLDER_OPEN).on_hover_text("Browse…").clicked() {
                    out.push(Msg::Pick(pick));
                }
                show_button(ui, &full(s), kind, out);
                r
            })
            .inner;
        if r.lost_focus() {
            *s = full(s);
        }
        let f = full(s);
        if r.has_focus() {
            if f != s.trim() {
                ui.weak(format!("→ {f}"));
            }
        } else if !f.is_empty() && kind != PathKind::Binary && !std::path::Path::new(&f).exists() {
            match kind {
                PathKind::NewFolder => ui.weak("A new folder, made when the models download."),
                PathKind::Folder => ui.colored_label(BAD, "No such folder"),
                _ => ui.colored_label(BAD, "No such file"),
            };
        }
        r
    })
    .inner
}

/// A KoboldCpp entry's settings (entry `i`), under its row; started with
/// the KoboldCpp binary in use, its chat model the wizard's if downloaded.
fn kobold_fields(
    ui: &mut egui::Ui,
    st: &AppState,
    i: usize,
    k: &mut KoboldServer,
    has_chat_entry: bool,
    out: &mut Vec<Msg>,
) {
    egui::Grid::new(("kobold", ui.next_auto_id())).num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
        ui.label("Runs");
        ui.horizontal(|ui| {
            let start = matches!(k.run, KoboldRun::Start { .. });
            if ui.radio(start, "started by IncreArt").clicked() && !start {
                k.run = KoboldRun::Start {
                    binary: increart_gui_core::kobold_binary(st),
                    config: String::new(),
                    host: "127.0.0.1".into(),
                    port: 5001,
                    device: Default::default(),
                    args: vec![],
                };
            }
            if ui.radio(!start, "already running").clicked() && start {
                k.run = KoboldRun::Connect { url: k.base_url(), password: String::new(), device: k.device() };
            }
        });
        ui.end_row();
        let wide = |ui: &mut egui::Ui, s: &mut String, hint: &str| {
            ui.add(egui::TextEdit::singleline(s).desired_width(f32::INFINITY).hint_text(hint));
        };
        match &mut k.run {
            KoboldRun::Start { binary, config, host, port, device, args } => {
                ui.label("Binary");
                let hint = "koboldcpp, or a command line (python3 koboldcpp.py)";
                path_edit(ui, binary, hint, PickKind::Kobold(i, PeFile::Binary), out);
                ui.end_row();
                ui.label("Settings");
                let hint = ".kcpps from KoboldCpp's launcher (optional; the rest goes over it)";
                path_edit(ui, config, hint, PickKind::Kobold(i, PeFile::Config), out);
                ui.end_row();
                ui.label("Address");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(host).desired_width(120.0));
                    ui.label("port");
                    ui.add(egui::DragValue::new(port).range(1024..=65535));
                });
                ui.end_row();
                ui.label("Runs on");
                ui.horizontal(|ui| {
                    let (auto, cpu) = (*device == PeDevice::Auto, matches!(device, PeDevice::Cpu { .. }));
                    if ui.radio(auto, "its choice").clicked() {
                        *device = PeDevice::Auto;
                    }
                    if ui.radio(cpu, "CPU").clicked() && !cpu {
                        *device = PeDevice::Cpu { threads: 0 };
                    }
                    let gpu = matches!(device, PeDevice::Gpu { .. });
                    if ui.radio(gpu, "GPUs").clicked() && !gpu {
                        *device = PeDevice::Gpu { devices: vec![], runtime: st.gpu_runtime };
                    }
                    if let PeDevice::Gpu { devices, runtime } = device {
                        crate::pe_settings::runtime_choice(ui, ("kobold_runtime", i), runtime);
                        let mut text = devices.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
                        let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(70.0).hint_text("all"));
                        r.on_hover_text(format!("GPU numbers as {} lists them; empty: every GPU", runtime.tool()));
                        *devices = text.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                    }
                });
                ui.end_row();
                ui.label("More args");
                let mut text = args.join(" ");
                ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(f32::INFINITY)
                        .hint_text("--sdflashattention …"),
                );
                *args = text.split_whitespace().map(String::from).collect();
                ui.end_row();
            }
            KoboldRun::Connect { url, password, device } => {
                ui.label("Address");
                wide(ui, url, "http://127.0.0.1:5001");
                ui.end_row();
                ui.label("Password");
                ui.add(
                    egui::TextEdit::singleline(password).password(true).desired_width(160.0).hint_text("if it has one"),
                );
                ui.end_row();
                ui.label("Device");
                ui.add(egui::TextEdit::singleline(device).desired_width(100.0).hint_text("for recipes"));
                ui.end_row();
            }
        }
        ui.label("Chat");
        ui.vertical(|ui| {
            let mut on = k.chat.is_some();
            let r = ui.checkbox(&mut on, "also prompt enhance's chat server");
            if has_chat_entry {
                r.on_hover_text("prompt enhance shares its asks with the preset's chat servers");
            }
            if on != k.chat.is_some() {
                // the wizard's Qwen3-VL, if it's downloaded
                k.chat = on.then(|| match tiers::chat_files(&st.config.sdserver.model_dirs) {
                    Some((model, mmproj)) => KoboldChat { model, mmproj, context: 8192 },
                    None => KoboldChat::default(),
                });
            }
            if let (Some(c), KoboldRun::Start { .. }) = (&mut k.chat, &k.run) {
                // labelled, the column's width alike
                let label = |ui: &mut egui::Ui, text: &str| ui.add_sized([56.0, 18.0], egui::Label::new(text));
                ui.horizontal(|ui| {
                    label(ui, "Model").on_hover_text("the chat model, a .gguf");
                    path_edit(ui, &mut c.model, "chat model .gguf", PickKind::Kobold(i, PeFile::Model), out);
                });
                ui.horizontal(|ui| {
                    label(ui, "mmproj").on_hover_text("the vision projector, a .gguf: lets it see the images");
                    let hint = "vision mmproj .gguf (empty: text only)";
                    path_edit(ui, &mut c.mmproj, hint, PickKind::Kobold(i, PeFile::Mmproj), out);
                });
                ui.horizontal(|ui| {
                    label(ui, "Context");
                    ui.add(egui::DragValue::new(&mut c.context).range(0..=131072).speed(256));
                    ui.weak("tokens (0: KoboldCpp's default)");
                });
            }
        });
        ui.end_row();
    });
}

pub fn servers(ui: &mut egui::Ui, st: &AppState, d: &ServerDialog, out: &mut Vec<Msg>) {
    centered(ui, "servers", 780.0, |ui| {
        ui.heading(ic(ph::HARD_DRIVES, "Servers"));
        ui.label(
            "The servers for this session: KoboldCpp or sd-server to generate images with, and a chat server \
             for prompt enhance (KoboldCpp can be both). Connected servers keep running when IncreArt exits.",
        );
        if let Some(n) = &d.notice {
            ui.add_space(4.0);
            ui.colored_label(NEUTRAL, ic(ph::WARNING, n));
        }
        ui.add_space(10.0);
        // Locked (and greyed) while opening; the progress below stays readable.
        let locked = ui.add_enabled_ui(!d.opening, |ui| presets(ui, st, d, out));
        if d.opening {
            // Disabled widgets only fade a little: dim the whole area too.
            let r = locked.response.rect.expand(6.0);
            ui.painter().rect_filled(r, 6.0, egui::Color32::from_black_alpha(120));
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                ic(ph::LOCK_SIMPLE, "locked while the servers open"),
                egui::FontId::proportional(16.0),
                egui::Color32::from_gray(200),
            );
        }
        if let Some(problem) = settings_problem(st)
            && !d.opening
        {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.colored_label(NEUTRAL, ic(ph::WARNING, &problem));
                if ui.small_button("Fix…").clicked() {
                    out.push(Msg::ServerSettings(true));
                }
            });
        }
        if !d.progress.is_empty() {
            ui.separator();
            progress(ui, st, d, out);
        }
        ui.separator();
        if d.wizard {
            server_wizard(ui.ctx(), st, d, out);
        }
        if let Some(editing) = d.editing {
            editor_window(ui.ctx(), st, d, editing, out);
        }
        if d.settings {
            settings_window(ui.ctx(), st, d, out);
        }
        if let Some(w) = &d.without {
            without_window(ui.ctx(), st, w, out);
        }
        let modal = d.editing.is_some()
            || d.wizard
            || d.settings
            || d.without.is_some()
            || st.confirm.is_some()
            || st.pe.editor.is_some();
        // the settings on the left, the ways out on the right
        let ((settings, logs), _) = dialog_footer(
            ui,
            |ui| {
                let settings = !d.opening
                    && ui
                        .button(ic(ph::GEAR, "Server settings…"))
                        .on_hover_text(
                            "KoboldCpp and sd-server binaries, the model directories: shared by every preset",
                        )
                        .clicked();
                let logs = ui
                    .button(ic(ph::SCROLL, "Logs…"))
                    .on_hover_text("the logs of the servers IncreArt started")
                    .clicked();
                (settings, logs)
            },
            |ui| ways_out(ui, st, d, modal, out),
        );
        if settings {
            out.push(Msg::ServerSettings(true));
        }
        if logs {
            out.push(Msg::OpenLogs(None));
        }
    });
}

/// Open the last used preset, continue, close, or (while opening) continue
/// in background / cancel.
fn ways_out(ui: &mut egui::Ui, st: &AppState, d: &ServerDialog, modal: bool, out: &mut Vec<Msg>) {
    if d.opening {
        if ui
            .button(ic(ph::ARROW_RIGHT, "Continue in background"))
            .on_hover_text("use IncreArt meanwhile; the servers join when they are up")
            .clicked()
        {
            out.push(Msg::ContinueInBackground);
        }
        if ui.button(ic(ph::X, "Cancel")).on_hover_text("stop opening (and the servers already started)").clicked() {
            out.push(Msg::CancelOpening);
        }
        return;
    }
    // the last used preset, named, as the main button (and Enter); not
    // from the main window while servers run: that is what Close keeps
    let last = st
        .config
        .last_preset
        .as_ref()
        .and_then(|n| st.config.server_presets.iter().position(|p| &p.name == n).map(|i| (i, n)));
    if let Some((i, name)) = last
        && !(d.from_main && !st.servers.is_empty())
    {
        let open =
            RichText::new(ic(ph::PLAY, &format!("Open {}   ⏎", short_name(name, MAIN_NAME)))).strong().color(ACCENT);
        let enter = !modal && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !ui.ctx().egui_wants_keyboard_input();
        if ui.button(open).on_hover_text(format!("{name}: the preset used last time")).clicked() || enter {
            out.push(Msg::OpenPreset(i));
        }
    }
    let up = d.progress.iter().filter(|p| **p == ServerProgress::Up).count();
    let failed = d.progress.iter().any(|p| matches!(p, ServerProgress::Failed(_)));
    if failed && up > 0 && ui.button(format!("Continue with {up} server(s)")).clicked() {
        out.push(Msg::ContinueWithServers);
    }
    // from the main window: back, keeping the servers in use (also Esc)
    if d.from_main {
        let keep = if st.servers.is_empty() { "Close" } else { "Close, keep the servers in use" };
        let esc = !modal && ui.input(|i| i.key_pressed(egui::Key::Escape));
        if ui.button(RichText::new(ic(ph::ARROW_LEFT, keep)).strong()).clicked() || esc {
            out.push(Msg::CloseServerDialog);
        }
    }
    let without = if d.from_main && !st.servers.is_empty() {
        "Stop the servers, continue without"
    } else {
        "Continue without servers"
    };
    // from the main window with no servers, Close does the same
    if !(d.from_main && st.servers.is_empty()) && ui.button(without).clicked() {
        out.push(Msg::AskContinueWithoutServers);
    }
}

/// What is wrong with the sd-server settings, when a preset starts servers:
/// a missing binary or model directory.
fn settings_problem(st: &AppState) -> Option<String> {
    let starts =
        st.config.server_presets.iter().flat_map(|p| &p.servers).any(|s| matches!(s, ServerConfig::Start { .. }));
    if !starts {
        return None;
    }
    st.sdserver_problem()
}

/// Per server: "starting — 12 s", up, or the error.
fn progress(ui: &mut egui::Ui, st: &AppState, d: &ServerDialog, out: &mut Vec<Msg>) {
    let secs = (st.now - d.started).max(0.0);
    if !st.stopping.is_empty() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak(format!("stopping the servers in use first ({})", st.stopping.join(", ")));
        });
    }
    for (i, (s, p)) in d.draft.sd_servers().iter().zip(&d.progress).enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(s.name()).strong());
            ui.weak(describe(s));
            match p {
                ServerProgress::Opening => {
                    ui.spinner();
                    let what = if s.is_started() { "starting" } else { "connecting" };
                    ui.label(format!("{what} — {secs:.0} s"));
                }
                ServerProgress::Up => {
                    ui.colored_label(GOOD, ic(ph::CHECK_CIRCLE, "up"));
                }
                ServerProgress::Failed(_) => {
                    ui.colored_label(BAD, ic(ph::WARNING, "failed"));
                }
            }
            if s.is_started() {
                crate::projects::log_button(ui, log_file(s), out);
            }
        });
        // the log's last line on its own line, cut to the dialog's width: on the
        // row above it would widen the dialog off centre; kept while opening so
        // the dialog doesn't jump when the first line arrives
        // why, below the row and wrapped to the dialog's width (on the row it
        // would widen the dialog off centre); of a long log tail, the last
        // lines, all of it on hover and in the log
        if let ServerProgress::Failed(e) = p {
            let e = e.strip_prefix(&format!("{}: ", s.name())).unwrap_or(e);
            let lines: Vec<&str> = e.lines().collect();
            let shown = match lines.len() {
                0..=3 => e.to_string(),
                n => format!("{}\n…\n{}", lines[0], lines[n - 2..].join("\n")),
            };
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                let r = ui.add(egui::Label::new(RichText::new(shown).color(BAD)).wrap());
                if lines.len() > 3 {
                    r.on_hover_text(e);
                }
            });
            // its binary missing: where to set it
            if s.is_started() && (e.contains(" not found: ") || e.contains(" isn't there")) {
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.weak("Set its binary in");
                    if ui.add_enabled(!d.opening, egui::Button::new(ic(ph::GEAR, "Server settings…"))).clicked() {
                        out.push(Msg::ServerSettings(true));
                    }
                });
            }
        }
        if matches!(p, ServerProgress::Opening) {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.set_min_height(ui.spacing().interact_size.y);
                if let Some(line) = st.start_log.get(i).filter(|l| !l.is_empty()) {
                    ui.add(egui::Label::new(RichText::new(line).weak().monospace()).truncate())
                        .on_hover_text(format!("from the server's log\n{line}"));
                }
            });
        }
    }
    // the chat server: on its own, never holding the sd-servers up
    if let Some(c) = d.draft.chat() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&c.name).strong());
            ui.weak("chat server");
            let (dot, state, color) = crate::app::pe_state(ui, st);
            let state = state.split_once(": ").map_or(state.as_str(), |(_, s)| s);
            if st.pe.status == increart_gui_core::PeStatus::Starting {
                ui.spinner();
            }
            ui.colored_label(color, format!("{dot} {state}"));
            if matches!(c.run, increart_core::pe::PeRun::KoboldCpp { .. }) {
                crate::projects::log_button(ui, increart_core::pe::log_name(&c.name), out);
            }
        });
    }
}

/// The presets: a click opens one, ✎ edits it, the bin deletes it (asks).
fn presets(ui: &mut egui::Ui, st: &AppState, d: &ServerDialog, out: &mut Vec<Msg>) {
    ui.label(RichText::new("Presets").strong());
    if st.config.server_presets.is_empty() {
        ui.label("No presets yet.");
        ui.add_space(4.0);
        // the way to start: the main action while there are no presets
        ui.horizontal(|ui| {
            if ui.button(RichText::new(ic(ph::MAGIC_WAND, WIZARD)).strong().color(ACCENT)).clicked() {
                out.push(Msg::ServerWizard(true));
            }
        });
        ui.weak("Or make one by hand with New preset (KoboldCpp or sd-server).");
        ui.add_space(6.0);
    }
    let last = st.config.last_preset.as_ref().and_then(|n| st.config.server_presets.iter().position(|p| &p.name == n));
    for (i, p) in st.config.server_presets.iter().enumerate() {
        let summary = p.servers.iter().map(describe).collect::<Vec<_>>().join(" · ");
        let text = format!("{}\n{}\n{}", p.name, summary, p.roles_line());
        // the preset used last time tagged, the tag in a muted accent (as in the library list)
        let text = RichText::new(text);
        let editing = d.editing == Some(Editing::Preset(i));
        // the text gives way to the buttons (wraps), a gap kept between them
        let (open, (edit, delete)) = egui::Sides::new().shrink_left().wrap().spacing(16.0).show(
            ui,
            |ui| ui.selectable_label(editing, text).on_hover_text("Open").clicked(),
            |ui| {
                // right to left: ✎ pinned to the right edge, the bin next to it, then the tag
                let edit = ui.small_button(ph::PENCIL_SIMPLE).on_hover_text("Edit").clicked();
                let delete = ui.small_button(ph::TRASH).on_hover_text("Delete preset…").clicked();
                if last == Some(i) {
                    ui.label(RichText::new("last used").color(ACCENT_MUTED));
                }
                (edit, delete)
            },
        );
        if open {
            out.push(Msg::OpenPreset(i));
        }
        if edit {
            out.push(Msg::EditPreset(i));
        }
        if delete {
            out.push(Msg::Ask(Confirm::new(format!("Delete preset {}?", p.name), "Delete", Msg::DeletePreset(i))));
        }
    }
    ui.add_space(6.0);
    // in a row, so the column doesn't stretch it to full width
    ui.horizontal(|ui| {
        if ui.button(ic(ph::PLUS, "New preset")).clicked() {
            out.push(Msg::NewPreset);
        }
        if !st.config.server_presets.is_empty()
            && ui
                .button(ic(ph::MAGIC_WAND, WIZARD))
                .on_hover_text("KoboldCpp with Qwen-Image, set up by each GPU's memory, as a new preset")
                .clicked()
        {
            out.push(Msg::ServerWizard(true));
        }
    });
    ui.add_space(6.0);
    ui.weak("Click a preset to open it");
}

/// **Continue without servers**: nothing generates meanwhile; prompt
/// enhance can go on with one of the presets' chat servers.
fn without_window(ctx: &egui::Context, st: &AppState, w: &Without, out: &mut Vec<Msg>) {
    let m = egui::Modal::new(egui::Id::unique("continue_without")).show(ctx, |ui| {
        ui.set_width(500.0);
        ui.heading(ic(ph::ARROW_RIGHT, "Continue without servers"));
        ui.label(if st.servers.is_empty() {
            "Browsing, review and the libraries work, but nothing generates until an sd-server is connected \
             (Servers… in the top bar)."
        } else {
            "The sd-servers in use stop. Nothing generates until one is connected again; queued runs wait."
        });
        if !st.active_runs.is_empty() {
            ui.label(format!(
                "{} run(s) are generating: the images being made now are dropped and made again later.",
                st.active_runs.len()
            ));
        }
        ui.add_space(8.0);
        // one per process; the one in use as it is (it may have been
        // edited in its preset since)
        let mut list: Vec<increart_core::pe::PeServer> = st.config.chat_servers().into_iter().cloned().collect();
        if let Some(c) = &st.pe.server {
            match list.iter_mut().find(|x| x.same_process(c)) {
                Some(x) => *x = c.clone(),
                None => list.insert(0, c.clone()),
            }
        }
        if list.is_empty() {
            ui.weak("Prompt enhance: no preset has a chat server.");
        } else {
            ui.label(RichText::new(ic(ph::MAGIC_WAND, "Prompt enhance meanwhile")).strong());
            let mut pick = w.chat.clone();
            ui.radio_value(&mut pick, None, "None");
            for c in &list {
                ui.radio_value(&mut pick, Some(c.clone()), &c.name).on_hover_text(crate::pe_settings::describe(c));
            }
            if pick != w.chat {
                out.push(Msg::WithoutDialog(Some(Without { chat: pick })));
            }
        }
        ui.add_space(8.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        dialog_buttons(ui, |ui| {
            if ui.button(RichText::new(ic(ph::ARROW_RIGHT, "Continue   Enter")).strong()).clicked() || enter {
                out.push(Msg::ContinueWithoutServers);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::WithoutDialog(None));
            }
        });
    });
    if m.should_close() {
        out.push(Msg::WithoutDialog(None));
    }
}

/// The setup wizard's button.
const WIZARD: &str = "Set up KoboldCpp for your GPUs…";

/// **Server settings…**: KoboldCpp, sd-server and the model directories,
/// shared by every preset.
fn settings_window(ctx: &egui::Context, st: &AppState, d: &ServerDialog, out: &mut Vec<Msg>) {
    let m = egui::Modal::new(egui::Id::unique("sd_server_settings")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading(ic(ph::GEAR, "Server settings"));
        ui.label("Shared by every preset.");
        ui.add_space(8.0);
        settings(ui, st, d, out);
        ui.add_space(8.0);
        dialog_buttons(ui, |ui| {
            if ui.button("Close   Esc").clicked() {
                out.push(Msg::ServerSettings(false));
            }
        });
    });
    if m.should_close() {
        out.push(Msg::ServerSettings(false));
    }
}

fn settings(ui: &mut egui::Ui, st: &AppState, d: &ServerDialog, out: &mut Vec<Msg>) {
    ui.label(RichText::new(ic(ph::IMAGE, "KoboldCpp")).strong());
    ui.weak(
        "Generates images, and runs prompt enhance too. New KoboldCpp entries and the setup wizard start this one.",
    );
    kobold_binary_field(ui, st, d, out);
    ui.add_space(12.0);
    ui.label(RichText::new(ic(ph::PLAY_CIRCLE, "sd-server")).strong());
    let mut c = st.config.sdserver.clone();
    sdserver_binary_field(ui, &mut c, out);
    ui.horizontal(|ui| {
        let label = if d.devices.is_some() { "List again" } else { "List devices (sd-server --list-devices)" };
        if ui.button(ic(ph::LIST_MAGNIFYING_GLASS, label)).clicked() {
            out.push(Msg::ListDevices);
        }
    });
    match &d.devices {
        None => {}
        Some(Ok(list)) if list.is_empty() => {
            ui.weak("none listed");
        }
        Some(Ok(list)) => {
            for l in list {
                ui.monospace(l);
            }
        }
        Some(Err(e)) => {
            ui.colored_label(BAD, e);
        }
    }
    ui.add_space(12.0);
    ui.label(RichText::new(ic(ph::FOLDER, "Both")).strong());
    models_field(ui, &mut c, MODEL_DIRS_NOTE, out);
    let wizard = match c.model_dirs.first() {
        Some(d) => format!("into {}", std::path::Path::new(d).join(tiers::MODEL_SUBDIR).display()),
        None => format!("into {}/ in it", tiers::MODEL_SUBDIR),
    };
    ui.weak(format!("KoboldCpp loads the recipes' model files from these too; the setup wizard downloads {wizard}."));
    leave_running_field(ui, &mut c);
    if c != st.config.sdserver {
        out.push(Msg::SetSdServer(c));
    }
    let mut check = !st.config.skip_ram_check;
    if ui
        .checkbox(&mut check, "Wait for free RAM before loading a model")
        .on_hover_text(
            "A KoboldCpp that offloads its weights (--sdoffloadcpu) waits up to 2 s for them to fit in free RAM with 8 GiB to spare",
        )
        .changed()
    {
        out.push(Msg::SetSkipRamCheck(!check));
    }
    ui.add_space(12.0);
    ui.label(RichText::new(ic(ph::TIMER, "Timeouts")).strong());
    crate::settings::timeouts(ui, st, out);
}

/// The shared KoboldCpp binary, and its version (checked when the field is
/// left, or a file picked).
fn kobold_binary_field(ui: &mut egui::Ui, st: &AppState, d: &ServerDialog, out: &mut Vec<Msg>) {
    let mut b = st.config.koboldcpp.clone();
    let fallback = increart_gui_core::kobold_binary(st);
    let left = ui.horizontal(|ui| {
        ui.add_sized([56.0, 18.0], egui::Label::new("Binary"));
        path_edit(ui, &mut b, &fallback, PickKind::KoboldBinary, out)
    });
    if b != st.config.koboldcpp {
        out.push(Msg::SetKoboldBinary(b));
    }
    if left.inner.lost_focus() {
        out.push(Msg::CheckKoboldVersion);
    }
    ui.horizontal(|ui| {
        ui.add_space(64.0);
        match &d.kobold_version {
            Some((binary, Ok(v))) if *binary == fallback => {
                ui.colored_label(GOOD, ic(ph::CHECK, &format!("KoboldCpp {v}")));
            }
            Some((binary, Err(e))) if *binary == fallback => {
                ui.colored_label(BAD, e);
            }
            _ if left.inner.has_focus() => {}
            _ => {
                ui.spinner();
                ui.weak("--version…");
            }
        }
    });
}

/// No image model is set here: each run asks for its model preset's files.
fn image_model_line(ui: &mut egui::Ui, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        ui.weak(ic(ph::CUBE, "Image model: each run's, from its model preset (files in the model directories)"))
            .on_hover_text(
                "The servers load the files of the model preset a run is made with (Generate's Model preset list); \
             KoboldCpp reloads with them, sd-server takes them per request.",
            );
        if ui.small_button(ph::GEAR).on_hover_text("Edit the model presets…").clicked() {
            out.push(Msg::OpenRecipes);
        }
    });
}

/// A started server's whole command line, environment first, as its log's
/// first line will show it (the admin password hidden).
fn command_line(ui: &mut egui::Ui, st: &AppState, i: usize, s: &ServerConfig) {
    let log_dir = increart_gui_core::runtime::log_dir();
    let state_dir = log_dir.parent().unwrap_or(&log_dir);
    let Some(line) = increart_core::launcher::command_line(s, &st.config.sdserver, state_dir) else { return };
    egui::CollapsingHeader::new(ic(ph::TERMINAL, "Command line")).id_salt(("command line", i)).show(
        ui,
        |ui| match line {
            Ok(line) => {
                ui.horizontal(|ui| {
                    if ui.small_button(ic(ph::COPY, "Copy")).clicked() {
                        ui.ctx().copy_text(line.clone());
                    }
                    ui.weak("as the server's log starts with it");
                });
                ui.add(egui::Label::new(RichText::new(line).monospace()).wrap().selectable(true));
            }
            Err(e) => {
                ui.colored_label(ui.visuals().warn_fg_color, e);
            }
        },
    );
}

fn sdserver_binary_field(ui: &mut egui::Ui, c: &mut increart_core::config::SdServerConfig, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        ui.add_sized([56.0, 18.0], egui::Label::new("Binary"));
        path_edit(ui, &mut c.binary, "sd-server", PickKind::SdServerBinary, out);
    });
}

fn leave_running_field(ui: &mut egui::Ui, c: &mut increart_core::config::SdServerConfig) {
    ui.checkbox(&mut c.leave_running, "Leave started servers running when IncreArt exits")
        .on_hover_text("The model stays loaded, so the next start is instant; the GPU memory stays in use meanwhile");
}

/// The sd-server binary, model directories and leave-running setting.
fn sdserver_fields(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let mut c = st.config.sdserver.clone();
    sdserver_binary_field(ui, &mut c, out);
    models_field(ui, &mut c, MODEL_DIRS_NOTE, out);
    leave_running_field(ui, &mut c);
    if c != st.config.sdserver {
        out.push(Msg::SetSdServer(c));
    }
}

/// The model directories' list editor on its own (the model presets editor
/// has it too, with `note` saying what they mean there): saved as it's
/// changed.
pub(crate) fn models_list(ui: &mut egui::Ui, st: &AppState, note: &str, out: &mut Vec<Msg>) {
    let mut c = st.config.sdserver.clone();
    models_field(ui, &mut c, note, out);
    if c != st.config.sdserver {
        out.push(Msg::SetSdServer(c));
    }
}

/// What the order of the model directories means, under their title.
const MODEL_DIRS_NOTE: &str = "Searched in this order for model files and LoRAs (in each one's loras/): \
     a path in more than one is the first one's. sd-server is started with each.";

/// The model directories, in order, in a frame of their own: sd-server's
/// `--model-dir`s, where KoboldCpp's files and both servers' LoRAs are
/// looked up too. Each can be typed, browsed to, moved or removed; Add
/// folder… browses to another.
fn models_field(ui: &mut egui::Ui, c: &mut increart_core::config::SdServerConfig, note: &str, out: &mut Vec<Msg>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(ic(ph::FOLDERS, "Model directories")).strong());
        ui.add(egui::Label::new(RichText::new(note).weak()).wrap());
        let n = c.model_dirs.len().max(1);
        let mut moved = None;
        // one width for every row (↑ ↓ × after each)
        let sp = ui.spacing().item_spacing.x;
        let w = ui.available_width() - 3.0 * (ui.spacing().interact_size.y + 8.0 + sp);
        for i in 0..n {
            ui.horizontal(|ui| {
                let mut dir = c.model_dirs.get(i).cloned().unwrap_or_default();
                ui.allocate_ui(egui::vec2(w, 18.0), |ui| {
                    path_edit(ui, &mut dir, models_hint(), PickKind::ModelDir(i), out)
                });
                if dir != c.model_dirs.get(i).cloned().unwrap_or_default() {
                    match c.model_dirs.get_mut(i) {
                        Some(d) => *d = dir,
                        None => c.model_dirs.push(dir),
                    }
                }
                let many = c.model_dirs.len() > 1;
                if ui
                    .add_enabled(many && i > 0, egui::Button::new(ph::ARROW_UP))
                    .on_hover_text("Search it earlier")
                    .clicked()
                {
                    moved = Some((i, i - 1));
                }
                if ui
                    .add_enabled(many && i + 1 < c.model_dirs.len(), egui::Button::new(ph::ARROW_DOWN))
                    .on_hover_text("Search it later")
                    .clicked()
                {
                    moved = Some((i, i + 1));
                }
                if ui
                    .add_enabled(i < c.model_dirs.len(), egui::Button::new(ph::X))
                    .on_hover_text("Remove it from the list (the folder stays on disk)")
                    .clicked()
                {
                    c.model_dirs.remove(i);
                }
            });
        }
        if let Some((from, to)) = moved {
            c.model_dirs.swap(from, to);
        }
        if ui
            .button(ic(ph::FOLDER_PLUS, "Add folder…"))
            .on_hover_text("Browse to another folder with model files")
            .clicked()
        {
            out.push(Msg::Pick(PickKind::ModelDir(c.model_dirs.len())));
        }
    });
}

/// The server setup wizard: KoboldCpp with a preset by each GPU's memory
/// ([`kobold_wizard`]), or sd-server, the GPUs to start it on (all GPUs
/// ticked), or a server already running; saved as a preset and opened.
fn server_wizard(ctx: &egui::Context, st: &AppState, d: &ServerDialog, out: &mut Vec<Msg>) {
    let m = egui::Modal::new(egui::Id::unique("server_wizard")).show(ctx, |ui| {
        // (KoboldCpp's presets table needs the room)
        ui.set_width(if d.kobold.is_some() { 700.0 } else { 560.0 });
        ui.heading(ic(ph::MAGIC_WAND, "Set up servers"));
        let busy = d.kobold.as_ref().is_some_and(|k| k.downloading.is_some());
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal(|ui| {
                if ui.radio(d.kobold.is_some(), "KoboldCpp, Qwen-Image set up for your GPUs (recommended)").clicked() {
                    out.push(Msg::WizardKobold(true));
                }
            });
            ui.horizontal(|ui| {
                if ui.radio(d.kobold.is_none(), "sd-server, with your own models").clicked() {
                    out.push(Msg::WizardKobold(false));
                }
            });
        });
        ui.add_space(8.0);
        if let Some(k) = &d.kobold {
            let binary = increart_gui_core::update::wizard_binary(k);
            let version = d.kobold_version.as_ref().filter(|(b, _)| *b == binary).map(|(_, r)| r);
            kobold_wizard(ui, k, version, &st.config.sdserver.model_dirs, out);
            return;
        }
        ui.label("IncreArt runs sd-server on your GPUs, one server per GPU.");
        ui.add_space(8.0);
        ui.label(RichText::new("1. sd-server").strong());
        sdserver_fields(ui, st, out);
        // its models are yours: the presets say which files each one is
        ui.horizontal(|ui| {
            let n = st.config.recipes.len();
            ui.weak(format!(
                "Image models: {n} model preset{}, each naming its files in Models",
                if n == 1 { "" } else { "s" }
            ));
            if ui.small_button(ic(ph::GEAR, "Edit model presets…")).clicked() {
                out.push(Msg::OpenRecipes);
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("2. GPUs to start it on").strong());
            if ui.small_button(ic(ph::ARROWS_CLOCKWISE, "Detect again")).clicked() {
                out.push(Msg::ListDevices);
            }
        });
        match &d.devices {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("sd-server --list-devices…");
                });
            }
            Some(Err(e)) => {
                ui.colored_label(BAD, e);
                ui.weak("Check the binary above, then Detect again.");
            }
            Some(Ok(list)) if list.is_empty() => {
                ui.weak("No devices listed.");
            }
            Some(Ok(list)) => {
                for line in list {
                    let device = increart_core::launcher::device_name(line);
                    let port = d.draft.servers.iter().find_map(|s| match s {
                        ServerConfig::Start { device: x, port, .. } if x == device => Some(*port),
                        _ => None,
                    });
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut port.is_some(), line.replace('\t', "  ")).changed() {
                            out.push(Msg::DraftDevice(device.into(), port.is_none()));
                        }
                        if let Some(p) = port {
                            ui.weak(format!("on :{p}"));
                        }
                    });
                }
            }
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Or connect to a server that's already running").strong());
        let connect = d.draft.servers.iter().position(|s| matches!(s, ServerConfig::Connect { .. }));
        ui.horizontal(|ui| {
            if ui.checkbox(&mut connect.is_some(), "Connect to").changed() {
                out.push(match connect {
                    Some(i) => Msg::RemoveServer(i),
                    None => Msg::AddConnectServer,
                });
            }
            if let Some(i) = connect {
                let mut s = d.draft.servers[i].clone();
                if let ServerConfig::Connect { url, .. } = &mut s {
                    ui.add(egui::TextEdit::singleline(url).desired_width(260.0));
                }
                if s != d.draft.servers[i] {
                    out.push(Msg::EditServer(i, s));
                }
            }
        });
        ui.add_space(10.0);
        ui.label(RichText::new("Prompt enhance (optional)").strong());
        ui.weak("A chat model that suggests and rewrites prompt text.");
        let chats: Vec<(usize, &increart_core::pe::PeServer)> = d
            .draft
            .servers
            .iter()
            .enumerate()
            .filter_map(|(i, s)| if let ServerConfig::Chat(c) = s { Some((i, c)) } else { None })
            .collect();
        for (i, c) in &chats {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&c.name).strong()).on_hover_text(crate::pe_settings::describe(c));
                if ui.small_button(ic(ph::GEAR, "Settings…")).clicked() {
                    out.push(Msg::PeEditorOpen(Some(*i)));
                }
                if ui.small_button(ph::X).on_hover_text("Remove").clicked() {
                    out.push(Msg::RemoveServer(*i));
                }
            });
        }
        ui.horizontal(|ui| {
            let add = if chats.is_empty() { "Add a chat server…" } else { "Add another (asks shared)…" };
            if ui.button(ic(ph::PLUS, add)).clicked() {
                out.push(Msg::PeEditorOpen(None));
            }
        });
        ui.add_space(10.0);
        if d.draft.servers.is_empty() {
            ui.weak("Tick a GPU, or a server to connect to.");
        } else {
            ui.weak(d.draft.servers.iter().map(describe).collect::<Vec<_>>().join(" · "));
        }
        ui.add_space(6.0);
        let (cancel, _) = dialog_footer(
            ui,
            |ui| ui.button("Cancel").clicked(),
            |ui| {
                let ok = !d.draft.servers.is_empty();
                let finish = egui::Button::new(RichText::new(ic(ph::PLAY, "Save as preset and open")).strong());
                if ui.add_enabled(ok, finish).clicked() {
                    out.push(Msg::FinishServerWizard);
                }
            },
        );
        if cancel {
            out.push(Msg::ServerWizard(false));
        }
    });
    if m.should_close() {
        out.push(Msg::ServerWizard(false));
    }
}

/// The wizard's KoboldCpp page: the binary, the model directory, the GPUs
/// (each one's memory picks its preset), how two or more share the work;
/// then the models downloaded and the servers opened.
/// `version`: the binary's `--version` (None while asking).
fn kobold_wizard(
    ui: &mut egui::Ui,
    k: &KoboldWizard,
    version: Option<&Result<String, String>>,
    model_dirs: &[String],
    out: &mut Vec<Msg>,
) {
    let mut new = k.clone();
    let busy = k.downloading.is_some();
    let plan = k.plan();
    // (scrolls when taller than the window; the progress and buttons stay)
    let room = ui.ctx().content_rect().height() - 260.0;
    egui::ScrollArea::vertical().max_height(room.max(200.0)).auto_shrink([false, true]).show(ui, |ui| {
        ui.label("IncreArt starts KoboldCpp with Qwen-Image 2.1 and prompt enhance, set up for each GPU's memory.");
        ui.add_space(8.0);
        ui.add_enabled_ui(!busy, |ui| {
            ui.label(RichText::new("1. KoboldCpp").strong());
            ui.horizontal(|ui| {
                ui.add_sized([LABEL_W, 18.0], egui::Label::new("Binary"));
                if path_edit(ui, &mut new.binary, "koboldcpp", PickKind::WizardBinary, out).lost_focus() {
                    out.push(Msg::CheckWizardBinary);
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(LABEL_W + 8.0);
                match version {
                    Some(Ok(v)) => {
                        ui.colored_label(GOOD, ic(ph::CHECK, &format!("KoboldCpp {v}")));
                    }
                    Some(Err(e)) => {
                        ui.add(egui::Label::new(RichText::new(e).color(BAD)).wrap());
                    }
                    None => {
                        ui.spinner();
                        ui.weak("--version…");
                    }
                }
            });
            if matches!(version, Some(Err(_))) {
                ui.horizontal(|ui| {
                    ui.add_space(LABEL_W + 8.0);
                    // (wrapped: a line this long would widen the dialog)
                    let text = "Download KoboldCpp from github.com/LostRuins/koboldcpp/releases and pick it with Browse…, \
                                or Save anyway and set it later in Server settings.";
                    ui.add(egui::Label::new(RichText::new(text).weak()).wrap());
                });
            }
            ui.horizontal(|ui| {
                ui.add_sized([LABEL_W, 18.0], egui::Label::new("Download to"));
                path_edit(ui, &mut new.models_dir, models_hint(), PickKind::WizardModels, out);
            });
            let dir = config::full_path(&new.models_dir);
            let to = std::path::Path::new(&dir).join(tiers::MODEL_SUBDIR);
            let mut found: Vec<&str> = model_dirs.iter().map(String::as_str).filter(|d| *d != dir).collect();
            found.push("Hugging Face's cache");
            ui.weak(format!(
                "{} (about 10 GB for 8–12 GB cards, 13 GB for bigger ones). \
                 Files already in {} are linked, not downloaded again.",
                if dir.is_empty() { "Choose where the models go".into() } else { format!("They go in {}", to.display()) },
                found.join(", ")
            ));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("2. GPUs").strong());
                if ui.small_button(ic(ph::ARROWS_CLOCKWISE, "Detect again")).clicked() {
                    out.push(Msg::WizardKobold(true));
                }
                ui.add_space(12.0);
                ui.label("through");
                let mut runtime = k.runtime;
                if crate::pe_settings::runtime_choice(ui, "wizard_runtime", &mut runtime) {
                    out.push(Msg::WizardRuntime(runtime));
                }
            });
            match &mut new.gpus {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak(format!("{}…", k.runtime.tool()));
                    });
                }
                Some(Err(e)) => {
                    ui.colored_label(BAD, e.as_str());
                    if ui.button(ic(ph::PLUS, "Add a GPU by hand")).clicked() {
                        let gpu = Gpu { number: 0, name: "GPU 0".into(), vram_gib: 8.0, integrated: false };
                        new.gpus = Some(Ok(vec![WizardGpu { gpu, on: true, preset: None, by_hand: true }]));
                    }
                }
                Some(Ok(list)) => {
                    let cards = plan.as_ref().map(|p| p.cards.clone()).unwrap_or_default();
                    egui::Grid::new("wizard_gpus").num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
                        for w in list.iter_mut() {
                            ui.checkbox(&mut w.on, format!("{}  {}", w.gpu.number, w.gpu.name));
                            if w.by_hand {
                                ui.add(
                                    egui::DragValue::new(&mut w.gpu.vram_gib)
                                        .range(0.0..=1024.0)
                                        .speed(0.5)
                                        .max_decimals(1)
                                        .suffix(" GB"),
                                )
                                .on_hover_text("Its memory");
                            } else {
                                ui.label(format!("{} GB", tiers::show_gb(w.gpu.vram_gib)));
                            }
                            let card = cards.iter().find(|c| c.gpu == w.gpu.number);
                            gpu_preset(ui, w, k.choose, card);
                            match card {
                                Some(c) if w.on => ui.label(c.role.describe()),
                                _ if w.on => ui.label(""),
                                _ => ui.weak("not used"),
                            };
                            ui.end_row();
                        }
                    });
                    if ui.small_button(ic(ph::PLUS, "Add a GPU by hand")).clicked() {
                        let number = list.iter().map(|w| w.gpu.number + 1).max().unwrap_or(0);
                        let gpu = Gpu { number, name: format!("GPU {number}"), vram_gib: 8.0, integrated: false };
                        list.push(WizardGpu { gpu, on: true, preset: None, by_hand: true });
                    }
                }
            }
            let ticked = k.ticked();
            if ticked.len() > 1 {
                ui.add_space(10.0);
                ui.label(RichText::new(format!("3. How to use {} GPUs", ticked.len())).strong());
                for layout in [Layout::ImagePerCard, Layout::PeOwnCard] {
                    let text = match k.plan_as(layout) {
                        Ok(p) => {
                            let n = p.images_at_once();
                            let roles: Vec<String> =
                                p.cards.iter().map(|c| format!("GPU {}: {}", c.gpu, c.role.short())).collect();
                            let at_once = if n == 1 { "1 image at a time".into() } else { format!("{n} images at a time") };
                            format!("{at_once} — {}", roles.join(", "))
                        }
                        Err(_) if layout == Layout::PeOwnCard => "Prompt enhance on a GPU of its own".into(),
                        Err(_) => "Images on every GPU".into(),
                    };
                    let hover = match layout {
                        Layout::ImagePerCard => "More images at once; prompt enhance shares a GPU with images",
                        Layout::PeOwnCard => "Prompt enhance answers at once and is never swapped; fewer images at once",
                    };
                    ui.radio_value(&mut new.layout, layout, text).on_hover_text(hover);
                }
                let current: Vec<u32> = plan
                    .as_ref()
                    .map(|p| p.pe_cards().iter().map(|c| c.gpu).collect())
                    .unwrap_or_else(|_| k.pe_gpus.clone());
                ui.horizontal(|ui| {
                    ui.label("Prompt enhance on");
                    for (g, _) in &ticked {
                        let mut on = current.contains(&g.number);
                        let only = on && current.len() == 1;
                        let hover = if only {
                            "Prompt enhance needs a GPU"
                        } else {
                            "Each GPU ticked runs a chat model; asks are shared among them"
                        };
                        let r = ui.add_enabled(!only, egui::Checkbox::new(&mut on, format!("GPU {}", g.number)));
                        if r.on_hover_text(hover).on_disabled_hover_text(hover).changed() {
                            new.pe_gpus = current.iter().copied().filter(|&n| n != g.number).collect();
                            if on {
                                new.pe_gpus.push(g.number);
                            }
                        }
                    }
                });
            }
            ui.add_space(10.0);
            presets_table(ui, &mut new, plan.as_ref().ok());
        });
        ui.add_space(10.0);
        match &plan {
            Ok(p) => {
                ui.label(RichText::new(&p.summary).strong());
                for n in &p.notes {
                    match n {
                        tiers::Note::Plan(s) => ui.label(s),
                        tiers::Note::Why(s) => ui.weak(s),
                        tiers::Note::Warn(s) => ui.colored_label(NEUTRAL, ic(ph::WARNING, s)),
                    };
                }
                wizard_files(ui, p, &config::full_path(&k.models_dir), out);
            }
            Err(e) if !k.ticked().is_empty() => {
                ui.colored_label(NEUTRAL, e);
            }
            Err(_) => {
                ui.weak("Tick a GPU.");
            }
        }
        if let Some(e) = &k.error {
            ui.colored_label(BAD, e);
        }
    });
    if let Some((p, at)) = &k.downloading {
        ui.add_space(6.0);
        download_progress(ui, p, at);
    }
    ui.add_space(6.0);
    // Cancel on its own at the left, away from the actions
    let (cancel, _) = dialog_footer(
        ui,
        |ui| !busy && ui.button("Cancel").clicked(),
        |ui| {
            if busy {
                if ui.button("Cancel download").clicked() {
                    out.push(Msg::CancelWizardDownload);
                }
                return;
            }
            let found = matches!(version, Some(Ok(_)));
            let finish =
                egui::Button::new(RichText::new(ic(ph::DOWNLOAD_SIMPLE, "Download, save as preset and open")).strong());
            let why = match version {
                None => "Checking KoboldCpp (--version)…",
                _ => "KoboldCpp isn't found: pick it under 1. KoboldCpp, or Save anyway",
            };
            if ui.add_enabled(plan.is_ok() && found, finish).on_disabled_hover_text(why).clicked() {
                out.push(Msg::FinishKoboldWizard);
            }
            if ui
                .add_enabled(plan.is_ok(), egui::Button::new("Save, download later"))
                .on_hover_text(
                    "Save the preset without downloading: get the files under Files yourself, or run the wizard again",
                )
                .clicked()
            {
                out.push(Msg::SaveKoboldWizardLater);
            }
            if matches!(version, Some(Err(_)))
            && ui
                .add_enabled(plan.is_ok(), egui::Button::new("Save anyway"))
                .on_hover_text("Download the models and save the preset without opening it; set KoboldCpp later in Server settings")
                .clicked()
        {
            out.push(Msg::SaveKoboldWizard);
        }
        },
    );
    if cancel {
        out.push(Msg::ServerWizard(false));
    }
    if new != *k && !busy {
        out.push(Msg::EditKoboldWizard(new));
    }
}

/// The plan's files: where each goes and comes from, and the commands
/// that fetch them (for getting them by hand).
fn wizard_files(ui: &mut egui::Ui, p: &tiers::Plan, dir: &str, out: &mut Vec<Msg>) {
    let dest = |file: &str| std::path::Path::new(dir).join(file).to_string_lossy().into_owned();
    egui::CollapsingHeader::new(format!("Files ({})", p.downloads.len())).id_salt("wizard_files").show(ui, |ui| {
        // (the dialog is tall already)
        egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
            for d in &p.downloads {
                ui.add(egui::Label::new(RichText::new(dest(&d.file)).monospace()).wrap().selectable(true));
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.add(egui::Label::new(RichText::new(&d.url).weak()).wrap().selectable(true));
                });
            }
        });
        let to = std::path::Path::new(dir).join(tiers::MODEL_SUBDIR);
        let there = to.is_dir();
        let tip = if there { "Open it in the file manager" } else { "Not made yet: it is when the models download" };
        let b = ui.add_enabled(there, egui::Button::new(ic(ph::ARROW_SQUARE_OUT, "Open the model directory")).small());
        if b.on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
            out.push(Msg::ShowInFileManager(to));
        }
        if ui
            .small_button(ic(ph::COPY, "Copy download commands"))
            .on_hover_text("curl, one line a file; a broken one carries on")
            .clicked()
        {
            let lines: Vec<String> = p
                .downloads
                .iter()
                .map(|d| format!("curl -L -C - --create-dirs -o \"{}\" \"{}\"", dest(&d.file), d.url))
                .collect();
            ui.ctx().copy_text(lines.join("\n"));
        }
    });
}

/// The models downloading: all of them (size, speed, time left), then the
/// file under way and the ones linked instead.
fn download_progress(ui: &mut egui::Ui, p: &tiers::Plan, at: &WizardDownload) {
    let gb = |b: u64| format!("{:.1}", b as f64 / 1e9);
    let name = |i: usize| {
        p.downloads.get(i).map(|d| d.file.trim_start_matches(&format!("{}/", tiers::MODEL_SUBDIR)).to_string())
    };
    if at.checking {
        ui.horizontal(|ui| {
            ui.spinner();
            let file = name(at.file).unwrap_or_default();
            ui.label(format!("Checking what's there already ({} of {}): {file}", at.file + 1, p.downloads.len()));
        });
    } else {
        let mut line = match at.all_total {
            Some(t) => format!("{} of {} GB", gb(at.all_done), gb(t)),
            None => format!("{} GB", gb(at.all_done)),
        };
        if let Some(r) = at.rate {
            line.push_str(&format!(" · {:.0} MB/s", r / 1e6));
        }
        if let Some(s) = at.eta_secs() {
            line.push_str(&format!(" · about {} left", show_secs(s)));
        }
        let fraction = at.all_total.filter(|t| *t > 0).map_or(0.0, |t| at.all_done as f32 / t as f32);
        ui.label("Downloading the models");
        ui.add(egui::ProgressBar::new(fraction).text(line));
        let file = name(at.file).unwrap_or_default();
        let size = match at.total {
            Some(t) => format!("{} of {} GB", gb(at.done), gb(t)),
            None => format!("{} GB", gb(at.done)),
        };
        ui.weak(format!("File {} of {}: {file} ({size})", at.file + 1, p.downloads.len()));
    }
    for (file, from) in &at.reused {
        let file = file.trim_start_matches(&format!("{}/", tiers::MODEL_SUBDIR));
        let text = if from.is_empty() { format!("{file}: there already") } else { format!("{file}: linked to {from}") };
        ui.weak(ic(ph::LINK, &text));
    }
}

/// `s` seconds as "40 s", "5 min" or "1 h 20 min".
fn show_secs(s: f64) -> String {
    let s = s.round() as u64;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s.div_ceil(60)),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// A ticked GPU's preset: by its memory, or (choosing) a menu of them;
/// prompt enhance's own card takes none.
fn gpu_preset(ui: &mut egui::Ui, w: &mut WizardGpu, choose: bool, card: Option<&tiers::Card>) {
    let auto = tiers::tier_for(w.gpu.vram_gib);
    if !w.on {
        ui.label("");
    } else if card.is_some_and(|c| c.role == tiers::Role::PeOnly) {
        ui.weak("—");
    } else if choose {
        let by_memory = match &auto {
            Some(t) => format!("Auto ({} GB)", t.gb),
            None => "Auto (too small)".into(),
        };
        let shown = w.preset.map(|gb| format!("{gb} GB")).unwrap_or(by_memory.clone());
        egui::ComboBox::from_id_salt(("wizard_preset", w.gpu.number)).selected_text(shown).show_ui(ui, |ui| {
            ui.selectable_value(&mut w.preset, None, by_memory);
            for t in tiers::tiers() {
                ui.selectable_value(&mut w.preset, Some(t.gb), format!("{} GB", t.gb));
            }
        });
    } else {
        match auto {
            Some(t) => ui.weak(format!("{} GB preset", t.gb)),
            None => ui.colored_label(NEUTRAL, "under 8 GB: too small"),
        };
    }
}

/// The presets (docs/vram-tiers.md), the plan's highlighted with its
/// GPUs; "Choose presets myself" unlocks them: a row's click sets every
/// image GPU's.
fn presets_table(ui: &mut egui::Ui, k: &mut KoboldWizard, plan: Option<&tiers::Plan>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Presets").strong());
        if ui.checkbox(&mut k.choose, "Choose presets myself").changed()
            && !k.choose
            && let Some(Ok(list)) = &mut k.gpus
        {
            list.iter_mut().for_each(|w| w.preset = None);
        }
    });
    let pe_only: Vec<u32> = plan
        .map(|p| p.cards.iter().filter(|c| c.role == tiers::Role::PeOnly).map(|c| c.gpu).collect())
        .unwrap_or_default();
    let mut clicked = None;
    egui::Grid::new("wizard_presets").num_columns(9).spacing([14.0, 3.0]).show(ui, |ui| {
        for h in ["", "Model", "Encoder", "VRAM cap", "PE", "Per image", "Peak VRAM", "Pinned RAM", ""] {
            ui.weak(h);
        }
        ui.end_row();
        for t in tiers::tiers() {
            let gpus: Vec<String> = plan
                .map(|p| p.cards.iter().filter(|c| c.preset == Some(t.gb)).map(|c| format!("GPU {}", c.gpu)).collect())
                .unwrap_or_default();
            let on = !gpus.is_empty();
            let row = ui.selectable_label(on, RichText::new(format!("{} GB", t.gb)).strong());
            if k.choose && row.clicked() {
                clicked = Some(t.gb);
            }
            if k.choose {
                row.on_hover_text("Use for every GPU making images");
            }
            let cell = |ui: &mut egui::Ui, s: String| {
                if on {
                    ui.label(RichText::new(s).color(ACCENT))
                } else {
                    ui.weak(s)
                };
            };
            cell(ui, t.quant().to_string());
            cell(ui, if t.clip_on_gpu { "GPU" } else { "CPU" }.into());
            cell(ui, if t.vram_limit_mb > 0 { format!("{} GiB", t.vram_limit_mb / 1024) } else { "none".into() });
            cell(ui, if t.swap { "swaps" } else { "stays loaded" }.into());
            cell(ui, format!("{} s", t.per_image_s));
            cell(ui, format!("{:.1} GiB", t.peak_vram_gib));
            cell(ui, format!("~{} GiB", t.pinned_ram_gib));
            if on {
                ui.label(RichText::new(format!("◀ {}", gpus.join(", "))).color(ACCENT).strong());
            } else {
                ui.label("");
            }
            ui.end_row();
        }
    });
    ui.weak("Timings and memory measured on a Radeon AI PRO R9700 with KoboldCpp 1.123 (docs/vram-tiers.md).");
    if let (Some(gb), Some(Ok(list))) = (clicked, &mut k.gpus) {
        for w in list.iter_mut().filter(|w| w.on && !pe_only.contains(&w.gpu.number)) {
            w.preset = Some(gb);
        }
    }
}

/// The editor (✎ / New preset): name, servers, Save / Save and open / Open
/// without saving / Cancel. Esc cancels; a click outside doesn't.
fn editor_window(ctx: &egui::Context, st: &AppState, d: &ServerDialog, editing: Editing, out: &mut Vec<Msg>) {
    egui::Modal::new(egui::Id::unique("preset_editor")).show(ctx, |ui| {
        ui.set_width(620.0);
        editor(ui, st, d, editing, out);
        if st.confirm.is_none() && st.pe.editor.is_none() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            out.push(Msg::CancelEdit);
        }
    });
}

fn editor(ui: &mut egui::Ui, st: &AppState, d: &ServerDialog, editing: Editing, out: &mut Vec<Msg>) {
    let title = if editing == Editing::New { "New preset" } else { "Edit preset" };
    ui.heading(ic(ph::PENCIL_SIMPLE, title));
    let mut name = d.draft.name.clone();
    ui.add(egui::TextEdit::singleline(&mut name).desired_width(240.0).hint_text("preset name"));
    if name != d.draft.name {
        out.push(Msg::SetPresetName(name));
    }
    ui.add_space(6.0);
    if d.draft.servers.is_empty() {
        ui.weak(
            "Add KoboldCpp (images, and prompt enhance too), or an sd-server to start or connect to and a chat \
             server for prompt enhance if you like.",
        );
    }
    let has_chat_entry = d.draft.chat().is_some();
    for (i, s) in d.draft.servers.iter().enumerate() {
        let mut edited = s.clone();
        // full width, × pinned to the right edge
        let remove = egui::Frame::group(ui.style())
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let (_, remove) = egui::Sides::new().show(
                    ui,
                    |ui| match &mut edited {
                        ServerConfig::Start { name, port, device, .. } => {
                            ui.label(ic(ph::PLAY_CIRCLE, "sd-server"));
                            ui.add(egui::TextEdit::singleline(name).desired_width(70.0).hint_text("name"));
                            ui.label("device");
                            ui.add(egui::TextEdit::singleline(device).desired_width(80.0));
                            ui.label("port");
                            ui.add(egui::DragValue::new(port).range(1024..=65535));
                        }
                        ServerConfig::Connect { name, url, device } => {
                            ui.label(ic(ph::PLUGS_CONNECTED, "sd-server at"));
                            ui.add(egui::TextEdit::singleline(name).desired_width(70.0).hint_text("name"));
                            ui.add(egui::TextEdit::singleline(url).desired_width(200.0));
                            ui.add(egui::TextEdit::singleline(device).desired_width(70.0).hint_text("device"));
                        }
                        ServerConfig::Chat(c) => {
                            ui.label(ic(ph::MAGIC_WAND, "Chat"));
                            ui.label(RichText::new(&c.name).strong()).on_hover_text(crate::pe_settings::describe(c));
                            if ui.small_button(ic(ph::GEAR, "Settings…")).clicked() {
                                out.push(Msg::PeEditorOpen(Some(i)));
                            }
                        }
                        ServerConfig::Kobold(k) => {
                            ui.label(ic(ph::IMAGE, "KoboldCpp"));
                            ui.add(egui::TextEdit::singleline(&mut k.name).desired_width(90.0).hint_text("name"));
                        }
                    },
                    |ui| ui.small_button(ph::X).on_hover_text("Remove").clicked(),
                );
                if let ServerConfig::Kobold(k) = &mut edited {
                    kobold_fields(ui, st, i, k, has_chat_entry, out);
                }
                command_line(ui, st, i, &edited);
                remove
            })
            .inner;
        if remove {
            out.push(Msg::RemoveServer(i));
        }
        if &edited != s {
            out.push(Msg::EditServer(i, edited));
        }
    }
    if d.draft.servers.iter().any(|s| !matches!(s, ServerConfig::Chat(_))) {
        image_model_line(ui, out);
    }
    if !d.draft.servers.is_empty() {
        ui.add_space(4.0);
        roles_grid(ui, &d.draft);
        ui.add_space(4.0);
    }
    ui.horizontal(|ui| {
        if ui
            .button(RichText::new(ic(ph::PLUS, "KoboldCpp")).strong().color(ACCENT))
            .on_hover_text("KoboldCpp generating images (and the chat server too, if you like)")
            .clicked()
        {
            out.push(Msg::AddKoboldServer);
        }
        if ui.button(ic(ph::PLUS, "sd-server")).on_hover_text("an sd-server IncreArt starts").clicked() {
            out.push(Msg::AddStartServer);
        }
        if ui.button(ic(ph::PLUS, "Connect to sd-server")).on_hover_text("an sd-server already running").clicked() {
            out.push(Msg::AddConnectServer);
        }
        if ui
            .button(ic(ph::MAGIC_WAND, "Chat server"))
            .on_hover_text("a chat model for prompt enhance; with several, asks are shared among them")
            .clicked()
        {
            out.push(Msg::PeEditorOpen(None));
        }
    });
    // the listed devices: a click adds a Start server on one (with sd-servers)
    let sd = d.draft.servers.iter().any(|s| matches!(s, ServerConfig::Start { .. } | ServerConfig::Connect { .. }));
    if sd {
        ui.horizontal_wrapped(|ui| {
            ui.weak("Devices:");
            match &d.devices {
                None => {
                    ui.spinner();
                }
                Some(Err(_)) => {
                    ui.weak("couldn't list them (Server settings…)");
                }
                Some(Ok(list)) => {
                    for device in list.iter().map(|l| increart_core::launcher::device_name(l)) {
                        let used = d
                            .draft
                            .servers
                            .iter()
                            .any(|s| matches!(s, ServerConfig::Start { device: x, .. } if x == device));
                        if ui
                            .add_enabled(!used, egui::Button::new(ic(ph::PLUS, device)).small())
                            .on_hover_text("Start a server on this device")
                            .clicked()
                        {
                            out.push(Msg::DraftDevice(device.into(), true));
                        }
                    }
                }
            }
        });
        ui.colored_label(
            Color32::GRAY,
            "sd-server ports default to 1234, 1235, …; device names as `--list-devices` prints them.",
        );
    }
    ui.add_space(8.0);
    let has_servers = !d.draft.servers.is_empty();
    dialog_buttons(ui, |ui| {
        let open = egui::Button::new(RichText::new(ic(ph::PLAY, "Save and open")).strong());
        if ui.add_enabled(has_servers, open).clicked() {
            out.push(Msg::SaveAndOpen);
        }
        if ui.button(ic(ph::FLOPPY_DISK, "Save")).clicked() {
            out.push(Msg::SavePreset);
        }
        if ui.add_enabled(has_servers, egui::Button::new("Open without saving")).clicked() {
            out.push(Msg::OpenDraft);
        }
        if ui.button("Cancel   Esc").clicked() {
            out.push(Msg::CancelEdit);
        }
    });
}

/// What each server of a preset is used for: images, prompt enhance's
/// text, and whether those asks carry the reference images.
fn roles_grid(ui: &mut egui::Ui, preset: &increart_core::config::ServerPreset) {
    ui.label(RichText::new(ic(ph::LIST, "What each server does")).strong());
    let mark = |ui: &mut egui::Ui, on: bool, tip: &str| {
        let text = if on { RichText::new(ph::CHECK).color(GOOD) } else { RichText::new("—").weak() };
        ui.label(text).on_hover_text(tip);
    };
    egui::Grid::new("server_roles").num_columns(4).spacing([18.0, 2.0]).striped(true).show(ui, |ui| {
        for h in ["", "Images", "Text (prompt enhance)", "Vision"] {
            ui.weak(h);
        }
        ui.end_row();
        for r in preset.roles() {
            ui.label(&r.name);
            mark(ui, r.images, "generates images: a run goes to an image server whose model list has its preset's files");
            match &r.note {
                Some(note) => {
                    ui.label(RichText::new("not asked").weak()).on_hover_text(note);
                }
                None => mark(ui, r.text, "prompt enhance's asks go to it, shared among the servers ticked here"),
            }
            let (text, tip) = match (r.text, r.vision) {
                (false, _) => (RichText::new("—").weak(), "not asked"),
                (true, Some(true)) => (RichText::new(ph::CHECK).color(GOOD), "its vision projector sees the reference images sent with the asks"),
                (true, Some(false)) => (
                    RichText::new("text only").color(NEUTRAL),
                    "no vision projector (mmproj): the asks go without the reference images, and Describe can't be used",
                ),
                (true, None) => (
                    RichText::new("assumed").weak(),
                    "can't tell from here (connected, or set in a .kcpps): the reference images are sent with the asks",
                ),
            };
            ui.label(text).on_hover_text(tip);
            ui.end_row();
        }
    });
    ui.weak(
        "Images: each run goes to an image server whose model list has all of its model preset's files. \
         Text: prompt enhance shares its asks among the ticked servers. Vision: those see the reference images.",
    );
}

/// An opening library has no count to show, so the bar fills over time,
/// slowing as it goes (half in a second, never quite full).
fn loading_fraction(ui: &egui::Ui, id: egui::Id) -> f32 {
    let now = ui.input(|i| i.time);
    let since = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(id, || now));
    (1.0 - 0.5f64.powf(now - since)) as f32
}
