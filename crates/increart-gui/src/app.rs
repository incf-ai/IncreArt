//! The eframe app: feeds runtime messages and UI messages through the
//! reducer, carries out effects, and draws the current state.

use std::path::PathBuf;

use eframe::egui::{self, Color32, Panel, RichText, Sense};
use increart_core::config::Config;
use increart_core::prompts::PromptPresets;
use increart_gui_core::{AppState, Msg, Phase, ProjectsTab, Runtime, Tab, update};

use crate::textures::Textures;
use crate::tour::Tour;
use crate::widgets::{BAD, GOOD, NEUTRAL, dialog_buttons, dialog_footer, ic, ph};
use crate::{home, ideas, projects, review};

pub struct App {
    st: AppState,
    rt: Runtime,
    tex: Textures,
    logs: crate::logs::Cache,
    tour: Option<Tour>,
    shown_library: Option<PathBuf>,
    /// The app's clock (`Msg::Tick`).
    started: std::time::Instant,
    /// The window manager was asked for attention (taskbar flash; on X11 the
    /// urgency hint, which stays until it is reset).
    attention: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        config_path: Option<PathBuf>,
        warnings: Vec<String>,
        startup: crate::Startup,
    ) -> App {
        let ctx = cc.egui_ctx.clone();
        let rt = Runtime::new(move || ctx.request_repaint());
        let mut st = AppState::new(config, config_path);
        st.gpu_runtime = increart_core::pe::GpuRuntime::likely();
        for w in warnings {
            st.messages_info(w);
        }
        if let Some(path) = st.config_path.as_deref().map(PromptPresets::dir_beside) {
            match PromptPresets::load_dir(&path) {
                Ok((p, bad)) => {
                    st.prompt_presets = p;
                    st.presets_path = Some(path);
                    for e in bad {
                        st.messages_error(e);
                    }
                }
                // not saved over: the file may be fixable by hand
                Err(e) => st.messages_error(format!("prompt presets not loaded ({e}); using the built-in ones")),
            }
        }
        st.startup_servers = startup.servers;
        let tour = Tour::from_env();
        let mut app = App {
            st,
            rt,
            tex: Textures::new(&cc.egui_ctx),
            logs: Default::default(),
            tour,
            shown_library: None,
            started: std::time::Instant::now(),
            attention: false,
        };
        if let Some(asked) = startup.library {
            app.skip_library(&asked);
        }
        app
    }

    /// The command line's `--library`: opens it if it's a library, else
    /// leaves the library screen up, saying why.
    fn skip_library(&mut self, asked: &str) {
        let path = if asked == "last" { self.st.config.recent_libraries.first().cloned() } else { Some(asked.into()) };
        let problem = match &path {
            None => Some("no library opened yet".to_string()),
            Some(p) if crate::startup::folder_state(p) != crate::startup::FolderState::Library => {
                Some("not an IncreArt library (no library.rsn)".into())
            }
            Some(_) => None,
        };
        match (problem, path) {
            (None, Some(p)) => {
                self.dispatch(Msg::SetLibraryPath(p));
                self.dispatch(Msg::OpenLibrary);
            }
            (Some(e), _) => {
                if let Phase::Library(d) = &mut self.st.phase {
                    d.error = Some(format!("--library {asked}: {e}"));
                }
            }
            _ => {}
        }
    }

    /// The interface zoom: Ctrl + / − / 0 here instead of egui's own, so the
    /// config keeps it, then the zoom applied (egui does at the next pass).
    fn zoom(&mut self, ctx: &egui::Context) {
        use egui::gui_zoom::kb_shortcuts as kb;
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let (reset, zoom_in, zoom_out) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&kb::ZOOM_RESET),
                i.consume_shortcut(&kb::ZOOM_IN) | i.consume_shortcut(&kb::ZOOM_IN_SECONDARY),
                i.consume_shortcut(&kb::ZOOM_OUT),
            )
        });
        if reset {
            self.dispatch(Msg::UiZoom { percent: 100, save: true });
        }
        if zoom_in {
            self.dispatch(Msg::UiZoomStep(true));
        }
        if zoom_out {
            self.dispatch(Msg::UiZoomStep(false));
        }
        let want = self.st.ui_zoom() as f32 / 100.0;
        if ctx.zoom_factor() != want {
            ctx.set_zoom_factor(want);
        }
    }

    fn dispatch(&mut self, msg: Msg) {
        for fx in update(&mut self.st, msg) {
            // the screenshot tour is silent: its Xvfb hides the windows, not
            // the speakers (its alert scenes would sound for real)
            if self.tour.is_some() && matches!(fx, increart_gui_core::Effect::PlaySound { .. }) {
                continue;
            }
            self.rt.run(fx);
        }
    }

    fn main_window(&mut self, ui: &mut egui::Ui, out: &mut Vec<Msg>) {
        let st = &self.st;
        ideas::global_keys(ui.ctx(), out);
        nav_keys(ui.ctx(), st, out);
        crate::widgets::drag_overlay(ui.ctx());
        Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                let back = ui
                    .add_enabled(!st.nav.back.is_empty(), egui::Button::new(ph::ARROW_LEFT))
                    .on_hover_text("Back (Alt+←, mouse back)");
                if back.clicked() {
                    out.push(Msg::NavBack);
                }
                let forward = ui
                    .add_enabled(!st.nav.forward.is_empty(), egui::Button::new(ph::ARROW_RIGHT))
                    .on_hover_text("Forward (Alt+→, mouse forward)");
                if forward.clicked() {
                    out.push(Msg::NavForward);
                }
                ui.add_space(6.0);
                ui.label(RichText::new("IncreArt").strong().size(18.0));
                ui.add_space(16.0);
                // Each tab is an icon, its name and a count; short of room
                // (a narrow window, a large zoom) the names go, leaving
                // "📥 (7)". The right side gets what it needs at its compact
                // size first.
                let projects_icon = if st.active_runs.is_empty() { ph::ROCKET_LAUNCH } else { ph::SPARKLE };
                // Home has no count
                let tabs = [
                    (Tab::Home, ph::HOUSE, "Home", None),
                    (Tab::IdeaInbox, ph::TRAY, "Idea Inbox", Some(st.unreviewed_ideas().len())),
                    (Tab::InputLibrary, ph::BOOKS, "Input Library", Some(st.library_ideas_all())),
                    (Tab::Projects, projects_icon, "Projects", Some(st.index.projects.len())),
                    (Tab::Review, ph::THUMBS_UP, "Review", Some(st.unreviewed_outputs())),
                    (Tab::OutputLibrary, ph::IMAGES, "Output Library", Some(st.kept_outputs().len())),
                ];
                let text = |icon: &str, name: Option<&str>, n: Option<usize>| {
                    let t = match (name, n) {
                        (Some(name), Some(n)) => format!("{icon} {name}  ({n})"),
                        (Some(name), None) => format!("{icon} {name}"),
                        (None, Some(n)) => format!("{icon} ({n})"),
                        (None, None) => icon.to_string(),
                    };
                    RichText::new(t).size(15.0)
                };
                let width = |ui: &egui::Ui, t: RichText| {
                    egui::WidgetText::from(t)
                        .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button)
                        .size()
                        .x
                        + 2.0 * ui.spacing().button_padding.x
                        + ui.spacing().item_spacing.x
                };
                let full: f32 = tabs.iter().map(|(_, i, name, n)| width(ui, text(i, Some(name), *n))).sum();
                let short = full > ui.available_width() - TOP_RIGHT_MIN;
                for (tab, icon, name, n) in tabs {
                    let r = ui.selectable_label(st.tab == tab, text(icon, (!short).then_some(name), n));
                    let tip = || n.map(|n| format!("{name} ({n})")).unwrap_or_else(|| name.to_string());
                    let r = if short { r.on_hover_text(tip()) } else { r };
                    if r.clicked() {
                        out.push(Msg::Tab(tab));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // short of room (a narrow window): shorter buttons
                    let compact = ui.available_width() < 320.0;
                    if ui.button(ph::GEAR).on_hover_text("Settings (notifications)").clicked() {
                        out.push(Msg::Settings(true));
                    }
                    library_menu(ui, st, out, compact);
                    server_statuses(ui, st, out, compact);
                });
            });
        });
        Panel::bottom("status").show(ui, |ui| {
            // the right side first, so the path and message shorten instead
            // of running into it
            let force_open = self.tour.as_ref().is_some_and(|t| t.jobs_popover());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                jobs_indicator(ui, st, out, force_open);
                let bell_open = self.tour.as_ref().is_some_and(|t| t.bell_popover());
                if self.tour.as_ref().is_some_and(|t| t.snooze_dialog()) {
                    crate::settings::open_snooze_dialog(ui.ctx(), "1h 3x");
                }
                crate::settings::bell(ui, st, out, bell_open);
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(2.0);
                zoom_control(ui, st, out);
                if let Some((done, total)) = st.usage_check {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                            .desired_width(120.0)
                            .desired_height(12.0)
                            .text(RichText::new(format!("{done} / {total}")).small()),
                    )
                    .on_hover_text("Checking each image's usage counts against the runs (after opening the library)");
                    ui.weak(RichText::new("checking images").small());
                    ui.separator();
                }
                // done: shown full a few seconds more
                let shown = increart_gui_core::USAGE_CHECKED_SHOWN;
                if let Some((total, at)) = st.usage_checked.filter(|(_, at)| st.now - at < shown) {
                    ui.add(
                        egui::ProgressBar::new(1.0)
                            .desired_width(120.0)
                            .desired_height(12.0)
                            .text(RichText::new(format!("{total} / {total}")).small()),
                    )
                    .on_hover_text(
                        "Each image's usage counts were checked against the runs (after opening the library)",
                    );
                    ui.weak(RichText::new(format!("checked {total} images")).small());
                    ui.separator();
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(shown - (st.now - at)));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if st.busy > 0 {
                        ui.spinner();
                    }
                    let latest = st.messages.iter().enumerate().next_back();
                    if let Some(p) = &st.library {
                        // the message, if any, gets the room first
                        let room = if latest.is_some() { ui.available_width() * 0.35 } else { ui.available_width() };
                        ui.scope(|ui| {
                            ui.set_max_width(room);
                            let path = RichText::new(p.display().to_string()).weak();
                            let path = egui::Label::new(path).truncate().sense(Sense::click());
                            let r = ui
                                .add(path)
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(format!("{}\nopen the library in the file manager", p.display()));
                            if r.clicked() {
                                out.push(Msg::ShowInFileManager(p.clone()));
                            }
                        });
                    }
                    if let Some((i, m)) = latest {
                        ui.separator();
                        if ui.small_button(ph::X).clicked() {
                            out.push(Msg::DismissMessage(i));
                        }
                        // before the text, which is cut to what's left
                        if m.alert {
                            crate::settings::alert_links(ui, out);
                        }
                        message_log_button(ui, m, out);
                        let force_open = self.tour.as_ref().is_some_and(|t| t.messages_popover());
                        messages_line(ui, st, m, out, force_open);
                    }
                });
            });
        });
        let tex = &mut self.tex;
        match st.tab {
            Tab::Home => home::view(ui, st, out),
            Tab::IdeaInbox => ideas::inbox(ui, st, tex, out),
            Tab::InputLibrary => ideas::input_library(ui, st, tex, out),
            Tab::Projects => projects::view(ui, st, tex, out),
            Tab::Review => review::view(ui, st, tex, out),
            Tab::OutputLibrary => review::output_library(ui, st, tex, out),
        }
    }
}

/// What the top bar's right side (statuses, Servers…, library, ⚙) needs
/// at its compact size; the tabs drop their names to leave it.
const TOP_RIGHT_MIN: f32 = 340.0;

/// Top right, left of the Servers… button: each server's and the PE
/// server's status, as long as there is room: in full, then the name and
/// dot, then the dots; with no room for those, the Servers… button shows
/// the worst dot. The full status is always in the tooltip.
fn server_statuses(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>, compact: bool) {
    struct Item {
        name: String,
        dot: &'static str,
        state: String,
        color: Color32,
        tip: String,
        jobs: bool,
    }
    let busy = st.progress.keys().count();
    let mut items: Vec<Item> = st
        .servers
        .iter()
        .map(|s| {
            let (dot, color) = match s.state.as_str() {
                "ready" => ("●", GOOD),
                st if st.contains("retrying") => ("●", BAD),
                _ => ("◐", NEUTRAL),
            };
            let name = if s.device.is_empty() { s.name.clone() } else { s.device.clone() };
            let model = s.model.as_ref().map(|m| format!(", {m}")).unwrap_or_default();
            // what it's used for: the model presets it can run, once it has said
            let role = match st.server_models.get(&s.name) {
                Some(l) => {
                    let runs: Vec<&str> =
                        st.config.recipes.iter().filter(|r| l.runs(&r.files)).map(|r| r.name.as_str()).collect();
                    if runs.is_empty() {
                        "generates images: none of the model presets (it lacks their files)".to_string()
                    } else {
                        format!("generates images of: {}", runs.join(", "))
                    }
                }
                None => "generates images".to_string(),
            };
            let tip = format!(
                "{}: {}{model}\n{role}\n{busy} seeds generating · click for the server status",
                s.name, s.state
            );
            // the sd-server's latest log line only in the tooltip
            let state = if s.state.starts_with("starting…") { "starting…".into() } else { s.state.clone() };
            Item { name, dot, state, color, tip, jobs: true }
        })
        .collect();
    // stopped ones, until their processes are gone and the GPU memory free
    let stopping: Vec<&String> = st.stopping.iter().filter(|n| !items.iter().any(|i| &i.name == *n)).collect();
    items.extend(stopping.into_iter().map(|n| Item {
        name: n.clone(),
        dot: "◌",
        state: "stopping…".into(),
        color: NEUTRAL,
        tip: format!("{n}: stopping: waiting for sd-server to exit and free its GPU memory"),
        jobs: false,
    }));
    if items.is_empty() {
        let weak = ui.visuals().weak_text_color();
        let tip = "No sd-server in use: Servers… to start or connect one".to_string();
        items.push(Item { name: "no servers".into(), dot: "", state: String::new(), color: weak, tip, jobs: false });
    }
    let (dot, full, color) = pe_state(ui, st);
    let mut tip = format!("Prompt enhance (PE): {full}");
    if let Some(s) = &st.pe.server {
        tip.push_str(if s.vision() {
            "\ntext, with the reference images (vision)"
        } else {
            "\ntext only: the asks go without the reference images"
        });
    }
    // the server's name only in the tooltip: "PE ● ready"; starting or
    // working, a few words ("PE ● writing 143 tok"), the rest in it too
    let state = crate::rewrite::brief(st)
        .unwrap_or_else(|| full.split_once(": ").map_or(full.as_str(), |(_, s)| s).to_string());
    for (i, j) in st.pe.jobs.iter().enumerate() {
        let what = crate::rewrite::ask_name(st, j);
        tip.push_str(&format!("\n{}. {what}: {}", i + 1, crate::rewrite::progress(st, j)));
    }
    tip.push_str("\nclick for the server status");
    items.push(Item { name: "PE".into(), dot, state, color, tip, jobs: false });
    let texts = |level: usize| -> Vec<String> {
        items
            .iter()
            .filter(|i| level < 2 || !i.dot.is_empty())
            .map(|i| match level {
                0 => [i.name.as_str(), i.dot, i.state.as_str()]
                    .iter()
                    .filter(|t| !t.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" "),
                1 => {
                    if i.dot.is_empty() {
                        i.name.clone()
                    } else {
                        format!("{} {}", i.name, i.dot)
                    }
                }
                _ => i.dot.to_string(),
            })
            .collect()
    };
    let width = |t: &str| {
        let text: egui::WidgetText = t.into();
        text.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body).size().x
    };
    let gap = ui.spacing().item_spacing.x;
    // in full, a status starting or working changes many times a second,
    // and the bar mustn't move with it: its slot only grows until it
    // settles, then is its own width again (cut at MAX, whole in its tooltip)
    const MAX: f32 = 160.0;
    let slots: Vec<f32> = items
        .iter()
        .zip(texts(0))
        .map(|(i, t)| {
            let id = egui::Id::unique(("status slot", &i.name));
            let own = width(&t).ceil();
            let w = if i.color == NEUTRAL {
                let w = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0).max(own);
                ui.data_mut(|d| d.insert_temp(id, w));
                w
            } else {
                ui.data_mut(|d| d.remove::<f32>(id));
                own
            };
            w.min(MAX)
        })
        .collect();
    let shown =
        |level: usize| -> Vec<usize> { (0..items.len()).filter(|&i| level < 2 || !items[i].dot.is_empty()).collect() };
    let fits = |level: usize| -> f32 {
        shown(level)
            .into_iter()
            .zip(texts(level))
            .map(|(i, t)| if level == 0 { slots[i] } else { width(&t) } + gap)
            .sum()
    };
    let title = if compact { String::new() } else { "  Servers…".to_string() };
    let button = width(&format!("{}{title} ●", ph::HARD_DRIVES)) + 2.0 * ui.spacing().button_padding.x + gap;
    let room = ui.available_width() - button - 16.0;
    let level = (0..3).find(|&l| fits(l) <= room);

    // the worst state on the button when the statuses don't fit
    let worst = items.iter().filter(|i| i.jobs).max_by_key(|i| match i.color {
        c if c == BAD => 2,
        c if c == NEUTRAL => 1,
        _ => 0,
    });
    let mut label = RichText::new(format!("{}{title}", ph::HARD_DRIVES));
    if level.is_none()
        && let Some(w) = worst
    {
        label = RichText::new(format!("{}{title} {}", ph::HARD_DRIVES, w.dot)).color(w.color);
    }
    let tips: Vec<&str> = items.iter().map(|i| i.tip.lines().next().unwrap_or("")).collect();
    let hover = if level.is_none() { format!("Server status\n{}", tips.join("\n")) } else { "Server status".into() };
    if ui.button(label).on_hover_text(hover).clicked() {
        out.push(Msg::ServerStatus(true));
    }
    let Some(level) = level else { return };
    for (n, text) in shown(level).into_iter().zip(texts(level)).rev() {
        let i = &items[n];
        let label = egui::Label::new(RichText::new(text).color(i.color)).sense(Sense::click());
        let r = if level == 0 {
            let size = egui::vec2(slots[n], ui.spacing().interact_size.y);
            let layout = egui::Layout::left_to_right(egui::Align::Center);
            ui.allocate_ui_with_layout(size, layout, |ui| {
                ui.set_width(slots[n]);
                ui.add(label.truncate())
            })
            .inner
        } else {
            ui.add(label)
        };
        let r = r.on_hover_text(&i.tip);
        if r.clicked() {
            out.push(Msg::ServerStatus(true));
        }
    }
}

/// Top right: the open library, and a menu to switch to another (the
/// servers stay connected).
fn library_menu(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>, compact: bool) {
    let Some(root) = &st.library else { return };
    let mut name = root.file_name().map_or(root.display().to_string(), |n| n.to_string_lossy().into_owned());
    let max = if compact { 10 } else { 24 };
    if name.chars().count() > max {
        name = format!("{}…", name.chars().take(max - 1).collect::<String>());
    }
    let r = ui.menu_button(ic(ph::BOOKS, &format!("{name} {}", ph::CARET_DOWN)), |ui| {
        ui.set_min_width(320.0);
        ui.label(RichText::new("Libraries").strong());
        let recent = &st.config.recent_libraries;
        for path in recent {
            let current = std::path::Path::new(path) == root.as_path();
            let text = if current { ic(ph::CHECK, path) } else { ic(ph::BOOKS, path) };
            let r = ui.add_enabled(!current, egui::Button::new(text).frame(false));
            let r = r.on_disabled_hover_text("open now");
            if r.on_hover_text("switch to this library").clicked() {
                out.push(Msg::SwitchLibrary(path.clone()));
                ui.close();
            }
        }
        ui.separator();
        if ui.button(ic(ph::FOLDER_OPEN, "Open an existing library…")).clicked() {
            let first = crate::startup::found_libraries(recent).into_iter().next().map(|(_, p)| p).unwrap_or_default();
            out.push(Msg::OtherLibrary(Some((increart_gui_core::LibraryWizard::Open, first))));
            ui.close();
        }
        if ui.button(ic(ph::FOLDER_PLUS, "Create a new library…")).clicked() {
            let first = crate::startup::suggested_libraries().into_iter().next().map(|(_, p)| p).unwrap_or_default();
            out.push(Msg::OtherLibrary(Some((increart_gui_core::LibraryWizard::Create, first))));
            ui.close();
        }
        if ui.button(ic(ph::FOLDER_OPEN, "Show in the file manager")).clicked() {
            out.push(Msg::ShowInFileManager(root.clone()));
            ui.close();
        }
        ui.separator();
        archive_rows(ui, st, out);
        ui.separator();
        ui.weak("Switching keeps the servers connected.");
    });
    r.response.on_hover_text(format!("Library: {}\nclick to switch libraries", root.display()));
}

/// Export library (zip, tar.gz) and Unpack a library archive, or the one
/// being written or unpacked, with Stop.
fn archive_rows(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    use increart_core::archive::ArchiveFormat;
    if let Some(a) = &st.archiving {
        archive_progress(ui, a, out);
        return;
    }
    let zip = ui.button(ic(ph::FILE_ZIP, "Export the library to a zip…")).on_hover_text(
        "One file holding the whole library, each image once. Any unzip tool on any OS keeps its links; \
         opened in IncreArt, they become links again.",
    );
    if zip.clicked() {
        out.push(Msg::ExportLibrary(ArchiveFormat::Zip));
        ui.close();
    }
    let tar = ui.button(ic(ph::FILE_ARCHIVE, "Export the library to a tar.gz…")).on_hover_text(
        "One file holding the whole library, its links as symlinks: unpack it with tar on Linux or macOS, \
         or open it in IncreArt on any OS (tools on Windows may not make the symlinks).",
    );
    if tar.clicked() {
        out.push(Msg::ExportLibrary(ArchiveFormat::TarGz));
        ui.close();
    }
    let open = ui.button(ic(ph::PACKAGE, UNPACK_ARCHIVE)).on_hover_text(UNPACK_ARCHIVE_HOVER);
    if open.clicked() {
        out.push(Msg::UnpackLibraryArchive);
        ui.close();
    }
}

pub const UNPACK_ARCHIVE: &str = "Unpack a library archive…";
pub const UNPACK_ARCHIVE_HOVER: &str = "A .zip or .tar.gz of a library: unpacked into a folder you pick (a library folder of its own \
     there), then opened. The archive stays as it is.";

/// An archive being written or unpacked: how far, and Stop.
pub fn archive_progress(ui: &mut egui::Ui, a: &increart_gui_core::Archiving, out: &mut Vec<Msg>) {
    let name = a.archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let what = if a.export { "exporting to" } else { "unpacking" };
    let mb = |n: u64| n as f64 / 1e6;
    let text = if a.total == 0 {
        format!("{what} {name}…")
    } else {
        format!("{what} {name}: {:.0} of {:.0} MB", mb(a.done), mb(a.total))
    };
    ui.horizontal(|ui| {
        let progress = if a.total == 0 { 0.0 } else { a.done as f32 / a.total as f32 };
        ui.add(egui::ProgressBar::new(progress).desired_width(260.0).text(text));
        if ui.button("Stop").clicked() {
            out.push(Msg::CancelArchive);
        }
    });
}

/// The interface zoom, "− 100% +" in the status bar: the buttons step by
/// 5%, the value can be typed (click) or dragged.
fn zoom_control(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    use increart_gui_core::{UI_ZOOM_MAX, UI_ZOOM_MIN};
    let z = st.ui_zoom();
    ui.scope(|ui| {
        // right to left: the plus first
        ui.spacing_mut().item_spacing.x = 2.0;
        let small = |s: &str| egui::Button::new(s).small().min_size(egui::vec2(18.0, 0.0));
        let r = ui.add_enabled(z < UI_ZOOM_MAX, small(ph::PLUS));
        if r.on_hover_text("zoom in   Ctrl +").clicked() {
            out.push(Msg::UiZoomStep(true));
        }
        let mut v = z;
        let r = ui
            .add(
                egui::DragValue::new(&mut v)
                    .range(UI_ZOOM_MIN..=UI_ZOOM_MAX)
                    .suffix("%")
                    .speed(0.5)
                    // typing "150" isn't 1%, then 15%
                    .update_while_editing(false),
            )
            .on_hover_text("Interface zoom: click to type, drag to adjust\nCtrl + / Ctrl − / Ctrl 0 (100%)");
        if r.changed() || r.drag_stopped() {
            out.push(Msg::UiZoom { percent: v, save: !r.dragged() });
        }
        let r = ui.add_enabled(z > UI_ZOOM_MIN, small(ph::MINUS));
        if r.on_hover_text("zoom out   Ctrl −").clicked() {
            out.push(Msg::UiZoomStep(false));
        }
    });
    ui.add_space(6.0);
}

/// The latest message on the status line ("(+1 more) All GPUs idle …");
/// click for a popover with all of them, newest first, and Clear all.
/// A message about a server the app started: a button opening its log in
/// the log viewer (none while it has no log file yet).
fn message_log_button(ui: &mut egui::Ui, m: &increart_gui_core::Message, out: &mut Vec<Msg>) {
    let Some(file) = &m.log else { return };
    if !increart_gui_core::runtime::log_dir().join(file).is_file() {
        return;
    }
    if ui.small_button(ic(ph::SCROLL, "Log")).on_hover_text(format!("open the server's log ({file})")).clicked() {
        out.push(Msg::OpenLogs(Some(file.clone())));
        egui::Popup::close_all(ui.ctx());
    }
}

fn messages_line(
    ui: &mut egui::Ui,
    st: &AppState,
    latest: &increart_gui_core::Message,
    out: &mut Vec<Msg>,
    force_open: bool,
) {
    let resp = ui
        .scope(|ui| {
            ui.style_mut().interaction.selectable_labels = false;
            if st.messages.len() > 1 {
                ui.weak(format!("(+{} more)", st.messages.len() - 1));
            }
            let c = if latest.error { BAD } else { Color32::LIGHT_GRAY };
            ui.add(egui::Label::new(RichText::new(&latest.text).color(c)).truncate());
        })
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("{}\nclick for all messages", latest.text));
    let mut popup = egui::Popup::from_toggle_button_response(&resp);
    if force_open {
        popup = popup.open_memory(egui::SetOpenCommand::Bool(true));
    }
    popup
        .align(egui::RectAlign::TOP_START)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(520.0)
        .show(|ui| {
            ui.set_min_width(520.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic(ph::CHAT_TEXT, &format!("Messages ({})", st.messages.len()))).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(ic(ph::BROOM, "Clear all")).clicked() {
                        out.push(Msg::ClearMessages);
                        egui::Popup::close_all(ui.ctx());
                    }
                });
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for (i, m) in st.messages.iter().enumerate().rev() {
                    ui.horizontal(|ui| {
                        if ui.small_button(ph::X).on_hover_text("dismiss").clicked() {
                            out.push(Msg::DismissMessage(i));
                        }
                        if m.alert {
                            crate::settings::alert_links(ui, out);
                        }
                        message_log_button(ui, m, out);
                        let (icon, c) = if m.error { (ph::WARNING, BAD) } else { (ph::INFO, Color32::LIGHT_GRAY) };
                        ui.add(egui::Label::new(RichText::new(format!("{icon} {}", m.text)).color(c)).wrap());
                    });
                }
            });
        });
}

/// Bottom-right: a summary of the runs in the engine with an overall
/// progress bar; click for a popover with each run and the servers.
fn jobs_indicator(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>, force_open: bool) {
    let sum = st.job_summary();
    let resp = ui
        .horizontal(|ui| {
            // a click on the text opens the popover, not a selection
            ui.style_mut().interaction.selectable_labels = false;
            // prompt enhance's asks: "PE 4 / 41" over every batch
            let batches = st.pe.batches();
            let pe = (!batches.is_empty()).then(|| {
                let total: usize = batches.iter().map(|b| b.total).sum();
                let at: usize = batches.iter().map(|b| b.total - b.ids.len()).sum::<usize>() + 1;
                format!("PE {at} / {total}")
            });
            if sum.runs == 0 && pe.is_none() {
                let servers = st.servers.iter().filter(|s| s.state == "ready").count();
                let text = if st.servers.is_empty() {
                    "no jobs".to_string()
                } else {
                    format!("no jobs · {servers} server(s) ready")
                };
                ui.weak(ic(ph::QUEUE, &text));
            } else if sum.runs == 0 {
                ui.spinner();
                ui.label(ic(ph::MAGIC_WAND, pe.as_deref().unwrap_or_default()));
            } else {
                ui.add(egui::ProgressBar::new(sum.fraction).desired_width(140.0).desired_height(12.0));
                let mut text = format!("{} run(s) · {} / {} seeds", sum.runs, sum.done, sum.total);
                if let Some(pe) = &pe {
                    text.push_str(&format!(" · {pe}"));
                }
                ui.label(ic(ph::SPARKLE, &text));
            }
        })
        .response
        .interact(Sense::click())
        .on_hover_text("Jobs: click for details");
    let mut popup = egui::Popup::from_toggle_button_response(&resp);
    if force_open {
        popup = popup.open_memory(egui::SetOpenCommand::Bool(true));
    }
    popup
        .align(egui::RectAlign::TOP_END)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(520.0)
        .show(|ui| {
            // a fixed width: the states and steps in it change all the time
            ui.set_min_width(520.0);
            ui.set_max_width(520.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic(ph::QUEUE, "Jobs")).strong());
                ui.add_space(12.0);
                crate::projects::order_toggle(ui, st, out);
            });
            // the stops for all stay in view; the runs scroll
            crate::projects::stop_all_row(ui, st, out);
            let n = out.len();
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| crate::projects::run_list(ui, st, out));
            if out[n..].iter().any(|m| matches!(m, Msg::JobInfo(_))) {
                egui::Popup::close_all(ui.ctx());
            }
            if !st.pe.jobs.is_empty() {
                ui.separator();
                ui.label(RichText::new(ic(ph::MAGIC_WAND, "Prompt enhance")).strong());
                crate::projects::pe_list(ui, st, out);
            }
            ui.separator();
            ui.label(RichText::new(ic(ph::HARD_DRIVES, "Servers")).strong());
            crate::projects::server_list(ui, st, out);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Open Projects → Jobs").clicked() {
                    out.push(Msg::Tab(Tab::Projects));
                    out.push(Msg::ProjectsTab(ProjectsTab::Jobs));
                    egui::Popup::close_all(ui.ctx());
                }
                if ui.button(ic(ph::GEAR, "Change servers…")).clicked() {
                    out.push(Msg::OpenServerDialog);
                    egui::Popup::close_all(ui.ctx());
                }
            });
        });
}

/// The Export… dialog: destination, metadata, file names. Enter exports,
/// Escape or a click outside cancels.
fn export_window(ctx: &egui::Context, e: &increart_gui_core::ExportDialog, out: &mut Vec<Msg>) {
    let mut d = e.clone();
    let m = egui::Modal::new(egui::Id::unique("export")).show(ctx, |ui| {
        ui.heading(ic(ph::EXPORT, &format!("Export {}", e.what)));
        ui.label(format!("{} image(s), copied as PNG; the library is unchanged.", d.images.len()));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("To");
            ui.add(egui::TextEdit::singleline(&mut d.dest).desired_width(320.0).hint_text("/path/to/folder"));
            if ui.button(ph::FOLDER_OPEN).on_hover_text("Browse…").clicked() {
                out.push(Msg::Pick(increart_gui_core::PickKind::ExportDir));
            }
        });
        ui.checkbox(&mut d.keep_metadata, "keep metadata (recipe, sd-server parameters, EXIF)");
        ui.horizontal(|ui| {
            ui.label("Names:");
            ui.radio_value(&mut d.by_id, false, "run / revision / seed");
            ui.radio_value(&mut d.by_id, true, "image ID");
        });
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.add_space(8.0);
        dialog_buttons(ui, |ui| {
            if ui.button(RichText::new(ic(ph::EXPORT, "Export   Enter")).strong()).clicked() || enter {
                out.push(Msg::RunExport);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::CloseExport);
            }
        });
    });
    if &d != e {
        out.push(Msg::EditExport(d));
    }
    if m.should_close() {
        out.push(Msg::CloseExport);
    }
}

/// The top bar's **Servers…**: each server's state and model, what is
/// generating, and the way to the server dialog.
fn server_status_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let m = egui::Modal::new(egui::Id::unique("server_status")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading(ic(ph::HARD_DRIVES, "Servers"));
        ui.add_space(6.0);
        crate::projects::server_list(ui, st, out);
        if st.background.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Servers are still starting; they join when they answer.");
            });
        }
        ui.add_space(6.0);
        let seeds = st.progress.keys().count();
        let runs = st.jobs().len();
        ui.weak(match (st.servers.is_empty(), runs) {
            (true, 0) => "Nothing can generate until a server is connected.".to_string(),
            (true, n) => format!("{n} run(s) waiting for a server."),
            (false, 0) => "Nothing generating.".to_string(),
            (false, n) => format!("{n} run(s) in the queue, {seeds} seed(s) generating now."),
        });
        if let Some(p) = &st.config.last_preset
            && !st.servers.is_empty()
        {
            ui.weak(format!("Preset: {p}"));
        }
        ui.add_space(8.0);
        ui.label(RichText::new(ic(ph::MAGIC_WAND, "Prompt enhance (PE)")).strong());
        pe_section(ui, st, out);
        ui.add_space(8.0);
        let (logs, _) = dialog_footer(
            ui,
            |ui| ui.button(ic(ph::SCROLL, "Logs…")).on_hover_text("the logs of the servers IncreArt started").clicked(),
            |ui| {
                if ui.button(RichText::new(ic(ph::GEAR, "Change servers…")).strong()).clicked() {
                    out.push(Msg::OpenServerDialog);
                }
                if ui.button(ic(ph::QUEUE, "Jobs")).on_hover_text("Projects → Jobs").clicked() {
                    out.push(Msg::ServerStatus(false));
                    out.push(Msg::Tab(Tab::Projects));
                    out.push(Msg::ProjectsTab(ProjectsTab::Jobs));
                }
                if ui.button("Close   Esc").clicked() {
                    out.push(Msg::ServerStatus(false));
                }
            },
        );
        if logs {
            out.push(Msg::OpenLogs(None));
        }
    });
    if m.should_close() {
        out.push(Msg::ServerStatus(false));
    }
}

/// An output image's title: Enter or Save keeps it (empty removes it),
/// Escape or Cancel closes.
fn title_window(ctx: &egui::Context, e: &increart_gui_core::TitleEdit, out: &mut Vec<Msg>) {
    let m = egui::Modal::new(egui::Id::unique("image_title")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading(ic(ph::TEXT_AA, "Image title"));
        ui.weak("Shown with the image and found by the Output library's search. Empty removes it.");
        ui.add_space(4.0);
        let mut text = e.text.clone();
        let r = ui.add(egui::TextEdit::singleline(&mut text).hint_text("a title").desired_width(f32::INFINITY));
        r.request_focus();
        if r.changed() {
            out.push(Msg::EditTitle(text));
        }
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.add_space(6.0);
        dialog_buttons(ui, |ui| {
            if ui.button(RichText::new("Save   Enter").strong()).clicked() || enter {
                out.push(Msg::CommitTitle);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::TitleDialog(None));
            }
        });
    });
    if m.should_close() {
        out.push(Msg::TitleDialog(None));
    }
}

/// After Queue with no server, once per session: the run would only wait.
fn no_server_window(ctx: &egui::Context, out: &mut Vec<Msg>) {
    let m = egui::Modal::new(egui::Id::unique("no_server")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.heading(RichText::new(ic(ph::WARNING, "No server is connected")).color(NEUTRAL));
        ui.add_space(4.0);
        ui.label(
            "The run will wait in the queue until a server joins. You won't be asked again until IncreArt restarts.",
        );
        ui.add_space(8.0);
        dialog_buttons(ui, |ui| {
            if ui.button(RichText::new(ic(ph::GEAR, "Set up servers…")).strong()).clicked() {
                out.push(Msg::CloseNoServerWarning);
                out.push(Msg::OpenServerDialog);
            }
            if ui.button(ic(ph::QUEUE, "Queue anyway")).clicked() {
                out.push(Msg::CloseNoServerWarning);
                out.push(Msg::QueueGenerate);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::CloseNoServerWarning);
            }
        });
    });
    if m.should_close() {
        out.push(Msg::CloseNoServerWarning);
    }
}

/// Back / forward from the keyboard and the mouse's side buttons, except
/// in a folder view (which goes through its own history with them).
fn nav_keys(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    if st.folder_view_shown() || st.confirm.is_some() || st.logs.is_some() || ctx.egui_wants_keyboard_input() {
        return;
    }
    ctx.input(|i| {
        if (i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft))
            || i.pointer.button_pressed(egui::PointerButton::Extra1)
        {
            out.push(Msg::NavBack);
        }
        if (i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight))
            || i.pointer.button_pressed(egui::PointerButton::Extra2)
        {
            out.push(Msg::NavForward);
        }
    });
}

/// The question of `AppState::confirm`: its verb, or Cancel.
fn confirm_window(ctx: &egui::Context, c: &increart_gui_core::Confirm, out: &mut Vec<Msg>) {
    let m = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading("Are you sure?");
        ui.label(&c.text);
        ui.add_space(8.0);
        dialog_buttons(ui, |ui| {
            if let Some((name, _)) = &c.fix
                && ui.button(RichText::new(name).strong()).clicked()
            {
                out.push(Msg::ConfirmFix);
            }
            if ui.button(RichText::new(&c.verb).color(BAD).strong()).clicked() {
                out.push(Msg::ConfirmYes);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::ConfirmNo);
            }
        });
    });
    if m.should_close() {
        out.push(Msg::ConfirmNo);
    }
}

impl Drop for App {
    /// Saves pending drafts and stops (or, if so set, leaves running) the
    /// servers the app started, before the window closes.
    fn drop(&mut self) {
        for fx in update(&mut self.st, Msg::FlushDrafts) {
            self.rt.save_now(fx);
        }
        self.rt.shutdown(self.st.config.sdserver.leave_running);
    }
}

/// "draft saved" / "unsaved changes…" next to a form.
pub fn draft_note(ui: &mut egui::Ui, st: &AppState, dirty: Option<f64>) {
    if dirty.is_some() {
        ui.weak(ic(ph::PENCIL_SIMPLE, "unsaved changes…"));
    } else if st.draft_saved.is_some_and(|t| st.now - t < 3.0) {
        ui.weak(ic(ph::CHECK, "draft saved"));
    }
}

pub trait Counts {
    fn library_ideas_all(&self) -> usize;
}

impl Counts for AppState {
    fn library_ideas_all(&self) -> usize {
        self.index.ideas.iter().filter(|e| matches!(e.loc, increart_core::IdeaLoc::Library(_))).count()
    }
}

impl eframe::App for App {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(t) = &mut self.tour {
            t.raw_input(raw);
        }
    }

    /// Runs even while the window is minimized or hidden (no `ui` then), so
    /// the clock, the runtime's messages and the notification alerts go on.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // a clock of our own: egui's input time stops while hidden
        self.dispatch(Msg::Tick(self.started.elapsed().as_secs_f64()));
        for m in self.rt.poll() {
            self.dispatch(m);
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused != self.st.alerts.focused {
            self.dispatch(Msg::Focused(focused));
        }
        // the flash is for an unfocused window, and goes once it is focused
        // or nothing is alerting any more
        if std::mem::take(&mut self.st.alerts.attention) && !focused {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
            self.attention = true;
        }
        let a = &self.st.alerts;
        if self.attention
            && (focused || (a.idle_text.is_none() && a.cleared_text.is_none() && a.backlog_text.is_none()))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Reset));
            self.attention = false;
        }
        // the config's timeouts for the clients made from now on
        if self.st.config.timeouts != increart_core::config::timeouts() {
            increart_core::config::set_timeouts(self.st.config.timeouts);
        }
        let n = &self.st.config.notifications;
        if n.gpu_idle.on || n.review_backlog.on {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.zoom(&ctx);
        if let Some(img) = self.st.clipboard_image.take() {
            let size = [img.0.width() as usize, img.0.height() as usize];
            ctx.copy_image(egui::ColorImage::from_rgba_unmultiplied(size, img.0.as_raw()));
        }
        self.tex.poll(&ctx);
        if self.st.library != self.shown_library {
            self.tex.clear();
            self.shown_library = self.st.library.clone();
        }
        let mut out = vec![];
        if let Some(t) = &mut self.tour {
            t.step(&ctx, &self.st, &self.tex, &mut out);
            if let Some((servers, size)) = t.top_bar() {
                self.st.servers = servers;
                if ctx.input(|i| i.viewport().inner_rect.map(|r| r.size())) != Some(size.into()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size.into()));
                }
            }
        }
        match self.st.phase.clone() {
            Phase::Library(d) => {
                let recent = &self.st.config.recent_libraries;
                let back = self.st.switching.and(self.st.library.as_deref());
                let archiving = self.st.archiving.as_ref();
                egui::CentralPanel::default()
                    .show(ui, |ui| crate::startup::library(ui, &d, recent, back, archiving, &mut out));
            }
            Phase::Servers(d) => {
                let st = &self.st;
                egui::CentralPanel::default().show(ui, |ui| crate::startup::servers(ui, st, &d, &mut out));
                // the preset editor's gear opens it here too
                crate::projects::recipes_window(&ctx, st, &mut out);
            }
            Phase::Main => self.main_window(ui, &mut out),
        }
        if self.st.phase == Phase::Main {
            if let Some(v) = &self.st.history_view {
                crate::history::window(&ctx, &self.st, v, &mut out);
            }
            crate::ideas::edit_window(&ctx, &self.st, &mut self.tex, &mut out);
            crate::settings::window(&ctx, &self.st, &mut out);
            crate::projects::resolutions_window(&ctx, &self.st, &mut out);
            crate::projects::recipes_window(&ctx, &self.st, &mut out);
            crate::projects::scale_window(&ctx, &self.st, &mut out);
            // under the modals it opens (picker, preview, job details)
            crate::tiles::window(&ctx, &self.st, &mut self.tex, &mut out);
            // under the preview, which its images open
            crate::projects::job_window(&ctx, &self.st, &mut self.tex, &mut out);
            crate::refpicker::window(&ctx, &self.st, &mut self.tex, &mut out);
            if let Some(e) = &self.st.title_edit {
                title_window(&ctx, e, &mut out);
            }
            let keys_open = self.tour.as_ref().is_some_and(|t| t.preview_keys());
            crate::projects::preview_window(&ctx, &self.st, &mut self.tex, keys_open, &mut out);
            crate::projects::retag_window(&ctx, &self.st, &mut out);
            crate::projects::new_project_window(&ctx, &self.st, &mut self.tex, &mut out);
            crate::projects::duplicate_window(&ctx, &self.st, &mut out);
            crate::home::start_project_window(&ctx, &self.st, &mut out);
            crate::projects::promote_window(&ctx, &self.st, &mut out);
            crate::projects::sweep_order_window(&ctx, &self.st, &mut out);
            crate::tree::rename_window(&ctx, &self.st, &mut out);
            if self.st.tab == Tab::Projects && self.st.projects_tab == ProjectsTab::Generate {
                crate::presets::window(&ctx, &self.st, &mut out);
            }
            crate::styles::window(&ctx, &self.st, &mut self.tex, &mut out);
            crate::presets::editor_window(&ctx, &self.st, &mut out);
            crate::rewrite::window(&ctx, &self.st, &mut self.tex, &mut out);
            crate::presets::save_window(&ctx, &self.st, &mut out);
            if let Some(e) = &self.st.export {
                export_window(&ctx, e, &mut out);
            }
            if self.st.server_status {
                server_status_window(&ctx, &self.st, &mut out);
            }
            if self.st.no_server_warning {
                no_server_window(&ctx, &mut out);
            }
            // over the preview and the Tile window it may be opened from
            crate::markup::window(&ctx, &self.st, &mut self.tex, &mut out);
        }
        // over the server dialog or the status window
        crate::pe_settings::window(&ctx, &self.st, &mut out);
        crate::presets::added_blanks_window(&ctx, &self.st, &mut out);
        if let Some(c) = &self.st.confirm {
            confirm_window(&ctx, c, &mut out);
        }
        // over the preset editor
        if let Some(t) = &self.st.server_test {
            crate::startup::server_test_window(&ctx, t, &mut out);
        }
        // over the server dialog
        if let Some(t) = &self.st.api_test {
            crate::apitest::window(&ctx, &self.st, t, &mut out);
        }
        // over whichever window it was opened from
        if let Some(v) = &self.st.logs {
            crate::logs::window(&ctx, &self.st, v, &mut self.logs, &mut out);
        }
        for m in out {
            self.dispatch(m);
        }
        let opening = matches!(&self.st.phase, Phase::Servers(d) if d.opening) || self.st.background.is_some();
        if self.st.busy > 0
            || self.st.engine_running
            || opening
            || self.st.generate_dirty.is_some()
            || self.st.idea_dirty.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

/// The PE server's dot, state and colour, for the top bar and Jobs.
pub fn pe_state(ui: &egui::Ui, st: &AppState) -> (&'static str, String, Color32) {
    use increart_gui_core::PeStatus;
    let weak = ui.visuals().weak_text_color();
    let Some(first) = st.pe.server.as_ref() else { return ("○", "no chat server".into(), weak) };
    let total = st.pe.pool_size();
    let name = if total > 1 { format!("{} + {} more", first.name, total - 1) } else { first.name.clone() };
    let server = increart_core::pe::PeServer { name, ..first.clone() };
    let waiting = st.pe.jobs.len();
    if waiting > 0 || st.pe.status == PeStatus::Starting {
        // the times shown go on
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
    match &st.pe.status {
        PeStatus::Off if server.start == increart_core::pe::PeStart::AtLaunch => {
            ("○", format!("{}: stopped", server.name), weak)
        }
        PeStatus::Off => ("○", format!("{}: starts when asked", server.name), weak),
        PeStatus::Starting => ("◌", format!("{}: {}", server.name, crate::rewrite::starting(st)), NEUTRAL),
        PeStatus::Ready if waiting > 0 => {
            let now = st.pe.working().map(|j| crate::rewrite::progress(st, j)).unwrap_or_default();
            ("●", format!("{}: working ({waiting}): {now}", server.name), NEUTRAL)
        }
        PeStatus::Ready if st.pe.ready < total => {
            ("●", format!("{}: {} of {total} ready", server.name, st.pe.ready), NEUTRAL)
        }
        PeStatus::Ready => ("●", format!("{}: ready", server.name), GOOD),
        PeStatus::Failed(_) => ("●", format!("{}: failed", server.name), BAD),
    }
}

/// The server status window's Prompt enhance section: the chat server in
/// use, how it runs, its state, and the system prompts (the built-in ones
/// unless changed), shared by every chat server.
fn pe_section(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    use increart_core::pe::JOBS;
    crate::pe_settings::summary(ui, st, out);
    let pe = &st.config.pe;
    if let increart_gui_core::PeStatus::Failed(e) = &st.pe.status {
        ui.colored_label(BAD, e);
    }
    ui.add_space(4.0);
    for (job, default) in JOBS {
        let title = match job {
            increart_core::pe::FILL_BLANKS => "System prompt: suggesting values for a sweep's blanks",
            increart_core::pe::REWRITE => "System prompt: rewriting a prompt or an idea",
            increart_core::pe::DESCRIBE => "System prompt: describing an image",
            _ => job,
        };
        let own = pe.system_prompts.get(job).filter(|p| !p.trim().is_empty());
        let own_sampling = pe.sampling.get(job);
        let head =
            if own.is_some() || own_sampling.is_some() { format!("{title}  (changed)") } else { title.to_string() };
        egui::CollapsingHeader::new(head).id_salt(("pe_system_head", job)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if own.is_none() {
                    ui.weak("built in; edit to change it for every chat server");
                } else if ui.small_button(ic(ph::ARROW_COUNTER_CLOCKWISE, "Default")).clicked() {
                    out.push(Msg::PeSystemPrompt { job: job.into(), text: String::new() });
                }
            });
            let mut text = own.cloned().unwrap_or_else(|| default.to_string());
            let r = egui::ScrollArea::vertical()
                .id_salt(("pe_system", job))
                .max_height(140.0)
                .show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut text).desired_width(f32::INFINITY).desired_rows(5))
                })
                .inner;
            if r.changed() {
                // the default, unchanged, stays the default
                let text = if text == default { String::new() } else { text };
                out.push(Msg::PeSystemPrompt { job: job.into(), text });
            }
            ui.horizontal(|ui| {
                let mut x = pe.sampling(job);
                ui.label("Temperature");
                ui.add(egui::Slider::new(&mut x.temperature, 0.0..=2.0).step_by(0.05))
                    .on_hover_text("hotter: more varied and surprising, but less on topic");
                ui.label("min_p");
                ui.add(egui::Slider::new(&mut x.min_p, 0.0..=0.5).step_by(0.01)).on_hover_text(
                    "drops words less than this fraction as likely as the likeliest: keeps a hot answer \
                     sensible (0: off)",
                );
                if own_sampling.is_some() && ui.small_button(ic(ph::ARROW_COUNTER_CLOCKWISE, "Default")).clicked() {
                    out.push(Msg::PeSampling { job: job.into(), sampling: None });
                } else if x != pe.sampling(job) {
                    let sampling = (x != increart_core::pe::Sampling::default_for(job)).then_some(x);
                    out.push(Msg::PeSampling { job: job.into(), sampling });
                }
            });
        });
    }
}
