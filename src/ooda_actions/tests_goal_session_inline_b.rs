#[cfg(test)]
mod tests_b {
    #[allow(unused_imports)]
    use crate::ooda_actions::goal_session::*;

    // -- GOAL_SESSION_OBJECTIVE prompt asset --

    #[test]
    fn parse_json_with_escaped_quotes_in_strings() {
        let response = r#"{"action": "noop", "reason": "user said \"go away\""}"#;
        let parsed = parse_goal_action(response).expect("escaped quotes must not break extraction");
        match parsed {
            GoalAction::Noop { reason } => assert_eq!(reason, r#"user said "go away""#),
            other => panic!("expected Noop, got {other:?}"),
        }
    }

    #[test]
    fn parse_returns_none_for_malformed_json() {
        let response = r#"{"action": "spawn_engineer", "task": "broken"#; // unclosed
        assert!(
            parse_goal_action(response).is_none(),
            "malformed JSON must return None, never panic"
        );
    }

    #[test]
    fn parse_returns_none_for_unknown_action_tag() {
        let response = r#"{"action": "explode_universe", "task": "whatever"}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "unknown action tag must return None"
        );
    }

    #[test]
    fn parse_returns_none_for_missing_required_field() {
        // spawn_engineer requires "task"
        let response = r#"{"action": "spawn_engineer", "files": []}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "missing required field must return None"
        );
    }

    #[test]
    fn parse_returns_none_for_noop_missing_reason() {
        let response = r#"{"action": "noop"}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "noop missing reason must return None"
        );
    }

    #[test]
    fn parse_returns_none_for_assess_only_missing_progress() {
        let response = r#"{"action": "assess_only", "assessment": "x"}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "assess_only missing progress_pct must return None"
        );
    }

    #[test]
    fn parse_returns_none_for_progress_pct_above_100() {
        let response = r#"{"action": "assess_only", "assessment": "x", "progress_pct": 150}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "progress_pct > 100 must be rejected"
        );
    }

    #[test]
    fn parse_returns_none_for_negative_progress_pct() {
        // u8 deserialization will reject negatives.
        let response = r#"{"action": "assess_only", "assessment": "x", "progress_pct": -1}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "negative progress_pct must be rejected"
        );
    }

    #[test]
    fn parse_returns_none_for_empty_string() {
        assert!(parse_goal_action("").is_none());
    }

    #[test]
    fn parse_returns_none_for_pure_prose() {
        let response = "I think we should spawn an engineer to fix this.";
        assert!(
            parse_goal_action(response).is_none(),
            "prose without JSON must return None"
        );
    }

    #[test]
    fn parse_returns_none_for_empty_task() {
        let response = r#"{"action": "spawn_engineer", "task": ""}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "empty task must be rejected (per design spec)"
        );
    }

    #[test]
    fn parse_returns_none_for_whitespace_only_task() {
        let response = r#"{"action": "spawn_engineer", "task": "   \t\n  "}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "whitespace-only task must be rejected"
        );
    }

    // -- placeholder-echo rejection (regression for daemon bug observed
    //    2026-04-23: LLM returned the schema example verbatim and
    //    poisoned the engineer loop with `<one-paragraph concrete task>`
    //    as the objective for every cycle) ---------------------------

    #[test]
    fn parse_rejects_verbatim_schema_placeholder_task() {
        let response = r#"{"action": "spawn_engineer", "task": "<one-paragraph concrete task>"}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "task that echoes the prompt's <one-paragraph concrete task> placeholder must be rejected"
        );
    }

    #[test]
    fn parse_rejects_generic_angle_bracket_placeholder() {
        let response = r#"{"action": "spawn_engineer", "task": "<your task here>"}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "generic angle-bracket placeholders like <your task here> must be rejected"
        );
    }

    #[test]
    fn parse_accepts_task_mentioning_angle_brackets_in_real_content() {
        // Legitimate task that happens to include angle brackets (e.g. a
        // generic type, HTML tag, or comparison operator). Must NOT be
        // rejected — the placeholder filter is whole-string exact, not
        // substring.
        let response = r#"{"action": "spawn_engineer", "task": "Fix generic parser to handle Vec<String> type annotations in src/parser.rs around line 142"}"#;
        assert!(
            parse_goal_action(response).is_some(),
            "real tasks that contain <...> substrings must still be accepted"
        );
    }

    #[test]
    fn parse_rejects_oversized_input() {
        // Per design: 64 KiB cap on input.
        let huge = "x".repeat(70 * 1024);
        let response = format!(r#"{{"action": "noop", "reason": "{huge}"}}"#);
        assert!(
            parse_goal_action(&response).is_none(),
            "input exceeding 64 KiB must be rejected"
        );
    }

    #[test]
    fn parse_rejects_excessive_brace_depth() {
        // Per design: 256 brace-depth cap to prevent parser DoS.
        let deep = "{".repeat(300) + &"}".repeat(300);
        assert!(
            parse_goal_action(&deep).is_none(),
            "brace depth > 256 must be rejected"
        );
    }

    #[test]
    fn parse_picks_first_complete_json_object() {
        // If multiple candidate JSON blocks appear, the first valid one wins.
        let response = r#"garbage {not json} more {"action": "noop", "reason": "first"} and {"action": "noop", "reason": "second"}"#;
        let parsed = parse_goal_action(response).expect("should extract first valid JSON");
        match parsed {
            GoalAction::Noop { reason } => assert_eq!(reason, "first"),
            other => panic!("expected Noop, got {other:?}"),
        }
    }

    #[test]
    fn goal_action_variants_are_distinct() {
        // Sanity: the three variants compare unequal.
        let a = GoalAction::Noop { reason: "x".into() };
        let b = GoalAction::AssessOnly {
            assessment: "x".into(),
            progress_pct: 0,
        };
        let c = GoalAction::SpawnEngineer {
            task: "x".into(),
            files: vec![],
            issue: None,
        };
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }
}
