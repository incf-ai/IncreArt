//! Ephemeral sessions (DESIGN.md, Ephemeral mode): what isn't the library
//! itself ([`Library::open_ephemeral`](crate::Library::open_ephemeral)):
//! the logs kept in RAM, and swap.

use std::path::Path;

/// From here on the app's logs (its own, the servers' it starts, the
/// traffic log) are kept in RAM: an overlay over the log folder, the logs
/// on disk read through it. `pin`: kept out of swap.
pub fn logs_to_ram(dir: &Path, pin: bool) -> std::io::Result<()> {
    if crate::vfs::in_ram(dir) {
        return Ok(());
    }
    crate::vfs::mount(dir, Some(dir), pin)?;
    crate::applog::reopen();
    crate::traffic::reopen();
    Ok(())
}

/// Back to the logs on disk: what was logged in RAM is dropped.
pub fn logs_to_disk(dir: &Path) {
    if crate::vfs::in_ram(dir) {
        crate::vfs::unmount(dir);
        crate::applog::reopen();
        crate::traffic::reopen();
    }
}

/// Whether what's in RAM can reach the disk through swap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swap {
    /// The machine has no swap.
    None,
    /// The OS lets the app keep `budget` out of swap.
    Pinned,
    /// It lets it keep only this many bytes out of swap.
    Limited(u64),
    /// Not known.
    Unknown,
}

/// Asks the OS to let the app keep `budget` bytes out of swap (raising
/// the limit as far as allowed), and says how that went.
pub fn prepare_pin(budget: u64) -> Swap {
    if swap_on() == Some(false) {
        return Swap::None;
    }
    pin_limit(budget)
}

/// The machine has swap (None: not known).
pub fn swap_on() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        let text = crate::vfs::read_to_string("/proc/swaps").ok()?;
        Some(text.lines().skip(1).any(|l| !l.trim().is_empty()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        // macOS swaps to files as it needs; Windows has its page file
        Some(true)
    }
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // rlim_t isn't u64 everywhere
fn pin_limit(budget: u64) -> Swap {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    if unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut lim) } != 0 {
        return Swap::Unknown;
    }
    if lim.rlim_cur != lim.rlim_max {
        let raised = libc::rlimit { rlim_cur: lim.rlim_max, rlim_max: lim.rlim_max };
        if unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &raised) } == 0 {
            lim = raised;
        }
    }
    if lim.rlim_cur == libc::RLIM_INFINITY || lim.rlim_cur as u64 >= budget {
        Swap::Pinned
    } else {
        Swap::Limited(lim.rlim_cur as u64)
    }
}

#[cfg(windows)]
fn pin_limit(budget: u64) -> Swap {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    // VirtualLock keeps pages in the working set: it must be big enough
    let min = budget as usize + (256 << 20);
    let max = min + (1 << 30);
    if unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), min, max) } != 0 {
        Swap::Pinned
    } else {
        Swap::Limited(0)
    }
}

#[cfg(not(any(unix, windows)))]
fn pin_limit(_: u64) -> Swap {
    Swap::Unknown
}

#[cfg(test)]
mod tests {
    #[test]
    fn logs_go_to_ram_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        crate::vfs::create_dir_all(&logs).unwrap();
        crate::vfs::write(logs.join("old.log"), b"before\n").unwrap();
        super::logs_to_ram(&logs, false).unwrap();
        assert_eq!(crate::vfs::read(logs.join("old.log")).unwrap(), b"before\n", "the old logs read through");
        crate::vfs::write(logs.join("sd-server-a.log"), b"a private prompt\n").unwrap();
        #[allow(clippy::disallowed_methods)]
        let on_disk = std::fs::exists(logs.join("sd-server-a.log")).unwrap();
        assert!(!on_disk);
        super::logs_to_disk(&logs);
        assert!(!crate::vfs::exists(logs.join("sd-server-a.log")), "dropped");
        assert!(crate::vfs::exists(logs.join("old.log")));
    }
}
