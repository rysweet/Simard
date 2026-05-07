use super::types::*;

#[test]
fn phase_outcome_variants() {
    let success = PhaseOutcome::Success;
    let failed = PhaseOutcome::Failed("reason".into());
    let skipped = PhaseOutcome::Skipped("why".into());
    assert_eq!(success, PhaseOutcome::Success);
    assert_ne!(success, failed);
    assert_ne!(failed, skipped);
}
