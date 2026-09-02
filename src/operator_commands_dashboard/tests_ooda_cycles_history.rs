//! Issue #21 — hermetic integration tests for the Thinking tab's FIRST HALF
//! (`/api/ooda-cycles`, the Cycle History surface) and a preservation guard for
//! the SECOND HALF (`/api/ooda-thinking`).
//!
//! These tests drive the required behaviour end-to-end through the real
//! endpoint handlers against a hermetic [`HermeticState`] state root (temp dir,
//! no network, no daemon):
//!
//!   * a run of N consecutive equivalent cycles collapses to ONE row carrying
//!     `repeat_count = N` and the newest/oldest cycle range;
//!   * `total_cycles` reports the RAW (pre-collapse) count so "N cycles
//!     recorded" reflects real activity;
//!   * rows carry real timestamps, and `—`/empty only for genuinely legacy
//!     cycles;
//!   * the duration trend is computed from the UNCOLLAPSED per-cycle series, so
//!     collapsing rows never starves the chart; and when no cycle carries a
//!     duration the trend reads `insufficient_data` and rows carry a null
//!     `duration_secs` (the data precondition for the renderer hiding the
//!     chart);
//!   * every row's `collapsed_summary` is a non-empty, difference-carrying
//!     string rather than count-boilerplate; and
//!   * the second half's strict-collapse deferral summary is preserved
//!     byte-for-byte.
//!
//! Each test constructs a `HermeticState` (which mutates process-global
//! `SIMARD_STATE_ROOT`) and is therefore serialised under the
//! `cognitive_memory` key, matching the repo-wide env-isolation contract.

use std::path::Path;

use serde_json::{Value, json};

use super::metrics::ooda_thinking;
use super::ooda_cycles::ooda_cycles;
use crate::test_support::HermeticState;

/// Write a raw cycle report body to `<state_root>/cycle_reports/cycle_<N>.json`,
/// mirroring what the producer persists on disk.
fn write_report(state_root: &Path, cycle: u32, body: &Value) {
    let dir = state_root.join("cycle_reports");
    std::fs::create_dir_all(&dir).expect("create cycle_reports dir");
    std::fs::write(
        dir.join(format!("cycle_{cycle}.json")),
        serde_json::to_string(body).expect("serialize cycle report"),
    )
    .expect("write cycle report");
}

/// A no-action deferral cycle ("goal already has a live, healthy engineer").
fn deferral_body(
    cycle: u32,
    goal: &str,
    duration_secs: Option<f64>,
    timestamp: Option<&str>,
) -> Value {
    let mut b = json!({
        "cycle_number": cycle,
        "summary": format!(
            "Cycle #{cycle} — 3 priorities considered, 2 of 2 actions succeeded · 2 goals tracked · 20 open issues · working tree clean"
        ),
        "outcomes": [{
            "action_kind": "AdvanceGoal",
            "action_description": "advance goal",
            "success": true,
            "goal_id": goal,
            "detail": "no_action - goal already has a live, healthy engineer",
        }],
    });
    if let Some(d) = duration_secs {
        b["duration_secs"] = json!(d);
    }
    if let Some(t) = timestamp {
        b["timestamp"] = json!(t);
    }
    b
}

/// A forward-progress cycle: launched an engineer and opened a PR.
fn progress_body(cycle: u32, goal: &str, timestamp: Option<&str>) -> Value {
    let mut b = json!({
        "cycle_number": cycle,
        "summary": format!(
            "Cycle #{cycle} — 3 priorities considered, 1 of 1 actions succeeded · 2 goals tracked · 20 open issues · working tree clean"
        ),
        "outcomes": [{
            "action_kind": "AdvanceGoal",
            "action_description": "opened PR #204",
            "success": true,
            "goal_id": goal,
            "detail": "spawn_engineer dispatched: agent='engineer-g1-1700', task='fix bug' (goal 'g1', pid=1234); launched, opened PR #204",
            "spawn_engineer": {"status": "live", "goal_id": goal},
        }],
    });
    if let Some(t) = timestamp {
        b["timestamp"] = json!(t);
    }
    b
}

fn cycles_of(resp: &Value) -> &Vec<Value> {
    resp["cycles"].as_array().expect("cycles array")
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn ooda_cycles_collapses_consecutive_deferrals_into_one_row() {
    let state = HermeticState::new();
    for cycle in 1..=5u32 {
        let ts = format!("2026-07-06T04:0{cycle}:00Z");
        write_report(
            state.state_root(),
            cycle,
            &deferral_body(cycle, "g1", Some(11.9), Some(&ts)),
        );
    }

    let resp = ooda_cycles().await.0;
    let cycles = cycles_of(&resp);

    assert_eq!(
        cycles.len(),
        1,
        "five identical deferrals must collapse to a single Cycle History row, got: {resp}"
    );
    let row = &cycles[0];
    assert_eq!(
        row["repeat_count"], 5,
        "row must carry the run's repeat count"
    );
    assert_eq!(row["disposition"], "deferring");
    assert_eq!(
        row["cycle_number_first"], 5,
        "first = newest cycle in the run"
    );
    assert_eq!(
        row["cycle_number_last"], 1,
        "last = oldest cycle in the run"
    );
    assert_eq!(
        row["collapsed_summary"].as_str().unwrap_or(""),
        "no-action: deferring to active engineer on g1",
        "collapsed row must carry the difference-carrying deferral summary"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn ooda_cycles_total_cycles_is_raw_precollapse_count() {
    let state = HermeticState::new();
    for cycle in 1..=5u32 {
        let ts = format!("2026-07-06T04:0{cycle}:00Z");
        write_report(
            state.state_root(),
            cycle,
            &deferral_body(cycle, "g1", Some(11.9), Some(&ts)),
        );
    }

    let resp = ooda_cycles().await.0;
    assert_eq!(cycles_of(&resp).len(), 1, "rows are collapsed to one");
    assert_eq!(
        resp["total_cycles"], 5,
        "total_cycles must reflect the RAW pre-collapse count so 'N cycles recorded' stays honest, got: {resp}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn ooda_cycles_latest_cycle_number_is_the_authoritative_index_not_the_row_count() {
    // Persisted cycle numbers are the cumulative daemon index, NOT 1..=N. The
    // Cycle History tab must report the real lifetime cycle number, so
    // `latest_cycle_number` tracks the highest persisted filename index — never
    // the number of rows scanned.
    let state = HermeticState::new();
    for cycle in [500u32, 501u32] {
        let ts = format!("2026-07-06T04:00:0{}Z", cycle % 10);
        write_report(
            state.state_root(),
            cycle,
            &progress_body(cycle, &format!("g{cycle}"), Some(&ts)),
        );
    }

    let resp = ooda_cycles().await.0;
    assert_eq!(
        resp["latest_cycle_number"], 501,
        "latest_cycle_number must be the highest persisted cycle index (501), got: {resp}"
    );
    assert_eq!(
        resp["total_cycles"], 2,
        "total_cycles is the count of reports in the window, got: {resp}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn ooda_cycles_latest_cycle_number_exceeds_capped_window() {
    // With more persisted cycles than the MAX_CYCLES=50 scan window, the tab
    // would otherwise read "50 cycles recorded" while the daemon is far past
    // that. `latest_cycle_number` must expose the true lifetime count so the UI
    // can render "Showing last 50 of <lifetime>" instead of undercounting.
    let state = HermeticState::new();
    for cycle in 1..=60u32 {
        let ts = format!("2026-07-06T05:{:02}:00Z", cycle % 60);
        write_report(
            state.state_root(),
            cycle,
            &progress_body(cycle, "g1", Some(&ts)),
        );
    }

    let resp = ooda_cycles().await.0;
    let total = resp["total_cycles"]
        .as_u64()
        .expect("total_cycles is a number");
    let latest = resp["latest_cycle_number"]
        .as_u64()
        .expect("latest_cycle_number is a number");
    assert_eq!(
        total, 50,
        "the window is capped at MAX_CYCLES=50, got: {resp}"
    );
    assert_eq!(
        latest, 60,
        "latest_cycle_number must reflect the true lifetime cycle count, got: {resp}"
    );
    assert!(
        latest > total,
        "the fix's premise: lifetime count exceeds the capped window, got: {resp}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn ooda_cycles_rows_carry_real_timestamps_and_legacy_gets_empty() {
    let state = HermeticState::new();
    // A progressing cycle WITH a timestamp, and a legacy deferral cycle WITHOUT
    // one. Different dispositions → they do not collapse together.
    write_report(
        state.state_root(),
        2,
        &progress_body(2, "g1", Some("2026-07-06T04:20:00Z")),
    );
    write_report(state.state_root(), 1, &deferral_body(1, "g1", None, None));

    let resp = ooda_cycles().await.0;
    let cycles = cycles_of(&resp);
    assert_eq!(
        cycles.len(),
        2,
        "distinct dispositions stay separate, got: {resp}"
    );

    assert_eq!(
        cycles[0]["timestamp"].as_str().unwrap_or(""),
        "2026-07-06T04:20:00Z",
        "newest row must surface its real timestamp"
    );
    assert_eq!(
        cycles[1]["timestamp"].as_str().unwrap_or("<absent>"),
        "",
        "a genuinely legacy cycle (no timestamp) must yield an empty timestamp (renderer shows —)"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn ooda_cycles_duration_trend_uses_uncollapsed_series() {
    let state = HermeticState::new();
    // Four deferrals on the same goal collapse to ONE row, but the duration
    // trend must still be computed from all four (uncollapsed) durations.
    // Newest cycles are faster → the trend must read "improving".
    let durations = [(4u32, 10.0), (3, 12.0), (2, 20.0), (1, 25.0)];
    for (cycle, dur) in durations {
        let ts = format!("2026-07-06T04:0{cycle}:00Z");
        write_report(
            state.state_root(),
            cycle,
            &deferral_body(cycle, "g1", Some(dur), Some(&ts)),
        );
    }

    let resp = ooda_cycles().await.0;
    assert_eq!(
        cycles_of(&resp).len(),
        1,
        "the four deferrals collapse to one row"
    );
    assert_eq!(cycles_of(&resp)[0]["repeat_count"], 4);
    assert_eq!(
        resp["duration_trend"]["direction"], "improving",
        "trend must be computed from the UNCOLLAPSED 4-point duration series, got: {resp}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn ooda_cycles_without_duration_reads_insufficient_and_null() {
    let state = HermeticState::new();
    for cycle in 1..=2u32 {
        let ts = format!("2026-07-06T04:0{cycle}:00Z");
        write_report(
            state.state_root(),
            cycle,
            &deferral_body(cycle, "g1", None, Some(&ts)),
        );
    }

    let resp = ooda_cycles().await.0;
    let cycles = cycles_of(&resp);
    assert_eq!(cycles.len(), 1, "the two deferrals collapse to one row");
    assert!(
        cycles[0]["duration_secs"].is_null(),
        "a row with no recorded duration must carry a null duration_secs, got: {resp}"
    );
    assert_eq!(
        resp["duration_trend"]["direction"], "insufficient_data",
        "with no duration data the trend verdict must read insufficient_data (chart hidden), got: {resp}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn ooda_cycles_progress_summary_is_difference_carrying() {
    let state = HermeticState::new();
    write_report(
        state.state_root(),
        1,
        &progress_body(1, "g1", Some("2026-07-06T04:10:00Z")),
    );

    let resp = ooda_cycles().await.0;
    let cycles = cycles_of(&resp);
    assert_eq!(cycles.len(), 1);
    let summary = cycles[0]["collapsed_summary"].as_str().unwrap_or("");
    assert!(
        !summary.is_empty(),
        "progressing row must have a non-empty summary"
    );
    assert!(
        !summary.contains("priorities considered"),
        "progressing summary must describe the ACTION, not the count-boilerplate, got: {summary}"
    );
}

// ---------------------------------------------------------------------------
// SECOND HALF preserved: `/api/ooda-thinking` strict collapse is unchanged.
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn ooda_thinking_preserves_legacy_strict_deferral_summary() {
    let state = HermeticState::new();
    for cycle in 1..=3u32 {
        let ts = format!("2026-07-06T04:0{cycle}:00Z");
        write_report(
            state.state_root(),
            cycle,
            &deferral_body(cycle, "g1", Some(11.9), Some(&ts)),
        );
    }

    let resp = ooda_thinking().await.0;
    let reports = resp["reports"].as_array().expect("reports array");
    assert_eq!(
        reports.len(),
        1,
        "second half still collapses the deferral run"
    );
    let summary = reports[0]["collapsed_summary"].as_str().unwrap_or("");
    assert!(
        summary.starts_with("Deferring to an active engineer on g1"),
        "the PRESERVED second half must keep the legacy strict deferral phrasing, got: {summary}"
    );
}
