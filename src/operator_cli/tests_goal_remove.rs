//! Failing TDD tests (issues
//! [#1923](https://github.com/rysweet/Simard/issues/1923) /
//! [#1925](https://github.com/rysweet/Simard/issues/1925)) for the
//! `simard goal remove <id>…` and `simard goal cleanup --placeholders`
//! operator subcommands.
//!
//! Contract under test (see
//! `docs/reference/simard-cli.md#simard-goal-remove-goal-id`):
//!
//! ## `goal remove`
//! - Variadic: accepts one or more ids in a single invocation.
//! - Removes the ids from both `active` and `backlog`.
//! - Routes through the daemon's IPC writer when available, otherwise
//!   takes the writer lock directly. (Tested indirectly — both paths
//!   share the same persistence contract; we assert outcomes, not
//!   tier choice.)
//! - Persists via `save_goal_board_with_removals` so the PR #1926
//!   resurrection failure mode is defeated.
//! - Idempotent — unknown ids are silent no-ops, no error.
//! - Exits non-zero on bridge-open / persistence failure (not tested
//!   here; covered by load_board failure-mode unit tests).
//! - No goal ids or descriptions are echoed to stdout — surface is
//!   scriptable.
//!
//! ## `goal cleanup --placeholders`
//! - Defence-in-depth sweep — removes every active or backlog goal
//!   whose description matches `^Goal <id>$` (the placeholder pattern
//!   emitted by the `tests_goal.rs::active_goal` helper).
//! - Same persistence pathway as `goal remove`.
//! - Idempotent — empty match set → no-op.
//! - `cleanup` without `--placeholders` is a usage error (exit non-zero).
//!
//! Tests fail until the implementation step adds the new subcommands to
//! `dispatch_goal_command` in `src/operator_cli/goal.rs`.

use std::path::{Path, PathBuf};

use serial_test::serial;
use tempfile::TempDir;

use crate::goal_curation::{
    ActiveGoal, BacklogItem, GoalBoard, GoalProgress, add_active_goal, add_backlog_item,
    load_goal_board, save_goal_board,
};
use crate::memory_ipc::launch_writer_bridge;
use crate::operator_cli::dispatch_operator_cli;
use crate::state_root::STATE_ROOT_ENV;

// ─── helpers ───────────────────────────────────────────────────────────────

fn isolated_state_root() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var(STATE_ROOT_ENV, &root);
    }
    (tmp, root)
}

fn active_goal_with_desc(id: &str, description: &str) -> ActiveGoal {
    ActiveGoal {
        id: id.to_string(),
        description: description.to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    }
}

fn backlog_item_with_desc(id: &str, description: &str) -> BacklogItem {
    BacklogItem {
        id: id.to_string(),
        description: description.to_string(),
        score: 0.5,
        source: "tdd-1923".to_string(),
    }
}

fn seed(root: &Path, active: Vec<ActiveGoal>, backlog: Vec<BacklogItem>) {
    let mut board = GoalBoard::new();
    for g in active {
        add_active_goal(&mut board, g).expect("add active");
    }
    for b in backlog {
        add_backlog_item(&mut board, b).expect("add backlog");
    }
    let bridge = launch_writer_bridge(root).expect("writer bridge");
    save_goal_board(&board, bridge.ops()).expect("save");
}

fn reload(root: &Path) -> GoalBoard {
    let bridge = launch_writer_bridge(root).expect("reader bridge");
    load_goal_board(bridge.ops()).expect("load_goal_board")
}

fn cli(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(args.iter().map(|s| s.to_string()))
}

// ─── `simard goal remove <id>…` — basic behaviour ──────────────────────────

#[test]
#[serial(cognitive_memory)]
fn goal_remove_single_id_removes_active_goal() {
    let (_tmp, root) = isolated_state_root();
    seed(
        &root,
        vec![
            active_goal_with_desc("alpha", "Goal alpha"),
            active_goal_with_desc("keeper", "Production goal description"),
        ],
        vec![],
    );

    cli(&["goal", "remove", "alpha"]).expect("goal remove alpha must exit 0");

    let board = reload(&root);
    assert!(
        !board.active.iter().any(|g| g.id == "alpha"),
        "alpha must be gone from active; got: {:?}",
        board.active.iter().map(|g| &g.id).collect::<Vec<_>>(),
    );
    assert!(
        board.active.iter().any(|g| g.id == "keeper"),
        "unrelated 'keeper' must survive"
    );
}

#[test]
#[serial(cognitive_memory)]
fn goal_remove_variadic_removes_multiple_ids_in_one_call() {
    // The #1923 fixture-leak vector documented in
    // docs/howto/clean-fixture-leaks.md:
    //   simard goal remove stuck-a stuck-b operator-blocked working alpha
    let (_tmp, root) = isolated_state_root();
    seed(
        &root,
        vec![
            active_goal_with_desc("stuck-a", "Goal stuck-a"),
            active_goal_with_desc("stuck-b", "Goal stuck-b"),
            active_goal_with_desc("operator-blocked", "Goal operator-blocked"),
            active_goal_with_desc("working", "Goal working"),
            active_goal_with_desc("alpha", "Goal alpha"),
        ],
        vec![],
    );

    cli(&[
        "goal",
        "remove",
        "stuck-a",
        "stuck-b",
        "operator-blocked",
        "working",
        "alpha",
    ])
    .expect("variadic goal remove must exit 0");

    let board = reload(&root);
    let remaining: Vec<&String> = board.active.iter().map(|g| &g.id).collect();
    assert!(
        remaining.is_empty(),
        "after removing all 5 fixture goals, active board must be empty; got: {remaining:?}",
    );
}

#[test]
#[serial(cognitive_memory)]
fn goal_remove_drops_from_backlog_too() {
    let (_tmp, root) = isolated_state_root();
    seed(
        &root,
        vec![],
        vec![
            backlog_item_with_desc("backlog-doomed", "Goal backlog-doomed"),
            backlog_item_with_desc("backlog-keeper", "Production backlog item"),
        ],
    );

    cli(&["goal", "remove", "backlog-doomed"]).expect("must exit 0");

    let board = reload(&root);
    assert!(!board.backlog.iter().any(|b| b.id == "backlog-doomed"));
    assert!(board.backlog.iter().any(|b| b.id == "backlog-keeper"));
}

// ─── idempotency: unknown ids are silent no-ops ────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn goal_remove_unknown_id_is_silent_no_op() {
    let (_tmp, _root) = isolated_state_root();

    // Empty board, unknown id — must not error.
    let result = cli(&["goal", "remove", "never-existed"]);
    assert!(
        result.is_ok(),
        "goal remove against unknown id must be idempotent (exit 0); got: {:?}",
        result.err().map(|e| e.to_string()),
    );
}

#[test]
#[serial(cognitive_memory)]
fn goal_remove_mixed_known_unknown_removes_known_and_ignores_unknown() {
    let (_tmp, root) = isolated_state_root();
    seed(
        &root,
        vec![active_goal_with_desc("alpha", "Goal alpha")],
        vec![],
    );

    cli(&["goal", "remove", "alpha", "phantom", "ghost"]).expect("must exit 0");

    let board = reload(&root);
    assert!(!board.active.iter().any(|g| g.id == "alpha"));
    assert!(board.active.is_empty());
}

#[test]
#[serial(cognitive_memory)]
fn goal_remove_is_re_runnable_without_error() {
    // Operator runbook safety: running the same `goal remove` twice
    // (e.g. after a script restart) must not error on the second run.
    let (_tmp, root) = isolated_state_root();
    seed(
        &root,
        vec![active_goal_with_desc("doomed", "Goal doomed")],
        vec![],
    );

    cli(&["goal", "remove", "doomed"]).expect("first run");
    cli(&["goal", "remove", "doomed"]).expect("second run must also exit 0");

    assert!(reload(&root).active.is_empty());
}

// ─── argument validation ───────────────────────────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn goal_remove_with_no_args_returns_error() {
    let (_tmp, _root) = isolated_state_root();
    let result = cli(&["goal", "remove"]);
    assert!(
        result.is_err(),
        "`goal remove` with no ids must error (at least one id is required)"
    );
}

// ─── PR #1926 regression at the CLI surface ────────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn goal_remove_defeats_pr_1926_resurrection_via_cli() {
    // Mirror of the API-level regression test but exercised through
    // the user-facing CLI surface — the path the runbook actually uses.
    let (_tmp, root) = isolated_state_root();
    seed(
        &root,
        vec![
            active_goal_with_desc("keeper", "Production goal"),
            active_goal_with_desc("doomed", "Goal doomed"),
        ],
        vec![],
    );

    cli(&["goal", "remove", "doomed"]).expect("must exit 0");

    let board = reload(&root);
    assert!(
        !board.active.iter().any(|g| g.id == "doomed"),
        "`simard goal remove doomed` must not be defeated by \
         merge-on-write resurrection; got remaining: {:?}",
        board.active.iter().map(|g| &g.id).collect::<Vec<_>>(),
    );
    assert!(board.active.iter().any(|g| g.id == "keeper"));
}

// ─── `simard goal cleanup --placeholders` ──────────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn goal_cleanup_placeholders_removes_only_goal_id_pattern() {
    // Mixed board: 3 placeholders (description == "Goal <id>") and
    // 2 production goals. Cleanup must remove only the placeholders.
    let (_tmp, root) = isolated_state_root();
    seed(
        &root,
        vec![
            active_goal_with_desc("stuck-a", "Goal stuck-a"),
            active_goal_with_desc("stuck-b", "Goal stuck-b"),
            active_goal_with_desc("operator-blocked", "Goal operator-blocked"),
            active_goal_with_desc(
                "enhance-simard-meeting-experience",
                "Enhance Simard meeting experience — richer handoffs",
            ),
            active_goal_with_desc("fix-broken-features", "Fix broken features audit"),
        ],
        vec![backlog_item_with_desc("zeta", "Goal zeta")],
    );

    cli(&["goal", "cleanup", "--placeholders"]).expect("cleanup must exit 0");

    let board = reload(&root);
    let active_ids: Vec<&str> = board.active.iter().map(|g| g.id.as_str()).collect();
    let backlog_ids: Vec<&str> = board.backlog.iter().map(|b| b.id.as_str()).collect();

    assert!(
        !active_ids.contains(&"stuck-a"),
        "placeholder stuck-a must be removed; remaining active: {active_ids:?}"
    );
    assert!(
        !active_ids.contains(&"stuck-b"),
        "placeholder stuck-b must be removed"
    );
    assert!(
        !active_ids.contains(&"operator-blocked"),
        "placeholder operator-blocked must be removed"
    );
    assert!(
        active_ids.contains(&"enhance-simard-meeting-experience"),
        "non-placeholder production goal must survive: {active_ids:?}"
    );
    assert!(
        active_ids.contains(&"fix-broken-features"),
        "non-placeholder production goal must survive"
    );
    assert!(
        !backlog_ids.contains(&"zeta"),
        "placeholder backlog item zeta must be removed; remaining backlog: {backlog_ids:?}"
    );
}

#[test]
#[serial(cognitive_memory)]
fn goal_cleanup_placeholders_on_clean_board_is_noop() {
    let (_tmp, root) = isolated_state_root();
    seed(
        &root,
        vec![active_goal_with_desc(
            "real-goal",
            "Production description that does not match placeholder pattern",
        )],
        vec![],
    );

    cli(&["goal", "cleanup", "--placeholders"]).expect("must exit 0 on clean board");

    let board = reload(&root);
    assert!(
        board.active.iter().any(|g| g.id == "real-goal"),
        "real production goal must survive a placeholder-cleanup sweep"
    );
}

#[test]
#[serial(cognitive_memory)]
fn goal_cleanup_placeholders_on_empty_board_is_noop() {
    let (_tmp, _root) = isolated_state_root();
    cli(&["goal", "cleanup", "--placeholders"]).expect("cleanup on empty board must exit 0");
}

#[test]
#[serial(cognitive_memory)]
fn goal_cleanup_without_criteria_flag_returns_error() {
    // Per docs/reference/simard-cli.md: "Invoking `simard goal cleanup`
    // with no criteria flag is an error (exit code 2, usage message on
    // stderr)." The CLI must require explicit criteria so future
    // additional flags do not silently change behaviour.
    let (_tmp, _root) = isolated_state_root();
    let result = cli(&["goal", "cleanup"]);
    assert!(
        result.is_err(),
        "`simard goal cleanup` with no criteria flag must error"
    );
}

#[test]
#[serial(cognitive_memory)]
fn goal_cleanup_placeholders_preserves_description_when_id_substring_matches() {
    // Edge case: the placeholder predicate is strictly `^Goal <id>$`
    // (anchored). A production description that *contains* the substring
    // "Goal x" but is not exactly equal must NOT be swept.
    let (_tmp, root) = isolated_state_root();
    seed(
        &root,
        vec![
            // Strictly equals "Goal alpha" — must be removed.
            active_goal_with_desc("alpha", "Goal alpha"),
            // Mentions "Goal beta" inside a longer description —
            // must survive.
            active_goal_with_desc("beta", "Improve Goal beta tooling and docs"),
            // Wrong prefix — must survive.
            active_goal_with_desc("gamma", "goal gamma"),
        ],
        vec![],
    );

    cli(&["goal", "cleanup", "--placeholders"]).expect("must exit 0");

    let board = reload(&root);
    let ids: Vec<&str> = board.active.iter().map(|g| g.id.as_str()).collect();
    assert!(!ids.contains(&"alpha"), "strict match removed");
    assert!(ids.contains(&"beta"), "substring match preserved");
    assert!(ids.contains(&"gamma"), "case-sensitive prefix preserved");
}
