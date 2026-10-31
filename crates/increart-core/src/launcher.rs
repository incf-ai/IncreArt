//! Starting sd-server processes, or attaching to ones already running
//! (DESIGN.md, Backend).

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::config::{SdServerConfig, ServerConfig};
use crate::sdserver::Client;

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("{name}: no server answers on {url}")]
    NotRunning { name: String, url: String },
    /// Not started at all ([`crate::process::spawn_error`]; also in its log).
    #[error("{name}: {why}")]
    Spawn { name: String, why: String },
    #[error("{name}: the server exited ({status}) during start-up; log {}:\n{tail}", log.display())]
    Exited { name: String, status: String, log: PathBuf, tail: String },
    #[error("{name}: opening cancelled")]
    Cancelled { name: String },
    #[error(
        "{name}: the server didn't answer, its log quiet for {secs} s (the server start timeout); log {}",
        log.display()
    )]
    Timeout { name: String, secs: u64, log: PathBuf },
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// A server the app talks to. Dropping it stops the process if the app
/// started it; an attached server is left running.
#[derive(Debug)]
pub struct ServerProcess {
    pub name: String,
    pub device: String,
    pub client: Client,
    child: Option<Child>,
    /// The log file, for a launched server.
    pub log: Option<PathBuf>,
    /// KoboldCpp: its process group is stopped, not just the launcher.
    group: bool,
}

impl ServerProcess {
    /// Whether the app started this process (vs attached to it).
    pub fn launched(&self) -> bool {
        self.child.is_some()
    }

    /// Lets a started server outlive the app: dropping this no longer stops it.
    pub fn detach(&mut self) {
        self.child = None;
    }

    pub fn stop(&mut self) {
        match self.child.take() {
            Some(child) if self.group => crate::process::stop(child),
            Some(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            None => {}
        }
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The command line for a `Start` server (empty for `Connect`).
pub fn command_args(server: &ServerConfig, shared: &SdServerConfig) -> Vec<String> {
    let ServerConfig::Start { host, port, device, args: own, .. } = server else { return vec![] };
    let backend = match visible_device(device) {
        Some((_, prefix, _)) => format!("{prefix}0"),
        None => device.clone(),
    };
    let mut args: Vec<String> =
        vec!["--listen-ip".into(), host.clone(), "--listen-port".into(), port.to_string(), "--backend".into(), backend];
    for dir in shared.model_dirs.iter().filter(|d| !d.trim().is_empty()) {
        args.extend(["--model-dir".into(), dir.clone()]);
    }
    args.extend(shared.args.iter().cloned());
    args.extend(own.iter().cloned());
    args
}

/// For a ROCm or CUDA `--backend` device: the variable that hides the other
/// GPUs from sd-server, the device's prefix and its number. Otherwise every
/// server opens every GPU, and one GPU's hung firmware stalls them all (each
/// memory map waits ~2 s on its TLB flush).
fn visible_device(device: &str) -> Option<(&'static str, &str, &str)> {
    let split = device.find(|c: char| c.is_ascii_digit())?;
    let (prefix, n) = device.split_at(split);
    if !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let var = match prefix.to_lowercase().as_str() {
        "rocm" => "HIP_VISIBLE_DEVICES",
        "cuda" => "CUDA_VISIBLE_DEVICES",
        _ => return None,
    };
    Some((var, prefix, n))
}

/// The environment for [`command_args`].
pub fn command_env(server: &ServerConfig) -> Vec<(String, String)> {
    let ServerConfig::Start { device, .. } = server else { return vec![] };
    visible_device(device).map(|(var, _, n)| vec![(var.into(), n.into())]).unwrap_or_default()
}

/// The command line the app starts `server` with, as its log's header shows
/// it ([`crate::process::command_text`]); `None` for one it connects to.
/// `state_dir` holds a started KoboldCpp's admin folder.
pub fn command_line(
    server: &ServerConfig,
    shared: &SdServerConfig,
    state_dir: &Path,
) -> Option<Result<String, String>> {
    use crate::kobold::KoboldRun;
    use crate::pe::PeRun;
    let (cmd, args, env) = match server {
        ServerConfig::Start { .. } => {
            (Ok(Command::new(shared.binary.trim())), command_args(server, shared), command_env(server))
        }
        ServerConfig::Connect { .. } => return None,
        ServerConfig::Chat(s) => match &s.run {
            PeRun::KoboldCpp { binary, .. } => {
                (crate::kobold::command(binary), crate::pe::command_args(&s.run), crate::pe::run_env(&s.run))
            }
            PeRun::Connect { .. } => return None,
        },
        ServerConfig::Kobold(k) => match &k.run {
            KoboldRun::Start { binary, device, .. } => {
                let dir = crate::kobold::admin_path(state_dir, &k.name);
                (crate::kobold::command(binary), k.start_args(&dir, "***"), crate::pe::device_env(device))
            }
            KoboldRun::Connect { .. } => return None,
        },
    };
    Some(cmd.map(|mut c| {
        c.args(args).envs(env);
        crate::process::command_text(&c)
    }))
}

/// Opens one server of the chosen preset. `Connect` checks that it answers.
/// `Start` launches sd-server, logging to `log_dir/sd-server-<name>.log`,
/// and waits for it to answer (given up after `wait` without a new line in
/// its log); if an sd-server already answers on that port (e.g. left over
/// from a crash) it is used as is.
pub fn open(
    server: &ServerConfig,
    shared: &SdServerConfig,
    log_dir: &Path,
    wait: Duration,
) -> Result<ServerProcess, LaunchError> {
    open_cancellable(server, shared, log_dir, wait, &std::sync::atomic::AtomicBool::new(false))
}

/// As [`open`]; setting `cancel` stops the wait (and a server it started)
/// within 200 ms.
pub fn open_cancellable(
    server: &ServerConfig,
    shared: &SdServerConfig,
    log_dir: &Path,
    wait: Duration,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<ServerProcess, LaunchError> {
    if let ServerConfig::Kobold(k) = server {
        return open_kobold(k, shared, log_dir, wait, cancel);
    }
    let name = server.name().to_string();
    let client = Client::new(&server.base_url());
    let mut proc = ServerProcess {
        name: name.clone(),
        device: server.device(),
        client: client.clone(),
        child: None,
        log: None,
        group: false,
    };
    if client.capabilities().is_ok() {
        return Ok(proc);
    }
    if matches!(server, ServerConfig::Connect { .. }) {
        return Err(LaunchError::NotRunning { name, url: server.base_url() });
    }
    fs::create_dir_all(log_dir)?;
    let log = log_path(log_dir, server);
    let mut out = OpenOptions::new().create(true).append(true).open(&log)?;
    let mut cmd = Command::new(shared.binary.trim());
    cmd.args(command_args(server, shared)).envs(command_env(server)).stdin(Stdio::null());
    let pipe = crate::process::output_to(&mut cmd, &out, !shared.leave_running)?;
    crate::process::background(&mut cmd, false);
    out.write_all(crate::process::log_header(&cmd).as_bytes())?;
    let start = out.metadata()?.len();
    let program = shared.binary.trim().to_string();
    let spawned = if shared.leave_running { cmd.spawn() } else { crate::process::spawn_with_app(cmd) };
    let child = spawned.map_err(|e| {
        let why = crate::process::spawn_error(&program, &e);
        let _ = writeln!(out, "--- IncreArt: {why}");
        LaunchError::Spawn { name: name.clone(), why }
    })?;
    if let Some(pipe) = pipe {
        crate::process::stamp_into(pipe, out.try_clone()?);
    }
    proc.child = Some(child);
    proc.log = Some(log.clone());
    wait_up(proc, log, start, wait, cancel)
}

/// KoboldCpp generating images: `Connect` checks it answers; `Start`
/// launches it with admin on (its folder and password in the state dir,
/// beside `log_dir`), logging to `log_dir/koboldcpp-<name>.log`; one
/// already answering on the port is used as is.
fn open_kobold(
    k: &crate::kobold::KoboldServer,
    shared: &SdServerConfig,
    log_dir: &Path,
    wait: Duration,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<ServerProcess, LaunchError> {
    use crate::kobold::{KoboldRun, admin_dir};
    use crate::kobold_image::Admin;
    let name = k.name.clone();
    let (client, binary, args, env, log) = match &k.run {
        KoboldRun::Connect { url, password, .. } => (Client::kobold(url, password, None)?, None, vec![], vec![], None),
        KoboldRun::Start { binary, device, .. } => {
            let (dir, password) = admin_dir(log_dir.parent().unwrap_or(log_dir), &name)?;
            let args = k.start_args(&dir, &password);
            let log = log_dir.join(crate::kobold::log_name(&name));
            let admin = Admin {
                binary: binary.clone(),
                args: args.clone(),
                dir,
                password,
                model_dirs: shared.model_dirs.clone(),
                log: Some(log.clone()),
                swap: k.swap && k.chat.as_ref().is_some_and(|c| !c.model.is_empty()),
            };
            let env = crate::pe::device_env(device);
            (Client::kobold(&k.base_url(), "", Some(admin))?, Some(binary.clone()), args, env, Some(log))
        }
    };
    let mut proc = ServerProcess {
        name: name.clone(),
        device: k.device(),
        client: client.clone(),
        child: None,
        log: None,
        group: true,
    };
    if client.capabilities().is_ok() {
        return Ok(proc);
    }
    let (Some(binary), Some(log)) = (binary, log) else {
        return Err(LaunchError::NotRunning { name, url: k.base_url() });
    };
    fs::create_dir_all(log_dir)?;
    let (child, start) = crate::kobold::spawn(&binary, &args, &env, &log, !shared.leave_running)
        .map_err(|why| LaunchError::Spawn { name: name.clone(), why })?;
    proc.child = Some(child);
    proc.log = Some(log.clone());
    wait_up(proc, log, start, wait, cancel)
}

/// Waits for a started server to answer (its log since `start` says why
/// if it exits).
fn wait_up(
    mut proc: ServerProcess,
    log: PathBuf,
    start: u64,
    wait: Duration,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<ServerProcess, LaunchError> {
    let name = proc.name.clone();
    let client = proc.client.clone();
    let mut stall = crate::process::LogStall::new(&log, wait);
    loop {
        if client.capabilities().is_ok() {
            return Ok(proc);
        }
        if let Some(status) = proc.child.as_mut().unwrap().try_wait()? {
            proc.child = None;
            return Err(LaunchError::Exited { name, status: status.to_string(), tail: tail(&log, start), log });
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            proc.stop();
            return Err(LaunchError::Cancelled { name });
        }
        if stall.stalled() {
            return Err(LaunchError::Timeout { name, secs: wait.as_secs(), log });
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Where a started server logs.
pub fn log_path(log_dir: &Path, server: &ServerConfig) -> PathBuf {
    match server {
        ServerConfig::Kobold(k) => log_dir.join(crate::kobold::log_name(&k.name)),
        _ => log_dir.join(log_name(server.name())),
    }
}

/// A started sd-server's log file name, by the server's name.
pub fn log_name(server: &str) -> String {
    format!("sd-server-{server}.log")
}

/// What a starting server is doing, from its log since byte `start`: the
/// latest line, tidied ("loading tensors 294/396 · 4.38GB/s", "loading
/// tensors completed, taking 1.32s"). Progress bars redraw with `\r`.
pub fn latest_line(log: &Path, start: u64) -> Option<String> {
    let mut f = fs::File::open(log).ok()?;
    let len = f.metadata().ok()?.len();
    let from = start.max(len.saturating_sub(4096));
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut last = None;
    for line in text.split(['\r', '\n']) {
        let line = line.replace("\x1b[K", "").replace("[K", "");
        let line = line.trim();
        if !line.is_empty() {
            last = Some(line.to_string());
        }
    }
    Some(tidy_line(&last?))
}

/// "[I] loading tensors completed --- model_loader.cpp:1405" → "loading
/// tensors completed"; "|####   | 294/396 - 4.38GB/s" → "294/396 · 4.38GB/s".
fn tidy_line(line: &str) -> String {
    let line = crate::logview::unstamped(line);
    let line = line.split(" --- ").next().unwrap_or(line);
    let line = ["[I] ", "[W] ", "[E] ", "[D] "].iter().find_map(|p| line.strip_prefix(p)).unwrap_or(line);
    let line = match line.strip_prefix('|').and_then(|l| l.split_once('|')) {
        Some((_, rest)) => rest.trim().replace(" - ", " · "),
        None => line.trim().to_string(),
    };
    line.chars().take(120).collect()
}

/// The last 2 KB written to `log` since byte `start`, without the times
/// before its lines.
fn tail(log: &Path, start: u64) -> String {
    let Ok(mut f) = fs::File::open(log) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = start.max(len.saturating_sub(2048));
    let mut s = String::new();
    let _ = f.seek(SeekFrom::Start(from)).and_then(|_| f.read_to_string(&mut s));
    s.split_inclusive('\n').map(crate::logview::unstamped).collect()
}

/// `sd-server --list-devices`, one line per device as printed.
pub fn list_devices(binary: &str) -> std::io::Result<Vec<String>> {
    let mut cmd = Command::new(binary);
    let out = crate::process::background(&mut cmd, false).arg("--list-devices").stdin(Stdio::null()).output()?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// The device name a `--list-devices` line starts with (`ROCm0` in
/// "ROCm0\tAMD Radeon …").
pub fn device_name(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("").trim_end_matches(':')
}

/// The configured device name as `--list-devices` spells it (matched
/// case-insensitively), if listed.
pub fn match_device<'a>(listed: &'a [String], wanted: &str) -> Option<&'a str> {
    let w = wanted.to_lowercase();
    listed.iter().map(String::as_str).find(|l| device_name(l).to_lowercase() == w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_starting_servers_latest_log_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("sd-server-gpu0.log");
        fs::write(&log, "old run\n").unwrap();
        let start = fs::metadata(&log).unwrap().len();
        assert_eq!(latest_line(&log, start), None, "nothing since the start");
        let bars = "\n  |######     |  41/396 - 158.91MB/s\x1b[K\r  |#######    | 294/396 -   4.38GB/s\x1b[K";
        fs::write(&log, format!("old run\n[I] loading model from 'q.gguf' --- model.cpp:12{bars}")).unwrap();
        assert_eq!(latest_line(&log, start).unwrap(), "294/396 ·   4.38GB/s");
        let done = "\n[I] loading tensors completed, taking 1.32s (read: 0.08s) --- model_loader.cpp:1405\n\n";
        fs::write(&log, format!("old run\n{bars}{done}")).unwrap();
        assert_eq!(latest_line(&log, start).unwrap(), "loading tensors completed, taking 1.32s (read: 0.08s)");
    }

    #[test]
    fn args() {
        let server = ServerConfig::Start {
            name: "gpu1".into(),
            host: "127.0.0.1".into(),
            port: 1235,
            device: "ROCm1".into(),
            args: vec!["--offload-to-cpu".into()],
        };
        let shared = SdServerConfig { model_dirs: vec!["/m".into()], ..Default::default() };
        assert_eq!(
            command_args(&server, &shared).join(" "),
            "--listen-ip 127.0.0.1 --listen-port 1235 --backend ROCm0 --model-dir /m --fa --conditioning-cache-size 32 --offload-to-cpu"
        );
        assert_eq!(command_env(&server), vec![("HIP_VISIBLE_DEVICES".to_string(), "1".to_string())]);
        let vulkan = ServerConfig::Start {
            name: "v".into(),
            host: "127.0.0.1".into(),
            port: 1236,
            device: "Vulkan1".into(),
            args: vec![],
        };
        assert!(command_args(&vulkan, &shared).join(" ").contains("--backend Vulkan1"));
        assert!(command_env(&vulkan).is_empty());
        let remote = ServerConfig::Connect { name: "box".into(), url: "http://h:1".into(), device: String::new() };
        assert!(command_args(&remote, &shared).is_empty());
    }

    #[test]
    fn command_lines_as_logs_show_them() {
        let shared =
            SdServerConfig { binary: "/opt/sd server".into(), model_dirs: vec!["/m".into()], ..Default::default() };
        let server = ServerConfig::Start {
            name: "gpu1".into(),
            host: "127.0.0.1".into(),
            port: 1235,
            device: "ROCm1".into(),
            args: vec![],
        };
        let state = Path::new("/state");
        assert!(
            command_line(&server, &shared, state)
                .unwrap()
                .unwrap()
                .starts_with("HIP_VISIBLE_DEVICES=1 \"/opt/sd server\" --listen-ip 127.0.0.1 --listen-port 1235 ")
        );
        let k = crate::kobold::KoboldServer {
            name: "k 1".into(),
            run: crate::kobold::KoboldRun::Start {
                binary: "python3 koboldcpp.py".into(),
                config: String::new(),
                host: "127.0.0.1".into(),
                port: 5002,
                device: Default::default(),
                args: vec![],
            },
            chat: None,
            swap: false,
        };
        let line = command_line(&ServerConfig::Kobold(k.clone()), &shared, state).unwrap().unwrap();
        let dir = state.join("koboldcpp").join("k_1");
        assert!(line.starts_with("python3 koboldcpp.py --host 127.0.0.1 --port 5002 "), "{line}");
        assert!(
            line.ends_with(&format!(" --nomodel --admin --admindir {} --adminpassword ***", dir.display())),
            "{line}"
        );
        assert!(!dir.exists(), "only shown, nothing made");
        let empty = crate::kobold::KoboldServer {
            run: crate::kobold::KoboldRun::Start {
                binary: " ".into(),
                config: String::new(),
                host: "h".into(),
                port: 1,
                device: Default::default(),
                args: vec![],
            },
            ..k
        };
        assert!(command_line(&ServerConfig::Kobold(empty), &shared, state).unwrap().is_err());
        let remote = ServerConfig::Connect { name: "box".into(), url: "http://h:1".into(), device: String::new() };
        assert!(command_line(&remote, &shared, state).is_none());
    }

    #[test]
    fn devices() {
        let listed = vec![
            "CPU: cpu".to_string(),
            "ROCm0: AMD Radeon AI PRO R9700".into(),
            "ROCm1: AMD Radeon AI PRO R9700".into(),
        ];
        assert_eq!(match_device(&listed, "rocm1"), Some("ROCm1: AMD Radeon AI PRO R9700"));
        assert_eq!(match_device(&listed, "Vulkan0"), None);
    }
}
