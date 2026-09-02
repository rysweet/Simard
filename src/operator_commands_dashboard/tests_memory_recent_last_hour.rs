//! File: src/operator_commands_dashboard/tests_memory_recent_last_hour.rs
//!
//! TDD regression pins for issue #2679 — the Memory dashboard reporting
//! "Simard has remembered 0 items in the last hour" while long-term memory is
//! actively growing.
//!
//! Root cause (confirmed in `memory.rs`): `memory_recent()` returns a hardcoded
//! literal `"last_hour_count": 0` (line 251) that never queries the trailing
//! hour. These tests pin the CORRECT behaviour so the placeholder cannot
//! silently return:
//!
//!   1. `memory_recent_at(state_root)` — an env-free, testable core (mirroring
//!      the `goals()` -> `goals_at()` split) — must compute
//!      `last_hour_count = max(0, live_long_term_total − baseline_long_term_total)`
//!      from the LIVE shared reader, not a literal.
//!   2. `select_last_hour_baseline(history, now_secs)` — a pure helper — must
//!      pin the trailing-hour window edge (`epoch_secs <= now − 3600`,
//!      at-or-before), the most-recent-such selection, and the earliest-snapshot
//!      fallback, so an off-by-one window boundary (cause d) cannot regress.
//!
//! These tests reference `memory_recent_at` and `select_last_hour_baseline`,
//! which do NOT exist yet — the file is expected to FAIL TO COMPILE until the
//! implementation lands. That compile failure IS the initial RED state.
//!
//! Contract note for the implementation: `select_last_hour_baseline` MUST be at
//! least `pub(crate)` so this sibling test module can drive it directly (the
//! surrounding ring-buffer helpers — `load_history`, `save_history`,
//! `compute_deltas` — are already `pub(crate)`, so this is consistent).

use std::sync::Arc;

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::memory_ipc::{
    clear_in_process_writer, clear_tier2_store_cache, register_in_process_writer, socket_path_for,
};
use crate::operator_commands_dashboard::memory::{
    MemorySnapshot, memory_recent_at, save_history, select_last_hour_baseline,
    select_last_hour_baseline_snapshot,
};
use crate::test_support::HermeticState;

/// Number of long-term memory items written inside the trailing hour by the
/// integration tests. The dashboard MUST report exactly this many — never the
/// old hardcoded `0`.
const N_IN_WINDOW: u64 = 5;

/// The trailing-hour window, in seconds. Duplicated locally (not imported) so
/// the test asserts the intended 3600 s contract independently of the
/// production constant — if someone silently changes the window, these tests
/// still describe what "last hour" is supposed to mean.
const ONE_HOUR_SECS: f64 = 3600.0;

// ---------------------------------------------------------------------------
// Shared tier-0 writer guard (mirrors tests_goals_crud::SharedMemoryGuard)
// ---------------------------------------------------------------------------

/// Registers ONE shared `LibraryCognitiveMemory` handle as the tier-0
/// in-process writer for the life of a test and clears the global registration
/// on drop (panic-safe).
///
/// Production wires the dashboard against a single shared cognitive-memory
/// handle registered via [`register_in_process_writer`]; `open_reader_client`
/// (which `memory_recent_at` uses) consults that tier-0 registry FIRST, so
/// reads issued by the handler observe writes made through `ops()` on the very
/// same handle. Registering here keeps the test on the production read path.
struct SharedMemoryGuard {
    writer: Arc<dyn CognitiveMemoryOps>,
}

impl SharedMemoryGuard {
    fn register(state: &HermeticState) -> Self {
        let writer: Arc<dyn CognitiveMemoryOps> = Arc::new(
            LibraryCognitiveMemory::open(state.state_root()).expect("open shared cognitive memory"),
        );
        register_in_process_writer(state.state_root().to_path_buf(), Arc::clone(&writer));
        Self { writer }
    }

    fn ops(&self) -> &dyn CognitiveMemoryOps {
        self.writer.as_ref()
    }
}

impl Drop for SharedMemoryGuard {
    fn drop(&mut self) {
        clear_in_process_writer();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Live long-term total = episodic + semantic + procedural + prospective,
/// exactly what "remembered" (consolidated long-term memory) means and what
/// `MemorySnapshot::from_stats` records as `long_term_total`.
fn live_long_term_total(ops: &dyn CognitiveMemoryOps) -> u64 {
    let s = ops.get_statistics().expect("get_statistics");
    s.episodic_count + s.semantic_count + s.procedural_count + s.prospective_count
}

/// Build a `MemorySnapshot` at `epoch_secs` carrying `long_term_total`. Only
/// the timestamp/epoch and the long-term total matter for baseline selection.
fn snapshot(epoch_secs: f64, long_term_total: u64) -> MemorySnapshot {
    MemorySnapshot {
        timestamp: String::new(),
        epoch_secs,
        sensory: 0,
        working: 0,
        episodic: 0,
        semantic: 0,
        procedural: 0,
        prospective: 0,
        total: long_term_total,
        long_term_total,
    }
}

/// Write `long_term_total` into a freshly-seeded `memory_history.json` baseline
/// dated `age_secs` before now, so the handler's trailing-hour delta has a
/// deterministic reference point.
fn seed_history_baseline(state: &HermeticState, age_secs: f64, long_term_total: u64) {
    let now = chrono::Utc::now().timestamp() as f64;
    let baseline = snapshot(now - age_secs, long_term_total);
    save_history(&state.state_root().join("memory_history.json"), &[baseline]);
}

// ---------------------------------------------------------------------------
// Integration: the headline regression pin
// ---------------------------------------------------------------------------

/// GIVEN N long-term items written within the trailing hour, the dashboard MUST
/// report `last_hour_count == N` — NOT the old hardcoded `0`. This is the core
/// #2679 pin: it fails against the placeholder implementation and passes once
/// `memory_recent_at` computes the live delta.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_recent_reports_n_items_remembered_in_last_hour() {
    let state = HermeticState::new();
    let guard = SharedMemoryGuard::register(&state);

    // Baseline long-term total BEFORE the in-window writes.
    let t0 = live_long_term_total(guard.ops());

    // Seed a baseline snapshot comfortably OLDER than the window edge
    // (61 min ago) recording that pre-write total, so the trailing-hour
    // baseline is unambiguously `t0` regardless of ms-level clock drift.
    seed_history_baseline(&state, ONE_HOUR_SECS + 60.0, t0);

    // Write N distinct long-term items inside the trailing hour.
    for i in 0..N_IN_WINDOW {
        guard
            .ops()
            .store_fact(
                &format!("last-hour-concept-{i}"),
                &format!("in-window fact {i}"),
                1.0,
                &[] as &[String],
                "tdd-2679",
            )
            .expect("store_fact must persist a new semantic (long-term) node");
    }

    // Precondition: the live long-term total actually grew by exactly N.
    assert_eq!(
        live_long_term_total(guard.ops()),
        t0 + N_IN_WINDOW,
        "precondition: {N_IN_WINDOW} distinct store_fact calls must raise the \
         live long-term total by {N_IN_WINDOW}",
    );

    // Exercise the handler core against the live shared reader.
    let resp = memory_recent_at(state.state_root()).await;
    let val = &resp.0;

    assert_eq!(
        val["last_hour_count"],
        serde_json::json!(N_IN_WINDOW),
        "dashboard MUST report {N_IN_WINDOW} items remembered in the last hour \
         (net long-term growth), not the old hardcoded 0: {val}",
    );
    assert!(
        val.get("error").is_none(),
        "a healthy live read must NOT fail closed: {val}",
    );

    // Back-compat: existing response fields are preserved.
    assert_eq!(
        val["total"],
        serde_json::json!(t0 + N_IN_WINDOW),
        "`total` must stay the live aggregate stored count: {val}",
    );
    // Per-item recent listing now works on the library backend: the same reader
    // that backs /api/memory/graph enumerates episodes. This test writes only
    // semantic facts (no episodes), so the recent-episode feed is empty — but
    // the capability is available, so `available` is true and `items` is an
    // empty array (not the retired always-`false` stub).
    assert_eq!(
        val["available"],
        serde_json::json!(true),
        "per-item recent listing is now available on the library backend: {val}",
    );
    assert!(val["items"].is_array(), "`items` must be an array: {val}",);
    assert_eq!(
        val["items"].as_array().map(|a| a.len()),
        Some(0),
        "no episodes were written, so the recent-episode feed is empty: {val}",
    );
    assert!(val["note"].is_string(), "`note` must be preserved: {val}");
    assert!(
        val["server_time"].is_string(),
        "`server_time` must be preserved: {val}",
    );
}

/// A pruning-dominated interval (live long-term total ends BELOW the one-hour
/// baseline) MUST clamp to `0` — you cannot "remember a negative count" — and
/// MUST NOT underflow a `u64` into a huge number. Pins resolution A4.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_recent_clamps_net_negative_interval_to_zero() {
    let state = HermeticState::new();
    let guard = SharedMemoryGuard::register(&state);

    let live_lt = live_long_term_total(guard.ops());

    // Seed a baseline whose long-term total is HIGHER than the current live
    // total, simulating an hour where pruning/consolidation removed more than
    // it added. The naive delta is negative.
    seed_history_baseline(&state, ONE_HOUR_SECS + 60.0, live_lt + 1_000);

    let resp = memory_recent_at(state.state_root()).await;
    let val = &resp.0;

    assert_eq!(
        val["last_hour_count"],
        serde_json::json!(0),
        "a net-negative (pruning-dominated) interval must clamp to 0, never \
         underflow into a huge count: {val}",
    );
}

/// With NO snapshot history at all (cold start / sub-hour uptime) and no items
/// yet, the baseline falls back to the live total, so the delta is `0` — an
/// HONEST zero, not the hardcoded placeholder. The endpoint must still return a
/// well-formed, back-compatible payload with a numeric `last_hour_count`.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_recent_cold_start_reports_honest_zero_not_placeholder() {
    let state = HermeticState::new();
    let _guard = SharedMemoryGuard::register(&state);
    // Intentionally seed NO memory_history.json.

    let resp = memory_recent_at(state.state_root()).await;
    let val = &resp.0;

    assert!(
        val["last_hour_count"].is_u64(),
        "cold start must still return a numeric last_hour_count: {val}",
    );
    assert_eq!(
        val["last_hour_count"],
        serde_json::json!(0),
        "cold start with no writes must read an honest 0 (live − live): {val}",
    );
    assert!(
        val.get("error").is_none(),
        "cold start is not an error condition: {val}",
    );
    assert!(val["total"].is_u64(), "`total` must be present: {val}");
}

/// Fail-closed contract (resolution A7 / acceptance #2): when the live read
/// cannot be served — here, a present-but-unconnectable memory socket, exactly
/// the #2896 divergent-reader hazard — the endpoint MUST surface an `error` and
/// MUST NOT emit a misleading `0`. It reports `last_hour_count: null` so the
/// frontend renders `—` rather than implying memory is idle.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_recent_fails_closed_on_unreadable_store() {
    // No tier-0 writer: force the read down the socket path.
    clear_in_process_writer();
    let state = HermeticState::new();

    // Place a regular file where the daemon socket would live: present but
    // unconnectable, so `open_reader_client` fails closed (bug #2896) instead
    // of silently opening a divergent, empty tier-2 store.
    let sock = socket_path_for(state.state_root());
    if let Some(parent) = sock.parent() {
        std::fs::create_dir_all(parent).expect("create socket parent dir");
    }
    std::fs::write(&sock, b"not a socket").expect("write placeholder at socket path");

    let resp = memory_recent_at(state.state_root()).await;
    clear_tier2_store_cache();
    let val = &resp.0;

    assert!(
        val.get("error").is_some(),
        "an unreadable store MUST surface an error, not a silent 0: {val}",
    );
    assert!(
        val["last_hour_count"].is_null(),
        "on read failure last_hour_count MUST be null (frontend shows '—'), \
         never a misleading 0: {val}",
    );
}

// ---------------------------------------------------------------------------
// Integration: per-item recent-episode feed (the #1997 "Recent Memories" panel)
// ---------------------------------------------------------------------------

/// Recent episodes MUST surface as `items` in the frontend's expected shape
/// (`category`/`summary`/`timestamp`), newest-first. Pins the fix that replaced
/// the always-empty `items:[]` + `available:false` stub — the Memory tab's
/// "Recent Memories" panel could never show anything even while thousands of
/// episodes were stored.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_recent_lists_recent_episodes_newest_first() {
    let state = HermeticState::new();
    let guard = SharedMemoryGuard::register(&state);

    // Store three distinct episodes in order; `list_all_episodes` returns them
    // newest-first, so the last stored ("gamma") must appear first.
    for content in ["alpha episode", "beta episode", "gamma episode"] {
        guard
            .ops()
            .store_episode(content, "tdd-recent-items", None)
            .expect("store_episode must persist a new episodic node");
    }

    let resp = memory_recent_at(state.state_root()).await;
    let val = &resp.0;

    assert!(
        val.get("error").is_none(),
        "a healthy live read must NOT fail closed: {val}",
    );
    assert_eq!(
        val["available"],
        serde_json::json!(true),
        "per-item recent listing is available once episodes exist: {val}",
    );

    let items = val["items"].as_array().expect("items must be an array");
    assert_eq!(
        items.len(),
        3,
        "all three stored episodes must be listed: {val}",
    );

    // Newest-first: the most recently stored episode is item 0.
    assert_eq!(
        items[0]["summary"].as_str(),
        Some("gamma episode"),
        "episodes must be newest-first (gamma stored last): {val}",
    );
    assert_eq!(
        items[0]["category"].as_str(),
        Some("Past event"),
        "episodes render under the 'Past event' category the frontend colors: {val}",
    );
    // Every item carries the frontend-required fields.
    for item in items {
        assert!(
            item["summary"].is_string(),
            "each item needs a `summary`: {item}",
        );
        assert!(
            item.get("category").and_then(|c| c.as_str()) == Some("Past event"),
            "each item needs the 'Past event' category: {item}",
        );
        // `timestamp` now carries the episode's real `created_at` as an RFC3339
        // instant (issue #4383): the library backend records a wall-clock time,
        // so the frontend can render a "time ago" label. The key must be present
        // and hold a parseable RFC3339 timestamp at or near "now".
        assert!(
            item.get("timestamp").is_some(),
            "each item must carry a `timestamp` key: {item}",
        );
        let ts = item["timestamp"]
            .as_str()
            .unwrap_or_else(|| panic!("library-backed episodes carry a real timestamp: {item}"));
        let parsed = chrono::DateTime::parse_from_rfc3339(ts)
            .unwrap_or_else(|e| panic!("`timestamp` must be RFC3339 ({e}): {item}"))
            .with_timezone(&chrono::Utc);
        let age = chrono::Utc::now()
            .signed_duration_since(parsed)
            .num_seconds();
        assert!(
            (-5..3600).contains(&age),
            "episode `timestamp` must be a recent wall-clock instant (age={age}s): {item}",
        );
    }
}

/// The recent-episode feed is bounded: even with more episodes than the cap,
/// the endpoint returns at most `RECENT_ITEMS_MAX` items so the panel stays a
/// glance, not an unbounded dump.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_recent_bounds_item_count() {
    let state = HermeticState::new();
    let guard = SharedMemoryGuard::register(&state);

    // Store comfortably more episodes than the cap (25).
    for i in 0..40 {
        guard
            .ops()
            .store_episode(&format!("episode {i}"), "tdd-recent-cap", None)
            .expect("store_episode must persist");
    }

    let resp = memory_recent_at(state.state_root()).await;
    let val = &resp.0;

    let items = val["items"].as_array().expect("items must be an array");
    assert!(
        items.len() <= 25,
        "recent-item feed must be capped at 25, got {}: {val}",
        items.len(),
    );
    assert!(
        !items.is_empty(),
        "with 40 episodes stored the feed must not be empty: {val}",
    );
}

/// A snapshot EXACTLY at the window edge (`now − 3600`) is "at-or-before" and
/// MUST be selected as the baseline. Locks the `<= cutoff` boundary so an
/// off-by-one (cause d) cannot slip in.
#[test]
fn baseline_selects_snapshot_exactly_at_window_edge() {
    let now = 1_000_000.0;
    let history = vec![snapshot(now - ONE_HOUR_SECS, 42)];
    assert_eq!(
        select_last_hour_baseline(&history, now),
        Some(42),
        "snapshot at exactly now-3600 must be counted as the one-hour baseline",
    );
}

/// A snapshot one second INSIDE the window (`now − 3599`) is NOT a valid
/// "one hour ago" baseline; with an older snapshot present, that older one
/// (`now − 7200`) must be chosen instead.
#[test]
fn baseline_excludes_snapshot_just_inside_the_window() {
    let now = 1_000_000.0;
    let history = vec![
        snapshot(now - 7200.0, 10), // 2h ago — the only valid baseline
        snapshot(now - 3599.0, 99), // inside the window — must be ignored
    ];
    assert_eq!(
        select_last_hour_baseline(&history, now),
        Some(10),
        "a snapshot at now-3599 is inside the last hour and must not be the baseline",
    );
}

/// Among multiple at-or-before-edge snapshots, the MOST RECENT one wins so the
/// baseline is the tightest available approximation of "one hour ago".
#[test]
fn baseline_picks_most_recent_at_or_before_edge() {
    let now = 1_000_000.0;
    let history = vec![
        snapshot(now - 10_800.0, 5),       // 3h ago
        snapshot(now - 7_200.0, 20),       // 2h ago
        snapshot(now - ONE_HOUR_SECS, 40), // exactly 1h ago — most recent <= cutoff
        snapshot(now - 60.0, 99),          // in-window
    ];
    assert_eq!(
        select_last_hour_baseline(&history, now),
        Some(40),
        "the most-recent snapshot at-or-before now-3600 must be the baseline",
    );
}

/// When ALL snapshots are inside the hour (sub-hour uptime), fall back to the
/// EARLIEST snapshot — an honest partial-window under-count rather than a
/// placeholder.
#[test]
fn baseline_falls_back_to_earliest_when_all_within_hour() {
    let now = 1_000_000.0;
    let history = vec![
        snapshot(now - 600.0, 100), // 10 min ago — earliest
        snapshot(now - 120.0, 130), // 2 min ago
    ];
    assert_eq!(
        select_last_hour_baseline(&history, now),
        Some(100),
        "with only sub-hour history, fall back to the earliest snapshot",
    );
}

/// Empty history has no baseline. The handler treats `None` as "use the live
/// total" (delta 0); the helper itself must simply report absence.
#[test]
fn baseline_is_none_for_empty_history() {
    assert_eq!(
        select_last_hour_baseline(&[], 1_000_000.0),
        None,
        "empty history has no baseline snapshot",
    );
}

// ---------------------------------------------------------------------------
// #4318: honest last-hour WINDOW — surface the actual span the count covers
// ---------------------------------------------------------------------------

/// `select_last_hour_baseline_snapshot` MUST return the SAME snapshot the scalar
/// `select_last_hour_baseline` reduces to a `long_term_total`, and expose its
/// `epoch_secs` so the handler can compute the true covered window. Guards the
/// contract that the snapshot- and scalar-returning helpers never diverge.
#[test]
fn baseline_snapshot_matches_scalar_and_exposes_epoch() {
    let now = 1_000_000.0;
    let history = vec![
        snapshot(now - 10_800.0, 5), // 3h ago
        snapshot(now - 9_360.0, 12), // 2.6h ago — most recent <= cutoff
        snapshot(now - 60.0, 99),    // in-window — ignored
    ];
    let snap = select_last_hour_baseline_snapshot(&history, now)
        .expect("a snapshot older than the window edge exists");
    assert_eq!(
        snap.long_term_total,
        select_last_hour_baseline(&history, now).unwrap(),
        "snapshot- and scalar-returning helpers must agree on the baseline total",
    );
    assert_eq!(
        snap.epoch_secs,
        now - 9_360.0,
        "the exposed baseline epoch must be the most-recent snapshot at-or-before now-3600",
    );
    // The window the handler would report is now − baseline.epoch = 9360s (2.6h).
    assert_eq!(now - snap.epoch_secs, 9_360.0);
}

/// Empty history has no baseline snapshot — the helper reports absence so the
/// handler renders `last_hour_window_secs: null` in the fail-closed branches.
#[test]
fn baseline_snapshot_is_none_for_empty_history() {
    assert!(
        select_last_hour_baseline_snapshot(&[], 1_000_000.0).is_none(),
        "empty history has no baseline snapshot",
    );
}

/// THE #4318 REGRESSION PIN. When `memory_history.json` has a gap wider than an
/// hour straddling the 1 h mark, the chosen baseline is ~2.6 h old, so
/// `last_hour_count` is net growth over 2.6 h — NOT one hour. The endpoint MUST
/// surface `last_hour_window_secs ≈ 9360` so the caption can tell the truth
/// ("in the last 2.6h") instead of the hardcoded, dishonest "in the last hour".
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_recent_surfaces_true_window_when_snapshots_are_sparse() {
    let state = HermeticState::new();
    let guard = SharedMemoryGuard::register(&state);

    let t0 = live_long_term_total(guard.ops());

    // Seed a single baseline 2.6 h old recording the pre-write total. There is
    // NO snapshot near the 1 h mark, so the trailing-hour baseline is this
    // 2.6 h-old one — the exact sparse-history hazard #4318 describes.
    let gap_secs = 9_360.0; // 2.6h
    seed_history_baseline(&state, gap_secs, t0);

    // Grow long-term memory by N inside that (over-wide) window.
    for i in 0..N_IN_WINDOW {
        guard
            .ops()
            .store_fact(
                &format!("wide-window-concept-{i}"),
                &format!("fact {i}"),
                1.0,
                &[] as &[String],
                "tdd-4318",
            )
            .expect("store_fact must persist a new long-term node");
    }

    let resp = memory_recent_at(state.state_root()).await;
    let val = &resp.0;

    // The count is the net growth over the WHOLE 2.6h window.
    assert_eq!(
        val["last_hour_count"],
        serde_json::json!(N_IN_WINDOW),
        "count is net long-term growth since the (2.6h-old) baseline: {val}",
    );

    // The endpoint must now DISCLOSE that the covered window is ~2.6h, not 1h.
    let window = val["last_hour_window_secs"]
        .as_f64()
        .unwrap_or_else(|| panic!("last_hour_window_secs must be numeric: {val}"));
    assert!(
        (9_300.0..=9_500.0).contains(&window),
        "the covered window must reflect the true ~9360s (2.6h) baseline age, \
         not be silently labeled 'last hour'; got {window}: {val}",
    );
    assert!(
        window > 3600.0 + 900.0,
        "the window must be materially wider than an hour (the #4318 defect): {val}",
    );
}

/// In steady state (a baseline snapshot ~1 h old), the window is within the
/// ±15 min "reads as an hour" tolerance, so the endpoint reports ~3600s and the
/// caption legitimately stays "in the last hour".
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_recent_reports_about_one_hour_window_in_steady_state() {
    let state = HermeticState::new();
    let guard = SharedMemoryGuard::register(&state);

    let t0 = live_long_term_total(guard.ops());
    // Baseline 61 min old — comfortably past the window edge but within the
    // ±15 min "last hour" tolerance.
    seed_history_baseline(&state, ONE_HOUR_SECS + 60.0, t0);

    let resp = memory_recent_at(state.state_root()).await;
    let val = &resp.0;

    let window = val["last_hour_window_secs"]
        .as_f64()
        .unwrap_or_else(|| panic!("last_hour_window_secs must be numeric: {val}"));
    assert!(
        (3600.0..=3720.0).contains(&window),
        "a ~61-min-old baseline must yield a ~3660s window: got {window}: {val}",
    );
    assert!(
        (window - 3600.0).abs() <= 900.0,
        "the steady-state window must fall inside the ±15 min 'last hour' band: {val}",
    );
}

/// Cold start (no seeded history): the handler seeds a fresh snapshot at `now`,
/// so the baseline is that same instant and the window is a small, honest
/// near-zero — a numeric value, never the misleading fixed 3600. This documents
/// that the field degrades gracefully rather than fabricating a full hour.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_recent_window_is_small_on_cold_start() {
    let state = HermeticState::new();
    let _guard = SharedMemoryGuard::register(&state);
    // No memory_history.json seeded.

    let resp = memory_recent_at(state.state_root()).await;
    let val = &resp.0;

    let window = val["last_hour_window_secs"]
        .as_f64()
        .unwrap_or_else(|| panic!("cold start must still return a numeric window: {val}"));
    assert!(
        (0.0..=5.0).contains(&window),
        "cold start seeds a now-snapshot, so the covered window is ~0s, not a \
         fabricated 3600: got {window}: {val}",
    );
}
