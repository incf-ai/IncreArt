//! **Validate servers…** (DESIGN.md, Validating servers): the suite's
//! cases as a tree, each checked to run and with its outcome; the log of
//! the one picked on the right; Run, Stop, Copy report.

use eframe::egui::{self, RichText};
use increart_core::apitest::{Cost, Outcome};
use increart_gui_core::{ApiRun, ApiTest, AppState, Msg};

use crate::widgets::{BAD, GOOD, NEUTRAL, dialog_footer, ic, ph};

pub fn window(ctx: &egui::Context, st: &AppState, t: &ApiTest, out: &mut Vec<Msg>) {
    let screen = ctx.content_rect();
    let m = egui::Modal::new(egui::Id::unique("api_test")).show(ctx, |ui| {
        let width = (screen.width() * 0.9).min(1200.0);
        let height = (screen.height() * 0.7).min(760.0);
        ui.set_width(width);
        ui.heading(ic(ph::STETHOSCOPE, "Validate servers"));
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Each server preset's servers are opened together, as the app opens them (those it starts, \
                     started by the suite and stopped after), then each checked case runs on them. Every request \
                     and answer is logged: Copy report gives it all, to read or to hand to an LLM.",
                )
                .weak(),
            )
            .wrap(),
        );
        ui.add_space(6.0);
        if !t.cases.is_empty() {
            preset_choice(ui, t, out);
            ui.add_space(4.0);
        }
        ui.horizontal_top(|ui| {
            let tree_w = (width * 0.42).max(340.0);
            ui.allocate_ui_with_layout(egui::vec2(tree_w, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_size(egui::vec2(tree_w, height));
                egui::ScrollArea::vertical()
                    .id_salt("api_tree")
                    .max_height(height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| tree(ui, t, out));
            });
            // a line of the panes' height (a separator spans all there is: unbounded in a modal)
            let (line, _) = ui.allocate_exact_size(egui::vec2(9.0, height), egui::Sense::hover());
            ui.painter().vline(line.center().x, line.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
            let w = ui.available_width();
            ui.allocate_ui_with_layout(egui::vec2(w, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_size(egui::vec2(w, height));
                detail(ui, t, height);
            });
        });
        ui.add_space(6.0);
        footer(ui, st, t, out);
    });
    if m.should_close() {
        out.push(Msg::ApiTest(false));
    }
}

/// **Server preset**: one preset's cases only (those for its setup
/// checked), or all presets'.
fn preset_choice(ui: &mut egui::Ui, t: &ApiTest, out: &mut Vec<Msg>) {
    const ALL: &str = "All presets";
    ui.horizontal(|ui| {
        ui.label("Server preset");
        let shown = t.preset.as_deref().unwrap_or(ALL);
        let mut chosen = None;
        ui.add_enabled_ui(!t.running, |ui| {
            egui::ComboBox::from_id_salt("api_preset").selected_text(shown).show_ui(ui, |ui| {
                for p in t.presets() {
                    if ui.selectable_label(t.preset.as_deref() == Some(p), p).clicked() {
                        chosen = Some(Some(p.to_string()));
                    }
                }
                ui.separator();
                if ui.selectable_label(t.preset.is_none(), ALL).clicked() {
                    chosen = Some(None);
                }
            })
        })
        .response
        .on_hover_text("choosing one checks its cases (not those n/a for it) and only those; its servers are the only ones started");
        let shown = t.shown_cases();
        let checked = shown.iter().filter(|&&i| t.cases[i].checked).count();
        ui.weak(format!("{checked} of {} cases checked", shown.len()));
        if let Some(p) = chosen.filter(|p| p != &t.preset) {
            out.push(Msg::ApiTestPreset(p));
        }
    });
}

fn tree(ui: &mut egui::Ui, t: &ApiTest, out: &mut Vec<Msg>) {
    if t.listing {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new());
            ui.label("Asking the suite for its cases…");
        });
        return;
    }
    let all = t.shown_cases();
    for preset in firsts(&all, |i| t.cases[i].case.preset.as_str(), t) {
        let ix: Vec<usize> = all.iter().copied().filter(|&i| t.cases[i].case.preset == preset).collect();
        let id = format!("preset {preset}");
        node(ui, t, &id, RichText::new(preset).strong(), &ix, out, |ui, out| {
            let servers = firsts(&ix, |i| t.cases[i].case.server.as_deref().unwrap_or(""), t);
            for server in servers {
                let ix: Vec<usize> =
                    ix.iter().copied().filter(|&i| t.cases[i].case.server.as_deref().unwrap_or("") == server).collect();
                if server.is_empty() {
                    let id = format!("{preset} together");
                    node(ui, t, &id, RichText::new("servers together").italics(), &ix, out, |ui, out| {
                        for &i in &ix {
                            row(ui, t, i, out);
                        }
                    });
                    continue;
                }
                let id = format!("{preset}/{server}");
                node(ui, t, &id, server, &ix, out, |ui, out| {
                    for group in firsts(&ix, |i| t.cases[i].case.group.as_str(), t) {
                        let ix: Vec<usize> = ix.iter().copied().filter(|&i| t.cases[i].case.group == group).collect();
                        let id = format!("{preset}/{server}/{group}");
                        node(ui, t, &id, group, &ix, out, |ui, out| {
                            for &i in &ix {
                                row(ui, t, i, out);
                            }
                        });
                    }
                });
            }
        });
    }
}

/// The distinct values of `key` over `ix`, in the order first seen.
fn firsts<'a>(ix: &[usize], key: impl Fn(usize) -> &'a str, _: &'a ApiTest) -> Vec<&'a str> {
    let mut v: Vec<&str> = vec![];
    for &i in ix {
        let k = key(i);
        if !v.contains(&k) {
            v.push(k);
        }
    }
    v
}

/// A branch: its checkbox checks all under it (half checked: some), its
/// mark is the worst of theirs.
fn node(
    ui: &mut egui::Ui,
    t: &ApiTest,
    id: &str,
    label: impl Into<egui::WidgetText>,
    ix: &[usize],
    out: &mut Vec<Msg>,
    body: impl FnOnce(&mut egui::Ui, &mut Vec<Msg>),
) {
    let on = ix.iter().filter(|&&i| t.cases[i].checked).count();
    let state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(id), true);
    state
        .show_header(ui, |ui| {
            let mut all = !ix.is_empty() && on == ix.len();
            let partly = on > 0 && on < ix.len();
            if ui.add(egui::Checkbox::without_text(&mut all).indeterminate(partly)).changed() {
                out.push(Msg::ApiTestCheck(ix.to_vec(), all));
            }
            let running = ix.iter().any(|&i| t.cases[i].running);
            let worst = ix.iter().filter_map(|&i| t.cases[i].outcome).min_by_key(|o| o.rank());
            mark(ui, running, worst);
            ui.label(label);
            ui.weak(format!("{on}/{}", ix.len()));
        })
        .body(|ui| body(ui, out));
}

fn row(ui: &mut egui::Ui, t: &ApiTest, i: usize, out: &mut Vec<Msg>) {
    let c = &t.cases[i];
    ui.horizontal(|ui| {
        let mut on = c.checked;
        if ui.checkbox(&mut on, "").changed() {
            out.push(Msg::ApiTestCheck(vec![i], on));
        }
        mark(ui, c.running, c.outcome);
        let mut text = RichText::new(&c.case.title);
        if c.case.not_applicable.is_some() {
            text = text.weak();
        }
        let mut hover = format!("{}\n{}\n\nExpects: {}", c.case.id, cost_text(c.case.cost), c.case.expects);
        if let Some(why) = &c.case.not_applicable {
            hover.push_str(&format!("\n\nn/a here: {why}"));
        }
        if !c.summary.is_empty() {
            hover.push_str(&format!("\n\n{}: {}", c.outcome.map_or("", |o| o.label()), c.summary));
        }
        if ui.selectable_label(t.selected == Some(i), text).on_hover_text(hover).clicked() {
            out.push(Msg::ApiTestSelect(Some(i)));
        }
        if c.outcome.is_some() {
            ui.weak(format!("{:.1} s", c.secs));
        }
    });
}

fn cost_text(cost: Cost) -> &'static str {
    match cost {
        Cost::Light => "light: asks only, nothing loaded",
        Cost::Loads => "loads a model",
        Cost::Gpu => "takes GPU time: generates",
    }
}

fn mark(ui: &mut egui::Ui, running: bool, outcome: Option<Outcome>) {
    if running {
        ui.add(egui::Spinner::new());
        return;
    }
    match outcome {
        Some(Outcome::Pass) => ui.colored_label(GOOD, ph::CHECK_CIRCLE),
        Some(Outcome::Warn) => ui.colored_label(NEUTRAL, ph::WARNING),
        Some(Outcome::Fail) => ui.colored_label(BAD, ph::X_CIRCLE),
        Some(Outcome::Skipped) => ui.weak(ph::MINUS_CIRCLE).on_hover_text("skipped"),
        Some(Outcome::NotApplicable) => ui.weak(ph::MINUS).on_hover_text("n/a"),
        None => ui.weak(ph::CIRCLE),
    };
}

/// The case picked (what it expects, what came back, its log and
/// traffic), or the run's own log.
fn detail(ui: &mut egui::Ui, t: &ApiTest, height: f32) {
    egui::ScrollArea::vertical()
        .id_salt("api_detail")
        .max_height(height)
        .auto_shrink([false, false])
        .stick_to_bottom(t.running)
        .show(ui, |ui| {
            if let Some(e) = &t.error {
                ui.add(egui::Label::new(RichText::new(e).color(BAD)).wrap());
                ui.add_space(4.0);
            }
            detail_body(ui, t)
        });
}

fn detail_body(ui: &mut egui::Ui, t: &ApiTest) {
    match t.selected.and_then(|i| t.cases.get(i)) {
        Some(c) => {
            ui.label(RichText::new(&c.case.title).strong());
            ui.weak(format!("{}  ·  {}", c.case.id, cost_text(c.case.cost)));
            ui.add_space(4.0);
            ui.add(egui::Label::new(format!("Expects: {}", c.case.expects)).wrap());
            ui.add_space(4.0);
            match (c.running, c.outcome) {
                (true, _) => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label("running…");
                    });
                }
                (false, Some(o)) => {
                    ui.horizontal_top(|ui| {
                        mark(ui, false, Some(o));
                        let text = format!("{}: {} ({:.1} s)", o.label(), c.summary, c.secs);
                        ui.add(egui::Label::new(RichText::new(text).strong()).wrap());
                    });
                }
                (false, None) => {
                    let why = c.case.not_applicable.as_deref().map(|w| format!("n/a here: {w}"));
                    ui.weak(why.unwrap_or_else(|| "not run yet".into()));
                }
            }
            if !c.log.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("Log").strong());
                lines(ui, c.log.iter().map(String::as_str));
            }
            if !c.traffic.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(format!("Traffic ({} requests)", c.traffic.len())).strong());
                lines(ui, c.traffic.iter().map(String::as_str));
            }
        }
        None => {
            ui.label(RichText::new("The run").strong());
            if t.log.is_empty() && !t.running {
                ui.weak("Check the cases to run on the left (a branch's box checks all in it), then Run. Click a case to see its log and the requests it made.");
            }
            lines(ui, t.log.iter().map(String::as_str));
            if t.running {
                ui.add(egui::Spinner::new());
            }
        }
    }
}

fn lines<'a>(ui: &mut egui::Ui, text: impl Iterator<Item = &'a str>) {
    let text: Vec<&str> = text.collect();
    if text.is_empty() {
        return;
    }
    ui.add(egui::Label::new(RichText::new(text.join("\n")).monospace()).wrap());
}

fn footer(ui: &mut egui::Ui, st: &AppState, t: &ApiTest, out: &mut Vec<Msg>) {
    let checked = t.cases.iter().filter(|c| c.checked).count();
    let failed = t.ids(ApiRun::Failed).len();
    let idle = !t.running && !t.listing;
    let report = t.done.as_ref().and_then(|(_, r, _)| r.clone());
    let zip = t.done.as_ref().and_then(|(_, _, z)| z.clone());
    let (left, right) = dialog_footer(
        ui,
        |ui| {
            let mut out = vec![];
            // of the preset shown: a hidden one's servers aren't started unseen
            let every = t.shown_cases();
            if ui.button("Check all").clicked() {
                out.push(Msg::ApiTestCheck(every.clone(), true));
            }
            if ui.button("Uncheck all").clicked() {
                out.push(Msg::ApiTestCheck(every.clone(), false));
            }
            let light: Vec<usize> = every.iter().copied().filter(|&i| t.cases[i].case.cost == Cost::Light).collect();
            if ui
                .button("Light only")
                .on_hover_text("checks only the cases that load nothing and generate nothing")
                .clicked()
            {
                out.push(Msg::ApiTestCheck(every.clone(), false));
                out.push(Msg::ApiTestCheck(light, true));
            }
            let basic: Vec<usize> = every
                .iter()
                .copied()
                .filter(|&i| t.cases[i].case.basic && t.cases[i].case.not_applicable.is_none())
                .collect();
            if ui
                .button("Basic only")
                .on_hover_text(
                    "checks only the basic cases: each server answers, lists its models and makes a small image, \
                     a chat server answers, the servers together route each model preset and run through the \
                     engine. A preset whose basic cases all run is noted as validated (or not) for its settings",
                )
                .clicked()
            {
                out.push(Msg::ApiTestCheck(every.clone(), false));
                out.push(Msg::ApiTestCheck(basic, true));
            }
            ui.separator();
            let copy = ui
                .add_enabled(report.is_some(), egui::Button::new(ic(ph::COPY, "Copy report")))
                .on_hover_text("report.md: each case, what it expects, its log and traffic; to read or hand to an LLM")
                .on_disabled_hover_text("once a run is over");
            if copy.clicked()
                && let Some(r) = &report
            {
                match std::fs::read_to_string(r.join("report.md")) {
                    Ok(text) => ui.ctx().copy_text(text),
                    Err(e) => ui.ctx().copy_text(format!("{}: {e}", r.join("report.md").display())),
                }
            }
            let show = ui
                .add_enabled(zip.is_some(), egui::Button::new(ic(ph::FILE_ZIP, "Show zip")))
                .on_hover_text("the report, its traffic and the started servers' logs, zipped: to send")
                .on_disabled_hover_text("once a run is over");
            if show.clicked()
                && let Some(z) = &zip
            {
                out.push(Msg::ShowInFileManager(z.clone()));
            }
            if ui
                .button(ic(ph::TERMINAL, "Copy command"))
                .on_hover_text("the same run from a terminal (increart-apitest, beside the app)")
                .clicked()
            {
                ui.ctx().copy_text(command_line(st, t));
            }
            out
        },
        |ui| {
            let mut out = vec![];
            let close = ui.add_enabled(!t.stopping, egui::Button::new(ic(ph::X, "Close")));
            if close.on_hover_text("while it runs: stops it first").clicked() {
                out.push(Msg::ApiTest(false));
            }
            if t.running {
                let text = if t.stopping { "Stopping…" } else { "Stop" };
                let stop = ui
                    .add_enabled(!t.stopping, egui::Button::new(ic(ph::STOP, text)))
                    .on_hover_text("ends the case going on, then stops the servers the suite started");
                if stop.clicked() {
                    out.push(Msg::ApiTestStop);
                }
            }
            if ui.add_enabled(idle && failed > 0, egui::Button::new(ic(ph::ARROW_CLOCKWISE, "Rerun failed"))).clicked()
            {
                out.push(Msg::ApiTestRun(ApiRun::Failed));
            }
            if ui.add_enabled(idle && !t.cases.is_empty(), egui::Button::new("Run all")).clicked() {
                out.push(Msg::ApiTestRun(ApiRun::All));
            }
            let run = egui::Button::new(RichText::new(ic(ph::PLAY, &format!("Run {checked} checked"))).strong());
            if ui.add_enabled(idle && checked > 0, run).clicked() {
                out.push(Msg::ApiTestRun(ApiRun::Checked));
            }
            out
        },
    );
    out.extend(left);
    out.extend(right);
}

/// The checked cases' run, for a terminal.
fn command_line(st: &AppState, t: &ApiTest) -> String {
    let quote = |s: &str| {
        if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-:,*".contains(c)) {
            s.to_string()
        } else {
            format!("\"{}\"", s.replace('"', "\\\""))
        }
    };
    let mut cmd = format!("{} run", quote(&increart_core::apitest::binary().display().to_string()));
    if let Some(p) = &st.config_path {
        cmd.push_str(&format!(" --config {}", quote(&p.display().to_string())));
    }
    if let Some(p) = &t.preset {
        cmd.push_str(&format!(" --preset {}", quote(p)));
    }
    let ids = t.ids(ApiRun::Checked);
    if !ids.is_empty() && ids.len() < t.shown_cases().len() {
        cmd.push_str(&format!(" --cases {}", quote(&ids.join(","))));
    }
    cmd
}
