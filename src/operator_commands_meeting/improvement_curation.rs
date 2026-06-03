use std::path::PathBuf;

use crate::goals::GoalStatus;
use crate::improvements::PersistedImprovementRecord;
use crate::operator_commands::{
    print_display, print_goal_section, print_text, prompt_root,
    resolved_improvement_curation_read_state_root,
};
use crate::sanitization::sanitize_terminal_text;
use crate::{
    BootstrapConfig, BootstrapInputs, FileBackedMemoryStore, MemoryScope, MemoryStore,
    latest_review_artifact, render_review_context_directives, run_local_session,
};

pub fn run_improvement_curation_probe(
    base_type: &str,
    topology: &str,
    operator_objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = crate::operator_commands::resolved_review_state_root(
        state_root_override,
        base_type,
        topology,
    )?;
    let (review_artifact_path, review) =
        latest_review_artifact(&state_root)?.ok_or("expected persisted review artifact")?;
    let objective = format!(
        "{}\n{}",
        render_review_context_directives(&review),
        operator_objective
    );
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.clone()),
        state_root: Some(state_root.clone()),
        identity: Some("simard-improvement-curator".to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
    let plan = crate::ImprovementPromotionPlan::parse(&objective)?;
    let memory_store = FileBackedMemoryStore::try_new(config.memory_store_path())?;
    let improvement_records = memory_store
        .list(MemoryScope::Decision)?
        .into_iter()
        .filter(|record| record.key.ends_with("improvement-curation-record"))
        .collect::<Vec<_>>();

    println!("Probe mode: improvement-curation-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    print_display("State root", config.state_root_path().display());
    print_display("Review artifact", review_artifact_path.display());
    print_text("Review id", &review.review_id);
    print_text("Review target", &review.target_label);
    println!("Review proposals: {}", review.proposals.len());
    println!("Approved proposals: {}", plan.approvals.len());
    for (index, approval) in plan.approvals.iter().enumerate() {
        println!(
            "Approved proposal {}: p{} [{}] {}",
            index + 1,
            approval.priority,
            approval.status,
            sanitize_terminal_text(&approval.title)
        );
    }
    println!("Deferred proposals: {}", plan.deferrals.len());
    for (index, deferral) in plan.deferrals.iter().enumerate() {
        println!(
            "Deferred proposal {}: {} ({})",
            index + 1,
            sanitize_terminal_text(&deferral.title),
            sanitize_terminal_text(&deferral.rationale)
        );
    }
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal);
    }
    println!(
        "Proposed goals count: {}",
        execution.snapshot.proposed_goal_count
    );
    for (index, goal) in execution.snapshot.proposed_goals.iter().enumerate() {
        print_text(&format!("Proposed goal {}", index + 1), goal);
    }
    println!("Decision records: {}", improvement_records.len());
    if let Some(record) = improvement_records.last() {
        print_text("Latest improvement record", &record.value);
    }
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

pub fn run_improvement_curation_read_probe(
    base_type: &str,
    _topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_improvement_curation_read_state_root(state_root_override, base_type)?;
    let (review_artifact_path, review) =
        latest_review_artifact(&state_root)?.ok_or("expected persisted review artifact")?;
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let latest_record = memory_store
        .list(MemoryScope::Decision)?
        .into_iter()
        .rfind(|record| record.key.ends_with("improvement-curation-record"))
        .ok_or("expected persisted improvement decision record")?;
    let parsed_record = PersistedImprovementRecord::parse(&latest_record.value)
        .map_err(|error| format!("{error}"))?;
    let goal_records = {
        // Read goals through `FileBackedGoalStore` to match the write
        // path in `assembly.rs` which persists via the same store to
        // `state/goal_store.json`.
        use crate::goals::GoalStore as _;
        let store = crate::goals::FileBackedGoalStore::try_new(
            state_root.join("state").join("goal_store.json"),
        )?;
        store.list()?
    };

    println!("Probe mode: improvement-curation-read");
    println!("Identity: simard-improvement-curator");
    print_text(
        "Selected base type",
        parsed_record
            .selected_base_type
            .as_deref()
            .unwrap_or(&review.selected_base_type),
    );
    print_text(
        "Topology",
        parsed_record
            .topology
            .as_deref()
            .unwrap_or(&review.topology),
    );
    print_display("State root", state_root.display());
    print_display("Latest review artifact", review_artifact_path.display());
    print_text("Review id", &review.review_id);
    print_text("Review target", &review.target_label);
    println!("Review proposals: {}", review.proposals.len());
    println!(
        "Approved proposals: {}",
        parsed_record.approved_proposals.len()
    );
    if parsed_record.approved_proposals.is_empty() {
        println!("Approved proposals: <none>");
    } else {
        for (index, approval) in parsed_record.approved_proposals.iter().enumerate() {
            print_text(
                &format!("Approved proposal {}", index + 1),
                approval.concise_label(),
            );
        }
    }
    println!(
        "Deferred proposals: {}",
        parsed_record.deferred_proposals.len()
    );
    if parsed_record.deferred_proposals.is_empty() {
        println!("Deferred proposals: <none>");
    } else {
        for (index, deferral) in parsed_record.deferred_proposals.iter().enumerate() {
            print_text(
                &format!("Deferred proposal {}", index + 1),
                format!("{} ({})", deferral.title, deferral.rationale),
            );
        }
    }
    print_goal_section(&goal_records, GoalStatus::Active, "Active");
    print_goal_section(&goal_records, GoalStatus::Proposed, "Proposed");
    print_text("Latest improvement record", parsed_record.concise_record());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn improvement_curation_read_probe_rejects_incomplete_state() {
        let dir = TempDir::new().unwrap();
        let result = run_improvement_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_err(),
            "expected error when review artifacts are missing"
        );
    }

    #[test]
    fn improvement_curation_read_probe_rejects_empty_dir() {
        let dir = TempDir::new().unwrap();
        let result = run_improvement_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_err(),
            "expected error when state root has no review artifacts"
        );
    }

    #[test]
    fn improvement_curation_read_probe_rejects_nonexistent_directory() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nonexistent-state-root");
        let result =
            run_improvement_curation_read_probe("local-harness", "single-process", Some(missing));
        assert!(result.is_err());
    }

    #[test]
    fn improvement_curation_read_probe_with_dir_but_no_review_artifacts() {
        let dir = TempDir::new().unwrap();
        // Has a memory file but no review-artifacts directory
        std::fs::write(dir.path().join("memory_records.json"), "[]").unwrap();
        let result = run_improvement_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Issue #2182: read probe must read goals from FileBackedGoalStore
    //
    // These tests verify the contract that the read probe reads goals from
    // `state/goal_store.json` (via FileBackedGoalStore), matching the
    // write path used by assembly.rs.  Before the fix, the read probe
    // used CognitiveMemoryGoalStore which reads from cognitive memory
    // bridges — empty in tests — causing "Active goals count: 0".
    // -----------------------------------------------------------------------

    /// Build a minimal but valid fixture at `state_root` containing:
    /// - `review-artifacts/<review_id>.json` with a valid ReviewArtifact
    /// - `memory_records.json` with a Decision-scoped improvement-curation-record
    /// - `state/goal_store.json` with the given goal records (if any)
    fn build_read_probe_fixture(state_root: &std::path::Path, goals: &[crate::goals::GoalRecord]) {
        // 1. Review artifact
        let artifacts_dir = state_root.join("review-artifacts");
        std::fs::create_dir_all(&artifacts_dir).unwrap();
        let review_json = serde_json::json!({
            "review_id": "fixture-review-001",
            "reviewed_at_unix_ms": 1700000000000_u128,
            "target_kind": "session",
            "target_label": "fixture-target",
            "identity_name": "simard-reviewer",
            "session_id": "session-00000000-0000-0000-0000-000000000001",
            "selected_base_type": "local-harness",
            "topology": "single-process",
            "objective_metadata": "test objective",
            "execution_summary": "test execution summary",
            "reflection_summary": "test reflection summary",
            "summary": "test summary",
            "measurement_notes": [],
            "evidence_summary": {
                "memory_records": 1,
                "evidence_records": 0,
                "decision_records": 1,
                "benchmark_records": 0,
                "exported_state": "none",
                "session_phase": null,
                "failed_signals": []
            },
            "proposals": [
                {
                    "category": "execution-evidence",
                    "title": "Capture denser execution evidence",
                    "rationale": "operators need denser evidence now",
                    "suggested_change": "add more logging",
                    "evidence": ["session output was sparse"]
                }
            ]
        });
        std::fs::write(
            artifacts_dir.join("fixture-review-001.json"),
            serde_json::to_string_pretty(&review_json).unwrap(),
        )
        .unwrap();

        // 2. Memory records with improvement-curation-record
        let memory_json = serde_json::json!([
            {
                "key": "fixture-session::improvement-curation-record",
                "scope": "decision",
                "value": "review=fixture-review-001 target=fixture-target approvals=[p1 [active] Capture denser execution evidence] deferred=[]",
                "session_id": "session-00000000-0000-0000-0000-000000000002",
                "recorded_in": "persistence"
            }
        ]);
        std::fs::write(
            state_root.join("memory_records.json"),
            serde_json::to_string_pretty(&memory_json).unwrap(),
        )
        .unwrap();

        // 3. Goal store at state/goal_store.json
        if !goals.is_empty() {
            let goal_dir = state_root.join("state");
            std::fs::create_dir_all(&goal_dir).unwrap();
            std::fs::write(
                goal_dir.join("goal_store.json"),
                serde_json::to_string(goals).unwrap(),
            )
            .unwrap();
        }
    }

    fn make_test_goal_record(
        title: &str,
        status: crate::goals::GoalStatus,
        priority: u8,
    ) -> crate::goals::GoalRecord {
        let update = crate::goals::GoalUpdate::new(title, "fixture rationale", status, priority)
            .expect("goal update should be valid");
        crate::goals::GoalRecord::from_update(
            update,
            "simard-improvement-curator",
            crate::session::SessionId::parse("session-00000000-0000-0000-0000-000000000003")
                .expect("session id should parse"),
            crate::session::SessionPhase::Persistence,
        )
        .expect("goal record should be valid")
    }

    #[test]
    fn read_probe_succeeds_with_goals_at_file_backed_store_path() {
        let dir = TempDir::new().unwrap();
        let goals = vec![make_test_goal_record(
            "Capture denser execution evidence",
            crate::goals::GoalStatus::Active,
            1,
        )];
        build_read_probe_fixture(dir.path(), &goals);

        let result = run_improvement_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_ok(),
            "read probe must succeed when goals exist at state/goal_store.json: {:?}",
            result.err()
        );
    }

    #[test]
    fn read_probe_succeeds_with_empty_goal_store() {
        let dir = TempDir::new().unwrap();
        build_read_probe_fixture(dir.path(), &[]);

        let result = run_improvement_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_ok(),
            "read probe must succeed even when no goals are persisted: {:?}",
            result.err()
        );
    }

    #[test]
    fn read_probe_succeeds_with_mixed_goal_statuses() {
        let dir = TempDir::new().unwrap();
        let goals = vec![
            make_test_goal_record("Active goal", crate::goals::GoalStatus::Active, 1),
            make_test_goal_record("Proposed goal", crate::goals::GoalStatus::Proposed, 2),
        ];
        build_read_probe_fixture(dir.path(), &goals);

        let result = run_improvement_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_ok(),
            "read probe must handle mixed goal statuses: {:?}",
            result.err()
        );
    }

    #[test]
    fn read_probe_goal_path_matches_bootstrap_config_path() {
        // The read probe constructs `state_root.join("state").join("goal_store.json")`.
        // This must match BootstrapConfig::goal_store_path() for the same state root.
        // We use SIMARD_BOOTSTRAP_MODE=builtin-defaults to avoid needing SIMARD_PROMPT_ROOT.
        let state_root = PathBuf::from("/tmp/test-state-root");
        let read_probe_path = state_root.join("state").join("goal_store.json");

        let config = crate::BootstrapConfig::resolve(crate::BootstrapInputs {
            state_root: Some(state_root.clone()),
            mode: Some("builtin-defaults".to_string()),
            ..crate::BootstrapInputs::default()
        })
        .expect("config should resolve with state_root override");

        assert_eq!(
            read_probe_path,
            config.goal_store_path(),
            "read probe path must match BootstrapConfig::goal_store_path() \
             so the read probe reads from the same store that assembly.rs writes to"
        );
    }
}
