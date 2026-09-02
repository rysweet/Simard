//! TDD (RED) tests for the recurring **monthly self-quality-audit** periodic
//! task (goal: reuse the brain-introspection / disk-health periodic-task
//! pattern, issue [#2419](https://github.com/rysweet/Simard/issues/2419)).
//!
//! Written **before** the production code. Expected to FAIL until
//! `crate::self_quality_audit` (the module) and
//! `prompt_assets/simard/recipes/monthly-self-quality-audit.yaml` land. The
//! unresolved `crate::self_quality_audit::…` import is the intended,
//! deterministic RED signal under `cargo test`.
//!
//! `cargo build` / `cargo build --release` stay GREEN — this module is
//! `#[cfg(test)]`, so the not-yet-existing references are never compiled into
//! the library or the daemon binary (mirrors `brain_introspection_tests.rs`).
//!
//! ## Contract pinned by these tests
//!
//! This periodic task is modeled on [`crate::disk_health`] (a **pure recipe
//! invoker**, no memory RPCs) with **one novel capability**: it persists its
//! last-run wall-clock timestamp to disk so the ~30-day cadence **survives
//! daemon restarts** (all sibling tasks gate on an in-process `Instant`, which
//! resets on reboot — fine at 24h, wrong at 30d).
//!
//! Split of labor: the **Rust hook** owns the interval gate, disk-backed
//! last-run persistence, subprocess spawn, marker parsing, and logging. The
//! **recipe (a `recipe-runner-rs` subprocess)** owns all LLM judgment: the five
//! SEEK→VALIDATE→FIX quality-audit waves, the bounded crusty-old-engineer proxy
//! review loop, and the self-merge decisions.
//!
//! Unlike `brain_introspection` (best-effort/graceful), the self-audit follows
//! the `disk_health` **no-fallback** contract: any recipe failure propagates as
//! `SimardError::AdapterInvocationFailed`; the daemon WARNs and continues, and
//! persists last-run regardless (on `Ok` AND `Err`) to prevent hot-looping.
//!
//! ```ignore
//! // crate::self_quality_audit — surface under test
//! pub const DEFAULT_INTERVAL_SECS: u64 = 2_592_000; // ~30 days
//!
//! pub fn interval_secs_from_env(raw: Option<&str>) -> u64;
//! pub fn should_run_self_audit(elapsed: Duration, interval_secs: u64) -> bool;
//!
//! pub fn read_last_run(path: &Path) -> Option<u64>;
//! pub fn write_last_run(path: &Path, epoch_secs: u64) -> std::io::Result<()>;
//!
//! pub struct SelfQualityAuditReport {
//!     pub waves_completed: u32,
//!     pub prs_opened: Vec<String>,
//!     pub prs_merged: Vec<String>,
//!     pub crusty_approved: Vec<String>,
//!     pub crusty_unresolved: Vec<String>,
//!     pub summary_line: String,
//! }
//! impl SelfQualityAuditReport { pub fn summary(&self) -> String; }
//!
//! // #4968: the brittle `parse_self_quality_audit_text` AUDIT_COMPLETE marker
//! //  scraper is RETIRED — the rail now reads a typed record fail-closed:
//! pub fn read_verified_self_quality_audit(path: &Path, invoke_start: SystemTime)
//!     -> SimardResult<SelfQualityAuditRecord>;   // R1–R7, in self_quality_audit_record.rs
//! pub fn run_self_quality_audit(repo_root: &Path, state_root: &Path,
//!     home_override: Option<&Path>) -> SimardResult<SelfQualityAuditReport>;
//! // resolve_recipe_path is a *private* fn (matches disk_health), so it is
//! // exercised indirectly through run_self_quality_audit's recipe-not-found path.
//! ```

#![allow(clippy::bool_assert_comparison)]

use crate::error::SimardError;
use crate::self_quality_audit::{
    DEFAULT_INTERVAL_SECS, SelfQualityAuditReport, interval_secs_from_env, read_last_run,
    run_self_quality_audit, should_run_self_audit, write_last_run,
};
// #4968: the typed-record read path that replaces `parse_self_quality_audit_text`.
// These symbols do not exist yet, so their unresolved import is the intended,
// deterministic RED signal under `cargo test` until the Builder lands
// `src/self_quality_audit_record.rs` + the `record-self-quality-audit` verb.
use crate::operator_cli::dispatch_operator_cli;
use crate::self_quality_audit_record::{
    MAX_AGE_SECS, SELF_QUALITY_AUDIT_SCHEMA, SelfQualityAuditRecord,
    read_verified_self_quality_audit,
};

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Current wall-clock time as unix epoch seconds (the same quantity the daemon
/// stores via `write_last_run`).
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_secs()
}

// ===========================================================================
// 1. Defaults + SIMARD_SELF_AUDIT_INTERVAL env parsing
// ===========================================================================

#[test]
fn default_interval_is_thirty_days() {
    // 2_592_000s == 30 * 24 * 60 * 60 == ~30 days (the goal's stated default).
    assert_eq!(
        DEFAULT_INTERVAL_SECS, 2_592_000,
        "cadence defaults to ~30 days"
    );
    assert_eq!(DEFAULT_INTERVAL_SECS, 30 * 24 * 60 * 60);
}

#[test]
fn interval_secs_unset_uses_default() {
    assert_eq!(interval_secs_from_env(None), DEFAULT_INTERVAL_SECS);
}

#[test]
fn interval_secs_empty_uses_default() {
    // Empty string is not a valid u64 -> conservative default, NOT disabled.
    assert_eq!(interval_secs_from_env(Some("")), DEFAULT_INTERVAL_SECS);
    assert_eq!(interval_secs_from_env(Some("   ")), DEFAULT_INTERVAL_SECS);
}

#[test]
fn interval_secs_parses_valid_value() {
    assert_eq!(interval_secs_from_env(Some("3600")), 3600);
    assert_eq!(interval_secs_from_env(Some("2592000")), 2_592_000);
    // Surrounding whitespace is tolerated (matches sibling env parsers).
    assert_eq!(interval_secs_from_env(Some("  86400  ")), 86_400);
}

#[test]
fn interval_secs_zero_is_honored_as_disabled_value() {
    // 0 is a *valid* parse meaning "disabled"; it must NOT fall back to default.
    assert_eq!(interval_secs_from_env(Some("0")), 0);
}

#[test]
fn interval_secs_garbage_falls_back_to_default() {
    assert_eq!(
        interval_secs_from_env(Some("not-a-number")),
        DEFAULT_INTERVAL_SECS
    );
    assert_eq!(interval_secs_from_env(Some("30d")), DEFAULT_INTERVAL_SECS);
    assert_eq!(interval_secs_from_env(Some("-1")), DEFAULT_INTERVAL_SECS);
}

// ===========================================================================
// 2. Scheduling gate — should_run_self_audit(elapsed, interval)
//    Pure: interval > 0 && elapsed >= interval
// ===========================================================================

#[test]
fn gate_disabled_when_interval_zero() {
    assert_eq!(
        should_run_self_audit(Duration::from_secs(10_000_000_000), 0),
        false,
        "interval 0 must disable the audit entirely, regardless of elapsed"
    );
}

#[test]
fn gate_holds_when_not_yet_elapsed() {
    // One second short of the default interval -> not due.
    assert_eq!(
        should_run_self_audit(
            Duration::from_secs(DEFAULT_INTERVAL_SECS - 1),
            DEFAULT_INTERVAL_SECS
        ),
        false,
        "must NOT fire before a full interval has elapsed"
    );
    assert_eq!(
        should_run_self_audit(Duration::from_secs(0), DEFAULT_INTERVAL_SECS),
        false,
        "must NOT fire immediately after a fresh init (elapsed == 0)"
    );
}

#[test]
fn gate_fires_exactly_at_interval_boundary() {
    assert_eq!(
        should_run_self_audit(
            Duration::from_secs(DEFAULT_INTERVAL_SECS),
            DEFAULT_INTERVAL_SECS
        ),
        true,
        "elapsed == interval is due (>=, not >)"
    );
}

#[test]
fn gate_fires_when_elapsed_exceeds_interval() {
    assert_eq!(
        should_run_self_audit(
            Duration::from_secs(DEFAULT_INTERVAL_SECS + 5_000),
            DEFAULT_INTERVAL_SECS
        ),
        true
    );
}

// ===========================================================================
// 3. Last-run persistence — write_last_run / read_last_run
//    (the one capability sibling periodic tasks lack)
// ===========================================================================

#[test]
fn last_run_round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit_last_run");

    let epoch = 1_735_689_600; // 2025-01-01T00:00:00Z
    write_last_run(&path, epoch).expect("write_last_run should succeed");

    assert!(path.is_file(), "last-run file must exist after write");
    assert_eq!(
        read_last_run(&path),
        Some(epoch),
        "read_last_run must return exactly what write_last_run persisted"
    );
}

#[test]
fn read_last_run_is_none_when_file_absent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit_last_run");
    assert!(!path.exists());
    assert_eq!(
        read_last_run(&path),
        None,
        "absent last-run file => None (daemon then init-to-now)"
    );
}

#[test]
fn read_last_run_is_none_on_garbage_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit_last_run");
    std::fs::write(&path, "not-an-epoch\n").unwrap();
    assert_eq!(
        read_last_run(&path),
        None,
        "unparseable contents are treated as missing (=> init-to-now, no crash)"
    );
}

#[test]
fn write_last_run_creates_missing_parent_dir() {
    // The daemon may write before the state dir subtree exists.
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("nested")
        .join("state")
        .join("self_quality_audit_last_run");
    assert!(!path.parent().unwrap().exists());

    write_last_run(&path, 42).expect("write_last_run must create parent dirs");
    assert_eq!(read_last_run(&path), Some(42));
}

// ===========================================================================
// 4. Restart survival — persistence + gate together
//    Fires ~monthly, NOT on every restart.
// ===========================================================================

#[test]
fn simulated_restart_does_not_immediately_refire() {
    // Simulate: a run just happened (last_run = now), then the daemon restarts
    // and reloads last_run from disk. Elapsed ≈ 0 -> gate must stay false.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit_last_run");

    let now = now_epoch();
    write_last_run(&path, now).unwrap();

    // "Restart": read the persisted epoch back and recompute elapsed.
    let last = read_last_run(&path).expect("persisted last-run survives restart");
    let elapsed = Duration::from_secs(now_epoch().saturating_sub(last));

    assert_eq!(
        should_run_self_audit(elapsed, DEFAULT_INTERVAL_SECS),
        false,
        "a restart moments after a run must NOT re-trigger the heavy 5-wave audit"
    );
}

#[test]
fn stale_last_run_triggers_audit_after_interval() {
    // Persisted last-run is older than one interval -> due on the next cycle,
    // even across a restart.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit_last_run");

    let stale = now_epoch().saturating_sub(DEFAULT_INTERVAL_SECS + 10);
    write_last_run(&path, stale).unwrap();

    let last = read_last_run(&path).expect("persisted last-run survives restart");
    let elapsed = Duration::from_secs(now_epoch().saturating_sub(last));

    assert_eq!(
        should_run_self_audit(elapsed, DEFAULT_INTERVAL_SECS),
        true,
        "a last-run older than the interval must fire the audit"
    );
}

// ===========================================================================
// 5. #4968 — typed SelfQualityAuditRecord read path (replaces the deleted
//    parse_self_quality_audit_text AUDIT_COMPLETE marker grammar).
//
//    The recipe's final ACT step writes ONE typed, owner-only (0o600),
//    freshness-checked record; the rail reads it FAIL-CLOSED via
//    read_verified_self_quality_audit (R1–R7). Every failure mode is a distinct
//    AdapterInvocationFailed carrying its R-code — NEVER a silent default. Also
//    covers the gated `simard cognition record-self-quality-audit` writer verb
//    and writer/reader parity (one shared type, no drift).
// ===========================================================================

/// Write `bytes` verbatim to `path` as an owner-only `0o600` file (so R6 passes
/// for every non-R6 case), creating parents. Used to author records the typed
/// writer would never produce — malformed JSON, an unknown key, waves > 5 — so
/// the reader's fail-closed matrix is exercised directly.
fn write_raw_600(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(path, bytes).expect("write raw record");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("chmod 0o600");
    }
}

/// A fully-valid, fresh `SelfQualityAuditRecord` as raw JSON (a base to tweak
/// per fail-closed test).
fn valid_audit_json(epoch: u64) -> serde_json::Value {
    serde_json::json!({
        "schema": "self-quality-audit/v1",
        "written_at_epoch": epoch,
        "waves_completed": 5,
        "prs_opened": ["https://github.com/rysweet/Simard/pull/5001"],
        "prs_merged": ["https://github.com/rysweet/Simard/pull/5001"],
        "crusty_approved": ["https://github.com/rysweet/Simard/pull/5001"],
        "crusty_unresolved": [],
        "summary_line": "5 waves, 1 PR opened, 1 merged"
    })
}

/// Assert a reader result is the fail-closed `Err` for R-code `code` (e.g.
/// `"R4"`) — an `AdapterInvocationFailed` whose reason names that check. The
/// reader must NEVER return `Ok` (a defaulted/partial record) on a bad input.
fn assert_read_r(result: Result<SelfQualityAuditRecord, SimardError>, code: &str) {
    match result {
        Ok(rec) => panic!("expected fail-closed {code}, got Ok({rec:?})"),
        Err(SimardError::AdapterInvocationFailed { reason, .. }) => assert!(
            reason.contains(code),
            "expected fail-closed reason to carry {code}, got: {reason}"
        ),
        Err(other) => panic!("expected AdapterInvocationFailed carrying {code}, got: {other:?}"),
    }
}

/// Drive the operator CLI (the gated writer verb entry point).
fn cli(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(args.iter().map(|s| s.to_string()))
}

// --- pins: schema + freshness constant ---

#[test]
fn audit_schema_pin_is_v1() {
    assert_eq!(SELF_QUALITY_AUDIT_SCHEMA, "self-quality-audit/v1");
}

#[test]
fn audit_max_age_secs_is_five_minutes() {
    assert_eq!(MAX_AGE_SECS, 300);
}

// --- happy path: a valid, fresh, 0o600 record is accepted ---

#[test]
fn read_accepts_a_valid_fresh_owner_only_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    let epoch = now_epoch();
    write_raw_600(
        &path,
        &serde_json::to_vec(&valid_audit_json(epoch)).unwrap(),
    );
    let invoke_start = SystemTime::now() - Duration::from_secs(5);

    let rec = read_verified_self_quality_audit(&path, invoke_start)
        .expect("a valid, fresh, 0o600 record must be accepted");
    assert_eq!(rec.schema, "self-quality-audit/v1");
    assert_eq!(rec.waves_completed, 5);
    assert_eq!(rec.prs_opened.len(), 1);
    assert_eq!(rec.summary_line, "5 waves, 1 PR opened, 1 merged");
}

// --- R1: absent / unreadable ---

#[test]
fn read_r1_absent_record_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_self_quality_audit(&path, invoke_start), "R1");
}

// --- R2: present but not valid JSON ---

#[test]
fn read_r2_malformed_json_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    write_raw_600(&path, b"not valid json at all {{{");
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_self_quality_audit(&path, invoke_start), "R2");
}

// --- R3: schema version pin ---

#[test]
fn read_r3_wrong_schema_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    let mut json = valid_audit_json(now_epoch());
    json["schema"] = serde_json::json!("self-quality-audit/v2");
    write_raw_600(&path, &serde_json::to_vec(&json).unwrap());
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_self_quality_audit(&path, invoke_start), "R3");
}

// --- R4: closed-type parse & bounds (deny_unknown_fields / waves > 5) ---

#[test]
fn read_r4_unknown_top_level_key_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    let mut json = valid_audit_json(now_epoch());
    json["bogus_extra_field"] = serde_json::json!("x");
    write_raw_600(&path, &serde_json::to_vec(&json).unwrap());
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_self_quality_audit(&path, invoke_start), "R4");
}

#[test]
fn read_r4_waves_completed_over_five_is_fail_closed() {
    // waves_completed is bounded to 0..=5; a value > 5 is a hard R4 reject.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    let mut json = valid_audit_json(now_epoch());
    json["waves_completed"] = serde_json::json!(6);
    write_raw_600(&path, &serde_json::to_vec(&json).unwrap());
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_self_quality_audit(&path, invoke_start), "R4");
}

// --- R5: required-field validity (summary_line must be non-empty) ---

#[test]
fn read_r5_empty_summary_line_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    let mut json = valid_audit_json(now_epoch());
    json["summary_line"] = serde_json::json!("   ");
    write_raw_600(&path, &serde_json::to_vec(&json).unwrap());
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_self_quality_audit(&path, invoke_start), "R5");
}

// --- R6: owner-only permissions ---

#[cfg(unix)]
#[test]
fn read_r6_non_owner_only_permissions_is_fail_closed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    write_raw_600(
        &path,
        &serde_json::to_vec(&valid_audit_json(now_epoch())).unwrap(),
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_self_quality_audit(&path, invoke_start), "R6");
}

// --- R7: freshness / anti-replay ---

#[test]
fn read_r7_mtime_predates_invoke_start_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    write_raw_600(
        &path,
        &serde_json::to_vec(&valid_audit_json(now_epoch())).unwrap(),
    );
    let invoke_start = SystemTime::now() + Duration::from_secs(1_000);
    assert_read_r(read_verified_self_quality_audit(&path, invoke_start), "R7");
}

#[test]
fn read_r7_stale_written_at_epoch_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    let stale = now_epoch().saturating_sub(10_000);
    write_raw_600(
        &path,
        &serde_json::to_vec(&valid_audit_json(stale)).unwrap(),
    );
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_self_quality_audit(&path, invoke_start), "R7");
}

// --- gated writer verb `simard cognition record-self-quality-audit` + parity ---

#[test]
fn cli_record_self_quality_audit_writes_a_record_the_reader_accepts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    let epoch = now_epoch();
    let invoke_start = SystemTime::now() - Duration::from_secs(5);

    cli(&[
        "cognition",
        "record-self-quality-audit",
        "--record-path",
        path.to_str().unwrap(),
        "--written-at-epoch",
        &epoch.to_string(),
        "--waves-completed",
        "5",
        "--summary-line",
        "5 waves, 2 PRs opened, 1 merged, 1 crusty-unresolved",
        "--pr-opened",
        "https://github.com/rysweet/Simard/pull/5001",
        "--pr-opened",
        "https://github.com/rysweet/Simard/pull/5002",
        "--pr-merged",
        "https://github.com/rysweet/Simard/pull/5001",
        "--crusty-approved",
        "https://github.com/rysweet/Simard/pull/5001",
        "--crusty-unresolved",
        "https://github.com/rysweet/Simard/pull/5002",
    ])
    .expect("a valid self-quality-audit record write must exit Ok");

    // Writer/reader parity — the reader accepts exactly what the writer produced.
    let rec = read_verified_self_quality_audit(&path, invoke_start)
        .expect("the reader must accept the record the writer just produced (no drift)");
    assert_eq!(rec.schema, "self-quality-audit/v1");
    assert_eq!(rec.waves_completed, 5);
    assert_eq!(rec.prs_opened.len(), 2);
    assert_eq!(rec.prs_merged.len(), 1);
    assert_eq!(rec.crusty_unresolved.len(), 1);
    assert_eq!(
        rec.summary_line,
        "5 waves, 2 PRs opened, 1 merged, 1 crusty-unresolved"
    );

    // And the file is owner-only 0o600 (persist_json's atomic 0o600 write).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "record must be written owner-only 0o600"
        );
    }
}

#[test]
fn cli_record_self_quality_audit_requires_summary_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    let res = cli(&[
        "cognition",
        "record-self-quality-audit",
        "--record-path",
        path.to_str().unwrap(),
        "--written-at-epoch",
        &now_epoch().to_string(),
        "--waves-completed",
        "0",
        // no --summary-line
    ]);
    assert!(
        res.is_err(),
        "a non-empty --summary-line is required (validate-all-then-write-once)"
    );
    assert!(
        !path.exists(),
        "a validation failure must leave NO file on disk"
    );
}

#[test]
fn cli_record_self_quality_audit_rejects_waves_over_five() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("self_quality_audit/record.json");
    let res = cli(&[
        "cognition",
        "record-self-quality-audit",
        "--record-path",
        path.to_str().unwrap(),
        "--written-at-epoch",
        &now_epoch().to_string(),
        "--waves-completed",
        "6",
        "--summary-line",
        "over the wave cap",
    ]);
    assert!(res.is_err(), "--waves-completed > 5 must be rejected");
    assert!(
        !path.exists(),
        "a rejected write must leave NO file on disk"
    );
}

#[test]
fn cli_record_self_quality_audit_rejects_non_absolute_record_path() {
    let res = cli(&[
        "cognition",
        "record-self-quality-audit",
        "--record-path",
        "relative/record.json",
        "--written-at-epoch",
        &now_epoch().to_string(),
        "--waves-completed",
        "1",
        "--summary-line",
        "ok",
    ]);
    assert!(res.is_err(), "--record-path must be absolute");
}

// ===========================================================================
// 6. SelfQualityAuditReport::summary — one-line daemon completion log
// ===========================================================================

#[test]
fn summary_mentions_key_counts() {
    let report = SelfQualityAuditReport {
        waves_completed: 5,
        prs_opened: vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ],
        prs_merged: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        crusty_approved: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        crusty_unresolved: vec!["d".to_string()],
        summary_line: "agent's own terminal summary".to_string(),
    };
    let s = report.summary();

    assert!(
        s.to_lowercase().contains("self quality-audit"),
        "completion log line should identify the task: {s}"
    );
    assert!(s.contains('5'), "should report waves_completed (5): {s}");
    assert!(s.contains('4'), "should report PRs opened (4): {s}");
    assert!(s.contains('3'), "should report PRs merged (3): {s}");
    assert!(
        s.contains('1'),
        "should report crusty-unresolved count (1): {s}"
    );
}

// ===========================================================================
// 7. run_self_quality_audit — no-fallback contract (disk_health model)
//    Recipe missing => hard SimardError::AdapterInvocationFailed (deterministic:
//    resolve_recipe_path returns None BEFORE any subprocess or config load).
// ===========================================================================

#[test]
fn run_errors_when_recipe_missing() {
    // Empty home => hot-reload path misses; nonexistent repo => in-tree misses.
    let empty_home = tempfile::tempdir().unwrap();
    let err = run_self_quality_audit(
        Path::new("/nonexistent/repo"),
        Path::new("/nonexistent/state"),
        Some(empty_home.path()),
    )
    .expect_err("no-fallback contract: a missing recipe must be a hard error");

    match err {
        SimardError::AdapterInvocationFailed { reason, .. } => {
            assert!(
                reason.contains("monthly-self-quality-audit.yaml"),
                "error reason should name the missing recipe file: {reason}"
            );
        }
        other => panic!("expected AdapterInvocationFailed, got: {other:?}"),
    }
}
