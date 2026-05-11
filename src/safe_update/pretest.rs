//! Phase 3: pre-swap self-test.
//!
//! Spawns `<new_bin> self-test` with a wall-clock budget. Captures combined
//! stdout+stderr to `state_dir/last-pretest.log` so the operator can inspect
//! a failed pre-test without re-running anything.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::errors::SafeUpdateError;

/// Pre-test result. `passed=false` is *not* an `Err` so the orchestrator
/// can capture the outcome and persist phase=`pretest_failed` before
/// returning the error to its caller.
#[derive(Debug, Clone)]
pub struct PretestOutcome {
    pub passed: bool,
    pub exit_code: Option<i32>,
    /// Tail of the captured output, used for the operator-facing error.
    pub detail: String,
    pub log_path: PathBuf,
    pub elapsed: Duration,
}

/// Run `<binary> self-test` with a wall-clock budget. The combined
/// stdout+stderr stream is written to `state_dir/last-pretest.log`.
pub fn run_pretest(
    binary: &Path,
    state_dir: &Path,
    pretest_timeout_seconds: u64,
) -> Result<PretestOutcome, SafeUpdateError> {
    fs::create_dir_all(state_dir).map_err(|e| SafeUpdateError::PretestSpawn {
        path: state_dir.to_path_buf(),
        reason: format!("mkdir {}: {e}", state_dir.display()),
    })?;
    let log_path = state_dir.join("last-pretest.log");

    let started = Instant::now();
    let mut child = Command::new(binary)
        .arg("self-test")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SafeUpdateError::PretestSpawn {
            path: binary.to_path_buf(),
            reason: e.to_string(),
        })?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let tx2 = tx.clone();

    if let Some(mut s) = stdout {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    }
    if let Some(mut s) = stderr {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            let _ = tx2.send(buf);
        });
    }

    let deadline = started + Duration::from_secs(pretest_timeout_seconds);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut combined = Vec::<u8>::new();
                while let Ok(buf) = rx.try_recv() {
                    combined.extend(buf);
                }
                let _ = fs::write(&log_path, &combined);
                let detail = tail_string(&combined, 800);
                return Ok(PretestOutcome {
                    passed: status.success(),
                    exit_code: status.code(),
                    detail,
                    log_path,
                    elapsed: started.elapsed(),
                });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let mut combined = Vec::<u8>::new();
                    while let Ok(buf) = rx.try_recv() {
                        combined.extend(buf);
                    }
                    let _ = fs::write(&log_path, &combined);
                    return Err(SafeUpdateError::PretestTimeout {
                        seconds: pretest_timeout_seconds,
                    });
                }
                thread::sleep(poll_interval_for(pretest_timeout_seconds));
            }
            Err(e) => {
                return Err(SafeUpdateError::PretestSpawn {
                    path: binary.to_path_buf(),
                    reason: format!("wait failed: {e}"),
                });
            }
        }
    }
}

fn poll_interval_for(timeout_seconds: u64) -> Duration {
    if timeout_seconds == 0 {
        Duration::from_millis(20)
    } else if timeout_seconds <= 2 {
        Duration::from_millis(50)
    } else {
        Duration::from_millis(200)
    }
}

fn tail_string(bytes: &[u8], max_bytes: usize) -> String {
    if bytes.len() <= max_bytes {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        let start = bytes.len() - max_bytes;
        let tail = &bytes[start..];
        format!("…{}", String::from_utf8_lossy(tail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn pretest_passes_for_true() {
        let dir = tempdir().unwrap();
        let bin = which("true").expect("/usr/bin/true must exist on this host");
        // Note: `true` does not understand `self-test`, but it always
        // exits 0 regardless of args, so this exercises the success path.
        let out = run_pretest(&bin, dir.path(), 5).unwrap();
        assert!(out.passed);
        assert_eq!(out.exit_code, Some(0));
        assert!(out.log_path.exists());
    }

    #[test]
    fn pretest_fails_for_false_exit_code() {
        let dir = tempdir().unwrap();
        let bin = which("false").expect("/usr/bin/false must exist on this host");
        let out = run_pretest(&bin, dir.path(), 5).unwrap();
        assert!(!out.passed);
        assert_eq!(out.exit_code, Some(1));
        assert!(out.log_path.exists());
    }

    #[test]
    fn pretest_classifies_spawn_error_for_missing_binary() {
        let dir = tempdir().unwrap();
        let bin = PathBuf::from("/no-such-binary-pretest-test");
        let err = run_pretest(&bin, dir.path(), 1).unwrap_err();
        assert!(matches!(err, SafeUpdateError::PretestSpawn { .. }));
    }

    #[test]
    fn pretest_times_out_when_command_runs_too_long() {
        let dir = tempdir().unwrap();
        let bin = which("sleep").expect("/usr/bin/sleep must exist");
        // A 1-second budget against a 30-second sleep should always time out.
        let started = Instant::now();
        let res = run_pretest_with_args(&bin, dir.path(), 1, &["30"]);
        let elapsed = started.elapsed();
        match res {
            Err(SafeUpdateError::PretestTimeout { seconds }) => assert_eq!(seconds, 1),
            other => panic!("expected PretestTimeout, got {other:?}"),
        }
        assert!(elapsed < Duration::from_secs(5), "elapsed {elapsed:?}");
    }

    /// Test-only variant that lets us pick the argv (so we can use
    /// `sleep 30` to provoke a timeout instead of `sleep self-test`).
    fn run_pretest_with_args(
        binary: &Path,
        state_dir: &Path,
        pretest_timeout_seconds: u64,
        args: &[&str],
    ) -> Result<PretestOutcome, SafeUpdateError> {
        fs::create_dir_all(state_dir).map_err(|e| SafeUpdateError::PretestSpawn {
            path: state_dir.to_path_buf(),
            reason: format!("mkdir {}: {e}", state_dir.display()),
        })?;
        let log_path = state_dir.join("last-pretest.log");
        let started = Instant::now();
        let mut child = Command::new(binary)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SafeUpdateError::PretestSpawn {
                path: binary.to_path_buf(),
                reason: e.to_string(),
            })?;
        let deadline = started + Duration::from_secs(pretest_timeout_seconds);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Ok(PretestOutcome {
                        passed: status.success(),
                        exit_code: status.code(),
                        detail: String::new(),
                        log_path,
                        elapsed: started.elapsed(),
                    });
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        return Err(SafeUpdateError::PretestTimeout {
                            seconds: pretest_timeout_seconds,
                        });
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(SafeUpdateError::PretestSpawn {
                        path: binary.to_path_buf(),
                        reason: format!("wait failed: {e}"),
                    });
                }
            }
        }
    }

    fn which(name: &str) -> Option<PathBuf> {
        for d in ["/usr/bin", "/bin", "/usr/local/bin"] {
            let p = PathBuf::from(d).join(name);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }
}
