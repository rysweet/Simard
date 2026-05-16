//! Proof-of-life probe for the worktree GC (issue #1886).
//!
//! Pruning a worktree whose path is the CWD of a live engineer
//! subprocess silently destroys that engineer's working environment.
//! The Copilot agent then loops on `ENOENT` / `Failed to start bash
//! process` errors until the wrapper's 1-hour timeout fires, wasting an
//! entire LLM session per affected worktree.
//!
//! This module isolates the "is there a live process in this directory?"
//! check behind a trait so the policy stays pure and tests stay
//! hermetic. Production uses [`ProcfsLiveProcessProbe`] (reads
//! `/proc/<pid>/cwd`); tests use [`FakeLiveProcessProbe`].
//!
//! Decision (per #1886): the probe FAIL-CLOSES — if it cannot answer
//! authoritatively (non-Linux host, /proc unreadable, canonicalize
//! failure), it reports "live" so GC declines to prune. That is the
//! correct safety bias: a false positive leaves an idle worktree on
//! disk one more cycle; a false negative destroys an active session.
//!
//! ## Scope
//!
//! Detects ANY process whose CWD lives under the worktree path. Does
//! not try to distinguish engineer subprocesses from random shells
//! someone left open — both are signals that pruning would surprise
//! the operator.
//!
//! ## Cost
//!
//! O(num_processes) `readlink` syscalls per worktree examined.
//! Concretely, ~500 processes on this host means a few ms per
//! worktree; the gh/git network calls already in `gather_inputs`
//! dominate by 2-3 orders of magnitude.

use std::path::{Path, PathBuf};

/// Decision interface: "is this worktree path the CWD of any live
/// process?" Implementations must be safe to call from
/// `gather_inputs`, which runs once per worktree.
pub trait LiveProcessProbe {
    /// Return `true` if at least one process on the host has its
    /// current working directory inside `dir` (after symlink
    /// resolution). Return `true` if the answer cannot be determined
    /// (fail-closed).
    fn worktree_has_live_process(&self, dir: &Path) -> bool;
}

/// Production implementation: scans `/proc/<pid>/cwd` symlinks on
/// Linux. Returns `true` (fail-closed) on any error so GC will not
/// prune a worktree we cannot verify is idle.
pub struct ProcfsLiveProcessProbe {
    proc_root: PathBuf,
}

impl ProcfsLiveProcessProbe {
    /// Default constructor: probes the system's `/proc`.
    pub fn new() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
        }
    }

    /// Test constructor: probes an arbitrary directory laid out like
    /// `/proc` (one subdir per fake pid, each containing a `cwd`
    /// symlink).
    #[cfg(test)]
    pub fn with_root(proc_root: PathBuf) -> Self {
        Self { proc_root }
    }
}

impl Default for ProcfsLiveProcessProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveProcessProbe for ProcfsLiveProcessProbe {
    fn worktree_has_live_process(&self, dir: &Path) -> bool {
        // Canonicalize the target once. If that fails the worktree
        // either does not exist (nothing to protect) or is unreadable
        // (fail-closed: refuse to prune).
        let canon = match dir.canonicalize() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    target: "simard::worktree_gc",
                    error = %e,
                    dir = %dir.display(),
                    "cannot canonicalize worktree for liveness check; treating as live",
                );
                return true;
            }
        };

        let entries = match std::fs::read_dir(&self.proc_root) {
            Ok(rd) => rd,
            Err(e) => {
                tracing::warn!(
                    target: "simard::worktree_gc",
                    error = %e,
                    proc_root = %self.proc_root.display(),
                    "cannot read /proc for liveness check; treating as live",
                );
                return true;
            }
        };

        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Only consider numeric PID dirs. Skip /proc/self, /proc/sys, etc.
            if !name_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let cwd_link = entry.path().join("cwd");
            // read_link follows the symlink to the target path; errors
            // are expected (process exited mid-scan, EPERM on other
            // users' processes) and are not fatal — skip silently.
            let Ok(target) = std::fs::read_link(&cwd_link) else {
                continue;
            };
            // The target is already canonical-ish (kernel resolves it
            // before exposing the symlink). starts_with handles the
            // sub-directory case (e.g. the engineer's child shell `cd`d
            // into a subdir).
            if target.starts_with(&canon) {
                return true;
            }
        }
        false
    }
}

/// Test double: a fixed map from worktree path → liveness.
/// Returns `false` (no live process) for unknown paths.
#[cfg(test)]
#[derive(Default)]
pub struct FakeLiveProcessProbe {
    pub live: std::sync::Mutex<std::collections::HashMap<PathBuf, bool>>,
}

#[cfg(test)]
impl FakeLiveProcessProbe {
    pub fn mark_live(&self, dir: impl Into<PathBuf>) {
        self.live.lock().unwrap().insert(dir.into(), true);
    }
}

#[cfg(test)]
impl LiveProcessProbe for FakeLiveProcessProbe {
    fn worktree_has_live_process(&self, dir: &Path) -> bool {
        let canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        let map = self.live.lock().unwrap();
        // Match by canonical AND by raw — tests may pass either form.
        map.get(&canon).copied().unwrap_or(false) || map.get(dir).copied().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn procfs_probe_detects_self_cwd() {
        // Setup: this test process has /proc/<our_pid>/cwd pointing
        // somewhere. Use a worktree path that contains our cwd, then
        // confirm the probe returns true.
        let probe = ProcfsLiveProcessProbe::new();
        let our_cwd = std::env::current_dir().expect("getcwd");
        assert!(
            probe.worktree_has_live_process(&our_cwd),
            "probe should detect this test process as live in its own cwd",
        );
    }

    #[test]
    fn procfs_probe_says_no_for_empty_directory() {
        let dir = tempdir().expect("tempdir");
        let probe = ProcfsLiveProcessProbe::new();
        // No process has its cwd inside this fresh tempdir.
        assert!(
            !probe.worktree_has_live_process(dir.path()),
            "fresh tempdir should have no live process",
        );
    }

    #[test]
    fn procfs_probe_fails_closed_on_unreadable_proc_root() {
        let dir = tempdir().expect("tempdir");
        let probe = ProcfsLiveProcessProbe::with_root(PathBuf::from(
            "/does/not/exist/proc-substitute-1886",
        ));
        // Unreadable /proc → treat as live (fail-closed).
        assert!(
            probe.worktree_has_live_process(dir.path()),
            "unreadable /proc must fail closed",
        );
    }

    #[test]
    fn procfs_probe_fails_closed_on_nonexistent_dir() {
        let probe = ProcfsLiveProcessProbe::new();
        let bogus = PathBuf::from("/does/not/exist/worktree-1886-aaaa");
        assert!(
            probe.worktree_has_live_process(&bogus),
            "uncanonicalizable target must fail closed",
        );
    }

    #[test]
    fn procfs_probe_with_fake_proc_root_finds_match() {
        let worktree = tempdir().expect("worktree");
        let proc_root = tempdir().expect("proc");
        let pid_dir = proc_root.path().join("12345");
        std::fs::create_dir_all(&pid_dir).expect("mkdir pid");
        // Symlink /proc/12345/cwd -> worktree dir
        std::os::unix::fs::symlink(worktree.path(), pid_dir.join("cwd")).expect("symlink cwd");
        // A non-numeric sibling that the probe must skip.
        std::fs::create_dir_all(proc_root.path().join("self")).expect("mkdir self");

        let probe = ProcfsLiveProcessProbe::with_root(proc_root.path().to_path_buf());
        assert!(
            probe.worktree_has_live_process(worktree.path()),
            "probe should find the fake pid's cwd inside the worktree",
        );
    }

    #[test]
    fn procfs_probe_with_fake_proc_root_skips_unrelated() {
        let worktree = tempdir().expect("worktree");
        let other = tempdir().expect("other");
        let proc_root = tempdir().expect("proc");
        let pid_dir = proc_root.path().join("99999");
        std::fs::create_dir_all(&pid_dir).expect("mkdir pid");
        std::os::unix::fs::symlink(other.path(), pid_dir.join("cwd")).expect("symlink");

        let probe = ProcfsLiveProcessProbe::with_root(proc_root.path().to_path_buf());
        assert!(
            !probe.worktree_has_live_process(worktree.path()),
            "probe should not match unrelated cwds",
        );
    }

    #[test]
    fn fake_probe_returns_marked_paths() {
        let probe = FakeLiveProcessProbe::default();
        let dir = tempdir().expect("tempdir");
        assert!(!probe.worktree_has_live_process(dir.path()));
        probe.mark_live(dir.path());
        assert!(probe.worktree_has_live_process(dir.path()));
    }
}
