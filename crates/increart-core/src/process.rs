//! Starting and stopping the server processes the app runs, the same on
//! Linux, macOS and Windows (DESIGN.md, Backend): a started server stops
//! with the app even when the app dies without stopping it (killed,
//! crashed), a server that runs helpers of its own (KoboldCpp) is stopped
//! with them, and on Windows no console window opens for any of them.

use std::io;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Readies `cmd` to run in the background: on Windows without a console
/// window, and with `group`, in a process group of its own (see [`stop`]).
/// A server starting: given up on once its log has had no new output for
/// `wait` (loading prints as it goes, so a slow load isn't cut off; a hung
/// one is).
pub struct LogStall {
    log: std::path::PathBuf,
    len: u64,
    until: Instant,
    wait: Duration,
}

impl LogStall {
    pub fn new(log: &std::path::Path, wait: Duration) -> LogStall {
        let len = std::fs::metadata(log).map_or(0, |m| m.len());
        LogStall { log: log.to_path_buf(), len, until: Instant::now() + wait, wait }
    }

    /// Whether the log has been quiet for the whole wait.
    pub fn stalled(&mut self) -> bool {
        let len = std::fs::metadata(&self.log).map_or(0, |m| m.len());
        if len != self.len {
            self.len = len;
            self.until = Instant::now() + self.wait;
        }
        Instant::now() >= self.until
    }
}

pub fn background(cmd: &mut Command, group: bool) -> &mut Command {
    #[cfg(unix)]
    if group {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NO_WINDOW | if group { CREATE_NEW_PROCESS_GROUP } else { 0 });
    }
    #[cfg(not(any(unix, windows)))]
    let _ = group;
    cmd
}

/// Why `program` couldn't be started, for people: a program that isn't
/// there said as such ("No such file or directory" alone doesn't say which
/// file).
pub fn spawn_error(program: &str, e: &io::Error) -> String {
    if e.kind() == io::ErrorKind::NotFound {
        if std::path::Path::new(program).components().count() > 1 {
            format!("{program} isn't there")
        } else {
            format!("{program} not found: it isn't a file in the working folder, and isn't on PATH")
        }
    } else {
        format!("can't start {program}: {e}")
    }
}

/// Sends `cmd`'s output (stdout and stderr) to `log`: with `stamped`
/// through a pipe returned for [`stamp_into`], which puts the time before
/// each line; else straight into the file (a server left running when the
/// app goes, which would lose the pipe's reader).
pub fn output_to(cmd: &mut Command, log: &std::fs::File, stamped: bool) -> io::Result<Option<io::PipeReader>> {
    if !stamped {
        cmd.stdout(log.try_clone()?).stderr(log.try_clone()?);
        return Ok(None);
    }
    let (reader, writer) = io::pipe()?;
    cmd.stdout(writer.try_clone()?).stderr(writer);
    Ok(Some(reader))
}

/// Copies a started server's output from `pipe` to `log` on a thread of its
/// own, [the time](crate::logview::stamp) before each line, until the server
/// (and whatever it started) has closed the pipe. Written as it comes, a
/// line's start before its end (progress bars rewrite theirs with `\r`).
pub fn stamp_into(pipe: io::PipeReader, log: std::fs::File) {
    use std::io::{Read, Write};
    let copy = move || {
        let (mut pipe, mut log) = (pipe, log);
        let (mut buf, mut line_start, mut out) = ([0u8; 16 << 10], true, Vec::new());
        loop {
            let n = match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let stamp = crate::logview::stamp(chrono::Local::now());
            out.clear();
            for &b in &buf[..n] {
                if line_start {
                    out.extend_from_slice(stamp.as_bytes());
                }
                out.push(b);
                line_start = b == b'\n';
            }
            // a log that can't be written: the pipe is still read (a full
            // one would stop the server)
            let _ = log.write_all(&out);
        }
    };
    if let Err(e) = std::thread::Builder::new().name("server output".into()).spawn(copy) {
        eprintln!("increart: can't read a server's output: {e}");
    }
}

/// The line a started server's log gets before its output: when, and the
/// command line ([`command_text`]).
pub fn log_header(cmd: &Command) -> String {
    log_note(&command_text(cmd))
}

/// A line of the app's own in a server's log: when, and `text`.
pub fn log_note(text: &str) -> String {
    format!("\n--- IncreArt, {}: {text}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))
}

/// `cmd` as one line: the environment the app sets first, then the program
/// and its arguments (passwords hidden).
pub fn command_text(cmd: &Command) -> String {
    let quote = |s: &str| if s.is_empty() || s.contains(char::is_whitespace) { format!("\"{s}\"") } else { s.into() };
    let mut words: Vec<String> = cmd
        .get_envs()
        .filter_map(|(k, v)| Some(format!("{}={}", k.to_string_lossy(), quote(&v?.to_string_lossy()))))
        .collect();
    words.push(quote(&cmd.get_program().to_string_lossy()));
    let mut secret = false;
    for a in cmd.get_args() {
        let a = a.to_string_lossy();
        words.push(if secret { "***".into() } else { quote(&a) });
        secret = a.ends_with("password");
    }
    words.join(" ")
}

/// Spawns `cmd` so that it stops when the app does, however the app ends.
/// Linux: the kernel sends it SIGTERM when the app goes. Windows: it joins
/// a job the system closes, killing it, when the app goes. macOS: a small
/// watcher sends SIGTERM once the app is gone.
pub fn spawn_with_app(cmd: Command) -> io::Result<Child> {
    imp::spawn_with_app(cmd)
}

/// Stops a server started with [`background`]`(cmd, true)` and everything
/// it started: asked first, killed after 5 s.
pub fn stop(mut child: Child) {
    imp::terminate_tree(&child);
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    imp::kill_tree(&child);
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::process::Stdio;

    /// Asks the kernel to send SIGTERM to what `cmd` runs when the app goes.
    fn with_app(cmd: &mut Command) {
        use std::os::unix::process::CommandExt;
        let app = std::process::id();
        // SAFETY: prctl and getppid are async-signal-safe; nothing else runs before exec.
        unsafe {
            cmd.pre_exec(move || {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                // the app died before the signal was asked for
                if libc::getppid() as u32 != app {
                    return Err(io::Error::other("the app has exited"));
                }
                Ok(())
            });
        }
    }

    /// The signal reaches only the server itself, so a server leading a
    /// process group (KoboldCpp) would leave its helpers running. A shell
    /// that gets the same signal stops the group then: asked, then killed
    /// after 5 s. It ends when the server does.
    fn watch_group(child: &Child) {
        let pid = child.id() as libc::pid_t;
        // SAFETY: getpgid has no memory effects.
        if unsafe { libc::getpgid(pid) } != pid {
            return;
        }
        // no `--` before the group: dash's kill rejects it
        let script = r#"trap 'kill -TERM "-$1"; sleep 5; kill -KILL "-$1"; exit' TERM
trap '' INT
while kill -0 "$1" 2>/dev/null; do sleep 1 & wait $!; done"#;
        let mut watcher = Command::new("/bin/sh");
        watcher
            .args(["-c", script, "increart-watch", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        with_app(&mut watcher);
        if let Ok(mut w) = watcher.spawn() {
            std::thread::spawn(move || w.wait());
        }
    }

    /// The signal is sent when the spawning *thread* ends, and servers are
    /// opened from short-lived threads, so one thread that lasts as long as
    /// the app does the spawning.
    pub fn spawn_with_app(mut cmd: Command) -> io::Result<Child> {
        use std::sync::mpsc;
        type Job = (Command, mpsc::Sender<io::Result<Child>>);
        static SPAWNER: std::sync::OnceLock<mpsc::Sender<Job>> = std::sync::OnceLock::new();
        with_app(&mut cmd);
        let spawner = SPAWNER.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            let spawned = std::thread::Builder::new().name("server-spawner".into()).spawn(move || {
                for (mut cmd, back) in rx {
                    let child = cmd.spawn();
                    if let Ok(c) = &child {
                        watch_group(c);
                    }
                    let _ = back.send(child);
                }
            });
            // (none: every spawn says the thread is gone)
            if let Err(e) = spawned {
                crate::applog::line("error", &format!("can't start the server-spawner thread: {e}"));
            }
            tx
        });
        let (back, child) = mpsc::channel();
        let gone = || io::Error::other("the server-spawner thread is gone");
        spawner.send((cmd, back)).map_err(|_| gone())?;
        child.recv().map_err(|_| gone())?
    }

    pub use super::unix::{kill_tree, terminate_tree};
}

#[cfg(all(unix, not(target_os = "linux")))]
mod imp {
    use super::*;
    use std::process::Stdio;

    /// No kernel help here: a shell polls, every second, whether the app is
    /// still there, and stops the server (its group, if it leads one) once
    /// it isn't. It ends when the server does.
    pub fn spawn_with_app(mut cmd: Command) -> io::Result<Child> {
        let child = cmd.spawn()?;
        let pid = child.id() as libc::pid_t;
        // SAFETY: getpgid has no memory effects.
        let target = if unsafe { libc::getpgid(pid) } == pid { format!("-{pid}") } else { pid.to_string() };
        let script = r#"while kill -0 "$1" 2>/dev/null && kill -0 "$2" 2>/dev/null; do sleep 1; done
kill -0 "$1" 2>/dev/null || kill -TERM "$3" 2>/dev/null"#;
        let watcher = Command::new("/bin/sh")
            .args(["-c", script, "increart-watch"])
            .args([std::process::id().to_string(), pid.to_string(), target])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut w) = watcher {
            std::thread::spawn(move || w.wait());
        }
        Ok(child)
    }

    pub use super::unix::{kill_tree, terminate_tree};
}

#[cfg(unix)]
mod unix {
    use super::*;

    fn signal(child: &Child, sig: libc::c_int) {
        let pid = child.id() as libc::pid_t;
        // SAFETY: kill(2) on the server's own process group.
        unsafe { libc::kill(-pid, sig) };
    }

    pub fn terminate_tree(child: &Child) {
        signal(child, libc::SIGTERM);
    }

    pub fn kill_tree(child: &Child) {
        signal(child, libc::SIGKILL);
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use std::process::Stdio;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    /// The app's job: its handle is closed by the system when the app ends,
    /// which kills every process in it (and what they started since).
    fn job() -> Option<usize> {
        static JOB: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
        *JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls on a job this function owns.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of_val(&info) as u32,
                );
                (ok != 0).then_some(job as usize)
            }
        })
    }

    pub fn spawn_with_app(mut cmd: Command) -> io::Result<Child> {
        let child = cmd.spawn()?;
        if let Some(job) = job() {
            // SAFETY: the job handle lives as long as the app; the child's
            // handle is valid while `child` is.
            unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) };
        }
        Ok(child)
    }

    fn taskkill(child: &Child, force: bool) {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/T", "/PID", &child.id().to_string()]);
        if force {
            cmd.arg("/F");
        }
        let _ = background(&mut cmd, false).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }

    /// Console servers have no window to close, so this asks only those
    /// that do; [`kill_tree`] follows.
    pub fn terminate_tree(child: &Child) {
        taskkill(child, false);
    }

    pub fn kill_tree(child: &Child) {
        taskkill(child, true);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_start_is_given_up_on_only_once_its_log_goes_quiet() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("s.log");
        std::fs::write(&log, "starting\n").unwrap();
        let mut stall = LogStall::new(&log, Duration::from_millis(300));
        let t = Instant::now();
        // a line every 100 ms for 600 ms: twice the wait, still going
        while t.elapsed() < Duration::from_millis(600) {
            std::fs::OpenOptions::new().append(true).open(&log).unwrap().write_all(b"loading\n").unwrap();
            assert!(!stall.stalled());
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(350));
        assert!(stall.stalled(), "quiet for the whole wait");
    }

    use super::*;
    use std::process::Stdio;

    #[test]
    fn a_missing_program_is_named() {
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(
            spawn_error("koboldcpp", &missing),
            "koboldcpp not found: it isn't a file in the working folder, and isn't on PATH"
        );
        let path = if cfg!(windows) { r"C:\kcpp\koboldcpp.exe" } else { "/opt/kcpp/koboldcpp" };
        assert_eq!(spawn_error(path, &missing), format!("{path} isn't there"));
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(spawn_error("koboldcpp", &denied).starts_with("can't start koboldcpp: "));
    }

    #[test]
    fn the_log_header_shows_the_command_without_passwords() {
        let mut c = Command::new("koboldcpp");
        c.env("CUDA_VISIBLE_DEVICES", "1").args(["--port", "5001", "--adminpassword", "s3cret", "--model", "a b.gguf"]);
        let h = log_header(&c);
        assert!(h.starts_with("\n--- IncreArt, ") && h.ends_with('\n'), "{h:?}");
        assert!(
            h.trim_end()
                .ends_with(": CUDA_VISIBLE_DEVICES=1 koboldcpp --port 5001 --adminpassword *** --model \"a b.gguf\""),
            "{h:?}"
        );
        assert!(!h.contains("s3cret"));
    }

    /// A long-running stand-in server: this test binary, running the
    /// ignored test below.
    fn stand_in() -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "process::tests::sleeps", "--ignored", "--nocapture"])
            .env("INCREART_SLEEP", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    #[test]
    #[ignore]
    fn sleeps() {
        if std::env::var_os("INCREART_SLEEP").is_some() {
            std::thread::sleep(Duration::from_secs(120));
        }
    }

    fn alive(pid: u32) -> bool {
        #[cfg(unix)]
        {
            // a zombie has stopped
            let out = Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().unwrap();
            let stat = String::from_utf8_lossy(&out.stdout);
            !stat.trim().is_empty() && !stat.trim().starts_with('Z')
        }
        #[cfg(windows)]
        {
            let mut cmd = Command::new("tasklist");
            cmd.args(["/NH", "/FI", &format!("PID eq {pid}")]);
            let out = background(&mut cmd, false).output().unwrap();
            String::from_utf8_lossy(&out.stdout).contains(&format!(" {pid} "))
        }
    }

    fn wait_gone(pid: u32) -> bool {
        let t = Instant::now();
        while alive(pid) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        !alive(pid)
    }

    /// A stand-in KoboldCpp: starts a helper of its own (the stand-in
    /// server, nothing tying it to the app), writes the helper's pid, then
    /// waits.
    #[test]
    #[ignore]
    #[allow(clippy::zombie_processes)] // stopped from outside
    fn stand_in_launcher() {
        let Ok(out) = std::env::var("INCREART_LAUNCHER") else { return };
        let helper = stand_in().spawn().unwrap();
        std::fs::write(&out, helper.id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }

    /// The stand-in app for the tests below: starts a server from a
    /// short-lived thread, writes its pid, then waits to be killed. With
    /// INCREART_GROUP the server is the stand-in launcher in a group of its
    /// own, and the pid written is its helper's.
    #[test]
    #[ignore]
    #[allow(clippy::zombie_processes)] // killed before it would wait
    fn stand_in_app() {
        let Ok(out) = std::env::var("INCREART_STAND_IN") else { return };
        if std::env::var_os("INCREART_GROUP").is_some() {
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "process::tests::stand_in_launcher", "--ignored", "--nocapture"])
                .env("INCREART_LAUNCHER", &out)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            background(&mut cmd, true);
            let _launcher = std::thread::spawn(|| spawn_with_app(cmd).unwrap()).join().unwrap();
            std::thread::sleep(Duration::from_secs(60));
            return;
        }
        let child = std::thread::spawn(|| spawn_with_app(stand_in()).unwrap()).join().unwrap();
        std::fs::write(&out, child.id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn a_started_server_stops_when_the_app_is_killed() {
        kill_the_app_then_check(false);
    }

    #[test]
    fn a_server_s_helpers_stop_when_the_app_is_killed() {
        kill_the_app_then_check(true);
    }

    /// Kills the stand-in app and checks that the process it reported
    /// (its server, or with `group` that server's helper) stops too.
    fn kill_the_app_then_check(group: bool) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pid");
        let mut app = Command::new(std::env::current_exe().unwrap());
        app.args(["--exact", "process::tests::stand_in_app", "--ignored", "--nocapture"])
            .env("INCREART_STAND_IN", &out)
            .stdout(Stdio::null());
        if group {
            app.env("INCREART_GROUP", "1");
        }
        let mut app = app.spawn().unwrap();
        let t = Instant::now();
        while std::fs::read_to_string(&out).map_or(true, |s| s.is_empty()) && t.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let server: u32 = std::fs::read_to_string(&out).unwrap().parse().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(alive(server), "the thread that opened it ending doesn't stop it");
        app.kill().unwrap();
        app.wait().unwrap();
        let stopped = wait_gone(server);
        if !stopped {
            let pid = server.to_string();
            let _ = if cfg!(windows) {
                Command::new("taskkill").args(["/F", "/PID", &pid]).status()
            } else {
                Command::new("kill").args(["-KILL", &pid]).status()
            };
        }
        assert!(stopped, "stopped with the app");
    }

    #[test]
    fn stopping_a_server_stops_what_it_started() {
        // a "launcher" that runs the stand-in server as its own child
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pid");
        let mut launcher = Command::new(std::env::current_exe().unwrap());
        launcher
            .args(["--exact", "process::tests::stand_in_app", "--ignored", "--nocapture"])
            .env("INCREART_STAND_IN", &out)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = background(&mut launcher, true).spawn().unwrap();
        let t = Instant::now();
        while std::fs::read_to_string(&out).map_or(true, |s| s.is_empty()) && t.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let server: u32 = std::fs::read_to_string(&out).unwrap().parse().unwrap();
        let launched = child.id();
        stop(child);
        assert!(wait_gone(launched) && wait_gone(server), "the launcher and its server stopped");
    }
}
