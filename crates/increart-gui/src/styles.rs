//! The style sweep's **Choose styles** window: tasks and their groups on
//! the left; on the right their presets, each with what it has made so far,
//! to tick for the sweep. OK keeps the ticks, Cancel puts back the sweep.

use eframe::egui::{self, RichText, ScrollArea};
use increart_core::config::StyleSort;
use increart_core::prompts::{PresetKey, PromptPreset, PromptPresets, PromptTask};
use increart_gui_core::{AppState, Msg, StyleFilter, StylePicker, StyleStats, StyleView, needs_refs};

use crate::textures::Textures;
use crate::widgets::{BAD, GOOD, ICON_FILL, NEUTRAL, dialog_footer, group, ic, ph};

const THUMB: f32 = 64.0;

pub fn window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let (Some(pk), Some(root)) = (&st.styles, &st.library) else { return };
    let list = &st.prompt_presets;
    let ticked = &st.generate.sweep.selected;
    let rows = st.style_rows(pk);
    let screen = ctx.content_rect().size();
    let width = (screen.x - 80.0).clamp(640.0, 1100.0);
    let height = (screen.y - 260.0).clamp(280.0, 640.0);
    let mut search_focused = false;
    egui::Modal::new(egui::Id::unique("choose_styles")).show(ctx, |ui| {
        ui.set_width(width);
        ui.heading(ic(ph::SWATCHES, "Choose styles"));
        ui.weak("A run per ticked style. Click a style to tick or untick it  ·  ★: favourite");
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            group(ui, "search", |ui| {
                let mut q = pk.search.clone();
                ui.label(ph::MAGNIFYING_GLASS);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut q)
                        .hint_text("Search names, descriptions and prompts")
                        .desired_width(300.0),
                );
                search_focused = r.has_focus();
                if !q.is_empty() && ui.small_button(ph::X).on_hover_text("clear the search").clicked() {
                    q.clear();
                }
                if q != pk.search {
                    out.push(Msg::StylesSearch(q));
                }
            });
            group(ui, "filter", |ui| filter_menu(ui, pk.filter, out));
            group(ui, "sort", |ui| {
                ui.label("Sort");
                let mut sort = st.config.style_sort;
                egui::ComboBox::from_id_salt("styles_sort").selected_text(sort.name()).show_ui(ui, |ui| {
                    for s in StyleSort::EVERY {
                        ui.selectable_value(&mut sort, s, s.name());
                    }
                });
                if sort != st.config.style_sort {
                    out.push(Msg::StylesSort(sort));
                }
            });
        });
        ui.separator();
        let row = egui::Layout::left_to_right(egui::Align::Min);
        let down = egui::Layout::top_down(egui::Align::Min);
        // bounded: a vertical separator takes all the height it is given
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), row, |ui| {
            ui.allocate_ui_with_layout(egui::vec2(240.0, height), down, |ui| {
                ScrollArea::vertical().id_salt("styles_tree").auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(230.0);
                    tree(ui, list, ticked, &pk.view, pk.search.is_empty(), out);
                });
            });
            ui.separator();
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), down, |ui| {
                ScrollArea::vertical().id_salt("styles_list").auto_shrink([false, false]).show(ui, |ui| {
                    style_list(ui, st, pk, &rows, root, tex, out);
                });
            });
        });
        ui.separator();
        footer(ui, st, &rows, out);
    });
    // Esc clears the search first; clicking outside does nothing (it would
    // drop the ticks)
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        out.push(if search_focused && !pk.search.is_empty() {
            Msg::StylesSearch(String::new())
        } else {
            Msg::StylesClose(false)
        });
    }
}

fn filter_menu(ui: &mut egui::Ui, filter: StyleFilter, out: &mut Vec<Msg>) {
    let n = filter.count();
    let label = if n == 0 { ic(ph::FUNNEL, "Filter") } else { ic(ph::FUNNEL, &format!("Filter ({n})")) };
    let label = if n == 0 { RichText::new(label) } else { RichText::new(label).color(NEUTRAL) };
    ui.menu_button(label, |ui| {
        let mut f = filter;
        ui.checkbox(&mut f.favourites, "★ Favourites only");
        ui.checkbox(&mut f.usable, "Usable with Generate's reference images");
        ui.checkbox(&mut f.untried_here, "Never used in this project");
        ui.checkbox(&mut f.has_good, "Has a Good image");
        if n > 0 {
            ui.separator();
            if ui.button(ic(ph::FUNNEL_X, "Clear filters")).clicked() {
                f = StyleFilter::default();
                ui.close();
            }
        }
        if f != filter {
            out.push(Msg::StylesFilter(f));
        }
    });
}

fn is_ticked(ticked: &[PresetKey], t: &PromptTask, p: &PromptPreset) -> bool {
    ticked.iter().any(|k| k.task == t.name && k.name == p.name)
}

/// "name  (ticked / all)", or "name  (all)" with none ticked.
fn counted(name: &str, k: usize, n: usize) -> String {
    if k > 0 { format!("{name}  ({k} / {n})") } else { format!("{name}  ({n})") }
}

fn tree(
    ui: &mut egui::Ui,
    list: &PromptPresets,
    ticked: &[PresetKey],
    view: &StyleView,
    enabled: bool,
    out: &mut Vec<Msg>,
) {
    ui.add_enabled_ui(enabled, |ui| {
        let all = list.tasks.iter().flat_map(|t| t.presets.iter().map(move |p| (t, p)));
        let favs = all.clone().filter(|(_, p)| p.favourite).count();
        let fav_ticked = all.clone().filter(|(t, p)| p.favourite && is_ticked(ticked, t, p)).count();
        let total = all.count();
        let mut item = |ui: &mut egui::Ui, v: StyleView, label: String| {
            if ui.selectable_label(*view == v, label).clicked() {
                out.push(Msg::StylesShow(v));
            }
        };
        item(ui, StyleView::Ticked, format!("{}  Ticked  ({})", ph::CHECK_SQUARE, ticked.len()));
        item(ui, StyleView::Favourites, counted(&format!("{}  Favourites", ph::STAR), fav_ticked, favs));
        item(ui, StyleView::All, counted(&format!("{}  All", ph::LIST), ticked.len(), total));
        for t in &list.tasks {
            ui.add_space(2.0);
            let k = t.presets.iter().filter(|p| is_ticked(ticked, t, p)).count();
            let mut groups: Vec<&str> = vec![];
            for p in &t.presets {
                if !p.group.is_empty() && !groups.contains(&p.group.as_str()) {
                    groups.push(&p.group);
                }
            }
            let here = matches!(view, StyleView::Task(x) | StyleView::Group(x, _) if *x == t.name);
            // folded at first unless it has ticks or is the one shown
            let id = ui.make_persistent_id(("styles_task", &t.name));
            let fold = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, k > 0 || here);
            let mut task = |ui: &mut egui::Ui| {
                let v = StyleView::Task(t.name.clone());
                let r = ui.selectable_label(*view == v, counted(&t.name, k, t.presets.len()));
                let r = if t.description.is_empty() { r } else { r.on_hover_text(&t.description) };
                if r.clicked() {
                    out.push(Msg::StylesShow(v));
                }
            };
            if groups.is_empty() {
                // nothing to fold: in line with the folders' names
                ui.horizontal(|ui| {
                    ui.add_space(ui.spacing().indent);
                    task(ui);
                });
                continue;
            }
            fold.show_header(ui, task).body(|ui| {
                for g in groups {
                    let of = t.presets.iter().filter(|p| p.group == g);
                    let k = of.clone().filter(|p| is_ticked(ticked, t, p)).count();
                    let v = StyleView::Group(t.name.clone(), g.to_string());
                    if ui.selectable_label(*view == v, counted(g, k, of.count())).clicked() {
                        out.push(Msg::StylesShow(v.clone()));
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

fn style_list(
    ui: &mut egui::Ui,
    st: &AppState,
    pk: &StylePicker,
    rows: &[(usize, usize)],
    root: &std::path::Path,
    tex: &mut Textures,
    out: &mut Vec<Msg>,
) {
    let list = &st.prompt_presets;
    let ticked = &st.generate.sweep.selected;
    let refs = st.generate.refs().len();
    let none = StyleStats::default();
    for &(ti, pi) in rows {
        let (t, p) = (&list.tasks[ti], &list.tasks[ti].presets[pi]);
        let key = PresetKey { task: t.name.clone(), name: p.name.clone() };
        let s = pk.stats.get(&key).unwrap_or(&none);
        let on = is_ticked(ticked, t, p);
        let need = needs_refs(&p.prompt);
        let frame = egui::Frame::new().inner_margin(4.0).corner_radius(4.0);
        let frame = if on { frame.fill(ui.visuals().selection.bg_fill.gamma_multiply(0.35)) } else { frame };
        let r = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let mut c = on;
                let mut toggle = ui.checkbox(&mut c, "").on_hover_text("in the sweep").clicked();
                let star = if p.favourite {
                    RichText::new(ph::STAR)
                        .family(egui::FontFamily::Name(ICON_FILL.into()))
                        .color(crate::widgets::ACCENT)
                } else {
                    RichText::new(ph::STAR).weak()
                };
                let tip = if p.favourite { "remove from the favourites" } else { "add to the favourites" };
                if ui.add(egui::Button::new(star).frame(false)).on_hover_text(tip).clicked() {
                    out.push(Msg::PresetFavourite(ti, pi));
                }
                let img = s.sample.as_ref().and_then(|i| tex.thumb(root, i));
                let tr = crate::widgets::thumb(ui, img.as_ref(), THUMB, false, None);
                let tr = match &s.sample {
                    Some(_) if s.good > 0 => tr.on_hover_text("its newest Good image"),
                    Some(_) => tr.on_hover_text("its newest image (none rated Good)"),
                    None => tr.on_hover_text("never used"),
                };
                toggle |= tr.clicked();
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        let mut tip = p.prompt.clone();
                        if !p.negative.is_empty() {
                            tip.push_str(&format!("\n\nNegative: {}", p.negative));
                        }
                        let name = egui::Button::new(RichText::new(&p.name).strong()).frame(false);
                        toggle |= ui.add(name).on_hover_text(tip).clicked();
                        let place =
                            if p.group.is_empty() { t.name.clone() } else { format!("{} → {}", t.name, p.group) };
                        ui.weak(place);
                        if need > refs {
                            ui.colored_label(NEUTRAL, format!("needs {need} reference image(s); Generate has {refs}"));
                        }
                    });
                    if !p.description.is_empty() {
                        ui.add(egui::Label::new(RichText::new(&p.description).weak()).wrap());
                    }
                    stats_line(ui, s);
                });
                if toggle {
                    out.push(Msg::SweepToggle(ti, pi));
                }
            });
        });
        // the row's empty space ticks too
        if ui.interact(r.response.rect, r.response.id.with("row"), egui::Sense::click()).clicked() {
            out.push(Msg::SweepToggle(ti, pi));
        }
    }
    if pk.view == StyleView::Ticked && pk.search.is_empty() {
        gone(ui, list, ticked, out);
    }
    if rows.is_empty() {
        ui.add_space(8.0);
        ui.weak(match (&pk.view, pk.search.is_empty(), pk.filter.count()) {
            (_, false, _) => "No style matches the search.",
            (_, _, n) if n > 0 => "No style here passes the filters.",
            (StyleView::Ticked, _, _) => "Nothing ticked: tick styles in the tasks on the left.",
            (StyleView::Favourites, _, _) => "No favourites yet: click ☆ next to a style to add it.",
            _ => "No styles here: add some with Edit presets….",
        });
    }
}

fn stats_line(ui: &mut egui::Ui, s: &StyleStats) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if s.images() == 0 && s.last.is_none() {
            ui.weak("never used");
            return;
        }
        for (n, what, color) in [(s.good, "Good", GOOD), (s.neutral, "Neutral", NEUTRAL), (s.bad, "Bad", BAD)] {
            if n > 0 {
                ui.colored_label(color, format!("{n} {what}"));
            }
        }
        if s.unrated > 0 {
            ui.weak(format!("{} unrated", s.unrated));
        }
        if let Some(last) = &s.last {
            ui.weak(format!("· last used {}", last.to_string().get(..10).unwrap_or_default()));
        }
        if s.here > 0 {
            ui.weak(format!("· {} run(s) in this project", s.here));
        }
    });
}

/// Ticks whose preset is no longer there (renamed or deleted), which the
/// sweep skips, to untick.
fn gone(ui: &mut egui::Ui, list: &PromptPresets, ticked: &[PresetKey], out: &mut Vec<Msg>) {
    let there =
        |k: &PresetKey| list.tasks.iter().any(|t| t.name == k.task && t.presets.iter().any(|p| p.name == k.name));
    let missing: Vec<_> = ticked.iter().filter(|k| !there(k)).collect();
    if missing.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(RichText::new("No longer there").strong().size(15.0));
    ui.weak("Renamed or deleted since they were ticked: the sweep skips them.");
    for k in missing {
        ui.horizontal(|ui| {
            let mut on = true;
            if ui.checkbox(&mut on, "").on_hover_text("untick").clicked() {
                out.push(Msg::SweepToggleKey(k.clone()));
            }
            ui.label(RichText::new(&k.name).weak().strikethrough());
            ui.weak(format!("({})", k.task));
        });
    }
}

fn footer(ui: &mut egui::Ui, st: &AppState, rows: &[(usize, usize)], out: &mut Vec<Msg>) {
    let f = &st.generate;
    let list = &st.prompt_presets;
    let refs = f.refs().len();
    let ticked = &f.sweep.selected;
    let usable = |&(t, p): &(usize, usize)| needs_refs(&list.tasks[t].presets[p].prompt) <= refs;
    let fit = list
        .tasks
        .iter()
        .flat_map(|t| t.presets.iter().map(move |p| (t, p)))
        .filter(|(t, p)| is_ticked(ticked, t, p) && needs_refs(&p.prompt) <= refs)
        .count();
    let n = if f.sweep.random > 0 { fit.min(f.sweep.random) } else { fit };
    let images = n as u64 * f.seeds;
    let secs = crate::projects::estimate(st, f) * images as f32;
    let mut line = format!("{} ticked", ticked.len());
    if f.sweep.random > 0 {
        line.push_str(&format!(" · a random {n} of them"));
    }
    line.push_str(&format!(
        " · {n} styles × {} seeds = {images} images ≈ {}",
        f.seeds,
        crate::projects::fmt_secs(secs)
    ));
    if fit < ticked.len() {
        line.push_str(&format!(" ({} left out: need more reference images)", ticked.len() - fit));
    }
    ui.label(line);
    let (mut tools, done) = dialog_footer(
        ui,
        |ui| {
            let mut out = vec![];
            let shown_ok: Vec<_> = rows.iter().copied().filter(usable).collect();
            let skipped = rows.len() - shown_ok.len();
            let tip = match skipped {
                0 => "tick every style listed".to_string(),
                n => format!("tick every style listed ({n} need more reference images and are left out)"),
            };
            if ui
                .add_enabled(!shown_ok.is_empty(), egui::Button::new(ic(ph::CHECK_SQUARE, "Tick all shown")))
                .on_hover_text(tip)
                .clicked()
            {
                out.push(Msg::SweepTicks(shown_ok, true));
            }
            if ui
                .add_enabled(!rows.is_empty(), egui::Button::new(ic(ph::SQUARE, "Untick all shown")))
                .on_hover_text("untick every style listed")
                .clicked()
            {
                out.push(Msg::SweepTicks(rows.to_vec(), false));
            }
            if ui
                .add_enabled(!ticked.is_empty(), egui::Button::new(ic(ph::X, "Clear all ticks")))
                .on_hover_text("untick every style, in every task")
                .clicked()
            {
                out.push(Msg::SweepClearTicks);
            }
            out
        },
        |ui| {
            let ok = ui.button(RichText::new(ic(ph::CHECK, "Apply")).strong()).clicked();
            let tip = "put back the styles as they were";
            let cancel = ui.button("Cancel   Esc").on_hover_text(tip).clicked();
            ok.then_some(true).or(cancel.then_some(false))
        },
    );
    out.append(&mut tools);
    if let Some(keep) = done {
        out.push(Msg::StylesClose(keep));
    }
}
