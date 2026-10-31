//! The log viewer (**Logs…**): the app's own log and those of the servers
//! it started, in the log folder (DESIGN.md, Server logs).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, RichText, ScrollArea, Vec2};
use increart_core::logview::{self, Level, Row};
use increart_gui_core::{AppState, LaunchShown, LogsView, Msg};

use crate::widgets::{BAD, GOOD, NEUTRAL, dialog_footer, ic, ph};

/// How much of a log's end is read.
const TAIL: u64 = 1 << 20;
/// What **Save for a bug report…** saves (its hover).
const BUG_REPORT_NOTE: &str = "A zip to attach to an issue: your config (and the copies kept beside it), the end of each \
log here (passwords masked), the file names and sizes in your model directories, and the version and system. \
No images, no prompts, nothing from the library. Paths in it show your user name.";
/// Wrapped lines are laid out whole, so only this many of the last are shown.
const WRAPPED: usize = 3000;

/// What was read from the log folder, kept between frames.
#[derive(Default)]
pub struct Cache {
    /// (name, size, modified), newest first.
    files: Vec<(String, u64, Option<SystemTime>)>,
    listed: Option<Instant>,
    file: Option<String>,
    /// The size and time `lines` were read at.
    read: Option<(u64, Option<SystemTime>)>,
    checked: Option<Instant>,
    lines: Vec<String>,
    /// Read from partway in: the log is longer than [`TAIL`].
    cut: bool,
    /// The last launch line ([`increart_core::process::log_header`]), and
    /// how far the file was searched for it.
    launch: Option<String>,
    searched: u64,
    /// Following: the text area's end as last measured (and whether it was
    /// the wrapped one), kept to from the next frame.
    kept: Option<(bool, f32)>,
    /// Where each launch starts in `lines`.
    launches: Vec<usize>,
    /// The folds clicked open: their first line; `opened` counts the clicks.
    open: HashSet<String>,
    opened: usize,
    shown: Shown,
    /// The match gone to (of `Shown::found`); a new search goes to its last.
    find_at: usize,
    find_new: bool,
    /// An item to scroll to (a match gone to).
    jump: Option<usize>,
    /// Each log as last looked at (size, modified), and whether it has an
    /// error since its latest launch.
    errors: HashMap<String, (u64, Option<SystemTime>, bool)>,
}

impl Cache {
    fn refresh(&mut self, dir: &Path, file: Option<&str>) {
        if self.listed.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            self.listed = Some(Instant::now());
            self.files = std::fs::read_dir(dir)
                .map(|rd| {
                    rd.flatten()
                        .filter_map(|e| {
                            let name = e.file_name().to_string_lossy().into_owned();
                            let meta = e.metadata().ok()?;
                            (meta.is_file() && name.ends_with(".log")).then(|| (name, meta.len(), meta.modified().ok()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            self.files.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
            for (name, len, modified) in &self.files {
                if self.errors.get(name).is_none_or(|e| (e.0, e.1) != (*len, *modified)) {
                    let trouble = latest_has_error(&dir.join(name), *len);
                    self.errors.insert(name.clone(), (*len, *modified, trouble));
                }
            }
        }
        if self.file.as_deref() != file {
            (self.file, self.read, self.checked) = (file.map(str::to_string), None, None);
            self.lines.clear();
            self.launches.clear();
            (self.find_at, self.jump) = (0, None);
            (self.launch, self.searched) = (None, 0);
        }
        let Some(name) = file else { return };
        if self.checked.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
            return;
        }
        self.checked = Some(Instant::now());
        let path = dir.join(name);
        let Ok(meta) = std::fs::metadata(&path) else {
            self.lines = vec![format!("{} isn't there (yet).", path.display())];
            self.read = None;
            return;
        };
        let now = (meta.len(), meta.modified().ok());
        if self.read != Some(now) {
            self.read = Some(now);
            (self.lines, self.cut) = read_tail(&path, now.0);
            self.launches = logview::launches(&self.lines);
            if now.0 < self.searched {
                (self.launch, self.searched) = (None, 0); // begun again
            }
            self.searched = find_launch(&path, self.searched, &mut self.launch);
        }
    }
}

/// Following: the offset to keep the text area at, its end as last
/// measured (not yet measured: none, and it's measured this frame).
fn kept_at(view: &LogsView, kept: Option<(bool, f32)>) -> Option<f32> {
    kept.filter(|(wrap, _)| view.follow && *wrap == view.wrap).map(|(_, y)| y)
}

/// Following: measures where the text area `o` ends, to keep it there from
/// the next frame; true when the user scrolled it back from `kept_at`
/// (wheel, bar) this frame, which ends following. A shorter text (search)
/// brings the end up to the offset, which is no scrolling back.
fn measure_end<R>(
    ctx: &egui::Context,
    o: &egui::scroll_area::ScrollAreaOutput<R>,
    view: &LogsView,
    kept_at: Option<f32>,
    kept: &mut Option<(bool, f32)>,
) -> bool {
    if !view.follow {
        *kept = None;
        return false;
    }
    let (end, at) = ((o.content_size.y - o.inner_rect.height()).max(0.0), o.state.clamped_offset().y);
    if kept_at.is_some_and(|k| at < k - 1.0) && at < end - 1.0 {
        *kept = None;
        return true;
    }
    *kept = Some((view.wrap, end));
    if at != end {
        ctx.request_repaint();
    }
    false
}

/// The last [`TAIL`] bytes as lines, as a terminal shows them: a line
/// rewritten with `\r` (progress bars) as its last state, colour codes out.
fn read_tail(path: &Path, len: u64) -> (Vec<String>, bool) {
    use std::io::{Read, Seek, SeekFrom};
    let mut bytes = vec![];
    let from = len.saturating_sub(TAIL);
    let ok = std::fs::File::open(path)
        .and_then(|mut f| f.seek(SeekFrom::Start(from)).and_then(|_| f.read_to_end(&mut bytes)))
        .is_ok();
    if !ok {
        return (vec![format!("{} can't be read.", path.display())], false);
    }
    let text = String::from_utf8_lossy(&bytes);
    // a line rewritten: its time (from its start) and its last state
    let last = |l: &str| {
        let (time, rest) = logview::split_stamp(l.trim_end_matches('\r'));
        strip_ansi(&format!("{time}{}", rest.rsplit('\r').next().unwrap_or("")))
    };
    let mut lines: Vec<String> = text.split('\n').map(last).collect();
    if from > 0 {
        lines.remove(0); // begun mid-line
    }
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    (lines, from > 0)
}

/// Whether the log at `path` (`len` long) has an error since its latest
/// launch, in its last 256 KB.
fn latest_has_error(path: &Path, len: u64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut bytes = vec![];
    let read = std::fs::File::open(path).and_then(|mut f| {
        f.seek(SeekFrom::Start(len.saturating_sub(256 << 10))).and_then(|_| f.read_to_end(&mut bytes))
    });
    if read.is_err() {
        return false;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let from = logview::launches(&lines).last().copied().unwrap_or(0);
    lines[from..].iter().any(|l| logview::level(l) == Level::Error)
}

/// Reads `path` on from `from` for launch lines, the last into `launch`;
/// returns where its last whole line ends.
fn find_launch(path: &Path, from: u64, launch: &mut Option<String>) -> u64 {
    use std::io::{BufRead, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return from };
    if f.seek(SeekFrom::Start(from)).is_err() {
        return from;
    }
    let (mut at, mut line) = (from, vec![]);
    let mut r = std::io::BufReader::new(f);
    while let Ok(n) = r.read_until(b'\n', &mut line) {
        if n == 0 || line.last() != Some(&b'\n') {
            break;
        }
        at += n as u64;
        if line.starts_with(b"--- IncreArt, ") {
            let l = String::from_utf8_lossy(&line).trim_end().to_string();
            // a KoboldCpp's reloads are noted too; the launch is the line wanted
            if !l.contains(": reloading with ") {
                *launch = Some(l);
            }
        }
        line.clear();
    }
    at
}

/// Without terminal escape sequences (`ESC [ … letter`).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

fn size(n: u64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.0} KB", n as f64 / (1 << 10) as f64),
        n => format!("{n} B"),
    }
}

fn age(t: Option<SystemTime>) -> String {
    let Some(s) = t.and_then(|t| t.elapsed().ok()).map(|d| d.as_secs()) else { return String::new() };
    match s {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}

/// A row of the text shown.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Line(usize),
    /// A line, this many times in a row.
    Repeat(usize, usize),
    /// Lines `from..to` of one kind (`what`), folded or opened.
    Fold {
        what: String,
        from: usize,
        to: usize,
        open: bool,
    },
    /// Lines left out (errors only).
    Gap,
}

/// What a view of the lines shows, worked out again when it changes.
#[derive(Default)]
struct Shown {
    key: Option<ShownKey>,
    items: Vec<Item>,
    /// The items `search` is found in.
    found: Vec<usize>,
    /// The lines in the launch shown.
    range: (usize, usize),
    /// The longest line's length (in chars), for the text's width.
    longest: usize,
}

#[derive(Clone, PartialEq)]
struct ShownKey {
    file: Option<String>,
    read: Option<(u64, Option<SystemTime>)>,
    launch: LaunchShown,
    trouble: bool,
    only_found: bool,
    search: String,
    opened: usize,
}

/// Lines around each warning or error when only those are shown.
const CONTEXT: usize = 2;

impl Shown {
    fn update(&mut self, cache: &Cache, view: &LogsView) {
        let key = ShownKey {
            file: cache.file.clone(),
            read: cache.read,
            launch: view.launch.clone(),
            trouble: view.trouble,
            only_found: view.only_found,
            search: view.search.clone(),
            opened: cache.opened,
        };
        if self.key.as_ref() == Some(&key) {
            return;
        }
        let lines = &cache.lines;
        let (lo, hi) = launch_range(lines, &cache.launches, &view.launch);
        let q = view.search.trim().to_ascii_lowercase();
        let hit = |i: usize| !q.is_empty() && lines[i].to_ascii_lowercase().contains(&q);
        let items: Vec<Item> = if view.trouble {
            logview::trouble(&lines[lo..hi], CONTEXT)
                .into_iter()
                .map(|r| r.map_or(Item::Gap, |i| Item::Line(lo + i)))
                .collect()
        } else if view.only_found && !q.is_empty() {
            (lo..hi).filter(|&i| hit(i)).map(Item::Line).collect()
        } else {
            // a fold is open when clicked open, or when what's looked for is in it
            let open = |from: usize, to: usize| cache.open.contains(&lines[lo + from]) || (lo + from..lo + to).any(hit);
            logview::rows(&lines[lo..hi], open)
                .into_iter()
                .map(|r| match r {
                    Row::Line(i) => Item::Line(lo + i),
                    Row::Repeat { line, times } => Item::Repeat(lo + line, times),
                    Row::Fold { what, from, to } => Item::Fold { what, from: lo + from, to: lo + to, open: false },
                    Row::Unfolded { what, from, to } => Item::Fold { what, from: lo + from, to: lo + to, open: true },
                })
                .collect()
        };
        self.found = items
            .iter()
            .enumerate()
            .filter(|(_, it)| matches!(it, Item::Line(i) | Item::Repeat(i, _) if hit(*i)))
            .map(|(k, _)| k)
            .collect();
        self.longest = lines[lo..hi].iter().map(|l| l.chars().count()).max().unwrap_or(0);
        (self.items, self.range, self.key) = (items, (lo, hi), Some(key));
    }

    /// The lines shown, for Copy: a fold's and a repeat's all.
    fn text(&self, lines: &[String]) -> String {
        let mut out = vec![];
        for it in &self.items {
            match it {
                Item::Line(i) => out.push(lines[*i].as_str()),
                Item::Repeat(i, n) => out.extend(std::iter::repeat_n(lines[*i].as_str(), *n)),
                // an open fold's lines follow it
                Item::Fold { from, to, open: false, .. } => out.extend(lines[*from..*to].iter().map(String::as_str)),
                Item::Fold { .. } => {}
                Item::Gap => out.push("…"),
            }
        }
        out.join("\n")
    }
}

/// The lines of the launch shown: from its launch line to the next.
fn launch_range(lines: &[String], launches: &[usize], shown: &LaunchShown) -> (usize, usize) {
    let latest = (launches.last().copied().unwrap_or(0), lines.len());
    match shown {
        LaunchShown::All => (0, lines.len()),
        LaunchShown::Latest => latest,
        LaunchShown::Of(l) => match launches.iter().position(|&i| lines[i] == *l) {
            Some(k) => (launches[k], launches.get(k + 1).copied().unwrap_or(lines.len())),
            None => latest,
        },
    }
}

/// A launch line's date and time (`2026-10-31 14:05:29`).
fn launch_label(line: &str) -> String {
    line.get(14..33).unwrap_or(line).to_string()
}

/// A log line laid out: its time dim, its text in its level's colour (a
/// source location dim), `find` (lower case) marked, more strongly when
/// `current`; `short`: paths as their file names.
pub fn line_job(ui: &egui::Ui, line: &str, find: &str, current: bool, short: bool) -> egui::text::LayoutJob {
    let v = ui.visuals();
    let color = match logview::level(line) {
        Level::Error => v.error_fg_color,
        Level::Warn => v.warn_fg_color,
        Level::App => v.strong_text_color(),
        Level::Plain => v.text_color(),
    };
    let (time, rest) = line.split_at(logview::time_len(line));
    let rest = if short { logview::short_paths(rest) } else { rest.to_string() };
    let src = logview::source_at(&rest).unwrap_or(rest.len());
    let mut job = egui::text::LayoutJob::default();
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let (mark, strong) = (v.selection.bg_fill, egui::Color32::from_rgb(240, 190, 40));
    for (text, color) in [(time, v.weak_text_color()), (&rest[..src], color), (&rest[src..], v.weak_text_color())] {
        let plain = egui::text::TextFormat::simple(font.clone(), color);
        let marked = egui::text::TextFormat {
            background: if current { strong } else { mark },
            color: if current { egui::Color32::BLACK } else { color },
            ..plain.clone()
        };
        let mut at = 0;
        if !find.is_empty() {
            for (i, m) in text.to_ascii_lowercase().match_indices(find) {
                job.append(&text[at..i], 0.0, plain.clone());
                job.append(&text[i..i + m.len()], 0.0, marked.clone());
                at = i + m.len();
            }
        }
        job.append(&text[at..], 0.0, plain);
    }
    job
}

/// What the log list calls a log, and its group (App, Image servers,
/// Prompt enhancer, other).
fn log_label(name: &str) -> (usize, String) {
    let stem = name.trim_end_matches(".log");
    match stem {
        "increart" => (0, "IncreArt".into()),
        "increart-previous" => (0, "IncreArt (before)".into()),
        _ => {
            if let Some(s) = stem.strip_prefix("sd-server-") {
                (1, format!("sd-server {s}"))
            } else if let Some(s) = stem.strip_prefix("koboldcpp-") {
                (1, format!("KoboldCpp {s}"))
            } else if let Some(s) = stem.strip_prefix("pe-") {
                (2, s.to_string())
            } else {
                (3, stem.to_string())
            }
        }
    }
}

const GROUPS: [&str; 4] = ["App", "Image servers", "Prompt enhancer", "Other"];

/// The servers in use, by their log's file name: their state and its colour.
fn in_use(ui: &egui::Ui, st: &AppState) -> HashMap<String, (String, egui::Color32)> {
    let mut m = HashMap::new();
    for s in &st.servers {
        let log = s.config.as_ref().map_or_else(
            || increart_core::launcher::log_name(&s.name),
            |c| increart_core::launcher::log_path(Path::new(""), c).to_string_lossy().into_owned(),
        );
        let c = if s.state == "ready" {
            GOOD
        } else if s.state.contains("retrying") || s.state.contains("failed") {
            BAD
        } else {
            NEUTRAL
        };
        m.insert(log, (s.state.clone(), c));
    }
    if let Some(s) = &st.pe.server {
        let (_, state, color) = crate::app::pe_state(ui, st);
        m.insert(increart_core::pe::log_name(&s.name), (state, color));
    }
    for s in &st.pe.more {
        m.entry(increart_core::pe::log_name(&s.name)).or_insert_with(|| ("in use".into(), NEUTRAL));
    }
    m
}

/// The viewer: the logs on the left (by kind, newest first), the chosen
/// one's latest launch on the right, following it as it grows.
pub fn window(ctx: &egui::Context, st: &AppState, view: &LogsView, cache: &mut Cache, out: &mut Vec<Msg>) {
    let dir = increart_gui_core::runtime::log_dir();
    cache.refresh(&dir, view.file.as_deref());
    let mut v = view.clone();
    // the newest when none is asked for
    if view.file.is_none()
        && let Some((name, ..)) = cache.files.first()
    {
        v.file = Some(name.clone());
    }
    let mut shown = std::mem::take(&mut cache.shown);
    shown.update(cache, view);
    // a new search: to its last match (the latest)
    if cache.find_new {
        cache.find_new = false;
        if let Some(&k) = shown.found.last() {
            (cache.find_at, cache.jump) = (shown.found.len() - 1, Some(k));
            v.follow = false;
        }
    }
    cache.find_at = cache.find_at.min(shown.found.len().saturating_sub(1));
    let find = view.search.trim().to_ascii_lowercase();
    let current = (!find.is_empty()).then(|| shown.found.get(cache.find_at).copied()).flatten();
    ctx.request_repaint_after(Duration::from_millis(500));
    let find_id = egui::Id::unique("log_find");
    let find_key = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::F);
    if ctx.input_mut(|i| i.consume_shortcut(&find_key)) {
        ctx.memory_mut(|m| m.request_focus(find_id));
    }
    let full = Vec2::new(1100.0, 760.0).min(ctx.content_rect().size() * 0.95 - Vec2::splat(16.0));
    let mut toggled: Option<String> = None;
    let m = egui::Modal::new(egui::Id::unique("logs")).show(ctx, |ui| {
        ui.set_width(full.x);
        ui.set_height(full.y);
        ui.horizontal(|ui| {
            ui.heading(ic(ph::SCROLL, "Logs"));
            ui.weak(dir.display().to_string());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut on = st.config.log_traffic;
                if ui
                    .checkbox(&mut on, "Log server traffic")
                    .on_hover_text(
                        "Every request to the image servers and its answer, in server-traffic.log: for \
                         debugging a server, a proxy in front of sd-server especially (Server settings has it too)",
                    )
                    .changed()
                {
                    out.push(Msg::SetLogTraffic(on));
                }
            });
        });
        ui.add_space(4.0);
        let body = (full.y - 90.0).max(120.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(230.0);
                let using = in_use(ui, st);
                ScrollArea::vertical().id_salt("log_files").max_height(body).auto_shrink([false, false]).show(
                    ui,
                    |ui| {
                        if cache.files.is_empty() {
                            ui.weak("No logs yet: servers the app starts write one here.");
                        }
                        let mut files: Vec<_> = cache.files.iter().map(|f| (log_label(&f.0), f)).collect();
                        files.sort_by_key(|((group, _), _)| *group);
                        let mut group = None;
                        for ((g, label), (name, len, modified)) in files {
                            if group != Some(g) {
                                group = Some(g);
                                ui.add_space(if g == 0 { 0.0 } else { 6.0 });
                                ui.label(RichText::new(GROUPS[g]).small().weak());
                            }
                            let on = v.file.as_deref() == Some(name.as_str());
                            let trouble = cache.errors.get(name).is_some_and(|e| e.2);
                            ui.horizontal(|ui| {
                                let text = if trouble {
                                    RichText::new(format!("● {label}")).color(BAD)
                                } else {
                                    RichText::new(&label)
                                };
                                let mut hover = format!("{name}\n{} · {}", size(*len), age(*modified));
                                if trouble {
                                    hover.push_str("\nan error since its latest launch");
                                }
                                if ui.selectable_label(on, text).on_hover_text(hover).clicked() && !on {
                                    (v.file, v.launch) = (Some(name.clone()), LaunchShown::Latest);
                                    v.follow = true;
                                }
                                if let Some((state, color)) = using.get(name) {
                                    ui.label(RichText::new(state).small().color(*color));
                                }
                            });
                        }
                    },
                );
            });
            ui.separator();
            ui.vertical(|ui| {
                // solid bars, always shown: floating ones (egui's default)
                // appear and widen under the pointer, over the text
                ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                ui.horizontal(|ui| {
                    ui.label(ph::MAGNIFYING_GLASS);
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut v.search)
                            .id(find_id)
                            .hint_text("find  (Ctrl+F)")
                            .desired_width(220.0),
                    );
                    if v.search != view.search {
                        cache.find_new = true;
                    }
                    let mut step = 0i32;
                    let back = ui.input(|i| i.modifiers.shift);
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        step = if back { -1 } else { 1 };
                        r.request_focus();
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::F3)) {
                        step = if back { -1 } else { 1 };
                    }
                    if !find.is_empty() {
                        let n = shown.found.len();
                        if n == 0 {
                            ui.colored_label(ui.visuals().warn_fg_color, "not found");
                        } else {
                            ui.weak(format!("{} of {n}", cache.find_at + 1));
                        }
                        if ui.small_button(ph::CARET_UP).on_hover_text("the one before  (Shift+Enter)").clicked() {
                            step = -1;
                        }
                        if ui.small_button(ph::CARET_DOWN).on_hover_text("the next  (Enter, F3)").clicked() {
                            step = 1;
                        }
                    }
                    ui.checkbox(&mut v.only_found, "only these lines");
                    let n = shown.found.len();
                    if step != 0 && n > 0 {
                        cache.find_at = (cache.find_at as i64 + step as i64).rem_euclid(n as i64) as usize;
                        cache.jump = Some(shown.found[cache.find_at]);
                        v.follow = false;
                    }
                });
                ui.horizontal(|ui| {
                    if !cache.launches.is_empty() {
                        let lines = &cache.lines;
                        let latest = cache.launches.last().map(|&i| launch_label(&lines[i])).unwrap_or_default();
                        let text = match &v.launch {
                            LaunchShown::Latest => format!("latest launch ({latest})"),
                            LaunchShown::Of(l) => format!("launch of {}", launch_label(l)),
                            LaunchShown::All => format!("all {} launches", cache.launches.len()),
                        };
                        egui::ComboBox::from_id_salt("log_launch").selected_text(text).show_ui(ui, |ui| {
                            ui.selectable_value(&mut v.launch, LaunchShown::Latest, format!("latest ({latest})"));
                            for &i in cache.launches.iter().rev().skip(1) {
                                let l = &lines[i];
                                ui.selectable_value(&mut v.launch, LaunchShown::Of(l.clone()), launch_label(l));
                            }
                            ui.selectable_value(&mut v.launch, LaunchShown::All, "all");
                        });
                    }
                    ui.checkbox(&mut v.trouble, "errors only")
                        .on_hover_text("warnings and errors (and the app's own lines), with the lines around them");
                    ui.checkbox(&mut v.wrap, "wrap");
                    ui.checkbox(&mut v.follow, "follow")
                        .on_hover_text("keep to the end as the log grows (scrolling back unticks it)");
                    let (lo, hi) = shown.range;
                    let lines = format!("{} lines", hi - lo);
                    ui.weak(if cache.cut { format!("{lines}, of the last {}", size(TAIL)) } else { lines });
                });
                let mono = |l: &str| RichText::new(l).monospace();
                // the launch line, when the part shown starts after it
                let top = ui.cursor().top();
                if let Some(l) =
                    cache.launch.as_ref().filter(|l| cache.cut && shown.range.0 == 0 && !cache.lines.contains(l))
                {
                    ui.add(egui::Label::new(mono(l).weak()).wrap().selectable(true))
                        .on_hover_text("how it was started (from before the part shown)");
                    ui.separator();
                }
                let body = body - (ui.cursor().top() - top);
                let jump = cache.jump.take();
                let at = if jump.is_some() { None } else { kept_at(view, cache.kept) };
                let lines = &cache.lines;
                let mut draw = |ui: &mut egui::Ui, k: usize, wrap: bool| -> egui::Response {
                    let label = |job: egui::text::LayoutJob| {
                        let l = egui::Label::new(job);
                        if wrap { l.wrap() } else { l.extend() }
                    };
                    match &shown.items[k] {
                        Item::Line(i) => ui.add(label(line_job(ui, &lines[*i], &find, current == Some(k), false))),
                        Item::Repeat(i, n) => {
                            let mut job = line_job(ui, &lines[*i], &find, current == Some(k), false);
                            let font = egui::TextStyle::Monospace.resolve(ui.style());
                            let f = egui::text::TextFormat::simple(font, ui.visuals().weak_text_color());
                            job.append(&format!("  ×{n}"), 0.0, f);
                            ui.add(label(job))
                        }
                        Item::Fold { what, from, to, open } => {
                            let text = format!("{} {what}: {} lines", if *open { "▾" } else { "▸" }, to - from);
                            let r = ui
                                .add(egui::Label::new(mono(&text).weak()).sense(egui::Sense::click()))
                                .on_hover_text(if *open { "fold them" } else { "show them" });
                            if r.clicked() {
                                toggled = Some(lines[*from].clone());
                            }
                            r
                        }
                        Item::Gap => ui.add(egui::Label::new(mono("⋯").weak())),
                    }
                };
                let scrolled_away = if v.wrap {
                    // wrapped: across is never scrolled
                    let area = ScrollArea::vertical()
                        .id_salt("log_wrapped")
                        .max_height(body - 28.0)
                        .auto_shrink([false, false]);
                    let area = if let Some(y) = at { area.vertical_scroll_offset(y) } else { area };
                    let o = area.show(ui, |ui| {
                        for k in shown.items.len().saturating_sub(WRAPPED)..shown.items.len() {
                            let r = draw(ui, k, true);
                            if jump == Some(k) {
                                r.scroll_to_me(Some(egui::Align::Center));
                            }
                        }
                    });
                    measure_end(ui.ctx(), &o, view, at, &mut cache.kept)
                } else {
                    let area =
                        ScrollArea::both().id_salt("log_text").max_height(body - 28.0).auto_shrink([false, false]);
                    let row = ui.text_style_height(&egui::TextStyle::Monospace);
                    // as wide as the longest line, not the longest one in view
                    // (only the rows in view are laid out): the bars hold still
                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                    let glyph = ui.fonts_mut(|f| f.glyph_width(&font, 'm'));
                    let width = shown.longest as f32 * glyph + 8.0;
                    let pitch = row + ui.spacing().item_spacing.y;
                    let area = match (jump, at) {
                        // what's found in the middle
                        (Some(k), _) => area.vertical_scroll_offset((k as f32 * pitch - (body - 28.0) / 2.0).max(0.0)),
                        (None, Some(y)) => area.vertical_scroll_offset(y),
                        (None, None) => area,
                    };
                    let o = area.show_rows(ui, row, shown.items.len(), |ui, rows| {
                        ui.set_min_width(width);
                        for k in rows {
                            draw(ui, k, false);
                        }
                    });
                    measure_end(ui.ctx(), &o, view, at, &mut cache.kept)
                };
                if scrolled_away {
                    v.follow = false;
                }
            });
        });
        ui.add_space(4.0);
        let path = v.file.as_ref().map(|f| dir.join(f));
        let (_, close) = dialog_footer(
            ui,
            |ui| {
                let Some(path) = &path else { return };
                if ui.button(ic(ph::NOTE_PENCIL, "Open in editor")).on_hover_text("config.rsn `editor`").clicked() {
                    out.push(Msg::OpenInEditor(path.clone()));
                }
                if ui.button(ic(ph::FOLDER_OPEN, "Show in folder")).clicked() {
                    out.push(Msg::ShowInFileManager(path.clone()));
                }
                if ui.button(ic(ph::COPY, "Copy")).on_hover_text("the lines shown (folded ones too)").clicked() {
                    ui.ctx().copy_text(shown.text(&cache.lines));
                }
            },
            |ui| {
                // right to left: Close at the edge
                let close = ui.button("Close   Esc").clicked();
                let save = ui
                    .add_enabled(!st.saving_bug_report, egui::Button::new(ic(ph::BUG, "Save for a bug report…")))
                    .on_hover_text(BUG_REPORT_NOTE)
                    .on_disabled_hover_text("Saving…");
                (save.clicked(), close)
            },
        );
        let (save, close) = close;
        if save {
            out.push(Msg::SaveBugReport);
        }
        close
    });
    if let Some(first) = toggled {
        if !cache.open.remove(&first) {
            cache.open.insert(first);
        }
        cache.opened += 1;
    }
    cache.shown = shown;
    if m.inner || m.should_close() {
        out.push(Msg::Logs(None));
    } else if v != *view {
        out.push(Msg::Logs(Some(v)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_log_reads_as_a_terminal_shows_it() {
        assert_eq!(strip_ansi("\u{1b}[1;32mready\u{1b}[0m at :1234"), "ready at :1234");
        let p = std::env::temp_dir().join(format!("increart-log-test-{}.log", std::process::id()));
        std::fs::write(&p, "loading\n 10%\r 50%\r100%\r\ndone\n").unwrap();
        let len = std::fs::metadata(&p).unwrap().len();
        let read = read_tail(&p, len);
        std::fs::remove_file(&p).unwrap();
        assert_eq!(read, (vec!["loading".into(), "100%".into(), "done".into()], false));
    }

    #[test]
    fn the_last_launch_line_is_found_as_the_log_grows() {
        let p = std::env::temp_dir().join(format!("increart-launch-test-{}.log", std::process::id()));
        std::fs::write(&p, "\n--- IncreArt, 2026-10-30 10:00:00: kcpp --port 1\nout\n--- IncreArt: no such file\n")
            .unwrap();
        let mut launch = None;
        let at = find_launch(&p, 0, &mut launch);
        assert_eq!(launch.as_deref(), Some("--- IncreArt, 2026-10-30 10:00:00: kcpp --port 1"));
        let more = "--- IncreArt, 2026-10-30 10:01:00: reloading with a.kcpps, the settings for: kcpp\n--- IncreArt, 2026-10-30 11:00:00: kcpp --port 2";
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(more.as_bytes()).unwrap();
        let at = find_launch(&p, at, &mut launch);
        assert_eq!(
            launch.as_deref(),
            Some("--- IncreArt, 2026-10-30 10:00:00: kcpp --port 1"),
            "reloads aren't launches; a part line waits"
        );
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(b"\n").unwrap();
        find_launch(&p, at, &mut launch);
        std::fs::remove_file(&p).unwrap();
        assert_eq!(launch.as_deref(), Some("--- IncreArt, 2026-10-30 11:00:00: kcpp --port 2"));
    }
}
