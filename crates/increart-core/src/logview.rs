//! Reading logs as people do (DESIGN.md, Server logs): each line's level,
//! where each launch starts, the noise folded (runs of a library's
//! `prefix:` lines, progress bars, a line repeated), and the time the app
//! puts before each line a server writes ([`stamp`]).

/// How a line reads: by its level, or the app's own (`--- IncreArt…`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Plain,
    Warn,
    Error,
    /// A line the app wrote into a server's log (a launch, a reload, why a
    /// start failed).
    App,
}

/// The time put before each line a server writes: `14:05:33.300 `.
pub fn stamp(now: chrono::DateTime<chrono::Local>) -> String {
    now.format("%H:%M:%S%.3f ").to_string()
}

const STAMP_LEN: usize = "14:05:33.300 ".len();

/// The line's [`stamp`] (with its space), if it has one, and the rest.
pub fn split_stamp(line: &str) -> (&str, &str) {
    let b = line.as_bytes();
    let is = b.len() >= STAMP_LEN
        && b[..STAMP_LEN].iter().enumerate().all(|(i, c)| match i {
            2 | 5 => *c == b':',
            8 => *c == b'.',
            12 => *c == b' ',
            _ => c.is_ascii_digit(),
        });
    if is { line.split_at(STAMP_LEN) } else { ("", line) }
}

/// The line without its [`stamp`].
pub fn unstamped(line: &str) -> &str {
    split_stamp(line).1
}

/// How long the time a line starts with is, with its space: a server
/// line's [`stamp`], or a date and time (`2026-10-31 14:05:33 `, the app's
/// log; `2026-10-31 14:05:32.522 `, a run's generation log); 0 without.
pub fn time_len(line: &str) -> usize {
    let stamp = split_stamp(line).0.len();
    if stamp > 0 {
        return stamp;
    }
    let b = line.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    let date_time = digits(0..4)
        && b.get(4) == Some(&b'-')
        && digits(5..7)
        && b.get(7) == Some(&b'-')
        && digits(8..10)
        && b.get(10) == Some(&b' ')
        && digits(11..13)
        && b.get(13) == Some(&b':')
        && digits(14..16)
        && b.get(16) == Some(&b':')
        && digits(17..19);
    if !date_time {
        return 0;
    }
    match b.get(19) {
        Some(b' ') => 20,
        Some(b'.') if digits(20..23) && b.get(23) == Some(&b' ') => 24,
        _ => 0,
    }
}

/// A launch line ([`crate::process::log_header`]), not a KoboldCpp's reload.
pub fn is_launch(line: &str) -> bool {
    line.starts_with("--- IncreArt, ") && !line.contains(": reloading with ")
}

/// Where each launch starts in `lines`.
pub fn launches<S: AsRef<str>>(lines: &[S]) -> Vec<usize> {
    lines.iter().enumerate().filter(|(_, l)| is_launch(l.as_ref())).map(|(i, _)| i).collect()
}

/// A line's level. sd.cpp's `[E]`/`[W]`; the app's log's `error:`,
/// `alert:` and `panic:`; else the words a failure or warning is told by.
pub fn level(line: &str) -> Level {
    let l = unstamped(line);
    if l.starts_with("--- IncreArt") {
        return Level::App;
    }
    if l.starts_with("[E]") || l.starts_with("[F]") {
        return Level::Error;
    }
    if l.starts_with("[W]") {
        return Level::Warn;
    }
    if l.starts_with("[I]") || l.starts_with("[D]") || l.starts_with("[T]") {
        return Level::Plain;
    }
    // the app's log: `2026-10-31 14:05:33 error: …`
    if let Some(kind) = l.get(20..).and_then(|r| r.split_once(": ")).map(|(k, _)| k)
        && !kind.contains(' ')
        && time_len(l) == 20
    {
        return match kind {
            "error" | "panic" => Level::Error,
            "alert" => Level::Warn,
            _ => Level::Plain,
        };
    }
    let lower = l.to_lowercase();
    const ERRORS: [&str; 9] =
        ["error", "failed", "failure", "cannot ", "can't ", "out of memory", "panicked", "traceback", "exception"];
    if ERRORS.iter().any(|w| lower.contains(w)) && !lower.contains("no error") && !lower.contains(" 0 error") {
        return Level::Error;
    }
    if lower.contains("warning") || lower.starts_with("warn") || lower.contains("deprecated") {
        return Level::Warn;
    }
    Level::Plain
}

/// Where a line's source location starts (sd.cpp's ` --- image.cpp:907`),
/// to be shown dimmed.
pub fn source_at(line: &str) -> Option<usize> {
    let at = line.rfind(" --- ")?;
    let src = &line[at + 5..];
    let (file, num) = src.rsplit_once(':')?;
    let ok = !num.is_empty()
        && num.bytes().all(|b| b.is_ascii_digit())
        && file.contains('.')
        && !file.contains(char::is_whitespace);
    ok.then_some(at)
}

/// A row as shown: a line, a line repeated, or a run of noise folded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Line(usize),
    /// The line at `line`, `times` times in a row.
    Repeat {
        line: usize,
        times: usize,
    },
    /// Lines `from..to`, all `what` (a library's `prefix:`, or progress).
    Fold {
        what: String,
        from: usize,
        to: usize,
    },
    /// A fold opened: its heading (its lines follow, as [`Row::Line`]s).
    Unfolded {
        what: String,
        from: usize,
        to: usize,
    },
}

/// A run this long or longer of one kind of line is folded.
pub const FOLD_MIN: usize = 8;

/// What kind of noise a line is, for folding: progress bars, or a
/// library's `prefix:` (`print_info:`, `llama_model_loader:`, `load:`, …).
pub fn noise(line: &str) -> Option<&str> {
    let l = unstamped(line);
    let t = l.trim_start();
    if t.starts_with('|') && (t.contains("it/s") || t.contains("s/it")) {
        return Some("progress");
    }
    if l.starts_with(' ') || l.starts_with("--- ") {
        return None;
    }
    let (prefix, _) = l.split_once(':')?;
    let word = !prefix.is_empty()
        && prefix.len() <= 32
        && prefix.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && prefix.bytes().next().is_some_and(|b| b.is_ascii_alphabetic());
    word.then_some(prefix)
}

/// `lines` as rows: runs of [`FOLD_MIN`] or more of one kind of [noise]
/// folded (those `open` says to show, given the run's `from..to`, under a
/// heading), a line repeated as one. A run with an error or a warning in
/// it isn't folded.
pub fn rows<S: AsRef<str>>(lines: &[S], open: impl Fn(usize, usize) -> bool) -> Vec<Row> {
    let mut rows = vec![];
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].as_ref();
        if let Some(kind) = noise(line) {
            let end = (i + 1..lines.len()).find(|&j| noise(lines[j].as_ref()) != Some(kind)).unwrap_or(lines.len());
            let quiet = lines[i..end].iter().all(|l| matches!(level(l.as_ref()), Level::Plain));
            if end - i >= FOLD_MIN && quiet {
                if open(i, end) {
                    rows.push(Row::Unfolded { what: kind.to_string(), from: i, to: end });
                    rows.extend((i..end).map(Row::Line));
                } else {
                    rows.push(Row::Fold { what: kind.to_string(), from: i, to: end });
                }
                i = end;
                continue;
            }
        }
        let same =
            (i + 1..lines.len()).find(|&j| unstamped(lines[j].as_ref()) != unstamped(line)).unwrap_or(lines.len());
        if same - i > 1 && !unstamped(line).trim().is_empty() {
            rows.push(Row::Repeat { line: i, times: same - i });
            i = same;
        } else {
            rows.push(Row::Line(i));
            i += 1;
        }
    }
    rows
}

/// The rows of a warning or error, with `context` lines around each; gaps
/// between them as None.
pub fn trouble<S: AsRef<str>>(lines: &[S], context: usize) -> Vec<Option<usize>> {
    let hits: Vec<usize> = (0..lines.len())
        .filter(|&i| matches!(level(lines[i].as_ref()), Level::Warn | Level::Error | Level::App))
        .collect();
    let n = lines.len();
    let mut ranges: Vec<(usize, usize)> = vec![];
    for h in hits {
        let (from, to) = (h.saturating_sub(context), (h + context + 1).min(n));
        match ranges.last_mut() {
            Some(r) if from <= r.1 => r.1 = r.1.max(to),
            _ => ranges.push((from, to)),
        }
    }
    let (mut out, mut at) = (vec![], 0);
    for (from, to) in ranges {
        if from > at {
            out.push(None);
        }
        out.extend((from..to).map(Some));
        at = to;
    }
    if at < n && !out.is_empty() {
        out.push(None);
    }
    out
}

/// Paths in `line` as their file names (a run's generation log names each
/// model file in full).
pub fn short_paths(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let (core, tail) = match word.find([',', ';', ')', ']']) {
            Some(i) if word[i..].chars().all(|c| matches!(c, ',' | ';' | ')' | ']' | '.')) => word.split_at(i),
            _ => (word.as_str(), ""),
        };
        let file = core.rsplit(['/', '\\']).next().unwrap_or(core);
        let path = (core.contains('/') || core.contains('\\'))
            && !core.contains("://")
            && file.contains('.')
            && !file.starts_with('.')
            && file.len() < core.len();
        out.push_str(if path { file } else { core });
        out.push_str(tail);
        word.clear();
    };
    for c in line.chars() {
        if c.is_whitespace() || c == '(' {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_read_from_each_kind_of_log() {
        assert_eq!(level("[E] failed to load model --- model.cpp:12"), Level::Error);
        assert_eq!(level("14:05:33.300 [W] --fa is deprecated --- common.cpp:780"), Level::Warn);
        assert_eq!(level("14:05:33.300 [I] generate_image completed in 29.23s --- image.cpp:1058"), Level::Plain);
        assert_eq!(level("2026-10-31 14:05:33 error: no server can load qwen"), Level::Error);
        assert_eq!(level("2026-10-31 14:05:33 info: Saved for a bug report"), Level::Plain);
        assert_eq!(level("2026-10-31 14:05:33 alert: done"), Level::Warn);
        assert_eq!(level("WARNING: radv is not a conformant Vulkan implementation"), Level::Warn);
        assert_eq!(level("ggml_vulkan: Device memory allocation failed"), Level::Error);
        assert_eq!(level("--- IncreArt, 2026-10-31 14:00:04: sd-server --listen-port 1235"), Level::App);
        assert_eq!(level("print_info: n_expert = 0"), Level::Plain);
        // a run's generation log
        assert_eq!(level("2026-10-31 14:05:32.522 queued 2 seeds (0…1) of qwen"), Level::Plain);
        assert_eq!(level("2026-10-31 14:05:40.100 sd-rocm0: seed 3 failed: out of memory"), Level::Error);
        assert_eq!(time_len("2026-10-31 14:05:32.522 queued"), 24);
        assert_eq!(time_len("2026-10-31 14:05:33 info: x"), 20);
        assert_eq!(time_len("14:05:33.300 [I] ready"), 13);
        assert_eq!(time_len("[I] ready"), 0);
    }

    #[test]
    fn stamps_are_told_from_the_text() {
        assert_eq!(split_stamp("14:05:33.300 [I] ready"), ("14:05:33.300 ", "[I] ready"));
        assert_eq!(split_stamp("[I] ready"), ("", "[I] ready"));
        assert_eq!(split_stamp("2026-10-31 14:05:33 info: x"), ("", "2026-10-31 14:05:33 info: x"));
        let s = stamp(chrono::Local::now());
        assert_eq!(split_stamp(&format!("{s}x")).1, "x");
    }

    #[test]
    fn source_locations_are_found() {
        let l = "[I] decoding 1 latents --- image.cpp:562";
        assert_eq!(source_at(l), Some(l.find(" --- ").unwrap()));
        assert_eq!(source_at("--- IncreArt, 2026-10-31 14:00:04: x"), None);
        assert_eq!(source_at("a --- b c:1"), None);
    }

    #[test]
    fn noise_is_folded_and_repeats_counted() {
        let mut lines = vec!["Loading Text Model: a.gguf".to_string()];
        lines.extend((0..10).map(|i| format!("print_info: n_{i} = {i}")));
        lines.push("load_tensors: offloaded 37/37 layers to GPU".into());
        lines.extend(["WARNING: radv".to_string(), "WARNING: radv".into(), "WARNING: radv".into()]);
        lines.extend((0..9).map(|i| format!("  |==>   | {i}/20 - 1.3s/it")));
        lines.push("[I] sampling completed".into());
        let r = rows(&lines, |_, _| false);
        assert_eq!(
            r,
            vec![
                Row::Line(0),
                Row::Fold { what: "print_info".into(), from: 1, to: 11 },
                Row::Line(11),
                Row::Repeat { line: 12, times: 3 },
                Row::Fold { what: "progress".into(), from: 15, to: 24 },
                Row::Line(24),
            ]
        );
        // opened: a heading, then its lines one by one
        let r = rows(&lines, |from, _| from == 1);
        assert_eq!(r[1..4], [Row::Unfolded { what: "print_info".into(), from: 1, to: 11 }, Row::Line(1), Row::Line(2)]);
        // a run with an error in it stays open
        lines[5] = "print_info: failed to read".into();
        assert!(matches!(rows(&lines, |_, _| false)[1], Row::Line(1)));
    }

    #[test]
    fn trouble_is_shown_with_its_context() {
        let lines: Vec<String> = (0..12)
            .map(|i| match i {
                3 => "[E] boom".to_string(),
                9 => "WARNING: x".to_string(),
                _ => format!("line {i}"),
            })
            .collect();
        let t = trouble(&lines, 1);
        assert_eq!(t, vec![None, Some(2), Some(3), Some(4), None, Some(8), Some(9), Some(10), None]);
        assert!(trouble(&["fine"], 2).is_empty());
    }

    #[test]
    fn launches_are_found_and_reloads_are_not_launches() {
        let lines = [
            "--- IncreArt, 2026-10-31 10:00:00: kcpp --port 1",
            "out",
            "--- IncreArt, 2026-10-31 10:01:00: reloading with a.kcpps, the settings for: kcpp",
            "--- IncreArt, 2026-10-31 11:00:00: kcpp --port 2",
        ];
        assert_eq!(launches(&lines), [0, 3]);
    }

    #[test]
    fn paths_show_as_file_names() {
        assert_eq!(
            short_paths("(koboldcpp/mmproj-Q8_0.gguf, /mnt/models/qwen.gguf); sd-rocm0: it doesn't have a/b.gguf"),
            "(mmproj-Q8_0.gguf, qwen.gguf); sd-rocm0: it doesn't have b.gguf"
        );
        assert_eq!(short_paths("at http://127.0.0.1:5002/v1/ ok"), "at http://127.0.0.1:5002/v1/ ok");
        assert_eq!(short_paths("1/20 steps, 37/37 layers"), "1/20 steps, 37/37 layers");
    }
}
