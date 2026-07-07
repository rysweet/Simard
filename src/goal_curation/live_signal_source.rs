//! Production live-signal source for closed-loop outcome verification (#2751).
//!
//! Composes thin adapters over signals the daemon **already** emits from
//! trusted origins — the deploy-reconcile detector and the daemon's own
//! structured log stream (`<state_root>/ooda.log`) — into the [`LiveSignal`]s
//! the outcome-verify brain reasons over. No new network calls, no shellouts:
//! every observation comes from an authenticated local source.
//!
//! # What sets `verified`
//!
//! The load-bearing [`LiveSignal::verified`] flag is set ONLY from a positive,
//! authenticated corroboration:
//!
//! - `deployed_running` — the reconcile detector reports the merged change is
//!   actually running (`!DeployDrift::needs_deploy`). This is the verified
//!   signal that lets a healthy self-affecting goal clear Rail-3.
//! - `external_repo` — for goals that route to another repo, the daemon has no
//!   live probe of that repo's production; it emits one verified marker so
//!   external goals continue to defer to the artifact done-gate (no regression)
//!   while making the "no live probe available" fact explicit to the reasoner.
//!
//! Failure signatures found in the recent daemon log (E2BIG on spawn, panics,
//! OOM kills — the kgpacks regression class) are emitted as **UNVERIFIED**
//! context (`verified = false`). They never satisfy Rail-3 on their own; the
//! brain weighs them to decide whether a landed artifact's live effect is
//! actually absent (the exact "artifact != outcome" call this step exists to
//! make). Because `verified` is never set from these log-derived strings, an
//! attacker who can write to the log cannot forge a completion.

use std::path::PathBuf;

use chrono::Utc;

use crate::error::SimardResult;

use super::completion_gate::is_self_affecting;
use super::live_signal::{LiveSignal, LiveSignalSource};
use super::types::ActiveGoal;

/// Bytes of the daemon log tail scanned for failure signatures. Bounded so the
/// read is cheap and cannot balloon prompt cost.
const LOG_TAIL_BYTES: u64 = 64 * 1024;

/// Known failure-signature substrings (case-insensitive) whose presence in the
/// recent daemon log means a landed change's live effect may be ABSENT. The
/// E2BIG family is the kgpacks regression this step exists to catch.
const FAILURE_SIGNATURES: &[(&str, &str)] = &[
    ("e2big", "E2BIG / argument list too long on spawn"),
    (
        "argument list too long",
        "E2BIG / argument list too long on spawn",
    ),
    ("panicked at", "engineer/daemon panic"),
    ("out of memory", "out-of-memory"),
    ("oom-kill", "OOM killer fired"),
];

/// Production [`LiveSignalSource`]. Reads the deploy-reconcile state and the
/// daemon's own log tail; both are local, authenticated origins.
pub struct DaemonLiveSignals {
    /// Repo the daemon deploys from — used by the reconcile detector.
    repo_root: PathBuf,
    /// Daemon state root; `ooda.log` (the daemon's structured log) lives here.
    state_root: PathBuf,
}

impl DaemonLiveSignals {
    pub fn new(repo_root: PathBuf, state_root: PathBuf) -> Self {
        Self {
            repo_root,
            state_root,
        }
    }

    /// `true` when the running binary is not behind merged `main` (the change
    /// is actually deployed and running). Fail-safe: a transient git error
    /// reports "no drift", so this never spuriously blocks — it only reports
    /// `false` when drift is positively observed. Mirrors the completion gate's
    /// `is_deployed`.
    fn deployed_running(&self) -> bool {
        let detector = crate::self_deploy::ReconcileDetector::new(
            crate::self_deploy::GitDeploySource::at(&self.repo_root),
        );
        !detector.detect().needs_deploy
    }

    /// Read the last [`LOG_TAIL_BYTES`] of the daemon log, best-effort. Returns
    /// `None` when the log is absent or unreadable (no failure context this
    /// cycle — never a hard error, so a fresh daemon does not stall).
    fn read_log_tail(&self) -> Option<String> {
        read_tail(&self.state_root.join("ooda.log"), LOG_TAIL_BYTES)
    }
}

impl LiveSignalSource for DaemonLiveSignals {
    fn gather(&self, goal: &ActiveGoal) -> SimardResult<Vec<LiveSignal>> {
        let now = Utc::now();
        let mut signals = Vec::new();

        if is_self_affecting(goal) {
            let running = self.deployed_running();
            signals.push(LiveSignal {
                source: "reconcile_detector".to_string(),
                kind: if running {
                    "deployed_running".to_string()
                } else {
                    "not_running".to_string()
                },
                // Authenticated positive corroboration only.
                verified: running,
                detail: if running {
                    "merged change is running (no deploy drift)".to_string()
                } else {
                    "running binary is behind merged main — not yet deployed".to_string()
                },
                observed_at: now,
            });

            // Failure-signature context from the daemon's own log — UNVERIFIED.
            if let Some(tail) = self.read_log_tail() {
                for (kind, detail) in classify_failure_signatures(&tail) {
                    signals.push(LiveSignal {
                        source: "daemon_log".to_string(),
                        kind,
                        verified: false,
                        detail,
                        observed_at: now,
                    });
                }
            }
        } else {
            // External-repo goal: no live probe of another repo's production.
            // One verified marker so these goals keep deferring to the artifact
            // gate, with the "no live probe" fact explicit for the reasoner.
            signals.push(LiveSignal {
                source: "daemon".to_string(),
                kind: "external_repo".to_string(),
                verified: true,
                detail: format!(
                    "goal routes to external repo {}; no live production probe available — defer to artifact gate",
                    goal.repo.as_deref().unwrap_or("<unknown>")
                ),
                observed_at: now,
            });
        }

        Ok(signals)
    }
}

/// Read the last `max_bytes` of a file as UTF-8 (lossy), best-effort. Seeks so a
/// large log is not read whole. Returns `None` on any IO error.
fn read_tail(path: &std::path::Path, max_bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    // The read size is known and bounded by `max_bytes`; pre-size the buffer so
    // `read_to_end` fills it in one shot instead of repeatedly reallocating as it
    // grows toward the 64 KiB tail on every cycle.
    let mut buf = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Classify a log tail into the failure-signature kinds present. Pure and
/// case-insensitive; each signature is reported at most once with a stable
/// `<snake>_present` kind. This is the only interesting logic in the source and
/// is unit-tested below.
fn classify_failure_signatures(tail: &str) -> Vec<(String, String)> {
    let lower = tail.to_ascii_lowercase();
    let mut out: Vec<(String, String)> = Vec::new();
    for (needle, detail) in FAILURE_SIGNATURES {
        if lower.contains(needle) {
            let kind = format!("{}_present", detail_kind(needle));
            if !out.iter().any(|(k, _)| k == &kind) {
                out.push((kind, (*detail).to_string()));
            }
        }
    }
    out
}

/// Stable, injection-free kind token for a signature needle.
fn detail_kind(needle: &str) -> String {
    match needle {
        "e2big" | "argument list too long" => "e2big".to_string(),
        "panicked at" => "panic".to_string(),
        "out of memory" | "oom-kill" => "oom".to_string(),
        other => other.replace(|c: char| !c.is_ascii_alphanumeric(), "_"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal_curation::GoalProgress;

    #[test]
    fn classifies_e2big_signature() {
        let tail = "2026-07-07 spawn failed: execve: Argument list too long (E2BIG)";
        let sigs = classify_failure_signatures(tail);
        assert!(sigs.iter().any(|(k, _)| k == "e2big_present"));
        // De-duped even though two needles ("e2big" and "argument list too
        // long") both match.
        assert_eq!(sigs.iter().filter(|(k, _)| k == "e2big_present").count(), 1);
    }

    #[test]
    fn classifies_panic_and_oom() {
        let tail = "thread 'main' panicked at 'x'\nkernel: oom-kill process";
        let sigs = classify_failure_signatures(tail);
        assert!(sigs.iter().any(|(k, _)| k == "panic_present"));
        assert!(sigs.iter().any(|(k, _)| k == "oom_present"));
    }

    #[test]
    fn clean_log_yields_no_signatures() {
        let tail = "2026-07-07 OODA cycle 42 complete; archived 1 goal";
        assert!(classify_failure_signatures(tail).is_empty());
    }

    #[test]
    fn external_repo_goal_gets_one_verified_marker() {
        let mut g = ActiveGoal::new("g", "improve amplihack docs", 1);
        g.repo = Some("amplihack-rs".to_string());
        g.status = GoalProgress::Completed;
        let src = DaemonLiveSignals::new(PathBuf::from("."), PathBuf::from("."));
        let signals = src.gather(&g).unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].kind, "external_repo");
        assert!(signals[0].verified);
    }

    #[test]
    fn read_tail_missing_file_is_none() {
        assert!(read_tail(std::path::Path::new("/no/such/ooda.log"), 1024).is_none());
    }

    #[test]
    fn read_tail_returns_whole_file_when_smaller_than_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ooda.log");
        std::fs::write(&path, b"cycle 1\ncycle 2\n").unwrap();
        assert_eq!(read_tail(&path, 64 * 1024).unwrap(), "cycle 1\ncycle 2\n");
    }

    #[test]
    fn read_tail_returns_only_last_max_bytes() {
        // A body larger than the cap must return exactly the trailing `max_bytes`
        // — the pre-sized read buffer must not change the tail semantics.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ooda.log");
        let body: String = (0..2000).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        std::fs::write(&path, body.as_bytes()).unwrap();
        let tail = read_tail(&path, 100).unwrap();
        assert_eq!(tail.len(), 100);
        assert_eq!(tail, &body[body.len() - 100..]);
    }
}
