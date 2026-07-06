//! Issue #26 — hermetic TDD tests for the Logs tab's "Cycle Reports" card
//! (`#cycle-reports`, fed by `/api/logs` → `cycle_reports`).
//!
//! The operator observed the card repeating an identical, detail-less, STALE
//! line ("Cycle #1 … uncommitted changes") while the live daemon was actually
//! at cycle 13+ on a CLEAN tree. Root cause: `logs()` reads a SINGLE directory
//! (`<state_root>/cycle_reports/`), lexicographically oldest-first, with no
//! union of the live `state/cycle_reports/` dir, no newest-first ordering, no
//! collapse, and no shared shape with the Thinking tab. These tests drive the
//! REQUIRED behaviour end-to-end through the real `logs()` and `ooda_thinking()`
//! handlers against a hermetic [`HermeticState`] (temp dir, no daemon), and
//! encode the fixed contract:
//!
//!   * R1 — the card shows the ACTUAL current cycle index (newest-first) and
//!     unions BOTH persisted dirs, so a live cycle in `state/cycle_reports/`
//!     wins over a stale `cycle_1.json` in the top-level dir; the displayed
//!     cycle number is the authoritative FILENAME number, never a frozen "#1";
//!   * R2 — each entry reflects that cycle's REAL tree status (clean vs dirty),
//!     not a stale constant; a clean live cycle reads "clean";
//!   * R3/R6 — each entry carries the Observe/Orient/Decide/Act phase detail AND
//!     the shared disposition annotation, i.e. the SAME raw report shape the
//!     Thinking tab renders (not aggregate counts, not a `{cycle_number, report}`
//!     wrapper);
//!   * R4 — a run of N genuinely-identical cycles collapses to ONE row carrying
//!     `repeat_count = N` and the newest/oldest cycle range;
//!   * R6 — the Logs card (`/api/logs`) and the Thinking view (`/api/ooda-thinking`)
//!     read from ONE shared reader and therefore AGREE on the same collapsed
//!     data.
//!
//! Each test mutates process-global `SIMARD_STATE_ROOT` via `HermeticState` and
//! is serialised under the `cognitive_memory` key, matching the repo-wide
//! env-isolation contract used by `tests_ooda_cycles_history.rs`.

use std::path::Path;

use serde_json::{Value, json};

use super::logs::logs;
use super::metrics::ooda_thinking;
use crate::test_support::HermeticState;

/// Write a raw cycle-report body to `<dir>/cycle_<N>.json`, mirroring what the
/// daemon persists on disk. `dir` is a full `.../cycle_reports` directory so a
/// test can target either the top-level or the live `state/` copy.
fn write_report_to(dir: &Path, cycle: u32, body: &Value) {
    std::fs::create_dir_all(dir).expect("create cycle_reports dir");
    std::fs::write(
        dir.join(format!("cycle_{cycle}.json")),
        serde_json::to_string(body).expect("serialize cycle report"),
    )
    .expect("write cycle report");
}

/// The top-level `<state_root>/cycle_reports/` directory (the ONLY one the
/// buggy `logs()` reads today).
fn top_dir(state_root: &Path) -> std::path::PathBuf {
    state_root.join("cycle_reports")
}

/// The live `<state_root>/state/cycle_reports/` directory the running daemon
/// actually writes to.
fn live_dir(state_root: &Path) -> std::path::PathBuf {
    state_root.join("state").join("cycle_reports")
}

/// Canonical daemon summary line carrying the machine tree token
/// (`tree=clean` / `tree=dirty`) that the client humanizer consumes.
fn summary_line(cycle: u32, tree: &str) -> String {
    format!(
        "OODA cycle #{cycle}: 3 priorities, 2 actions (2/2 succeeded), goals=2, issues=20, tree={tree}"
    )
}

/// A no-action deferral cycle ("goal already has a live, healthy engineer").
/// Consecutive deferrals on the same goal collapse into one row.
fn deferral_body(cycle: u32, goal: &str, tree: &str) -> Value {
    json!({
        "cycle_number": cycle,
        "summary": summary_line(cycle, tree),
        "observation": {
            "goal_count": 2,
            "environment": {
                "open_issues": 20,
                "recent_commits": 1,
                "git_status": if tree == "clean" { "" } else { " M src/foo.rs" },
            },
        },
        "outcomes": [{
            "action_kind": "AdvanceGoal",
            "action_description": "advance goal",
            "success": true,
            "goal_id": goal,
            "detail": "no_action - goal already has a live, healthy engineer",
        }],
    })
}

/// A forward-progress cycle carrying full Observe/Orient/Decide/Act detail:
/// launched an engineer and opened a PR. Distinct cycle numbers never collapse.
fn progress_body(cycle: u32, goal: &str, tree: &str) -> Value {
    json!({
        "cycle_number": cycle,
        "summary": summary_line(cycle, tree),
        "observation": {
            "goal_count": 2,
            "environment": {
                "open_issues": 20,
                "recent_commits": 3,
                "git_status": if tree == "clean" { "" } else { " M src/foo.rs" },
            },
        },
        "priorities": [{ "goal_id": goal, "urgency": 0.8, "reason": "needs work" }],
        "planned_actions": [{ "kind": "advance-goal", "goal_id": goal, "description": "open a PR" }],
        "outcomes": [{
            "action_kind": "AdvanceGoal",
            "action_description": "opened PR #204",
            "success": true,
            "goal_id": goal,
            "detail": "spawn_engineer dispatched: agent='engineer-g1-1700'; launched, opened PR #204",
        }],
    })
}

/// The `cycle_reports` array from a `/api/logs` response.
fn cycle_reports_of(resp: &Value) -> Vec<Value> {
    resp["cycle_reports"]
        .as_array()
        .cloned()
        .expect("/api/logs response must carry a cycle_reports array")
}

// ---------------------------------------------------------------------------
// R1 — live cycle number: newest-first, unions both dirs, never a frozen "#1".
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn cycle_reports_show_live_cycle_from_union_of_both_dirs_newest_first() {
    let state = HermeticState::new();
    // A STALE report in the top-level dir (the only dir the buggy reader sees)…
    write_report_to(
        &top_dir(state.state_root()),
        5,
        &progress_body(5, "g1", "dirty"),
    );
    // …and the LIVE, higher-numbered report the running daemon writes under
    // state/cycle_reports/.
    write_report_to(
        &live_dir(state.state_root()),
        20,
        &progress_body(20, "g2", "clean"),
    );

    let resp = logs().await.0;
    let reports = cycle_reports_of(&resp);

    assert!(
        !reports.is_empty(),
        "Cycle Reports must include the live cycle written under state/cycle_reports/ \
         (the buggy reader only scans the top-level dir and returns nothing here), got: {resp}"
    );
    let newest = reports[0]["cycle_number"].as_u64().unwrap_or(0);
    assert_eq!(
        newest, 20,
        "the newest (first) entry must carry the ACTUAL current cycle index 20 from the \
         union of both persisted dirs, not the stale top-level #5 — got cycle #{newest}: {resp}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn cycle_reports_cycle_number_comes_from_authoritative_filename_not_frozen_body() {
    // The daemon's process-local body counter resets to 1 on restart (#1680),
    // while the persisted FILENAME carries the cumulative number. The card must
    // display the filename number (13), never the frozen body "#1".
    let state = HermeticState::new();
    let mut body = progress_body(13, "g1", "clean");
    body["cycle_number"] = json!(1); // stale/reset in-body counter
    write_report_to(&top_dir(state.state_root()), 13, &body);

    let resp = logs().await.0;
    let reports = cycle_reports_of(&resp);
    assert_eq!(
        reports.len(),
        1,
        "one report on disk → one row, got: {resp}"
    );
    assert_eq!(
        reports[0]["cycle_number"].as_u64().unwrap_or(0),
        13,
        "the displayed cycle number must be the authoritative filename number 13, \
         not the frozen in-body #1: {resp}"
    );
}

// ---------------------------------------------------------------------------
// R2 — accurate per-cycle tree status: clean vs dirty from the input, never a
// stale constant.
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn cycle_reports_tree_status_reflects_each_cycles_real_state() {
    let state = HermeticState::new();
    // Two distinct progressing cycles (never collapse): an older DIRTY cycle and
    // a newer CLEAN cycle. The card must surface each cycle's real tree status.
    write_report_to(
        &top_dir(state.state_root()),
        1,
        &progress_body(1, "g1", "dirty"),
    );
    write_report_to(
        &top_dir(state.state_root()),
        2,
        &progress_body(2, "g1", "clean"),
    );

    let resp = logs().await.0;
    let reports = cycle_reports_of(&resp);
    assert_eq!(
        reports.len(),
        2,
        "two distinct progressing cycles must remain two rows, got: {resp}"
    );

    let newest_summary = reports[0]["summary"].as_str().unwrap_or("");
    let oldest_summary = reports[1]["summary"].as_str().unwrap_or("");
    assert!(
        newest_summary.contains("tree=clean"),
        "the newest (live) cycle's tree status must read clean, got: {newest_summary:?}"
    );
    assert!(
        oldest_summary.contains("tree=dirty"),
        "the older cycle must retain its own dirty tree status, got: {oldest_summary:?}"
    );
    assert_ne!(
        newest_summary, oldest_summary,
        "tree status must vary per cycle, not be a stale constant across every row: {resp}"
    );
    // The structured git_status must likewise differ per cycle so the phase
    // renderer can honestly show clean vs dirty.
    let newest_git = reports[0]["observation"]["environment"]["git_status"]
        .as_str()
        .unwrap_or("<absent>");
    assert_eq!(
        newest_git, "",
        "a clean cycle must carry an empty git_status (renderer shows 'clean'), got: {resp}"
    );
}

// ---------------------------------------------------------------------------
// R4 — dedup/collapse: N genuinely-identical cycles → one row with a count.
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn cycle_reports_collapse_identical_cycles_into_one_counted_row() {
    let state = HermeticState::new();
    for cycle in 1..=5u32 {
        write_report_to(
            &top_dir(state.state_root()),
            cycle,
            &deferral_body(cycle, "g1", "clean"),
        );
    }

    let resp = logs().await.0;
    let reports = cycle_reports_of(&resp);
    assert_eq!(
        reports.len(),
        1,
        "five identical no-progress deferrals must collapse into a SINGLE Cycle Reports row, \
         got {} rows: {resp}",
        reports.len()
    );
    assert_eq!(
        reports[0]["repeat_count"].as_u64().unwrap_or(0),
        5,
        "the collapsed row must carry the run's repeat count of 5: {resp}"
    );
    assert_eq!(
        reports[0]["cycle_number_first"].as_u64().unwrap_or(0),
        5,
        "cycle_number_first = newest cycle in the collapsed run: {resp}"
    );
    assert_eq!(
        reports[0]["cycle_number_last"].as_u64().unwrap_or(0),
        1,
        "cycle_number_last = oldest cycle in the collapsed run: {resp}"
    );
}

// ---------------------------------------------------------------------------
// R3/R6 — per-cycle detail in the SHARED Thinking-tab shape: Observe/Orient/
// Decide/Act fields plus the disposition annotation, at the TOP LEVEL of each
// entry (not nested under a `{cycle_number, report}` wrapper, not just counts).
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn cycle_reports_carry_full_ooda_detail_and_disposition() {
    let state = HermeticState::new();
    write_report_to(
        &top_dir(state.state_root()),
        7,
        &progress_body(7, "g1", "clean"),
    );

    let resp = logs().await.0;
    let reports = cycle_reports_of(&resp);
    assert_eq!(reports.len(), 1, "one report → one row, got: {resp}");
    let row = &reports[0];

    // The Observe/Orient/Decide/Act detail the Thinking tab renders must be
    // present at the top level so the shared entry-renderer can display it.
    for field in ["observation", "priorities", "planned_actions", "outcomes"] {
        assert!(
            row.get(field).is_some_and(|v| !v.is_null()),
            "entry must carry top-level `{field}` detail (shared Thinking-tab shape), got: {row}"
        );
    }
    // The shared collapse annotation must be present so a run stands out from a
    // single progressing cycle — the Logs card can no longer be a divergent,
    // detail-less copy.
    assert_eq!(
        row["disposition"].as_str().unwrap_or(""),
        "progressing",
        "entry must carry the shared disposition annotation, got: {row}"
    );
    assert!(
        row.get("repeat_count").is_some(),
        "entry must carry a repeat_count from the shared collapse pass, got: {row}"
    );
}

// ---------------------------------------------------------------------------
// R6 — reconciliation: the Logs card and the Thinking view read from ONE shared
// reader and therefore AGREE on the same collapsed data.
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn cycle_reports_agree_with_thinking_view_on_same_data() {
    let state = HermeticState::new();
    // One progressing cycle plus a run of three identical deferrals.
    write_report_to(
        &top_dir(state.state_root()),
        10,
        &progress_body(10, "g1", "clean"),
    );
    for cycle in 7..=9u32 {
        write_report_to(
            &top_dir(state.state_root()),
            cycle,
            &deferral_body(cycle, "g2", "clean"),
        );
    }

    let logs_reports = cycle_reports_of(&logs().await.0);
    let thinking_resp = ooda_thinking().await.0;
    let thinking_reports = thinking_resp["reports"]
        .as_array()
        .cloned()
        .expect("/api/ooda-thinking must carry a reports array");

    assert_eq!(
        logs_reports, thinking_reports,
        "the Logs 'Cycle Reports' card and the Thinking 'Cycle History' view must render \
         the SAME collapsed cycle data from one shared reader — they diverge today: \
         logs={logs_reports:?} thinking={thinking_reports:?}"
    );
}
