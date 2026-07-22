//! Tests for [`super::health`]: the `SelfHealthReport` AND-of-probes invariant
//! and the durable `simard self-health --json` shape.

use super::health::{
    BrainsLlmBackedProbe, EntrypointParityProbe, GoalBoardIntactProbe, MemoryIntactProbe,
    NoQuarantineProbe, SelfHealthProbes, SelfHealthReport, VersionAdvancedProbe,
};

fn all_healthy_probes() -> SelfHealthProbes {
    SelfHealthProbes {
        version_advanced: VersionAdvancedProbe {
            healthy: true,
            running: "deadbeef".to_string(),
            target: "deadbeef".to_string(),
        },
        memory_intact: MemoryIntactProbe {
            healthy: true,
            live_facts: 1206,
            baseline_facts: Some(1206),
        },
        goal_board_intact: GoalBoardIntactProbe {
            healthy: true,
            active_goals: 5,
        },
        brains_llm_backed: BrainsLlmBackedProbe {
            healthy: true,
            fallback_records: 0,
        },
        no_quarantine: NoQuarantineProbe {
            healthy: true,
            quarantined: false,
        },
        entrypoint_parity: EntrypointParityProbe {
            healthy: true,
            installed_version: "simard 0.35.0".to_string(),
            path_version: "simard 0.35.0".to_string(),
            resolved_path: "/home/you/.local/bin/simard".to_string(),
            canonical_path: "/home/you/.simard/bin/simard".to_string(),
            path_mismatch: false,
            foreign_shadow: false,
        },
    }
}

#[test]
fn report_is_healthy_only_when_every_probe_is_healthy() {
    let report = SelfHealthReport::compute(all_healthy_probes());
    assert!(report.healthy);
    assert!(report.is_healthy());
    assert!(report.probes.all_healthy());
}

#[test]
fn any_single_unhealthy_probe_fails_the_report() {
    // Each probe in turn flips the whole report unhealthy (AND semantics).
    let mut p = all_healthy_probes();
    p.memory_intact.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);

    let mut p = all_healthy_probes();
    p.version_advanced.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);

    let mut p = all_healthy_probes();
    p.goal_board_intact.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);

    let mut p = all_healthy_probes();
    p.brains_llm_backed.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);

    let mut p = all_healthy_probes();
    p.no_quarantine.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);

    let mut p = all_healthy_probes();
    p.entrypoint_parity.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);
}

#[test]
fn report_serializes_with_documented_top_level_keys() {
    let report = SelfHealthReport::compute(all_healthy_probes());
    let v: serde_json::Value = serde_json::to_value(&report).unwrap();
    assert!(v.get("healthy").is_some());
    let probes = v.get("probes").expect("probes object");
    for key in [
        "version_advanced",
        "memory_intact",
        "goal_board_intact",
        "brains_llm_backed",
        "no_quarantine",
        "entrypoint_parity",
    ] {
        assert!(probes.get(key).is_some(), "probes.{key} must be present");
    }
}

#[test]
fn report_deserializes_from_documented_json_and_is_unhealthy() {
    // Pin the durable schema from docs/reference/self-deploy-api.md#self-health-output.
    let doc_json = r#"{
        "healthy": false,
        "probes": {
            "version_advanced": { "healthy": true,  "running": "abc123", "target": "abc123" },
            "memory_intact":    { "healthy": false, "live_facts": 1180, "baseline_facts": 1206 },
            "goal_board_intact":{ "healthy": true,  "active_goals": 5 },
            "brains_llm_backed":{ "healthy": true,  "fallback_records": 0 },
            "no_quarantine":    { "healthy": true,  "quarantined": false }
        }
    }"#;
    let report: SelfHealthReport = serde_json::from_str(doc_json).unwrap();
    assert!(!report.healthy);
    assert_eq!(report.probes.memory_intact.live_facts, 1180);
    assert_eq!(report.probes.memory_intact.baseline_facts, Some(1206));
    assert!(!report.probes.memory_intact.healthy);
    assert_eq!(report.probes.goal_board_intact.active_goals, 5);
    assert_eq!(report.probes.brains_llm_backed.fallback_records, 0);
    assert!(!report.probes.no_quarantine.quarantined);
    // The AND invariant agrees with the recorded top-level flag.
    assert_eq!(report.healthy, report.probes.all_healthy());
}

#[test]
fn memory_intact_baseline_is_optional() {
    // When `--pre-deploy-facts` is omitted the probe reports the live count only.
    let json = r#"{ "healthy": true, "live_facts": 900, "baseline_facts": null }"#;
    let probe: MemoryIntactProbe = serde_json::from_str(json).unwrap();
    assert_eq!(probe.baseline_facts, None);
    assert_eq!(probe.live_facts, 900);
}
