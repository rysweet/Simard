//! G3 (agentic-over-brittle) contract for the self-diagnose-on-step-error
//! prompt asset (issue #2640, PART 2).
//!
//! When a decision-cycle / engineer / terminal-shell step fails, Simard must
//! not merely LOG the error and move on — she must INSPECT and DIAGNOSE *why*
//! it happened, then drive a corrective action. Per guideline G3 the WHY/remedy
//! reasoning lives in a prompt asset (agentic), with the Rust classifier acting
//! only as a thin structured trigger.
//!
//! This pins the existence and intent of that prompt asset. TDD status: RED
//! until `prompt_assets/simard/overseer/self_diagnose.md` is authored.

use std::path::PathBuf;

fn self_diagnose_asset_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets/simard/overseer/self_diagnose.md")
}

#[test]
fn self_diagnose_prompt_asset_exists() {
    let path = self_diagnose_asset_path();
    assert!(
        path.is_file(),
        "the self-diagnosis prompt asset must exist at {} — the operator ask is \
         that Simard diagnoses WHY a step failed (agentic, G3), not just logs it",
        path.display()
    );
}

#[test]
fn self_diagnose_prompt_asset_drives_why_and_remedy() {
    let path = self_diagnose_asset_path();
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let lower = body.to_ascii_lowercase();

    // It must center on WHY (root cause), not just restating the error.
    assert!(
        lower.contains("why"),
        "self_diagnose.md must ask WHY the failure occurred (root cause), \
         mirroring the operator principle 'always ask WHY, not just log it'"
    );
    // It must classify the failure into a cause.
    assert!(
        lower.contains("cause") || lower.contains("classif"),
        "self_diagnose.md must classify the failure's cause"
    );
    // It must produce a corrective remedy / action, not a log line.
    assert!(
        lower.contains("remed") || lower.contains("corrective") || lower.contains("fix"),
        "self_diagnose.md must output a corrective remedy / action"
    );
    // It must consume the error + last terminal output as inputs.
    assert!(
        (lower.contains("error") || lower.contains("exit"))
            && (lower.contains("terminal")
                || lower.contains("output")
                || lower.contains("transcript")),
        "self_diagnose.md must be fed the error + last terminal output to reason over"
    );
    // The headline live defect (E2BIG / arg-list-too-long) should appear as an
    // illustrative cause so the reasoning is grounded in the real incident.
    assert!(
        lower.contains("arg")
            && (lower.contains("too long")
                || lower.contains("e2big")
                || lower.contains("arg_max")
                || lower.contains("arg-list")),
        "self_diagnose.md should name the arg-list-too-long / E2BIG failure mode \
         (the live #2640 defect) as an example cause"
    );
}
