#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::ooda_actions::goal_session::*;

    // -- GOAL_SESSION_OBJECTIVE prompt asset --

    #[test]
    fn goal_session_objective_prompt_is_non_empty() {
        const GOAL_SESSION_OBJECTIVE: &str =
            include_str!("../../prompt_assets/simard/goal_session_objective.md");
        assert!(!GOAL_SESSION_OBJECTIVE.trim().is_empty());
    }

    // -- GOAL_SESSION_IDENTITY prompt asset --

    #[test]
    fn goal_session_identity_prompt_is_non_empty() {
        const GOAL_SESSION_IDENTITY: &str =
            include_str!("../../prompt_assets/simard/goal_session_identity.md");
        assert!(!GOAL_SESSION_IDENTITY.trim().is_empty());
    }

    // -- assess_only_outcome: error surfacing (issue #1258) --

    #[test]
    fn assess_only_outcome_surfaces_error_when_goal_id_not_found() {
        use crate::goal_curation::GoalBoard;
        use crate::ooda_loop::{ActionKind, PlannedAction};

        // GoalBoard with no matching goal_id (entirely empty active list).
        let mut board = GoalBoard::new();

        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("missing-goal".to_string()),
            description: "advance missing-goal".to_string(),
        };

        let outcome = assess_only_outcome(
            &action,
            &mut board,
            "missing-goal",
            "looks ~halfway done",
            50,
        );

        // Failure must be surfaced — not silently swallowed as success.
        assert!(
            !outcome.success,
            "expected failed outcome when update_goal_progress errors, got success",
        );
        // The outcome detail must carry the underlying error so the OODA
        // journal records the cause (issue #1258).
        assert!(
            outcome.detail.contains("assess_only failed"),
            "detail missing failure marker: {}",
            outcome.detail,
        );
        assert!(
            outcome.detail.contains("missing-goal"),
            "detail missing goal id: {}",
            outcome.detail,
        );
        assert!(
            outcome.detail.contains("not found"),
            "detail missing underlying SimardError reason: {}",
            outcome.detail,
        );
    }

    #[test]
    fn assess_only_outcome_succeeds_when_goal_id_matches() {
        use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
        use crate::ooda_loop::{ActionKind, PlannedAction};

        let mut board = GoalBoard::new();
        board.active.push(ActiveGoal {
            id: "goal-real".to_string(),
            description: "do the thing".to_string(),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
        });

        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("goal-real".to_string()),
            description: "advance goal-real".to_string(),
        };

        let outcome = assess_only_outcome(&action, &mut board, "goal-real", "halfway", 50);

        assert!(outcome.success, "expected success when goal exists");
        assert!(
            outcome.detail.starts_with("assess_only:"),
            "expected success detail format, got: {}",
            outcome.detail,
        );
        assert_eq!(
            board.active[0].status,
            GoalProgress::InProgress { percent: 50 },
            "board progress should have been updated on Ok path",
        );
    }

    #[test]
    fn objective_buffer_contains_goal_info() {
        use std::fmt::Write;

        let goal_id = "goal-42";
        let percent = 25u32;
        let description = "Implement authentication";
        let prompt = "Test objective instructions";

        let mut objective = String::with_capacity(256);
        let _ = write!(
            objective,
            "Goal '{}' ({}% complete): {}\n\n{}\n\nEnvironment context:\n- Git status: ",
            goal_id, percent, description, prompt,
        );
        objective.push_str("clean");

        assert!(objective.contains("goal-42"));
        assert!(objective.contains("25% complete"));
        assert!(objective.contains("Implement authentication"));
        assert!(objective.contains("clean"));
    }

    #[test]
    fn objective_formats_git_changes_count() {
        use std::fmt::Write;

        let git_status = "M file1.rs\nM file2.rs\nA file3.rs";
        let mut objective = String::new();
        objective.push_str("- Git status: ");
        if git_status.is_empty() {
            objective.push_str("clean");
        } else {
            let _ = write!(objective, "{} changed files", git_status.lines().count());
        }
        assert!(objective.contains("3 changed files"));
    }

    #[test]
    fn objective_formats_open_issues() {
        let issues = ["Issue #1".to_string(), "Issue #2".to_string()];
        let mut objective = String::new();
        objective.push_str("- Open issues: ");
        if issues.is_empty() {
            objective.push_str("none");
        } else {
            for (i, issue) in issues.iter().enumerate() {
                if i > 0 {
                    objective.push_str("; ");
                }
                objective.push_str(issue);
            }
        }
        assert!(objective.contains("Issue #1; Issue #2"));
    }

    #[test]
    fn objective_formats_empty_issues_as_none() {
        let issues: Vec<String> = vec![];
        let mut objective = String::new();
        objective.push_str("- Open issues: ");
        if issues.is_empty() {
            objective.push_str("none");
        }
        assert!(objective.contains("none"));
    }

    #[test]
    fn objective_limits_commits_to_five() {
        let commits: Vec<String> = (0..10).map(|i| format!("commit-{i}")).collect();
        let mut objective = String::new();
        objective.push_str("- Recent commits: ");
        for (i, commit) in commits.iter().take(5).enumerate() {
            if i > 0 {
                objective.push_str("; ");
            }
            objective.push_str(commit);
        }
        assert!(objective.contains("commit-4"));
        assert!(!objective.contains("commit-5"));
    }

    // ===== Issue #929: parse_goal_action tests =====
    //
    // These tests specify the contract for the new GoalAction enum and
    // parse_goal_action() function. They MUST fail until the parser is
    // implemented in this module.

    use crate::ooda_actions::goal_session::{GoalAction, parse_goal_action};

    #[test]
    fn parse_clean_spawn_engineer_json() {
        let response = r#"{"action": "spawn_engineer", "task": "fix the auth bug", "files": ["src/auth.rs", "src/lib.rs"]}"#;
        let parsed = parse_goal_action(response).expect("clean spawn_engineer JSON must parse");
        match parsed {
            GoalAction::SpawnEngineer { task, files, .. } => {
                assert_eq!(task, "fix the auth bug");
                assert_eq!(
                    files,
                    vec!["src/auth.rs".to_string(), "src/lib.rs".to_string()]
                );
            }
            other => panic!("expected SpawnEngineer, got {other:?}"),
        }
    }

    #[test]
    fn parse_spawn_engineer_default_files_when_missing() {
        let response = r#"{"action": "spawn_engineer", "task": "do the thing"}"#;
        let parsed = parse_goal_action(response).expect("missing files should default to empty");
        match parsed {
            GoalAction::SpawnEngineer { task, files, .. } => {
                assert_eq!(task, "do the thing");
                assert!(files.is_empty(), "files should default to empty Vec");
            }
            other => panic!("expected SpawnEngineer, got {other:?}"),
        }
    }

    #[test]
    fn parse_clean_noop_json() {
        let response = r#"{"action": "noop", "reason": "all goals are already in progress"}"#;
        let parsed = parse_goal_action(response).expect("clean noop JSON must parse");
        match parsed {
            GoalAction::Noop { reason } => {
                assert_eq!(reason, "all goals are already in progress");
            }
            other => panic!("expected Noop, got {other:?}"),
        }
    }

    #[test]
    fn parse_clean_assess_only_json() {
        let response = r#"{"action": "assess_only", "assessment": "good progress, no spawn needed", "progress_pct": 65}"#;
        let parsed = parse_goal_action(response).expect("clean assess_only JSON must parse");
        match parsed {
            GoalAction::AssessOnly {
                assessment,
                progress_pct,
            } => {
                assert_eq!(assessment, "good progress, no spawn needed");
                assert_eq!(progress_pct, 65);
            }
            other => panic!("expected AssessOnly, got {other:?}"),
        }
    }

    #[test]
    fn parse_assess_only_at_zero_percent() {
        let response =
            r#"{"action": "assess_only", "assessment": "not started", "progress_pct": 0}"#;
        let parsed = parse_goal_action(response).expect("0% should be valid");
        assert!(matches!(
            parsed,
            GoalAction::AssessOnly {
                progress_pct: 0,
                ..
            }
        ));
    }

    #[test]
    fn parse_assess_only_at_100_percent() {
        let response = r#"{"action": "assess_only", "assessment": "done", "progress_pct": 100}"#;
        let parsed = parse_goal_action(response).expect("100% should be valid");
        assert!(matches!(
            parsed,
            GoalAction::AssessOnly {
                progress_pct: 100,
                ..
            }
        ));
    }

    #[test]
    fn parse_json_embedded_in_prose() {
        let response = r#"After thinking carefully, here is my decision:

{"action": "noop", "reason": "everything is fine"}

Hope that helps!"#;
        let parsed = parse_goal_action(response).expect("JSON embedded in prose must be extracted");
        match parsed {
            GoalAction::Noop { reason } => assert_eq!(reason, "everything is fine"),
            other => panic!("expected Noop, got {other:?}"),
        }
    }

    #[test]
    fn parse_json_embedded_in_code_fence() {
        let response = "```json\n{\"action\": \"spawn_engineer\", \"task\": \"refactor\"}\n```";
        let parsed = parse_goal_action(response).expect("JSON in code fence must parse");
        assert!(matches!(parsed, GoalAction::SpawnEngineer { .. }));
    }

    #[test]
    fn parse_json_with_nested_braces_in_strings() {
        // The brace-balanced extractor must respect string boundaries and
        // not be confused by literal { or } inside JSON string values.
        let response = r#"prefix {"action": "spawn_engineer", "task": "implement fn foo() { return {}; }"} suffix"#;
        let parsed = parse_goal_action(response)
            .expect("nested braces inside strings must not break extraction");
        match parsed {
            GoalAction::SpawnEngineer { task, .. } => {
                assert_eq!(task, "implement fn foo() { return {}; }");
            }
            other => panic!("expected SpawnEngineer, got {other:?}"),
        }
    }
}
