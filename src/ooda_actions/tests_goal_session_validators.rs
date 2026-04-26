#[cfg(test)]
mod label_sanitizer_tests {
    use crate::ooda_actions::goal_session::is_plausible_label;

    #[test]
    fn rejects_ellipsis_placeholders() {
        assert!(!is_plausible_label("..."));
        assert!(!is_plausible_label(".…"));
        assert!(!is_plausible_label("…"));
        assert!(!is_plausible_label("---"));
        assert!(!is_plausible_label(""));
        assert!(!is_plausible_label("   "));
    }

    #[test]
    fn accepts_real_labels() {
        assert!(is_plausible_label("bug"));
        assert!(is_plausible_label("enhancement"));
        assert!(is_plausible_label("good first issue"));
        assert!(is_plausible_label("workflow:default"));
        assert!(is_plausible_label("parity"));
    }

    #[test]
    fn rejects_too_long_labels() {
        let long = "x".repeat(60);
        assert!(!is_plausible_label(&long));
    }
}

mod placeholder_echo_tests {
    use crate::ooda_actions::goal_session::{GoalAction, action_is_valid, is_placeholder_echo};

    #[test]
    fn rejects_short_title_single_line_placeholder() {
        // Exact bug observed 2026-04-25 — daemon filed issues #1247-1249 with
        // these literal template tokens as the title and body.
        assert!(is_placeholder_echo("<short title, single line>"));
        assert!(is_placeholder_echo("<markdown body, can be multi-line>"));
        assert!(is_placeholder_echo("<comment body, can be multi-line>"));
        assert!(is_placeholder_echo("<reason for closing>"));
    }

    #[test]
    fn rejects_existing_placeholders_still() {
        assert!(is_placeholder_echo("<one-paragraph concrete task>"));
        assert!(is_placeholder_echo("<title>"));
        assert!(is_placeholder_echo("<body>"));
    }

    #[test]
    fn accepts_real_titles_and_bodies() {
        assert!(!is_placeholder_echo(
            "P3: issue creator dedup by title-hash"
        ));
        assert!(!is_placeholder_echo("Fix race in cleanup hook"));
        // Bodies that legitimately use angle-bracketed jargon must pass.
        assert!(!is_placeholder_echo(
            "The handler returns `Result<T, E>` instead of panicking."
        ));
    }

    #[test]
    fn gh_issue_create_with_placeholder_title_is_invalid() {
        let action = GoalAction::GhIssueCreate {
            title: "<short title, single line>".into(),
            body: "real body content".into(),
            repo: None,
            labels: vec![],
        };
        assert!(
            !action_is_valid(&action),
            "placeholder title must be rejected"
        );
    }

    #[test]
    fn gh_issue_create_with_placeholder_body_is_invalid() {
        let action = GoalAction::GhIssueCreate {
            title: "real title".into(),
            body: "<markdown body, can be multi-line>".into(),
            repo: None,
            labels: vec![],
        };
        assert!(
            !action_is_valid(&action),
            "placeholder body must be rejected"
        );
    }

    #[test]
    fn gh_issue_create_with_real_content_is_valid() {
        let action = GoalAction::GhIssueCreate {
            title: "Fix the issue creator template echo bug".into(),
            body: "When the LLM returns the schema scaffold verbatim, we file garbage.".into(),
            repo: None,
            labels: vec!["bug".into()],
        };
        assert!(action_is_valid(&action));
    }

    #[test]
    fn gh_issue_comment_rejects_placeholder_body() {
        let action = GoalAction::GhIssueComment {
            issue: 1247,
            body: "<comment body, can be multi-line>".into(),
            repo: None,
        };
        assert!(!action_is_valid(&action));
    }

    #[test]
    fn gh_pr_comment_rejects_placeholder_body() {
        let action = GoalAction::GhPrComment {
            pr: 1240,
            body: "<comment body, can be multi-line>".into(),
            repo: None,
        };
        assert!(!action_is_valid(&action));
    }
}

mod makework_title_tests {
    use crate::ooda_actions::goal_session::{GoalAction, action_is_valid, is_makework_title};

    #[test]
    fn rejects_verify_existing_issue() {
        // Exact pattern observed: "verify existing issue #1177" — 5 dupes
        // closed manually 2026-04-25 as part of P3 cleanup.
        assert!(is_makework_title("verify existing issue #1177"));
        assert!(is_makework_title("Verify existing issue #1177"));
        assert!(is_makework_title("verify existing: foo bar"));
    }

    #[test]
    fn rejects_test_only_titles() {
        assert!(is_makework_title("test-only sanity check"));
        assert!(is_makework_title("test-only: validate cleanup"));
    }

    #[test]
    fn rejects_monitor_pr_titles() {
        assert!(is_makework_title("monitor-pr-1234"));
        assert!(is_makework_title("monitor pr 1234"));
    }

    #[test]
    fn rejects_rebase_and_merge_pr_titles() {
        assert!(is_makework_title("rebase-and-merge-pr-9999"));
        assert!(is_makework_title("Rebase and merge PR 1240"));
    }

    #[test]
    fn rejects_bare_verb_titles() {
        assert!(is_makework_title("observe"));
        assert!(is_makework_title("check"));
        assert!(is_makework_title("monitor"));
        assert!(is_makework_title("verify"));
        assert!(is_makework_title("Observe "));
        assert!(is_makework_title("observe "));
        assert!(is_makework_title("check: gym health"));
    }

    /// Regression #1261: bare slug titles like `test-only` (issue #1260)
    /// previously slipped through because the prefix matcher required a
    /// trailing space/colon and the bare-token allowlist only covered the
    /// single-verb forms.
    #[test]
    fn rejects_bare_slug_titles_1261() {
        assert!(is_makework_title("test-only"));
        assert!(is_makework_title("Test-Only"));
        assert!(is_makework_title(" test-only "));
        assert!(is_makework_title("verify-existing"));
        assert!(is_makework_title("monitor-pr"));
        assert!(is_makework_title("rebase-and-merge-pr"));
    }

    #[test]
    fn accepts_real_engineering_titles() {
        // These have legitimate verbs but are real work, not theater.
        assert!(!is_makework_title("Fix race in cleanup hook"));
        assert!(!is_makework_title("Add eval watchdog for dead-signal"));
        assert!(!is_makework_title(
            "P4: cap /tmp/simard-engineer-target at 10 GB"
        ));
        assert!(!is_makework_title(
            "refactor scheduler to use bounded queue"
        ));
        // Title that *contains* the word "verify" but isn't a make-work
        // pattern must still pass.
        assert!(!is_makework_title("Add tests to verify rate limiter"));
    }

    #[test]
    fn gh_issue_create_with_makework_title_is_invalid() {
        let action = GoalAction::GhIssueCreate {
            title: "verify existing issue #1177".into(),
            body: "Reviewing...".into(),
            repo: None,
            labels: vec![],
        };
        assert!(
            !action_is_valid(&action),
            "make-work title must be rejected"
        );
    }

    #[test]
    fn gh_issue_create_with_real_title_is_valid() {
        let action = GoalAction::GhIssueCreate {
            title: "P5: audit fail-open paths in src/".into(),
            body: "97 .ok() discards need classification.".into(),
            repo: None,
            labels: vec!["enhancement".into()],
        };
        assert!(action_is_valid(&action));
    }
}
