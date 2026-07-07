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
//!   actually running (`!DeployDrift::needs_deploy`, via the fail-*closed*
//!   [`ReconcileDetector::try_detect`](crate::self_deploy::ReconcileDetector::try_detect)).
//!   This is the verified signal that lets a healthy self-affecting goal clear
//!   Rail-3. A git/source probe *error* is NOT this signal: it is emitted as an
//!   UNVERIFIED `deploy_state_unknown` marker, so an unknown deploy state can
//!   never forge a completion.
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

/// Three-way result of probing whether the merged change is live on the running
/// binary. The distinction between [`Self::Running`] and [`Self::Unknown`] is
/// load-bearing: Rail-3 (issue #2751) only accepts *positively confirmed*
/// running as a `verified` live signal, so a git/source probe error must be an
/// explicit "unknown" rather than being folded into "running".
enum DeployProbe {
    /// Reconcile detector positively confirmed no drift — the merged change is
    /// running. The only state that yields a `verified` live signal.
    Running,
    /// Drift positively observed: the running binary is behind merged `main`.
    NotRunning,
    /// The deploy state could not be determined (git/source probe error). Never
    /// a verified signal — the carried reason is surfaced to the reasoner.
    Unknown(String),
}

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

    /// Probe whether the running binary reflects the merged change, as a
    /// three-way state. Rail-3 stakes `verified: true` on *authenticated
    /// positive corroboration*, so this deliberately does **not** collapse a git
    /// probe error into "running":
    ///
    /// - [`DeployProbe::Running`] — the reconcile detector positively confirmed
    ///   no drift (`!needs_deploy`). Only this yields a `verified` live signal.
    /// - [`DeployProbe::NotRunning`] — drift positively observed; the running
    ///   binary is behind merged `main`. Unverified.
    /// - [`DeployProbe::Unknown`] — the git/source probe errored, so the deploy
    ///   state could not be determined. Unverified: the *absence* of a signal
    ///   must never be reported as a positive one (issue #2751). Uses the
    ///   fail-closed [`ReconcileDetector::try_detect`], not the fail-safe
    ///   `detect`, precisely so a transient git error cannot forge a verified
    ///   signal that clears Rail-3 for a self-affecting goal.
    fn deploy_state(&self) -> DeployProbe {
        let detector = crate::self_deploy::ReconcileDetector::new(
            crate::self_deploy::GitDeploySource::at(&self.repo_root),
        );
        match detector.try_detect() {
            Ok(drift) if drift.needs_deploy => DeployProbe::NotRunning,
            Ok(_) => DeployProbe::Running,
            Err(e) => DeployProbe::Unknown(e.to_string()),
        }
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
            let (kind, verified, detail) = match self.deploy_state() {
                DeployProbe::Running => (
                    "deployed_running",
                    true,
                    "merged change is running (no deploy drift)".to_string(),
                ),
                DeployProbe::NotRunning => (
                    "not_running",
                    false,
                    "running binary is behind merged main — not yet deployed".to_string(),
                ),
                // A probe error is an UNKNOWN deploy state, never a verified
                // "running". Reported as an unverified signal so the brain can
                // weigh it, but it can NEVER satisfy Rail-3 (issue #2751).
                DeployProbe::Unknown(reason) => (
                    "deploy_state_unknown",
                    false,
                    format!(
                        "deploy state could not be determined ({reason}) — unverified, not live proof"
                    ),
                ),
            };
            signals.push(LiveSignal {
                source: "reconcile_detector".to_string(),
                kind: kind.to_string(),
                // Authenticated positive corroboration only.
                verified,
                detail,
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
    fn self_affecting_goal_probe_error_is_unverified_unknown_not_forged_running() {
        // Regression (#2751): the load-bearing Rail-3 guard. A self-affecting
        // goal (repo=None → routes to Simard) whose deploy probe ERRORS — here a
        // nonexistent repo makes `ReconcileDetector::try_detect` return `Err` —
        // must NOT get a `verified: true` "deployed_running" signal. Previously
        // the fail-*open* `detect()` folded that error into `needs_deploy=false`
        // → `verified: true`, silently forging live proof for exactly the class
        // of goal (self-affecting, deploy unverifiable) this feature exists to
        // protect. It must instead emit an UNVERIFIED `deploy_state_unknown`
        // marker so Rail-3 sees zero verified live signals.
        let g = ActiveGoal::new("g", "eliminate E2BIG on spawn", 1);
        assert!(
            crate::goal_curation::completion_gate::is_self_affecting(&g),
            "fixture must be self-affecting for this guard to be meaningful"
        );
        let src = DaemonLiveSignals::new(
            PathBuf::from("/no-such-repo-xyz-123"),
            PathBuf::from("/no-such-state-root-xyz-123"),
        );
        let signals = src.gather(&g).unwrap();
        let recon = signals
            .iter()
            .find(|s| s.source == "reconcile_detector")
            .expect("a self-affecting goal must emit a reconcile_detector signal");
        assert_eq!(
            recon.kind, "deploy_state_unknown",
            "a probe error must be reported as an unknown deploy state"
        );
        assert!(
            !recon.verified,
            "an UNKNOWN deploy state must never be a verified live signal"
        );
        assert_eq!(
            signals.iter().filter(|s| s.verified).count(),
            0,
            "Rail-3 must see zero verified live signals when the deploy state is unknown"
        );
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
