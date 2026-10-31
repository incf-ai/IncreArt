//! Starting and stopping KoboldCpp (DESIGN.md, KoboldCpp): the command, its
//! own process group, the log, and stopping with the app. Prompt enhance
//! starts it for a chat model; image servers for an image model (and a chat
//! model beside it, to be prompt enhance's server too).

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::pe::{PeDevice, PeRun, PeServer, PeStart};

/// KoboldCpp as an image server, an entry of a server preset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "KoboldServer")]
pub struct KoboldServer {
    pub name: String,
    pub run: KoboldRun,
    /// Prompt enhance's chat server too: a started one runs this chat
    /// model beside the image model; a connected one is asked as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<KoboldChat>,
    /// A started one with a chat model: the chat model and the image model
    /// take turns, only one loaded at a time (for smaller cards).
    #[serde(default, skip_serializing_if = "is_false")]
    pub swap: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KoboldRun {
    /// The app starts it with admin mode on, and loads each run's model by
    /// restarting it with that model's settings.
    Start {
        /// A file, or a command line (`python3 …/koboldcpp.py`).
        binary: String,
        /// A `.kcpps` from KoboldCpp's launcher to start from.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        config: String,
        #[serde(default = "localhost")]
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "is_auto")]
        device: PeDevice,
        /// More arguments, last.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    /// One already running: it keeps the image model it runs.
    Connect {
        url: String,
        /// Its `--password`, if it has one.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        password: String,
        /// The device it runs on, for recipes (optional).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        device: String,
    },
}

/// The chat model a started KoboldCpp runs beside the image model.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "KoboldChat", default)]
pub struct KoboldChat {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Its vision projector (empty: text only).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mmproj: String,
    /// `--contextsize` (0: KoboldCpp's default).
    #[serde(skip_serializing_if = "is_zero")]
    pub context: u32,
}

fn localhost() -> String {
    "127.0.0.1".into()
}

fn is_auto(d: &PeDevice) -> bool {
    *d == PeDevice::Auto
}

fn is_false(b: &bool) -> bool {
    !b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl KoboldServer {
    pub fn base_url(&self) -> String {
        match &self.run {
            KoboldRun::Start { host, port, .. } => format!("http://{host}:{port}"),
            KoboldRun::Connect { url, .. } => url.trim_end_matches('/').to_string(),
        }
    }

    /// The device, for recipes: `Vulkan1`, `CUDA0`, `CPU`, or as
    /// configured.
    pub fn device(&self) -> String {
        match &self.run {
            KoboldRun::Start { device: PeDevice::Gpu { devices, runtime }, .. } => {
                let ids: Vec<String> = devices.iter().map(u32::to_string).collect();
                format!("{}{}", runtime.label(), ids.join(","))
            }
            KoboldRun::Start { device: PeDevice::Cpu { .. }, .. } => "CPU".into(),
            KoboldRun::Start { device: PeDevice::Auto, .. } => String::new(),
            KoboldRun::Connect { device, .. } => device.clone(),
        }
    }

    /// Prompt enhance's chat server, when it is one: asked at its address
    /// (a started one is running by the time it's asked); text only when
    /// started without a projector (and not from a `.kcpps` that may name
    /// one).
    pub fn chat_server(&self) -> Option<PeServer> {
        let chat = self.chat.as_ref()?;
        let run = PeRun::Connect { url: self.base_url() };
        let text_only = match &self.run {
            KoboldRun::Start { config, .. } => chat.mmproj.is_empty() && !(chat.model.is_empty() && !config.is_empty()),
            KoboldRun::Connect { .. } => false,
        };
        Some(PeServer { name: self.name.clone(), run, start: PeStart::OnDemand, text_only })
    }

    /// The launch command line: no image model (each is loaded by an
    /// admin reload), the chat model if any, admin on with `admin_dir`.
    pub fn start_args(&self, admin_dir: &Path, password: &str) -> Vec<String> {
        let KoboldRun::Start { config, host, port, device, args, .. } = &self.run else { return vec![] };
        let mut v: Vec<String> = vec![];
        if !config.is_empty() {
            v.extend(["--config".into(), config.clone()]);
        }
        v.extend(["--host".into(), host.clone(), "--port".into(), port.to_string()]);
        v.extend(["--skiplauncher", "--quiet"].map(String::from));
        v.extend(crate::pe::device_args(device));
        match self.chat.as_ref().filter(|c| !c.model.is_empty()) {
            Some(c) => {
                v.extend(["--model".into(), c.model.clone(), "--jinja".into()]);
                if !c.mmproj.is_empty() {
                    v.extend(["--mmproj".into(), c.mmproj.clone()]);
                }
                if c.context > 0 {
                    v.extend(["--contextsize".into(), c.context.to_string()]);
                }
            }
            None => v.push("--nomodel".into()),
        }
        v.extend(["--admin".into(), "--admindir".into(), admin_dir.to_string_lossy().into_owned()]);
        v.extend(["--adminpassword".into(), password.into()]);
        v.extend(args.iter().cloned());
        v
    }
}

/// Where [`admin_dir`] keeps `server`'s admin folder (not made here).
pub fn admin_path(state_dir: &Path, server: &str) -> PathBuf {
    let safe: String = server.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
    state_dir.join("koboldcpp").join(safe)
}

/// A started KoboldCpp's admin folder, `<state>/koboldcpp/<name>`, and its
/// admin password (made once and kept there, so a server left running
/// from an earlier session still takes the app's reloads).
pub fn admin_dir(state_dir: &Path, server: &str) -> std::io::Result<(PathBuf, String)> {
    let dir = admin_path(state_dir, server);
    fs::create_dir_all(&dir)?;
    owner_only(&dir, 0o700)?;
    let file = dir.join("password");
    if let Ok(p) = fs::read_to_string(&file)
        && !p.trim().is_empty()
    {
        return Ok((dir, p.trim().to_string()));
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    let password: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    fs::write(&file, &password)?;
    owner_only(&file, 0o600)?;
    Ok((dir, password))
}

/// Unix: only the owner may read it (`mode`). Elsewhere a user's own app
/// data folder is private already.
fn owner_only(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// A started KoboldCpp image server's log file name.
pub fn log_name(server: &str) -> String {
    format!("koboldcpp-{server}.log")
}

/// The command `binary` names: a path, or a command line with arguments
/// (`python3 /opt/koboldcpp/koboldcpp.py`) when no file has that name.
pub fn command(binary: &str) -> Result<Command, String> {
    let b = binary.trim();
    if b.is_empty() {
        return Err("no KoboldCpp binary set".into());
    }
    if Path::new(b).exists() {
        return Ok(Command::new(b));
    }
    let mut parts = b.split_whitespace();
    let mut c = Command::new(parts.next().unwrap_or(b));
    c.args(parts);
    Ok(c)
}

/// `binary --version`: KoboldCpp's version ("1.123.1").
pub fn version(binary: &str) -> Result<String, String> {
    let mut cmd = command(binary)?;
    cmd.arg("--version").stdin(Stdio::null()).stderr(Stdio::null());
    let out = crate::process::background(&mut cmd, false)
        .output()
        .map_err(|e| crate::process::spawn_error(binary.trim(), &e))?;
    let text = String::from_utf8_lossy(&out.stdout);
    match text.lines().map(str::trim).rfind(|l| !l.is_empty()) {
        Some(v) if out.status.success() => Ok(v.to_string()),
        _ => Err(format!("{}: no version ({})", binary.trim(), out.status)),
    }
}

/// KoboldCpp somewhere usual, for when nothing names it: on `PATH`, then
/// in the home folder, `~/Downloads`, `~/.local/bin` or a `~/koboldcpp*`
/// folder. Its release builds (`koboldcpp-linux-x64-nocuda`,
/// `koboldcpp-mac-arm64`, `koboldcpp_cu12.exe`, …) count.
pub fn find_binary() -> Option<String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    find_binary_in(&path, home.as_deref())
}

pub fn find_binary_in(path: &std::ffi::OsStr, home: Option<&Path>) -> Option<String> {
    let mut dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    if let Some(h) = home {
        dirs.extend([h.to_path_buf(), h.join("Downloads"), h.join(".local/bin")]);
        let mut own: Vec<PathBuf> = fs::read_dir(h)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().to_lowercase().starts_with("koboldcpp"))
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        own.sort();
        dirs.extend(own);
    }
    dirs.iter().find_map(|d| {
        let mut found: Vec<PathBuf> =
            fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| is_koboldcpp(p)).collect();
        // the plain name first, then the newest-looking release
        found.sort_by_key(|p| (stem(p) != "koboldcpp", std::cmp::Reverse(p.clone())));
        found.first().map(|p| p.to_string_lossy().into_owned())
    })
}

/// A KoboldCpp executable by its name: `koboldcpp`, or a release build
/// (`koboldcpp-…`, `koboldcpp_…`); `.exe` on Windows, no extension
/// elsewhere (not its `.py`, `.so` or a `.part` download).
fn is_koboldcpp(p: &Path) -> bool {
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let stem = stem(p);
    let named = cfg!(windows) == name.ends_with(".exe")
        && !stem.contains('.')
        && (stem == "koboldcpp" || stem.starts_with("koboldcpp-") || stem.starts_with("koboldcpp_"));
    named && fs::metadata(p).is_ok_and(|m| m.is_file() && executable(&m))
}

/// The file name, lowercase, without a Windows `.exe`.
fn stem(p: &Path) -> String {
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    name.strip_suffix(".exe").map(str::to_string).unwrap_or(name)
}

#[cfg(unix)]
fn executable(m: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &fs::Metadata) -> bool {
    true
}

/// Starts `binary` with `args`, its output appended to `log`, in a process
/// group of its own: its Linux build is a launcher that runs the server as
/// its child, so both are stopped with [`crate::process::stop`]. `with_app`: stopped too if the app dies
/// without stopping it (killed, crashed); the launcher passes the signal on
/// to the server it runs.
pub fn spawn(
    binary: &str,
    args: &[String],
    env: &[(String, String)],
    log: &Path,
    with_app: bool,
) -> Result<(Child, u64), String> {
    use std::io::Write;
    let log_error = |e: std::io::Error| format!("can't write its log {}: {e}", log.display());
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir).map_err(log_error)?;
    }
    let mut out = OpenOptions::new().create(true).append(true).open(log).map_err(log_error)?;
    let mut cmd = command(binary)?;
    crate::process::background(&mut cmd, true).args(args).envs(env.iter().cloned()).stdin(Stdio::null());
    let pipe = crate::process::output_to(&mut cmd, &out, with_app).map_err(log_error)?;
    // the command line first, and then where the server's output starts
    out.write_all(crate::process::log_header(&cmd).as_bytes()).map_err(log_error)?;
    let start = out.metadata().map_err(log_error)?.len();
    let program = cmd.get_program().to_string_lossy().into_owned();
    let child = if with_app { crate::process::spawn_with_app(cmd) } else { cmd.spawn() }.map_err(|e| {
        let why = crate::process::spawn_error(&program, &e);
        let _ = writeln!(out, "--- IncreArt: {why}");
        why
    })?;
    if let Some(pipe) = pipe {
        crate::process::stamp_into(pipe, out.try_clone().map_err(log_error)?);
    }
    Ok((child, start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(c: &Command) -> Vec<String> {
        std::iter::once(c.get_program()).chain(c.get_args()).map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn an_image_server_starts_empty_with_admin() {
        let mut k = KoboldServer {
            name: "k".into(),
            run: KoboldRun::Start {
                binary: "kcpp".into(),
                config: String::new(),
                host: "127.0.0.1".into(),
                port: 5002,
                device: PeDevice::Gpu { devices: vec![1], runtime: Default::default() },
                args: vec!["--sdflashattention".into()],
            },
            chat: None,
            swap: false,
        };
        assert_eq!(
            k.start_args(Path::new("/a"), "pw").join(" "),
            "--host 127.0.0.1 --port 5002 --skiplauncher --quiet --usevulkan 1 --gpulayers 999 --nomodel --admin --admindir /a --adminpassword pw --sdflashattention"
        );
        assert_eq!(k.device(), "Vulkan1");
        assert!(k.chat_server().is_none());
        k.chat = Some(KoboldChat { model: "c.gguf".into(), mmproj: String::new(), context: 8192 });
        assert!(
            k.start_args(Path::new("/a"), "pw")
                .join(" ")
                .contains(" --model c.gguf --jinja --contextsize 8192 --admin ")
        );
        assert_eq!(k.chat_server().unwrap().base_url(), "http://127.0.0.1:5002");
        let dir = tempfile::tempdir().unwrap();
        let (d, pw) = admin_dir(dir.path(), "gpu 1").unwrap();
        assert_eq!((d.file_name().unwrap().to_str(), pw.len()), (Some("gpu_1"), 32));
        assert_eq!(admin_dir(dir.path(), "gpu 1").unwrap().1, pw, "kept");
    }

    #[test]
    fn a_binary_is_a_path_or_a_command_line() {
        assert_eq!(argv(&command("/usr/bin/env").unwrap()), ["/usr/bin/env"]);
        assert_eq!(argv(&command(" python3 /opt/kcpp/koboldcpp.py ").unwrap()), ["python3", "/opt/kcpp/koboldcpp.py"]);
        assert!(command("  ").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn started_with_its_output_in_the_log_and_stopped() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("logs/k.log");
        let (child, start) = spawn("sh", &["-c".into(), "echo up; sleep 30".into()], &[], &log, false).unwrap();
        let output = || fs::read_to_string(&log).unwrap_or_default()[start as usize..].to_string();
        let deadline = Instant::now() + Duration::from_secs(5);
        while output().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(output(), "up\n");
        let header = &fs::read_to_string(&log).unwrap()[..start as usize];
        assert!(
            header.starts_with("\n--- IncreArt, ") && header.ends_with(": sh -c \"echo up; sleep 30\"\n"),
            "{header:?}"
        );
        let t = Instant::now();
        crate::process::stop(child);
        assert!(t.elapsed() < Duration::from_secs(4), "stopped by TERM, not waited out");
    }

    /// Stopped with the app: its lines (stdout and stderr) get the time they
    /// came; a line rewritten with `\r` once, at its start.
    #[cfg(unix)]
    #[test]
    fn a_server_stopped_with_the_app_has_its_lines_timed() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("k.log");
        let script = "printf 'up\\n 1/2\\r 2/2\\n'; echo oops >&2; sleep 30";
        let (child, start) = spawn("sh", &["-c".into(), script.into()], &[], &log, true).unwrap();
        let output = || fs::read_to_string(&log).unwrap_or_default()[start as usize..].to_string();
        let deadline = Instant::now() + Duration::from_secs(5);
        while output().lines().count() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        crate::process::stop(child);
        let out = output();
        let lines: Vec<(&str, &str)> = out.lines().map(crate::logview::split_stamp).collect();
        assert!(lines.iter().all(|(stamp, _)| !stamp.is_empty()), "{out:?}");
        assert_eq!(lines.iter().map(|l| l.1).collect::<Vec<_>>(), ["up", " 1/2\r 2/2", "oops"]);
    }

    #[test]
    fn a_binary_that_isnt_there_is_said_in_its_log_too() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("k.log");
        let missing = dir.path().join("no").join("koboldcpp");
        let why = spawn(&missing.to_string_lossy(), &["--port".into(), "5001".into()], &[], &log, false).unwrap_err();
        assert_eq!(why, format!("{} isn't there", missing.display()));
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.contains(" --port 5001\n"), "{text:?}");
        assert!(text.ends_with(&format!("--- IncreArt: {why}\n")), "{text:?}");
    }

    #[cfg(unix)]
    #[test]
    fn koboldcpp_found_on_the_path_or_at_home() {
        use std::os::unix::fs::PermissionsExt;
        let make = |p: &Path, exec: bool| {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(if exec { 0o755 } else { 0o644 })).unwrap();
        };
        let t = tempfile::tempdir().unwrap();
        let (bin, home) = (t.path().join("bin"), t.path().join("home"));
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&home).unwrap();
        let path = std::env::join_paths([&bin]).unwrap();
        assert_eq!(find_binary_in(&path, Some(&home)), None);
        make(&home.join("Downloads/koboldcpp-linux-x64-nocuda.part"), true);
        make(&home.join("koboldcpp.py"), true);
        make(&home.join("koboldcpp_vulkan.so"), true);
        make(&home.join("koboldcpp_cu12.exe"), true);
        make(&home.join("koboldcpp"), false);
        assert_eq!(find_binary_in(&path, Some(&home)), None, "not executable, a part, a script, a library, Windows'");
        let release = home.join("koboldcpp-1.123/koboldcpp-linux-x64-nocuda");
        make(&release, true);
        assert_eq!(find_binary_in(&path, Some(&home)).as_deref(), Some(release.to_str().unwrap()));
        make(&bin.join("koboldcpp"), true);
        assert_eq!(find_binary_in(&path, Some(&home)).as_deref(), Some(bin.join("koboldcpp").to_str().unwrap()));
    }
}
