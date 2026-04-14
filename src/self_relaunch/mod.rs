//! Canary deployment and handover for self-relaunch.
//!
//! Gate sequence: Smoke -> UnitTest -> GymBaseline -> BridgeHealth.
//! All gates must pass before handover. Failures reject the canary (Pillar 11).
//!
//! For coordinated multi-process handoff with leader election, see
//! [`coordinated_relaunch`] which uses [`self_relaunch_semaphore`].

mod canary;
mod gates;
mod types;

// Re-export all public items so `crate::self_relaunch::X` still works.
pub use canary::{build_canary, coordinated_relaunch, handover};
pub use gates::{all_gates_passed, verify_canary};
pub use types::{GateResult, RelaunchConfig, RelaunchGate, default_gates};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn default_gates_returns_four_in_order() {
        let gates = default_gates();
        assert_eq!(gates.len(), 4);
        assert_eq!(gates[0], RelaunchGate::Smoke);
        assert_eq!(gates[1], RelaunchGate::UnitTest);
        assert_eq!(gates[2], RelaunchGate::GymBaseline);
        assert_eq!(gates[3], RelaunchGate::BridgeHealth);
    }

    #[test]
    fn all_gates_passed_returns_true_for_all_passing() {
        let results = vec![
            GateResult {
                gate: RelaunchGate::Smoke,
                passed: true,
                detail: "ok".into(),
            },
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: true,
                detail: "ok".into(),
            },
        ];
        assert!(all_gates_passed(&results));
    }

    #[test]
    fn all_gates_passed_returns_false_when_any_fails() {
        let results = vec![
            GateResult {
                gate: RelaunchGate::Smoke,
                passed: true,
                detail: "ok".into(),
            },
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: "fail".into(),
            },
        ];
        assert!(!all_gates_passed(&results));
    }

    #[test]
    fn verify_canary_with_nonexistent_binary_fails_smoke() {
        let config = RelaunchConfig::default();
        let results = verify_canary(
            Path::new("/no-such-binary-simard-test"),
            &[RelaunchGate::Smoke],
            &config,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
    }

    #[test]
    fn relaunch_config_default_values() {
        let config = RelaunchConfig::default();
        assert_eq!(config.health_timeout, std::time::Duration::from_secs(30));
    }

    #[test]
    fn handover_rejects_zero_pid() {
        let err = handover(0, Path::new("/usr/bin/true")).unwrap_err();
        assert!(err.to_string().contains("current_pid"));
    }
}
