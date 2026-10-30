//! Idempotent `gh label` ensurer for the OODA no-progress / brain-failure
//! tracking-issue path.
//!
//! Three sites file an operator-facing tracking issue with
//! `gh issue create --label ooda-stuck`:
//!
//! * the deterministic brain-failure safeguard
//!   ([`crate::ooda_actions::advance_goal::spawn`], the `.output()` + `match`
//!   site),
//! * the `EngineerLifecycleDecision::OpenTrackingIssue` path (same file, the
//!   `.status()` + `if let Err` site), and
//! * the no-progress breaker filer
//!   ([`crate::ooda_loop::no_progress::GhIssueFiler`]).
//!
//! Every one hard-coded `--label ooda-stuck`, but that label does not exist in
//! the repo, so `gh` exits non-zero *every* time with
//! `could not add label: 'ooda-stuck' not found`. The breaker still records
//! `escalated=1` internally while the GitHub issue is never created — a silent
//! broken escalation path that hides stuck goals from the operator.
//!
//! This module makes tracking-issue creation robust to the missing label:
//! before creating the issue, each site idempotently ensures the label exists
//! (`gh label create ooda-stuck`, treating "already exists" as success). If the
//! label cannot be ensured, the caller degrades gracefully by filing the issue
//! *without* `--label` (visible, non-silent). The ensurer itself emits **no**
//! tracing and never panics — it returns a [`LabelEnsure`] classification the
//! caller logs with its own static tracing `target:`.
//!
//! Security: the label is the compile-time constant [`OODA_STUCK_LABEL`], never
//! a runtime-derived string, and invocation is argv-only (`std::process::Command`
//! with separate args) — never `sh -c` / string interpolation.

use std::io::{Read, Seek, SeekFrom};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The tracking-issue label. A fixed, compile-time `&'static str` — never a
/// runtime-derived value — so no title/body/branch string can ever flow into
/// the `--label` argument (argument-injection safe).
pub(crate) const OODA_STUCK_LABEL: &str = "ooda-stuck";

/// Maximum time spent on the optional label subprocess before escalation.
const LABEL_CREATE_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of idempotently ensuring the tracking-issue label exists.
///
/// The caller uses [`LabelEnsure::label_present`] to decide whether it may pass
/// `--label` to `gh issue create`, and logs the outcome with its own static
/// tracing `target:` (this type emits no tracing itself).
#[derive(Debug)]
pub(crate) enum LabelEnsure {
    /// `gh label create` succeeded — the label was created by this call.
    Created,
    /// `gh label create` reported the label already exists — the idempotent
    /// success path (treated as success, not failure).
    AlreadyExists,
    /// The label could not be ensured (auth failure, `gh` spawn error, unknown
    /// non-zero exit, timeout). Carries an operator-legible, non-empty cause so the
    /// caller can `warn!` it — failures are surfaced, never silently swallowed.
    /// The caller degrades to filing the issue *without* `--label`.
    Unavailable(String),
}

impl LabelEnsure {
    /// True when the label is present (freshly created or pre-existing), so the
    /// caller may safely include `--label`. False for [`LabelEnsure::Unavailable`],
    /// signalling the degraded unlabeled-but-still-filed path.
    fn label_present(&self) -> bool {
        matches!(self, LabelEnsure::Created | LabelEnsure::AlreadyExists)
    }

    /// Build the `gh issue create` argv for a tracking issue, appending
    /// `--label ooda-stuck` only when the label is present. In the degraded
    /// ([`LabelEnsure::Unavailable`]) path the issue is still filed — just
    /// without the label — so a stuck goal is never hidden from the operator.
    ///
    /// This is the single source of truth for the labeled/unlabeled decision
    /// shared by all three tracking-issue filing sites (issue #4472 arose from
    /// that decision being duplicated and fixed inconsistently across sites).
    pub(crate) fn issue_create_args<'a>(&self, title: &'a str, body: &'a str) -> Vec<&'a str> {
        let mut args = vec!["issue", "create", "--title", title, "--body", body];
        if self.label_present() {
            args.push("--label");
            args.push(OODA_STUCK_LABEL);
        }
        args
    }
}

/// Idempotently ensure `label` exists via `gh label create`, classifying the
/// result. Emits no tracing and never panics; returns a [`LabelEnsure`] the
/// caller logs. The child has a five-second deadline; expiry returns
/// `Unavailable` after termination and reaping so issue filing can proceed.
pub(crate) fn ensure_gh_label(label: &'static str) -> LabelEnsure {
    run_label_create(
        Command::new("gh").args(["label", "create", label]),
        LABEL_CREATE_TIMEOUT,
    )
}

fn run_label_create(command: &mut Command, timeout: Duration) -> LabelEnsure {
    // File capture avoids pipe-buffer deadlocks and inherited-pipe EOF waits.
    let mut stderr = match tempfile::tempfile() {
        Ok(file) => file,
        Err(error) => {
            return LabelEnsure::Unavailable(format!(
                "gh label create stderr capture setup failed: {error}"
            ));
        }
    };
    let stderr_writer = match stderr.try_clone() {
        Ok(file) => file,
        Err(error) => {
            return LabelEnsure::Unavailable(format!(
                "gh label create stderr capture setup failed: {error}"
            ));
        }
    };
    let started = Instant::now();
    let mut child = match command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr_writer)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return LabelEnsure::Unavailable(format!(
                "gh label create process spawn failed: {error}"
            ));
        }
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return stop_label_create(
                        &mut child,
                        format!("gh label create timed out after {} ms", timeout.as_millis()),
                    );
                }
                std::thread::sleep(remaining.min(Duration::from_millis(20)));
            }
            Err(error) => {
                return stop_label_create(
                    &mut child,
                    format!("gh label create process wait failed: {error}"),
                );
            }
        }
    };
    // Bound diagnostic memory even if a broken gh writes excessive stderr.
    let mut bytes = Vec::new();
    if let Err(error) = stderr
        .seek(SeekFrom::Start(0))
        .and_then(|_| stderr.take(64 * 1024).read_to_end(&mut bytes))
    {
        return LabelEnsure::Unavailable(format!(
            "gh label create stderr capture read failed: {error}"
        ));
    }
    classify_label_create(status.success(), &String::from_utf8_lossy(&bytes))
}

fn stop_label_create(child: &mut Child, mut reason: String) -> LabelEnsure {
    if let Err(error) = child.kill() {
        reason.push_str(&format!("; child termination failed: {error}"));
    }
    if let Err(error) = child.wait() {
        reason.push_str(&format!("; child reap failed: {error}"));
    }
    LabelEnsure::Unavailable(reason)
}

/// Pure classification of a `gh label create` invocation from its exit-success
/// flag and captured stderr. Extracted from [`ensure_gh_label`] so the decision
/// logic is testable without spawning `gh`.
///
/// Contract:
/// * `success == true` -> [`LabelEnsure::Created`].
/// * failure whose stderr contains `"already exists"` (case-insensitive) ->
///   [`LabelEnsure::AlreadyExists`] (idempotent success).
/// * any other failure -> [`LabelEnsure::Unavailable`] carrying a **non-empty**
///   reason that preserves the underlying cause (never a silent/empty
///   degradation).
fn classify_label_create(success: bool, stderr: &str) -> LabelEnsure {
    if success {
        return LabelEnsure::Created;
    }
    if stderr.to_ascii_lowercase().contains("already exists") {
        return LabelEnsure::AlreadyExists;
    }
    let trimmed = stderr.trim();
    let reason = if trimmed.is_empty() {
        "gh label create failed with a non-zero exit and no stderr output".to_string()
    } else {
        trimmed.to_string()
    };
    LabelEnsure::Unavailable(reason)
}

#[cfg(test)]
mod tests {
    use super::{LabelEnsure, OODA_STUCK_LABEL, classify_label_create};

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(simard_process_reaper)]
    fn subprocess_results_preserve_label_classification() {
        use std::process::Command;
        use std::time::Duration;

        for (script, expected) in [
            ("exit 0", "created"),
            ("printf 'label already exists' >&2; exit 1", "exists"),
            (
                "printf 'HTTP 403: label unavailable' >&2; exit 1",
                "unavailable",
            ),
            ("exit 1", "empty"),
        ] {
            let result = super::run_label_create(
                Command::new("/bin/sh").args(["-c", script]),
                Duration::from_secs(2),
            );
            match (expected, result) {
                ("created", LabelEnsure::Created) | ("exists", LabelEnsure::AlreadyExists) => {}
                ("unavailable", LabelEnsure::Unavailable(reason)) => {
                    assert!(reason.contains("HTTP 403: label unavailable"));
                }
                ("empty", LabelEnsure::Unavailable(reason)) => {
                    assert!(reason.contains("non-zero exit and no stderr"));
                }
                (expected, result) => panic!("expected {expected}, got {result:?}"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(simard_process_reaper)]
    fn excessive_stderr_does_not_block_and_has_bounded_diagnostics() {
        let fixture = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(fixture.path(), vec![b'x'; 1024 * 1024]).unwrap();
        let result = super::run_label_create(
            std::process::Command::new("/bin/sh")
                .args(["-c", "/bin/cat \"$1\" >&2; exit 1", "fixture"])
                .arg(fixture.path()),
            std::time::Duration::from_secs(2),
        );
        match result {
            LabelEnsure::Unavailable(reason) => assert_eq!(reason.len(), 64 * 1024),
            other => panic!("expected captured stderr, got {other:?}"),
        }
    }

    #[test]
    fn missing_executable_is_unavailable() {
        let fixture = tempfile::tempdir().unwrap();
        let result = super::run_label_create(
            &mut std::process::Command::new(fixture.path().join("missing-gh")),
            std::time::Duration::from_secs(1),
        );
        assert!(
            matches!(result, LabelEnsure::Unavailable(reason) if reason.contains("spawn failed"))
        );
    }

    #[test]
    fn label_constant_is_the_compile_time_ooda_stuck_literal() {
        // Security/DRY: the label is a fixed &'static str shared by all three
        // sites, never a runtime-derived string.
        assert_eq!(OODA_STUCK_LABEL, "ooda-stuck");
    }

    #[test]
    fn successful_gh_label_create_is_classified_as_created() {
        assert!(
            matches!(classify_label_create(true, ""), LabelEnsure::Created),
            "a zero-exit `gh label create` must classify as Created",
        );
    }

    #[test]
    fn already_exists_failure_is_treated_as_success() {
        // `gh label create` exits non-zero when the label already exists; this
        // is the idempotent success path that stops the recurring journal
        // signature (`could not add label: 'ooda-stuck' not found`).
        let stderr = "failed to create label: 'ooda-stuck' already exists";
        assert!(
            matches!(
                classify_label_create(false, stderr),
                LabelEnsure::AlreadyExists
            ),
            "an 'already exists' failure must be treated as success",
        );
    }

    #[test]
    fn already_exists_match_is_case_insensitive() {
        // gh wording varies across versions; match the stable substring
        // regardless of case so a capitalisation change never regresses us into
        // the broken (silent) path.
        let stderr = "GraphQL: Label already exists (createLabel)";
        assert!(
            matches!(
                classify_label_create(false, stderr),
                LabelEnsure::AlreadyExists
            ),
            "'already exists' matching must be case-insensitive",
        );
    }

    #[test]
    fn genuine_failure_is_unavailable_and_preserves_the_cause() {
        // A real failure (e.g. auth) must NOT be silently swallowed — the
        // operator-visible cause is preserved so the caller can warn! it before
        // degrading to the unlabeled path.
        let stderr = "HTTP 401: Bad credentials";
        match classify_label_create(false, stderr) {
            LabelEnsure::Unavailable(reason) => assert!(
                reason.contains("Bad credentials"),
                "the degraded-path reason must preserve the cause, got: {reason}",
            ),
            other => panic!("expected Unavailable for a genuine failure, got {other:?}"),
        }
    }

    #[test]
    fn failure_with_empty_stderr_still_yields_a_nonempty_reason() {
        // No silent/empty degradation: even with no stderr, the reason must be a
        // non-empty, operator-legible string.
        match classify_label_create(false, "") {
            LabelEnsure::Unavailable(reason) => assert!(
                !reason.trim().is_empty(),
                "an Unavailable classification must never carry an empty reason",
            ),
            other => panic!("expected Unavailable for a stderr-less failure, got {other:?}"),
        }
    }

    #[test]
    fn issue_create_args_include_label_when_present() {
        // When the label is ensured, `--label ooda-stuck` is appended so the
        // tracking issue is filed with its operator-facing label.
        for ensured in [LabelEnsure::Created, LabelEnsure::AlreadyExists] {
            let args = ensured.issue_create_args("t", "b");
            assert_eq!(
                args,
                vec![
                    "issue",
                    "create",
                    "--title",
                    "t",
                    "--body",
                    "b",
                    "--label",
                    OODA_STUCK_LABEL
                ],
                "a present label must append `--label {OODA_STUCK_LABEL}`",
            );
        }
    }

    #[test]
    fn issue_create_args_omit_label_but_still_file_when_unavailable() {
        // Degraded path: the label could not be ensured, so the issue is filed
        // WITHOUT `--label` — never silently dropped. This is the core fix for
        // the silent broken-escalation bug (issue #4472).
        let args = LabelEnsure::Unavailable("boom".to_string()).issue_create_args("t", "b");
        assert_eq!(
            args,
            vec!["issue", "create", "--title", "t", "--body", "b"],
            "an unavailable label must still file the issue, just without `--label`",
        );
    }
}
