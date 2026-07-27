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
//!   - `goal remove <id>…` — variadic, idempotent. Surgically removes the ids
//!     from the authoritative store under the shared flock and writes durable
//!     tombstones (issue #1).
//!   - `goal complete <id>` — mark a goal done, remove it, and tombstone it
//!     (a standing/perpetual goal is refused and auto-reopened instead).
//!   - `goal reprioritize <id> <p>` — alias of `set-priority`.
//!   - `goal cleanup --placeholders` — defence-in-depth sweep that
//!     removes every active or backlog goal whose description is exactly
//!     `Goal <id>` (the placeholder pattern emitted by test fixtures).
//!
//! Persistence is the authoritative goal store
//! ([`crate::goal_board_store`]) — `<state_root>/state/goal_board.json`, guarded
//! by the shared `goal-board.lock` flock, atomic read-modify-write, read-your-
//! writes (issue #1). Every mutation runs under the lock so it cannot be
//! clobbered by (or clobber) a concurrent OODA daemon cycle, and is mirrored to
//! the cognitive-memory cache so the dashboard and daemon see it immediately.
//! Honours `SIMARD_STATE_ROOT`. Audit traces are emitted to stderr so operators
//! can grep `journalctl --user -u simard-ooda` after the runbook step.

use std::error::Error;

use crate::goal_curation::{GoalDecomposer, GoalProgress, labels, simard_state_root};
use crate::memory_ipc::launch_writer_client;
use crate::ooda_actions::advance_goal::spawn::is_brain_failure_marker;

use super::args::{next_required, reject_extra_args};

pub(super) const GOAL_HELP: &str = "\
Simard goal subcommand

Usage: simard goal <command> [args]

Commands:
  list [--tag <tag>]...        Print active + backlog goal snapshot. `--tag`
                               is repeatable and filters to goals carrying ALL
                               given tags (AND). A trailing LABELS column shows
                               each goal's tags.
  add <priority> [--repo <slug>] [--standing] <description>
                              Add a new active goal at given priority (1-7).
                              `--repo <slug>` routes the goal's engineer to
                              ~/src/<slug> (default: the daemon's own repo).
                              `--standing` marks the goal standing/perpetual —
                              it never completes or tombstones and rolls to a
                              fresh cycle when a unit of work finishes.
  complete <goal-id>          Mark a goal done, remove it, and tombstone it so
                              nothing re-seeds it (idempotent). A standing goal
                              is refused and auto-reopened for a fresh cycle.
  reprioritize <goal-id> <p>  Change an active goal's priority (alias of
                              set-priority).
  demote <goal-id>            Move an active goal to the backlog.
  set-priority <goal-id> <p>  Change an active goal's priority.
  unblock <goal-id>           Clear Blocked status (unconditional).
  set-done-gate <goal-id> [--pr <n>] [--issue <n>] [--criteria <text>]
                              Bind a machine-checkable finish line to a goal so
                              the completion gate can certify it automatically.
                              --pr/--issue link a PR (checked MERGED) and/or an
                              issue (checked CLOSED); --criteria replaces the
                              plain-English finish line. Requires at least one of
                              --pr/--issue. Clears the no-progress breaker and
                              restores the goal to NotStarted.
  unblock-all                 Bulk-clear brain-failure-marker blocks only.
  remove <id>...              Drop one or more goal ids (variadic, idempotent).
  decompose <goal-id> [--max-children <N>] [--dry-run]
                              Break a large goal into 2-6 linked sub-goals
                              (writes parent->child edges into the graph).
                              --max-children caps the fan-out (clamped to 2-6);
                              --dry-run prints the proposed sub-goals without
                              writing anything.
  cleanup --placeholders      Sweep placeholder goals (description = 'Goal <id>').
  label <goal-id> add <tag>   Add a free-form tag to a goal (idempotent).
  label <goal-id> remove <tag>  Remove a tag from a goal (no-op if absent).
  label <goal-id> list        Print a goal's tags, one per line ('(none)' if bare).
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
            let tags = parse_list_tags(args)?;
            handle_list(&tags)
        }
        "unblock" => {
            let goal_id = next_required(&mut args, "goal id")?;
            reject_extra_args(args)?;
            handle_unblock(&goal_id)
        }
        "set-done-gate" => {
            let goal_id = next_required(&mut args, "goal id")?;
            let flags: Vec<String> = args.collect();
            handle_set_done_gate(&goal_id, &flags)
        }
        "unblock-all" => {
            reject_extra_args(args)?;
            handle_unblock_all()
        }
        "add" => {
            let priority_str = next_required(&mut args, "priority (1-7)")?;
            let rest: Vec<String> = args.collect();
            let (repo, standing, desc_tokens) = extract_add_flags(rest)?;
            let description = desc_tokens.join(" ");
            if description.is_empty() {
                return Err(
                    "usage: simard goal add <priority> [--repo <slug>] [--standing] <description>"
                        .into(),
                );
            }
            handle_add(&priority_str, &description, repo.as_deref(), standing)
        }
        "demote" => {
            let goal_id = next_required(&mut args, "goal id")?;
            reject_extra_args(args)?;
            handle_demote(&goal_id)
        }
        "set-priority" => {
            let goal_id = next_required(&mut args, "goal id")?;
            let priority_str = next_required(&mut args, "priority (1-7)")?;
            reject_extra_args(args)?;
            handle_set_priority(&goal_id, &priority_str)
        }
        "reprioritize" => {
            let goal_id = next_required(&mut args, "goal id")?;
            let priority_str = next_required(&mut args, "priority (1-7)")?;
            reject_extra_args(args)?;
            handle_set_priority(&goal_id, &priority_str)
        }
        "complete" => {
            let goal_id = next_required(&mut args, "goal id")?;
            reject_extra_args(args)?;
            handle_complete(&goal_id)
        }
        "remove" => {
            let ids: Vec<String> = args.collect();
            handle_remove(&ids)
        }
        "decompose" => {
            let goal_id = next_required(&mut args, "goal id")?;
            let flags: Vec<String> = args.collect();
            handle_decompose(&goal_id, &flags)
        }
        "cleanup" => {
            let flags: Vec<String> = args.collect();
            handle_cleanup(&flags)
        }
        "label" => {
            let goal_id = next_required(&mut args, "goal id")?;
            let sub = next_required(&mut args, "label subcommand (add|remove|list)")?;
            handle_label(&goal_id, &sub, args)
        }
        other => Err(format!("unsupported command 'goal {other}'").into()),
    }
}

/// Load the authoritative goal board (issue #1).
///
/// Reads `<state_root>/state/goal_board.json` (the single durable, flock-guarded
/// source of truth) with read-your-writes semantics, migrating the legacy
/// cognitive-memory snapshot into it on first use so a fresh CLI still sees the
/// live board. Surfaces I/O / parse failures as `Err` so the CLI exits non-zero.
fn load_board() -> Result<crate::goal_curation::GoalBoard, Box<dyn Error>> {
    let state_root = simard_state_root();
    let memory = launch_writer_client(&state_root)
        .map_err(|e| format!("failed to open cognitive memory writer memory: {e}"))?;
    let persistent = crate::goal_board_store::load_or_migrate(&state_root, memory.ops())
        .map_err(|e| format!("failed to load authoritative goal store: {e}"))?;
    Ok(persistent.board)
}

/// Atomically apply `f` to the authoritative goal board **under the shared store
/// flock**, then mirror the committed board to the cognitive-memory cache so the
/// dashboard and the running OODA daemon observe the change immediately.
///
/// This is the anti-clobber write path (issue #1): the whole read-modify-write
/// runs while the cross-process lock is held, so an operator mutation cannot be
/// lost to (or lose) a concurrent daemon cycle flush. `f` must **validate before
/// mutating**: on `Err` the board is restored to its pre-image and the error is
/// surfaced, so a rejected command (unknown id, board at capacity) never leaves
/// a partial write.
fn with_board<R>(
    f: impl FnOnce(&mut crate::goal_curation::GoalBoard) -> Result<R, Box<dyn Error>>,
) -> Result<R, Box<dyn Error>> {
    let state_root = simard_state_root();
    let memory = launch_writer_client(&state_root)
        .map_err(|e| format!("failed to open cognitive memory writer memory: {e}"))?;
    crate::goal_board_store::load_or_migrate(&state_root, memory.ops())
        .map_err(|e| format!("failed to load authoritative goal store: {e}"))?;
    let out = crate::goal_board_store::mutate(&state_root, move |s| {
        let snapshot = s.board.clone();
        match f(&mut s.board) {
            Ok(r) => Ok(r),
            Err(e) => {
                s.board = snapshot;
                Err(e)
            }
        }
    })
    .map_err(|e| -> Box<dyn Error> {
        format!("failed to persist authoritative goal store: {e}").into()
    })??;
    let committed = crate::goal_board_store::load(&state_root).board;
    if let Err(e) = crate::goal_curation::overwrite_memory_cache(&committed, memory.ops()) {
        eprintln!("[simard] goal: warning: memory cache refresh failed: {e}");
    }
    Ok(out)
}

/// Atomically apply `f` to the whole authoritative [`PersistentGoalState`] —
/// board **and** the no-progress breaker counters — under the shared store
/// flock, then mirror the committed board to the cognitive-memory cache.
///
/// Identical anti-clobber semantics to [`with_board`], but `f` receives the full
/// persistent state so a mutation can both edit a goal and reset its breaker
/// bookkeeping (`no_progress`) in one atomic window. On `Err`, the pre-image is
/// restored and the error surfaced so a rejected command never leaves a partial
/// write.
fn with_state<R>(
    f: impl FnOnce(&mut crate::goal_board_store::PersistentGoalState) -> Result<R, Box<dyn Error>>,
) -> Result<R, Box<dyn Error>> {
    let state_root = simard_state_root();
    let memory = launch_writer_client(&state_root)
        .map_err(|e| format!("failed to open cognitive memory writer memory: {e}"))?;
    crate::goal_board_store::load_or_migrate(&state_root, memory.ops())
        .map_err(|e| format!("failed to load authoritative goal store: {e}"))?;
    let out = crate::goal_board_store::mutate(&state_root, move |s| {
        let snapshot = s.clone();
        match f(s) {
            Ok(r) => Ok(r),
            Err(e) => {
                *s = snapshot;
                Err(e)
            }
        }
    })
    .map_err(|e| -> Box<dyn Error> {
        format!("failed to persist authoritative goal store: {e}").into()
    })??;
    let committed = crate::goal_board_store::load(&state_root).board;
    if let Err(e) = crate::goal_curation::overwrite_memory_cache(&committed, memory.ops()) {
        eprintln!("[simard] goal: warning: memory cache refresh failed: {e}");
    }
    Ok(out)
}

/// Blind-overwrite the authoritative board with `board` under the store lock,
/// then refresh the memory cache. Used only by `goal decompose`, where the
/// mutated board (parent placement + new child goals) IS the operator's explicit
/// intent, so a surgical merge would fight the demotion the operator asked for.
fn commit_board_blind(board: &crate::goal_curation::GoalBoard) -> Result<(), Box<dyn Error>> {
    let state_root = simard_state_root();
    let b = board.clone();
    crate::goal_board_store::mutate(&state_root, move |s| {
        s.board = b;
    })
    .map_err(|e| format!("failed to persist goal board: {e}"))?;
    let memory = launch_writer_client(&state_root)
        .map_err(|e| format!("failed to open cognitive memory writer memory: {e}"))?;
    if let Err(e) = crate::goal_curation::overwrite_memory_cache(board, memory.ops()) {
        eprintln!("[simard] goal: warning: memory cache refresh failed: {e}");
    }
    Ok(())
}

/// Record `ids` as durable tombstones so no path — default seeding, memory
/// recall, a meeting handoff, or the daemon's cycle reconcile — can resurrect a
/// removed or completed goal (issue #1, requirement 3).
fn tombstone(ids: &[String]) -> Result<(), Box<dyn Error>> {
    let state_root = simard_state_root();
    crate::ooda_loop::tombstone_goals(&state_root, ids)
        .map_err(|e| format!("failed to record tombstones: {e}"))?;
    Ok(())
}

/// Parse the repeatable `--tag <tag>` / `--tag=<tag>` flags for `goal list`.
/// Each tag is trimmed (empty-after-trim rejected). Any other token is a usage
/// error. Repeated tags combine with AND at filter time.
fn parse_list_tags(args: impl Iterator<Item = String>) -> Result<Vec<String>, Box<dyn Error>> {
    let mut tags = Vec::new();
    let mut iter = args;
    while let Some(tok) = iter.next() {
        let raw = if tok == "--tag" {
            iter.next()
                .ok_or_else(|| -> Box<dyn Error> { "usage: --tag requires a <tag>".into() })?
        } else if let Some(v) = tok.strip_prefix("--tag=") {
            v.to_string()
        } else {
            return Err(format!(
                "unexpected argument '{tok}' (usage: simard goal list [--tag <tag>]...)"
            )
            .into());
        };
        let tag = labels::validate_tag(&raw)?;
        tags.push(tag);
    }
    Ok(tags)
}

/// Pure formatter for the active-goals section of `goal list`, filtered to
/// goals carrying ALL of `tags` (AND; empty `tags` matches everything). Returns
/// the lines to print — a count line (annotated `(filtered by tag)` when a
/// filter is active), then a TSV header with a trailing `LABELS` column and one
/// row per goal. Kept pure so it is unit-testable without a live state root.
fn format_active_goal_lines(
    board: &crate::goal_curation::GoalBoard,
    tags: &[String],
) -> Vec<String> {
    let filtered: Vec<&crate::goal_curation::ActiveGoal> = board
        .active
        .iter()
        .filter(|g| labels::matches_all_tags(&g.labels, tags))
        .collect();
    let note = if tags.is_empty() {
        ""
    } else {
        " (filtered by tag)"
    };
    let mut out = vec![format!(
        "active goals: {} / {}{}",
        filtered.len(),
        crate::goal_curation::MAX_ACTIVE_GOALS,
        note,
    )];
    if filtered.is_empty() {
        out.push("  (none)".to_string());
    } else {
        // TSV-ish header so operators can pipe into awk / cut. `LABELS` is
        // appended AFTER the existing columns, so scripts that read the first
        // five fields keep working.
        out.push("ID\tPRIORITY\tSTATUS\tASSIGNED\tDESCRIPTION\tLABELS".to_string());
        for g in &filtered {
            let assigned = g.assigned_to.as_deref().unwrap_or("-");
            out.push(format!(
                "{}\tp{}\t{}\t{}\t{}\t{}",
                g.id,
                g.priority,
                g.status,
                assigned,
                g.description,
                g.labels.join(","),
            ));
        }
    }
    out
}

/// Pure formatter for `goal label <id> list`: one tag per line, or `(none)`.
fn format_label_list(goal_labels: &[String]) -> Vec<String> {
    if goal_labels.is_empty() {
        vec!["(none)".to_string()]
    } else {
        goal_labels.to_vec()
    }
}

fn handle_list(tags: &[String]) -> Result<(), Box<dyn Error>> {
    let board = load_board()?;
    for line in format_active_goal_lines(&board, tags) {
        println!("{line}");
    }
    // A tag filter is an active-goal query; the backlog carries no labels, so
    // suppress it when filtering to keep the filtered view focused.
    if tags.is_empty() {
        println!("backlog: {} item(s)", board.backlog.len());
        if !board.backlog.is_empty() {
            println!("ID\tSCORE\tSOURCE\tDESCRIPTION");
            for b in &board.backlog {
                println!("{}\t{:.2}\t{}\t{}", b.id, b.score, b.source, b.description);
            }
        }
    }
    Ok(())
}

/// `simard goal label <goal-id> <add|remove|list> [<tag>]` — deterministic
/// label CRUD on an active goal. Mutations persist through the same
/// flock-guarded read-modify-write path as `goal add`/`remove`.
fn handle_label(
    goal_id: &str,
    sub: &str,
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn Error>> {
    match sub {
        "add" => {
            let tag = next_required(&mut args, "tag")?;
            reject_extra_args(args)?;
            handle_label_add(goal_id, &tag)
        }
        "remove" => {
            let tag = next_required(&mut args, "tag")?;
            reject_extra_args(args)?;
            handle_label_remove(goal_id, &tag)
        }
        "list" => {
            reject_extra_args(args)?;
            handle_label_list(goal_id)
        }
        other => Err(format!(
            "unsupported label subcommand '{other}' (expected: add, remove, list)"
        )
        .into()),
    }
}

fn handle_label_add(goal_id: &str, raw_tag: &str) -> Result<(), Box<dyn Error>> {
    let tag = labels::validate_tag(raw_tag)?;
    if labels::is_source_tag(&tag) {
        return Err(format!(
            "tag '{tag}' is in the reserved 'source:*' provenance namespace, \
             which is stamped automatically at goal creation and cannot be added by hand"
        )
        .into());
    }
    let added = with_board(|board| {
        let goal = board
            .active
            .iter_mut()
            .find(|g| g.id == goal_id)
            .ok_or_else(|| -> Box<dyn Error> {
                format!("goal '{goal_id}' not found on active board").into()
            })?;
        Ok(labels::add_label(&mut goal.labels, &tag))
    })?;
    if added {
        eprintln!("[simard] goal label: added '{tag}' to '{goal_id}'");
    } else {
        eprintln!("[simard] goal label: '{goal_id}' already has '{tag}' (no-op)");
    }
    Ok(())
}

fn handle_label_remove(goal_id: &str, raw_tag: &str) -> Result<(), Box<dyn Error>> {
    let tag = labels::validate_tag(raw_tag)?;
    if labels::is_source_tag(&tag) {
        return Err(format!(
            "tag '{tag}' is a code-managed 'source:*' provenance label and cannot be removed by hand"
        )
        .into());
    }
    let removed = with_board(|board| {
        let goal = board
            .active
            .iter_mut()
            .find(|g| g.id == goal_id)
            .ok_or_else(|| -> Box<dyn Error> {
                format!("goal '{goal_id}' not found on active board").into()
            })?;
        Ok(labels::remove_label(&mut goal.labels, &tag))
    })?;
    if removed {
        eprintln!("[simard] goal label: removed '{tag}' from '{goal_id}'");
    } else {
        eprintln!("[simard] goal label: '{goal_id}' has no '{tag}' (no-op)");
    }
    Ok(())
}

fn handle_label_list(goal_id: &str) -> Result<(), Box<dyn Error>> {
    let board = load_board()?;
    let goal = board
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .ok_or_else(|| -> Box<dyn Error> {
            format!("goal '{goal_id}' not found on active board").into()
        })?;
    for line in format_label_list(&goal.labels) {
        println!("{line}");
    }
    Ok(())
}

fn handle_unblock(goal_id: &str) -> Result<(), Box<dyn Error>> {
    let prior = with_board(|board| {
        let goal = board
            .active
            .iter_mut()
            .find(|g| g.id == goal_id)
            .ok_or_else(|| -> Box<dyn Error> {
                format!("goal '{goal_id}' not found on active board (no Blocked status to clear)")
                    .into()
            })?;
        let prior = goal.status.clone();
        goal.status = GoalProgress::NotStarted;
        Ok(prior)
    })?;
    eprintln!("[simard] goal unblock: '{goal_id}' restored to NotStarted (was: {prior})");
    Ok(())
}

/// Parsed `goal set-done-gate` flags.
struct DoneGateFlags {
    pr: Option<String>,
    issue: Option<String>,
    criteria: Option<String>,
}

/// Parse `[--pr <n>] [--issue <n>] [--criteria <text…>]`. `--criteria` is
/// greedy: it consumes the remainder of the argument list as the finish-line
/// text so an operator can pass an unquoted multi-word criterion.
fn parse_done_gate_flags(flags: &[String]) -> Result<DoneGateFlags, Box<dyn Error>> {
    let mut pr = None;
    let mut issue = None;
    let mut criteria = None;
    let mut i = 0;
    while i < flags.len() {
        match flags[i].as_str() {
            "--pr" => {
                let v = flags
                    .get(i + 1)
                    .ok_or_else(|| -> Box<dyn Error> { "--pr requires a PR number".into() })?;
                pr = Some(parse_ref_number("--pr", v)?);
                i += 2;
            }
            "--issue" => {
                let v = flags.get(i + 1).ok_or_else(|| -> Box<dyn Error> {
                    "--issue requires an issue number".into()
                })?;
                issue = Some(parse_ref_number("--issue", v)?);
                i += 2;
            }
            "--criteria" => {
                let text = flags[i + 1..].join(" ");
                if text.trim().is_empty() {
                    return Err("--criteria requires a non-empty finish-line description".into());
                }
                criteria = Some(text.trim().to_string());
                i = flags.len();
            }
            other => {
                return Err(format!(
                    "unknown flag '{other}' (expected --pr, --issue, or --criteria)"
                )
                .into());
            }
        }
    }
    Ok(DoneGateFlags {
        pr,
        issue,
        criteria,
    })
}

/// Normalise a PR/issue reference to a bare positive integer string, accepting a
/// leading `#`. The completion gate resolves state via `gh <kind> view <num>`,
/// so a non-numeric token would silently never certify.
fn parse_ref_number(flag: &str, raw: &str) -> Result<String, Box<dyn Error>> {
    let trimmed = raw.trim().trim_start_matches('#');
    if trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("{flag} expects a numeric reference, got '{raw}'").into());
    }
    Ok(trimmed.to_string())
}

/// `simard goal set-done-gate <id> [--pr n] [--issue n] [--criteria text]` —
/// bind a machine-checkable finish line to a goal so the completion gate
/// (`src/goal_curation/completion_gate.rs`) can certify it automatically. A goal
/// whose done-criteria lived only in prose has no PR/issue the gate can observe,
/// so the no-progress breaker parks it as `UNCLEAR-CRITERIA`; this command is the
/// supported operator remedy — it links the PR (checked MERGED) and/or issue
/// (checked CLOSED), rewrites the plain-English finish line, resets the breaker,
/// and restores the goal to `NotStarted` so work resumes toward a checkable end.
///
/// The edit is made **durable** against the daemon's in-flight-wins reconcile by
/// recording a [`crate::goal_board_store::DoneGatePin`]: the daemon re-asserts
/// the pinned anchor + finish line every cycle instead of clobbering the goal
/// back to unmeasurable prose (the failure mode that made prior manual edits
/// evaporate within a cycle).
fn handle_set_done_gate(goal_id: &str, flags: &[String]) -> Result<(), Box<dyn Error>> {
    let parsed = parse_done_gate_flags(flags)?;
    if parsed.pr.is_none() && parsed.issue.is_none() {
        return Err(
            "set-done-gate requires at least one of --pr <n> or --issue <n> (the anchor the \
             completion gate measures)"
                .into(),
        );
    }
    let pin = crate::goal_board_store::DoneGatePin {
        pr: parsed.pr.clone(),
        issue: parsed.issue.clone(),
        criteria: parsed.criteria.clone(),
    };
    let anchor = pin.anchor();
    let goal_id_owned = goal_id.to_string();
    let pin_for_board = pin.clone();
    with_state(move |s| {
        let goal = s
            .board
            .active
            .iter_mut()
            .find(|g| g.id == goal_id_owned)
            .ok_or_else(|| -> Box<dyn Error> {
                format!("goal '{goal_id_owned}' not found on the active board").into()
            })?;

        // Bind the measurable anchor(s) and rewrite the plain-English finish
        // line via the shared pin logic (identical to the daemon's re-assert).
        pin_for_board.apply_to(goal);

        goal.status = GoalProgress::NotStarted;
        goal.current_activity = Some(format!(
            "done-gate pinned by operator: {}",
            pin_for_board.anchor()
        ));

        // The goal now has a checkable finish line — treat that as concrete
        // progress so the no-progress breaker forgets its prior no-action count,
        // spent guided-retry, and re-investigation bookkeeping for this goal.
        s.no_progress.record_progress(&goal_id_owned);

        Ok(())
    })?;

    // Record the durable pin so the daemon's per-cycle reconcile re-asserts the
    // finish line instead of reverting it. A pin-write failure is non-fatal: the
    // board edit already landed; we only warn that it may not survive a cycle.
    let state_root = simard_state_root();
    if let Err(e) = crate::goal_board_store::record_done_gate_pin(&state_root, goal_id, pin) {
        eprintln!(
            "[simard] goal set-done-gate: warning: durable pin not recorded (edit applied but \
             the daemon may revert it next cycle): {e}"
        );
    }

    eprintln!(
        "[simard] goal set-done-gate: '{goal_id}' pinned — {anchor}; breaker reset; status \
         NotStarted"
    );
    Ok(())
}

fn handle_unblock_all() -> Result<(), Box<dyn Error>> {
    let (cleared, left) = with_board(|board| {
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
        Ok((cleared, left))
    })?;
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

/// Parsed `goal add` flags: `(repo slug, is-standing, description tokens)`.
type AddFlags = (Option<String>, bool, Vec<String>);

/// `simard goal add <priority> [--standing] <description>` — add a new active
/// goal. `--standing` marks the goal as standing/perpetual (issue #2580): it
/// has no terminal done-state, is never tombstoned, and rolls to a fresh cycle
/// when a unit of work finishes.
fn handle_add(
    priority_str: &str,
    description: &str,
    repo: Option<&str>,
    standing: bool,
) -> Result<(), Box<dyn Error>> {
    let priority: u32 = priority_str
        .parse()
        .map_err(|_| format!("invalid priority '{priority_str}': must be 1-7"))?;
    if priority == 0 || priority > 7 {
        return Err(format!("priority must be 1-7, got {priority}").into());
    }
    // Issue #2359 (BUG 1): validate the target-repo slug at ingress (shape
    // only — existence is checked later by `resolve_goal_repo` at spawn time).
    if let Some(slug) = repo {
        crate::ooda_actions::advance_goal::repo_resolver::validate_repo_slug(slug)
            .map_err(|e| -> Box<dyn Error> { e.to_string().into() })?;
    }
    // Slug from the operator's original text so the id stays clean; the stored
    // description carries the durable standing marker when `--standing` is set.
    let id = crate::goals::goal_slug(description);
    let stored_description = if standing {
        format!(
            "{}{description}",
            crate::goal_curation::STANDING_MARKER_PREFIX
        )
    } else {
        description.to_string()
    };
    let repo_owned = repo.map(str::to_string);
    {
        let id = id.clone();
        with_board(move |board| {
            if board.active.iter().any(|g| g.id == id) {
                return Err(format!("goal '{id}' is already active").into());
            }
            if board.active.len() >= crate::goal_curation::MAX_ACTIVE_GOALS {
                return Err(format!(
                    "board is at capacity ({}); demote or remove a goal first",
                    crate::goal_curation::MAX_ACTIVE_GOALS
                )
                .into());
            }
            board.active.push(crate::goal_curation::ActiveGoal {
                parent_goal_id: None,
                priority_explicit: false,
                id,
                description: stored_description,
                priority,
                status: GoalProgress::NotStarted,
                assigned_to: None,
                repo: repo_owned,
                current_activity: None,
                wip_refs: vec![],
                last_progress_update_at: None,
                labels: vec![crate::goal_curation::labels::SOURCE_OPERATOR.to_string()],
            });
            Ok(())
        })?;
    }
    let repo_note = repo
        .map(|r| format!(" -> repo '{r}'"))
        .unwrap_or_else(|| " -> repo Simard (daemon)".to_string());
    let standing_note = if standing {
        " [standing/perpetual]"
    } else {
        ""
    };
    eprintln!("[simard] goal add: '{id}' added at p{priority}{repo_note}{standing_note}");
    Ok(())
}

/// Extract the optional `--repo <slug>` / `--repo=<slug>` and `--standing`
/// flags from the trailing `goal add` tokens, returning `(repo, standing,
/// description_tokens)`. Flags may appear in any order relative to the
/// description; remaining tokens form the goal description.
fn extract_add_flags(tokens: Vec<String>) -> Result<AddFlags, Box<dyn Error>> {
    let mut repo: Option<String> = None;
    let mut standing = false;
    let mut rest: Vec<String> = Vec::with_capacity(tokens.len());
    let mut iter = tokens.into_iter();
    while let Some(tok) = iter.next() {
        if tok == "--repo" {
            let slug = iter
                .next()
                .ok_or_else(|| -> Box<dyn Error> { "usage: --repo requires a <slug>".into() })?;
            repo = Some(slug);
        } else if let Some(slug) = tok.strip_prefix("--repo=") {
            repo = Some(slug.to_string());
        } else if tok == "--standing" || tok == "--perpetual" {
            standing = true;
        } else {
            rest.push(tok);
        }
    }
    Ok((repo, standing, rest))
}

/// `simard goal demote <goal-id>` — move an active goal to the backlog.
fn handle_demote(goal_id: &str) -> Result<(), Box<dyn Error>> {
    with_board(|board| {
        let position = board
            .active
            .iter()
            .position(|g| g.id == goal_id)
            .ok_or_else(|| -> Box<dyn Error> {
                format!("goal '{goal_id}' not found on active board").into()
            })?;
        let goal = board.active.remove(position);
        board.backlog.push(crate::goal_curation::BacklogItem {
            id: goal.id.clone(),
            description: goal.description,
            source: "operator:demote".to_string(),
            score: 0.5,
        });
        Ok(())
    })?;
    eprintln!("[simard] goal demote: '{goal_id}' moved to backlog");
    Ok(())
}

/// `simard goal set-priority <goal-id> <priority>` — change priority. Also the
/// implementation behind `simard goal reprioritize <goal-id> <priority>`.
fn handle_set_priority(goal_id: &str, priority_str: &str) -> Result<(), Box<dyn Error>> {
    let priority: u32 = priority_str
        .parse()
        .map_err(|_| format!("invalid priority '{priority_str}': must be 1-7"))?;
    if priority == 0 || priority > 7 {
        return Err(format!("priority must be 1-7, got {priority}").into());
    }
    let old = with_board(|board| {
        let goal = board
            .active
            .iter_mut()
            .find(|g| g.id == goal_id)
            .ok_or_else(|| -> Box<dyn Error> {
                format!("goal '{goal_id}' not found on active board").into()
            })?;
        let old = goal.priority;
        goal.priority = priority;
        // Issue #2695 follow-up: an operator explicitly setting a priority is the
        // one path that marks it operator-set provenance, so the prioritization
        // pass leaves this exact value intact instead of differentiating it away.
        goal.priority_explicit = true;
        Ok(old)
    })?;
    eprintln!("[simard] goal set-priority: '{goal_id}' changed from p{old} to p{priority}");
    Ok(())
}

/// `simard goal complete <goal-id>` — mark a goal done, remove it from the
/// board, and write a durable tombstone so no path (default seeding, memory
/// recall, a meeting handoff, or the daemon's cycle reconcile) can resurrect it.
/// Idempotent: completing an absent goal still records the tombstone.
///
/// Standing/perpetual goals (issue #2580) are the exception: `complete` refuses
/// to terminate one and instead **auto-reopens** it for a fresh cycle — no
/// removal, no tombstone — because a standing goal has no terminal done-state.
fn handle_complete(goal_id: &str) -> Result<(), Box<dyn Error>> {
    enum CompleteOutcome {
        /// A standing goal — reopened for a fresh cycle instead of terminating.
        Reopened,
        /// A normal goal that existed and was removed.
        Completed,
        /// No matching goal on the board (still recorded as a tombstone).
        Absent,
    }

    let outcome = with_board(|board| {
        if let Some(goal) = board.active.iter_mut().find(|g| g.id == goal_id)
            && goal.is_perpetual()
        {
            goal.roll_to_new_cycle();
            return Ok(CompleteOutcome::Reopened);
        }
        let before = board.active.len() + board.backlog.len();
        board.active.retain(|g| g.id != goal_id);
        board.backlog.retain(|b| b.id != goal_id);
        let existed = before != board.active.len() + board.backlog.len();
        Ok(if existed {
            CompleteOutcome::Completed
        } else {
            CompleteOutcome::Absent
        })
    })?;

    match outcome {
        CompleteOutcome::Reopened => {
            eprintln!(
                "[simard] goal complete: '{goal_id}' is a standing goal — refused to terminate; \
                 reopened it for a fresh cycle (no tombstone)"
            );
        }
        CompleteOutcome::Completed => {
            tombstone(&[goal_id.to_string()])?;
            eprintln!(
                "[simard] goal complete: '{goal_id}' marked done, removed from board, and tombstoned"
            );
        }
        CompleteOutcome::Absent => {
            tombstone(&[goal_id.to_string()])?;
            eprintln!(
                "[simard] goal complete: '{goal_id}' not on board; recorded tombstone (idempotent)"
            );
        }
    }
    Ok(())
}

/// `simard goal remove <id>…` — variadic, idempotent. Surgically removes the
/// ids from the authoritative store under the shared flock and writes durable
/// tombstones so the daemon's cycle reconcile (and memory recall / meeting
/// handoffs) can never resurrect them (issue #1).
fn handle_remove(ids: &[String]) -> Result<(), Box<dyn Error>> {
    if ids.is_empty() {
        return Err("usage: simard goal remove <id> [<id>...]; at least one id is required".into());
    }
    with_board(|board| {
        let removals: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        board.active.retain(|g| !removals.contains(g.id.as_str()));
        board.backlog.retain(|b| !removals.contains(b.id.as_str()));
        Ok(())
    })?;
    // Record tombstones so nothing (default seeding, memory recall, meeting
    // handoffs, or the daemon's cycle reconcile) re-ingests these goals.
    tombstone(ids)?;
    eprintln!(
        "[simard] goal remove: requested removal of {} id(s): {}",
        ids.len(),
        ids.join(", "),
    );
    Ok(())
}

/// `simard goal decompose <goal-id> [--max-children <N>] [--dry-run]` — break a
/// large active goal into 2-6 bounded sub-goals (issue #2405). Routes through
/// the same cognitive-memory **writer memory** as `goal add` / `goal remove`,
/// so the write is serialized by the daemon when one is running and takes the
/// local writer lock otherwise. The parent->child `decomposes_into` edges are
/// written into the graph (and are queryable back), then the mutated board is
/// persisted. `--dry-run` prints the proposed sub-goals without writing.
fn handle_decompose(goal_id: &str, flags: &[String]) -> Result<(), Box<dyn Error>> {
    crate::engineer_worktree::validate_goal_id(goal_id)
        .map_err(|e| -> Box<dyn Error> { format!("invalid goal id '{goal_id}': {e}").into() })?;

    let mut max_children = crate::goal_curation::MAX_SUBGOALS;
    let mut dry_run = false;
    let mut iter = flags.iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--dry-run" => dry_run = true,
            "--max-children" => {
                let n = iter.next().ok_or_else(|| -> Box<dyn Error> {
                    "usage: --max-children requires a number".into()
                })?;
                max_children = parse_max_children(n)?;
            }
            other if other.starts_with("--max-children=") => {
                let n = other.trim_start_matches("--max-children=");
                max_children = parse_max_children(n)?;
            }
            other => {
                return Err(format!(
                    "unsupported flag '{other}' for goal decompose (expected --max-children <N> or --dry-run)"
                )
                .into());
            }
        }
    }

    let state_root = simard_state_root();
    let memory = launch_writer_client(&state_root)
        .map_err(|e| format!("failed to open cognitive memory writer memory: {e}"))?;
    let ops = memory.ops();

    // Load from the authoritative store (issue #1); the memory memory is still
    // used below for the durable graph-edge writes performed by decompose_goal.
    let mut board = load_board()?;
    let parent = board
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .cloned()
        .ok_or_else(|| -> Box<dyn Error> {
            format!("goal '{goal_id}' not found on active board; cannot decompose").into()
        })?;

    let repo_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let decomposer = crate::goal_curation::RecipeGoalDecomposer::new(&repo_root).ok_or_else(
        || -> Box<dyn Error> {
            "decomposition is unavailable: recipe-runner-rs and the goal-decomposition recipe must \
             be installed (or run decomposition via the OODA daemon)"
                .into()
        },
    )?;

    if dry_run {
        let proposals = decomposer
            .propose_subgoals(&parent, max_children)
            .map_err(|e| format!("decomposition failed: {e}"))?;
        eprintln!(
            "[simard] goal decompose --dry-run: '{goal_id}' would produce {} sub-goal(s) (clamped to 2-6 on apply); nothing written:",
            proposals.len()
        );
        for (i, p) in proposals.iter().enumerate() {
            eprintln!(
                "{}",
                render_dry_run_proposal(i + 1, &p.description, &p.done_criterion)
            );
        }
        return Ok(());
    }

    let outcome =
        crate::goal_curation::decompose_goal(ops, &mut board, goal_id, &decomposer, max_children)
            .map_err(|e| format!("decomposition failed: {e}"))?;

    // Persist the decomposition (parent placement + new child goals) to the
    // authoritative store (issue #1) so it sticks across daemon cycles. The
    // mutated `board` IS the operator's explicit intent, so a blind commit is
    // correct here (a surgical merge would fight the parent demotion).
    commit_board_blind(&board)
        .map_err(|e| format!("failed to persist decomposed goal board: {e}"))?;

    eprintln!(
        "[simard] goal decompose: '{}' -> {} child goal(s) [{:?}]: {}",
        outcome.parent_id,
        outcome.child_ids.len(),
        outcome.placement,
        outcome.child_ids.join(", "),
    );
    Ok(())
}

/// Render one `--dry-run` preview line for a proposed sub-goal.
///
/// `description` / `done_criterion` are **untrusted** LLM-authored text (issue
/// [#2405](https://github.com/rysweet/Simard/issues/2405) review finding F1):
/// the apply path only ever echoes charset-validated ids, but the dry-run
/// preview is the one place raw model output reaches the operator's terminal.
/// Sanitize it first — strip terminal control/escape sequences and redact
/// secret-shaped lines via [`crate::sanitization::sanitize_terminal_text`] —
/// then fold any residual newlines/tabs to spaces so a single proposal cannot
/// spoof extra numbered rows in the preview.
fn render_dry_run_proposal(index: usize, description: &str, done_criterion: &str) -> String {
    let clean =
        |raw: &str| crate::sanitization::sanitize_terminal_text(raw).replace(['\n', '\t'], " ");
    format!(
        "  {index}. {} (done: {})",
        clean(description),
        clean(done_criterion)
    )
}

/// Parse and validate the `--max-children` value (must be a positive integer;
/// the decompose driver clamps it into `[2, 6]`).
fn parse_max_children(raw: &str) -> Result<usize, Box<dyn Error>> {
    let n: usize = raw.parse().map_err(|_| -> Box<dyn Error> {
        format!("invalid --max-children '{raw}': must be a non-negative integer").into()
    })?;
    Ok(n)
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

    let removals = with_board(|board| {
        let mut removals: Vec<String> = Vec::new();
        board.active.retain(|g| {
            if is_id_placeholder(&g.id, &g.description) {
                removals.push(g.id.clone());
                false
            } else {
                true
            }
        });
        board.backlog.retain(|b| {
            if is_id_placeholder(&b.id, &b.description) {
                if !removals.contains(&b.id) {
                    removals.push(b.id.clone());
                }
                false
            } else {
                true
            }
        });
        Ok(removals)
    })?;

    if removals.is_empty() {
        eprintln!("[simard] goal cleanup --placeholders: no placeholder goals found; no-op");
        return Ok(());
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- label formatting + tag parsing (issue #2743) ---------------------

    fn goal_with(id: &str, labels: &[&str]) -> crate::goal_curation::ActiveGoal {
        crate::goal_curation::ActiveGoal::new(id, format!("desc {id}"), 1)
            .with_labels(labels.iter().map(|s| (*s).to_string()).collect())
    }

    #[test]
    fn parse_list_tags_collects_repeatable_flags() {
        let args = ["--tag", "source:creative-ideas", "--tag=area:dashboard"]
            .into_iter()
            .map(String::from);
        let tags = parse_list_tags(args).expect("parse");
        assert_eq!(tags, vec!["source:creative-ideas", "area:dashboard"]);
    }

    #[test]
    fn parse_list_tags_rejects_empty_and_stray_tokens() {
        // Empty-after-trim tag is rejected.
        assert!(parse_list_tags(["--tag", "   "].into_iter().map(String::from)).is_err());
        // A bare positional is a usage error.
        assert!(parse_list_tags(["oops"].into_iter().map(String::from)).is_err());
        // --tag without a value is an error.
        assert!(parse_list_tags(["--tag"].into_iter().map(String::from)).is_err());
    }

    #[test]
    fn parse_list_tags_rejects_overlong_and_control_char_tags() {
        // H2: the --tag filter is an operator-input boundary, so it enforces the
        // same length cap and control-char rejection as label add.
        let over = "x".repeat(labels::MAX_TAG_LEN + 1);
        assert!(parse_list_tags(["--tag".to_string(), over].into_iter()).is_err());
        assert!(
            parse_list_tags(["--tag".to_string(), "area:\u{1b}bad".to_string()].into_iter())
                .is_err()
        );
    }

    #[test]
    fn parse_list_tags_allows_source_namespace_for_filtering() {
        // H1 guards *mutation*, not filtering: you can still filter by provenance.
        let tags =
            parse_list_tags(["--tag".to_string(), "source:operator".to_string()].into_iter())
                .expect("filtering by a source:* tag is allowed");
        assert_eq!(tags, vec!["source:operator".to_string()]);
    }

    #[test]
    fn label_add_rejects_reserved_source_namespace() {
        // H1: an operator cannot forge a provenance chip. Rejection happens
        // before any board I/O, so no state root is needed.
        let err = handle_label_add("g1", "source:operator")
            .expect_err("adding a source:* label must be rejected");
        assert!(err.to_string().contains("source:*"), "got: {err}");
    }

    #[test]
    fn label_add_rejects_overlong_tag() {
        let long = "a".repeat(labels::MAX_TAG_LEN + 1);
        let err = handle_label_add("g1", &long).expect_err("over-long tag must be rejected");
        assert!(err.to_string().contains("too long"), "got: {err}");
    }

    #[test]
    fn label_add_rejects_control_char_tag() {
        let err = handle_label_add("g1", "area:\u{1b}[31mbad")
            .expect_err("control-char tag must be rejected");
        assert!(err.to_string().contains("control"), "got: {err}");
    }

    #[test]
    fn label_remove_rejects_reserved_source_namespace() {
        // H1: provenance is immutable from the operator CLI — it cannot be
        // stripped by hand either.
        let err = handle_label_remove("g1", "source:seed")
            .expect_err("removing a source:* label must be rejected");
        assert!(err.to_string().contains("source:*"), "got: {err}");
    }

    #[test]
    fn format_active_goal_lines_appends_labels_column_unfiltered() {
        let mut board = crate::goal_curation::GoalBoard::new();
        board
            .active
            .push(goal_with("g1", &["source:seed", "area:x"]));
        board.active.push(goal_with("g2", &[]));
        let lines = format_active_goal_lines(&board, &[]);
        assert_eq!(lines[0], "active goals: 2 / 20"); // no "(filtered by tag)"
        assert!(
            lines[1].ends_with("\tLABELS"),
            "header has trailing LABELS: {}",
            lines[1]
        );
        assert!(lines[2].ends_with("\tsource:seed,area:x"));
        assert!(
            lines[3].ends_with("\t"),
            "an unlabelled goal shows an empty LABELS cell"
        );
    }

    #[test]
    fn format_active_goal_lines_filters_with_and_and_annotates_count() {
        let mut board = crate::goal_curation::GoalBoard::new();
        board.active.push(goal_with(
            "g1",
            &["source:creative-ideas", "area:dashboard"],
        ));
        board
            .active
            .push(goal_with("g2", &["source:creative-ideas"]));
        board.active.push(goal_with("g3", &["source:operator"]));

        // Single tag: two match.
        let lines = format_active_goal_lines(&board, &["source:creative-ideas".to_string()]);
        assert_eq!(lines[0], "active goals: 2 / 20 (filtered by tag)");

        // AND of two tags: only g1 matches.
        let lines = format_active_goal_lines(
            &board,
            &[
                "source:creative-ideas".to_string(),
                "area:dashboard".to_string(),
            ],
        );
        assert_eq!(lines[0], "active goals: 1 / 20 (filtered by tag)");
        assert!(lines.iter().any(|l| l.starts_with("g1\t")));
        assert!(!lines.iter().any(|l| l.starts_with("g2\t")));

        // A tag no goal has -> empty filtered view.
        let lines = format_active_goal_lines(&board, &["nope".to_string()]);
        assert_eq!(lines[0], "active goals: 0 / 20 (filtered by tag)");
        assert_eq!(lines[1], "  (none)");
    }

    #[test]
    fn format_label_list_lists_tags_or_none() {
        assert_eq!(format_label_list(&[]), vec!["(none)".to_string()]);
        assert_eq!(
            format_label_list(&["a".to_string(), "b".to_string()]),
            vec!["a".to_string(), "b".to_string()],
        );
    }

    // ---- is_id_placeholder ------------------------------------------------

    #[test]
    fn placeholder_matches_exact_format() {
        assert!(is_id_placeholder("abc-123", "Goal abc-123"));
    }

    #[test]
    fn placeholder_rejects_different_id() {
        assert!(!is_id_placeholder("abc-123", "Goal xyz-456"));
    }

    #[test]
    fn placeholder_rejects_wrong_case() {
        assert!(!is_id_placeholder("abc", "goal abc"));
    }

    #[test]
    fn placeholder_rejects_substring_match() {
        assert!(!is_id_placeholder("abc", "Goal abc extra text"));
    }

    #[test]
    fn placeholder_rejects_empty_desc() {
        assert!(!is_id_placeholder("abc", ""));
    }

    // ---- extract_add_flags (issue #2580 --standing) -----------------------

    fn toks(s: &[&str]) -> Vec<String> {
        s.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn extract_add_flags_plain_description() {
        let (repo, standing, rest) = extract_add_flags(toks(&["ship", "the", "mvp"])).unwrap();
        assert_eq!(repo, None);
        assert!(!standing);
        assert_eq!(rest, toks(&["ship", "the", "mvp"]));
    }

    #[test]
    fn extract_add_flags_parses_standing_anywhere() {
        let (repo, standing, rest) =
            extract_add_flags(toks(&["--standing", "watch", "CI"])).unwrap();
        assert!(standing);
        assert_eq!(repo, None);
        assert_eq!(rest, toks(&["watch", "CI"]));

        // Flag may trail the description too, and --perpetual is an alias.
        let (_r, standing2, rest2) =
            extract_add_flags(toks(&["watch", "CI", "--perpetual"])).unwrap();
        assert!(standing2);
        assert_eq!(rest2, toks(&["watch", "CI"]));
    }

    #[test]
    fn extract_add_flags_combines_repo_and_standing() {
        let (repo, standing, rest) = extract_add_flags(toks(&[
            "--repo",
            "amplihack-rs",
            "--standing",
            "steward",
            "ci",
        ]))
        .unwrap();
        assert_eq!(repo.as_deref(), Some("amplihack-rs"));
        assert!(standing);
        assert_eq!(rest, toks(&["steward", "ci"]));
    }

    #[test]
    fn placeholder_with_empty_id_matches_goal_space() {
        // `format!("Goal {}", "")` produces `"Goal "`, so this matches.
        assert!(is_id_placeholder("", "Goal "));
    }

    #[test]
    fn placeholder_rejects_empty_desc_with_empty_id() {
        assert!(!is_id_placeholder("", ""));
    }

    // ---- GOAL_HELP constant -----------------------------------------------

    #[test]
    fn goal_help_contains_all_subcommands() {
        assert!(GOAL_HELP.contains("list"));
        assert!(GOAL_HELP.contains("add"));
        assert!(GOAL_HELP.contains("demote"));
        assert!(GOAL_HELP.contains("set-priority"));
        assert!(GOAL_HELP.contains("unblock"));
        assert!(GOAL_HELP.contains("unblock-all"));
        assert!(GOAL_HELP.contains("remove"));
        assert!(GOAL_HELP.contains("cleanup --placeholders"));
        assert!(GOAL_HELP.contains("help"));
    }

    // ---- dispatch_goal_command routing -------------------------------------

    #[test]
    fn dispatch_help_flag() {
        let args = vec!["--help".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_help_word() {
        let args = vec!["help".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_short_help() {
        let args = vec!["-h".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_missing_subcommand() {
        let args: Vec<String> = vec![];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("goal command"),
            "expected 'goal command' in: {msg}"
        );
    }

    #[test]
    fn dispatch_unsupported_subcommand() {
        let args = vec!["nonexistent".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unsupported command 'goal nonexistent'"));
    }

    #[test]
    fn dispatch_list_rejects_extra_args() {
        let args = vec!["list".to_string(), "extra".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_unblock_all_rejects_extra_args() {
        let args = vec!["unblock-all".to_string(), "extra".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_unblock_requires_goal_id() {
        let args = vec!["unblock".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("goal id"), "expected 'goal id' in: {msg}");
    }

    // ---- decompose verb (issue #2405) -------------------------------------

    #[test]
    fn goal_help_documents_decompose() {
        assert!(
            GOAL_HELP.contains("decompose"),
            "the goal help text must document the `decompose` verb"
        );
    }

    #[test]
    fn dispatch_decompose_requires_goal_id() {
        // `decompose` must be a recognized verb that requires a goal id —
        // NOT fall through to the `unsupported command` arm. Reaching the
        // missing-id error proves the verb is wired without touching the
        // cognitive-memory writer memory.
        let args = vec!["decompose".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("goal id"),
            "expected a missing-'goal id' error, got: {msg}"
        );
        assert!(
            !msg.contains("unsupported command"),
            "`decompose` must be a recognized verb, got: {msg}"
        );
    }

    // ---- handle_remove ----------------------------------------------------

    #[test]
    fn remove_empty_ids_returns_error() {
        let result = handle_remove(&[]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("at least one id is required"));
    }

    // ---- handle_cleanup ---------------------------------------------------

    #[test]
    fn cleanup_no_flags_returns_error() {
        let result = handle_cleanup(&[]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("at least one criteria flag is required"));
    }

    #[test]
    fn cleanup_unknown_flag_returns_error() {
        let flags = vec!["--unknown".to_string()];
        let result = handle_cleanup(&flags);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unsupported flag '--unknown'"));
    }

    #[test]
    fn cleanup_rejects_partial_flag() {
        let flags = vec!["--placeholder".to_string()];
        let result = handle_cleanup(&flags);
        assert!(result.is_err());
    }

    // ---- handle_add -------------------------------------------------------

    #[test]
    fn add_rejects_zero_priority() {
        let result = handle_add("0", "test goal", None, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("1-7"));
    }

    #[test]
    fn add_rejects_priority_above_7() {
        let result = handle_add("8", "test goal", None, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("1-7"));
    }

    #[test]
    fn add_rejects_non_numeric_priority() {
        let result = handle_add("high", "test goal", None, false);
        assert!(result.is_err());
    }

    // ---- dispatch new commands ------------------------------------------------

    #[test]
    fn dispatch_add_requires_priority() {
        let args = vec!["add".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_add_requires_description() {
        let args = vec!["add".to_string(), "1".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_demote_requires_goal_id() {
        let args = vec!["demote".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_set_priority_requires_goal_id() {
        let args = vec!["set-priority".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_set_priority_requires_priority() {
        let args = vec!["set-priority".to_string(), "some-goal".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
    }

    // ---- render_dry_run_proposal (#2405 F1: sanitize untrusted LLM text) ----

    #[test]
    fn dry_run_proposal_renders_plain_text_unchanged() {
        assert_eq!(
            render_dry_run_proposal(1, "Add a parser", "parser round-trips fixtures"),
            "  1. Add a parser (done: parser round-trips fixtures)"
        );
    }

    #[test]
    fn dry_run_proposal_strips_terminal_control_sequences() {
        // A malicious model could embed ANSI/OSC escapes to recolor, hide, or
        // hyperlink-spoof the operator's console. They must be stripped.
        let line = render_dry_run_proposal(
            2,
            "\u{1b}[31mwipe the disk\u{1b}[0m",
            "\u{1b}]8;;https://evil.invalid\u{7}done\u{1b}]8;;\u{7}",
        );
        assert_eq!(line, "  2. wipe the disk (done: done)");
        assert!(!line.contains('\u{1b}'));
    }

    #[test]
    fn dry_run_proposal_redacts_secret_shaped_text() {
        // Secret-looking lines in untrusted output are redacted, not echoed.
        let line = render_dry_run_proposal(3, "token=sk_live_abc123", "ok");
        assert_eq!(line, "  3. token=[REDACTED] (done: ok)");
    }

    #[test]
    fn dry_run_proposal_folds_newlines_to_prevent_row_spoofing() {
        // Newlines/tabs survive sanitize_terminal_text; fold them so a single
        // proposal cannot forge an extra "  7. ..." preview row.
        let line =
            render_dry_run_proposal(4, "real goal\n  7. forged sibling", "criterion\twith tab");
        assert_eq!(
            line,
            "  4. real goal   7. forged sibling (done: criterion with tab)"
        );
        assert!(!line.contains('\n'));
        assert!(!line.contains('\t'));
    }

    // ---- `simard goal complete` (escalation-triage complete-delivered-goal) ----
    //
    // TDD contract for the CLI verb the escalation-triage docs prescribe for the
    // `complete-delivered-goal` course-correction (issue #17 worked example):
    // marking a goal whose work a merged PR already delivered must remove it from
    // the board AND write a durable tombstone so the daemon's cycle reconcile can
    // never resurrect it. Standing/perpetual goals are the exception — they have
    // no terminal done-state, so `complete` reopens rather than terminates them.
    //
    // These are hermetic: each pins a private SIMARD_STATE_ROOT (serialised on
    // the `cognitive_memory` key, matching the goal_curation store tests) so the
    // authoritative board + tombstone file live in a throwaway temp dir.

    /// Pin `SIMARD_STATE_ROOT` at a fresh temp dir for the duration of a test.
    /// Returns the `TempDir` (keep it alive) and its path.
    fn hermetic_state_root() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        // SAFETY: serialised across modules by the `cognitive_memory` serial key,
        // so no other test reads SIMARD_STATE_ROOT while it is pinned here.
        unsafe { std::env::set_var(crate::state_root::STATE_ROOT_ENV, &root) };
        (tmp, root)
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn complete_removes_delivered_goal_and_writes_tombstone() {
        let (_tmp, root) = hermetic_state_root();
        let goal_id = "fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca";

        with_board(|board| {
            board.active.push(crate::goal_curation::ActiveGoal::new(
                goal_id,
                "int8/PQ embedding quantization spike (delivered by merged PR #40)",
                3,
            ));
            Ok(())
        })
        .expect("seed the delivered goal");

        handle_complete(goal_id).expect("completing a delivered goal must succeed");

        let board = load_board().expect("reload the authoritative board");
        assert!(
            !board.active.iter().any(|g| g.id == goal_id),
            "a completed goal must be gone from the active board"
        );
        assert!(
            !board.backlog.iter().any(|b| b.id == goal_id),
            "a completed goal must be gone from the backlog"
        );

        let tombstones = crate::ooda_loop::load_tombstones(&root);
        assert!(
            tombstones.contains(goal_id),
            "completing a goal must write a durable tombstone so it cannot re-stick"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn complete_is_idempotent_for_an_absent_goal() {
        let (_tmp, root) = hermetic_state_root();
        let goal_id = "never-on-the-board";

        // No seeding: the goal is not on the board. Completion must still
        // succeed and still record the tombstone (idempotent contract).
        handle_complete(goal_id).expect("completing an absent goal is a no-op success");
        assert!(
            crate::ooda_loop::load_tombstones(&root).contains(goal_id),
            "an absent completion must still record a durable tombstone"
        );

        // A second call must also be a clean success.
        handle_complete(goal_id).expect("a repeated completion must remain idempotent");
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn complete_refuses_a_standing_goal_and_reopens_it_without_tombstone() {
        let (_tmp, root) = hermetic_state_root();
        let goal_id = "steward-ci-health";

        with_board(|board| {
            board.active.push(
                crate::goal_curation::ActiveGoal::new(goal_id, "steward CI health", 2)
                    .mark_standing(),
            );
            Ok(())
        })
        .expect("seed the standing goal");

        handle_complete(goal_id).expect("completing a standing goal must succeed (by reopening)");

        let board = load_board().expect("reload the authoritative board");
        assert!(
            board.active.iter().any(|g| g.id == goal_id),
            "a standing goal has no terminal done-state: it must be reopened, never removed"
        );
        assert!(
            !crate::ooda_loop::load_tombstones(&root).contains(goal_id),
            "a reopened standing goal must NOT be tombstoned"
        );
    }

    #[test]
    fn dispatch_complete_requires_goal_id() {
        // `complete` must be a recognized verb that requires a goal id — not
        // fall through to the `unsupported command` arm. Reaching the missing-id
        // error proves the verb is wired without touching any state root.
        let args = vec!["complete".to_string()];
        let result = dispatch_goal_command(args.into_iter());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("goal id"),
            "expected a missing-'goal id' error, got: {msg}"
        );
        assert!(
            !msg.contains("unsupported command"),
            "`complete` must be a recognized verb, got: {msg}"
        );
    }

    #[test]
    fn goal_help_documents_complete() {
        assert!(
            GOAL_HELP.contains("complete"),
            "the goal help text must document the `complete` verb the docs prescribe"
        );
    }
}
