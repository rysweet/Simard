//! Tests for `activity.rs` handler functions (issue #1750).
//!
//! Both `traces()` and `activity()` gracefully handle missing files and
//! failing external commands, so we test them with controlled (empty) state
//! roots and ensure the JSON shape is always well-formed.

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::memory_ipc::{clear_in_process_writer, register_in_process_writer};
use crate::operator_commands_dashboard::activity::{activity, traces};
use crate::operator_commands_dashboard::metrics::memory_metrics;
use crate::test_support::HermeticState;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// traces()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn traces_returns_valid_json_structure() {
    // Even with no trace files or journald, traces() should return a valid
    // JSON response with the expected top-level keys.
    let result = traces().await;
    let val = &result.0;

    assert!(val.get("span_count").is_some(), "missing 'span_count' key");
    assert!(val.get("spans").is_some(), "missing 'spans' key");
    assert!(val["spans"].is_array(), "spans should be an array");
    assert!(
        val.get("otel_enabled").is_some(),
        "missing 'otel_enabled' key"
    );
    assert!(val.get("timestamp").is_some(), "missing 'timestamp' key");
}

#[tokio::test]
async fn traces_span_count_matches_array_length() {
    let result = traces().await;
    let val = &result.0;

    let count = val["span_count"].as_u64().unwrap_or(0);
    let arr_len = val["spans"].as_array().map(|a| a.len()).unwrap_or(0);
    assert_eq!(
        count as usize, arr_len,
        "span_count ({count}) should match spans array length ({arr_len})"
    );
}

#[tokio::test]
async fn traces_otel_enabled_reflects_env_var() {
    // Without the env var set, otel should be disabled.
    let result = traces().await;
    let val = &result.0;
    let has_otel_env = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok();
    assert_eq!(
        val["otel_enabled"].as_bool().unwrap_or(false),
        has_otel_env,
        "otel_enabled should reflect whether OTEL_EXPORTER_OTLP_ENDPOINT is set"
    );
}

#[tokio::test]
async fn traces_timestamp_is_valid_rfc3339() {
    let result = traces().await;
    let ts = result.0["timestamp"].as_str().unwrap();
    assert!(
        chrono::DateTime::parse_from_rfc3339(ts).is_ok(),
        "timestamp should be valid RFC3339: {ts}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn traces_reads_cost_ledger_when_present() {
    // HermeticState sets SIMARD_STATE_ROOT to a tempdir and cleans up on Drop,
    // so tests never touch $HOME/.simard and panics cannot leave state residue.
    let state = HermeticState::new();
    let ledger_dir = state.state_root().join("costs");
    std::fs::create_dir_all(&ledger_dir).unwrap();

    let ledger_path = ledger_dir.join("ledger.jsonl");
    let test_line = r#"{"model":"test","cost_usd":0.001,"timestamp":"2025-01-01T00:00:00Z"}"#;
    std::fs::write(&ledger_path, format!("{test_line}\n")).unwrap();

    let result = traces().await;
    let spans = result.0["spans"].as_array().unwrap();
    let has_cost = spans.iter().any(|s| s["source"] == "cost");
    assert!(has_cost, "should have at least one cost span from ledger");
}

// ---------------------------------------------------------------------------
// activity()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activity_returns_valid_json_structure() {
    let result = activity().await;
    let val = &result.0;

    assert!(val.get("daemon").is_some(), "missing 'daemon' key");
    assert!(
        val.get("recent_cycles").is_some(),
        "missing 'recent_cycles' key"
    );
    // #26 FIX 1: the duplicative Overview "Open PRs" card was removed, so
    // /api/activity must no longer emit its `open_prs` data key.
    assert!(
        val.get("open_prs").is_none(),
        "activity() must NOT emit 'open_prs' after the duplicative Open PRs \
         card removal (#26); Merge Readiness (/api/merge-readiness) is the \
         single source for open-PR state"
    );
    assert!(
        val.get("assigned_issues").is_some(),
        "missing 'assigned_issues' key"
    );
    assert!(val.get("timestamp").is_some(), "missing 'timestamp' key");
}

#[tokio::test]
async fn activity_daemon_section_has_expected_fields() {
    let result = activity().await;
    let daemon = &result.0["daemon"];

    assert!(daemon.get("status").is_some(), "daemon missing 'status'");
    assert!(
        daemon.get("current_cycle").is_some(),
        "daemon missing 'current_cycle'"
    );
    assert!(
        daemon.get("last_heartbeat").is_some(),
        "daemon missing 'last_heartbeat'"
    );
    assert!(
        daemon.get("actions_taken").is_some(),
        "daemon missing 'actions_taken'"
    );
}

#[tokio::test]
async fn activity_timestamp_is_valid_rfc3339() {
    let result = activity().await;
    let ts = result.0["timestamp"].as_str().unwrap();
    assert!(
        chrono::DateTime::parse_from_rfc3339(ts).is_ok(),
        "timestamp should be valid RFC3339: {ts}"
    );
}

#[tokio::test]
async fn activity_recent_cycles_is_array() {
    let result = activity().await;
    assert!(
        result.0["recent_cycles"].is_array(),
        "recent_cycles should be an array"
    );
}

#[tokio::test]
async fn activity_omits_open_prs() {
    // #26 FIX 1: the duplicative Overview "Open PRs" card and its data
    // producer were removed. The Merge Readiness card (fed by the separate
    // /api/merge-readiness object) is the single source for open-PR state, so
    // /api/activity must no longer carry an `open_prs` array at all.
    let result = activity().await;
    assert!(
        result.0.get("open_prs").is_none(),
        "activity() must not expose 'open_prs' — it duplicated Merge Readiness (#26)"
    );
}

#[tokio::test]
async fn activity_assigned_issues_is_array() {
    let result = activity().await;
    assert!(
        result.0["assigned_issues"].is_array(),
        "assigned_issues should be an array"
    );
}

#[tokio::test]
async fn activity_daemon_status_is_string() {
    let result = activity().await;
    let status = &result.0["daemon"]["status"];
    assert!(
        status.is_string(),
        "daemon status should be a string, got: {status}"
    );
    let s = status.as_str().unwrap();
    // The value is derived from daemon_health.json — should be a known status string
    assert!(!s.is_empty(), "daemon status should be a non-empty string");
}

/// RAII guard that sets an env var for its lifetime and restores on Drop.
/// Used to redirect XDG_DATA_HOME so `dirs::data_local_dir()` resolves
/// inside the hermetic tempdir.
struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: tests using this guard are serialised via
        // #[serial(cognitive_memory)].
        unsafe { std::env::set_var(key, value.as_os_str()) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn activity_reads_daemon_health_when_present() {
    // HermeticState isolates SIMARD_STATE_ROOT. We also redirect
    // XDG_DATA_HOME so dirs::data_local_dir() resolves inside the tempdir.
    let state = HermeticState::new();
    let xdg_root = state.state_root().join("xdg_data");
    let _xdg_guard = EnvGuard::set("XDG_DATA_HOME", &xdg_root);

    let health_dir = xdg_root.join("simard");
    std::fs::create_dir_all(&health_dir).unwrap();

    let now = chrono::Utc::now().to_rfc3339();
    let fake_health = serde_json::json!({
        "status": "running",
        "cycle_number": 42,
        "timestamp": now,
        "actions_taken": ["advance-goal", "consolidate-memory"]
    });
    std::fs::write(
        health_dir.join("daemon_health.json"),
        fake_health.to_string(),
    )
    .unwrap();

    let result = activity().await;
    let daemon = &result.0["daemon"];
    assert_eq!(daemon["current_cycle"], 42);
    assert_eq!(daemon["status"], "running");
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn activity_cycle_uses_persistent_report_after_restart() {
    // Regression for #1680: after a daemon restart the process-local
    // daemon_health counter resets to #1, but the persisted cycle reports keep
    // the cumulative count. The Overview header (current_cycle) must show the
    // persistent number so it agrees with the Thinking tab / Recent Actions.
    let state = HermeticState::new();
    let xdg_root = state.state_root().join("xdg_data");
    let _xdg_guard = EnvGuard::set("XDG_DATA_HOME", &xdg_root);

    let health_dir = xdg_root.join("simard");
    std::fs::create_dir_all(&health_dir).unwrap();
    let fake_health = serde_json::json!({
        "status": "running",
        "cycle_number": 1,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(
        health_dir.join("daemon_health.json"),
        fake_health.to_string(),
    )
    .unwrap();

    // A persisted report from before the restart.
    let reports_dir = state.state_root().join("cycle_reports");
    std::fs::create_dir_all(&reports_dir).unwrap();
    std::fs::write(
        reports_dir.join("cycle_369.json"),
        serde_json::json!({"cycle_number": 369, "summary": "pre-restart"}).to_string(),
    )
    .unwrap();

    let result = activity().await;
    assert_eq!(
        result.0["daemon"]["current_cycle"], 369,
        "current_cycle must reflect the persisted cumulative count, not the process-local #1"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn activity_cycle_uses_persistent_report_when_daemon_absent() {
    // Degraded state (daemon stopped, no daemon_health.json) must still report
    // the persisted cumulative cycle number, matching workboard()/current_work()
    // so the "Cycle #N" single-source invariant holds in every state, not just
    // when the daemon is live.
    let state = HermeticState::new();
    let xdg_root = state.state_root().join("xdg_data");
    let _xdg_guard = EnvGuard::set("XDG_DATA_HOME", &xdg_root);
    // Deliberately do NOT write daemon_health.json.

    let reports_dir = state.state_root().join("cycle_reports");
    std::fs::create_dir_all(&reports_dir).unwrap();
    std::fs::write(
        reports_dir.join("cycle_500.json"),
        serde_json::json!({"cycle_number": 500, "summary": "last before stop"}).to_string(),
    )
    .unwrap();

    let result = activity().await;
    assert_eq!(
        result.0["daemon"]["current_cycle"], 500,
        "with the daemon stopped, current_cycle must still show the persisted count"
    );
    assert_eq!(result.0["daemon"]["status"], "stopped");
}

// ---------------------------------------------------------------------------
// memory_metrics() — live memory-consolidation state (#26 FIX 2)
//
// The operator reported the memory-consolidation component "is always the
// same" even though consolidation IS actively running (~30 consolidate-memory
// actions / 30 min; episodic memory grew 756 -> 1088 today). The card was
// rendering a stale JSON-file mtime instead of the live consolidation state.
// These tests pin the live contract: `last_consolidation` tracks the newest
// live consolidate-memory OODA action, a `recent_consolidation_activity`
// {count,last} datum reports the live stream, and the episodic count reflects
// the live cognitive store — all of which must CHANGE as memory grows.
// ---------------------------------------------------------------------------

/// Registers a shared in-process cognitive-memory writer for the hermetic
/// state root — the tier-0 path `memory_metrics()` reads through — and clears
/// it on drop. Mirrors the MemGuard used by the journal tests.
struct MemGuard {
    writer: Arc<dyn CognitiveMemoryOps>,
}

impl MemGuard {
    fn register(state: &HermeticState) -> Self {
        let writer: Arc<dyn CognitiveMemoryOps> =
            Arc::new(LibraryCognitiveMemory::open(state.state_root()).expect("open store"));
        register_in_process_writer(state.state_root().to_path_buf(), Arc::clone(&writer));
        Self { writer }
    }
    fn ops(&self) -> &dyn CognitiveMemoryOps {
        self.writer.as_ref()
    }
}

impl Drop for MemGuard {
    fn drop(&mut self) {
        clear_in_process_writer();
    }
}

/// Write a `cycle_<n>.json` report under the hermetic state root carrying a
/// single `consolidate-memory` outcome stamped at `ts` (rfc3339). This is the
/// live OODA action stream the memory card must read for "last consolidation".
fn write_consolidation_cycle(state: &HermeticState, cycle: u64, ts: &str) {
    let reports_dir = state.state_root().join("cycle_reports");
    std::fs::create_dir_all(&reports_dir).unwrap();
    let report = serde_json::json!({
        "cycle_number": cycle,
        "timestamp": ts,
        "outcomes": [{
            "action_kind": "consolidate-memory",
            "action_description": "consolidated recent episodes into semantic memory",
            "success": true,
        }],
    });
    std::fs::write(
        reports_dir.join(format!("cycle_{cycle}.json")),
        report.to_string(),
    )
    .unwrap();
}

/// Compare two rfc3339 strings as instants so `+00:00` vs `Z` and any
/// parse/re-emit by the handler do not make the assertions brittle.
fn instant_eq(a: &str, b: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(a),
        chrono::DateTime::parse_from_rfc3339(b),
    ) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_last_consolidation_reflects_live_consolidate_action() {
    let state = HermeticState::new();

    // A legacy JSON snapshot exists with a *current* mtime — the stale source
    // the old code keyed off. The live consolidation timestamp is deliberately
    // in the past, so equality proves the value came from the live OODA action
    // stream, not the file's modification time.
    std::fs::write(state.state_root().join("memory_records.json"), "[]").unwrap();

    let ts = "2026-05-01T10:00:00+00:00";
    write_consolidation_cycle(&state, 100, ts);

    let result = memory_metrics().await;
    let last = result.0["last_consolidation"].as_str();
    assert!(
        last.is_some(),
        "last_consolidation must be populated from the live consolidate-memory \
         action, got null — the card would show 'Not tracked yet' while \
         consolidation is actively running (#26). value: {:?}",
        result.0["last_consolidation"]
    );
    assert!(
        instant_eq(last.unwrap(), ts),
        "last_consolidation must equal the live consolidate-memory action \
         timestamp ({ts}), NOT a stale JSON-file mtime — got {last:?}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_recent_consolidation_activity_is_reported() {
    let state = HermeticState::new();
    let ts = "2026-05-02T08:30:00+00:00";
    write_consolidation_cycle(&state, 200, ts);

    let result = memory_metrics().await;
    let activity = &result.0["recent_consolidation_activity"];
    assert!(
        activity.is_object(),
        "memory_metrics must expose a 'recent_consolidation_activity' object so \
         the card can show consolidation is actively running (#26) — got {activity}"
    );
    let count = activity["count"].as_u64();
    assert!(
        count.is_some_and(|c| c >= 1),
        "recent_consolidation_activity.count must reflect >=1 live \
         consolidate-memory action — got {:?}",
        activity["count"]
    );
    let last = activity["last"].as_str();
    assert!(
        last.is_some_and(|l| instant_eq(l, ts)),
        "recent_consolidation_activity.last must be the most recent live \
         consolidate-memory timestamp ({ts}) — got {last:?}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_consolidation_state_updates_as_memory_grows() {
    let state = HermeticState::new();

    let t1 = "2026-05-03T09:00:00+00:00";
    write_consolidation_cycle(&state, 300, t1);
    let first = memory_metrics().await;
    let last1 = first.0["last_consolidation"].as_str().map(str::to_string);
    let count1 = first.0["recent_consolidation_activity"]["count"]
        .as_u64()
        .unwrap_or(0);
    assert!(
        last1.is_some(),
        "first snapshot must record a live consolidation time"
    );
    assert!(
        count1 >= 1,
        "first snapshot must count the consolidate action"
    );

    // A newer consolidation cycle runs — the card must MOVE, proving it renders
    // live state rather than a frozen snapshot (the exact reported bug).
    let t2 = "2026-05-03T09:30:00+00:00";
    write_consolidation_cycle(&state, 301, t2);
    let second = memory_metrics().await;
    let last2 = second.0["last_consolidation"].as_str().map(str::to_string);
    let count2 = second.0["recent_consolidation_activity"]["count"]
        .as_u64()
        .unwrap_or(0);

    assert!(
        last2.as_deref().is_some_and(|l| instant_eq(l, t2)),
        "last_consolidation must advance to the newest consolidate action \
         ({t2}) — got {last2:?}"
    );
    assert!(
        !instant_eq(last1.as_deref().unwrap(), last2.as_deref().unwrap()),
        "last_consolidation MUST change when a newer consolidation runs — it \
         stayed constant ({last1:?}), which is exactly the stale-display bug (#26)"
    );
    assert!(
        count2 > count1,
        "recent_consolidation_activity.count must grow as more consolidate \
         actions occur — was {count1}, now {count2}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_episodic_count_reflects_live_store() {
    let state = HermeticState::new();
    let mem = MemGuard::register(&state);

    for i in 0..3 {
        mem.ops()
            .store_episode(&format!("live episode {i}"), "test", None)
            .expect("store episode");
    }

    let first = memory_metrics().await;
    let nm = &first.0["native_memory"];
    assert!(
        nm.is_object(),
        "native_memory must be populated from the live cognitive store — got {nm}"
    );
    let ep1 = nm["episodic"].as_u64().unwrap_or(0);
    assert!(
        ep1 >= 3,
        "episodic count must reflect the 3 live episodes just stored — got {ep1}"
    );

    // Grow memory; the panel must move with it (episodic 756 -> 1088 in the bug
    // report). A live source increases; a stale snapshot would not.
    for i in 3..6 {
        mem.ops()
            .store_episode(&format!("live episode {i}"), "test", None)
            .expect("store episode");
    }
    let second = memory_metrics().await;
    let ep2 = second.0["native_memory"]["episodic"].as_u64().unwrap_or(0);
    assert!(
        ep2 > ep1,
        "episodic count must increase as memory grows (live) — was {ep1}, now {ep2}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_consolidation_activity_never_leaks_raw_action_text() {
    // The memory card renders only a numeric count and RFC-3339 timestamps from
    // `recent_consolidation_activity` — never the raw action text. Even a cycle
    // report whose consolidate-memory outcome carries a hostile description must
    // not leak that string into the endpoint payload (#26). This backs the
    // no-unescaped-render property: the datum is a count + timestamp, full stop.
    let state = HermeticState::new();
    let reports_dir = state.state_root().join("cycle_reports");
    std::fs::create_dir_all(&reports_dir).unwrap();
    let ts = "2026-05-04T12:00:00+00:00";
    let hostile = "<script>alert('xss')</script>";
    let report = serde_json::json!({
        "cycle_number": 400,
        "timestamp": ts,
        "outcomes": [{
            "action_kind": "consolidate-memory",
            "action_description": hostile,
            "detail": hostile,
            "success": true,
        }],
    });
    std::fs::write(reports_dir.join("cycle_400.json"), report.to_string()).unwrap();

    let result = memory_metrics().await;
    let activity = &result.0["recent_consolidation_activity"];
    assert_eq!(
        activity["count"].as_u64(),
        Some(1),
        "the hostile consolidate-memory action must still be counted (numeric only)"
    );
    assert!(
        activity["last"]
            .as_str()
            .and_then(|l| chrono::DateTime::parse_from_rfc3339(l).ok())
            .is_some(),
        "recent_consolidation_activity.last must be a parseable RFC-3339 timestamp"
    );
    assert!(
        !activity.to_string().contains("<script>"),
        "recent_consolidation_activity must never carry raw action text — got {activity}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_consolidation_fails_closed_when_no_history() {
    // A brand-new state root has no consolidate-memory action yet. The card must
    // fail closed to `null` (→ "Not tracked yet") and report a zero activity
    // count — never a fabricated or frozen timestamp (#26). No legacy-JSON-mtime
    // or directory-mtime fallback is allowed.
    let state = HermeticState::new();

    // A legacy JSON snapshot with a *current* mtime exists — the old stale
    // source. It must NOT resurface as a consolidation time.
    std::fs::write(state.state_root().join("memory_records.json"), "[]").unwrap();

    let result = memory_metrics().await;
    assert!(
        result.0["last_consolidation"].is_null(),
        "last_consolidation must be null with no consolidate-memory history — \
         got {:?} (legacy-file mtime must not leak back in)",
        result.0["last_consolidation"]
    );
    let activity = &result.0["recent_consolidation_activity"];
    assert_eq!(
        activity["count"].as_u64(),
        Some(0),
        "recent_consolidation_activity.count must be 0 with no history — got {activity}"
    );
    assert!(
        activity["last"].is_null(),
        "recent_consolidation_activity.last must be null with no history — got {activity}"
    );
}
