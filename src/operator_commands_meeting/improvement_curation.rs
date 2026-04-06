use std::path::PathBuf;

use crate::goals::{FileBackedGoalStore, GoalStatus, GoalStore};
use crate::improvements::PersistedImprovementRecord;
use crate::operator_commands::{
    print_display, print_goal_section, print_text, prompt_root,
    resolved_improvement_curation_read_state_root,
};
use crate::sanitization::sanitize_terminal_text;
use crate::{
    BootstrapConfig, BootstrapInputs, CognitiveMemoryType, FileBackedMemoryStore, MemoryStore,
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
        .list(CognitiveMemoryType::Semantic)?
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
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root =
        resolved_improvement_curation_read_state_root(state_root_override, base_type, topology)?;
    let (review_artifact_path, review) =
        latest_review_artifact(&state_root)?.ok_or("expected persisted review artifact")?;
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let latest_record = memory_store
        .list(CognitiveMemoryType::Semantic)?
        .into_iter()
        .rfind(|record| record.key.ends_with("improvement-curation-record"))
        .ok_or("expected persisted improvement decision record")?;
    let parsed_record = PersistedImprovementRecord::parse(&latest_record.value)
        .map_err(|error| format!("{error}"))?;
    let goal_store = FileBackedGoalStore::try_new(state_root.join("goal_records.json"))?;
    let goal_records = goal_store.list()?;

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
}
