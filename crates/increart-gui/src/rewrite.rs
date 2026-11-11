//! The rewrite / describe window: the PE server's rewrite of the Generate
//! prompt or an idea's text, shown as a word diff against the original, or
//! its description of an image. The answer can be edited, asked for again
//! with instructions, used, kept as an idea's variant, or discarded.

use eframe::egui::{self, Color32, RichText, ScrollArea, text::LayoutJob};
use increart_core::pe::{Diff, lost_tags, word_diff};
use increart_gui_core::{AppState, Msg, PeJobState, PeKey, PeStatus, PeStep, PeTarget, PeUse};

use crate::textures::Textures;
use crate::widgets::{BAD, GOOD, ic, ph, thumb};

pub fn window(ctx: &egui::Context, st: &AppState, tex: &mut Textures, out: &mut Vec<Msg>) {
    let Some(t) = &st.pe.text else { return };
    if let Some(d) = &t.develop {
        return develop_window(ctx, st, tex, t, d, out);
    }
    let describe = matches!(t.target, PeTarget::Describe(_));
    let title = match &t.target {
        PeTarget::Prompt => "Rewrite the prompt",
        PeTarget::NewIdea | PeTarget::Idea(_) => "Rewrite the idea",
        PeTarget::Describe(_) => "Describe the image",
    };
    let server = st.pe.server.as_ref().map_or("PE", |s| s.name.as_str());
    let mut open = true;
    let (mut how, mut text) = (t.how.clone(), t.text.clone());
    egui::Window::new(ic(ph::MAGIC_WAND, title))
        .id(egui::Id::unique("pe_text"))
        .open(&mut open)
        .default_width(620.0)
        // clear of the Generate prompt, which it's often about
        .pivot(egui::Align2::CENTER_TOP)
        .default_pos(ctx.content_rect().center_top() + egui::vec2(160.0, 90.0))
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            if let (PeTarget::Describe(img), Some(root)) = (&t.target, st.library.as_deref()) {
                let th = tex.thumb(root, img);
                thumb(ui, th.as_ref(), 128.0, false, None);
            }
            ui.horizontal(|ui| {
                ui.label("Instructions");
                let hint = if describe {
                    "optional: e.g. focus on the lighting, as a watercolour"
                } else {
                    "optional: e.g. more cinematic, shorter, keep it 1920s"
                };
                let r = ui.add(egui::TextEdit::singleline(&mut how).hint_text(hint).desired_width(360.0));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let again = if describe { "Describe again" } else { "Rewrite again" };
                let b = ui.add_enabled(t.waiting.is_none(), egui::Button::new(ic(ph::ARROW_CLOCKWISE, again)));
                if (b.clicked() || enter) && t.waiting.is_none() {
                    // the instructions go with the ask
                    out.push(Msg::PeTextEdit { how: how.clone(), text: text.clone() });
                    out.push(Msg::PeTextRetry);
                }
            });
            if !describe {
                for_model(ui, st, t, out);
            }
            let job = t.waiting.and_then(|id| st.pe.job(id));
            // the answer as it's written
            let live = match job.map(|j| &j.step) {
                Some(PeStep::Writing { text, .. }) => Some(text.as_str()),
                _ => None,
            };
            if let Some(j) = job {
                ui.horizontal(|ui| {
                    ui.spinner();
                    if ui.button(ic(ph::STOP, "Stop")).on_hover_text("stop asking").clicked() {
                        out.push(Msg::PeStop(vec![j.id]));
                    }
                    ui.label(format!("{server}: {}", progress(st, j)));
                });
            } else if let Some(e) = &t.error {
                ui.colored_label(BAD, e);
            }
            if !describe {
                ui.add_space(4.0);
                ui.label(RichText::new("Changes").strong());
                ScrollArea::vertical().id_salt("pe_diff").max_height(160.0).show(ui, |ui| match live {
                    _ if !text.is_empty() => _ = ui.label(diff_job(ui, &t.original, &text)),
                    Some(live) => _ = ui.label(diff_job(ui, &t.original, live)),
                    None => _ = ui.weak(&t.original),
                });
            }
            if let (Some(live), true) = (live, text.is_empty()) {
                ui.add_space(4.0);
                ui.label(RichText::new(if describe { "Description" } else { "Rewritten" }).strong());
                ScrollArea::vertical().id_salt("pe_answer").max_height(220.0).stick_to_bottom(true).show(ui, |ui| {
                    let mut shown = live.to_string();
                    ui.add(
                        egui::TextEdit::multiline(&mut shown)
                            .interactive(false)
                            .desired_rows(5)
                            .desired_width(f32::INFINITY),
                    );
                });
            } else if !text.is_empty() || t.waiting.is_none() {
                ui.add_space(4.0);
                let label = if describe { "Description" } else { "Rewritten" };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).strong());
                    if text != t.answer {
                        ui.weak("(edited)");
                        if ui.small_button(ic(ph::ARROW_COUNTER_CLOCKWISE, "As answered")).clicked() {
                            text = t.answer.clone();
                        }
                    }
                });
                ScrollArea::vertical().id_salt("pe_answer").max_height(220.0).show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut text).desired_rows(5).desired_width(f32::INFINITY));
                });
                let lost = if text.is_empty() { vec![] } else { lost_tags(&t.original, &text) };
                if !lost.is_empty() {
                    ui.colored_label(BAD, format!("{} {} no longer in it", ph::WARNING, lost.join(", ")));
                }
            }
            ui.separator();
            use_buttons(ui, t, &how, &text, out);
        });
    if !open {
        out.push(Msg::PeTextClose(PeUse::Discard));
    } else if (how.as_str(), text.as_str()) != (t.how.as_str(), t.text.as_str()) {
        out.push(Msg::PeTextEdit { how, text });
    }
}

/// Where an ask is, with its time: "queued, 2 ahead", "reading the prompt
/// and image… 9 s (13 s last time)", "writing… ~84 tokens, 6.2/s".
/// What an ask is for: "values for {material}", "Duotone's prompt".
pub fn ask_name(st: &AppState, j: &PeJobState) -> String {
    match &j.key {
        PeKey::Blank(b) => match &b.style {
            Some(s) => format!("values for {{{}}} of {}", b.slot, s.name),
            None => format!("values for {{{}}}", b.slot),
        },
        PeKey::Style(k) => format!("{}'s prompt", k.name),
        PeKey::Text(_) => match st.pe.text.as_ref().map(|t| &t.target) {
            Some(PeTarget::Describe(_)) => "describing an image".into(),
            _ => "a rewrite".into(),
        },
    }
}

pub fn progress(st: &AppState, j: &PeJobState) -> String {
    let now = step_text(st, j);
    match &j.on {
        // several servers: which one
        Some(on) if st.pe.pool_size() > 1 && !matches!(j.step, PeStep::Queued) => format!("{now} on {on}"),
        _ => now,
    }
}

fn step_text(st: &AppState, j: &PeJobState) -> String {
    let secs = |since: f64| (st.now - since).max(0.0);
    let last = |t: Option<f64>| t.map_or(String::new(), |t| format!(" ({t:.0} s last time)"));
    match &j.step {
        PeStep::Queued if st.pe.status == PeStatus::Starting => starting(st),
        PeStep::Queued => match st.pe.ahead(j.id) {
            0 => "about to ask".into(),
            n => format!("queued, {n} ahead"),
        },
        PeStep::Preparing => "preparing the images…".into(),
        PeStep::Reading { images } => {
            let what = match images {
                0 => "the prompt",
                1 => "the prompt and image",
                _ => "the prompt and images",
            };
            format!("reading {what}… {:.0} s{}", secs(j.since), last(st.pe.read_times.get(images).copied()))
        }
        PeStep::Writing { pieces, .. } => {
            let rate = j.rate.map_or(String::new(), |(_, r)| format!(", {r:.1}/s"));
            format!("writing… ~{pieces} tokens{rate}")
        }
    }
}

/// The PE server's state in a few words, for the top bar while it starts
/// or works ("writing 143 tok (+1)"); `None` when it does neither. The
/// rest is in `progress` and `starting`.
pub fn brief(st: &AppState) -> Option<String> {
    let secs = |since: f64| (st.now - since).max(0.0);
    if st.pe.status == PeStatus::Starting {
        return Some(format!("loading {:.0} s", secs(st.pe.starting_since)));
    }
    let j = st.pe.working().filter(|_| st.pe.status == PeStatus::Ready)?;
    let now = match &j.step {
        PeStep::Queued => "queued".into(),
        PeStep::Preparing => "preparing".into(),
        PeStep::Reading { .. } => format!("reading {:.0} s", secs(j.since)),
        PeStep::Writing { pieces, .. } => format!("writing {pieces} tok"),
    };
    let more = st.pe.jobs.len() - 1;
    Some(if more > 0 { format!("{now} (+{more})") } else { now })
}

/// "starting… 23 s (loading the model; 58 s last time)".
pub fn starting(st: &AppState) -> String {
    let last = st.pe.last_load.map_or(String::new(), |t| format!("; {t:.0} s last time"));
    format!("starting… {:.0} s (loading the model{last})", (st.now - st.pe.starting_since).max(0.0))
}

/// `a` to `b` word by word: removed words struck through in red, added
/// ones green.
fn diff_job(ui: &egui::Ui, a: &str, b: &str) -> LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let plain = ui.visuals().text_color();
    let mut job = LayoutJob::default();
    for (kind, w) in word_diff(a, b) {
        let mut fmt = egui::TextFormat { font_id: font.clone(), color: plain, ..Default::default() };
        match kind {
            Diff::Same => {}
            Diff::Added => {
                fmt.color = GOOD;
                fmt.background = Color32::from_rgba_unmultiplied(60, 160, 80, 40);
            }
            Diff::Removed => {
                fmt.color = BAD;
                fmt.strikethrough = egui::Stroke::new(1.0, BAD);
            }
        }
        job.append(w, 0.0, fmt);
    }
    job.wrap.max_width = ui.available_width();
    job
}

/// The Rewrite button for a text: enabled with a PE server set up, and
/// showing the window's state while it's for this text.
pub fn button(ui: &mut egui::Ui, st: &AppState, target: PeTarget, label: &str, out: &mut Vec<Msg>) {
    let Some(server) = st.pe.server.as_ref() else {
        ui.add_enabled(false, egui::Button::new(ic(ph::MAGIC_WAND, label)))
            .on_disabled_hover_text("Needs a prompt-enhance server: add one under pe in config.rsn");
        return;
    };
    let busy = st.pe.text.as_ref().is_some_and(|t| t.target == target && t.waiting.is_some());
    let label = if busy { format!("{label}…") } else { label.to_string() };
    let hint = if server.vision() {
        format!("Ask {} for a better prompt (it sees the images too), to look over before using", server.name)
    } else {
        format!("Ask {} for a better prompt, to look over before using", server.name)
    };
    if ui.button(ic(ph::MAGIC_WAND, &label)).on_hover_text(hint).clicked() {
        out.push(Msg::PeRewrite(target));
    }
}

/// **Write for**: IncreArt's general rewrite or MiniMax-H3-Image's (long English
/// captions, references as numbered pictures); a change asks again.
fn for_model(ui: &mut egui::Ui, st: &AppState, t: &increart_gui_core::PeText, out: &mut Vec<Msg>) {
    use increart_core::pe::{REWRITE, REWRITE_H3};
    ui.horizontal(|ui| {
        ui.label("Write for");
        let idle = t.waiting.is_none();
        let mut pick = |ui: &mut egui::Ui, job: &'static str, label: &str, hint: &str| {
            let r = ui.add_enabled(idle, egui::Button::selectable(t.job == job, label)).on_hover_text(hint);
            if r.clicked() && t.job != job {
                out.push(Msg::PeTextJob(job));
            }
        };
        pick(ui, REWRITE, "Any model", "the general rewrite: clearer, more specific and more visual");
        pick(
            ui,
            REWRITE_H3,
            "MiniMax-H3-Image",
            "one detailed English caption of the finished image, as MiniMax-H3-Image's own prompt \
             enhancer writes them; edits name what they keep from each image (<Subject 1>…), say how \
             each is kept, then describe only what changes",
        );
        if st.h3_form() && t.job != REWRITE_H3 {
            ui.weak("(the Generate form's model is MiniMax-H3-Image)");
        }
    });
}

/// The window's last row: use the text (as the prompt, an idea's text or
/// one of its variants), copy it, or discard it.
fn use_buttons(ui: &mut egui::Ui, t: &increart_gui_core::PeText, how: &str, text: &str, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        let ready = !text.trim().is_empty();
        let (use_label, use_hint) = match &t.target {
            PeTarget::Prompt => ("Use as the prompt", "the prompt before goes in its history"),
            PeTarget::Describe(_) => ("Use as the Generate prompt", "the prompt before goes in its history"),
            _ => ("Use as the text", "the text before goes in its history"),
        };
        let edit = || Msg::PeTextEdit { how: how.to_string(), text: text.to_string() };
        if ui
            .add_enabled(ready, egui::Button::new(RichText::new(ic(ph::CHECK, use_label)).strong()))
            .on_hover_text(use_hint)
            .clicked()
        {
            out.push(edit());
            out.push(Msg::PeTextClose(PeUse::Use));
        }
        if matches!(t.target, PeTarget::Idea(_))
            && ui
                .add_enabled(ready, egui::Button::new(ic(ph::STACK, "Keep as a variant")))
                .on_hover_text("added to the idea's variants; its text stays as it is")
                .clicked()
        {
            out.push(edit());
            out.push(Msg::PeTextClose(PeUse::Variant));
        }
        if ui.add_enabled(ready, egui::Button::new(ic(ph::COPY, "Copy"))).clicked() {
            ui.ctx().copy_text(text.to_string());
            out.push(Msg::Info("Copied to the clipboard".into()));
        }
        if ui.button("Discard").clicked() {
            out.push(Msg::PeTextClose(PeUse::Discard));
        }
    });
}

/// The images that go with a text: the Generate form's references, or the
/// idea's images.
fn target_images(st: &AppState, target: &PeTarget) -> Vec<increart_core::model::ImageRef> {
    use increart_core::model::ImageRef;
    let inputs = |ids: &[increart_core::Id]| ids.iter().cloned().map(ImageRef::input).collect();
    match target {
        PeTarget::Prompt => st.generate.refs().to_vec(),
        PeTarget::NewIdea => inputs(&st.draft.images),
        PeTarget::Idea(id) => match &st.idea_edit {
            Some(ed) if ed.id == *id => inputs(&ed.images),
            _ => st.idea(id).map(|e| inputs(&e.idea.images)).unwrap_or_default(),
        },
        PeTarget::Describe(img) => vec![img.clone()],
    }
}

/// The Develop idea window: the idea, the conversation with the chat model
/// (its questions, the answers, the prompts it wrote), the reply to send,
/// and the last prompt to look over and use.
fn develop_window(
    ctx: &egui::Context,
    st: &AppState,
    tex: &mut Textures,
    t: &increart_gui_core::PeText,
    d: &increart_gui_core::Develop,
    out: &mut Vec<Msg>,
) {
    use increart_core::pe::DevelopReply;
    let server = st.pe.server.as_ref().map_or("PE", |s| s.name.as_str());
    let mut open = true;
    let (mut idea, mut reply, mut text) = (d.idea.clone(), d.reply.clone(), t.text.clone());
    let job = t.waiting.and_then(|id| st.pe.job(id));
    let idle = job.is_none();
    // the conversation's height: what the screen leaves after the rest
    let chat_height = (ctx.content_rect().height() - 560.0).clamp(140.0, 360.0);
    // a modal: closed by its × or Esc, not a click beside it (that would lose
    // the conversation)
    egui::Modal::new(egui::Id::unique("pe_develop")).show(ctx, |ui| {
        ui.set_width(660.0);
        ui.horizontal(|ui| {
            ui.heading(ic(ph::CHATS_CIRCLE, "Develop an idea"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let hint = match t.target {
                    PeTarget::Prompt => "close (the conversation is kept with the draft)",
                    _ => "close, discarding the conversation",
                };
                if ui.button(ph::X).on_hover_text(hint).clicked() {
                    open = false;
                }
            });
        });
        ui.weak(format!(
            "Describe what you have in mind; {server} asks about what's unclear, then writes the prompt \
                 by the guide for the model. Answer, or ask for changes, as often as you like."
        ));
        for_model(ui, st, t, out);
        ui.horizontal(|ui| {
            ui.label(RichText::new(ic(ph::LIGHTBULB, "Idea")).strong());
            let refs = target_images(st, &t.target);
            if !refs.is_empty()
                && let Some(root) = st.library.as_deref()
            {
                ui.weak("with");
                for (i, r) in refs.iter().enumerate() {
                    let th = tex.thumb(root, r);
                    thumb(ui, th.as_ref(), 28.0, false, None).on_hover_text(format!("<image{}>", i + 1));
                }
            }
            if !d.turns.is_empty()
                && ui
                    .add_enabled(idle, egui::Button::new(ic(ph::ARROW_COUNTER_CLOCKWISE, "Start over")))
                    .on_hover_text("forget the conversation and ask again from the idea")
                    .clicked()
            {
                out.push(Msg::PeDevelopEdit { idea: idea.clone(), reply: reply.clone() });
                out.push(Msg::PeDevelopRestart);
            }
        });
        ui.add(
            egui::TextEdit::multiline(&mut idea)
                .hint_text("e.g. me from <image1> riding the dog from <image2> on a beach at dusk, film look")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        ui.separator();
        let bubble = |ui: &mut egui::Ui, who: String, mine: bool, body: &mut dyn FnMut(&mut egui::Ui)| {
            let fill = if mine { ui.visuals().faint_bg_color } else { ui.visuals().extreme_bg_color };
            let layout = if mine {
                egui::Layout::right_to_left(egui::Align::Min)
            } else {
                egui::Layout::left_to_right(egui::Align::Min)
            };
            ui.with_layout(layout, |ui| {
                egui::Frame::new().fill(fill).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
                    ui.set_max_width(ui.available_width() * 0.88);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(who).small().weak());
                        body(ui);
                    });
                });
            });
            ui.add_space(4.0);
        };
        ScrollArea::vertical().id_salt("pe_develop_chat").max_height(chat_height).stick_to_bottom(true).show(
            ui,
            |ui| {
                if d.turns.is_empty() && idle && t.error.is_none() {
                    ui.weak("Nothing asked yet: write the idea and press Start.");
                }
                let n = d.turns.len();
                for (i, turn) in d.turns.iter().enumerate() {
                    match &turn.said {
                        DevelopReply::Questions(q) => {
                            bubble(ui, format!("{} {server} asks", ph::ROBOT), false, &mut |ui| {
                                ui.label(q.as_str());
                            })
                        }
                        // the last is below, to edit and use; earlier ones
                        // folded, to look back at
                        DevelopReply::Prompt(_) if i + 1 == n => {
                            bubble(ui, format!("{} {server}", ph::ROBOT), false, &mut |ui| {
                                ui.label(ic(ph::ARROW_DOWN, "wrote the prompt: it's below, to edit and use"));
                            })
                        }
                        DevelopReply::Prompt(p) => bubble(ui, format!("{} {server}", ph::ROBOT), false, &mut |ui| {
                            egui::CollapsingHeader::new("wrote a prompt")
                                .id_salt(("pe_develop_prompt_turn", i))
                                .show(ui, |ui| ui.label(p.as_str()));
                        }),
                    }
                    if !turn.answer.is_empty() {
                        bubble(ui, format!("{} You", ph::USER), true, &mut |ui| {
                            ui.label(turn.answer.as_str());
                        });
                    }
                }
                if let Some(j) = job {
                    let live = match &j.step {
                        PeStep::Writing { text, .. } => Some(text.clone()),
                        _ => None,
                    };
                    bubble(ui, format!("{} {server}", ph::ROBOT), false, &mut |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.weak(progress(st, j));
                        });
                        if let Some(live) = &live {
                            ui.label(live.as_str());
                        }
                    });
                }
                if let Some(e) = &t.error {
                    ui.colored_label(BAD, e);
                }
            },
        );
        let last = d.turns.last();
        let asked = matches!(last, Some(l) if matches!(l.said, DevelopReply::Questions(_)) && l.answer.is_empty());
        let wrote = matches!(last, Some(l) if matches!(l.said, DevelopReply::Prompt(_)));
        let hint = if asked {
            "your answers (anything left out: its suggestion is used)"
        } else if wrote {
            "changes to make, e.g. warmer light, closer framing, no text"
        } else {
            "anything to add (optional)"
        };
        ui.add_space(2.0);
        let r = ui.add_enabled(
            idle,
            egui::TextEdit::multiline(&mut reply).hint_text(hint).desired_rows(2).desired_width(f32::INFINITY),
        );
        let ctrl_enter = r.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            let send_label = if d.turns.is_empty() { "Start" } else { "Send" };
            let ready = !idea.trim().is_empty() && (!wrote || !reply.trim().is_empty());
            let send = ui
                .add_enabled(
                    idle && ready,
                    egui::Button::new(RichText::new(ic(ph::PAPER_PLANE_RIGHT, send_label)).strong()),
                )
                .on_hover_text("send (Ctrl+Enter)");
            if (send.clicked() || (ctrl_enter && idle && ready)) && idle {
                out.push(Msg::PeDevelopEdit { idea: idea.clone(), reply: reply.clone() });
                out.push(Msg::PeDevelopSend { write_now: false });
            }
            if asked
                && ui
                    .add_enabled(idle, egui::Button::new(ic(ph::PEN_NIB, "Write the prompt now")))
                    .on_hover_text("no more questions: the answers given, suggestions for the rest")
                    .clicked()
            {
                out.push(Msg::PeDevelopEdit { idea: idea.clone(), reply: reply.clone() });
                out.push(Msg::PeDevelopSend { write_now: true });
            }
            if let Some(j) = job
                && ui.button(ic(ph::STOP, "Stop")).clicked()
            {
                out.push(Msg::PeStop(vec![j.id]));
            }
        });
        if !text.is_empty() {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Prompt").strong());
                if text != t.answer {
                    ui.weak("(edited)");
                    if ui.small_button(ic(ph::ARROW_COUNTER_CLOCKWISE, "As written")).clicked() {
                        text = t.answer.clone();
                    }
                }
            });
            ScrollArea::vertical().id_salt("pe_develop_prompt").max_height(160.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut text).desired_rows(4).desired_width(f32::INFINITY));
            });
            let lost = lost_tags(&idea, &text);
            if !lost.is_empty() {
                ui.colored_label(BAD, format!("{} {} no longer in it", ph::WARNING, lost.join(", ")));
            }
        }
        ui.separator();
        use_buttons(ui, t, &t.how, &text, out);
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    if !open {
        out.push(Msg::PeTextClose(PeUse::Discard));
        return;
    }
    if (idea.as_str(), reply.as_str()) != (d.idea.as_str(), d.reply.as_str()) {
        out.push(Msg::PeDevelopEdit { idea, reply });
    }
    if text != t.text {
        out.push(Msg::PeTextEdit { how: t.how.clone(), text });
    }
}

/// The **Develop idea…** button beside Rewrite: the conversation window
/// for this text (the Generate prompt's goes on where it was left).
pub fn develop_button(ui: &mut egui::Ui, st: &AppState, target: PeTarget, out: &mut Vec<Msg>) {
    let Some(server) = st.pe.server.as_ref() else {
        ui.add_enabled(false, egui::Button::new(ic(ph::CHATS_CIRCLE, "Develop idea…")))
            .on_disabled_hover_text("Needs a prompt-enhance server");
        return;
    };
    let what = if target == PeTarget::Prompt { "the idea in the prompt" } else { "the idea" };
    let mut hint = format!(
        "Talk {what} through with {}: it asks about what's unclear, then writes the prompt \
         the way the model reads best",
        server.name
    );
    if target == PeTarget::Prompt && st.generate.develop.is_some() {
        hint.push_str(" (goes on with the conversation kept with this draft)");
    }
    if ui.button(ic(ph::CHATS_CIRCLE, "Develop idea…")).on_hover_text(hint).clicked() {
        out.push(Msg::PeDevelop(target));
    }
}
