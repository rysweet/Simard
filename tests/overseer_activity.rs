//! TDD contract tests for the Operator/Overseer recent-activity feed (#2419).
//!
//! These tests are written **before** the implementation (Step 7: tests-first).
//! They FAIL to compile today because `simard::overseer::activity` and the new
//! `StatusSnapshot.overseer` section do not exist yet; once the feature lands
//! they must compile and pass unchanged. They pin the public, cross-process
//! surface only — the durable JSON feed store and the status provider/render
//! reader — so the dashboard `serve` process and the TUI process (both distinct
//! from the daemon writer) can surface the same honest data.
//!
//! Crate-internal surfaces (the dashboard `/api/overseer` handler, the dashboard
//! "Overseer" tab HTML, and the TUI Overseer pane) are `pub(crate)` /
//! binary-internal and therefore cannot be reached from an integration test;
//! their tests-first specs are delivered as co-located `#[cfg(test)]` modules
//! (see the session artifacts referenced in the PR / Step 7 output) and land
//! alongside their implementation.
//!
//! Contract summary this file pins (implementer MUST match these names/shapes):
//!
//! * `simard::overseer::activity::{SCHEMA_VERSION, MAX_RECORDS}`
//! * `OverseerActivity { schema_version, enabled, cadence_secs, author_login,
//!    last_tick_at, totals, threads, recent }` (Serialize + Deserialize + Default)
//! * `OverseerActivityRecord { timestamp, enabled, report: OverseerTickReport }`
//! * `OverseerThreadStatus { id, enabled, last_run, next_due, last_success,
//!    consecutive_errors, backoff_until, health }`
//! * `OverseerTotals { problems, issues_filed, recipes_launched, prs_merged,
//!    deploys, escalations, held, errors }`
//! * `activity_path(&Path) -> PathBuf`  → `<state_root>/overseer/activity.json`
//! * `write_atomic(&Path, &OverseerActivity) -> io::Result<()>` (0600, tmp+rename)
//! * `read(&Path) -> Option<OverseerActivity>` (degrade-to-None, never panic)
//! * `OverseerActivity::push_record(&mut self, OverseerActivityRecord)`
//!   (front-push, cap `MAX_RECORDS`, refresh `last_tick_at` + recompute `totals`)
//! * `record_tick(&Path, OverseerActivityRecord, Vec<OverseerThreadStatus>,
//!   enabled: bool, cadence_secs: u64, author_login: &str)
//!   -> io::Result<OverseerActivity>` (durable append; thread-safe via atomic file)
//! * `simard::status::render::SECTION_HEADERS` contains `"OVERSEER"`
//! * `simard::status::StatusSnapshot.overseer: SectionEnvelope<OverseerActivity>`
//! * `simard::status::assemble` populates `.overseer` honestly from the feed.

use std::collections::VecDeque;
use std::path::Path;

use simard::overseer::OverseerTickReport;
use simard::overseer::activity::{
    self, MAX_RECORDS, OverseerActivity, OverseerActivityRecord, OverseerThreadStatus,
    SCHEMA_VERSION, record_tick,
};
use simard::status::provider::AssembleOptions;

// ─────────────────────────── test helpers ──────────────────────────────────

/// An `OverseerTickReport` with the given intervention tallies; other fields 0.
fn report(
    problems: usize,
    issues_filed: usize,
    recipes_launched: usize,
    prs_merged: usize,
    held: usize,
) -> OverseerTickReport {
    OverseerTickReport {
        problems,
        issues_filed,
        recipes_launched,
        prs_merged,
        held,
        duration_ms: 42,
        ..OverseerTickReport::default()
    }
}

/// A record stamped `now`, gate-enabled, wrapping `rep`.
fn record_now(rep: OverseerTickReport) -> OverseerActivityRecord {
    OverseerActivityRecord {
        timestamp: simard::telemetry::snapshot::now_rfc3339(),
        enabled: true,
        report: rep,
        problem_entries: Vec::new(),
    }
}

/// A record stamped `ts`, with an explicit `enabled` gate.
fn record_at(ts: &str, enabled: bool, rep: OverseerTickReport) -> OverseerActivityRecord {
    OverseerActivityRecord {
        timestamp: ts.to_string(),
        enabled,
        report: rep,
        problem_entries: Vec::new(),
    }
}

/// Set or clear `SIMARD_OVERSEER_ENABLED` for a serialized test.
///
/// SAFETY: every caller is annotated `#[serial_test::serial(overseer_env)]`, so
/// no other thread reads or writes this process-global env var concurrently —
/// which is exactly the soundness precondition Rust 2024 requires for
/// `std::env::set_var` / `remove_var`.
fn set_overseer_enabled(val: Option<&str>) {
    unsafe {
        match val {
            Some(v) => std::env::set_var("SIMARD_OVERSEER_ENABLED", v),
            None => std::env::remove_var("SIMARD_OVERSEER_ENABLED"),
        }
    }
}

/// RFC3339 timestamp `secs_ago` seconds before now.
fn rfc3339_secs_ago(secs_ago: i64) -> String {
    let t = chrono::Utc::now() - chrono::Duration::seconds(secs_ago);
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The single overseer meta-thread status, healthy and enabled.
fn overseer_thread_ok() -> OverseerThreadStatus {
    OverseerThreadStatus {
        id: "overseer".to_string(),
        enabled: true,
        last_run: Some(simard::telemetry::snapshot::now_rfc3339()),
        next_due: Some(rfc3339_secs_ago(-900)),
        last_success: Some(true),
        consecutive_errors: 0,
        backoff_until: None,
        health: "ok".to_string(),
    }
}

/// Build an `OverseerActivity` feed directly (bypassing the durable writer) so
/// freshness / honesty states can be constructed deterministically.
fn feed(
    enabled: bool,
    cadence_secs: u64,
    last_tick_at: Option<String>,
    threads: Vec<OverseerThreadStatus>,
    recent: Vec<OverseerActivityRecord>,
) -> OverseerActivity {
    let mut a = OverseerActivity {
        schema_version: SCHEMA_VERSION,
        enabled,
        cadence_secs,
        author_login: "simard-overseer[bot]".to_string(),
        last_tick_at,
        totals: Default::default(),
        threads,
        recent: VecDeque::new(),
    };
    for r in recent {
        a.push_record(r);
    }
    a
}

// ══════════════════════════ 1. store: constants ════════════════════════════

#[test]
fn schema_version_is_one_and_cap_is_hundred() {
    assert_eq!(SCHEMA_VERSION, 1, "feed schema starts at 1");
    assert_eq!(MAX_RECORDS, 100, "ring buffer retains the last 100 ticks");
}

// ══════════════════ 2. store: ring buffer bounded + newest-first ════════════

#[test]
fn push_record_caps_at_max_records_newest_first() {
    let mut a = feed(true, 900, None, vec![], vec![]);

    // Push 150 records; each carries a unique, monotonically increasing problem
    // count so ordering is observable.
    for i in 0..150usize {
        a.push_record(record_now(report(i, 0, 0, 0, 0)));
    }

    assert_eq!(
        a.recent.len(),
        MAX_RECORDS,
        "the ring must retain exactly the last {MAX_RECORDS} ticks, evicting oldest"
    );
    // Newest-first: front is the most recent push (problems == 149), back is the
    // oldest retained (problems == 50, because 0..=49 were evicted).
    assert_eq!(
        a.recent.front().unwrap().report.problems,
        149,
        "front of `recent` must be the newest tick"
    );
    assert_eq!(
        a.recent.back().unwrap().report.problems,
        50,
        "back of `recent` must be the oldest still-retained tick"
    );
}

#[test]
fn push_record_recomputes_totals_and_last_tick() {
    let mut a = feed(true, 900, None, vec![], vec![]);
    a.push_record(record_at(
        "2026-07-05T15:00:00Z",
        true,
        report(2, 1, 1, 0, 1),
    ));
    a.push_record(record_at(
        "2026-07-05T15:15:00Z",
        true,
        report(3, 2, 0, 1, 0),
    ));

    // Totals are the sum over currently-retained records.
    assert_eq!(a.totals.problems, 5);
    assert_eq!(a.totals.issues_filed, 3);
    assert_eq!(a.totals.recipes_launched, 1);
    assert_eq!(a.totals.prs_merged, 1);
    assert_eq!(a.totals.held, 1);
    // last_tick_at tracks the most recent record's timestamp.
    assert_eq!(a.last_tick_at.as_deref(), Some("2026-07-05T15:15:00Z"));
}

// ══════════════════ 3. store: durable file round-trip + degrade ═════════════

#[test]
fn activity_path_is_under_state_root_overseer_dir() {
    let root = Path::new("/tmp/some-state-root");
    let p = activity::activity_path(root);
    assert!(
        p.ends_with("overseer/activity.json"),
        "feed path must be <state_root>/overseer/activity.json, got {p:?}"
    );
    assert!(p.starts_with(root));
}

#[test]
fn write_atomic_then_read_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = activity::activity_path(dir.path());

    let original = feed(
        true,
        900,
        Some("2026-07-05T15:15:00Z".to_string()),
        vec![overseer_thread_ok()],
        vec![record_at(
            "2026-07-05T15:15:00Z",
            true,
            report(2, 1, 1, 0, 1),
        )],
    );

    activity::write_atomic(&path, &original).expect("atomic write must succeed");
    let read_back = activity::read(&path).expect("readable feed must deserialize");

    assert_eq!(
        read_back, original,
        "write→read must be an identity round-trip"
    );
}

#[test]
fn read_missing_file_is_none_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = activity::activity_path(dir.path());
    assert!(
        activity::read(&path).is_none(),
        "a missing feed file degrades to None (honest 'no data'), never panics"
    );
}

#[test]
fn read_corrupt_file_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = activity::activity_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{ this is not valid json ]]").unwrap();
    assert!(
        activity::read(&path).is_none(),
        "a corrupt feed file degrades to None, never panics"
    );
}

#[test]
fn write_atomic_never_leaves_world_readable_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = activity::activity_path(dir.path());
    let a = feed(true, 900, None, vec![], vec![]);
    activity::write_atomic(&path, &a).unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "feed file must be private (0600), got {mode:o}"
    );
}

// ══════════════════ 4. store: durable writer (record_tick) ══════════════════

#[test]
fn record_tick_appends_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let a1 = record_tick(
        root,
        record_now(report(1, 1, 0, 0, 0)),
        vec![overseer_thread_ok()],
        true,
        900,
        "simard-overseer[bot]",
    )
    .expect("record_tick writes the feed");
    assert_eq!(a1.recent.len(), 1);
    assert!(a1.enabled);
    assert_eq!(a1.cadence_secs, 900);
    assert_eq!(a1.author_login, "simard-overseer[bot]");

    // A second tick appends to the persisted feed (read-modify-write).
    let a2 = record_tick(
        root,
        record_now(report(0, 0, 1, 0, 0)),
        vec![overseer_thread_ok()],
        true,
        900,
        "simard-overseer[bot]",
    )
    .expect("second record_tick appends");
    assert_eq!(
        a2.recent.len(),
        2,
        "record_tick must accumulate across ticks"
    );

    // The durable file reflects the latest state.
    let on_disk = activity::read(&activity::activity_path(root)).unwrap();
    assert_eq!(on_disk.recent.len(), 2);
}

#[test]
fn record_tick_is_bounded_and_thread_safe() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // 8 concurrent writers × 30 ticks each = 240 attempted appends onto a
    // 100-slot ring. Concurrency is last-writer-wins (no lock), but the atomic
    // tmp+rename guarantees the file is NEVER torn: every read yields a valid,
    // bounded feed.
    let mut handles = Vec::new();
    for w in 0..8u32 {
        let root = root.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..30usize {
                let _ = record_tick(
                    &root,
                    record_now(report(w as usize, i, 0, 0, 0)),
                    vec![overseer_thread_ok()],
                    true,
                    900,
                    "simard-overseer[bot]",
                );
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let feed = activity::read(&activity::activity_path(&root))
        .expect("after concurrent writers the feed must still be a valid, readable file");
    assert!(
        feed.recent.len() <= MAX_RECORDS,
        "feed must stay bounded at ≤{MAX_RECORDS} under concurrent writers, got {}",
        feed.recent.len()
    );
}

// ══════════════════ 5. serde: record + tick report ══════════════════════════

#[test]
fn overseer_tick_report_is_serializable() {
    // The feature adds `#[derive(Serialize, Deserialize)]` to OverseerTickReport
    // (additive; no logic change). This asserts the derive is present.
    let rep = report(2, 1, 1, 0, 1);
    let v = serde_json::to_value(rep).expect("OverseerTickReport must be Serialize");
    assert_eq!(v["problems"], 2);
    assert_eq!(v["issues_filed"], 1);
    assert_eq!(v["recipes_launched"], 1);
    assert_eq!(v["held"], 1);
    assert_eq!(v["panicked"], false);
}

#[test]
fn activity_record_json_round_trips() {
    let rec = record_at("2026-07-05T15:15:00Z", true, report(3, 2, 1, 1, 0));
    let json = serde_json::to_string(&rec).unwrap();
    let back: OverseerActivityRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back, rec);
    assert_eq!(back.timestamp, "2026-07-05T15:15:00Z");
    assert_eq!(back.report.issues_filed, 2);
}

// ══════════════════ 6. provider: honest states via assemble ═════════════════
//
// These go through the real `status::assemble`, reading a hermetic state root.
// They mutate the process env (`SIMARD_OVERSEER_ENABLED`) so they are serialized.

#[test]
#[serial_test::serial(overseer_env)]
fn assemble_surfaces_live_feed_when_enabled_and_fresh() {
    set_overseer_enabled(None); // default = enabled (opt-out)
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let f = feed(
        true,
        900,
        Some(simard::telemetry::snapshot::now_rfc3339()),
        vec![overseer_thread_ok()],
        vec![record_now(report(2, 1, 1, 0, 1))],
    );
    activity::write_atomic(&activity::activity_path(root), &f).unwrap();

    let snap = simard::status::assemble(&AssembleOptions::with_state_root(root.to_path_buf()));
    assert!(
        snap.overseer.is_present(),
        "a fresh, enabled feed must yield a present overseer section"
    );
    let data = snap
        .overseer
        .data
        .as_ref()
        .expect("live section carries data");
    assert!(data.enabled, "data.enabled reflects the gate = true");
    assert_eq!(data.recent.len(), 1);
    assert!(
        data.threads.iter().any(|t| t.id == "overseer"),
        "the overseer meta-thread must be listed"
    );
}

#[test]
#[serial_test::serial(overseer_env)]
fn assemble_reports_disabled_state_honestly() {
    // Disabled is a PRESENT state (data.enabled = false), distinct from "no data".
    set_overseer_enabled(Some("0"));
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Even with no feed file, a disabled overseer must be reported plainly.
    let snap = simard::status::assemble(&AssembleOptions::with_state_root(root.to_path_buf()));
    set_overseer_enabled(None);

    let rendered = simard::status::render::to_terminal(&snap);
    assert!(
        rendered.to_lowercase().contains("disabled"),
        "a disabled overseer must render the word 'disabled', got:\n{rendered}"
    );
}

#[test]
#[serial_test::serial(overseer_env)]
fn assemble_reports_enabled_observing_zero_interventions_honestly() {
    set_overseer_enabled(None);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Enabled, has ticked, but took zero interventions (all tallies 0).
    let f = feed(
        true,
        900,
        Some(simard::telemetry::snapshot::now_rfc3339()),
        vec![overseer_thread_ok()],
        vec![record_now(report(0, 0, 0, 0, 0))],
    );
    activity::write_atomic(&activity::activity_path(root), &f).unwrap();

    let snap = simard::status::assemble(&AssembleOptions::with_state_root(root.to_path_buf()));
    let rendered = simard::status::render::to_terminal(&snap);
    assert!(
        rendered.contains("0 interventions"),
        "an enabled-but-idle overseer must render 'observing, 0 interventions', got:\n{rendered}"
    );
}

#[test]
#[serial_test::serial(overseer_env)]
fn assemble_marks_stale_when_last_tick_is_old() {
    set_overseer_enabled(None);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // last_tick_at 1 hour ago with a 900s cadence: 3600 > 2×900 ⇒ stale.
    let old = rfc3339_secs_ago(3600);
    let f = feed(
        true,
        900,
        Some(old.clone()),
        vec![overseer_thread_ok()],
        vec![record_at(&old, true, report(1, 0, 0, 0, 0))],
    );
    activity::write_atomic(&activity::activity_path(root), &f).unwrap();

    let snap = simard::status::assemble(&AssembleOptions::with_state_root(root.to_path_buf()));
    assert_eq!(
        snap.overseer.freshness,
        simard::status::Freshness::Stale,
        "a feed older than 2×cadence must be marked stale"
    );
}

#[test]
#[serial_test::serial(overseer_env)]
fn assemble_absent_when_enabled_but_never_ticked() {
    set_overseer_enabled(None);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // No feed file written at all.
    let snap = simard::status::assemble(&AssembleOptions::with_state_root(root.to_path_buf()));
    assert!(
        !snap.overseer.is_present(),
        "with no feed file the overseer section must be absent (no data), honestly"
    );
    assert_eq!(snap.overseer.freshness, simard::status::Freshness::Absent);
}

// ══════════════════ 7. render: OVERSEER section headers ═════════════════════

#[test]
fn section_headers_include_overseer() {
    assert!(
        simard::status::render::SECTION_HEADERS.contains(&"OVERSEER"),
        "the canonical render must add an OVERSEER section header"
    );
}

#[test]
#[serial_test::serial(overseer_env)]
fn render_shows_overseer_header_and_thread_row() {
    set_overseer_enabled(None);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let f = feed(
        true,
        900,
        Some(simard::telemetry::snapshot::now_rfc3339()),
        vec![overseer_thread_ok()],
        vec![record_now(report(2, 1, 1, 0, 1))],
    );
    activity::write_atomic(&activity::activity_path(root), &f).unwrap();

    let snap = simard::status::assemble(&AssembleOptions::with_state_root(root.to_path_buf()));
    let rendered = simard::status::render::to_terminal(&snap);
    assert!(
        rendered.contains("OVERSEER"),
        "must render the OVERSEER header"
    );
    assert!(
        rendered.contains("overseer"),
        "must render the overseer thread row (thread id)"
    );
}
