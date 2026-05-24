//! `simard goal` operator subcommands: `list`, `unblock <id>`,
//! `unblock-all`, `remove <id>…`, `cleanup --placeholders`. Operator
//! escape hatch for the issue-#1911 OODA goal lockout (and a
//! general-purpose board-inspection tool) plus the
//! [#1923](https://github.com/rysweet/Simard/issues/1923) /
//! [#1925](https://github.com/rysweet/Simard/issues/1925) fixture-leak
//! cleanup tooling.
//!
//! Subcommand semantics (asymmetric by design — see spec A4):
//!   - `goal list`         — print active + backlog snapshot to stdout.
//!   - `goal unblock <id>` — unconditional override: clears `Blocked` to
//!     `NotStarted` regardless of the reason text.
//!   - `goal unblock-all`  — narrowly scoped bulk-clear: only goals
//!     whose `Blocked` reason matches the issue-#1911 brain-failure
//!     safeguard marker (`is_brain_failure_marker`). Operator-set,
//!     scope-blocked, dependency-blocked, and subordinate-blocked
//!     goals are untouched.
//!   - `goal remove <id>…` — variadic, idempotent. Persists via
//!     `save_goal_board_with_removals` so the PR #1926 merge-on-write
//!     resurrection failure mode is defeated.
//!   - `goal cleanup --placeholders` — defence-in-depth sweep that
//!     removes every active or backlog goal whose description is exactly
//!     `Goal <id>` (the placeholder pattern emitted by test fixtures).
//!
//! Persistence is cognitive memory via `launch_writer_bridge` against
//! `simard_state_root()` (honours `SIMARD_STATE_ROOT`). Audit traces are
//! emitted to stderr so operators can grep `journalctl --user -u
//! simard-ooda` after the runbook step.

use std::error::Error;

use crate::goal_curation::{
    GoalProgress, load_goal_board, save_goal_board, save_goal_board_with_removals,
    simard_state_root,
};
use crate::memory_ipc::launch_writer_bridge;
use crate::ooda_actions::advance_goal::spawn::is_brain_failure_marker;

use super::args::{next_required, reject_extra_args};

pub(super) const GOAL_HELP: &str = "\
Simard goal subcommand

Usage: simard goal <command> [args]

Commands:
  list                        Print active + backlog goal snapshot.
  unblock <goal-id>           Clear Blocked status (unconditional).
  unblock-all                 Bulk-clear brain-failure-marker blocks only.
  remove <id>...              Drop one or more goal ids (variadic, idempotent).
  cleanup --placeholders      Sweep placeholder goals (description = 'Goal <id>').
  help, -h, --help            Show this help message and exit.
";

/// Top-level `simard goal …` dispatcher. Routes to the per-verb handler
/// and surfaces missing/unknown subcommand errors with the message
/// patterns required by `tests_mod::test_goal_subcommand_*`.
pub(super) fn dispatch_goal_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn Error>> {
    let subcommand = next_required(&mut args, "goal command")?;
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{GOAL_HELP}");
            Ok(())
        }
        "list" => {
            reject_extra_args(args)?;
            handle_list()
        }
        "unblock" => {
            let goal_id = next_required(&mut args, "goal id")?;
            reject_extra_args(args)?;
            handle_unblock(&goal_id)
        }
        "unblock-all" => {
            reject_extra_args(args)?;
            handle_unblock_all()
        }
        "remove" => {
            let ids: Vec<String> = args.collect();
            handle_remove(&ids)
        }
        "cleanup" => {
            let flags: Vec<String> = args.collect();
            handle_cleanup(&flags)
        }
        other => Err(format!("unsupported command 'goal {other}'").into()),
    }
}

/// Load the persisted goal board from cognitive memory at the operator's
/// state root. Surfaces I/O / parse failures as `Err` so the CLI exits
/// non-zero; callers should not silently degrade.
fn load_board() -> Result<crate::goal_curation::GoalBoard, Box<dyn Error>> {
    let state_root = simard_state_root();
    let bridge = launch_writer_bridge(&state_root)
        .map_err(|e| format!("failed to open cognitive memory writer bridge: {e}"))?;
    let board = load_goal_board(bridge.ops())
        .map_err(|e| format!("failed to read goal board from cognitive memory: {e}"))?;
    Ok(board)
}

/// Persist the mutated board back to cognitive memory.
fn save_board(board: &crate::goal_curation::GoalBoard) -> Result<(), Box<dyn Error>> {
    let state_root = simard_state_root();
    let bridge = launch_writer_bridge(&state_root)
        .map_err(|e| format!("failed to open cognitive memory writer bridge: {e}"))?;
    save_goal_board(board, bridge.ops())
        .map_err(|e| format!("failed to persist goal board: {e}"))?;
    Ok(())
}

/// Persist the in-flight `board` with explicit removal of `ids`. Used by
/// `goal remove` and `goal cleanup --placeholders` so both routes share
/// the post-merge filter that defeats PR #1926's resurrection failure.
fn save_board_with_removals(
    board: &crate::goal_curation::GoalBoard,
    ids: &[String],
) -> Result<(), Box<dyn Error>> {
    let state_root = simard_state_root();
    let bridge = launch_writer_bridge(&state_root)
        .map_err(|e| format!("failed to open cognitive memory writer bridge: {e}"))?;
    save_goal_board_with_removals(board, ids, bridge.ops())
        .map_err(|e| format!("failed to persist goal board with removals: {e}"))?;
    Ok(())
}

fn handle_list() -> Result<(), Box<dyn Error>> {
    let board = load_board()?;
    println!(
        "active goals: {} / {}",
        board.active.len(),
        crate::goal_curation::MAX_ACTIVE_GOALS
    );
    if board.active.is_empty() {
        println!("  (none)");
    } else {
        // TSV-ish header so operators can pipe into awk / cut.
        println!("ID\tPRIORITY\tSTATUS\tASSIGNED\tDESCRIPTION");
        for g in &board.active {
            let assigned = g.assigned_to.as_deref().unwrap_or("-");
            println!(
                "{}\tp{}\t{}\t{}\t{}",
                g.id, g.priority, g.status, assigned, g.description,
            );
        }
    }
    println!("backlog: {} item(s)", board.backlog.len());
    if !board.backlog.is_empty() {
        println!("ID\tSCORE\tSOURCE\tDESCRIPTION");
        for b in &board.backlog {
            println!("{}\t{:.2}\t{}\t{}", b.id, b.score, b.source, b.description);
        }
    }
    Ok(())
}

fn handle_unblock(goal_id: &str) -> Result<(), Box<dyn Error>> {
    let mut board = load_board()?;
    let goal = board
        .active
        .iter_mut()
        .find(|g| g.id == goal_id)
        .ok_or_else(|| {
            format!("goal '{goal_id}' not found on active board (no Blocked status to clear)")
        })?;
    let prior = goal.status.clone();
    goal.status = GoalProgress::NotStarted;
    save_board(&board)?;
    eprintln!("[simard] goal unblock: '{goal_id}' restored to NotStarted (was: {prior})");
    Ok(())
}

fn handle_unblock_all() -> Result<(), Box<dyn Error>> {
    let mut board = load_board()?;
    let mut cleared = Vec::new();
    let mut left = 0usize;
    for goal in board.active.iter_mut() {
        match &goal.status {
            GoalProgress::Blocked(reason) if is_brain_failure_marker(reason) => {
                cleared.push(goal.id.clone());
                goal.status = GoalProgress::NotStarted;
            }
            GoalProgress::Blocked(_) => left += 1,
            _ => {}
        }
    }
    if !cleared.is_empty() {
        save_board(&board)?;
    }
    eprintln!(
        "[simard] goal unblock-all: cleared {} brain-failure marker(s); left {} non-marker Blocked goal(s) untouched",
        cleared.len(),
        left,
    );
    for id in &cleared {
        eprintln!("[simard] goal unblock-all: '{id}' restored to NotStarted");
    }
    Ok(())
}

/// `simard goal remove <id>…` — variadic, idempotent. Routes through
/// `save_goal_board_with_removals` so the post-merge filter defeats the
/// PR #1926 resurrection failure mode.
fn handle_remove(ids: &[String]) -> Result<(), Box<dyn Error>> {
    if ids.is_empty() {
        return Err("usage: simard goal remove <id> [<id>...]; at least one id is required".into());
    }
    let board = load_board()?;
    save_board_with_removals(&board, ids)?;
    eprintln!(
        "[simard] goal remove: requested removal of {} id(s): {}",
        ids.len(),
        ids.join(", "),
    );
    Ok(())
}

/// `simard goal cleanup --placeholders` — sweeps every active / backlog
/// goal whose description is exactly `"Goal <id>"` (the strict
/// placeholder pattern emitted by `tests_goal.rs::active_goal`). Other
/// criteria flags can be added later; the parser rejects any unknown
/// flag and requires at least one explicit criteria flag.
fn handle_cleanup(flags: &[String]) -> Result<(), Box<dyn Error>> {
    let mut placeholders = false;
    for flag in flags {
        match flag.as_str() {
            "--placeholders" => placeholders = true,
            other => {
                return Err(format!(
                    "unsupported flag '{other}' for 'goal cleanup'; valid flags: --placeholders"
                )
                .into());
            }
        }
    }
    if !placeholders {
        return Err(
            "usage: simard goal cleanup --placeholders; at least one criteria flag is required"
                .into(),
        );
    }

    let board = load_board()?;
    let mut removals: Vec<String> = Vec::new();
    for g in &board.active {
        if is_id_placeholder(&g.id, &g.description) {
            removals.push(g.id.clone());
        }
    }
    for b in &board.backlog {
        if is_id_placeholder(&b.id, &b.description) && !removals.contains(&b.id) {
            removals.push(b.id.clone());
        }
    }

    if removals.is_empty() {
        eprintln!("[simard] goal cleanup --placeholders: no placeholder goals found; no-op");
        return Ok(());
    }

    save_board_with_removals(&board, &removals)?;
    eprintln!(
        "[simard] goal cleanup --placeholders: removed {} placeholder goal(s): {}",
        removals.len(),
        removals.join(", "),
    );
    Ok(())
}

/// Strict per-id placeholder predicate: returns `true` iff `desc` is
/// exactly `"Goal <id>"`. Anchored on both ends so a production
/// description that merely *contains* the substring `Goal x` (or has the
/// wrong case like `goal x`) survives the sweep. See
/// `tests_goal_remove::goal_cleanup_placeholders_preserves_description_when_id_substring_matches`.
fn is_id_placeholder(id: &str, desc: &str) -> bool {
    let expected = format!("Goal {id}");
    desc == expected
}
