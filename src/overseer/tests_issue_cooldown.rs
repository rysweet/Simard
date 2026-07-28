//! TDD contract tests for the durable **issue-cooldown ledger** (Problem 1 /
//! issue #4930 — the OODA-core auto-issue storm).
//!
//! These are written **first** (red): they pin the not-yet-built contract from
//! [`docs/reference/issue-cooldown-ledger-api.md`] so the implementation has an
//! exact, executable specification. They MUST fail — first as a compile error on
//! the missing `crate::overseer::issue_cooldown` module / API, then as assertion
//! failures — until the ledger is implemented, and MUST pass once it is.
//!
//! Contract under test (see the reference doc):
//!
//! - `IssueCooldownLedger`, `FindingKind`, `CooldownKey`, `CooldownDecision` live
//!   in `src/overseer/issue_cooldown.rs`.
//! - A given `(finding_kind, subject)` opens **at most one** tracking issue per
//!   cooldown window; the window floor spans **≥ 1 full OODA cycle** and the
//!   guarantee **survives daemon exec-reload / restart** (a fresh ledger over the
//!   same durable memory still throttles).
//! - The window reuses [`WhisperGate::with_backoff`] verbatim: `6h → 12h → 24h`,
//!   capped at 24 h.
//! - Durable backing is a standalone cognitive-memory fact namespace
//!   (`overseer:issue-cooldown`) written via `store_fact_with_caller_key`
//!   (in-place upsert — **never** a duplicate fact per key).
//! - `subject` is canonicalized to `[a-z0-9:_-]` so it is stable across cycles
//!   and cannot inject `gh --search` qualifiers (SR-V3).
//! - Reads **fail open**: a memory-read error yields `Emit`, never silent
//!   permanent suppression (SR-D3).
//! - The stored fact holds only `{ last_emit_secs, strikes, issue_number }` — no
//!   issue body, no token (SR-D1/D2).
//!
//! Everything is exercised with injected fakes — no network, no `~/.simard`, no
//! second cognitive store.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};
use crate::stewardship::gh_client::GhIssue;

use crate::overseer::config::{
    issue_cooldown_base_secs_from, issue_cooldown_cap_per_hour_from, issue_cooldown_cap_secs_from,
    issue_cooldown_enabled_from,
};
use crate::overseer::guardrails::WhisperGate;
use crate::overseer::issue_cooldown::{
    CooldownDecision, CooldownKey, FindingKind, IssueCooldownLedger,
};

// --- Contract constants (mirror the reference doc defaults) ---------------

/// Backoff window floor — one operator-facing default (6 h), already well above
/// the one-OODA-cycle hard floor.
const BASE_SECS: i64 = 21_600;
/// Backoff window cap — 24 h, so a still-open finding re-surfaces at least daily.
const CAP_SECS: i64 = 86_400;
/// A per-hour budget high enough that it never masks the window under test.
const CAP_PER_HOUR: usize = 100_000;

// --- Fakes ----------------------------------------------------------------

/// Minimal in-memory `CognitiveMemoryOps` that models the ONE behavior the
/// ledger depends on: `store_fact_with_caller_key` performs an in-place **upsert**
/// keyed by `caller_key` (supersede, not append), while plain `store_fact`
/// appends. `search_facts` returns facts whose `concept` contains the query.
///
/// `fail` flips both reads and caller-key writes to a memory-integrity error so
/// the fail-open contract can be exercised.
#[derive(Default)]
struct InMemoryFacts {
    facts: Mutex<Vec<CognitiveFact>>,
    append_counter: Mutex<u64>,
    fail: AtomicBool,
}

impl InMemoryFacts {
    fn new() -> Self {
        Self::default()
    }

    fn failing() -> Self {
        let m = Self::default();
        m.fail.store(true, Ordering::SeqCst);
        m
    }

    /// Test-only helper: every fact currently stored under `concept`.
    fn facts_for(&self, concept: &str) -> Vec<CognitiveFact> {
        self.facts
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f.concept == concept)
            .cloned()
            .collect()
    }
}

fn integrity_err() -> SimardError {
    SimardError::MemoryIntegrityError {
        path: std::path::PathBuf::from("<fake>"),
        reason: "injected".to_string(),
    }
}

fn make_fact(
    node_id: &str,
    concept: &str,
    content: &str,
    tags: &[String],
    source_id: &str,
) -> CognitiveFact {
    CognitiveFact {
        node_id: node_id.to_string(),
        concept: concept.to_string(),
        content: content.to_string(),
        confidence: 1.0,
        source_id: source_id.to_string(),
        tags: tags.to_vec(),
        usage_count: 0,
        last_accessed_at: None,
    }
}

impl CognitiveMemoryOps for InMemoryFacts {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("s".to_string())
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }
    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Ok("w".to_string())
    }
    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }
    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Ok(0)
    }
    fn store_episode(
        &self,
        _content: &str,
        _source_label: &str,
        _metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        Ok("ep".to_string())
    }
    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        Ok(None)
    }

    /// Plain `store_fact` APPENDS — a fresh row on every call. If the ledger were
    /// (incorrectly) built on this, `cooldown_upsert_is_idempotent` would catch
    /// the duplicate.
    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        _cf: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(integrity_err());
        }
        let node_id = {
            let mut n = self.append_counter.lock().unwrap();
            *n += 1;
            format!("append-{n}")
        };
        self.facts
            .lock()
            .unwrap()
            .push(make_fact(&node_id, concept, content, tags, source_id));
        Ok(node_id)
    }

    /// Caller-key variant UPSERTS in place: a re-write bearing the same
    /// `caller_key` supersedes the prior row instead of appending.
    fn store_fact_with_caller_key(
        &self,
        caller_key: &str,
        concept: &str,
        content: &str,
        _cf: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(integrity_err());
        }
        let mut facts = self.facts.lock().unwrap();
        if let Some(existing) = facts.iter_mut().find(|f| f.node_id == caller_key) {
            existing.concept = concept.to_string();
            existing.content = content.to_string();
            existing.tags = tags.to_vec();
            existing.source_id = source_id.to_string();
        } else {
            facts.push(make_fact(caller_key, concept, content, tags, source_id));
        }
        Ok(caller_key.to_string())
    }

    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        _min_conf: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(integrity_err());
        }
        Ok(self
            .facts
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f.concept.contains(query))
            .take(limit as usize)
            .cloned()
            .collect())
    }

    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("p".to_string())
    }
    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Ok(vec![])
    }
    fn store_prospective(&self, _d: &str, _t: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("pr".to_string())
    }
    fn check_triggers(&self, _content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics::default())
    }
}

// --- Helpers ---------------------------------------------------------------

/// A ledger over the given memory using the documented default backoff window.
fn ledger_over(mem: Arc<dyn CognitiveMemoryOps>) -> IssueCooldownLedger {
    IssueCooldownLedger::new(
        mem,
        WhisperGate::with_backoff(BASE_SECS, CAP_SECS, CAP_PER_HOUR),
    )
}

fn key(kind: FindingKind, subject: &str) -> CooldownKey {
    CooldownKey::new(kind, subject)
}

fn sample_issue(number: u64, body: &str) -> GhIssue {
    GhIssue {
        number,
        url: format!("https://github.com/rysweet/Simard/issues/{number}"),
        title: "ooda-stuck: goal move-the-roster".to_string(),
        body: body.to_string(),
    }
}

// --- Unit tests: the ledger contract --------------------------------------

/// First `allow_emit` → `Emit`; every subsequent in-window consult → `Throttle`.
/// This is the storm's core fix: one issue per window, not one per OODA cycle.
#[test]
fn cooldown_emits_once_then_throttles() {
    let mem = Arc::new(InMemoryFacts::new());
    let ledger = ledger_over(mem);
    let k = key(FindingKind::OodaStuck, "goal:move-the-roster-a8f57a50");

    assert_eq!(ledger.allow_emit(&k, 0), CooldownDecision::Emit);
    ledger
        .record_emit(&k, &sample_issue(4930, "signature"), 0)
        .unwrap();

    // Two later cycles, both still inside the 6 h base window → suppressed.
    assert_eq!(ledger.allow_emit(&k, 900), CooldownDecision::Throttle);
    assert_eq!(ledger.allow_emit(&k, 1_800), CooldownDecision::Throttle);
}

/// A still-open finding re-emits exactly once the window elapses (never
/// permanently silenced), then throttles again.
#[test]
fn cooldown_refires_after_window() {
    let mem = Arc::new(InMemoryFacts::new());
    let ledger = ledger_over(mem);
    let k = key(FindingKind::OodaStuck, "goal:g1");

    assert_eq!(ledger.allow_emit(&k, 0), CooldownDecision::Emit);
    ledger.record_emit(&k, &sample_issue(1, "sig"), 0).unwrap();

    assert_eq!(
        ledger.allow_emit(&k, BASE_SECS - 1),
        CooldownDecision::Throttle
    );
    assert_eq!(ledger.allow_emit(&k, BASE_SECS), CooldownDecision::Emit);
}

/// The window follows `6h → 12h → 24h` and never exceeds the 24 h cap, exactly
/// mirroring `WhisperGate::with_backoff`. Each re-emit doubles the window; the
/// fourth strike would be 48 h but is clamped to 24 h.
#[test]
fn cooldown_window_doubles_and_caps_at_24h() {
    let mem = Arc::new(InMemoryFacts::new());
    let ledger = ledger_over(mem);
    let k = key(FindingKind::RecurringGoalReblock, "goal:g1");

    // Strike 1 → base (6 h) window.
    assert_eq!(ledger.allow_emit(&k, 0), CooldownDecision::Emit);
    ledger.record_emit(&k, &sample_issue(1, "s"), 0).unwrap();
    assert_eq!(
        ledger.allow_emit(&k, BASE_SECS - 1),
        CooldownDecision::Throttle
    );
    assert_eq!(ledger.allow_emit(&k, BASE_SECS), CooldownDecision::Emit);

    // Strike 2 → window doubles to 12 h.
    let t2 = BASE_SECS;
    ledger.record_emit(&k, &sample_issue(2, "s"), t2).unwrap();
    assert_eq!(
        ledger.allow_emit(&k, t2 + 2 * BASE_SECS - 1),
        CooldownDecision::Throttle
    );
    assert_eq!(
        ledger.allow_emit(&k, t2 + 2 * BASE_SECS),
        CooldownDecision::Emit
    );

    // Strike 3 → window is 24 h (4 * 6 h == cap).
    let t3 = t2 + 2 * BASE_SECS;
    ledger.record_emit(&k, &sample_issue(3, "s"), t3).unwrap();
    assert_eq!(
        ledger.allow_emit(&k, t3 + CAP_SECS - 1),
        CooldownDecision::Throttle
    );
    assert_eq!(ledger.allow_emit(&k, t3 + CAP_SECS), CooldownDecision::Emit);

    // Strike 4 → 48 h would exceed the cap; window stays at 24 h.
    let t4 = t3 + CAP_SECS;
    ledger.record_emit(&k, &sample_issue(4, "s"), t4).unwrap();
    assert_eq!(
        ledger.allow_emit(&k, t4 + CAP_SECS - 1),
        CooldownDecision::Throttle
    );
    assert_eq!(ledger.allow_emit(&k, t4 + CAP_SECS), CooldownDecision::Emit);
}

/// Distinct `(kind, subject)` keys never share a window: rate-limiting one
/// finding must not silence a genuinely different one.
#[test]
fn cooldown_keys_are_per_subject_isolated() {
    let mem = Arc::new(InMemoryFacts::new());
    let ledger = ledger_over(mem);
    let a = key(FindingKind::OodaStuck, "goal:aaaa");
    let b = key(FindingKind::OodaStuck, "goal:bbbb");
    let c = key(FindingKind::WorkstreamGapIssue, "goal:aaaa");

    assert_eq!(ledger.allow_emit(&a, 0), CooldownDecision::Emit);
    ledger.record_emit(&a, &sample_issue(1, "s"), 0).unwrap();

    // Same subject A is throttled, but a different subject and a different kind
    // for the same subject each fire immediately.
    assert_eq!(ledger.allow_emit(&a, 1), CooldownDecision::Throttle);
    assert_eq!(ledger.allow_emit(&b, 1), CooldownDecision::Emit);
    assert_eq!(ledger.allow_emit(&c, 1), CooldownDecision::Emit);
}

/// Re-recording the same key UPSERTS the durable fact in place — never a
/// duplicate row. This is the proximate fix for the 9-duplicate storm: the
/// ledger must use `store_fact_with_caller_key`, not plain `store_fact`.
#[test]
fn cooldown_upsert_is_idempotent() {
    let mem = Arc::new(InMemoryFacts::new());
    let inspect = mem.clone();
    let ledger = ledger_over(mem);
    let k = key(FindingKind::OodaStuck, "goal:g1");

    ledger.record_emit(&k, &sample_issue(1, "s"), 0).unwrap();
    ledger
        .record_emit(&k, &sample_issue(1, "s"), BASE_SECS)
        .unwrap();
    ledger
        .record_emit(&k, &sample_issue(1, "s"), 3 * BASE_SECS)
        .unwrap();

    assert_eq!(
        inspect.facts_for(&k.fact_concept()).len(),
        1,
        "each key must own exactly one durable fact (upsert, not append)"
    );
}

/// A memory-read error fails OPEN: `allow_emit` returns `Emit` so a storage
/// hiccup can never permanently suppress a genuinely new finding (SR-D3).
#[test]
fn cooldown_read_fails_open() {
    let mem = Arc::new(InMemoryFacts::failing());
    let ledger = ledger_over(mem);
    let k = key(FindingKind::OodaStuck, "goal:g1");

    assert_eq!(ledger.allow_emit(&k, 0), CooldownDecision::Emit);
    assert_eq!(ledger.allow_emit(&k, 1_000_000), CooldownDecision::Emit);
}

/// The cooldown survives daemon exec-reload / restart: a FRESH ledger built over
/// the SAME durable memory still throttles an in-window key. This is the exact
/// failure the in-memory `WhisperGate` had — its reset re-opened the storm.
#[test]
fn cooldown_survives_reload() {
    let mem = Arc::new(InMemoryFacts::new());
    let k = key(FindingKind::OodaStuck, "goal:g1");

    {
        let ledger1 = ledger_over(mem.clone());
        assert_eq!(ledger1.allow_emit(&k, 0), CooldownDecision::Emit);
        ledger1.record_emit(&k, &sample_issue(1, "s"), 0).unwrap();
    } // ledger1 (and its in-process state) dropped — simulating exec-reload.

    let ledger2 = ledger_over(mem);
    assert_eq!(
        ledger2.allow_emit(&k, 900),
        CooldownDecision::Throttle,
        "a reloaded ledger must reconstruct the cooldown from durable memory"
    );
}

/// `subject` is canonicalized to `[a-z0-9:_-]`, so untrusted goal/gap text cannot
/// inject `gh --search` qualifiers (SR-V3), and two raw subjects that canonicalize
/// to the same value collapse to the same key.
#[test]
fn cooldown_subject_rejects_search_qualifiers() {
    let malicious = key(
        FindingKind::WorkstreamGapIssue,
        "goal is:issue label:\"pwn\" OR state:open --json",
    );
    let concept = malicious.fact_concept();

    assert!(
        concept.starts_with("overseer:issue-cooldown:"),
        "concept must live in the isolated namespace, got {concept}"
    );
    assert!(
        concept
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, ':' | '_' | '-')),
        "concept must be charset-restricted to [a-z0-9:_-], got {concept}"
    );
    for bad in [' ', '"', '\n', '\t'] {
        assert!(
            !concept.contains(bad),
            "canonical key must not contain {bad:?}"
        );
    }

    // Two raw subjects that canonicalize to the same value share one key.
    let k1 = key(FindingKind::OodaStuck, "Goal FOO");
    let k2 = key(FindingKind::OodaStuck, "goal___foo");
    assert_eq!(
        k1.fact_concept(),
        k2.fact_concept(),
        "canonicalization must collapse equivalent subjects to one stable key"
    );
}

/// Keys unseen for longer than the cap are prunable so the ledger stays bounded;
/// a fresh key is never pruned. `prune` returns the count reclaimed.
#[test]
fn cooldown_prunes_stale_keys() {
    let mem = Arc::new(InMemoryFacts::new());
    let ledger = ledger_over(mem);
    let stale = key(FindingKind::OodaStuck, "goal:old");
    let fresh = key(FindingKind::OodaStuck, "goal:new");

    ledger
        .record_emit(&stale, &sample_issue(1, "s"), 0)
        .unwrap();
    ledger
        .record_emit(&fresh, &sample_issue(2, "s"), CAP_SECS)
        .unwrap();

    // At now = CAP + 1: `stale` (age CAP+1 > cap) is evicted; `fresh` (age 1) stays.
    let pruned = ledger.prune(CAP_SECS + 1).unwrap();
    assert_eq!(pruned, 1, "exactly the one stale key should be reclaimed");
}

/// The durable fact holds only non-sensitive metadata
/// (`{ last_emit_secs, strikes, issue_number }`) — never the issue body or a
/// token (SR-D1/D2).
#[test]
fn cooldown_fact_stores_no_sensitive_body() {
    let mem = Arc::new(InMemoryFacts::new());
    let inspect = mem.clone();
    let ledger = ledger_over(mem);
    let k = key(FindingKind::OodaStuck, "goal:g1");

    let secret = "GHTOKEN-DEADBEEF-do-not-store-me";
    ledger
        .record_emit(&k, &sample_issue(4930, secret), 42)
        .unwrap();

    let facts = inspect.facts_for(&k.fact_concept());
    assert_eq!(facts.len(), 1);
    let content = &facts[0].content;
    assert!(
        !content.contains(secret),
        "issue body must never be persisted in the cooldown fact: {content}"
    );
    assert!(
        content.contains("4930"),
        "the issue number is the only issue-derived datum kept: {content}"
    );
}

// --- Config contract: the window floor is one OODA cycle -------------------

/// `SIMARD_OVERSEER_ISSUE_COOLDOWN_BASE_SECS` is clamped up to at least one full
/// OODA cycle (`overseer_interval_secs()`), so the same `(goal, finding)` can
/// never re-file every cycle — the storm's defining symptom.
#[test]
fn cooldown_window_floor_is_ooda_cycle() {
    // A base far below the cycle cadence is clamped up to the cadence.
    let clamped = issue_cooldown_base_secs_from(env(&[
        ("SIMARD_OVERSEER_ISSUE_COOLDOWN_BASE_SECS", "10"),
        ("SIMARD_OVERSEER_INTERVAL_SECS", "3600"),
    ]));
    assert_eq!(clamped, 3_600, "base must clamp up to one OODA cycle");

    // The documented default (nothing set) is 6 h, already above the floor.
    let default = issue_cooldown_base_secs_from(env(&[]));
    assert_eq!(default, BASE_SECS);
}

/// The cap defaults to 24 h and is clamped to be `>= base`.
#[test]
fn cooldown_cap_defaults_to_24h_and_clamps_to_base() {
    assert_eq!(issue_cooldown_cap_secs_from(env(&[])), CAP_SECS);

    // A cap below base is lifted to base (window can never be negative).
    let lifted = issue_cooldown_cap_secs_from(env(&[
        ("SIMARD_OVERSEER_ISSUE_COOLDOWN_BASE_SECS", "50000"),
        ("SIMARD_OVERSEER_ISSUE_COOLDOWN_MAX_SECS", "10"),
    ]));
    assert!(
        lifted >= 50_000,
        "cap must clamp up to >= base, got {lifted}"
    );
}

/// The rolling-hour emit budget defaults to 20.
#[test]
fn cooldown_cap_per_hour_defaults_to_20() {
    assert_eq!(issue_cooldown_cap_per_hour_from(env(&[])), 20);
}

/// The ledger is on by default and opt-out via the additive env flag.
#[test]
fn cooldown_enabled_by_default_and_opt_out() {
    assert!(issue_cooldown_enabled_from(env(&[])), "on by default");
    for off in ["0", "false", "no", "off"] {
        assert!(
            !issue_cooldown_enabled_from(env(&[("SIMARD_OVERSEER_ISSUE_COOLDOWN", off)])),
            "value {off:?} must disable the ledger"
        );
    }
}

/// Build an injectable env resolver from a fixed map — no `std::env` mutation, so
/// the config tests are hermetic and parallel-safe.
fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| map.get(k).cloned()
}
