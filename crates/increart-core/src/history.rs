//! Prompt history (DESIGN.md, Prompt history): the last [`MAX`] substantial
//! edits of a text, as JSON lines next to what they belong to.
//!
//! - a project's Generate form (the prompt and every setting, and the runs
//!   queued from it): `projects/<project>/prompt-history.jsonl`
//! - the New idea form: `input/new-idea-history.jsonl`
//! - an idea's text: `<idea folder>/text-history.jsonl` (moves with the idea)

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::fsutil;
use crate::id::{Id, Timestamp};
use crate::model::GenerateDraft;

/// Entries kept per history.
pub const MAX: usize = 1000;
/// The file is compacted to [`MAX`] once it holds this many.
const COMPACT_AT: usize = MAX + 100;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryKind {
    /// A pause after editing (merged with the previous edit when small).
    Edit,
    /// The prompt a run was queued with (its label).
    Queued(String),
    BeforeClear,
    BeforeRestore,
    /// Before the prompt's `<imageN>` tags were renumbered to follow the
    /// image list.
    BeforeRetag,
    /// Before a prompt preset replaced the prompt.
    BeforePreset,
    /// An idea added from the New idea form, or an idea's text saved.
    Saved,
    /// Before a PE rewrite (or an image's description) replaced the text.
    BeforeRewrite,
}

impl HistoryKind {
    /// Checkpoints are never merged into and are trimmed last.
    pub fn is_checkpoint(&self) -> bool {
        *self != HistoryKind::Edit
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub time: Timestamp,
    pub kind: HistoryKind,
    pub text: String,
    /// The negative prompt (Generate) or the title (ideas).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extra: String,
    /// Generate: the whole form (older entries have only the prompt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<Box<GenerateDraft>>,
    /// Queued: the runs it made (several for a sweep).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<Id>,
    /// Generate: the project's newest run before this entry (edits made
    /// "since r05").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
}

impl HistoryEntry {
    /// A text entry (no form, no runs).
    pub fn text(time: Timestamp, kind: HistoryKind, text: String, extra: String) -> HistoryEntry {
        HistoryEntry { time, kind, text, extra, form: None, runs: vec![], after: None }
    }

    /// The same texts and settings. The form's label and first seed move on
    /// by themselves after each queue, so they don't count.
    pub fn same_content(&self, o: &HistoryEntry) -> bool {
        self.text == o.text && self.extra == o.extra && settings_eq(self.form.as_deref(), o.form.as_deref())
    }
}

/// Two forms' settings are the same (label and first seed aside; no form
/// on either side: nothing to compare).
pub fn settings_eq(a: Option<&GenerateDraft>, b: Option<&GenerateDraft>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            let norm = |d: &GenerateDraft| GenerateDraft { label: String::new(), first_seed: 0, ..d.clone() };
            norm(a) == norm(b)
        }
        _ => true,
    }
}

/// All entries, oldest first (unreadable lines are skipped).
pub fn read(path: &Path) -> Vec<HistoryEntry> {
    fs::read_to_string(path)
        .map(|s| s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

/// Keeps the newest [`MAX`], dropping the oldest plain edits first.
pub fn trim(entries: &mut Vec<HistoryEntry>) {
    while entries.len() > MAX {
        match entries.iter().position(|e| !e.kind.is_checkpoint()) {
            Some(i) => {
                entries.remove(i);
            }
            None => {
                entries.remove(0);
            }
        }
    }
}

fn write_all(path: &Path, entries: &[HistoryEntry]) -> Result<()> {
    let mut text = String::new();
    for e in entries {
        text += &serde_json::to_string(e).map_err(|e| Error::encode(path.display(), e))?;
        text.push('\n');
    }
    fsutil::write_atomic(path, text.as_bytes())
}

/// Adds an entry, or with `replace_last` replaces the newest one (a merged
/// edit). Appends a line in the common case; rewrites the file to merge or
/// compact.
pub fn record(path: &Path, entry: &HistoryEntry, replace_last: bool) -> Result<()> {
    // Compared with the file's real last entry (the app's memory of it is
    // empty after a restart): the same text again adds nothing, except that
    // a checkpoint (queued, before clear, …) takes over an edit's place.
    if !replace_last {
        let mut all = read(path);
        if let Some(l) = all.last()
            && l.same_content(entry)
        {
            if entry.kind == HistoryKind::Edit || l.kind == entry.kind {
                return Ok(());
            }
            if l.kind == HistoryKind::Edit {
                all.pop();
                all.push(entry.clone());
                return write_all(path, &all);
            }
        }
    }
    if replace_last {
        let mut all = read(path);
        all.pop();
        all.push(entry.clone());
        return write_all(path, &all);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).at(dir)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path).at(path)?;
    let line = serde_json::to_string(entry).map_err(|e| Error::encode(path.display(), e))?;
    writeln!(f, "{line}").at(path)?;
    drop(f);
    if fs::read_to_string(path).map(|s| s.lines().count()).unwrap_or(0) >= COMPACT_AT {
        let mut all = read(path);
        trim(&mut all);
        write_all(path, &all)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(kind: HistoryKind, text: &str) -> HistoryEntry {
        HistoryEntry::text(crate::id::now(), kind, text.into(), String::new())
    }

    #[test]
    fn append_merge_and_compact() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("h.jsonl");
        record(&p, &e(HistoryKind::Edit, "a fox"), false).unwrap();
        record(&p, &e(HistoryKind::Edit, "a red fox"), true).unwrap();
        // the edit becomes the checkpoint when its text is queued unchanged
        record(&p, &e(HistoryKind::Queued("r01".into()), "a red fox"), false).unwrap();
        let all = read(&p);
        assert_eq!(all.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["a red fox"]);
        assert_eq!(all[0].kind, HistoryKind::Queued("r01".into()));
        // the same text again (e.g. the first save after a restart): nothing new
        record(&p, &e(HistoryKind::Edit, "a red fox"), false).unwrap();
        record(&p, &e(HistoryKind::Queued("r01".into()), "a red fox"), false).unwrap();
        assert_eq!(read(&p).len(), 1);
        // another queue of the same text is a checkpoint of its own
        record(&p, &e(HistoryKind::Queued("r02".into()), "a red fox"), false).unwrap();
        assert_eq!(read(&p).len(), 2);
        for i in 0..COMPACT_AT {
            record(&p, &e(HistoryKind::Edit, &format!("edit {i}")), false).unwrap();
        }
        let all = read(&p);
        assert!((MAX..COMPACT_AT).contains(&all.len()), "{}", all.len());
        assert_eq!(all[0].kind, HistoryKind::Queued("r01".into()), "checkpoints are trimmed last");
        assert_eq!(all.last().unwrap().text, format!("edit {}", COMPACT_AT - 1));
    }
}
