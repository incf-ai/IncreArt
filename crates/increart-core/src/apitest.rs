//! `increart-apitest`'s protocol (DESIGN.md, Validating servers): the
//! cases it has (`list --json`), and a run's events as JSON lines on its
//! stdout (`run --jsonl`), which the app's **Validate servers** window
//! reads. A `stop` line on its stdin stops the run (cancelling its jobs on
//! the servers), on Windows too.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The program, beside the app's in a release.
pub const BINARY: &str = "increart-apitest";

/// What a case costs the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cost {
    /// Calls only: nothing loaded, nothing generated.
    Light,
    /// Loads a model preset (or starts a server).
    Loads,
    /// Generates: GPU time.
    Gpu,
}

impl Cost {
    pub fn label(self) -> &'static str {
        match self {
            Cost::Light => "light",
            Cost::Loads => "loads a model",
            Cost::Gpu => "GPU time",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    /// It works, but not as IncreArt would like.
    Warn,
    Fail,
    /// Not for this server or setup (the case says why).
    NotApplicable,
    /// Not run: what it needs failed, or the run was stopped.
    Skipped,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Warn => "warning",
            Outcome::Fail => "FAIL",
            Outcome::NotApplicable => "n/a",
            Outcome::Skipped => "skipped",
        }
    }

    /// Worse first: a group shows its worst.
    pub fn rank(self) -> u8 {
        match self {
            Outcome::Fail => 0,
            Outcome::Warn => 1,
            Outcome::Skipped => 2,
            Outcome::Pass => 3,
            Outcome::NotApplicable => 4,
        }
    }
}

/// One test case of one server (or of a preset's servers together).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Case {
    /// `preset/server/group.key`, or `preset/group.key` for the servers
    /// together.
    pub id: String,
    pub preset: String,
    /// None: the preset's servers together.
    pub server: Option<String>,
    pub group: String,
    pub title: String,
    pub cost: Cost,
    /// Why it isn't for this server or setup (it's then n/a).
    pub not_applicable: Option<String>,
    /// What IncreArt expects of the server here (for whoever reads the
    /// report: the API contract in a few lines).
    pub expects: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The cases, in the order they run.
    Plan {
        cases: Vec<Case>,
    },
    Start {
        id: String,
    },
    /// A line of a case's log (None: of the run, between cases).
    Log {
        id: Option<String>,
        text: String,
    },
    /// A request and its answer, during a case.
    Traffic {
        id: Option<String>,
        text: String,
    },
    End {
        id: String,
        outcome: Outcome,
        summary: String,
        secs: f64,
    },
    /// The run is over: its report's folder and zip.
    Done {
        counts: Counts,
        report: Option<PathBuf>,
        zip: Option<PathBuf>,
    },
    /// It couldn't run at all (no config, a bad argument).
    Error {
        text: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub pass: u32,
    pub warn: u32,
    pub fail: u32,
    pub not_applicable: u32,
    pub skipped: u32,
}

impl Counts {
    pub fn add(&mut self, o: Outcome) {
        match o {
            Outcome::Pass => self.pass += 1,
            Outcome::Warn => self.warn += 1,
            Outcome::Fail => self.fail += 1,
            Outcome::NotApplicable => self.not_applicable += 1,
            Outcome::Skipped => self.skipped += 1,
        }
    }

    pub fn line(&self) -> String {
        format!(
            "{} passed, {} warnings, {} failed, {} skipped, {} n/a",
            self.pass, self.warn, self.fail, self.skipped, self.not_applicable
        )
    }
}

/// `increart-apitest` beside this program (a release has both), else
/// its bare name: then it's looked for on PATH.
pub fn binary() -> PathBuf {
    let name = format!("{BINARY}{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(&name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Whether `pattern` (`*` for any text) picks the case `id`.
pub fn matches(pattern: &str, id: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == id;
    }
    let mut rest = id;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(at) = rest.find(part) else { return false };
            rest = &rest[at + part.len()..];
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(matches("Remote/proxy/jobs.cancel-queued", "Remote/proxy/jobs.cancel-queued"));
        assert!(!matches("Remote/proxy", "Remote/proxy/jobs.queue"));
        assert!(matches("Remote/proxy/*", "Remote/proxy/jobs.queue"));
        assert!(matches("*/jobs.*", "Remote/proxy/jobs.queue"));
        assert!(matches("*cancel*", "Remote/proxy/jobs.cancel-queued"));
        assert!(!matches("*/models.*", "Remote/proxy/jobs.queue"));
    }

    #[test]
    fn events_are_json_lines() {
        let e = Event::End { id: "a/b".into(), outcome: Outcome::NotApplicable, summary: "x".into(), secs: 1.5 };
        let text = serde_json::to_string(&e).unwrap();
        assert_eq!(text, r#"{"event":"end","id":"a/b","outcome":"not_applicable","summary":"x","secs":1.5}"#);
        assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), e);
    }
}
