//! **Validate servers…** (DESIGN.md, Validating servers). Its basic view:
//! a preset's validation, what it checks, its steps as they run, and a
//! summary at the end (what went wrong, Open the preset). Its advanced
//! view: the suite's cases as a tree, each checked to run and with its
//! outcome; the log of the one picked on the right; Run, Stop, Copy report.

use eframe::egui::{self, RichText};
use increart_core::apitest::{Case, Cost, Outcome};
use increart_gui_core::{ApiRun, ApiTest, AppState, Msg};

use crate::widgets::{ACCENT, BAD, GOOD, NEUTRAL, dialog_footer, ic, ph};

pub fn window(ctx: &egui::Context, st: &AppState, t: &ApiTest, out: &mut Vec<Msg>) {
    let screen = ctx.content_rect();
    let advanced = st.api_test_advanced;
    let m = egui::Modal::new(egui::Id::unique("api_test")).show(ctx, |ui| {
        let width = (screen.width() * 0.9).min(if advanced { 1200.0 } else { 760.0 });
        let height = (screen.height() * 0.7).min(if advanced { 760.0 } else { 520.0 });
        ui.set_width(width);
        if !advanced {
            ui.heading(ic(ph::STETHOSCOPE, "Validate servers"));
            basic(ui, st, t, height, out);
            return;
        }
        ui.heading(ic(ph::STETHOSCOPE, "Validate servers: advanced"));
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
                    try_line(ui, Some(o), &c.summary);
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
            if let Some((counts, ..)) = t.done.as_ref().filter(|_| !t.running) {
                let color = if counts.fail > 0 {
                    BAD
                } else if counts.warn > 0 {
                    NEUTRAL
                } else {
                    GOOD
                };
                ui.label(RichText::new(format!("Last run: {}", counts.line())).color(color));
            }
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

/// The basic view: the preset shown, validated step by step.
fn basic(ui: &mut egui::Ui, st: &AppState, t: &ApiTest, height: f32, out: &mut Vec<Msg>) {
    let preset = t.preset.as_deref();
    if !t.cases.is_empty() && !t.running {
        ui.horizontal(|ui| {
            ui.label("Server preset");
            let mut chosen = None;
            egui::ComboBox::from_id_salt("api_basic_preset").selected_text(preset.unwrap_or("choose one")).show_ui(
                ui,
                |ui| {
                    for p in t.presets() {
                        if ui.selectable_label(preset == Some(p), p).clicked() {
                            chosen = Some(p.to_string());
                        }
                    }
                },
            );
            if let Some(p) = chosen.filter(|p| Some(p.as_str()) != preset) {
                out.push(Msg::ApiTestPreset(Some(p)));
            }
            if let Some(p) = preset {
                status_line(ui, st, p);
            }
        });
        ui.add_space(6.0);
    }
    egui::ScrollArea::vertical().id_salt("api_basic").max_height(height).auto_shrink([false, true]).show(ui, |ui| {
        ui.set_min_height(height * 0.6);
        if t.listing {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label("Getting ready…");
            });
        } else if t.running {
            running(ui, st, t, preset.unwrap_or("the preset"));
        } else if t.finished() {
            summary(ui, st, t, preset.unwrap_or("The preset"));
        } else {
            if let Some(e) = &t.error {
                ui.add(egui::Label::new(RichText::new(e).color(BAD)).wrap());
                ui.add_space(6.0);
            }
            ready(ui, t, preset);
        }
    });
    ui.add_space(6.0);
    basic_footer(ui, t, out);
}

/// What the preset's marks says: validated when, or not.
fn status_line(ui: &mut egui::Ui, st: &AppState, preset: &str) {
    use increart_core::validation::Status;
    let Some(p) = st.config.preset(preset) else { return };
    let (icon, text, color) = match st.validations.status(&st.config, p) {
        Status::Passed(s) => (ph::SEAL_CHECK, format!("validated on {}", s.day()), GOOD),
        Status::Failed(s) => (ph::SEAL_WARNING, format!("validation failed on {}", s.day()), BAD),
        Status::Changed(s) => {
            (ph::SEAL_QUESTION, format!("not validated since its settings changed ({})", s.day()), NEUTRAL)
        }
        Status::Never => (ph::SEAL_QUESTION, "not validated yet".to_string(), NEUTRAL),
    };
    ui.label(RichText::new(ic(icon, &text)).color(color));
}

/// Before a run: what validating does, and its steps.
fn ready(ui: &mut egui::Ui, t: &ApiTest, preset: Option<&str>) {
    let Some(preset) = preset else {
        ui.label("Choose the server preset to validate above.");
        return;
    };
    ui.add(
        egui::Label::new(format!(
            "Checks that IncreArt can use {preset}'s servers, as it will when you open the preset: each server \
             answers, lists its models and makes a small image, then a short run goes through them all. It takes \
             a few minutes; the first image takes the longest. Servers the preset starts are started for the \
             check and stopped after."
        ))
        .wrap(),
    );
    ui.add_space(8.0);
    ui.label(RichText::new("What it checks").strong());
    steps(ui, t, &t.basic_cases());
    ui.add_space(8.0);
    ui.weak(
        "Press Start validation below. When it's done, a summary says whether the servers work, and what to do if not.",
    );
}

/// During a run: how far it is, its steps.
fn running(ui: &mut egui::Ui, st: &AppState, t: &ApiTest, preset: &str) {
    let ran: Vec<usize> = t.ran.iter().copied().filter(|&i| t.cases[i].case.not_applicable.is_none()).collect();
    let done = ran.iter().filter(|&&i| t.cases[i].outcome.is_some()).count();
    let title =
        if t.stopping { format!("Stopping the validation of {preset}…") } else { format!("Validating {preset}…") };
    ui.label(RichText::new(title).strong().size(17.0));
    ui.add_space(4.0);
    let secs = (st.now - t.started).max(0.0);
    ui.add(egui::ProgressBar::new(if ran.is_empty() { 0.0 } else { done as f32 / ran.len() as f32 }).text(format!(
        "step {} of {} · {}",
        (done + 1).min(ran.len()),
        ran.len(),
        duration(secs)
    )));
    ui.add_space(6.0);
    steps(ui, t, &ran);
    ui.add_space(6.0);
    ui.weak("The first image can take a minute or two while the model loads. You can stop at any time.");
}

/// After a run: whether it works, what went wrong, what to do.
fn summary(ui: &mut egui::Ui, st: &AppState, t: &ApiTest, preset: &str) {
    let ran: Vec<usize> = t.ran.iter().copied().filter(|&i| t.cases[i].case.not_applicable.is_none()).collect();
    let is = |i: usize, o: &[Outcome]| t.cases[i].outcome.is_some_and(|x| o.contains(&x));
    let failed: Vec<usize> = ran.iter().copied().filter(|&i| is(i, &[Outcome::Fail])).collect();
    let skipped: Vec<usize> =
        ran.iter().copied().filter(|&i| is(i, &[Outcome::Skipped]) || t.cases[i].outcome.is_none()).collect();
    let warned: Vec<usize> = ran.iter().copied().filter(|&i| is(i, &[Outcome::Warn])).collect();
    let took = t.ended.map(|e| duration((e - t.started).max(0.0))).unwrap_or_default();
    let (icon, color, title, text) = if t.stopped {
        (
            ph::STOP_CIRCLE,
            NEUTRAL,
            "Stopped before it finished".to_string(),
            format!("{preset} isn't validated: Run again to finish the checks."),
        )
    } else if !failed.is_empty() || t.error.is_some() {
        let n = failed.len().max(1);
        (
            ph::X_CIRCLE,
            BAD,
            format!("{n} problem{} found", if n == 1 { "" } else { "s" }),
            format!(
                "IncreArt may not be able to generate with {preset} until {} fixed. What went wrong is below. Copy \
                 report gives every request and answer, to send to whoever runs the servers (or to paste to an \
                 LLM); Advanced view shows each check's log.",
                if n == 1 { "it's" } else { "they're" }
            ),
        )
    } else if !warned.is_empty() {
        (
            ph::CHECK_CIRCLE,
            GOOD,
            format!("{preset} works with IncreArt"),
            format!(
                "Every check passed, {} with something worth knowing (below). You can open the preset.",
                if warned.len() == 1 { "one".to_string() } else { warned.len().to_string() }
            ),
        )
    } else {
        (
            ph::CHECK_CIRCLE,
            GOOD,
            format!("{preset} works with IncreArt"),
            "Every check passed. You can open the preset.".into(),
        )
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).color(color).size(30.0));
        ui.vertical(|ui| {
            ui.label(RichText::new(title).strong().size(19.0).color(color));
            ui.weak(format!("{} checks in {took}", ran.len()));
        });
    });
    ui.add_space(4.0);
    ui.add(egui::Label::new(text).wrap());
    if let Some(e) = &t.error {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(e).color(BAD)).wrap());
    }
    // whether the Servers screen now marks it validated
    let basic = t.basic_cases();
    if !t.stopped && !basic.is_empty() && basic.iter().all(|i| t.ran.contains(i)) {
        use increart_core::validation::Status;
        let marked = st.config.preset(preset).map(|p| st.validations.status(&st.config, p));
        ui.add_space(4.0);
        match marked {
            Some(Status::Passed(_)) => {
                ui.label(RichText::new(ic(ph::SEAL_CHECK, "Marked validated on the Servers screen.")).color(GOOD));
            }
            Some(Status::Failed(_)) => {
                ui.label(
                    RichText::new(ic(ph::SEAL_WARNING, "Marked validation failed on the Servers screen.")).color(BAD),
                );
            }
            _ => {}
        }
    }
    let problems = |ui: &mut egui::Ui, heading: &str, ix: &[usize]| {
        if ix.is_empty() {
            return;
        }
        ui.add_space(8.0);
        ui.label(RichText::new(heading).strong());
        for &i in ix {
            let c = &t.cases[i];
            ui.horizontal_top(|ui| {
                mark(ui, false, c.outcome.or(Some(Outcome::Skipped)));
                ui.vertical(|ui| {
                    ui.label(step_name(&c.case));
                    let why = if c.summary.is_empty() { "not run" } else { c.summary.as_str() };
                    ui.add(egui::Label::new(RichText::new(why).weak()).wrap());
                    try_line(ui, c.outcome, &c.summary);
                });
            });
        }
    };
    problems(ui, "What went wrong", &failed);
    if !t.stopped {
        problems(ui, "Not checked (what they need went wrong first)", &skipped);
    }
    problems(ui, "Worth knowing", &warned);
    ui.add_space(8.0);
    egui::CollapsingHeader::new(RichText::new("Every check").strong())
        .id_salt(("api_basic_every", t.run))
        .default_open(failed.is_empty() && warned.is_empty())
        .show(ui, |ui| steps(ui, t, &ran));
}

/// What to try for a failure or warning, if it's a common one.
fn try_line(ui: &mut egui::Ui, outcome: Option<Outcome>, summary: &str) {
    if let Some(h) = outcome.and_then(|o| increart_core::apitest::hint(o, summary)) {
        ui.add(egui::Label::new(RichText::new(ic(ph::LIGHTBULB, &format!("Try: {h}"))).color(ACCENT)).wrap());
    }
}

/// The cases as steps: their mark, name and (once run) what came back.
fn steps(ui: &mut egui::Ui, t: &ApiTest, ix: &[usize]) {
    for &i in ix {
        let c = &t.cases[i];
        ui.horizontal(|ui| {
            mark(ui, c.running, c.outcome);
            let name = RichText::new(step_name(&c.case));
            ui.label(if c.running { name.strong() } else { name });
            if c.outcome.is_some() && !c.summary.is_empty() {
                ui.add(egui::Label::new(RichText::new(&c.summary).weak()).truncate()).on_hover_text(&c.summary);
            }
        });
    }
}

/// A case as said to someone who doesn't know the suite.
fn step_name(c: &Case) -> String {
    let key = c.id.rsplit('/').next().unwrap_or(&c.id);
    let what = match key {
        "together.open" => return "Start or connect to the servers".into(),
        "together.routing" => return "Each model preset has a server to run on".into(),
        "together.end-to-end" => return "A short run through all of them, as the app uses them".into(),
        "connect.capabilities" => "answers".to_string(),
        "models.list" => "lists its models".into(),
        "generate.txt2img" => "makes a small image".into(),
        "prompt-enhance.answers" => "answers a prompt-enhance question".into(),
        _ => c.title.clone(),
    };
    match &c.server {
        Some(s) => format!("{s}: {what}"),
        None => what,
    }
}

/// `secs` as "42 s" or "3 min 5 s".
fn duration(secs: f64) -> String {
    let s = secs.round() as u64;
    if s < 60 { format!("{s} s") } else { format!("{} min {} s", s / 60, s % 60) }
}

fn basic_footer(ui: &mut egui::Ui, t: &ApiTest, out: &mut Vec<Msg>) {
    let preset = t.preset.clone();
    let idle = !t.running && !t.listing;
    let finished = t.finished();
    let report = t.done.as_ref().and_then(|(_, r, _)| r.clone());
    let zip = t.done.as_ref().and_then(|(_, _, z)| z.clone());
    let failed = finished
        && (t.stopped
            || t.error.is_some()
            || t.ran.iter().any(|&i| {
                t.cases[i].case.not_applicable.is_none()
                    && !matches!(t.cases[i].outcome, Some(Outcome::Pass | Outcome::Warn))
            }));
    let (left, right) = dialog_footer(
        ui,
        |ui| {
            let mut out = vec![];
            if ui
                .add_enabled(idle, egui::Button::new(ic(ph::SLIDERS_HORIZONTAL, "Advanced view…")))
                .on_hover_text(
                    "every case of the suite as a tree, each one's log and the requests it made; run any of them",
                )
                .clicked()
            {
                out.push(Msg::ApiTestAdvanced(true));
            }
            if finished {
                if ui
                    .add_enabled(report.is_some(), egui::Button::new(ic(ph::COPY, "Copy report")))
                    .on_hover_text("every check, what it expects, its log and the requests it made: to send, or to paste to an LLM")
                    .clicked()
                    && let Some(r) = &report
                {
                    match std::fs::read_to_string(r.join("report.md")) {
                        Ok(text) => ui.ctx().copy_text(text),
                        Err(e) => ui.ctx().copy_text(format!("{}: {e}", r.join("report.md").display())),
                    }
                }
                if ui
                    .add_enabled(zip.is_some(), egui::Button::new(ic(ph::FILE_ZIP, "Show report zip")))
                    .on_hover_text("the report, the requests and the started servers' logs, zipped: to attach")
                    .clicked()
                    && let Some(z) = &zip
                {
                    out.push(Msg::ShowInFileManager(z.clone()));
                }
            }
            out
        },
        |ui| {
            let mut out = vec![];
            if t.running {
                let text = if t.stopping { "Stopping…" } else { "Stop" };
                let stop = ui
                    .add_enabled(!t.stopping, egui::Button::new(ic(ph::STOP, text)))
                    .on_hover_text("ends the check going on, then stops the servers it started");
                if stop.clicked() {
                    out.push(Msg::ApiTestStop);
                }
                return out;
            }
            if ui
                .add_enabled(idle, egui::Button::new(ic(ph::X, "Close")))
                .on_hover_text("back to the Servers screen")
                .clicked()
            {
                out.push(Msg::ApiTest(false));
            }
            let Some(p) = preset.filter(|_| idle && !t.cases.is_empty()) else { return out };
            if finished {
                if ui.button(ic(ph::ARROW_CLOCKWISE, "Run again")).clicked() {
                    out.push(Msg::ValidateBasic(p.clone()));
                }
                let mut label =
                    RichText::new(ic(ph::PLAY, if failed { "Open anyway" } else { "Open the preset" })).strong();
                if !failed {
                    label = label.color(ACCENT);
                }
                if ui.button(label).on_hover_text(format!("closes this and opens {p}")).clicked() {
                    out.push(Msg::ApiTestContinue);
                }
            } else {
                if ui.button("Skip and open").on_hover_text(format!("opens {p} without validating it")).clicked() {
                    out.push(Msg::ApiTestContinue);
                }
                let start = RichText::new(ic(ph::PLAY, "Start validation")).strong().color(ACCENT);
                if ui.add_enabled(!t.basic_cases().is_empty(), egui::Button::new(start)).clicked() {
                    out.push(Msg::ValidateBasic(p));
                }
            }
            out
        },
    );
    out.extend(left);
    out.extend(right);
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
            if ui
                .add_enabled(idle, egui::Button::new(ic(ph::LIST_CHECKS, "Basic view")))
                .on_hover_text("a preset's validation step by step, and a summary at the end")
                .clicked()
            {
                out.push(Msg::ApiTestAdvanced(false));
            }
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
