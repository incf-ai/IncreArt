//! Prompt presets in Generate: the Presets window (pick one), Edit
//! presets… and Save as preset… (DESIGN.md, Prompt presets).

use eframe::egui::{self, ComboBox, RichText, ScrollArea};
use increart_core::prompts::{PresetKey, Sweep};
use increart_core::prompts::{PromptPreset, PromptPresets, PromptTask};
use increart_gui_core::preset_matches as matches;
use increart_gui_core::{AppState, GenerateForm, Msg, PresetsEditor, PresetsView, SavePresetDialog, needs_refs};

use crate::widgets::{BAD, ICON_FILL, dialog_buttons, dialog_footer, ic, ph, short};

/// The Generate prompt's text field.
pub fn prompt_id() -> egui::Id {
    egui::Id::unique("gen_prompt")
}

/// The Presets window, beside Generate: tasks on the left, their presets on
/// the right. Click replaces the prompt, Shift+click inserts at its cursor.
pub fn window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let Some(panel) = &st.presets_panel else { return };
    let list = &st.prompt_presets;
    let refs = st.generate.refs().len();
    let view = &panel.view;
    let mut open = true;
    egui::Window::new(ic(ph::SWATCHES, "Prompt presets"))
        .id(egui::Id::unique("prompt_presets_window"))
        .open(&mut open)
        // over the images pane, so the prompt stays in view
        .pivot(egui::Align2::RIGHT_TOP)
        .default_pos(ctx.content_rect().right_top() + egui::vec2(-16.0, 56.0))
        .default_width(720.0)
        .default_height((ctx.content_rect().height() - 130.0).clamp(380.0, 690.0))
        .min_height(300.0)
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let mut q = panel.search.clone();
                ui.label(ph::MAGNIFYING_GLASS);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut q)
                        .hint_text("Search names, descriptions and prompts")
                        .desired_width(320.0),
                );
                if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    q.clear();
                }
                if !q.is_empty() && ui.small_button(ph::X).on_hover_text("clear the search").clicked() {
                    q.clear();
                }
                if q != panel.search {
                    out.push(Msg::PresetsSearch(q));
                }
                ui.weak(format!("{} presets in {} tasks", list.count(), list.tasks.len()));
            });
            ui.weak("Click: replace the prompt  ·  Shift+click: insert at the prompt's cursor  ·  ★: favourite");
            ui.separator();
            // the lists take what the (resizable) window leaves above the buttons
            let height = (ui.available_height() - 40.0).max(120.0);
            let row = egui::Layout::left_to_right(egui::Align::Min);
            // bounded: a vertical separator takes all the height it is given
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), row, |ui| {
                let down = egui::Layout::top_down(egui::Align::Min);
                ui.allocate_ui_with_layout(egui::vec2(200.0, height), down, |ui| {
                    ScrollArea::vertical().id_salt("preset_tasks").max_height(height).auto_shrink([false, false]).show(
                        ui,
                        |ui| {
                            ui.set_width(190.0);
                            task_list(ui, list, view, panel.search.is_empty(), out)
                        },
                    );
                });
                ui.separator();
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), down, |ui| {
                    ScrollArea::vertical()
                        .id_salt("preset_list")
                        .max_height(height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| preset_list(ui, list, view, &panel.search.to_lowercase(), refs, out));
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                let at = view.task().unwrap_or(0);
                if ui.button(ic(ph::PENCIL_SIMPLE, "Edit presets…")).clicked() {
                    out.push(Msg::OpenPresetEditor(at, None));
                }
                if ui
                    .button(ic(ph::FLOPPY_DISK, "Save prompt as preset…"))
                    .on_hover_text("the Generate prompt (and negative) as a new preset")
                    .clicked()
                {
                    out.push(Msg::OpenSavePreset);
                }
            });
        });
    if !open {
        out.push(Msg::PresetsPanel(false));
    }
}

fn task_list(ui: &mut egui::Ui, list: &PromptPresets, view: &PresetsView, enabled: bool, out: &mut Vec<Msg>) {
    ui.add_enabled_ui(enabled, |ui| {
        let favs = list.tasks.iter().flat_map(|t| &t.presets).filter(|p| p.favourite).count();
        let fav = RichText::new(format!("{}  Favourites  ({favs})", ph::STAR));
        if ui.selectable_label(*view == PresetsView::Favourites, fav).clicked() {
            out.push(Msg::PresetsShow(PresetsView::Favourites));
        }
        ui.add_space(4.0);
        // each task a folder of its groups (the one shown starts open)
        for (i, t) in list.tasks.iter().enumerate() {
            let n = t.presets.len();
            let label = format!("{}  ({n})", t.name);
            let groups = task_groups(t);
            if groups.is_empty() {
                // nothing to fold: the row, lined up with the folders' labels
                ui.horizontal(|ui| {
                    ui.add_space(ui.spacing().indent);
                    task_row(ui, i, t, label, view, out);
                });
                continue;
            }
            let id = ui.make_persistent_id(("preset task", i));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, view.task() == Some(i))
                .show_header(ui, |ui| task_row(ui, i, t, label, view, out))
                .body(|ui| {
                    for (g, n) in groups {
                        let shown = PresetsView::Group(i, g.to_string());
                        if ui.selectable_label(*view == shown, format!("{g}  ({n})")).clicked() {
                            out.push(Msg::PresetsShow(shown));
                        }
                    }
                });
        }
    });
    if !enabled {
        ui.add_space(6.0);
        ui.weak("Searching every task.");
    }
}

fn task_row(ui: &mut egui::Ui, i: usize, t: &PromptTask, label: String, view: &PresetsView, out: &mut Vec<Msg>) {
    let r = ui.selectable_label(*view == PresetsView::Task(i), label);
    let r = if t.description.is_empty() { r } else { r.on_hover_text(&t.description) };
    if r.clicked() {
        out.push(Msg::PresetsShow(PresetsView::Task(i)));
    }
}

/// A task's groups (headings) in order, with their preset counts; empty
/// when it has none.
fn task_groups(t: &PromptTask) -> Vec<(&str, usize)> {
    let mut v: Vec<(&str, usize)> = vec![];
    for p in t.presets.iter().filter(|p| !p.group.is_empty()) {
        match v.iter_mut().find(|(g, _)| *g == p.group) {
            Some((_, n)) => *n += 1,
            None => v.push((&p.group, 1)),
        }
    }
    v
}

fn preset_list(ui: &mut egui::Ui, list: &PromptPresets, view: &PresetsView, q: &str, refs: usize, out: &mut Vec<Msg>) {
    let task = view.task();
    // (task, preset) shown, under (task or group) headings
    let mut count = 0;
    let mut heading = String::new();
    let list_of = shown(list, view, q);
    for (ti, t) in list.tasks.iter().enumerate() {
        let in_view = if !q.is_empty() { true } else { task.is_none_or(|x| x == ti) };
        if !in_view {
            continue;
        }
        if q.is_empty() && task.is_some() && !t.description.is_empty() {
            ui.weak(&t.description);
            ui.add_space(4.0);
        }
        for (pi, p) in t.presets.iter().enumerate() {
            if !list_of.contains(&(ti, pi)) {
                continue;
            }
            let h = match (task, q.is_empty()) {
                (Some(_), true) => p.group.clone(),
                _ => t.name.clone(),
            };
            if h != heading {
                ui.add_space(6.0);
                ui.label(RichText::new(&h).strong().size(15.0));
                heading = h;
            }
            preset_row(ui, ti, pi, p, refs, out);
            count += 1;
        }
    }
    if count == 0 {
        ui.add_space(8.0);
        ui.weak(match (q.is_empty(), task) {
            (false, _) => "No preset matches.",
            (true, None) => "No favourites yet: click ☆ next to a preset to add it here.",
            (true, Some(_)) if view.group().is_some() => "No presets in this group.",
            (true, Some(_)) => "No presets in this task: add some with Edit presets….",
        });
    }
}

/// The presets (task, preset) the window lists: those matching the search
/// `q` (lower case), else the view's.
fn shown(list: &PromptPresets, view: &PresetsView, q: &str) -> Vec<(usize, usize)> {
    let task = view.task();
    let mut v = vec![];
    for (ti, t) in list.tasks.iter().enumerate() {
        if q.is_empty() && task.is_some_and(|x| x != ti) {
            continue;
        }
        for (pi, p) in t.presets.iter().enumerate() {
            let keep = match (q.is_empty(), task) {
                (false, _) => matches(p, q),
                (true, Some(_)) => view.group().is_none_or(|g| g == p.group),
                (true, None) => p.favourite,
            };
            if keep {
                v.push((ti, pi));
            }
        }
    }
    v
}

fn preset_row(ui: &mut egui::Ui, task: usize, preset: usize, p: &PromptPreset, refs: usize, out: &mut Vec<Msg>) {
    let need = needs_refs(&p.prompt);
    let ok = need <= refs;
    ui.horizontal_top(|ui| {
        let star = if p.favourite {
            RichText::new(ph::STAR).family(egui::FontFamily::Name(ICON_FILL.into())).color(crate::widgets::ACCENT)
        } else {
            RichText::new(ph::STAR).weak()
        };
        let tip = if p.favourite { "remove from the favourites" } else { "add to the favourites" };
        if ui.add(egui::Button::new(star).frame(false)).on_hover_text(tip).clicked() {
            out.push(Msg::PresetFavourite(task, preset));
        }
        ui.vertical(|ui| {
            let r = ui.add_enabled(ok, egui::Button::new(RichText::new(&p.name).strong()).frame(false));
            let mut tip = p.prompt.clone();
            if !p.negative.is_empty() {
                tip.push_str(&format!("\n\nNegative: {}", p.negative));
            }
            let r = r
                .on_hover_text(&tip)
                .on_disabled_hover_text(format!("Needs {need} reference image(s); Generate has {refs}.\n\n{tip}"));
            if r.clicked() {
                let shift = ui.input(|i| i.modifiers.shift);
                let insert_at = shift.then(|| cursor(ui.ctx())).flatten();
                out.push(Msg::ApplyPreset { task, preset, insert_at: insert_at.or(shift.then_some(usize::MAX)) });
            }
            if !p.description.is_empty() {
                ui.add(egui::Label::new(RichText::new(&p.description).weak()).wrap());
            }
        });
    });
}

/// Where the cursor is in the Generate prompt (characters), if it was ever
/// focused.
fn cursor(ctx: &egui::Context) -> Option<usize> {
    let s = egui::TextEdit::load_state(ctx, prompt_id())?;
    s.cursor.char_range().map(|r| r.primary.index.0)
}

/// The Generate prompt's selection (characters, start ≤ end; empty at the
/// cursor), if it was ever focused.
pub fn selection(ctx: &egui::Context) -> Option<std::ops::Range<usize>> {
    let s = egui::TextEdit::load_state(ctx, prompt_id())?;
    s.cursor.char_range().map(|r| {
        let (a, b) = (r.primary.index.0, r.secondary.index.0);
        a.min(b)..a.max(b)
    })
}

/// Puts the Generate prompt's cursor at character `at` and focuses it.
pub fn focus_prompt_at(ctx: &egui::Context, at: usize) {
    let id = prompt_id();
    let mut s = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    s.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(at))));
    s.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// Selects `st.prompt_select` in the prompt field (once).
pub fn select_in_prompt(ui: &egui::Ui, st: &AppState, resp: &egui::Response, out: &mut Vec<Msg>) {
    let Some(r) = &st.prompt_select else { return };
    let id = prompt_id();
    let mut s = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    s.cursor.set_char_range(Some(egui::text::CCursorRange::two(
        egui::text::CCursor::new(r.start),
        egui::text::CCursor::new(r.end),
    )));
    s.store(ui.ctx(), id);
    resp.request_focus();
    out.push(Msg::PromptSelected);
}

/// **Edit presets…**: tasks, their presets, and the selected preset's fields,
/// all on a working copy saved or dropped as a whole.
pub fn editor_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let Some(e0) = &st.presets_editor else { return };
    let mut e = e0.clone();
    let m = egui::Modal::new(egui::Id::unique("presets_editor")).show(ctx, |ui| {
        ui.set_width(980.0);
        ui.heading(ic(ph::PENCIL_SIMPLE, "Edit prompt presets"));
        ui.weak(
            "Shared by every library; yours are files in prompt-presets next to config.rsn. Write <image1>, <image2>… for reference images and {name} for blanks to fill in.",
        );
        ui.add_space(6.0);
        let height = 440.0;
        let down = egui::Layout::top_down(egui::Align::Min);
        let row = egui::Layout::left_to_right(egui::Align::Min);
        // bounded: a vertical separator takes all the height it is given
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), row, |ui| {
            ui.allocate_ui_with_layout(egui::vec2(220.0, height), down, |ui| {
                ui.set_width(220.0);
                tasks_column(ui, &mut e);
            });
            ui.separator();
            ui.allocate_ui_with_layout(egui::vec2(260.0, height), down, |ui| {
                ui.set_width(260.0);
                presets_column(ui, &mut e);
            });
            ui.separator();
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), down, |ui| fields_column(ui, &mut e));
        });
        ui.separator();
        if let Some(err) = &e.error {
            ui.colored_label(BAD, err);
        }
        let (restore, save) = dialog_footer(
            ui,
            |ui| {
                ui.button(ic(ph::ARROW_COUNTER_CLOCKWISE, "Restore defaults"))
                    .on_hover_text("the built-in presets in place of all of these (Cancel still undoes it)")
                    .clicked()
            },
            |ui| {
                let save = ui.button(RichText::new(ic(ph::CHECK, "Save")).strong()).clicked();
                if ui.button("Cancel   Esc").clicked() {
                    out.push(Msg::ClosePresetEditor);
                }
                save
            },
        );
        if save {
            out.push(Msg::SavePresetEditor);
        }
        if restore {
            out.push(Msg::RestorePresetDefaults);
        }
    });
    if &e != e0 {
        e.error = None;
        // the edit lands before a Save sent in the same frame
        out.insert(0, Msg::EditPresetEditor(e));
    }
    if m.should_close() {
        out.push(Msg::ClosePresetEditor);
    }
}

/// A delete button for item `i`, if any is selected; true when clicked.
/// (No moving: the built-in order is kept, the user's presets sorted in.)
fn delete_button(ui: &mut egui::Ui, i: Option<usize>, what: &str) -> bool {
    i.is_some() && ui.button(ph::TRASH).on_hover_text(format!("delete this {what}")).clicked()
}

fn tasks_column(ui: &mut egui::Ui, e: &mut PresetsEditor) {
    ui.label(RichText::new("Tasks").strong());
    ScrollArea::vertical().id_salt("edit_tasks").max_height(250.0).auto_shrink([false, true]).show(ui, |ui| {
        for (i, t) in e.list.tasks.iter().enumerate() {
            if ui.selectable_label(i == e.task, format!("{}  ({})", t.name, t.presets.len())).clicked() {
                e.task = i;
                e.preset = None;
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button(ph::PLUS).on_hover_text("new task").clicked() {
            let name = unique_name(e.list.tasks.iter().map(|t| t.name.as_str()), "New task");
            e.list.tasks.push(PromptTask { name, ..Default::default() });
            e.task = e.list.tasks.len() - 1;
            e.preset = None;
        }
        let n = e.list.tasks.len();
        if delete_button(ui, (e.task < n).then_some(e.task), "task and its presets") {
            e.list.tasks.remove(e.task);
            e.task = e.task.min(e.list.tasks.len().saturating_sub(1));
            e.preset = None;
        }
    });
    if let Some(t) = e.list.tasks.get_mut(e.task) {
        ui.add_space(6.0);
        ui.label("Task name");
        ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(f32::INFINITY));
        ui.label("Description");
        ui.add(egui::TextEdit::multiline(&mut t.description).desired_rows(2).desired_width(f32::INFINITY));
    }
}

fn presets_column(ui: &mut egui::Ui, e: &mut PresetsEditor) {
    let Some(t) = e.list.tasks.get_mut(e.task) else {
        ui.weak("No tasks: add one with +.");
        return;
    };
    ui.label(RichText::new(format!("Presets in {}", t.name)).strong());
    ScrollArea::vertical().id_salt("edit_presets").max_height(390.0).auto_shrink([false, false]).show(ui, |ui| {
        let mut group = String::new();
        for (i, p) in t.presets.iter().enumerate() {
            if p.group != group {
                ui.add_space(4.0);
                ui.weak(&p.group);
                group = p.group.clone();
            }
            if ui.selectable_label(e.preset == Some(i), &p.name).clicked() {
                e.preset = Some(i);
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button(ph::PLUS).on_hover_text("new preset").clicked() {
            let name = unique_name(t.presets.iter().map(|p| p.name.as_str()), "New preset");
            // in the selected one's group, after it
            let at = e.preset.map_or(t.presets.len(), |i| i + 1);
            let group = e.preset.and_then(|i| t.presets.get(i)).map(|p| p.group.clone()).unwrap_or_default();
            t.presets.insert(at, PromptPreset { name, group, prompt: "<image1>".into(), ..Default::default() });
            e.preset = Some(at);
        }
        if let Some(i) = e.preset.filter(|i| *i < t.presets.len())
            && ui.button(ph::COPY).on_hover_text("duplicate this preset").clicked()
        {
            let mut p = t.presets[i].clone();
            p.name = unique_name(t.presets.iter().map(|p| p.name.as_str()), &format!("{} copy", p.name));
            p.favourite = false;
            t.presets.insert(i + 1, p);
            e.preset = Some(i + 1);
        }
        let n = t.presets.len();
        if let Some(i) = e.preset.filter(|i| *i < n)
            && delete_button(ui, Some(i), "preset")
        {
            t.presets.remove(i);
            e.preset = (!t.presets.is_empty()).then(|| i.min(t.presets.len() - 1));
        }
    });
}

fn fields_column(ui: &mut egui::Ui, e: &mut PresetsEditor) {
    let names: Vec<String> = e.list.tasks.iter().map(|t| t.name.clone()).collect();
    let Some(t) = e.list.tasks.get_mut(e.task) else { return };
    let groups: Vec<String> = {
        let mut g: Vec<String> = t.presets.iter().map(|p| p.group.clone()).filter(|g| !g.is_empty()).collect();
        g.dedup();
        g
    };
    let Some(i) = e.preset.filter(|i| *i < t.presets.len()) else {
        ui.weak("Select a preset to edit it.");
        return;
    };
    let mut move_to = None;
    let p = &mut t.presets[i];
    ui.set_width(ui.available_width());
    egui::Grid::new("preset_fields").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(f32::INFINITY));
        ui.end_row();
        ui.label("Group");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut p.group).hint_text("none").desired_width(200.0));
            if !groups.is_empty() {
                ComboBox::from_id_salt("preset_group").selected_text("existing…").show_ui(ui, |ui| {
                    for g in &groups {
                        if ui.selectable_label(*g == p.group, g).clicked() {
                            p.group = g.clone();
                        }
                    }
                });
            }
        });
        ui.end_row();
        ui.label("Task");
        ComboBox::from_id_salt("preset_task").selected_text(&names[e.task]).show_ui(ui, |ui| {
            for (j, n) in names.iter().enumerate() {
                if ui.selectable_label(j == e.task, n).clicked() && j != e.task {
                    move_to = Some(j);
                }
            }
        });
        ui.end_row();
    });
    ui.label("Description");
    ui.add(
        egui::TextEdit::multiline(&mut p.description)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("what the name means, loosely, and what you get"),
    );
    ui.label("Prompt");
    ui.add(egui::TextEdit::multiline(&mut p.prompt).desired_rows(6).desired_width(f32::INFINITY));
    let slots = increart_core::prompts::slots(&p.prompt);
    let need = needs_refs(&p.prompt);
    ui.weak(format!(
        "{} reference image(s){}",
        need,
        if slots.is_empty() {
            String::new()
        } else {
            let names: Vec<&str> = slots.iter().map(|r| &p.prompt[r.clone()]).collect();
            format!(" · to fill in: {}", names.join(", "))
        }
    ));
    ui.label("Negative prompt");
    ui.add(egui::TextEdit::singleline(&mut p.negative).desired_width(f32::INFINITY).hint_text("optional"));
    if let Some(j) = move_to {
        let p = t.presets.remove(i);
        e.list.tasks[j].presets.push(p);
        e.task = j;
        e.preset = Some(e.list.tasks[j].presets.len() - 1);
    }
}

/// `base`, or `base 2`, `base 3`… if taken.
fn unique_name<'a>(taken: impl Iterator<Item = &'a str> + Clone, base: &str) -> String {
    let taken = |n: &str| taken.clone().any(|t| t.eq_ignore_ascii_case(n));
    increart_core::config::first_free(|i| if i == 1 { base.to_string() } else { format!("{base} {i}") }, taken)
}

/// **Save as preset…**: name the Generate prompt and pick its task.
pub fn save_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let Some(d0) = &st.save_preset else { return };
    let mut d: SavePresetDialog = d0.clone();
    let tasks = &st.prompt_presets.tasks;
    let m = egui::Modal::new(egui::Id::unique("save_preset")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading(ic(ph::FLOPPY_DISK, "Save as preset"));
        ui.weak(short(&d.preset.prompt, 160));
        ui.add_space(6.0);
        let mut enter = false;
        egui::Grid::new("save_preset_fields").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Task");
            let current = tasks.get(d.task).map_or("?", |t| t.name.as_str());
            ComboBox::from_id_salt("save_preset_task").selected_text(current).show_ui(ui, |ui| {
                for (i, t) in tasks.iter().enumerate() {
                    ui.selectable_value(&mut d.task, i, &t.name);
                }
            });
            ui.end_row();
            ui.label("Name");
            let r = ui.add(egui::TextEdit::singleline(&mut d.preset.name).desired_width(f32::INFINITY));
            crate::widgets::focus_until_done(&r);
            enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.end_row();
            ui.label("Group");
            ui.add(egui::TextEdit::singleline(&mut d.preset.group).hint_text("none").desired_width(f32::INFINITY));
            ui.end_row();
        });
        ui.label("Description");
        ui.add(
            egui::TextEdit::multiline(&mut d.preset.description)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("what the name means, loosely, and what you get"),
        );
        if let Some(e) = &d.error {
            ui.colored_label(BAD, e);
        }
        ui.add_space(4.0);
        dialog_buttons(ui, |ui| {
            if ui.button(RichText::new(ic(ph::CHECK, "Save   Enter")).strong()).clicked() || enter {
                out.push(Msg::ConfirmSavePreset);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::CancelSavePreset);
            }
        });
    });
    if &d != d0 {
        d.error = None;
        // the edit lands before a Save sent in the same frame
        out.insert(0, Msg::EditSavePreset(d));
    }
    if m.should_close() {
        out.push(Msg::CancelSavePreset);
    }
}

/// Sweep mode's settings under the template: where the styles come from,
/// all or a random few, the `{slot}` values, and what Queue will make.
/// The Generate form's sweep: what it varies, then the styles (a prompt
/// preset per run) or the setting's values (DESIGN.md, Sweeps).
pub fn sweep_panel(ui: &mut egui::Ui, st: &AppState, f: &mut GenerateForm, out: &mut Vec<Msg>) {
    use increart_core::prompts::SweepVary;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(ic(ph::SQUARES_FOUR, "Sweep")).strong());
            ui.separator();
            ui.label("Vary");
            for v in SweepVary::EVERY {
                let tip = match v {
                    SweepVary::Style => "a run per chosen prompt preset",
                    SweepVary::Strength if f.init().is_none() => "needs a start image",
                    SweepVary::Model => "a run per model preset, each with its own steps and CFG",
                    _ => "a run per value, the rest as the form has it",
                };
                let ok = v != SweepVary::Strength || f.init().is_some();
                let r = ui.add_enabled(ok, egui::Button::selectable(f.sweep.vary == v, v.name()));
                if r.on_hover_text(tip).on_disabled_hover_text(tip).clicked() && f.sweep.vary != v {
                    f.sweep.vary = v;
                    f.sweep.values = increart_gui_core::update::sweep_defaults(v, f);
                    if v == SweepVary::Model && f.sweep.models.is_empty() {
                        f.sweep.models = st.config.recipes.get(f.preset).map(|p| p.name.clone()).into_iter().collect();
                    }
                }
            }
        });
        if f.sweep.vary == SweepVary::Style {
            style_sweep(ui, st, f, out);
        } else {
            setting_sweep(ui, st, f);
        }
    });
}

/// A setting sweep's values (a model sweep's model presets), the runs
/// they make and how long they take.
fn setting_sweep(ui: &mut egui::Ui, st: &AppState, f: &mut GenerateForm) {
    use increart_core::prompts::SweepVary;
    use increart_gui_core::update::{param_runs, range_values};
    if f.sweep.vary == SweepVary::Model {
        ui.horizontal_wrapped(|ui| {
            ui.label("Models");
            // those the servers run, and any still ticked
            let runnable = st.runnable_recipes();
            for (i, p) in st.config.recipes.iter().enumerate() {
                let mut on = f.sweep.models.contains(&p.name);
                if !on && !runnable.contains(&i) {
                    continue;
                }
                let tip = format!("{} steps · cfg {}", p.steps, p.cfg);
                if ui.checkbox(&mut on, &p.name).on_hover_text(tip).changed() {
                    if on {
                        f.sweep.models.push(p.name.clone());
                    } else {
                        f.sweep.models.retain(|m| *m != p.name);
                    }
                }
            }
        });
        ui.weak("Each model runs with its own preset's steps, CFG and flow shift.");
    } else {
        ui.horizontal(|ui| {
            ui.label("Values");
            ui.add(
                egui::TextEdit::singleline(&mut f.sweep.values).hint_text("e.g. 8, 12, 20, 30").desired_width(260.0),
            );
            // from … to … by …, kept for the session
            let id = egui::Id::unique(("sweep_range", f.sweep.vary.key()));
            let mut r: [f32; 3] = ui.data(|d| d.get_temp(id)).unwrap_or(match f.sweep.vary {
                SweepVary::Steps => [10.0, 40.0, 10.0],
                SweepVary::Cfg => [1.0, 5.0, 1.0],
                SweepVary::FlowShift => [1.0, 6.0, 1.0],
                _ => [0.3, 0.9, 0.15],
            });
            ui.separator();
            ui.label("from");
            ui.add(egui::DragValue::new(&mut r[0]).speed(0.1).max_decimals(2));
            ui.label("to");
            ui.add(egui::DragValue::new(&mut r[1]).speed(0.1).max_decimals(2));
            ui.label("by");
            ui.add(egui::DragValue::new(&mut r[2]).speed(0.05).range(0.01..=1000.0).max_decimals(2));
            if ui.button("Fill").on_hover_text("replace the values with this range").clicked() {
                f.sweep.values = range_values(r[0], r[1], r[2]);
            }
            ui.data_mut(|d| d.insert_temp(id, r));
        });
    }
    match param_runs(f, &st.config.recipes) {
        Err(e) => {
            ui.colored_label(BAD, e);
        }
        Ok(runs) => {
            let secs: f32 = runs.iter().map(|(r, _)| crate::projects::estimate(r) * r.seeds as f32).sum();
            let images = runs.len() as u64 * f.seeds;
            let line = format!(
                "{} {} × {} seeds = {images} images ≈ {} on one R9700 · same seeds for every value",
                runs.len(),
                if runs.len() == 1 { "value" } else { "values" },
                f.seeds,
                crate::projects::fmt_secs(secs)
            );
            if images > increart_gui_core::update::BIG_SWEEP {
                ui.colored_label(crate::widgets::NEUTRAL, format!("{line} (Queue asks first)"));
            } else {
                ui.label(line);
            }
            let rows: Vec<String> = runs.iter().filter_map(|(_, t)| t.param.as_ref().map(|p| p.label())).collect();
            ui.add(egui::Label::new(RichText::new(rows.join(" · ")).weak()).wrap());
            if f.sweep.vary == SweepVary::Cfg && runs.iter().any(|(r, _)| r.cfg <= 1.0) {
                ui.colored_label(
                    crate::widgets::NEUTRAL,
                    "At CFG 1 or below the negative prompt isn't used (and each step is about twice as fast).",
                );
            }
        }
    }
}

fn style_sweep(ui: &mut egui::Ui, st: &AppState, f: &mut GenerateForm, out: &mut Vec<Msg>) {
    use increart_core::prompts::{Removed, SweepSource, slot_names, sweep_presets};
    let list = &st.prompt_presets;
    let per = crate::projects::estimate(f);
    let seeds = f.seeds;
    let refs = f.images.iter().filter(|i| i.reference).count();
    let prompt = f.prompt.clone();
    let sw = &mut f.sweep;
    let base = sw.clone();
    {
        let count = |src: SweepSource| {
            sweep_presets(list, &Sweep { source: src, random: 0, removed: vec![], ..base.clone() }).len()
        };
        let name = |src: &SweepSource| match src {
            SweepSource::Selected => format!("{} ticked", base.selected.len()),
            SweepSource::Favourites => format!("★ Favourites ({})", count(SweepSource::Favourites)),
            SweepSource::Task(t) => format!("{t} ({})", count(src.clone())),
            SweepSource::Group(t, g) => format!("{t} → {g} ({})", count(src.clone())),
        };
        ui.horizontal(|ui| {
            ui.label("Styles");
            // the styles stay as they are while PE suggests for them
            if locked(st) {
                ui.disable();
            }
            let r = ui.button(ic(ph::SWATCHES, &format!("{}  ·  Choose…", name(&sw.source))));
            if r.on_hover_text("browse, search and tick the styles").clicked() {
                out.push(Msg::StylesOpen);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Use");
            if locked(st) {
                ui.disable();
            }
            let mut random = sw.random > 0;
            ui.radio_value(&mut random, false, "all of them");
            ui.radio_value(&mut random, true, "a random");
            let mut k = sw.random.max(1);
            ui.add_enabled(random, egui::DragValue::new(&mut k).range(1..=999));
            ui.label("of them");
            sw.random = if random { k } else { 0 };
            if random && ui.button(ic(ph::SHUFFLE, "Reroll")).on_hover_text("pick another random set").clicked() {
                sw.random_seed = (ui.input(|i| i.time) * 1e6) as u64 ^ sw.random_seed.rotate_left(17);
                sw.removed.clear();
            }
        });
        let chosen = sweep_presets(list, sw);
        // a preset not in the pick for a replaced one to take its place
        let spare = sw.random > 0 && sweep_presets(list, &Sweep { random: 0, ..sw.clone() }).len() > chosen.len();
        let mut remove: Option<Removed> = None;
        // the slots of the template and the chosen presets, asked once each,
        // with what needs them
        let names = slot_names(std::iter::once(prompt.as_str()).chain(sw.blank_texts(&chosen)));
        // styles' own values, from before their prompts could be edited,
        // become their prompts
        let keyed: Vec<_> = chosen
            .iter()
            .map(|(t, p)| (PresetKey { task: t.name.clone(), name: p.name.clone() }, p.prompt.as_str()))
            .collect();
        sw.fold_style_slots(&prompt, &keyed);
        if !names.is_empty() {
            let in_template = slot_names([prompt.as_str()]);
            pe_bar(ui, st, &names, sw, out);
            // while PE suggests, only its suggestions can be picked
            table_box(
                ui,
                "shared_values",
                &format!("Shared values ({} blanks)", names.len()),
                "one value per blank, in every run that has it (the template's and every style's)",
                |ui| slot_table(ui, st, &names, &in_template, &chosen, sw, out),
            );
            suggest_dialog(ui.ctx(), st, sw, out);
            batch_dialog(ui.ctx(), st, &names, out);
        }
        let fit: Vec<_> = chosen.iter().filter(|(_, p)| needs_refs(&p.prompt) <= refs).copied().collect();
        if !fit.is_empty() {
            ui.add_space(8.0);
            style_bar(ui, st, sw, out);
            table_box(
                ui,
                "style_prompts",
                &format!("Each style's prompt ({} styles)", fit.len()),
                "what each style's run is sent: built from the template, the style and the values, or improved by \
                 prompt enhance or by hand",
                |ui| remove = style_prompts(ui, st, &prompt, &fit, spare, sw, out),
            );
            if st.pe.style_dialog.is_some() {
                enhance_dialog(ui.ctx(), st, out);
            }
        }
        if !sw.removed.is_empty() {
            ui.horizontal_wrapped(|ui| {
                let (rep, om): (Vec<_>, Vec<_>) = sw.removed.iter().partition(|r| r.replace);
                let list = |v: &[&Removed]| v.iter().map(|r| r.preset.name.as_str()).collect::<Vec<_>>().join(", ");
                let mut parts = vec![];
                if !rep.is_empty() {
                    parts.push(format!("replaced: {}", list(&rep)));
                }
                if !om.is_empty() {
                    parts.push(format!("left out: {}", list(&om)));
                }
                ui.label(RichText::new(parts.join(" · ")).weak());
                if ui.small_button(ic(ph::ARROW_COUNTER_CLOCKWISE, "Put back")).clicked() {
                    sw.removed.clear();
                }
            });
        }
        if let Some(r) = remove {
            sw.removed.retain(|x| x.preset != r.preset);
            sw.removed.push(r);
            ui.ctx().request_repaint();
        }
        sw.slots.retain(|k, v| names.contains(k) || !v.is_empty());
        let (fit, unfit): (Vec<_>, Vec<_>) = chosen.iter().partition(|(_, p)| needs_refs(&p.prompt) <= refs);
        let names: Vec<&str> = fit.iter().map(|(_, p)| p.name.as_str()).collect();
        if names.is_empty() {
            ui.colored_label(BAD, "No styles chosen.");
        } else {
            let images = names.len() as u64 * seeds;
            let secs = per * images as f32;
            let line = format!(
                "{} styles × {} seeds = {images} images ≈ {} on one R9700 · same seeds in every style",
                names.len(),
                seeds,
                crate::projects::fmt_secs(secs)
            );
            if images > increart_gui_core::update::BIG_SWEEP {
                ui.colored_label(crate::widgets::NEUTRAL, format!("{line} (Queue asks first)"));
            } else {
                ui.label(line);
            }
            ui.add(egui::Label::new(RichText::new(names.join(" · ")).weak()).wrap());
        }
        if !unfit.is_empty() {
            ui.colored_label(
                crate::widgets::NEUTRAL,
                format!("{} chosen preset(s) left out: they need more than {refs} reference image(s).", unfit.len()),
            );
        }
    }
}

/// The prompt's blanks, each with a value (or prompt enhance's
/// suggestions), for a single run or a setting sweep: filled in when
/// queued.
pub fn prompt_blanks(ui: &mut egui::Ui, st: &AppState, f: &mut GenerateForm, out: &mut Vec<Msg>) {
    use increart_core::prompts::slot_names;
    let names = slot_names([f.prompt.as_str()]);
    if names.is_empty() {
        return;
    }
    let sw = &mut f.sweep;
    pe_bar(ui, st, &names, sw, out);
    table_box(
        ui,
        "prompt_blanks",
        &format!("Blanks ({})", names.len()),
        "a value for each of the prompt's blanks, filled in when queued (or type over it in the prompt)",
        |ui| slot_table(ui, st, &names, &names, &[], sw, out),
    );
    suggest_dialog(ui.ctx(), st, sw, out);
    batch_dialog(ui.ctx(), st, &names, out);
    let empty: Vec<String> = names
        .iter()
        .filter(|n| sw.slots.get(*n).is_none_or(|v| v.trim().is_empty()))
        .map(|n| format!("{{{n}}}"))
        .collect();
    if !empty.is_empty() {
        ui.colored_label(crate::widgets::NEUTRAL, format!("Fill in {} before queueing.", empty.join(", ")));
    }
}

/// A table in a box of its own, with its title and what it's for; the
/// title's arrow folds it up (for the session, by `id`).
fn table_box(ui: &mut egui::Ui, id: &str, title: &str, what: &str, add: impl FnOnce(&mut egui::Ui)) {
    let key = egui::Id::unique(("table_box_folded", id));
    let folded: bool = ui.ctx().data(|d| d.get_temp(key)).unwrap_or(false);
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            let arrow = if folded { ph::CARET_RIGHT } else { ph::CARET_DOWN };
            let r = ui
                .add(egui::Button::new(RichText::new(format!("{arrow} {title}")).strong()).frame(false))
                .on_hover_text(if folded { "show it" } else { "fold it up" });
            if r.clicked() {
                ui.ctx().data_mut(|d| d.insert_temp(key, !folded));
            }
            ui.weak(what);
        });
        if !folded {
            ui.separator();
            add(ui);
        }
    });
}

/// Text of a table cell, cut to its width (the whole of it on hover).
fn cell_text(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(egui::Label::new(text).truncate())
}

/// Above the blanks: Clear all, Random picks (of the values offered),
/// prompt enhance's state, and Suggest for every empty blank.
fn pe_bar(ui: &mut egui::Ui, st: &AppState, names: &[String], sw: &mut Sweep, out: &mut Vec<Msg>) {
    use increart_gui_core::{BlankKey, PeStatus};
    let busy = st.pe.blanks_waiting() > 0;
    ui.horizontal_wrapped(|ui| {
        let filled = sw.slots.values().any(|v| !v.trim().is_empty()) || !sw.style_slots.is_empty();
        if ui
            .add_enabled(filled && !busy, egui::Button::new(ic(ph::ERASER, "Clear values")))
            .on_hover_text("empty every value: the shared ones and the styles' own (the suggestions stay)")
            .clicked()
        {
            sw.slots.clear();
            sw.style_slots.clear();
        }
        // every blank with values offered
        let mut offered: Vec<(BlankKey, &Vec<String>)> = vec![];
        for n in names {
            let k = BlankKey { style: None, slot: n.clone() };
            if let Some(vs) = st.blank_shown(&k) {
                offered.push((k, vs));
            }
        }
        if ui
            .add_enabled(!offered.is_empty() && !busy, egui::Button::new(ic(ph::BROOM, "Clear suggestions")))
            .on_hover_text("every blank's suggestion chips (the values stay; asks still avoid them)")
            .clicked()
        {
            out.extend(offered.iter().map(|(k, _)| Msg::PeClearSuggestions(k.clone())));
        }
        if ui
            .add_enabled(!offered.is_empty() && !busy, egui::Button::new(ic(ph::SHUFFLE, "Random picks")))
            .on_hover_text("each blank with suggestions gets one of them at random (again for others)")
            .clicked()
        {
            let mut seed = (ui.input(|i| i.time) * 1e6) as u64 | 1;
            for (k, vs) in offered.iter().filter(|(_, vs)| !vs.is_empty()) {
                // xorshift
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let v = vs[(seed % vs.len() as u64) as usize].clone();
                match &k.style {
                    None => _ = sw.slots.insert(k.slot.clone(), v),
                    Some(style) => sw.set_own_slot(style, &k.slot, v),
                }
            }
        }
        ui.separator();
        let Some(server) = st.pe.server.as_ref() else {
            ui.weak(ic(ph::MAGIC_WAND, "Prompt enhance can suggest values:"));
            if ui
                .small_button("Servers…")
                .on_hover_text("add a chat server to a server preset, or pick one to continue without sd-servers")
                .clicked()
            {
                out.push(Msg::OpenServerDialog);
            }
            return;
        };
        // the shared blanks short of a full set of suggestions
        let count = increart_core::prompts::SUGGEST_COUNT;
        let [few, ..] = st.batch_picks(names, count);
        let few: Vec<_> = few.into_iter().map(|slot| BlankKey { style: None, slot }).collect();
        let tip = format!(
            "ask the PE server for more values for every shared blank with fewer than {count} suggestions, \
             with the default settings{}",
            if server.vision() { "; it looks at the reference images" } else { "" }
        );
        let b = egui::Button::new(ic(ph::MAGIC_WAND, &format!("Basic suggest ({})", few.len())));
        if ui.add_enabled(!few.is_empty() && !busy, b).on_hover_text(tip).clicked() {
            out.push(Msg::PeSuggest(few.clone()));
        }
        let b = egui::Button::new(ph::SLIDERS_HORIZONTAL);
        let tip = "Suggest for blanks…: pick the blanks, with instructions, how many, how varied (kept with \
                   the project)";
        if ui.add_enabled(!busy, b).on_hover_text(tip).clicked() {
            out.push(Msg::PeBatchSuggestOpen(few.into_iter().map(|k| k.slot).collect()));
        }
        // only while busy, or failed: idle (off or ready) is in the top bar
        let status = match &st.pe.status {
            PeStatus::Off | PeStatus::Ready => None,
            PeStatus::Starting => Some((format!("PE {}: {}", server.name, crate::rewrite::starting(st)), None)),
            PeStatus::Failed(e) => Some((format!("PE {}: failed: {e}", server.name), Some(BAD))),
        };
        let blanks: Vec<_> =
            st.pe.jobs.iter().filter(|j| matches!(j.key, increart_gui_core::PeKey::Blank(_))).collect();
        // Stop before the text, which changes as the ask goes, so it stays put
        if !blanks.is_empty() {
            ui.spinner();
            if ui.small_button(ic(ph::STOP, "Stop")).on_hover_text("stop asking for these blanks").clicked() {
                out.push(Msg::PeStop(blanks.iter().map(|j| j.id).collect()));
            }
        }
        let status = match blanks.first() {
            Some(j) => {
                let total = st.pe.blank_batch.max(blanks.len());
                let n = total - blanks.len() + 1;
                let now = crate::rewrite::progress(st, j);
                let again = match &j.key {
                    increart_gui_core::PeKey::Blank(k) => retry_text(st, k),
                    _ => String::new(),
                };
                let text = format!(
                    "Suggesting {n} of {total} · {now}{again} · until it's done, only suggestions can be picked"
                );
                Some((text, None))
            }
            None => status,
        };
        match status {
            Some((text, Some(c))) => _ = ui.colored_label(c, text),
            Some((text, None)) => _ = ui.weak(text),
            None => {}
        }
    });
}

/// The ✨ button for a blank (a spinner while asked), then the values PE
/// offered for it: a click on one puts it in `value`.
fn suggest_cells(
    row: &mut egui_extras::TableRow,
    st: &AppState,
    key: increart_gui_core::BlankKey,
    value: &mut String,
    out: &mut Vec<Msg>,
) {
    let offered = st.blank_shown(&key);
    row.col(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if let Some(j) = st.pe.blank_job(&key) {
            let tip = format!("{}{}\nclick to stop", crate::rewrite::progress(st, j), retry_text(st, &key));
            if ui.add(egui::Spinner::new()).interact(egui::Sense::click()).on_hover_text(tip).clicked() {
                out.push(Msg::PeStop(vec![j.id]));
            }
        } else if st.pe.server.as_ref().is_some() {
            let tip = if offered.is_some() {
                "suggest more values, added to these (none offered before)"
            } else {
                "suggest values (prompt enhance)"
            };
            let b = ui.add_enabled(!locked(st), egui::Button::new(ph::MAGIC_WAND).small());
            if b.on_hover_text(tip).clicked() {
                out.push(Msg::PeSuggest(vec![key.clone()]));
            }
            if offered.is_some() {
                let b = ui.add_enabled(!locked(st), egui::Button::new(ph::DICE_FIVE).small());
                if b.on_hover_text("reroll: new values in place of these (none offered before)").clicked() {
                    out.push(Msg::PeReroll(key.clone()));
                }
            }
            let b = ui.add_enabled(!locked(st), egui::Button::new(ph::SLIDERS_HORIZONTAL).small());
            if b.on_hover_text("Suggest…: with instructions, how many, how varied").clicked() {
                out.push(Msg::PeSuggestOpen(key.clone()));
            }
        }
        if offered.is_some() {
            let b = ui.add_enabled(!locked(st), egui::Button::new(ph::BROOM).small());
            if b.on_hover_text("clear these suggestion chips (the value stays; asks still avoid them)").clicked() {
                out.push(Msg::PeClearSuggestions(key.clone()));
            }
        }
        if let Some(why) = st.pe.blank_failed.get(&key) {
            ui.colored_label(BAD, ph::WARNING)
                .on_hover_text(format!("the last ask got nothing new: {why}\n(gone when asked again)"));
        }
        // every value offered, when the cell is too narrow to show them
        if let Some(vs) = offered {
            ui.menu_button(RichText::new(format!("{} ▾", vs.len())).small(), |ui| {
                for v in vs {
                    if ui.selectable_label(value.trim() == v, v).clicked() {
                        *value = v.clone();
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("the values offered");
        }
    });
    row.col(|ui| {
        ui.spacing_mut().item_spacing = CHIP_SPACING;
        let id = chips_height_id(&key);
        // centred in the row: its height is known from the frame before
        let known = ui.ctx().data(|d| d.get_temp::<f32>(id)).filter(|h| *h > 0.0);
        let avail = ui.available_height();
        let chips = ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            if let Some(h) = known {
                ui.add_space(((avail - h) / 2.0).max(0.0));
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true), |ui| {
                for v in offered.into_iter().flatten() {
                    let on = value.trim() == v;
                    let (label, long) = chip_label(v);
                    let tip = if long { format!("use this: {v}") } else { "use this".into() };
                    // a bubble, filled when it's the value
                    let chip = egui::Button::new(label).small().selected(on).corner_radius(8.0);
                    if ui.add(chip).on_hover_text(tip).clicked() {
                        *value = v.clone();
                    }
                }
            })
            .response
            .rect
            .height()
        });
        // the height they took, for the row's height (and centring) next frame
        let h = if offered.is_some_and(|v| !v.is_empty()) { chips.inner } else { 0.0 };
        if known.unwrap_or(0.0) != h {
            ui.ctx().data_mut(|d| d.insert_temp(id, h));
            ui.ctx().request_repaint();
        }
    });
}

/// **Suggest…** for one blank: instructions, how many, how varied, whether
/// it sees the images and avoids the values offered before; the values
/// offered, to pick from, and the message the ask sends.
fn suggest_dialog(ctx: &egui::Context, st: &AppState, sw: &mut Sweep, out: &mut Vec<Msg>) {
    let (Some(d), Some(server)) = (&st.pe.suggest, st.pe.server.as_ref()) else { return };
    let mut d = d.clone();
    let key = d.key.clone();
    let job = st.pe.blank_job(&key);
    let mut value = match &key.style {
        None => sw.slots.get(&key.slot).cloned().unwrap_or_default(),
        Some(style) => sw.own_slot(style, &key.slot).to_string(),
    };
    let mut close = false;
    let m = egui::Modal::new(egui::Id::unique("pe_suggest")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading(ic(ph::MAGIC_WAND, &format!("Suggest values for {{{}}}", key.slot)));
        match &key.style {
            Some(s) => ui.weak(format!("{}'s own value, for the {} style only", s.name, s.name)),
            None => ui.weak("the shared value, for every run that has it"),
        };
        ui.add_space(6.0);
        form_grid(ui, st, server, "pe_suggest_grid", &mut d.form, Some(&key), out);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if let Some(j) = job {
                ui.spinner();
                if ui.button(ic(ph::STOP, "Stop")).clicked() {
                    out.push(Msg::PeStop(vec![j.id]));
                }
                ui.label(crate::rewrite::progress(st, j));
            } else {
                let b = egui::Button::new(RichText::new(ic(ph::MAGIC_WAND, "Suggest")).strong());
                // other blanks being asked for wait their turn
                if ui.add(b).on_hover_text(format!("ask {}", server.name)).clicked() {
                    out.push(Msg::PeSuggestDialog(Some(d.clone())));
                    out.push(Msg::PeSuggestAsk);
                }
            }
        });
        ui.add_space(4.0);
        ui.separator();
        let offered = st.blank_shown(&key).filter(|v| !v.is_empty());
        ui.horizontal(|ui| {
            ui.label(RichText::new("Values offered").strong());
            if offered.is_some()
                && ui.small_button("Clear").on_hover_text("those shown; asks still avoid them").clicked()
            {
                out.push(Msg::PeClearSuggestions(key.clone()));
            }
        });
        egui::ScrollArea::vertical().id_salt("pe_suggest_values").max_height(200.0).show(ui, |ui| {
            ui.spacing_mut().item_spacing = CHIP_SPACING;
            ui.horizontal_wrapped(|ui| match offered {
                None => _ = ui.weak("none yet"),
                Some(vs) => {
                    for v in vs {
                        let chip = egui::Button::new(v.as_str()).small().selected(value.trim() == v).corner_radius(8.0);
                        if ui.add(chip).on_hover_text("use this").clicked() {
                            value = v.clone();
                        }
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label("Value");
            ui.add(egui::TextEdit::singleline(&mut value).desired_width(f32::INFINITY));
        });
        egui::CollapsingHeader::new("The message sent").id_salt("pe_suggest_message").show(ui, |ui| {
            let text =
                increart_gui_core::suggest_message(st, &d.key, &d.form).unwrap_or_else(|| "(the blank is gone)".into());
            egui::ScrollArea::vertical().id_salt("pe_suggest_text").max_height(160.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).monospace().small()).wrap());
            });
            ui.weak("the system prompt goes first: see Settings → Prompt enhance");
        });
        ui.separator();
        dialog_buttons(ui, |ui| {
            close = ui.button("Close   Esc").clicked();
        });
    });
    let old = match &key.style {
        None => sw.slots.get(&key.slot).cloned().unwrap_or_default(),
        Some(style) => sw.own_slot(style, &key.slot).to_string(),
    };
    if value != old {
        match &key.style {
            None => _ = sw.slots.insert(key.slot.clone(), value),
            Some(style) => sw.set_own_slot(style, &key.slot, value),
        }
    }
    if close || m.should_close() {
        out.push(Msg::PeSuggestDialog(None));
    } else if Some(&d) != st.pe.suggest.as_ref() {
        out.push(Msg::PeSuggestDialog(Some(d)));
    }
}

/// **Suggest for blanks…**: the shared blanks to ask for (ticked), the
/// settings each ask gets, and the first one's message.
fn batch_dialog(ctx: &egui::Context, st: &AppState, names: &[String], out: &mut Vec<Msg>) {
    use increart_gui_core::BlankKey;
    let (Some(b), Some(server)) = (&st.pe.batch_suggest, st.pe.server.as_ref()) else { return };
    let mut b = b.clone();
    let (mut ask, mut close) = (false, false);
    let ticked = |b: &increart_gui_core::BatchSuggest| -> Vec<String> {
        names.iter().filter(|n| b.blanks.contains(n)).cloned().collect()
    };
    let m = egui::Modal::new(egui::Id::unique("pe_batch_suggest")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading(ic(ph::MAGIC_WAND, "Suggest values for blanks"));
        ui.weak("one ask for each blank ticked, each with these settings (kept with the project)");
        ui.add_space(6.0);
        let value = |n: &String| st.generate.sweep.slots.get(n).map_or("", |v| v.trim());
        let key = |n: &String| BlankKey { style: None, slot: n.clone() };
        let chips = |n: &String| st.blank_shown(&key(n)).map_or(0, |v| v.len());
        // each ticks exactly those it names, unticking the rest
        let want = b.form.count;
        let [fewer, empty, failed] = st.batch_picks(names, want);
        let picks: [(String, String, Vec<String>); 5] = [
            ("All".into(), "every blank".into(), names.to_vec()),
            ("None".into(), "no blank".into(), vec![]),
            (
                format!("Fewer than {want} suggestions"),
                format!("those with 0 to {} suggestions showing (How many: {want})", want.saturating_sub(1)),
                fewer,
            ),
            ("No value".into(), "those whose value is empty, suggestions or not".into(), empty),
            (format!("Failed {}", ph::WARNING), "those whose last ask got nothing new".into(), failed),
        ];
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Tick only").strong());
            for (label, tip, set) in picks {
                let (n, none) = (set.len(), label == "None");
                let text = if none { label } else { format!("{label} ({n})") };
                let r = ui.add_enabled(none || n > 0, egui::Button::new(text).small());
                if r.on_hover_text(format!("tick {tip}; untick the rest")).clicked() {
                    b.blanks = set;
                }
            }
        });
        ui.add_space(2.0);
        // one blank a row: what it has can't be read as its neighbour's
        egui::ScrollArea::vertical().id_salt("pe_batch_blanks").max_height(220.0).show(ui, |ui| {
            egui::Grid::new("pe_batch_blanks_grid").num_columns(3).striped(true).spacing([16.0, 2.0]).show(ui, |ui| {
                ui.weak("Blank");
                ui.weak("Suggestions");
                ui.weak("Value");
                ui.end_row();
                for n in names {
                    let mut on = b.blanks.contains(n);
                    if ui.checkbox(&mut on, RichText::new(format!("{{{n}}}")).monospace()).changed() {
                        b.blanks.retain(|b| b != n);
                        if on {
                            b.blanks.push(n.clone());
                        }
                    }
                    ui.horizontal(|ui| {
                        let c = chips(n);
                        let text = format!("{c}");
                        if c < want {
                            ui.colored_label(crate::widgets::NEUTRAL, text)
                        } else {
                            ui.label(text)
                        };
                        if let Some(why) = st.pe.blank_failed.get(&key(n)) {
                            ui.colored_label(BAD, ph::WARNING)
                                .on_hover_text(format!("the last ask got nothing new: {why}"));
                        }
                    });
                    match value(n) {
                        "" => ui.weak("—"),
                        v if v.chars().count() > 40 => {
                            let cut: String = v.chars().take(39).collect();
                            ui.label(format!("{cut}…")).on_hover_text(v)
                        }
                        v => ui.label(v),
                    };
                    ui.end_row();
                }
            });
        });
        ui.add_space(4.0);
        ui.separator();
        form_grid(ui, st, server, "pe_batch_grid", &mut b.form, None, out);
        ui.add_space(4.0);
        let asked = ticked(&b);
        dialog_buttons(ui, |ui| {
            let busy = st.pe.blanks_waiting() > 0;
            let label = RichText::new(ic(ph::MAGIC_WAND, &format!("Suggest ({})", asked.len()))).strong();
            let tip = if busy { "wait for the asks going on" } else { "ask for each blank ticked" };
            let r = ui.add_enabled(!asked.is_empty() && !busy, egui::Button::new(label));
            ask = r.on_hover_text(format!("{tip} ({})", server.name)).on_disabled_hover_text(tip).clicked();
            close = ui.button("Close   Esc").clicked();
        });
        egui::CollapsingHeader::new("The message sent (the first blank's)").id_salt("pe_batch_message").show(
            ui,
            |ui| {
                let first = asked.first().map(|slot| BlankKey { style: None, slot: slot.clone() });
                let text = first
                    .and_then(|k| increart_gui_core::suggest_message(st, &k, &b.form))
                    .unwrap_or_else(|| "(no blank ticked)".into());
                egui::ScrollArea::vertical().id_salt("pe_batch_text").max_height(160.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(text).monospace().small()).wrap());
                });
                ui.weak("each blank's names that blank; the system prompt goes first: see Settings → Prompt enhance");
            },
        );
    });
    if Some(&b) != st.pe.batch_suggest.as_ref() {
        out.push(Msg::PeBatchSuggestDialog(Some(b)));
    }
    if ask {
        out.push(Msg::PeBatchSuggestAsk);
    } else if close || m.should_close() {
        out.push(Msg::PeBatchSuggestDialog(None));
    }
}

/// A Suggest dialog's settings: instructions, how many, how varied, whether
/// the asks see the images and avoid the values offered before (`key`'s,
/// else each blank's), and whether they add to them or replace them.
fn form_grid(
    ui: &mut egui::Ui,
    st: &AppState,
    server: &increart_core::pe::PeServer,
    id: &str,
    form: &mut increart_gui_core::SuggestForm,
    key: Option<&increart_gui_core::BlankKey>,
    out: &mut Vec<Msg>,
) {
    use increart_core::pe::FILL_BLANKS;
    egui::Grid::new(id).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Instructions");
        ui.add(
            egui::TextEdit::singleline(&mut form.how)
                .hint_text("optional: e.g. only stones, stranger, from nature, 1920s")
                .desired_width(f32::INFINITY),
        );
        ui.end_row();
        ui.label("How many");
        ui.add(egui::DragValue::new(&mut form.count).range(1..=30));
        ui.end_row();
        ui.label("Temperature");
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut form.sampling.temperature, 0.0..=2.0).step_by(0.05))
                .on_hover_text("hotter: more varied and surprising, but less on topic");
            if ui.small_button("Default").on_hover_text("the server's, as set in Settings").clicked() {
                form.sampling = st.config.pe.sampling(FILL_BLANKS);
            }
        });
        ui.end_row();
        ui.label("min_p");
        ui.add(egui::Slider::new(&mut form.sampling.min_p, 0.0..=0.5).step_by(0.01)).on_hover_text(
            "drops words less than this fraction as likely as the likeliest: keeps a hot answer \
             sensible (0: off)",
        );
        ui.end_row();
        ui.label("");
        ui.vertical(|ui| {
            let refs = st.generate.refs().len() + st.generate.init().is_some() as usize;
            let can = server.vision() && refs > 0;
            let tip = if !server.vision() {
                "this server has no vision"
            } else if refs == 0 {
                "there are no reference or start images"
            } else {
                "off: values for the template and styles alone, not matched to the images"
            };
            let mut images = form.images && can;
            ui.add_enabled(can, egui::Checkbox::new(&mut images, "Look at the images"))
                .on_hover_text(tip)
                .on_disabled_hover_text(tip);
            if can {
                form.images = images;
            }
            match key {
                Some(key) => {
                    let before = st.blank_offered(key).len();
                    ui.horizontal(|ui| {
                        ui.add_enabled(
                            before > 0,
                            egui::Checkbox::new(&mut form.avoid, format!("Not the {before} values offered before")),
                        );
                        if before > 0
                            && ui
                                .small_button("Forget them")
                                .on_hover_text("asks may offer them again; those shown stay")
                                .clicked()
                        {
                            out.push(Msg::PeForgetOffered(key.clone()));
                        }
                    });
                }
                None => _ = ui.checkbox(&mut form.avoid, "Not the values offered before (each blank's)"),
            }
            ui.horizontal(|ui| {
                ui.radio_value(&mut form.replace, false, "Add to the values offered");
                ui.radio_value(&mut form.replace, true, "Replace them");
            });
        });
        ui.end_row();
    });
}

/// " · retry 1 of 2" while a blank is asked again after an answer with
/// nothing new.
fn retry_text(st: &AppState, key: &increart_gui_core::BlankKey) -> String {
    match st.pe.blank_retry(key) {
        0 => String::new(),
        r => format!(" · retry {r} of {}", increart_gui_core::BLANK_RETRIES),
    }
}

/// While PE suggests values for blanks, the tables are locked but for
/// picking the values offered.
fn locked(st: &AppState) -> bool {
    st.pe.blanks_waiting() > 0
}

const CHIP_SPACING: egui::Vec2 = egui::vec2(4.0, 3.0);

/// A suggestion as its chip shows it: at most 6 words and 36 characters
/// (and whether it was cut).
fn chip_label(v: &str) -> (String, bool) {
    let words: Vec<&str> = v.split_whitespace().collect();
    let mut s = words[..words.len().min(6)].join(" ");
    if s.chars().count() > 36 {
        s = s.chars().take(35).collect::<String>().trim_end().to_string();
    }
    let long = s != v.trim();
    if long {
        s.push('…');
    }
    (s, long)
}

fn chips_height_id(key: &increart_gui_core::BlankKey) -> egui::Id {
    egui::Id::unique(("pe_chips_height", key))
}

/// How tall a row is with these suggestions wrapped in `width`: as they
/// were last drawn, else as estimated.
fn chips_height(ui: &egui::Ui, key: &increart_gui_core::BlankKey, offered: Option<&Vec<String>>, width: f32) -> f32 {
    let min = 26.0;
    let Some(vs) = offered.filter(|v| !v.is_empty()) else { return min };
    if let Some(h) = ui.ctx().data(|d| d.get_temp::<f32>(chips_height_id(key))).filter(|h| *h > 0.0) {
        return (h + 6.0).max(min);
    }
    let font = egui::TextStyle::Small.resolve(ui.style());
    let pad = ui.spacing().button_padding;
    let chip_h = ui.fonts_mut(|f| f.row_height(&font)) + 2.0 * pad.y;
    let (mut x, mut lines) = (0.0, 1);
    for v in vs {
        let w = ui.fonts_mut(|f| f.layout_no_wrap(chip_label(v).0, font.clone(), egui::Color32::WHITE).size().x)
            + 2.0 * pad.x;
        if x > 0.0 && x + w > width {
            (x, lines) = (0.0, lines + 1);
        }
        x += w + CHIP_SPACING.x;
    }
    (lines as f32 * chip_h + (lines - 1) as f32 * CHIP_SPACING.y + 6.0).max(min)
}

/// The sweep's blanks: a row per `{slot}` of the template and the chosen
/// styles, its value (the same in every run unless a style has its own),
/// PE's suggestions for it and what needs it.
fn slot_table(
    ui: &mut egui::Ui,
    st: &AppState,
    names: &[String],
    in_template: &[String],
    chosen: &[(&PromptTask, &PromptPreset)],
    sw: &mut Sweep,
    out: &mut Vec<Msg>,
) {
    use egui_extras::{Column, TableBuilder};
    use increart_core::prompts::needing_slot;
    TableBuilder::new(ui)
        .id_salt("sweep_slots_v2")
        .striped(true)
        .vscroll(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(90.0))
        .column(Column::initial(170.0).at_least(110.0).clip(true))
        .column(Column::auto())
        .column(Column::remainder().at_least(160.0).clip(true))
        .column(Column::initial(100.0).at_least(70.0).clip(true))
        .header(20.0, |mut h| {
            for title in ["Blank", "Value, in every run", "", "Suggestions (click to use)", "Needed by"] {
                h.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|mut body| {
            let width = body.widths()[3];
            for n in names {
                let key = increart_gui_core::BlankKey { style: None, slot: n.clone() };
                let h = chips_height(body.ui_mut(), &key, st.blank_shown(&key), width);
                body.row(h, |mut row| {
                    row.col(|ui| {
                        ui.monospace(format!("{{{n}}}"));
                    });
                    let v = sw.slots.entry(n.clone()).or_default();
                    row.col(|ui| {
                        let empty = v.trim().is_empty();
                        // right to left: the clear button (always there, so the field keeps its
                        // width), then the field takes what's left
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let b = ui.add_enabled(!empty && !locked(st), egui::Button::new(ph::X).small());
                            if b.on_hover_text("clear this value (the suggestions stay)").clicked() {
                                v.clear();
                            }
                            let edit = egui::TextEdit::singleline(v)
                                .desired_width(ui.available_width())
                                .hint_text(RichText::new("fill in").color(crate::widgets::NEUTRAL));
                            let r = ui.add_enabled(!locked(st), edit);
                            if empty && !r.has_focus() {
                                ui.painter().rect_stroke(
                                    r.rect,
                                    2.0,
                                    egui::Stroke::new(1.0, crate::widgets::NEUTRAL),
                                    egui::StrokeKind::Inside,
                                );
                            }
                        });
                    });
                    suggest_cells(&mut row, st, increart_gui_core::BlankKey { style: None, slot: n.clone() }, v, out);
                    row.col(|ui| {
                        let need = needing_slot(sw, chosen, n);
                        let mut parts = vec![];
                        if in_template.contains(n) {
                            // (no styles: a single run's prompt)
                            parts.push(if chosen.is_empty() { "the prompt" } else { "the template" }.to_string());
                        }
                        match need.len() {
                            0 => {}
                            1 => parts.push(need[0].1.name.clone()),
                            k => parts.push(format!("{k} styles")),
                        }
                        let tip = need.iter().map(|(_, p)| p.name.as_str()).collect::<Vec<_>>().join("\n");
                        // one style of several: its value sets nothing for the rest
                        if need.len() == 1 && parts.len() == 1 && chosen.len() > 1 {
                            cell_text(
                                ui,
                                RichText::new(format!("only {}", need[0].1.name)).color(crate::widgets::NEUTRAL),
                            )
                            .on_hover_text(format!(
                                "only {}'s prompt has {{{n}}}: the other {} styles don't use its value. To set a \
                                     scene or era for every style, put it in the template.",
                                need[0].1.name,
                                chosen.len() - 1
                            ));
                            return;
                        }
                        let r = cell_text(ui, RichText::new(parts.join(", ")).weak());
                        if need.len() > 1 {
                            r.on_hover_text(tip);
                        }
                    });
                });
            }
        });
}

/// Above the styles' prompts: **Enhance style prompts** for those the
/// settings take, its instructions, **Enhance…** for the rest of the
/// settings, back to as built, and the asks' progress.
fn style_bar(ui: &mut egui::Ui, st: &AppState, sw: &mut Sweep, out: &mut Vec<Msg>) {
    use increart_gui_core::PeKey;
    let styles: Vec<_> = st.pe.jobs.iter().filter(|j| matches!(j.key, PeKey::Style(_))).collect();
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                !sw.style_prompts.is_empty(),
                egui::Button::new(ic(ph::ARROW_COUNTER_CLOCKWISE, "All back to as built")),
            )
            .on_hover_text(
                "drop every style's enhanced or edited prompt: each is built from the template and values again",
            )
            .clicked()
        {
            sw.style_prompts.clear();
        }
        ui.separator();
        if st.pe.server.as_ref().is_some() {
            let n = increart_gui_core::update::styles_to_enhance(st).len();
            let b = egui::Button::new(ic(ph::MAGIC_WAND, &format!("Basic enhance ({n})")));
            let tip = "prompt enhance improves each style's prompt not enhanced yet (or enhanced from older values), \
                       filling any blank left, with the default settings; your edits are left as they are";
            if ui.add_enabled(n > 0 && styles.is_empty(), b).on_hover_text(tip).clicked() {
                out.push(Msg::PeStyleEnhance(vec![]));
            }
            let tip = "Enhance style prompts…: pick the styles, with instructions, from what, images, sampling, \
                       length, the system prompt and the message sent (kept with the project)";
            if ui.add_enabled(styles.is_empty(), egui::Button::new(ph::SLIDERS_HORIZONTAL)).on_hover_text(tip).clicked()
            {
                out.push(Msg::PeStyleOpen);
            }
        } else {
            ui.weak(ic(ph::MAGIC_WAND, "Prompt enhance can improve these:"));
            if ui
                .small_button("Servers…")
                .on_hover_text("add a chat server to a server preset, or pick one to continue without sd-servers")
                .clicked()
            {
                out.push(Msg::OpenServerDialog);
            }
        }
        if let Some(j) = styles.first() {
            ui.spinner();
            let total = st.pe.style_batch.max(styles.len());
            let n = total - styles.len() + 1;
            if ui.small_button(ic(ph::STOP, "Stop")).on_hover_text("stop improving these styles' prompts").clicked() {
                out.push(Msg::PeStop(styles.iter().map(|j| j.id).collect()));
            }
            ui.weak(format!("Enhancing {n} of {total} · {}", crate::rewrite::progress(st, j)));
        }
    });
}

/// The styles that will run, a row each: the prompt its run gets (built
/// from the template, the style and the values, or its own: enhanced or
/// edited), where it is, ✨ to improve it, back to as built, and Replace /
/// Omit to drop the style.
fn style_prompts(
    ui: &mut egui::Ui,
    st: &AppState,
    template: &str,
    styles: &[(&PromptTask, &PromptPreset)],
    spare: bool,
    sw: &mut Sweep,
    out: &mut Vec<Msg>,
) -> Option<increart_core::prompts::Removed> {
    use increart_core::prompts::{Removed, StylePrompt, sweep_prompt};
    use increart_gui_core::update::StyleRow;
    let mut remove = None;
    let can_ask = st.pe.server.as_ref().is_some();
    // in the form, not a box of its own that scrolls: the form scrolls
    for (i, (t, p)) in styles.iter().enumerate() {
        let key = PresetKey { task: t.name.clone(), name: p.name.clone() };
        // from the form as it is this frame
        let built = sweep_prompt(template, &p.prompt, &sw.slots_for(&key));
        let own = sw.style_prompt(&key).cloned();
        let row = match &own {
            None => StyleRow::Built,
            Some(o) if o.enhanced => StyleRow::Enhanced { stale: o.from != built },
            Some(o) => StyleRow::Edited { stale: o.from != built },
        };
        let waiting = st.pe.style_job(&key);
        if i > 0 {
            ui.add_space(2.0);
            ui.separator();
        }
        // the name, where it is and the buttons; the prompt under them, the
        // whole width
        ui.horizontal(|ui| {
            let g = if p.group.is_empty() { &t.name } else { &p.group };
            ui.label(RichText::new(&p.name).strong()).on_hover_text(format!("{g}\n\n{}", p.prompt));
            ui.weak(g);
            let (label, color) = match row {
                StyleRow::Built => ("as built".to_string(), None),
                StyleRow::Enhanced { stale: false } => (ic(ph::MAGIC_WAND, "enhanced"), Some(crate::widgets::GOOD)),
                StyleRow::Enhanced { stale: true } => {
                    (ic(ph::MAGIC_WAND, "older values"), Some(crate::widgets::NEUTRAL))
                }
                StyleRow::Edited { stale: false } => ("edited".to_string(), Some(crate::widgets::ACCENT)),
                StyleRow::Edited { stale: true } => ("edited, older values".to_string(), Some(crate::widgets::NEUTRAL)),
            };
            let tip = match row {
                StyleRow::Built => "built from the template, the style and the values".to_string(),
                StyleRow::Enhanced { stale: true } | StyleRow::Edited { stale: true } => format!(
                    "made before the template or values changed; as built now:\n\n{built}\n\nthe wand makes it again"
                ),
                _ => format!("as built:\n\n{built}"),
            };
            ui.separator();
            let r = match color {
                Some(c) => ui.colored_label(c, label),
                None => ui.weak(label),
            };
            r.on_hover_text(tip);
            ui.spacing_mut().item_spacing.x = 2.0;
            if let Some(j) = waiting {
                let again =
                    if st.pe.style_tries.get(&key).is_some_and(|t| t.retry > 0) { " · asked again" } else { "" };
                let tip = format!("{}{again}\nclick to stop", crate::rewrite::progress(st, j));
                if ui.add(egui::Spinner::new()).interact(egui::Sense::click()).on_hover_text(tip).clicked() {
                    out.push(Msg::PeStop(vec![j.id]));
                }
            } else if can_ask {
                let tip = "improve this style's prompt, from the one built, with the default settings (prompt \
                           enhance)";
                if ui.add_enabled(!locked(st), egui::Button::new(ph::MAGIC_WAND).small()).on_hover_text(tip).clicked() {
                    out.push(Msg::PeStyleEnhance(vec![key.clone()]));
                }
            }
            if let Some(why) = st.pe.style_failed.get(&key) {
                ui.colored_label(BAD, ph::WARNING)
                    .on_hover_text(format!("the last ask failed: {why}\n(gone when asked again)"));
            }
            if own.is_some()
                && ui
                    .add_enabled(waiting.is_none(), egui::Button::new(ph::ARROW_COUNTER_CLOCKWISE).small())
                    .on_hover_text("back to as built")
                    .clicked()
            {
                sw.set_style_prompt(&key, None);
            }
            if locked(st) {
                ui.disable();
            }
            if spare
                && ui
                    .small_button(ph::SHUFFLE)
                    .on_hover_text(format!("replace {} with another random style", p.name))
                    .clicked()
            {
                remove = Some(Removed { preset: key.clone(), replace: true });
            }
            if ui.small_button(ph::X).on_hover_text(format!("leave {} out of this sweep", p.name)).clicked() {
                remove = Some(Removed { preset: key.clone(), replace: false });
            }
        });
        let mut text = own.as_ref().map_or_else(|| built.clone(), |o| o.prompt.clone());
        let r = ui.add_enabled(
            waiting.is_none() && !locked(st),
            egui::TextEdit::multiline(&mut text)
                .id_salt(("style_prompt", &key))
                .desired_width(f32::INFINITY)
                .desired_rows(1),
        );
        if r.changed() {
            let p = if text == built {
                None
            } else {
                let (from, enhanced) = own.as_ref().map_or((built.clone(), false), |o| (o.from.clone(), o.enhanced));
                Some(StylePrompt { preset: key.clone(), prompt: text, from, enhanced })
            };
            sw.set_style_prompt(&key, p);
        }
    }
    remove
}

/// When an enhance batch ends with blanks prompt enhance added to styles'
/// prompts: keep them to fill in, or make them plain words. Closed, they're
/// kept.
pub fn added_blanks_window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    if !st.pe.added_blanks_ask {
        return;
    }
    let m = egui::Modal::new(egui::Id::unique("added_blanks")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.heading(ic(ph::MAGIC_WAND, "Prompt enhance added blanks"));
        ui.label(
            "These styles' prompts came back with blanks they didn't have. Kept, they're in Shared values to fill \
             in; made words, their braces go ({scrap_metal} → scrap metal).",
        );
        ui.add_space(6.0);
        ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
            egui::Grid::new("added_blanks_grid").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                for (key, names) in &st.pe.added_blanks {
                    ui.label(&key.name);
                    ui.monospace(names.iter().map(|n| format!("{{{n}}}")).collect::<Vec<_>>().join(", "));
                    ui.end_row();
                }
            });
        });
        ui.add_space(8.0);
        dialog_buttons(ui, |ui| {
            if ui.button(RichText::new("Keep as blanks").strong()).clicked() {
                out.push(Msg::PeAddedBlanks(true));
            }
            if ui.button("Make them words").clicked() {
                out.push(Msg::PeAddedBlanks(false));
            }
        });
    });
    if m.should_close() {
        out.push(Msg::PeAddedBlanks(true));
    }
}

/// **Enhance style prompts…**: the styles that will run, a row each to tick
/// (with where its prompt is and the prompt), **Tick only** buttons, the
/// settings each ask gets, the system prompt and the first one's message.
fn enhance_dialog(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    use increart_core::pe::STYLE_PROMPT;
    use increart_gui_core::update::{StyleRow, enhance_picks, running_styles, style_message_for, style_row};
    let (Some(a), Some(server)) = (&st.pe.style_dialog, st.pe.server.as_ref()) else { return };
    let mut a = a.clone();
    let running = running_styles(st);
    let busy = st.pe.styles_waiting() > 0;
    let (mut ask, mut close) = (false, false);
    let m = egui::Modal::new(egui::Id::unique("enhance_styles")).show(ctx, |ui| {
        ui.set_width(640.0);
        ui.heading(ic(ph::MAGIC_WAND, "Enhance style prompts"));
        ui.weak(
            "one ask for each style ticked, each with these settings (kept with the project): its prompt, the \
             template with the style and the values in, improved",
        );
        ui.add_space(6.0);
        // each ticks exactly those it names, unticking the rest
        let [not_yet, failed] = enhance_picks(st);
        let picks = [
            ("All".to_string(), "every style that will run", running.iter().map(|(k, _)| k.clone()).collect()),
            ("None".to_string(), "no style", vec![]),
            ("Not enhanced yet".to_string(), "those as built, or enhanced from older values (not your edits)", not_yet),
            (format!("Failed {}", ph::WARNING), "those whose last ask failed", failed),
        ];
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Tick only").strong());
            for (label, tip, set) in picks {
                let (n, none) = (set.len(), label == "None");
                let text = if none { label } else { format!("{label} ({n})") };
                let r = ui.add_enabled(none || n > 0, egui::Button::new(text).small());
                if r.on_hover_text(format!("tick {tip}; untick the rest")).clicked() {
                    a.styles = set;
                }
            }
        });
        ui.add_space(2.0);
        // one style a row: where its prompt is, and the prompt
        let f = &st.generate;
        ScrollArea::vertical().id_salt("enhance_styles_list").max_height(220.0).show(ui, |ui| {
            egui::Grid::new("enhance_styles_grid").num_columns(3).striped(true).spacing([16.0, 2.0]).show(ui, |ui| {
                ui.weak("Style");
                ui.weak("Its prompt");
                ui.weak("");
                ui.end_row();
                for (k, p) in &running {
                    let mut on = a.styles.contains(k);
                    if ui.checkbox(&mut on, &p.name).changed() {
                        a.styles.retain(|s| s != k);
                        if on {
                            a.styles.push(k.clone());
                        }
                    }
                    ui.horizontal(|ui| {
                        let (state, tip) = match style_row(st, k, &p.prompt) {
                            StyleRow::Built => ("as built", "from the template, the style and the values"),
                            StyleRow::Enhanced { stale: false } => ("enhanced", "improved by prompt enhance"),
                            StyleRow::Enhanced { stale: true } => {
                                ("enhanced, older values", "improved from values changed since")
                            }
                            StyleRow::Edited { stale: false } => ("edited", "changed by hand"),
                            StyleRow::Edited { stale: true } => {
                                ("edited, older values", "changed by hand, from values changed since")
                            }
                        };
                        ui.weak(state).on_hover_text(tip);
                        if let Some(why) = st.pe.style_failed.get(k) {
                            ui.colored_label(BAD, ph::WARNING).on_hover_text(format!("the last ask failed: {why}"));
                        }
                    });
                    let text = f.sweep.prompt_for(&f.prompt, k, &p.prompt);
                    let cut: String = text.chars().take(50).collect();
                    let shown = if cut.len() < text.len() { format!("{cut}…") } else { cut };
                    ui.label(RichText::new(shown).small()).on_hover_text(text);
                    ui.end_row();
                }
            });
        });
        ui.add_space(4.0);
        ui.separator();
        ui.label("Instructions");
        ui.add(
            egui::TextEdit::multiline(&mut a.how)
                .desired_width(f32::INFINITY)
                .desired_rows(2)
                .hint_text("more cinematic · keep it under 40 words · night scenes …"),
        );
        egui::Grid::new("enhance_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Improve");
            ui.horizontal(|ui| {
                ui.radio_value(&mut a.from_shown, false, "the prompt as built")
                    .on_hover_text("from the template, the style and the values: a fresh start each time");
                ui.radio_value(&mut a.from_shown, true, "the prompt as shown")
                    .on_hover_text("an edit or enhancement is improved further");
            });
            ui.end_row();
            ui.label("Images");
            ui.add_enabled(
                server.vision(),
                egui::Checkbox::new(&mut a.images, "look at the reference and start images"),
            )
            .on_disabled_hover_text("this server's model has no vision (mmproj)");
            ui.end_row();
            ui.label("Sampling");
            ui.horizontal(|ui| {
                let usual = st.config.pe.sampling(STYLE_PROMPT);
                let mut x = a.sampling.unwrap_or(usual);
                ui.label("Temperature");
                ui.add(egui::Slider::new(&mut x.temperature, 0.0..=2.0).step_by(0.05))
                    .on_hover_text("hotter: more varied and surprising, but less on topic");
                ui.label("min_p");
                ui.add(egui::Slider::new(&mut x.min_p, 0.0..=0.5).step_by(0.01))
                    .on_hover_text("drops words less than this fraction as likely as the likeliest (0: off)");
                if a.sampling.is_some()
                    && ui
                        .small_button(ic(ph::ARROW_COUNTER_CLOCKWISE, "Default"))
                        .on_hover_text("the server's, as set in Settings")
                        .clicked()
                {
                    a.sampling = None;
                } else if x != usual {
                    a.sampling = Some(x);
                }
            });
            ui.end_row();
            ui.label("Longest");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut a.max_tokens).range(100..=2000).speed(10));
                ui.weak("tokens (about ¾ of a word each)");
            });
            ui.end_row();
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new("System prompt (for every style)").id_salt("enhance_system").show(ui, |ui| {
            let default = increart_core::pe::DEFAULT_STYLE_PROMPT;
            let own = st.config.pe.system_prompts.get(STYLE_PROMPT).filter(|p| !p.trim().is_empty());
            let mut text = own.cloned().unwrap_or_else(|| default.to_string());
            if own.is_some() && ui.small_button(ic(ph::ARROW_COUNTER_CLOCKWISE, "Default")).clicked() {
                out.push(Msg::PeSystemPrompt { job: STYLE_PROMPT.into(), text: String::new() });
            }
            let r = ScrollArea::vertical().id_salt("enhance_system_text").max_height(140.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut text).desired_width(f32::INFINITY).desired_rows(5))
            });
            if r.inner.changed() {
                let text = if text == default { String::new() } else { text };
                out.push(Msg::PeSystemPrompt { job: STYLE_PROMPT.into(), text });
            }
        });
        let ticked: Vec<_> = running.iter().filter(|(k, _)| a.styles.contains(k)).collect();
        if let Some((k, _)) = ticked.first() {
            egui::CollapsingHeader::new(format!("The message sent (for {}, the first)", k.name))
                .id_salt("enhance_preview")
                .show(ui, |ui| {
                    let msg = style_message_for(st, k, &a).unwrap_or_default();
                    ScrollArea::vertical().id_salt("enhance_preview_text").max_height(180.0).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(msg).monospace()).wrap());
                    });
                });
        }
        ui.add_space(8.0);
        dialog_buttons(ui, |ui| {
            let label = RichText::new(ic(ph::MAGIC_WAND, &format!("Enhance ({})", ticked.len()))).strong();
            let tip = if busy { "wait for the asks going on" } else { "ask for each style ticked" };
            let r = ui.add_enabled(!ticked.is_empty() && !busy, egui::Button::new(label));
            ask = r.on_hover_text(format!("{tip} ({})", server.name)).on_disabled_hover_text(tip).clicked();
            close = ui.button("Close   Esc").clicked();
        });
    });
    if Some(&a) != st.pe.style_dialog.as_ref() {
        out.push(Msg::PeStyleDialog(Some(a)));
    }
    if ask {
        out.push(Msg::PeStyleAsk);
    } else if close || m.should_close() {
        out.push(Msg::PeStyleDialog(None));
    }
}
