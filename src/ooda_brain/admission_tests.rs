//! Hermetic tests for RESOURCE-AWARE engineer admission.
//!
//! These prove the two things the framing requires of Step-7 tests:
//!   1. **The seam** — reason → apply: a (stubbed) brain decision flows through
//!      the hard rail into the resolved [`AdmissionGate`], and a brain *error*
//!      surfaces (NO FALLBACK).
//!   2. **The hard rail** — a known disk% at/above the ceiling BLOCKS admission
//!      regardless of what the brain decided (ENOSPC is never reachable), and
//!      fails *open* when disk% is unknown.
//!
//! The reasoning *quality* (when to ADMIT vs DEFER vs RECLAIM) lives in the
//! prompt, not here — so the brain is stubbed and only the deterministic seam +
//! rail are asserted. Everything here is pure and side-effect free.

use super::admission::{
    AdmissionDecision, AdmissionGate, CEILING_MAX, CEILING_MIN, DEFAULT_CEILING_PCT,
    DeterministicAdmissionBrain, OodaAdmissionBrain, ResourceAdmissionCtx, clamp_ceiling,
    configured_ceiling_pct, judge_and_resolve, parse_ceiling, resolve_admission,
};
use crate::error::{SimardError, SimardResult};

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

/// A hermetic stub brain returning a preset decision (or error). No IO, no LLM.
struct StubAdmissionBrain {
    result: SimardResult<AdmissionDecision>,
}

impl StubAdmissionBrain {
    fn admit() -> Self {
        Self {
            result: Ok(AdmissionDecision::Admit {
                rationale: "stub: admit".to_string(),
            }),
        }
    }
    fn defer() -> Self {
        Self {
            result: Ok(AdmissionDecision::Defer {
                rationale: "stub: defer".to_string(),
            }),
        }
    }
    fn reclaim() -> Self {
        Self {
            result: Ok(AdmissionDecision::ReclaimFirst {
                rationale: "stub: reclaim".to_string(),
            }),
        }
    }
    fn failing() -> Self {
        Self {
            result: Err(SimardError::AdapterInvocationFailed {
                base_type: "resource-admission".to_string(),
                reason: "stub brain intentionally failing".to_string(),
            }),
        }
    }
}

impl OodaAdmissionBrain for StubAdmissionBrain {
    fn judge_admission(&self, _ctx: &ResourceAdmissionCtx) -> SimardResult<AdmissionDecision> {
        // Clone the preset result so the stub can be consulted repeatedly
        // (repeated structured thought — one call per admission cycle).
        match &self.result {
            Ok(d) => Ok(d.clone()),
            Err(e) => Err(SimardError::AdapterInvocationFailed {
                base_type: "resource-admission".to_string(),
                reason: format!("{e}"),
            }),
        }
    }
}

fn ctx(disk_usage_pct: Option<u8>, ceiling_pct: u8) -> ResourceAdmissionCtx {
    ResourceAdmissionCtx {
        disk_usage_pct,
        worktree_cache_bytes: Some(12_345),
        load_avg_1m: Some(1.5),
        cpu_count: Some(8),
        in_flight_engineers: 2,
        ceiling_pct,
    }
}

// ---------------------------------------------------------------------------
// AdmissionDecision — serde round-trip (matches the LLM JSON schema)
// ---------------------------------------------------------------------------

#[test]
fn decision_admit_roundtrips() {
    let raw = r#"{"choice":"admit","rationale":"disk 40%, load light"}"#;
    let parsed: AdmissionDecision = serde_json::from_str(raw).expect("parse");
    assert_eq!(
        parsed,
        AdmissionDecision::Admit {
            rationale: "disk 40%, load light".to_string()
        }
    );
    assert_eq!(parsed.label(), "admit");
    assert_eq!(parsed.rationale(), "disk 40%, load light");
}

#[test]
fn decision_defer_roundtrips() {
    let raw = r#"{"choice":"defer","rationale":"disk 88%, backing off"}"#;
    let parsed: AdmissionDecision = serde_json::from_str(raw).expect("parse");
    assert_eq!(parsed.label(), "defer");
    assert!(matches!(parsed, AdmissionDecision::Defer { .. }));
}

#[test]
fn decision_reclaim_first_roundtrips() {
    let raw = r#"{"choice":"reclaim_first","rationale":"cache huge, reclaim then retry"}"#;
    let parsed: AdmissionDecision = serde_json::from_str(raw).expect("parse");
    assert_eq!(parsed.label(), "reclaim_first");
    assert!(matches!(parsed, AdmissionDecision::ReclaimFirst { .. }));
}

#[test]
fn decision_ignores_extra_fields() {
    // Forward-compat: the prompt may add fields the enum doesn't model yet.
    let raw = r#"{"choice":"admit","rationale":"ok","confidence":0.9,"future":42}"#;
    let parsed: AdmissionDecision = serde_json::from_str(raw).expect("parse");
    assert!(matches!(parsed, AdmissionDecision::Admit { .. }));
}

#[test]
fn decision_unknown_choice_fails_to_parse() {
    // An unknown tag must NOT silently deserialize to Admit — the caller
    // surfaces the parse error instead of a phantom admission.
    let raw = r#"{"choice":"launch_the_missiles","rationale":"nope"}"#;
    let parsed: Result<AdmissionDecision, _> = serde_json::from_str(raw);
    assert!(parsed.is_err());
}

// ---------------------------------------------------------------------------
// HARD RAIL (FR-5) — the safety-critical deterministic guard
// ---------------------------------------------------------------------------

#[test]
fn rail_blocks_admit_at_ceiling_boundary() {
    // disk == ceiling must block (>= comparison, not >).
    let gate = resolve_admission(
        &ctx(Some(90), 90),
        AdmissionDecision::Admit {
            rationale: "brain wanted to admit".to_string(),
        },
    );
    assert!(
        !gate.is_proceed(),
        "disk == ceiling must block admission, got {gate:?}"
    );
    assert!(matches!(gate, AdmissionGate::Defer { .. }));
}

#[test]
fn rail_blocks_admit_above_ceiling() {
    let gate = resolve_admission(
        &ctx(Some(97), 90),
        AdmissionDecision::Admit {
            rationale: "brain wanted to admit".to_string(),
        },
    );
    assert!(!gate.is_proceed());
    if let AdmissionGate::Defer { reason } = gate {
        assert!(reason.contains("hard-rail"), "reason={reason}");
        assert!(reason.contains("97"));
        assert!(reason.contains("90"));
    } else {
        panic!("expected Defer, got {gate:?}");
    }
}

#[test]
fn rail_allows_admit_below_ceiling() {
    let gate = resolve_admission(
        &ctx(Some(89), 90),
        AdmissionDecision::Admit {
            rationale: "healthy".to_string(),
        },
    );
    assert_eq!(gate, AdmissionGate::Proceed);
}

#[test]
fn rail_fails_open_when_disk_unknown() {
    // A failed `df` probe (None) must NOT block — a spurious block would
    // deadlock all progress. The unknown was already handed to the reasoner.
    let gate = resolve_admission(
        &ctx(None, 90),
        AdmissionDecision::Admit {
            rationale: "healthy, disk read failed".to_string(),
        },
    );
    assert_eq!(gate, AdmissionGate::Proceed);
}

#[test]
fn rail_never_proceeds_when_over_ceiling_for_any_decision() {
    // The core safety invariant: over-ceiling ⇒ never Proceed, for EVERY
    // possible brain decision.
    let over = ctx(Some(95), 90);
    for decision in [
        AdmissionDecision::Admit {
            rationale: "a".to_string(),
        },
        AdmissionDecision::Defer {
            rationale: "d".to_string(),
        },
        AdmissionDecision::ReclaimFirst {
            rationale: "r".to_string(),
        },
    ] {
        let gate = resolve_admission(&over, decision.clone());
        assert!(
            !gate.is_proceed(),
            "decision {decision:?} produced Proceed while disk 95% >= ceiling 90% (ENOSPC reachable!)"
        );
    }
}

// ---------------------------------------------------------------------------
// Decision mapping (FR-4) under healthy disk
// ---------------------------------------------------------------------------

#[test]
fn admit_maps_to_proceed_when_healthy() {
    let gate = resolve_admission(
        &ctx(Some(30), 90),
        AdmissionDecision::Admit {
            rationale: "plenty of headroom".to_string(),
        },
    );
    assert_eq!(gate, AdmissionGate::Proceed);
}

#[test]
fn defer_maps_to_defer_and_preserves_reason() {
    let gate = resolve_admission(
        &ctx(Some(30), 90),
        AdmissionDecision::Defer {
            rationale: "load spiking, back off one cycle".to_string(),
        },
    );
    assert_eq!(
        gate,
        AdmissionGate::Defer {
            reason: "load spiking, back off one cycle".to_string()
        }
    );
}

#[test]
fn reclaim_first_maps_to_reclaim_and_preserves_reason() {
    let gate = resolve_admission(
        &ctx(Some(30), 90),
        AdmissionDecision::ReclaimFirst {
            rationale: "build cache is 200GB, reclaim then retry".to_string(),
        },
    );
    assert_eq!(
        gate,
        AdmissionGate::Reclaim {
            reason: "build cache is 200GB, reclaim then retry".to_string()
        }
    );
}

#[test]
fn reclaim_first_honored_even_over_ceiling() {
    // Over the ceiling, a ReclaimFirst is strictly more proactive than a plain
    // block — it must be honored (still non-admitting).
    let gate = resolve_admission(
        &ctx(Some(93), 90),
        AdmissionDecision::ReclaimFirst {
            rationale: "over ceiling, reclaim".to_string(),
        },
    );
    assert!(matches!(gate, AdmissionGate::Reclaim { .. }));
    assert!(!gate.is_proceed());
}

// ---------------------------------------------------------------------------
// SEAM (FR-1/FR-2) — reason → apply via a stub brain
// ---------------------------------------------------------------------------

#[test]
fn seam_admit_decision_proceeds_when_healthy() {
    let gate = judge_and_resolve(&StubAdmissionBrain::admit(), &ctx(Some(50), 90)).unwrap();
    assert_eq!(gate, AdmissionGate::Proceed);
}

#[test]
fn seam_defer_decision_defers() {
    let gate = judge_and_resolve(&StubAdmissionBrain::defer(), &ctx(Some(50), 90)).unwrap();
    assert!(matches!(gate, AdmissionGate::Defer { .. }));
}

#[test]
fn seam_reclaim_decision_reclaims() {
    let gate = judge_and_resolve(&StubAdmissionBrain::reclaim(), &ctx(Some(50), 90)).unwrap();
    assert!(matches!(gate, AdmissionGate::Reclaim { .. }));
}

#[test]
fn seam_rail_overrides_brain_admit_when_over_ceiling() {
    // Even when the (stubbed) brain says ADMIT, the seam's hard rail blocks it
    // if disk is at/over the ceiling. This is the whole point of the feature.
    let gate = judge_and_resolve(&StubAdmissionBrain::admit(), &ctx(Some(92), 90)).unwrap();
    assert!(
        !gate.is_proceed(),
        "rail must override brain ADMIT, got {gate:?}"
    );
    assert!(matches!(gate, AdmissionGate::Defer { .. }));
}

#[test]
fn seam_surfaces_brain_error_no_fallback() {
    // NO FALLBACK: a brain error must propagate, not silently become an admit.
    let result = judge_and_resolve(&StubAdmissionBrain::failing(), &ctx(Some(10), 90));
    assert!(
        result.is_err(),
        "brain error must surface as an Err, not a phantom admission: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// DeterministicAdmissionBrain — no-LLM floor, still beaten by the rail
// ---------------------------------------------------------------------------

#[test]
fn deterministic_brain_admits() {
    let d = DeterministicAdmissionBrain
        .judge_admission(&ctx(Some(10), 90))
        .unwrap();
    assert!(matches!(d, AdmissionDecision::Admit { .. }));
}

#[test]
fn deterministic_brain_admit_proceeds_when_healthy() {
    let gate = judge_and_resolve(&DeterministicAdmissionBrain, &ctx(Some(10), 90)).unwrap();
    assert_eq!(gate, AdmissionGate::Proceed);
}

#[test]
fn deterministic_brain_still_blocked_by_rail_over_ceiling() {
    // The no-LLM floor always admits — but the deterministic rail still guards
    // ENOSPC over the ceiling.
    let gate = judge_and_resolve(&DeterministicAdmissionBrain, &ctx(Some(99), 90)).unwrap();
    assert!(
        !gate.is_proceed(),
        "rail must protect even the deterministic floor"
    );
}

// ---------------------------------------------------------------------------
// Ceiling parse / clamp (A4)
// ---------------------------------------------------------------------------

#[test]
fn parse_ceiling_defaults_when_absent_or_garbage() {
    assert_eq!(parse_ceiling(None), DEFAULT_CEILING_PCT);
    assert_eq!(parse_ceiling(Some("")), DEFAULT_CEILING_PCT);
    assert_eq!(parse_ceiling(Some("not-a-number")), DEFAULT_CEILING_PCT);
    // 300 does not fit in u8, so parse fails → default (not clamped).
    assert_eq!(parse_ceiling(Some("300")), DEFAULT_CEILING_PCT);
}

#[test]
fn parse_ceiling_reads_and_trims_valid_values() {
    assert_eq!(parse_ceiling(Some("90")), 90);
    assert_eq!(parse_ceiling(Some("  85 ")), 85);
}

#[test]
fn parse_ceiling_clamps_out_of_range() {
    // 0 would block everything forever; 250 (a valid u8) is too high.
    assert_eq!(parse_ceiling(Some("0")), CEILING_MIN);
    assert_eq!(parse_ceiling(Some("250")), CEILING_MAX);
    assert_eq!(clamp_ceiling(0), CEILING_MIN);
    assert_eq!(clamp_ceiling(255), CEILING_MAX);
    assert_eq!(clamp_ceiling(50), 50);
}

#[test]
fn configured_ceiling_is_in_range() {
    // Whatever the ambient env, the configured ceiling is always usable.
    let c = configured_ceiling_pct();
    assert!(
        (CEILING_MIN..=CEILING_MAX).contains(&c),
        "ceiling {c} out of range"
    );
}

// ---------------------------------------------------------------------------
// Context serde (A8) — the recipe reads this as JSON context
// ---------------------------------------------------------------------------

#[test]
fn ctx_roundtrips_including_unknown_probes() {
    let original = ResourceAdmissionCtx {
        disk_usage_pct: None, // probe failed
        worktree_cache_bytes: Some(999),
        load_avg_1m: None,
        cpu_count: Some(4),
        in_flight_engineers: 3,
        ceiling_pct: 90,
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let back: ResourceAdmissionCtx = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, back);
}
