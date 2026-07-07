//! Resolution of the protected daemon working directories.
//!
//! Removing a directory that a daemon `chdir`'d into crash-loops it with
//! `status=200/CHDIR`. [`resolve_daemon_working_dirs`] computes the union of
//! every directory that must therefore never be reclaimed. `proc_root` is
//! injectable so tests can point it at a fabricated `/proc` tree — no real
//! process inspection, fully hermetic.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The `comm` value of the OODA daemon process.
const DAEMON_COMM: &str = "simard-ooda";

/// Environment override for the systemd unit path (test seam). When unset the
/// well-known system location is consulted best-effort.
const SERVICE_FILE_ENV: &str = "SIMARD_OODA_SERVICE_FILE";

/// Default systemd unit path for the daemon.
const DEFAULT_SERVICE_FILE: &str = "/etc/systemd/system/simard-ooda.service";

/// Returns the set of directories that must never be removed because a daemon
/// runs there. The union of:
///
/// - the **hardcoded** [`crate::disk_reclaim::HARDCODED_PROTECTED_MAIN`]
///   (always present — guarantees the set is never empty),
/// - the **own process** cwd (`<proc_root>/self/cwd`),
/// - a **`/proc` comm scan** for `simard-ooda` processes, resolving each
///   `<proc_root>/<pid>/cwd`,
/// - the service file's `WorkingDirectory=` (best-effort).
///
/// Unreadable entries are skipped; the hardcoded `main` keeps the set non-empty.
pub fn resolve_daemon_working_dirs(proc_root: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();

    // Always protect the canonical daemon working directory, even if the live
    // service has been relocated — defense in depth against the crash-loop.
    out.insert(PathBuf::from(super::HARDCODED_PROTECTED_MAIN));

    // Our own process cwd.
    if let Ok(target) = std::fs::read_link(proc_root.join("self").join("cwd")) {
        out.insert(target);
    }

    // Scan /proc for any live simard-ooda process and protect its cwd.
    if let Ok(entries) = std::fs::read_dir(proc_root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) else {
                continue;
            };
            if comm.trim() == DAEMON_COMM
                && let Ok(target) = std::fs::read_link(entry.path().join("cwd"))
            {
                out.insert(target);
            }
        }
    }

    // The service unit's declared WorkingDirectory (best-effort).
    if let Some(dir) = service_working_directory() {
        out.insert(dir);
    }

    out
}

/// Parse `WorkingDirectory=` from the systemd unit file, honouring the
/// [`SERVICE_FILE_ENV`] override. Returns `None` when the file is absent or has
/// no `WorkingDirectory=` line.
fn service_working_directory() -> Option<PathBuf> {
    let path = std::env::var(SERVICE_FILE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SERVICE_FILE));
    let contents = std::fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("WorkingDirectory=") {
            let val = val.trim();
            if !val.is_empty() {
                return Some(PathBuf::from(val));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::tempdir;

    #[test]
    fn hardcoded_main_is_always_present_even_with_empty_proc_root() {
        let proc = tempdir().expect("proc");
        let set = resolve_daemon_working_dirs(proc.path());
        assert!(
            set.contains(&PathBuf::from(super::super::HARDCODED_PROTECTED_MAIN)),
            "hardcoded worktrees/main must always be protected",
        );
        assert!(!set.is_empty(), "the deny-set must never be empty");
    }

    #[test]
    fn comm_scan_protects_simard_ooda_cwd() {
        let proc = tempdir().expect("proc");
        let daemon_wd = tempdir().expect("daemon wd");

        // Fabricate /proc/4242 for a simard-ooda process whose cwd is daemon_wd.
        let pid_dir = proc.path().join("4242");
        std::fs::create_dir_all(&pid_dir).expect("mkdir pid");
        std::fs::write(pid_dir.join("comm"), "simard-ooda\n").expect("write comm");
        std::os::unix::fs::symlink(daemon_wd.path(), pid_dir.join("cwd")).expect("symlink cwd");

        let set = resolve_daemon_working_dirs(proc.path());
        let canon = daemon_wd.path().canonicalize().unwrap();
        assert!(
            set.iter().any(|p| p == daemon_wd.path() || *p == canon),
            "the live daemon's cwd must be in the deny-set: {set:?}",
        );
    }

    #[test]
    fn comm_scan_ignores_non_daemon_processes() {
        let proc = tempdir().expect("proc");
        let other_wd = tempdir().expect("other wd");

        let pid_dir = proc.path().join("777");
        std::fs::create_dir_all(&pid_dir).expect("mkdir pid");
        std::fs::write(pid_dir.join("comm"), "bash\n").expect("write comm");
        std::os::unix::fs::symlink(other_wd.path(), pid_dir.join("cwd")).expect("symlink cwd");

        let set = resolve_daemon_working_dirs(proc.path());
        let canon = other_wd.path().canonicalize().unwrap();
        assert!(
            !set.iter().any(|p| p == other_wd.path() || *p == canon),
            "an unrelated process cwd must NOT be protected: {set:?}",
        );
    }

    #[test]
    fn self_cwd_is_protected() {
        let proc = tempdir().expect("proc");
        let self_wd = tempdir().expect("self wd");
        let self_dir = proc.path().join("self");
        std::fs::create_dir_all(&self_dir).expect("mkdir self");
        std::os::unix::fs::symlink(self_wd.path(), self_dir.join("cwd")).expect("symlink");

        let set = resolve_daemon_working_dirs(proc.path());
        let canon = self_wd.path().canonicalize().unwrap();
        assert!(set.iter().any(|p| p == self_wd.path() || *p == canon));
    }

    #[test]
    #[serial(cognitive_memory)]
    fn service_file_working_directory_is_protected() {
        let proc = tempdir().expect("proc");
        let service = tempfile::NamedTempFile::new().expect("service file");
        std::fs::write(
            service.path(),
            "[Service]\nWorkingDirectory=/srv/relocated-daemon\nExecStart=/x\n",
        )
        .unwrap();
        // SAFETY: guarded by #[serial]; restored immediately after.
        unsafe { std::env::set_var(SERVICE_FILE_ENV, service.path()) };
        let set = resolve_daemon_working_dirs(proc.path());
        unsafe { std::env::remove_var(SERVICE_FILE_ENV) };

        assert!(
            set.contains(&PathBuf::from("/srv/relocated-daemon")),
            "WorkingDirectory= from the unit file must be protected: {set:?}",
        );
    }
}
