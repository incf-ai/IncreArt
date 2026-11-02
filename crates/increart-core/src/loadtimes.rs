//! How long each server took to load each model (DESIGN.md, Timing), in
//! `load-times.rsn` in the app's state folder, for the time-left
//! estimates: a run whose model no server has waits for a load. Kept per
//! machine, not per library: it's the servers' and disks' speed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "load-times.rsn";

/// `load-times.rsn` in the app's state folder.
pub fn path() -> Option<PathBuf> {
    crate::config::user_dir("XDG_STATE_HOME", ".local/state", "LOCALAPPDATA").map(|d| d.join(FILE))
}

/// Seconds to load, by server name, then model recipe.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadTimes {
    pub servers: BTreeMap<String, BTreeMap<String, f32>>,
}

impl LoadTimes {
    /// Empty when there's none or it can't be read. A `load-times.json`
    /// from before RSN becomes `load-times.rsn` first.
    pub fn load(path: &Path) -> LoadTimes {
        crate::rsnfmt::convert_json::<LoadTimes>(path);
        crate::rsnfmt::read(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> crate::error::Result<()> {
        crate::rsnfmt::write(path, self)
    }

    /// A load that took `s` seconds: the mean with the last one, so one
    /// slow load (a cold disk cache) counts but doesn't take over.
    pub fn record(&mut self, server: &str, recipe: &str, s: f32) {
        let t = self.servers.entry(server.into()).or_default().entry(recipe.into()).or_insert(s);
        *t = (*t + s) / 2.0;
    }

    /// Roughly how long `server` takes to load `recipe`: its own time,
    /// else the model's mean on the other servers, else the server's mean
    /// for the other models.
    pub fn get(&self, server: &str, recipe: &str) -> Option<f32> {
        let mean = |v: Vec<f32>| (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32);
        self.servers
            .get(server)
            .and_then(|m| m.get(recipe).copied())
            .or_else(|| mean(self.servers.values().filter_map(|m| m.get(recipe).copied()).collect()))
            .or_else(|| self.servers.get(server).and_then(|m| mean(m.values().copied().collect())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_servers_load_else_the_models_else_the_servers() {
        let mut t = LoadTimes::default();
        assert_eq!(t.get("gpu0", "qwen"), None);
        t.record("gpu0", "qwen", 40.0);
        assert_eq!(t.get("gpu0", "qwen"), Some(40.0));
        t.record("gpu0", "qwen", 60.0);
        assert_eq!(t.get("gpu0", "qwen"), Some(50.0), "the mean with the last");
        t.record("gpu1", "qwen", 30.0);
        t.record("gpu1", "krea2", 20.0);
        assert_eq!(t.get("gpu0", "krea2"), Some(20.0), "the model's on another server");
        assert_eq!(t.get("gpu2", "qwen"), Some(40.0), "the model's mean");
        assert_eq!(t.get("gpu1", "flux"), Some(25.0), "the server's mean");
        assert_eq!(t.get("gpu2", "flux"), None);

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE);
        t.save(&p).unwrap();
        assert_eq!(LoadTimes::load(&p), t);
        assert_eq!(LoadTimes::load(&dir.path().join("none.rsn")), LoadTimes::default());
    }
}
