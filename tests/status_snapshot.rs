//! Tests for the unified `StatusSnapshot` type model, JSON contract, and the
//! process-agnostic `assemble` provider (issue #2528).
//!
//! Pins `docs/reference/status-snapshot-api.md`: the section envelope with
//! availability/freshness, the "no silent zeros" rule (a missing count is
//! `absent`, never `0`), the additive JSON schema, and that `assemble` never
//! panics and reads only durable sources under an explicit state root.

use serial_test::serial;

use simard::status::{
    self, Availability, Daemon, Freshness, GoalBoard, GoalItem, LedgerWindow, LlmUsage,
    SectionEnvelope, StatusSnapshot, provider::AssembleOptions,
};
use simard::telemetry::names;
use simard::telemetry::snapshot::{self, MetricsSnapshot};

// ── env guard (env is process-global; serialize env-touching tests) ──────────

struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: env-touching tests here are `#[serial(status_env)]`.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }
    fn unset(key: &'static str) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: env-touching tests here are `#[serial(status_env)]`.
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: serialized via `#[serial(status_env)]`.
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Assemble options pinned to a temp state root and a deliberately-nonexistent
/// unit, so the test never reads the real `simard.service` or `~/.simard`.
fn hermetic_opts(state_root: &std::path::Path) -> AssembleOptions {
    let mut opts = AssembleOptions::with_state_root(state_root.to_path_buf());
    opts.service_unit = "simard-status-test-nonexistent.service".to_string();
    opts
}

// ── envelope + type model ────────────────────────────────────────────────────

#[test]
fn envelope_default_is_unavailable_absent_with_no_data() {
    let env: SectionEnvelope<Daemon> = SectionEnvelope::default();
    assert_eq!(env.availability, Availability::Unavailable);
    assert_eq!(env.freshness, Freshness::Absent);
    assert!(env.data.is_none());
    assert!(!env.is_present());
}

#[test]
fn envelope_constructors_set_expected_state() {
    let live = SectionEnvelope::live(Daemon::default(), Some("2026-07-03T00:00:00Z".into()));
    assert_eq!(live.availability, Availability::Ok);
    assert_eq!(live.freshness, Freshness::Live);
    assert!(live.is_present());

    let absent: SectionEnvelope<Daemon> = SectionEnvelope::absent("gh: not authenticated");
    assert_eq!(absent.availability, Availability::Unavailable);
    assert_eq!(absent.freshness, Freshness::Absent);
    assert!(absent.data.is_none());
    assert_eq!(absent.note.as_deref(), Some("gh: not authenticated"));
}

#[test]
fn empty_snapshot_has_all_ten_sections_absent() {
    let snap = StatusSnapshot::empty();
    assert_eq!(snap.schema_version, status::SCHEMA_VERSION);
    assert!(!snap.generated_at.is_empty());

    // All ten operator sections default to absent — never fabricated zeros.
    for present in [
        snap.daemon.is_present(),
        snap.resources.is_present(),
        snap.llm.is_present(),
        snap.memory.is_present(),
        snap.gym.is_present(),
        snap.goals.is_present(),
        snap.workstreams.is_present(),
        snap.completed.is_present(),
        snap.self_improvement.is_present(),
        snap.telemetry.is_present(),
    ] {
        assert!(!present, "empty snapshot section should be absent");
    }
}

// ── JSON contract ────────────────────────────────────────────────────────────

#[test]
fn json_round_trips_and_is_additive() {
    let mut snap = StatusSnapshot::empty();
    snap.daemon = SectionEnvelope::live(
        Daemon {
            state: "active (running)".into(),
            version: "0.24.0".into(),
            main_pid: Some(4242),
            deployed_commit: Some("e5764c6d".into()),
            instance_uptime: Some("2h14m".into()),
            n_restarts: Some(0),
            running_since: Some("2026-07-03T01:40:31Z".into()),
        },
        Some("2026-07-03T03:55:04Z".into()),
    );
    snap.goals = SectionEnvelope::live(
        GoalBoard {
            active: vec![GoalItem {
                short_id: "goal-2f9c".into(),
                priority: "p0".into(),
                status: "in-progress".into(),
                summary: "Rationalize telemetry".into(),
            }],
        },
        None,
    );

    let json = status::json::to_string_pretty(&snap).expect("serialize");
    assert!(json.contains("\"availability\": \"ok\""));
    assert!(json.contains("\"freshness\": \"absent\"")); // the still-absent sections
    assert!(json.contains("\"n_restarts\": 0"));

    // Unknown fields are ignored; missing fields default (additive growth).
    let grown = json.replace(
        "\"schema_version\": 1",
        "\"schema_version\": 1,\n  \"future_field\": {\"x\": 1}",
    );
    let parsed = status::json::from_str(&grown).expect("deserialize with unknown field");
    assert_eq!(parsed, snap);
}

#[test]
fn absent_section_serializes_as_unavailable_absent_not_zero() {
    let snap = StatusSnapshot::empty();
    let json = status::json::to_string(&snap).expect("serialize");
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse");

    let daemon = &value["daemon"];
    assert_eq!(daemon["availability"], "unavailable");
    assert_eq!(daemon["freshness"], "absent");
    // No fabricated data payload for an absent section.
    assert!(daemon.get("data").is_none() || daemon["data"].is_null());
}

// ── assemble: never panics, all sections present, honest degradation ─────────

#[test]
#[serial(status_env)]
fn assemble_on_empty_state_root_never_panics_and_degrades() {
    let _skip = EnvGuard::unset("SIMARD_SKIP_GYM");
    let dir = tempfile::tempdir().expect("tempdir");
    // The daemon section falls back to the durable `daemon_health.json`
    // heartbeat under `dirs::data_local_dir()/simard/`, which is NOT under the
    // state root. Pin `XDG_DATA_HOME` to the empty tempdir so the heartbeat
    // resolves to a nonexistent path — otherwise a live daemon writing its
    // heartbeat on the host would leak in and this test would depend on the
    // environment rather than the (empty) state root.
    let _data_home = EnvGuard::set("XDG_DATA_HOME", &dir.path().to_string_lossy());
    let snap = status::assemble(&hermetic_opts(dir.path()));

    // Structurally complete: generated + schema version set.
    assert_eq!(snap.schema_version, status::SCHEMA_VERSION);
    assert!(!snap.generated_at.is_empty());

    // Heavy/unwired sources degrade honestly rather than inventing data.
    assert!(!snap.daemon.is_present(), "bogus unit -> daemon absent");
    assert!(
        !snap.telemetry.is_present(),
        "no snapshot file -> telemetry absent"
    );
    assert!(!snap.llm.is_present(), "no ledger -> llm absent");
    assert!(snap.memory.note.is_some());
    assert!(snap.completed.note.is_some());

    // Gym is always answerable from the environment.
    assert!(snap.gym.is_present());
    assert_eq!(snap.gym.data.as_ref().map(|g| g.skip_gym), Some(false));
}

#[test]
#[serial(status_env)]
fn assemble_reflects_skip_gym_env() {
    let _skip = EnvGuard::set("SIMARD_SKIP_GYM", "1");
    let dir = tempfile::tempdir().expect("tempdir");
    let snap = status::assemble(&hermetic_opts(dir.path()));

    let gym = snap.gym.data.as_ref().expect("gym present");
    assert!(gym.skip_gym, "SIMARD_SKIP_GYM=1 must surface as skip_gym");
    let tele = snap.telemetry.data.as_ref();
    // Even absent-snapshot telemetry still knows gym is skipped once wired; here
    // the snapshot is absent so telemetry is absent — gym section is the pin.
    assert!(tele.is_none());
}

#[test]
#[serial(status_env)]
fn assemble_derives_distill_fail_pct_from_metrics_snapshot() {
    let _skip = EnvGuard::unset("SIMARD_SKIP_GYM");
    let dir = tempfile::tempdir().expect("tempdir");

    // Fixture: 8 ok distill runs, 2 parse failures -> 20% fail.
    let mut fixture = MetricsSnapshot::empty();
    fixture
        .counters
        .push(simard::telemetry::snapshot::CounterSeries {
            name: names::DISTILL_RUNS.into(),
            attrs: vec![(names::ATTR_RESULT.into(), "ok".into())],
            value: 8,
        });
    fixture
        .counters
        .push(simard::telemetry::snapshot::CounterSeries {
            name: names::DISTILL_RUNS.into(),
            attrs: vec![(names::ATTR_RESULT.into(), "parse_fail".into())],
            value: 2,
        });
    let path = dir.path().join("telemetry").join("metrics_snapshot.json");
    snapshot::write_atomic(&path, &fixture).expect("write fixture");

    let snap = status::assemble(&hermetic_opts(dir.path()));
    let tele = snap.telemetry.data.as_ref().expect("telemetry present");
    let pct = tele.distill_fail_pct.expect("fail pct derived");
    assert!((pct - 20.0).abs() < 1e-9, "expected 20% got {pct}");
    assert_eq!(tele.parse_fix_holding, Some(false));

    // A just-written snapshot is fresh.
    assert_eq!(snap.telemetry.availability, Availability::Ok);
    assert_eq!(snap.telemetry.freshness, Freshness::Live);
}

#[test]
#[serial(status_env)]
fn assemble_reads_cost_ledger_under_state_root() {
    let _budget = EnvGuard::set("SIMARD_DAILY_BUDGET_USD", "25");
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = dir.path().join("costs").join("ledger.jsonl");
    std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let line = format!(
        "{{\"timestamp\":\"{now}\",\"session_id\":\"s\",\"model\":\"gpt\",\"prompt_tokens_est\":1000,\"completion_tokens_est\":200,\"cost_usd_est\":1.5,\"context\":\"c\"}}"
    );
    std::fs::write(&ledger, format!("{line}\n{line}\n")).unwrap();

    let snap = status::assemble(&hermetic_opts(dir.path()));
    let llm: &LlmUsage = snap.llm.data.as_ref().expect("llm present");
    let all: &LedgerWindow = llm.ledger_all_time.as_ref().expect("all-time window");
    assert!(
        (all.cost_usd - 3.0).abs() < 1e-9,
        "cost sum was {}",
        all.cost_usd
    );
    assert_eq!(all.tokens_in, 2000);
    assert_eq!(all.tokens_out, 400);
    assert_eq!(llm.daily_budget_usd, Some(25.0));
}

// ── daily budget display guard (issue #6) ────────────────────────────────────
//
// The status display must report the *actual* guard. The daily budget is always
// guarded — `crate::overseer::config::resolve_daily_budget_usd` falls back to
// `DEFAULT_DAILY_BUDGET_USD` when the env is unset/empty/unparseable/non-positive
// — so the provider must never emit `None` (rendered "unset (no guard)") when a
// default applies. These pin that the provider single-sources the ceiling
// through the canonical resolver rather than reading the raw env directly.

/// Write a minimal single-entry cost ledger so the `llm` section assembles as
/// `live` (the provider returns `absent` when no ledger exists).
fn write_cost_ledger(state_root: &std::path::Path) {
    let ledger = state_root.join("costs").join("ledger.jsonl");
    std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let line = format!(
        "{{\"timestamp\":\"{now}\",\"session_id\":\"s\",\"model\":\"gpt\",\"prompt_tokens_est\":1000,\"completion_tokens_est\":200,\"cost_usd_est\":1.5,\"context\":\"c\"}}"
    );
    std::fs::write(&ledger, format!("{line}\n")).unwrap();
}

#[test]
#[serial(status_env)]
fn assemble_daily_budget_defaults_to_guard_when_env_unset() {
    // Bug #6: running outside the daemon's systemd env (var absent) previously
    // reported no budget guard. The budget is always guarded, so an unset env
    // must surface the canonical default ceiling — never `None`.
    let _skip = EnvGuard::unset("SIMARD_SKIP_GYM");
    let _budget = EnvGuard::unset("SIMARD_DAILY_BUDGET_USD");
    let dir = tempfile::tempdir().expect("tempdir");
    write_cost_ledger(dir.path());

    let snap = status::assemble(&hermetic_opts(dir.path()));
    let llm: &LlmUsage = snap.llm.data.as_ref().expect("llm present");
    assert_eq!(
        llm.daily_budget_usd,
        Some(simard::overseer::config::DEFAULT_DAILY_BUDGET_USD),
        "unset env must resolve to the canonical default guard, not None"
    );
}

#[test]
#[serial(status_env)]
fn assemble_daily_budget_reflects_explicit_env() {
    let _skip = EnvGuard::unset("SIMARD_SKIP_GYM");
    let _budget = EnvGuard::set("SIMARD_DAILY_BUDGET_USD", "250");
    let dir = tempfile::tempdir().expect("tempdir");
    write_cost_ledger(dir.path());

    let snap = status::assemble(&hermetic_opts(dir.path()));
    let llm: &LlmUsage = snap.llm.data.as_ref().expect("llm present");
    assert_eq!(llm.daily_budget_usd, Some(250.0));
}

#[test]
#[serial(status_env)]
fn assemble_daily_budget_nonpositive_env_falls_back_to_guard() {
    // A non-positive value is not a real ceiling; the canonical resolver rejects
    // it and applies the default guard. The display must mirror that reality
    // rather than reporting a misleading `0`.
    let _skip = EnvGuard::unset("SIMARD_SKIP_GYM");
    let _budget = EnvGuard::set("SIMARD_DAILY_BUDGET_USD", "0");
    let dir = tempfile::tempdir().expect("tempdir");
    write_cost_ledger(dir.path());

    let snap = status::assemble(&hermetic_opts(dir.path()));
    let llm: &LlmUsage = snap.llm.data.as_ref().expect("llm present");
    assert_eq!(
        llm.daily_budget_usd,
        Some(simard::overseer::config::DEFAULT_DAILY_BUDGET_USD),
        "non-positive env must fall back to the canonical default guard"
    );
}
