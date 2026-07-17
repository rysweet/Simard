//! TDD contract (issue #893): the Overseer meta-thread must **self-heal** from a
//! *transient* cycle failure instead of latching in `"erroring"`.
//!
//! Background. #4080 already fixed the *isolated act-error* regression by adding
//! [`OverseerTickReport::cycle_failed`], so a single per-intervention `act`
//! failure no longer pins the thread — the daemon derives
//! `last_success = !panicked && !cycle_failed`. That is necessary but not
//! sufficient: a *genuine cycle failure* whose cause is transient/external (a
//! GitHub `503`, a socket timeout, a rate-limit) still latches the meta-thread in
//! `"erroring"` until a later clean tick, degrading the outer OODA stewardship
//! loop even though nothing is actually wrong with Simard.
//!
//! This module is the **failing** (red-phase) specification for the fix. It pins
//! four public contracts that do not yet exist in the codebase:
//!
//!   1. [`crate::overseer::wiring::is_transient`] — a pure, **fail-closed**
//!      classifier over [`OverseerError`]. Known-retryable capability failures
//!      (HTTP 5xx / `502` / `503`, timeouts, connection resets, rate limits) are
//!      transient; everything else — including unknown capability details and
//!      *every* non-`Capability` variant — is fatal.
//!   2. [`OverseerTickReport::transient_cycle_failure`] — an additive
//!      (`#[serde(default)]`, so `SCHEMA_VERSION` is unchanged) flag set **only**
//!      when the tick's `run_cycle` failed for a transient reason. A panic is
//!      never transient.
//!   3. [`OverseerThreadStatus::overseer_meta`] — extended to consume the report
//!      plus a running `consecutive_transient` count and a configured ceiling,
//!      and to map:
//!         * completed tick                                   → `"ok"`
//!         * transient failure, `consecutive_transient <= N`  → `"backoff"`
//!           (self-healing: `backoff_until = now + cadence`, `consecutive_errors = 0`)
//!         * transient failure, `consecutive_transient  > N`  → `"erroring"`
//!           (SR-2 bounded self-healing: a hard-down dependency cannot hide
//!           behind an infinite backoff)
//!         * fatal / panicked failure                         → `"erroring"`
//!   4. [`crate::overseer::config::overseer_transient_backoff_ceiling_from`] — a
//!      fail-safe env-injectable resolver for the ceiling `N`.
//!
//! All assertions here reference only PUBLIC API, so this file is a standalone,
//! self-contained module. Tick-level integration (that the `run_cycle` Err arm
//! actually *populates* `transient_cycle_failure` via `is_transient`) is pinned
//! separately inside `wiring.rs`'s test module, where the private capability
//! stubs live.

use crate::overseer::activity::OverseerThreadStatus;
use crate::overseer::capabilities::OverseerError;
use crate::overseer::config::{
    DEFAULT_OVERSEER_TRANSIENT_BACKOFF_CEILING, OVERSEER_TRANSIENT_BACKOFF_CEILING_ENV,
    overseer_transient_backoff_ceiling_from,
};
use crate::overseer::wiring::{OverseerTickReport, is_transient};

// ── helpers ────────────────────────────────────────────────────────────────

/// Injectable env resolver from a fixed slice — no `std::env` mutation.
fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

/// A capability failure carrying `detail`. Most transient signals arrive as a
/// `Capability` error whose `detail` string names the upstream fault.
fn cap(detail: &str) -> OverseerError {
    OverseerError::Capability {
        what: "status",
        detail: detail.to_string(),
    }
}

const CADENCE: u64 = 900;

// ── 1. is_transient: fail-closed classification ──────────────────────────────

#[test]
fn is_transient_flags_known_retryable_upstream_failures() {
    // Each of these is an external, self-clearing fault the next tick can retry.
    let transient = [
        "upstream returned HTTP 503 Service Unavailable",
        "github api: 502 Bad Gateway",
        "server error: 500 Internal Server Error",
        "request timed out after 30s",
        "connection reset by peer",
        "secondary rate limit exceeded (429), retry later",
        "service temporarily unavailable",
    ];
    for d in transient {
        assert!(
            is_transient(&cap(d)),
            "a known-retryable upstream fault must be classified transient: {d:?}"
        );
    }
}

#[test]
fn is_transient_is_fail_closed_for_unknown_capability_details() {
    // The wildcard arm MUST return false: an unrecognised capability detail is
    // treated as a real (fatal) defect so it still latches "erroring". This is
    // the SR-1 invariant — only an explicit allowlist routes to self-healing.
    let fatal = [
        "malformed goal board: unknown field `widget`",
        "assertion failed: invariant violated",
        "permission denied",
        "no such file or directory",
        "", // empty detail carries no transient signal → fatal
    ];
    for d in fatal {
        assert!(
            !is_transient(&cap(d)),
            "an unknown/opaque capability detail must be fatal (fail-closed): {d:?}"
        );
    }
}

#[test]
fn is_transient_is_false_for_every_non_capability_variant() {
    // Gate/budget/recursion/conflict/not-ready outcomes are DECISIONS, never
    // transient infrastructure faults — they must never route to backoff.
    let non_transient = [
        OverseerError::Gated {
            intervention: "deploy".to_string(),
            risk: "high",
        },
        OverseerError::Budget {
            spent_usd: 600.0,
            budget_usd: 500.0,
        },
        OverseerError::Recursion {
            subject: "own-pr".to_string(),
        },
        OverseerError::Conflict {
            with: "engineer-x".to_string(),
        },
        OverseerError::NotMergeReady {
            pr: 42,
            reason: "checks red".to_string(),
        },
    ];
    for e in &non_transient {
        assert!(
            !is_transient(e),
            "a non-Capability variant must never be transient: {e:?}"
        );
    }
}

// ── 2. transient_cycle_failure: additive, serde-default, SCHEMA unchanged ─────

#[test]
fn transient_cycle_failure_defaults_false_on_a_default_report() {
    let report = OverseerTickReport::default();
    assert!(
        !report.transient_cycle_failure,
        "a fresh report is not a transient failure"
    );
}

#[test]
fn transient_cycle_failure_deserializes_to_false_from_a_legacy_feed_record() {
    // Durable JSON written before this field existed must still parse: the
    // struct-level `#[serde(default)]` fills it with the CONSERVATIVE `false`
    // (SR-5) so a truncated/legacy record can never masquerade as transient and
    // downgrade a real failure. No `SCHEMA_VERSION` bump.
    let legacy = r#"{
        "problems": 2,
        "errors": 1,
        "panicked": false,
        "cycle_failed": true,
        "duration_ms": 7
    }"#;
    let report: OverseerTickReport = serde_json::from_str(legacy)
        .expect("a legacy record without transient_cycle_failure must parse");
    assert!(report.cycle_failed);
    assert!(
        !report.transient_cycle_failure,
        "a legacy record must default transient_cycle_failure to false (SR-5)"
    );

    // And a round-trip now emits the additive field explicitly.
    let json = serde_json::to_string(&report).expect("serialize");
    assert!(
        json.contains("\"transient_cycle_failure\":false"),
        "the additive field must be emitted on write: {json}"
    );
}

// ── 3. overseer_meta: self-healing health mapping ────────────────────────────
//
// Contract for the extended constructor:
//
//   overseer_meta(cadence_secs, &report, consecutive_transient, transient_ceiling)
//
// where `consecutive_transient` is the running count of consecutive transient
// failures INCLUDING the current tick, and `transient_ceiling` is the configured
// bound `N`. The daemon owns the counter (reset to 0 on any completed tick;
// incremented on a transient failure; left unchanged on a fatal failure).

fn meta(
    report: &OverseerTickReport,
    consecutive_transient: u32,
    ceiling: u32,
) -> OverseerThreadStatus {
    OverseerThreadStatus::overseer_meta(CADENCE, report, consecutive_transient, ceiling)
}

#[test]
fn a_completed_tick_is_ok_and_carries_no_backoff() {
    let clean = OverseerTickReport::default();
    let s = meta(&clean, 0, 3);
    assert_eq!(s.id, "overseer");
    assert_eq!(s.health, "ok");
    assert_eq!(s.consecutive_errors, 0);
    assert_eq!(s.backoff_until, None);
    assert_eq!(s.last_success, Some(true));
}

#[test]
fn an_isolated_act_error_still_reads_ok() {
    // Isolated act error, cycle otherwise healthy (#4080 recovery path): the
    // extended constructor must preserve the ok mapping.
    let isolated = OverseerTickReport {
        errors: 1,
        cycle_failed: false,
        panicked: false,
        ..OverseerTickReport::default()
    };
    let s = meta(&isolated, 0, 3);
    assert_eq!(s.health, "ok");
    assert_eq!(s.consecutive_errors, 0);
}

#[test]
fn a_transient_cycle_failure_self_heals_to_backoff_not_erroring() {
    // The core fix: a transient cycle failure within the ceiling must map to the
    // self-healing "backoff" state (which `derive_health` treats as healthy),
    // NOT "erroring". backoff_until is armed one cadence out and the escalating
    // consecutive_errors counter stays 0.
    let transient = OverseerTickReport {
        errors: 1,
        cycle_failed: true,
        transient_cycle_failure: true,
        panicked: false,
        ..OverseerTickReport::default()
    };
    for count in 1..=3u32 {
        let s = meta(&transient, count, 3);
        assert_eq!(
            s.health, "backoff",
            "consecutive_transient={count} within ceiling=3 must self-heal to backoff, not erroring"
        );
        assert!(
            s.backoff_until.is_some(),
            "a self-healing tick must arm backoff_until (one cadence out)"
        );
        assert_eq!(
            s.consecutive_errors, 0,
            "a transient backoff must not accrue the erroring counter"
        );
        assert_eq!(s.last_success, Some(false));
    }
}

#[test]
fn transient_failures_escalate_to_erroring_once_the_ceiling_is_exceeded() {
    // SR-2 bounded self-healing: a persistent "always transient" state must not
    // hide a hard-down dependency forever. Once the consecutive count exceeds the
    // ceiling, the thread escalates to "erroring".
    let transient = OverseerTickReport {
        errors: 1,
        cycle_failed: true,
        transient_cycle_failure: true,
        panicked: false,
        ..OverseerTickReport::default()
    };
    let s = meta(&transient, 4, 3);
    assert_eq!(
        s.health, "erroring",
        "consecutive_transient=4 exceeds ceiling=3 → escalate to erroring (SR-2)"
    );
    assert!(
        s.consecutive_errors >= 1,
        "an escalated failure must surface as an erroring count"
    );
    assert_eq!(
        s.backoff_until, None,
        "an escalated failure is no longer backing off"
    );
}

#[test]
fn a_fatal_cycle_failure_is_erroring_immediately() {
    // A non-transient (unknown/fatal) cycle failure must NOT self-heal: it maps
    // straight to "erroring" regardless of the transient counter.
    let fatal = OverseerTickReport {
        errors: 1,
        cycle_failed: true,
        transient_cycle_failure: false,
        panicked: false,
        ..OverseerTickReport::default()
    };
    let s = meta(&fatal, 0, 3);
    assert_eq!(s.health, "erroring");
    assert!(s.consecutive_errors >= 1);
    assert_eq!(s.backoff_until, None);
}

#[test]
fn a_panicked_tick_is_erroring_and_never_transient() {
    // A panic is the most severe cycle failure. Even if a (bogus) transient flag
    // were present, a panic must map to "erroring" — panics are never self-healed.
    let panicked = OverseerTickReport {
        panicked: true,
        cycle_failed: true,
        transient_cycle_failure: false,
        ..OverseerTickReport::default()
    };
    let s = meta(&panicked, 0, 3);
    assert_eq!(s.health, "erroring");
    assert_eq!(s.backoff_until, None);
    assert_eq!(s.last_success, Some(false));
}

// ── 4. config: fail-safe transient-backoff ceiling resolution ────────────────

#[test]
fn transient_backoff_ceiling_defaults_when_unset() {
    assert_eq!(
        overseer_transient_backoff_ceiling_from(env(&[])),
        DEFAULT_OVERSEER_TRANSIENT_BACKOFF_CEILING,
        "an unset ceiling must resolve to the built-in default"
    );
    const {
        assert!(
            DEFAULT_OVERSEER_TRANSIENT_BACKOFF_CEILING >= 1,
            "the default ceiling must permit at least one self-healing backoff"
        );
    }
}

#[test]
fn transient_backoff_ceiling_parses_explicit_values() {
    assert_eq!(
        overseer_transient_backoff_ceiling_from(env(&[(
            OVERSEER_TRANSIENT_BACKOFF_CEILING_ENV,
            "5"
        )])),
        5
    );
    // Surrounding whitespace is trimmed, matching the sibling `*_from` resolvers.
    assert_eq!(
        overseer_transient_backoff_ceiling_from(env(&[(
            OVERSEER_TRANSIENT_BACKOFF_CEILING_ENV,
            "  7  "
        )])),
        7
    );
}

#[test]
fn transient_backoff_ceiling_is_fail_safe_for_garbage_or_empty() {
    for bad in ["", "  ", "abc", "-1", "not-a-number"] {
        assert_eq!(
            overseer_transient_backoff_ceiling_from(env(&[(
                OVERSEER_TRANSIENT_BACKOFF_CEILING_ENV,
                bad
            )])),
            DEFAULT_OVERSEER_TRANSIENT_BACKOFF_CEILING,
            "a garbage ceiling {bad:?} must fall back to the default, never disable self-healing"
        );
    }
}
