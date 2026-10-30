//! The Settings dialog (top bar ⚙) and the status bar's notification bell.

use eframe::egui::{self, RichText};
use increart_core::model::StoreFormat;
use increart_core::notify::{self, AlertRule, Notifications, Sound};
use increart_gui_core::state::SettingsTab;
use increart_gui_core::{AppState, Msg};

use crate::widgets::{BAD, GOOD, dialog_footer, ic, ph};

pub fn window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    if !st.settings_open {
        return;
    }
    let mut n = st.config.notifications.clone();
    // a modal (the rest dims); changes apply as they are made, so only Close
    let m = egui::Modal::new(egui::Id::unique("settings")).show(ctx, |ui| {
        ui.set_width(620.0);
        ui.heading(ic(ph::GEAR, "Settings"));
        ui.horizontal(|ui| {
            for (tab, icon, name) in [
                (SettingsTab::Notifications, ph::BELL, "Notifications"),
                (SettingsTab::Library, ph::BOOKS, "Library"),
                (SettingsTab::Timeouts, ph::TIMER, "Timeouts"),
            ] {
                if ui.selectable_label(st.settings_tab == tab, ic(icon, name)).clicked() && st.settings_tab != tab {
                    out.push(Msg::SettingsTab(tab));
                }
            }
        });
        ui.separator();
        match st.settings_tab {
            SettingsTab::Notifications => notifications(ui, st, &mut n, out),
            SettingsTab::Library => library(ui, st, out),
            SettingsTab::Timeouts => timeouts(ui, st, out),
        }
        ui.add_space(8.0);
        // the version, for bug reports
        dialog_footer(
            ui,
            |ui| {
                let v = crate::version();
                ui.label(RichText::new(&v).weak());
                if ui.small_button(ph::COPY).on_hover_text("Copy the version").clicked() {
                    ui.ctx().copy_text(v);
                }
            },
            |ui| ui.button("Close   Esc").clicked(),
        )
        .1
    });
    if n != st.config.notifications {
        out.push(Msg::SetNotifications(n));
    }
    if m.inner || m.should_close() {
        out.push(Msg::Settings(false));
    }
}

/// How long the servers may go quiet before they're given up on (here and
/// in Server settings): each a stall limit, never a cap on something slow
/// that keeps coming.
pub fn timeouts(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    use increart_core::config::Timeouts;
    ui.weak(
        "How long something may go without a sign of progress before it's given up on. \
         Slow is fine: a download, answer or load that keeps coming is never cut off.",
    );
    let mut t = st.config.timeouts;
    let rows: [(&str, &str, &mut u64); 4] = [
        (
            "Server start",
            "a server starting: no new line in its log (loading a big model from a slow disk can pause)",
            &mut t.server_start,
        ),
        ("Image server requests", "sending a job or reading a result: no data", &mut t.request),
        (
            "Prompt enhance answer",
            "no word yet, or none since the last (with images on a CPU the first can take minutes)",
            &mut t.chat,
        ),
        ("Model downloads", "no data (a broken download carries on where it stopped)", &mut t.download),
    ];
    egui::Grid::new("timeouts").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
        for (name, what, secs) in rows {
            ui.label(name);
            ui.add(egui::DragValue::new(secs).range(1..=86_400).speed(1.0).suffix(" s"));
            ui.add(egui::Label::new(RichText::new(what).weak()).wrap());
            ui.end_row();
        }
    });
    ui.horizontal(|ui| {
        let reset = egui::Button::new("Reset to defaults");
        if ui.add_enabled(t != Timeouts::DEFAULT, reset).clicked() {
            t = Timeouts::DEFAULT;
        }
        ui.weak("Used by servers and downloads started from now on.");
    });
    if t != st.config.timeouts {
        out.push(Msg::SetTimeouts(t));
    }
}

/// This library's own settings: how its images are stored.
fn library(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    if let Some(root) = &st.library {
        ui.weak(format!("For this library only ({})", root.display()));
    }
    ui.add_space(4.0);
    ui.label(RichText::new("Image storage").strong());
    let Some(info) = st.store else {
        ui.spinner();
        return;
    };
    let mut f = info.format;
    ui.add_enabled_ui(st.converting.is_none(), |ui| {
        ui.radio_value(&mut f, StoreFormat::Png, "PNG").on_hover_text("readable by every image program");
        ui.radio_value(&mut f, StoreFormat::Webp, "WebP, lossless")
            .on_hover_text("about half the size of PNG, the same pixels exactly; most image programs read it");
    });
    if f != info.format {
        out.push(Msg::SetStoreFormat(f));
    }
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.weak("New images are stored this way. Images already stored stay as they are until you convert them.");
    });
    ui.add_space(8.0);
    ui.label(RichText::new("Stored now").strong());
    let c = info.counts;
    for (name, (n, bytes)) in [("PNG", c.png), ("WebP", c.webp)] {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(format!("{name}: {n} image(s), {}", size(bytes)));
        });
    }
    let others = match info.format {
        StoreFormat::Png => c.webp.0,
        StoreFormat::Webp => c.png.0,
    };
    ui.add_space(4.0);
    if let Some((done, total)) = st.converting {
        ui.horizontal(|ui| {
            let progress = if total == 0 { 0.0 } else { done as f32 / total as f32 };
            ui.add(
                egui::ProgressBar::new(progress)
                    .desired_width(380.0)
                    .text(format!("converting to {}: {done} of {total}", info.format.name())),
            );
            if ui.button("Stop").on_hover_text("after the images being converted now").clicked() {
                out.push(Msg::CancelConvert);
            }
        });
    } else if others > 0 {
        let label = format!("Convert {others} image(s) to {}…", info.format.name());
        if ui.button(ic(ph::ARROWS_CLOCKWISE, &label)).clicked() {
            out.push(Msg::ConvertStore);
        }
    } else {
        ui.weak(format!("Every image is stored as {}.", info.format.name()));
    }
    if info.format == StoreFormat::Webp || c.webp.0 > 0 {
        ui.add_space(4.0);
        ui.weak("Older versions of IncreArt can't open a library holding WebP images.");
    }
}

fn size(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.2} GB", n as f64 / (1 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
        n => format!("{:.0} KB", n as f64 / (1 << 10) as f64),
    }
}

fn notifications(ui: &mut egui::Ui, st: &AppState, n: &mut Notifications, out: &mut Vec<Msg>) {
    ui.label(RichText::new("GPU idle").strong());
    let gpus = if n.all_gpus { "every GPU" } else { "a GPU" };
    rule(ui, "idle", &mut n.gpu_idle, &format!("Sound when {gpus} has had no job for"), "min", out);
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.radio_value(&mut n.all_gpus, false, "any GPU idle");
        ui.radio_value(&mut n.all_gpus, true, "all GPUs idle");
    });
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.checkbox(&mut n.pe_counts_as_work, "Prompt enhance asks count as work").on_hover_text(
            "while prompt enhance is asking nothing counts as idle, and idle time counts again from its last ask",
        );
    });
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.weak("Counts only while servers are connected and ready (not while loading a model).");
    });
    ui.add_space(8.0);
    ui.label(RichText::new("GPU queue cleared").strong());
    ui.horizontal(|ui| {
        let what = if n.queue_cleared_all { "the last busy GPU" } else { "a GPU" };
        ui.checkbox(&mut n.queue_cleared.on, format!("Sound at once when {what} finishes its last job"));
        ui.add_enabled_ui(n.queue_cleared.on, |ui| {
            ui.label("· sound");
            sound_pick(ui, "cleared", &mut n.queue_cleared.sound);
            if ui.small_button(ph::PLAY).on_hover_text("play it").clicked() {
                out.push(Msg::TestSound(n.queue_cleared.sound.clone()));
            }
        });
    });
    ui.add_enabled_ui(n.queue_cleared.on, |ui| {
        if let Sound::File(f) = &mut n.queue_cleared.sound {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.label("file");
                ui.add(egui::TextEdit::singleline(f).hint_text("/path/to/sound.wav").desired_width(380.0));
            });
        }
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.radio_value(&mut n.queue_cleared_all, false, "each GPU");
            ui.radio_value(&mut n.queue_cleared_all, true, "only when all GPUs are done");
        });
    });
    ui.add_space(8.0);
    ui.label(RichText::new("Review backlog").strong());
    rule(ui, "backlog", &mut n.review_backlog, "Sound when more images than", "wait for review", out);
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.weak(format!("now: {} (the Review tab's count)", st.unreviewed_outputs()));
    });
    ui.separator();

    ui.label(RichText::new("Sound").strong());
    ui.horizontal(|ui| {
        ui.label("Volume");
        let mut pct = (n.volume * 100.0).round() as u32;
        if ui.add(egui::Slider::new(&mut pct, 0..=100).suffix(" %")).changed() {
            n.volume = pct as f32 / 100.0;
        }
    });
    player(ui, n);
    ui.horizontal(|ui| {
        if ui.button(ic(ph::SPEAKER_HIGH, "Test")).on_hover_text("plays the GPU idle sound now").clicked() {
            out.push(Msg::TestSound(n.gpu_idle.sound.clone()));
        }
        match &st.alerts.test_result {
            Some(Ok(p)) => {
                ui.colored_label(GOOD, format!("played with {p}"));
            }
            Some(Err(e)) => {
                ui.colored_label(BAD, e);
            }
            None => {}
        }
    });
    ui.separator();
    ui.checkbox(&mut n.only_unfocused, "Only when IncreArt isn't the active window");
    ui.checkbox(&mut n.flash_taskbar, "Also ask for attention (taskbar flash)");
    ui.checkbox(&mut n.show_message, "Also say why in the status bar");
    ui.weak("Snooze from the bell in the status bar. Settings are saved in config.rsn.");
}

/// An alert's on/threshold/repeat/sound row.
fn rule(ui: &mut egui::Ui, id: &str, r: &mut AlertRule, what: &str, unit: &str, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut r.on, what);
        ui.add_enabled(r.on, egui::DragValue::new(&mut r.threshold).range(1..=100_000));
        ui.label(unit);
    });
    ui.add_enabled_ui(r.on, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label("repeat every");
            ui.add(egui::DragValue::new(&mut r.repeat_minutes).range(1..=1440));
            ui.label("min while it lasts · sound");
            sound_pick(ui, id, &mut r.sound);
            if ui.small_button(ph::PLAY).on_hover_text("play it").clicked() {
                out.push(Msg::TestSound(r.sound.clone()));
            }
        });
        if let Sound::File(f) = &mut r.sound {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.label("file");
                ui.add(egui::TextEdit::singleline(f).hint_text("/path/to/sound.wav").desired_width(380.0));
            });
        }
    });
}

fn sound_pick(ui: &mut egui::Ui, id: &str, s: &mut Sound) {
    let label = if matches!(s, Sound::File(_)) { "file…".to_string() } else { s.name() };
    egui::ComboBox::from_id_salt(("sound", id)).selected_text(label).show_ui(ui, |ui| {
        for b in Sound::BUILT_IN {
            let name = b.name();
            ui.selectable_value(s, b, name);
        }
        if ui.selectable_label(matches!(s, Sound::File(_)), "a file of your own…").clicked()
            && !matches!(s, Sound::File(_))
        {
            *s = Sound::File(String::new());
        }
    });
}

/// The player: automatic for this OS, a ready-made command, or your own.
fn player(ui: &mut egui::Ui, n: &mut Notifications) {
    let preset = notify::PLAYERS.iter().find(|(_, c)| *c == n.player).map(|(name, _)| *name);
    let current = if n.player.is_empty() {
        "automatic".to_string()
    } else {
        preset.map_or("your own command".to_string(), String::from)
    };
    ui.horizontal(|ui| {
        ui.label("Player");
        egui::ComboBox::from_id_salt("player").selected_text(current).width(300.0).show_ui(ui, |ui| {
            if ui.selectable_label(n.player.is_empty(), "automatic").clicked() {
                n.player.clear();
            }
            for (name, cmd) in notify::PLAYERS {
                if ui.selectable_label(n.player == *cmd, *name).clicked() {
                    n.player = cmd.to_string();
                }
            }
            if ui.selectable_label(!n.player.is_empty() && preset.is_none(), "your own command…").clicked()
                && (n.player.is_empty() || preset.is_some())
            {
                n.player = format!("{} ", notify::auto_players().first().copied().unwrap_or("player {file}"));
            }
        });
    });
    if n.player.is_empty() {
        let tries: Vec<_> = notify::auto_players().iter().filter_map(|c| c.split_whitespace().next()).collect();
        ui.weak(format!("Tries {} on this system, the first one found.", tries.join(", ")));
    } else {
        ui.horizontal(|ui| {
            ui.label("Command");
            ui.add(egui::TextEdit::singleline(&mut n.player).font(egui::TextStyle::Monospace).desired_width(470.0));
        });
        if let Err(e) = notify::split_command(&n.player) {
            ui.colored_label(BAD, e);
        }
    }
    ui.weak(
        "Placeholders: {file}, {volume} (0.00–1.00), {volume_percent}, {volume_pa} (0–65536). \
         The built-in sounds are made at the chosen volume (their {volume} is 1.00).",
    );
}

const OPEN_BELL: &str = "open_bell";

/// Opens the status bar's bell menu (its snooze) on the next frame.
pub fn open_bell(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(OPEN_BELL), true));
    ctx.request_repaint();
}

/// Settings, on its Notifications tab.
pub fn notification_settings() -> [Msg; 2] {
    [Msg::Settings(true), Msg::SettingsTab(increart_gui_core::state::SettingsTab::Notifications)]
}

/// An alert message's links: the bell's snooze menu and the settings.
pub fn alert_links(ui: &mut egui::Ui, out: &mut Vec<Msg>) {
    if ui.small_button(ic(ph::BELL_Z, "Snooze…")).on_hover_text("the bell's snooze menu").clicked() {
        egui::Popup::close_all(ui.ctx());
        open_bell(ui.ctx());
    }
    if ui.small_button(ph::GEAR).on_hover_text("notification settings").clicked() {
        egui::Popup::close_all(ui.ctx());
        out.extend(notification_settings());
    }
}

/// The status bar's bell: alerts on, off, sounding or snoozed; click for
/// what holds, snooze and the settings.
/// `force_open`: for the screenshot tour.
pub fn bell(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>, force_open: bool) {
    let n = &st.config.notifications;
    if !n.gpu_idle.on && !n.queue_cleared.on && !n.review_backlog.on {
        if ui.small_button(ph::BELL_SLASH).on_hover_text("No notifications: click to set them up").clicked() {
            out.extend(notification_settings());
        }
        return;
    }
    let a = &st.alerts;
    let holding: Vec<&String> = [&a.idle_text, &a.cleared_text, &a.backlog_text].into_iter().flatten().collect();
    let snoozed = a.snoozed(st.now);
    // while an alert holds (and isn't snoozed) the button is filled solid,
    // so it's seen from across the room; back to plain once nothing holds
    let ringing = !snoozed && !holding.is_empty();
    let (icon, color, fill) = if a.sound_error.is_some() {
        (ph::WARNING, egui::Color32::BLACK, Some(BAD))
    } else if snoozed {
        (ph::BELL_Z, ui.visuals().weak_text_color(), None)
    } else if ringing {
        (ph::BELL_RINGING, egui::Color32::BLACK, Some(crate::widgets::NEUTRAL))
    } else {
        (ph::BELL, ui.visuals().text_color(), None)
    };
    let mut button = egui::Button::new(RichText::new(icon).color(color).strong()).small();
    if let Some(f) = fill {
        button = button.fill(f);
    }
    let tip = match holding.first() {
        Some(h) if ringing => format!("Notifications: {h}"),
        _ => "Notifications".to_string(),
    };
    let r = ui.add(button).on_hover_text(tip);
    // closed by the buttons themselves: a click in the minutes field keeps it open
    let mut popup = egui::Popup::menu(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    // asked for by an alert's message (see `open_bell`)
    let asked = ui.ctx().data_mut(|d| d.remove_temp::<bool>(egui::Id::unique(OPEN_BELL))).unwrap_or(false);
    if force_open || asked {
        popup = popup.open_memory(egui::SetOpenCommand::Bool(true));
    }
    popup.show(|ui| {
        ui.set_min_width(260.0);
        if holding.is_empty() {
            ui.weak("Nothing to report.");
        }
        for h in &holding {
            ui.label(*h);
        }
        if let Some(e) = &a.sound_error {
            ui.colored_label(BAD, format!("sound: {e}"));
        }
        ui.separator();
        if snoozed {
            let until = if a.muted {
                "until IncreArt restarts".to_string()
            } else {
                let left = ((a.snooze_until.unwrap_or(0.0) - st.now) / 60.0).ceil() as u64;
                match left {
                    0..60 => format!("for {left} more min"),
                    _ => format!("for {} h {:02} min more", left / 60, left % 60),
                }
            };
            ui.weak(format!("Snoozed {until}"));
            if ui.button(ic(ph::BELL, "Unsnooze")).clicked() {
                out.push(Msg::Unsnooze);
                ui.close();
            }
        } else {
            let snooze = |ui: &mut egui::Ui, label: &str| ui.button(ic(ph::BELL_Z, label)).clicked();
            let mut picked = None;
            if snooze(ui, "Snooze 5 min") {
                picked = Some(Some(300.0));
            }
            if snooze(ui, "Snooze 30 min") {
                picked = Some(Some(1800.0));
            }
            if snooze(ui, "Snooze 1 h") {
                picked = Some(Some(3600.0));
            }
            // a duration of your own, in minutes (kept for the session); the
            // submenu stays open while the field is clicked and typed in
            let key = egui::Id::unique("snooze_custom_min");
            let mut min = ui.ctx().data(|d| d.get_temp::<u32>(key)).unwrap_or(15);
            let config =
                egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
            let mut custom = None;
            egui::containers::menu::SubMenuButton::new(ic(ph::BELL_Z, "Snooze for…")).config(config).ui(ui, |ui| {
                ui.horizontal(|ui| {
                    let field = ui.add(egui::DragValue::new(&mut min).range(1..=1440).suffix(" min"));
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let go = ui.button(ic(ph::BELL_Z, "Snooze")).clicked();
                    if go || enter {
                        custom = Some(min);
                        ui.close_kind(egui::UiKind::Menu);
                    }
                });
                ui.weak("1 min to 24 h; drag or type, Enter snoozes");
            });
            ui.ctx().data_mut(|d| d.insert_temp(key, min));
            if let Some(m) = custom {
                picked = Some(Some(m as f64 * 60.0));
            }
            if snooze(ui, "Until restart") {
                picked = Some(None);
            }
            if let Some(secs) = picked {
                out.push(Msg::Snooze(secs));
                ui.close();
            }
        }
        ui.separator();
        if ui.button(ic(ph::GEAR, "Notification settings…")).clicked() {
            out.extend(notification_settings());
            ui.close();
        }
    });
}
