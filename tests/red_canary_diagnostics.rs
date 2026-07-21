// tests/red_canary_diagnostics.rs
//
// TDD (Step 7 — tests first) for the red-canary deploy-refusal diagnostics and
// the corrected candidate RPC-health gate (rysweet/Simard, Problem 1).
//
// These tests specify the CONTRACT for the implementation and are expected to
// FAIL until it lands. They are deliberately confined to this integration test
// target so the not-yet-existing symbols (`RedCanaryDetail`, `first_failure`,
// the `DeployRefusal::RedCanary(RedCanaryDetail)` payload, and the additive
// `DeployContext::red_canary_detail` field) fail to compile HERE only — the
// library and every other test target still build.
//
// Contract source of truth:
//   docs/reference/canary-gate-diagnostics-api.md
//   docs/concepts/canary-gate-diagnostics.md
//
// Brick 1 (diagnostics, side-channel — must be verdict-identical):
//   * `first_failure(&[GateResult]) -> Option<&GateResult>` — first failing
//     gate in gate order; `None` when all pass.
//   * `RedCanaryDetail { failed_gate: String, detail: String }` — additive,
//     `Default`-able; `from_results` + `summary`; sanitized + length-bounded.
//   * `DeployRefusal::RedCanary(RedCanaryDetail)` — enriched payload; `Display`
//     names the gate, falling back to the legacy wording for a `Default` value.
//   * `DeployContext` gains an additive `red_canary_detail: RedCanaryDetail`.
//   * The deploy notification carries the sanitized failing-gate summary at
//     dual-channel parity (invariant #2590 preserved).
//
// Brick 2 (fix — evidence-gated): the RPC-health gate is fail-closed and never
// default-passes. A drifted live daemon can no longer redden a healthy
// candidate (the positive path needs a real build and is exercised elsewhere;
// here we pin the fail-closed contract that must always hold).
//
// SAFETY: `verify_canary` is only ever driven with curated gate lists that
// EXCLUDE `RelaunchGate::UnitTest`, whose real impl shells `cargo test` and
// would recurse for 30+ minutes under `cargo test`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use simard::overseer::deploy::{
    CRASH_LOOP_CHURN_THRESHOLD, DeployContext, DeployRefusal, evaluate_deploy_gate,
};
use simard::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};
use simard::self_relaunch::{
    GateResult, RedCanaryDetail, RelaunchConfig, RelaunchGate, first_failure, verify_canary,
};

// ─────────────────────────── test helpers ──────────────────────────────────

fn pass(gate: RelaunchGate) -> GateResult {
    GateResult {
        gate,
        passed: true,
        detail: "ok".to_string(),
    }
}

fn fail(gate: RelaunchGate, detail: &str) -> GateResult {
    GateResult {
        gate,
        passed: false,
        detail: detail.to_string(),
    }
}

/// A base, clean-forward deploy context that includes the NEW additive
/// `red_canary_detail` field (empty by default — an all-passing canary).
fn base_ctx() -> DeployContext {
    DeployContext {
        running_commit: "aaaaaaaaaaaa".to_string(),
        target_commit: "bbbbbbbbbbbb".to_string(),
        target_is_ancestor_of_running: false,
        canary_passed: true,
        recent_restart_churn: 0,
        red_canary_detail: RedCanaryDetail::default(),
    }
}

// ─────────────────── Brick 1: first_failure accessor ───────────────────────

#[test]
fn first_failure_is_none_when_all_gates_pass() {
    let results = vec![
        pass(RelaunchGate::Smoke),
        pass(RelaunchGate::GymBaseline),
        pass(RelaunchGate::RpcHealth),
    ];
    assert!(first_failure(&results).is_none(), "no failing gate ⇒ None");
}

#[test]
fn first_failure_returns_the_single_failing_gate() {
    let results = vec![
        pass(RelaunchGate::Smoke),
        fail(RelaunchGate::RpcHealth, "connection refused"),
    ];
    let f = first_failure(&results).expect("one gate failed");
    assert_eq!(f.gate, RelaunchGate::RpcHealth);
    assert!(f.detail.contains("connection refused"), "{}", f.detail);
}

#[test]
fn first_failure_is_deterministic_first_in_gate_order() {
    // Two gates fail; the accessor must name the FIRST in slice/gate order,
    // never the last, so the reported culprit is deterministic.
    let results = vec![
        pass(RelaunchGate::Smoke),
        fail(RelaunchGate::GymBaseline, "gym probe failed"),
        fail(RelaunchGate::RpcHealth, "rpc health failed"),
    ];
    let f = first_failure(&results).expect("a failure");
    assert_eq!(
        f.gate,
        RelaunchGate::GymBaseline,
        "first failing gate in order must win"
    );
}

#[test]
fn first_failure_on_empty_results_is_none() {
    assert!(first_failure(&[]).is_none());
}

// ─────────────────── Brick 1: RedCanaryDetail payload ───────────────────────

#[test]
fn red_canary_detail_default_is_empty() {
    let d = RedCanaryDetail::default();
    assert_eq!(d.failed_gate, "");
    assert_eq!(d.detail, "");
}

#[test]
fn red_canary_detail_from_results_is_default_when_all_pass() {
    let results = vec![pass(RelaunchGate::Smoke), pass(RelaunchGate::RpcHealth)];
    let d = RedCanaryDetail::from_results(&results);
    assert_eq!(d, RedCanaryDetail::default(), "all-green ⇒ empty detail");
}

#[test]
fn red_canary_detail_from_results_names_first_failing_gate() {
    let results = vec![
        pass(RelaunchGate::Smoke),
        fail(RelaunchGate::RpcHealth, "connection refused"),
    ];
    let d = RedCanaryDetail::from_results(&results);
    assert_eq!(d.failed_gate, "rpc-health", "gate slug must be recorded");
    assert!(d.detail.contains("connection refused"), "{}", d.detail);
}

#[test]
fn red_canary_detail_from_results_is_deterministic() {
    let results = vec![
        fail(RelaunchGate::Smoke, "binary exited 1"),
        fail(RelaunchGate::RpcHealth, "rpc down"),
    ];
    let d = RedCanaryDetail::from_results(&results);
    assert_eq!(d.failed_gate, "smoke", "first failing gate in order");
}

#[test]
fn red_canary_detail_summary_names_the_gate_and_reason() {
    let d = RedCanaryDetail {
        failed_gate: "rpc-health".to_string(),
        detail: "connection refused".to_string(),
    };
    let s = d.summary();
    assert!(s.contains("rpc-health"), "summary names the gate: {s}");
    assert!(
        s.contains("connection refused"),
        "summary carries reason: {s}"
    );
}

#[test]
fn red_canary_detail_summary_default_uses_legacy_wording() {
    // A Default (empty) payload is equivalent to the prior, detail-free refusal,
    // so its one-liner must be the legacy wording the operator already knows.
    let s = RedCanaryDetail::default().summary();
    assert!(
        s.to_lowercase().contains("one or more gates failed"),
        "default summary must fall back to legacy wording: {s}"
    );
}

#[test]
fn red_canary_detail_derives_value_semantics() {
    let a = RedCanaryDetail {
        failed_gate: "smoke".to_string(),
        detail: "boom".to_string(),
    };
    let b = a.clone();
    assert_eq!(a, b, "Clone + PartialEq/Eq");
    let dbg = format!("{a:?}");
    assert!(dbg.contains("smoke"), "Debug renders fields: {dbg}");
}

// ─────────────── Brick 1: telemetry hygiene (sanitization) ──────────────────

#[test]
fn red_canary_detail_is_length_bounded() {
    // Candidate output is untrusted; the surfaced detail must be length-bounded
    // (≤ 512 chars per the hygiene contract) so a huge stderr cannot flood logs
    // or a notification.
    let huge = "x".repeat(5_000);
    let results = vec![fail(RelaunchGate::RpcHealth, &huge)];
    let d = RedCanaryDetail::from_results(&results);
    assert!(
        d.detail.len() <= 512,
        "detail must be length-bounded (≤512), got {}",
        d.detail.len()
    );
    assert!(
        d.summary().len() <= 640,
        "summary must stay bounded, got {}",
        d.summary().len()
    );
}

#[test]
fn red_canary_detail_handles_multibyte_utf8_without_panicking() {
    // Char-boundary-safe truncation: a multi-byte tail must not panic.
    let huge = "héllo wörld café ".repeat(200);
    let results = vec![fail(RelaunchGate::RpcHealth, &huge)];
    let d = RedCanaryDetail::from_results(&results);
    assert!(d.detail.len() <= 512, "bounded: {}", d.detail.len());
}

// ─────────── Brick 1: DeployRefusal::RedCanary carries the detail ───────────

#[test]
fn evaluate_deploy_gate_red_canary_carries_detail() {
    let mut c = base_ctx();
    c.canary_passed = false;
    c.red_canary_detail = RedCanaryDetail {
        failed_gate: "rpc-health".to_string(),
        detail: "rpc health failed (exit 1): connection refused".to_string(),
    };
    match evaluate_deploy_gate(&c) {
        Err(DeployRefusal::RedCanary(detail)) => {
            assert_eq!(detail.failed_gate, "rpc-health");
            assert!(
                detail.detail.contains("connection refused"),
                "{}",
                detail.detail
            );
        }
        other => panic!("expected RedCanary(detail), got {other:?}"),
    }
}

#[test]
fn red_canary_display_names_the_gate() {
    let refusal = DeployRefusal::RedCanary(RedCanaryDetail {
        failed_gate: "rpc-health".to_string(),
        detail: "connection refused".to_string(),
    });
    let s = refusal.to_string();
    assert!(s.contains("red canary"), "keeps the red-canary label: {s}");
    assert!(s.contains("rpc-health"), "names the failing gate: {s}");
}

#[test]
fn red_canary_display_default_payload_uses_legacy_wording() {
    let refusal = DeployRefusal::RedCanary(RedCanaryDetail::default());
    assert!(
        refusal.to_string().contains("one or more gates failed"),
        "empty payload ⇒ legacy wording: {refusal}"
    );
}

// ─────────── Brick 1: verdict-identical (diagnostic is side-channel) ────────

#[test]
fn gate_still_allows_a_clean_forward_deploy() {
    assert!(
        evaluate_deploy_gate(&base_ctx()).is_ok(),
        "diagnostics must not change the clean-forward verdict"
    );
}

#[test]
fn gate_still_refuses_red_canary_even_with_empty_diagnostic() {
    // The verdict depends only on `canary_passed`; an empty RedCanaryDetail must
    // still produce a refusal (proving the diagnostic is purely side-channel).
    let mut c = base_ctx();
    c.canary_passed = false; // red_canary_detail stays Default
    assert!(matches!(
        evaluate_deploy_gate(&c),
        Err(DeployRefusal::RedCanary(_))
    ));
}

#[test]
fn gate_verdicts_for_other_shapes_are_unchanged() {
    // no-op
    let mut c = base_ctx();
    c.target_commit = c.running_commit.clone();
    assert_eq!(evaluate_deploy_gate(&c), Err(DeployRefusal::NoOp));

    // rollback
    let mut c = base_ctx();
    c.target_is_ancestor_of_running = true;
    assert_eq!(evaluate_deploy_gate(&c), Err(DeployRefusal::Rollback));

    // crash-loop
    let mut c = base_ctx();
    c.recent_restart_churn = CRASH_LOOP_CHURN_THRESHOLD;
    assert_eq!(
        evaluate_deploy_gate(&c),
        Err(DeployRefusal::CrashLoop {
            churn: CRASH_LOOP_CHURN_THRESHOLD
        })
    );
}

// ─────────── Brick 1: dual-channel notification parity (#2590) ──────────────

struct Capture {
    channel: &'static str,
    seen: Arc<Mutex<Vec<OperatorNotification>>>,
}

impl NotifyChannel for Capture {
    fn name(&self) -> &str {
        self.channel
    }
    fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
        self.seen.lock().unwrap().push(n.clone());
        ChannelDelivery::Sent
    }
}

#[test]
fn red_canary_notification_carries_detail_at_dual_channel_parity() {
    let detail = RedCanaryDetail {
        failed_gate: "rpc-health".to_string(),
        detail: "rpc health failed (exit 1): connection refused".to_string(),
    };

    // The diagnostic rides inside the EXISTING deploy notification via its
    // `gate_summary` argument — no new constructor, no new variant.
    let note = OperatorNotification::deploy(
        "bbbbbbbbbbbb",
        "aaaaaaaaaaaa",
        "rysweet/Simard",
        &detail.summary(),
    );

    let signal_seen = Arc::new(Mutex::new(Vec::new()));
    let email_seen = Arc::new(Mutex::new(Vec::new()));
    let notifier = DualChannelNotifier::new(vec![
        Box::new(Capture {
            channel: "signal",
            seen: signal_seen.clone(),
        }),
        Box::new(Capture {
            channel: "email",
            seen: email_seen.clone(),
        }),
    ]);

    let report = notifier.notify(&note);
    assert!(
        report.dispatched(),
        "invariant #2590: notification must dispatch"
    );

    let s = signal_seen.lock().unwrap();
    let e = email_seen.lock().unwrap();
    assert_eq!(s.len(), 1, "signal channel fired once");
    assert_eq!(e.len(), 1, "email channel fired once");
    assert_eq!(s[0], e[0], "dual-channel parity: identical notification");

    let body = s[0].plain_text();
    assert!(
        body.contains("rpc-health"),
        "notification body names the failing gate: {body}"
    );
}

// ─────────── Brick 2: candidate RPC-health gate is fail-closed ──────────────

#[test]
fn rpc_health_gate_fails_closed_for_a_missing_candidate_binary() {
    // A spawn error (missing candidate binary) must score the gate RED — never
    // a default-pass. Curated single-gate list; no UnitTest.
    let config = RelaunchConfig::default();
    let results = verify_canary(
        Path::new("/no-such-simard-candidate-binary-38217"),
        &[RelaunchGate::RpcHealth],
        &config,
    )
    .expect("verify_canary returns results, not an error");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].gate, RelaunchGate::RpcHealth);
    assert!(
        !results[0].passed,
        "rpc-health must fail-closed for a missing candidate binary"
    );
}

#[test]
fn rpc_health_gate_fails_closed_on_non_zero_probe_exit() {
    // A candidate whose probe exits non-zero must score RED (never default-pass).
    // `/usr/bin/false` exits 1 regardless of args.
    if !Path::new("/usr/bin/false").exists() {
        return; // platform without the coreutil — skip rather than false-fail
    }
    let config = RelaunchConfig::default();
    let results = verify_canary(
        Path::new("/usr/bin/false"),
        &[RelaunchGate::RpcHealth],
        &config,
    )
    .expect("verify_canary returns results");
    assert!(
        !results[0].passed,
        "rpc-health must fail-closed on a non-zero probe exit"
    );
}

// ─────────── Brick 2: verify_canary surfaces the failed gate ────────────────

#[test]
fn verify_canary_run_surfaces_first_failed_gate_via_diagnostics() {
    // Drive a real (curated) canary run against a missing binary — every gate
    // fails — and prove the diagnostics name the FIRST failing gate in order.
    // Excludes UnitTest (would recurse into `cargo test`).
    let config = RelaunchConfig::default();
    let gates = [
        RelaunchGate::Smoke,
        RelaunchGate::GymBaseline,
        RelaunchGate::RpcHealth,
    ];
    let results = verify_canary(Path::new("/no-such-binary-99917"), &gates, &config)
        .expect("verify_canary returns results");
    assert_eq!(results.len(), 3, "all curated gates run (no short-circuit)");

    let f = first_failure(&results).expect("at least one gate failed");
    assert_eq!(
        f.gate,
        RelaunchGate::Smoke,
        "smoke is first in gate order, so it is the reported culprit"
    );

    let detail = RedCanaryDetail::from_results(&results);
    assert_eq!(detail.failed_gate, "smoke");
    assert!(
        !detail.detail.is_empty(),
        "detail is populated from the gate"
    );
}
