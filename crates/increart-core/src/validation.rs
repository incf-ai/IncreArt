//! Which server presets passed the server validation suite's basic cases
//! (DESIGN.md, Validating servers): a stamp per run of them, in
//! `validated.json` in the app's state folder, written by
//! `increart-apitest` and read by the app (its **Servers** screen marks
//! each preset; opening one never validated asks once to validate it).
//!
//! A stamp holds for the preset's settings it was run with (its
//! [`fingerprint`]): editing the preset, or the sd-server settings a
//! started one uses, takes it back to not validated. Its seal is a check
//! against a stamp edited by hand, not a secret (the source is public):
//! what shows a setup was validated is the report the stamp names, its
//! hash in the stamp, which a bug report carries.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::apitest::{Counts, Outcome};
use crate::config::{Config, ServerConfig, ServerPreset};

pub const FILE: &str = "validated.json";

/// Stamps kept per preset (the newest).
const KEEP: usize = 5;

/// The suite's basic cases, `group.key`: each server answers, lists its
/// models and makes a small image, a chat server answers, and the servers
/// together route each model preset and run through the engine.
pub const BASIC: &[&str] = &[
    "together.open",
    "connect.capabilities",
    "models.list",
    "generate.txt2img",
    "prompt-enhance.answers",
    "together.routing",
    "together.end-to-end",
];

/// Whether a case ID (`preset/server/group.key`, `preset/group.key`) is
/// one of the [`BASIC`] cases.
pub fn is_basic(id: &str) -> bool {
    BASIC.iter().any(|b| id.ends_with(&format!("/{b}")))
}

/// `validated.json` in the app's state folder.
pub fn path() -> Option<PathBuf> {
    crate::config::user_dir("XDG_STATE_HOME", ".local/state", "LOCALAPPDATA").map(|d| d.join(FILE))
}

/// The preset's settings as a stamp holds for them: its servers (not its
/// name), and the settings of the app a started one is run with.
pub fn fingerprint(config: &Config, preset: &ServerPreset) -> String {
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(&preset.servers).unwrap_or_default());
    if preset.servers.iter().any(|s| matches!(s, ServerConfig::Start { .. })) {
        let sd = &config.sdserver;
        h.update(serde_json::to_vec(&(&sd.binary, &sd.model_dirs, &sd.args)).unwrap_or_default());
    }
    if preset.servers.iter().any(|s| matches!(s, ServerConfig::Kobold(_)) && s.is_started()) {
        h.update(config.koboldcpp.as_bytes());
    }
    hex(&h.finalize())[..16].to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of a file, in hex.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    Ok(hex(&Sha256::digest(std::fs::read(path)?)))
}

/// A run of a preset's basic cases.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    pub preset: String,
    pub fingerprint: String,
    /// When it ran (RFC 3339, local time).
    pub when: String,
    /// Pass (warnings allowed: [`Outcome::Warn`]) or Fail.
    pub outcome: Outcome,
    /// Of the basic cases.
    pub counts: Counts,
    /// Each server, and what it reported running (sd-server's build,
    /// KoboldCpp's version), when it said.
    pub servers: Vec<(String, String)>,
    /// `increart-apitest`'s version line.
    pub suite: String,
    /// The SHA-256 of the run's `report.json`.
    pub report_sha256: String,
    /// The run's zipped report.
    pub zip: Option<PathBuf>,
    pub seal: String,
}

impl Stamp {
    fn seal_of(&self) -> String {
        let mut h = Sha256::new();
        let fields = (
            &self.preset,
            &self.fingerprint,
            &self.when,
            self.outcome,
            self.counts,
            &self.servers,
            &self.suite,
            &self.report_sha256,
        );
        h.update(b"increart validation stamp 1\n");
        h.update(serde_json::to_vec(&fields).unwrap_or_default());
        hex(&h.finalize())
    }

    /// Seals it (once its fields are set).
    pub fn sealed(mut self) -> Stamp {
        self.seal = self.seal_of();
        self
    }

    pub fn intact(&self) -> bool {
        self.seal == self.seal_of()
    }

    pub fn passed(&self) -> bool {
        matches!(self.outcome, Outcome::Pass | Outcome::Warn)
    }

    /// Its date, as said beside a preset ("2026-10-31").
    pub fn day(&self) -> &str {
        self.when.get(..10).unwrap_or(&self.when)
    }
}

/// What a preset's stamps say of it now.
#[derive(Clone, Debug, PartialEq)]
pub enum Status<'a> {
    /// Never validated.
    Never,
    /// Validated, but with other settings than it has now.
    Changed(&'a Stamp),
    Passed(&'a Stamp),
    Failed(&'a Stamp),
}

/// The stamps file: every stamp, and the settings the app has asked about
/// (asked once each).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Validations {
    #[serde(default)]
    pub stamps: Vec<Stamp>,
    /// Fingerprints opened once, asked whether to validate.
    #[serde(default)]
    pub asked: Vec<String>,
}

impl Validations {
    /// The file at `path`; none (or unreadable): empty.
    pub fn load(path: &Path) -> Validations {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }

    /// What the stamps say of `preset` as it is in `config` (the newest
    /// intact stamp of its settings, else of its name).
    pub fn status(&self, config: &Config, preset: &ServerPreset) -> Status<'_> {
        let fp = fingerprint(config, preset);
        let newest = |f: &dyn Fn(&Stamp) -> bool| self.stamps.iter().rev().find(|s| s.intact() && f(s));
        match newest(&|s| s.fingerprint == fp) {
            Some(s) if s.passed() => Status::Passed(s),
            Some(s) => Status::Failed(s),
            None => newest(&|s| s.preset == preset.name).map_or(Status::Never, Status::Changed),
        }
    }

    pub fn was_asked(&self, config: &Config, preset: &ServerPreset) -> bool {
        self.asked.contains(&fingerprint(config, preset))
    }

    /// The newest stamp of each preset, newest first.
    pub fn newest_each(&self) -> Vec<&Stamp> {
        let mut seen: Vec<&str> = vec![];
        let mut out = vec![];
        for s in self.stamps.iter().rev() {
            if !seen.contains(&s.preset.as_str()) {
                seen.push(&s.preset);
                out.push(s);
            }
        }
        out
    }
}

/// Adds `stamp` to the file at `path` (keeping each preset's newest).
pub fn record(path: &Path, stamp: Stamp) -> std::io::Result<()> {
    let mut v = Validations::load(path);
    v.stamps.push(stamp);
    let mut kept: Vec<Stamp> = vec![];
    for s in v.stamps.into_iter().rev() {
        if kept.iter().filter(|k| k.preset == s.preset).count() < KEEP {
            kept.push(s);
        }
    }
    kept.reverse();
    v.stamps = kept;
    v.save(path)
}

/// Notes in the file at `path` that opening these settings asked.
pub fn mark_asked(path: &Path, fingerprint: &str) -> std::io::Result<()> {
    let mut v = Validations::load(path);
    if !v.asked.iter().any(|a| a == fingerprint) {
        v.asked.push(fingerprint.to_string());
    }
    v.save(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(url: &str) -> ServerPreset {
        ServerPreset {
            name: "Remote".into(),
            servers: vec![ServerConfig::Connect { name: "proxy".into(), url: url.into(), device: String::new() }],
        }
    }

    fn stamp(config: &Config, p: &ServerPreset, outcome: Outcome) -> Stamp {
        Stamp {
            preset: p.name.clone(),
            fingerprint: fingerprint(config, p),
            when: "2026-10-31T17:00:00+01:00".into(),
            outcome,
            counts: Counts::default(),
            servers: vec![("proxy".into(), "sd-server fd21ecf".into())],
            suite: "increart-apitest 0.1.0".into(),
            report_sha256: "ab".into(),
            zip: None,
            seal: String::new(),
        }
        .sealed()
    }

    #[test]
    fn a_stamp_holds_for_the_settings_it_ran_with() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state").join(FILE);
        let config = Config::default();
        let p = preset("http://10.0.0.2:7860");
        assert_eq!(Validations::load(&path).status(&config, &p), Status::Never);
        record(&path, stamp(&config, &p, Outcome::Warn)).unwrap();
        let v = Validations::load(&path);
        assert!(matches!(v.status(&config, &p), Status::Passed(s) if s.day() == "2026-10-31"));
        // renamed: still its settings
        let renamed = ServerPreset { name: "Other".into(), ..p.clone() };
        assert!(matches!(v.status(&config, &renamed), Status::Passed(_)));
        // edited: validated before, not as it is
        let moved = preset("http://10.0.0.3:7860");
        assert!(matches!(v.status(&config, &moved), Status::Changed(_)));
        // a later failed run is what holds
        record(&path, stamp(&config, &p, Outcome::Fail)).unwrap();
        assert!(matches!(Validations::load(&path).status(&config, &p), Status::Failed(_)));
    }

    #[test]
    fn a_stamp_edited_by_hand_doesnt_hold() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE);
        let config = Config::default();
        let p = preset("http://10.0.0.2:7860");
        record(&path, stamp(&config, &p, Outcome::Fail)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap().replace("\"fail\"", "\"pass\"");
        std::fs::write(&path, text).unwrap();
        assert_eq!(Validations::load(&path).status(&config, &p), Status::Never);
    }

    #[test]
    fn a_started_sd_server_holds_for_the_sd_server_settings() {
        let mut config = Config::default();
        let p = ServerPreset {
            name: "Local".into(),
            servers: vec![ServerConfig::Start {
                name: "sd".into(),
                host: "127.0.0.1".into(),
                port: 1234,
                device: "ROCm0".into(),
                args: vec![],
            }],
        };
        let before = fingerprint(&config, &p);
        config.sdserver.binary = "/opt/sd-server".into();
        assert_ne!(fingerprint(&config, &p), before);
        // a connected one doesn't use them
        let c = preset("http://10.0.0.2:7860");
        assert_eq!(fingerprint(&Config::default(), &c), fingerprint(&config, &c));
    }

    #[test]
    fn asked_once_per_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE);
        let config = Config::default();
        let p = preset("http://10.0.0.2:7860");
        mark_asked(&path, &fingerprint(&config, &p)).unwrap();
        mark_asked(&path, &fingerprint(&config, &p)).unwrap();
        let v = Validations::load(&path);
        assert_eq!(v.asked.len(), 1);
        assert!(v.was_asked(&config, &p));
        assert!(!v.was_asked(&config, &preset("http://10.0.0.3:7860")));
    }

    #[test]
    fn basic_case_ids() {
        assert!(is_basic("Remote/proxy/generate.txt2img"));
        assert!(is_basic("Remote/together.end-to-end"));
        assert!(!is_basic("Remote/proxy/generate.large-request"));
    }
}
