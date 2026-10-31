//! The run's report: `report.md` (a summary, then each case with what
//! IncreArt expects, what happened, its log and its traffic: for a person,
//! or to paste into an LLM), `report.json`, all the traffic, and the logs
//! of the servers the suite started; zipped to hand on.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use increart_core::apitest::{Case, Counts, Event, Outcome};
use serde::Serialize;
use serde_json::{Value, json};

/// Per case, the traffic shown in `report.md` (all of it in `traffic.log`).
const SHOWN_TRAFFIC: usize = 60;

#[derive(Clone, Debug, Default, Serialize)]
pub struct CaseResult {
    pub outcome: Option<Outcome>,
    pub summary: String,
    pub secs: f64,
    pub log: Vec<String>,
    pub traffic: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub started: chrono::DateTime<chrono::Local>,
    pub version: String,
    pub config_path: Option<PathBuf>,
    /// The presets tested, as config.rsn has them (passwords hidden).
    pub presets_text: String,
    pub cases: Vec<Case>,
    pub results: BTreeMap<String, CaseResult>,
    /// Lines between cases.
    pub run_log: Vec<String>,
    /// All of it, each marked with its case.
    pub traffic: Vec<String>,
}

impl Report {
    pub fn new(version: String, config_path: Option<PathBuf>, presets_text: String) -> Report {
        Report {
            started: chrono::Local::now(),
            version,
            config_path,
            presets_text,
            cases: vec![],
            results: BTreeMap::new(),
            run_log: vec![],
            traffic: vec![],
        }
    }

    pub fn record(&mut self, e: &Event) {
        match e {
            Event::Plan { cases } => self.cases = cases.clone(),
            Event::Log { id: Some(id), text } => self.results.entry(id.clone()).or_default().log.push(text.clone()),
            Event::Log { id: None, text } => self.run_log.push(text.clone()),
            Event::Traffic { id, text } => {
                let mark = id.as_deref().unwrap_or("between cases");
                self.traffic.push(format!("[{mark}] {text}"));
                if let Some(id) = id {
                    self.results.entry(id.clone()).or_default().traffic.push(text.clone());
                }
            }
            Event::End { id, outcome, summary, secs } => {
                let r = self.results.entry(id.clone()).or_default();
                r.outcome = Some(*outcome);
                r.summary = summary.clone();
                r.secs = *secs;
            }
            Event::Start { .. } | Event::Done { .. } | Event::Error { .. } => {}
        }
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for r in self.results.values() {
            if let Some(o) = r.outcome {
                c.add(o);
            }
        }
        c
    }

    fn result(&self, id: &str) -> CaseResult {
        self.results.get(id).cloned().unwrap_or_default()
    }

    pub fn markdown(&self) -> String {
        let mut md = String::new();
        let date = self.started.format("%Y-%m-%d %H:%M");
        md.push_str(&format!("# IncreArt server validation, {date}\n\n"));
        md.push_str(&format!(
            "{}; {} {}; config {}.\n\n**{}.**\n\n",
            self.version,
            std::env::consts::OS,
            std::env::consts::ARCH,
            self.config_path.as_ref().map_or("(none)".into(), |p| p.display().to_string()),
            self.counts().line()
        ));
        md.push_str(
            "## How to read this\n\n\
             IncreArt is an image app. It generates through stable-diffusion.cpp's `sd-server` HTTP API \
             (`/sdcpp/v1/...`: capabilities, model-files, models/status, models/load, img_gen, jobs/{id}, \
             jobs/{id}/cancel), or KoboldCpp's through the same client, and asks an OpenAI-compatible chat \
             server (`/v1/chat/completions`) for prompt enhance. A server preset is a set of such servers \
             used together. Each case below says what IncreArt expects of the server, what happened, the \
             case's log and the requests it made with their answers (base64 data shown as its length; long \
             bodies cut). A failure means the app will misbehave with that server as it is; a warning means \
             it works, but not as well as it could. To find what's wrong, compare a failed case's traffic \
             with what it expects.\n\n",
        );
        md.push_str("## Summary\n\n| Result | Case | Summary |\n|---|---|---|\n");
        for c in &self.cases {
            let r = self.result(&c.id);
            let label = r.outcome.map_or("not run", Outcome::label);
            md.push_str(&format!("| {label} | `{}` | {} |\n", c.id, r.summary.replace('|', "\\|").replace('\n', " ")));
        }
        if !self.presets_text.is_empty() {
            md.push_str(&format!("\n## The presets tested\n\n```\n{}\n```\n", self.presets_text.trim_end()));
        }
        if !self.run_log.is_empty() {
            md.push_str(&format!("\n## The run\n\n```text\n{}\n```\n", self.run_log.join("\n")));
        }
        md.push_str("\n## Cases\n");
        for c in &self.cases {
            let r = self.result(&c.id);
            let label = r.outcome.map_or("not run", Outcome::label);
            md.push_str(&format!("\n### `{}`: {label}\n\n", c.id));
            md.push_str(&format!("**{}** ({}, {:.1} s)\n\n", c.title, c.cost.label(), r.secs));
            md.push_str(&format!("Expects: {}\n\n", c.expects));
            if !r.summary.is_empty() {
                md.push_str(&format!("Result: {}\n\n", r.summary));
            }
            if let Some(h) = r.outcome.and_then(|o| increart_core::apitest::hint(o, &r.summary)) {
                md.push_str(&format!("Try: {h}\n\n"));
            }
            if !r.log.is_empty() {
                md.push_str(&format!("Log:\n\n```text\n{}\n```\n\n", r.log.join("\n")));
            }
            if !r.traffic.is_empty() {
                let shown = r.traffic.len().min(SHOWN_TRAFFIC);
                md.push_str(&format!("Traffic ({} requests", r.traffic.len()));
                if shown < r.traffic.len() {
                    md.push_str(&format!(", the first {shown}: all in traffic.log"));
                }
                md.push_str("):\n\n```text\n");
                md.push_str(&r.traffic[..shown].join("\n"));
                md.push_str("\n```\n");
            }
        }
        md
    }

    pub fn json(&self) -> Value {
        let cases: Vec<Value> = self
            .cases
            .iter()
            .map(|c| {
                let mut v = serde_json::to_value(c).unwrap_or_default();
                v["result"] = serde_json::to_value(self.result(&c.id)).unwrap_or_default();
                v
            })
            .collect();
        json!({
            "version": self.version,
            "started": self.started.to_rfc3339(),
            "os": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            "config": self.config_path,
            "counts": self.counts(),
            "presets": self.presets_text,
            "run_log": self.run_log,
            "cases": cases,
        })
    }

    /// Writes it into `dir` and zips it beside (`<dir>.zip`): the zip.
    pub fn write(&self, dir: &Path) -> std::io::Result<PathBuf> {
        fs::create_dir_all(dir)?;
        fs::write(dir.join("report.md"), self.markdown())?;
        fs::write(dir.join("report.json"), serde_json::to_string_pretty(&self.json()).unwrap_or_default())?;
        fs::write(dir.join("traffic.log"), self.traffic.join("\n") + "\n")?;
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("increart-apitest").to_string();
        let zip_path = dir.with_file_name(format!("{name}.zip"));
        let mut files = vec!["report.md".to_string(), "report.json".into(), "traffic.log".into()];
        if let Ok(rd) = fs::read_dir(dir.join("servers")) {
            let mut logs: Vec<String> = rd
                .flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| n.ends_with(".log"))
                .map(|n| format!("servers/{n}"))
                .collect();
            logs.sort();
            files.extend(logs);
        }
        let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path)?);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for f in files {
            let bytes = fs::read(dir.join(&f))?;
            zip.start_file(format!("{name}/{f}"), opts).map_err(std::io::Error::other)?;
            zip.write_all(&bytes)?;
        }
        zip.finish().map_err(std::io::Error::other)?;
        Ok(zip_path)
    }
}

/// The presets as config.rsn has them, passwords as `***`.
pub fn presets_text(presets: &[increart_core::config::ServerPreset]) -> String {
    let text = increart_core::rsnfmt::to_string(&presets.to_vec())
        .unwrap_or_else(|e| format!("(the presets couldn't be written out: {e})"));
    text.lines()
        .map(|l| match l.find("password: \"") {
            Some(at) => format!("{}password: \"***\",", &l[..at]),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
