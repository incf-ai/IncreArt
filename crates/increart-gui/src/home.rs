//! Home: how the tabs fit together (ideas in, images out), buttons to
//! start, and the latest projects; and its Start a new project dialog.

use eframe::egui::{self, Color32, RichText, ScrollArea, Vec2};
use increart_gui_core::{AppState, InboxTab, Msg, PickKind, StartProjectDialog, StartRefs, Tab};

use crate::app::Counts;
use crate::widgets::{ACCENT, BAD, dialog_buttons, ic, ph, when};

/// The widest the page gets: wider, its lines are hard to read.
const PAGE: f32 = 960.0;
/// The narrowest a step's card gets; as many as fit go in a row, up to 3.
const STEP_MIN_W: f32 = 260.0;
const STEP_H: f32 = 176.0;
/// The arrow between two cards in a row.
const ARROW_W: f32 = 22.0;

pub fn view(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    egui::CentralPanel::default().show(ui, |ui| {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let side = ((ui.available_width() - PAGE) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(side);
                ui.vertical(|ui| {
                    ui.set_max_width(PAGE.min(ui.available_width()));
                    ui.add_space(12.0);
                    welcome(ui, st, out);
                    ui.add_space(20.0);
                    ui.heading("How it works");
                    ui.weak("Ideas come in on the left, finished images come out on the right. Each step is a tab at the top.");
                    ui.add_space(8.0);
                    steps(ui, st, out);
                    ui.add_space(20.0);
                    recent_projects(ui, st, out);
                    ui.add_space(12.0);
                });
            });
        });
    });
}

fn welcome(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    ui.label(RichText::new("Welcome to IncreArt").size(26.0).strong());
    ui.label(
        "Collect ideas and reference images, keep the good ones, turn them into projects, \
         generate many seeds on your GPUs, and keep the best results.",
    );
    ui.add_space(12.0);
    let big = |icon: &str, text: &str| {
        egui::Button::new(RichText::new(ic(icon, text)).size(16.0)).min_size(Vec2::new(0.0, 36.0))
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(10.0, 10.0);
        let start = egui::Button::new(
            RichText::new(ic(ph::ROCKET_LAUNCH, "Start a new project…")).size(16.0).strong().color(Color32::WHITE),
        )
        .fill(ACCENT.gamma_multiply(0.75))
        .min_size(Vec2::new(0.0, 36.0));
        if ui.add(start).on_hover_text("name it, pick its reference images, then write the prompt").clicked() {
            out.push(Msg::StartProject(true));
        }
        if ui
            .add(big(ph::NOTE_PENCIL, "Write an idea"))
            .on_hover_text("a prompt, images, or both, into the Idea inbox")
            .clicked()
        {
            out.push(Msg::Tab(Tab::IdeaInbox));
            out.push(Msg::InboxTab(InboxTab::NewIdea));
        }
        if ui
            .add(big(ph::FILE_ARROW_DOWN, "Import images…"))
            .on_hover_text("image or .txt files, each a new idea in the Idea inbox")
            .clicked()
        {
            out.push(Msg::Pick(PickKind::ImportFiles));
        }
        if ui.add(big(ph::FOLDER_OPEN, "Open the drop folder")).on_hover_text(drop_folder_tip(st)).clicked() {
            out.push(Msg::ShowInFileManager("input/inbox_drop".into()));
        }
    });
    // what's waiting, if anything
    let ideas = st.unreviewed_ideas().len();
    let images = st.unreviewed_outputs();
    if ideas + images > 0 {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.weak("Waiting for you:");
            if ideas > 0 && ui.link(ic(ph::TRAY, &(plural(ideas, "idea", "ideas") + " to sort"))).clicked() {
                out.push(Msg::Tab(Tab::IdeaInbox));
                out.push(Msg::InboxTab(InboxTab::Grid));
            }
            if images > 0 && ui.link(ic(ph::THUMBS_UP, &(plural(images, "image", "images") + " to review"))).clicked() {
                out.push(Msg::Tab(Tab::Review));
            }
        });
    }
}

fn drop_folder_tip(st: &AppState) -> String {
    let dir = st.library.as_ref().map(|l| l.join("input").join("inbox_drop").display().to_string());
    format!(
        "{}\nimages and .txt files saved here become ideas in the Idea inbox by themselves",
        dir.unwrap_or_else(|| "input/inbox_drop".into())
    )
}

struct Step {
    icon: &'static str,
    title: &'static str,
    text: &'static str,
    keys: &'static str,
    /// Empty: none.
    count: String,
    /// The card's button: icon, text and tooltip; and what it does.
    button: (&'static str, &'static str, String),
    open: Vec<Msg>,
}

fn steps(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let steps = [
        Step {
            icon: ph::TRAY_ARROW_DOWN,
            title: "Collect",
            text: "Save scraped or downloaded images (and .txt prompts) into the drop folder, drag files onto the \
                   window, or write an idea. Everything lands in the Idea inbox, unsorted.",
            keys: "",
            // files are imported as soon as they have finished copying:
            // only a stuck one waits
            count: match st.index.inbox_drop.len() {
                0 => String::new(),
                n => format!("{n} waiting to import"),
            },
            button: (ph::FOLDER_OPEN, "Open the drop folder", drop_folder_tip(st)),
            open: vec![Msg::ShowInFileManager("input/inbox_drop".into())],
        },
        Step {
            icon: ph::TRAY,
            title: "Sort the inbox",
            text: "Go through the inbox and keep or discard each idea. Kept ideas move to the Input library; \
                   discarded ones wait under Discarded, where they can be restored.",
            keys: "1 / G keep and star · 2 / N keep · 3 / B discard · Ctrl+Z undo",
            count: format!("{} to sort", st.unreviewed_ideas().len()),
            button: open_tab(Tab::IdeaInbox),
            open: vec![Msg::Tab(Tab::IdeaInbox), Msg::InboxTab(InboxTab::Grid)],
        },
        Step {
            icon: ph::BOOKS,
            title: "Input library",
            text: "Your kept ideas and reference images, filed in folders, starred or not. Promote ideas into a \
                   project, or use their images as references.",
            keys: "P promote",
            count: format!("{} kept ideas", st.library_ideas_all()),
            button: open_tab(Tab::InputLibrary),
            open: vec![Msg::Tab(Tab::InputLibrary)],
        },
        Step {
            icon: ph::ROCKET_LAUNCH,
            title: "Projects",
            text: "A project holds runs: one prompt over many seeds, with reference images. Generate queues \
                   them across all your GPUs.",
            keys: "",
            count: plural(st.index.projects.len(), "project", "projects"),
            button: open_tab(Tab::Projects),
            open: vec![Msg::Tab(Tab::Projects)],
        },
        Step {
            icon: ph::THUMBS_UP,
            title: "Review",
            text: "Generated images land here, from every project. Rate each one; good and neutral ones go to \
                   the Output library.",
            keys: "1 / G good · 2 / N neutral · 3 / B bad",
            count: format!("{} to review", st.unreviewed_outputs()),
            button: open_tab(Tab::Review),
            open: vec![Msg::Tab(Tab::Review)],
        },
        Step {
            icon: ph::IMAGES,
            title: "Output library",
            text: "The images you kept, with the recipe and lineage of each. Use one as a reference, start a \
                   new project from it, or export.",
            keys: "",
            count: format!("{} kept images", st.kept_outputs().len()),
            button: open_tab(Tab::OutputLibrary),
            open: vec![Msg::Tab(Tab::OutputLibrary)],
        },
    ];
    // rows of equal cards, an arrow between two in a row
    let room = ui.available_width();
    // an arrow and the spacing on its two sides
    let gap = ARROW_W + 2.0 * ui.spacing().item_spacing.x;
    let cols = (((room + gap) / (STEP_MIN_W + gap)).floor() as usize).clamp(1, 3);
    let w = ((room - gap * (cols - 1) as f32) / cols as f32).floor();
    for (r, row) in steps.chunks(cols).enumerate() {
        ui.horizontal_top(|ui| {
            for (i, s) in row.iter().enumerate() {
                if i > 0 {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(ARROW_W, STEP_H), egui::Sense::hover());
                    let color = ui.visuals().weak_text_color();
                    let font = egui::FontId::proportional(18.0);
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, ph::CARET_RIGHT, font, color);
                }
                step_card(ui, r * cols + i + 1, s, w, out);
            }
        });
        ui.add_space(8.0);
    }
}

fn step_card(ui: &mut egui::Ui, n: usize, s: &Step, w: f32, out: &mut Vec<Msg>) {
    let margin = 12.0;
    let frame = egui::Frame::group(ui.style()).inner_margin(margin).corner_radius(8.0);
    // the size given is the frame's, stroke included
    let inner = Vec2::new(w, STEP_H) - Vec2::splat(2.0 * margin + 2.0 * frame.stroke.width);
    frame.show(ui, |ui| {
        ui.set_width(inner.x);
        ui.set_height(inner.y);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{n}")).size(22.0).strong().color(ACCENT));
                ui.label(RichText::new(ic(s.icon, s.title)).size(17.0).strong());
            });
            ui.add_space(4.0);
            ui.label(s.text);
            if !s.keys.is_empty() {
                ui.add_space(2.0);
                ui.label(RichText::new(ic(ph::KEYBOARD, s.keys)).size(12.0).weak());
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.horizontal(|ui| {
                    let (icon, text, tip) = &s.button;
                    if ui.button(ic(icon, text)).on_hover_text(tip).clicked() {
                        out.extend(s.open.iter().cloned());
                    }
                    ui.add(egui::Label::new(RichText::new(&s.count).weak()).truncate());
                });
            });
        });
    });
}

fn open_tab(t: Tab) -> (&'static str, &'static str, String) {
    let text = match t {
        Tab::Home => "Open Home",
        Tab::IdeaInbox => "Open Idea Inbox",
        Tab::InputLibrary => "Open Input Library",
        Tab::Projects => "Open Projects",
        Tab::Review => "Open Review",
        Tab::OutputLibrary => "Open Output Library",
    };
    (ph::ARROW_RIGHT, text, "open its tab".into())
}

/// The projects with the latest runs (or made last), to pick up where you
/// left off.
fn recent_projects(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    let mut projects: Vec<_> = st
        .index
        .projects
        .iter()
        .map(|p| (p.runs.iter().map(|r| r.created).max().unwrap_or(p.project.created).max(p.project.created), p))
        .collect();
    if projects.is_empty() {
        ui.heading("Projects");
        ui.weak("No projects yet. Start one above, or promote ideas from the Input library.");
        return;
    }
    projects.sort_by_key(|p| std::cmp::Reverse(p.0));
    ui.heading("Pick up where you left off");
    ui.add_space(4.0);
    egui::Grid::new("home_recent").num_columns(3).spacing([16.0, 6.0]).show(ui, |ui| {
        for (at, p) in projects.into_iter().take(6) {
            if ui
                .link(RichText::new(ic(ph::ROCKET_LAUNCH, &p.name())).strong())
                .on_hover_text("open it in Projects")
                .clicked()
            {
                out.push(Msg::SelectProject(p.project.id.clone()));
                out.push(Msg::Tab(Tab::Projects));
            }
            ui.weak(format!(
                "{} · {}",
                plural(p.runs.len(), "run", "runs"),
                plural(p.outputs.len(), "image", "images")
            ));
            ui.weak(when(at));
            ui.end_row();
        }
    });
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Start a new project: its name, and whether to pick reference images
/// once it is made.
pub fn start_project_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let Some(d) = &st.start_project else { return };
    let m = egui::Modal::new(egui::Id::unique("start_project")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading(ic(ph::ROCKET_LAUNCH, "Start a new project"));
        ui.add_space(6.0);
        ui.label("Name");
        let mut name = d.name.clone();
        let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
        crate::widgets::focus_until_done(&r);
        if name != d.name {
            out.push(Msg::EditStartProject(StartProjectDialog { name: name.clone(), refs: d.refs }));
        }
        let invalid = increart_core::library::validate_name(name.trim()).err();
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && invalid.is_none() {
            out.push(Msg::ConfirmStartProject);
        }
        if let Some(e) = &invalid {
            ui.colored_label(BAD, e.to_string());
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Add reference images?").strong());
        ui.weak("The prompt points at them as <image1>, <image2>, …: a style, a character, a layout to follow.");
        ui.add_space(4.0);
        for (refs, label, tip) in [
            (StartRefs::Library, ic(ph::IMAGES, "Yes, from my libraries…"), "kept ideas' images and generated images"),
            (
                StartRefs::Files,
                ic(ph::FILE_IMAGE, "Yes, image files…"),
                "files from the disk, copied into the input library",
            ),
            (StartRefs::None, ic(ph::X, "Not now"), "add them later with Add file… or From library… on Generate"),
        ] {
            if ui.radio(d.refs == refs, label).on_hover_text(tip).clicked() {
                out.push(Msg::EditStartProject(StartProjectDialog { name: name.clone(), refs }));
            }
        }
        ui.add_space(8.0);
        dialog_buttons(ui, |ui| {
            let create = ui.add_enabled(
                invalid.is_none(),
                egui::Button::new(RichText::new(ic(ph::CHECK, "Create   Enter")).strong()),
            );
            if create.clicked() {
                out.push(Msg::ConfirmStartProject);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::StartProject(false));
            }
        });
    });
    if m.should_close() {
        out.push(Msg::StartProject(false));
    }
}
