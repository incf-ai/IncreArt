//! The log viewer (**Logs…**): the app's own log and those of the servers
//! it started, in the log folder (DESIGN.md, Server logs).

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, RichText, ScrollArea, Vec2};
use increart_gui_core::{AppState, LogsView, Msg};

use crate::widgets::{dialog_footer, ic, ph};

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
        }
        if self.file.as_deref() != file {
            (self.file, self.read, self.checked) = (file.map(str::to_string), None, None);
            self.lines.clear();
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
            if now.0 < self.searched {
                (self.launch, self.searched) = (None, 0); // begun again
            }
            self.searched = find_launch(&path, self.searched, &mut self.launch);
        }
    }
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
    let mut lines: Vec<String> =
        text.split('\n').map(|l| strip_ansi(l.trim_end_matches('\r').rsplit('\r').next().unwrap_or(""))).collect();
    if from > 0 {
        lines.remove(0); // begun mid-line
    }
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    (lines, from > 0)
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

/// The viewer: the logs on the left (newest first), the chosen one's end
/// on the right, following it as it grows.
pub fn window(ctx: &egui::Context, st: &AppState, view: &LogsView, cache: &mut Cache, out: &mut Vec<Msg>) {
    let dir = increart_gui_core::runtime::log_dir();
    cache.refresh(&dir, view.file.as_deref());
    // the newest when none is asked for
    if view.file.is_none()
        && let Some((name, ..)) = cache.files.first()
    {
        out.push(Msg::Logs(Some(LogsView { file: Some(name.clone()), ..view.clone() })));
    }
    ctx.request_repaint_after(Duration::from_millis(500));
    let set = |out: &mut Vec<Msg>, v: LogsView| out.push(Msg::Logs(Some(v)));
    let full = Vec2::new(1100.0, 760.0).min(ctx.content_rect().size() * 0.95 - Vec2::splat(16.0));
    let m = egui::Modal::new(egui::Id::unique("logs")).show(ctx, |ui| {
        ui.set_width(full.x);
        ui.set_height(full.y);
        ui.horizontal(|ui| {
            ui.heading(ic(ph::SCROLL, "Logs"));
            ui.weak(dir.display().to_string());
        });
        ui.add_space(4.0);
        let body = (full.y - 90.0).max(120.0);
        let filtered: Vec<&str> = {
            let q = view.search.to_lowercase();
            cache.lines.iter().map(String::as_str).filter(|l| q.is_empty() || l.to_lowercase().contains(&q)).collect()
        };
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(230.0);
                ScrollArea::vertical().id_salt("log_files").max_height(body).auto_shrink([false, false]).show(
                    ui,
                    |ui| {
                        if cache.files.is_empty() {
                            ui.weak("No logs yet: servers the app starts write one here.");
                        }
                        for (name, len, modified) in &cache.files {
                            let on = view.file.as_deref() == Some(name.as_str());
                            let r = ui.selectable_label(on, name.trim_end_matches(".log")).on_hover_text(format!(
                                "{name}\n{} · {}",
                                size(*len),
                                age(*modified)
                            ));
                            if r.clicked() && !on {
                                set(out, LogsView { file: Some(name.clone()), ..view.clone() });
                            }
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
                    let mut v = view.clone();
                    ui.label(ph::MAGNIFYING_GLASS);
                    ui.add(
                        egui::TextEdit::singleline(&mut v.search).hint_text("only lines with…").desired_width(220.0),
                    );
                    ui.checkbox(&mut v.wrap, "wrap");
                    ui.checkbox(&mut v.follow, "follow")
                        .on_hover_text("keep to the end as the log grows (untick to scroll back)");
                    let shown = if view.search.is_empty() {
                        format!("{} lines", cache.lines.len())
                    } else {
                        format!("{} of {} lines", filtered.len(), cache.lines.len())
                    };
                    ui.weak(if cache.cut { format!("{shown}, the last {}", size(TAIL)) } else { shown });
                    if v != *view {
                        set(out, v);
                    }
                });
                let mono = |l: &str| RichText::new(l).monospace();
                // the launch line, when the part shown starts after it
                let top = ui.cursor().top();
                if let Some(l) = cache.launch.as_ref().filter(|l| cache.cut && !cache.lines.contains(l)) {
                    ui.add(egui::Label::new(mono(l).weak()).wrap().selectable(true))
                        .on_hover_text("how it was started (from before the part shown)");
                    ui.separator();
                }
                let body = body - (ui.cursor().top() - top);
                if view.wrap {
                    // wrapped: across is never scrolled
                    let area = ScrollArea::vertical()
                        .id_salt("log_wrapped")
                        .max_height(body - 28.0)
                        .auto_shrink([false, false]);
                    // following: as far down as it goes (the offset is clamped)
                    let area = if view.follow { area.vertical_scroll_offset(1e9) } else { area };
                    area.show(ui, |ui| {
                        for l in &filtered[filtered.len().saturating_sub(WRAPPED)..] {
                            ui.add(egui::Label::new(mono(l)).wrap());
                        }
                    });
                } else {
                    let area =
                        ScrollArea::both().id_salt("log_text").max_height(body - 28.0).auto_shrink([false, false]);
                    let row = ui.text_style_height(&egui::TextStyle::Monospace);
                    // as wide as the longest line, not the longest one in view
                    // (only the rows in view are laid out): the bars hold still
                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                    let glyph = ui.fonts_mut(|f| f.glyph_width(&font, 'm'));
                    let longest = filtered.iter().map(|l| l.chars().count()).max().unwrap_or(0);
                    let width = longest as f32 * glyph + 8.0;
                    // following: the end (show_rows turns the offset into a row number)
                    let end = filtered.len() as f32 * (row + ui.spacing().item_spacing.y) - (body - 28.0);
                    let area = if view.follow { area.vertical_scroll_offset(end.max(0.0)) } else { area };
                    area.show_rows(ui, row, filtered.len(), |ui, rows| {
                        ui.set_min_width(width);
                        for l in &filtered[rows] {
                            ui.add(egui::Label::new(mono(l)).extend());
                        }
                    });
                }
            });
        });
        ui.add_space(4.0);
        let path = view.file.as_ref().map(|f| dir.join(f));
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
                if ui.button(ic(ph::COPY, "Copy")).on_hover_text("the lines shown").clicked() {
                    ui.ctx().copy_text(filtered.join("\n"));
                }
            },
            |ui| {
                let save = ui
                    .add_enabled(!st.saving_bug_report, egui::Button::new(ic(ph::BUG, "Save for a bug report…")))
                    .on_hover_text(BUG_REPORT_NOTE)
                    .on_disabled_hover_text("Saving…");
                (save.clicked(), ui.button("Close   Esc").clicked())
            },
        );
        let (save, close) = close;
        if save {
            out.push(Msg::SaveBugReport);
        }
        close
    });
    if m.inner || m.should_close() {
        out.push(Msg::Logs(None));
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
