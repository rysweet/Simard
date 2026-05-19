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
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved =
        resolved_improvement_curation_read_state_root(state_root_override, base_type, topology)?;
    let state_root = resolved.path;

    // When the operator passes an explicit state root, the strict
    // mode-specific validation in `resolved_improvement_curation_read_state_root`
    // already enforced that `review-artifacts/` and `memory_records.json`
    // exist — so `latest_review_artifact` and the decision-record
    // lookup must succeed or hard-fail (preserves the prior contract).
    //
    // When we fell back to the canonical daemon store (no explicit
    // override), missing artifacts must render as `<none>` / `0` so
    // `simard improvement-curation read <base> <topology>` works on a
    // fresh machine (issue #1909).
    let review_lookup = latest_review_artifact(&state_root)?;
    let (review_artifact_path, review) = match review_lookup {
        Some(pair) => pair,
        None if resolved.used_override => {
            return Err("expected persisted review artifact".into());
        }
        None => {
            return print_improvement_curation_empty_fallback(base_type, topology, &state_root);
        }
    };

    let memory_records_path = state_root.join("memory_records.json");
    let parsed_record = if memory_records_path.is_file() {
        let memory_store = FileBackedMemoryStore::try_new(&memory_records_path)?;
        let latest = memory_store
            .list(MemoryScope::Decision)?
            .into_iter()
            .rfind(|record| record.key.ends_with("improvement-curation-record"));
        match latest {
            Some(record) => Some(
                PersistedImprovementRecord::parse(&record.value)
                    .map_err(|error| format!("{error}"))?,
            ),
            None if resolved.used_override => {
                return Err("expected persisted improvement decision record".into());
            }
            None => None,
        }
    } else if resolved.used_override {
        return Err("expected persisted improvement decision record".into());
    } else {
        None
    };

    let goal_records = {
        // Issue #1590 follow-up: read goals through the
        // `CognitiveMemoryGoalStore` so the read probe surfaces the
        // same records the curator persisted via the `GoalStore`
        // trait (the previous `load_goal_board` path read a different
        // fact concept and never saw curator writes).
        use crate::goals::GoalStore as _;
        let store = crate::goals::CognitiveMemoryGoalStore::new(state_root.clone())?;
        store.list()?
    };

    println!("Probe mode: improvement-curation-read");
    println!("Identity: simard-improvement-curator");
    let displayed_base_type = parsed_record
        .as_ref()
        .and_then(|record| record.selected_base_type.as_deref())
        .unwrap_or(&review.selected_base_type);
    print_text("Selected base type", displayed_base_type);
    let displayed_topology = parsed_record
        .as_ref()
        .and_then(|record| record.topology.as_deref())
        .unwrap_or(&review.topology);
    print_text("Topology", displayed_topology);
    print_display("State root", state_root.display());
    print_display("Latest review artifact", review_artifact_path.display());
    print_text("Review id", &review.review_id);
    print_text("Review target", &review.target_label);
    println!("Review proposals: {}", review.proposals.len());

    match parsed_record.as_ref() {
        Some(record) => {
            println!("Approved proposals: {}", record.approved_proposals.len());
            if record.approved_proposals.is_empty() {
                println!("Approved proposals: <none>");
            } else {
                for (index, approval) in record.approved_proposals.iter().enumerate() {
                    print_text(
                        &format!("Approved proposal {}", index + 1),
                        approval.concise_label(),
                    );
                }
            }
            println!("Deferred proposals: {}", record.deferred_proposals.len());
            if record.deferred_proposals.is_empty() {
                println!("Deferred proposals: <none>");
            } else {
                for (index, deferral) in record.deferred_proposals.iter().enumerate() {
                    print_text(
                        &format!("Deferred proposal {}", index + 1),
                        format!("{} ({})", deferral.title, deferral.rationale),
                    );
                }
            }
        }
        None => {
            // Daemon-fallback path with a review artifact present but no
            // persisted improvement-curation decision record. Render
            // explicit empty sections (Pillar 11 — issue #1909).
            println!("Approved proposals: 0");
            println!("Approved proposals: <none>");
            println!("Deferred proposals: 0");
            println!("Deferred proposals: <none>");
        }
    }
    print_goal_section(&goal_records, GoalStatus::Active, "Active");
    print_goal_section(&goal_records, GoalStatus::Proposed, "Proposed");
    match parsed_record.as_ref() {
        Some(record) => print_text("Latest improvement record", record.concise_record()),
        None => print_text("Latest improvement record", "<none>"),
    }
    Ok(())
}

/// Render the empty-state output for the daemon-fallback path when no
/// review artifact exists yet. Mirrors the shape of the populated path
/// so operators see "0 / <none>" instead of a hard error (issue #1909).
fn print_improvement_curation_empty_fallback(
    base_type: &str,
    topology: &str,
    state_root: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let goal_records = {
        use crate::goals::GoalStore as _;
        let store = crate::goals::CognitiveMemoryGoalStore::new(state_root.to_path_buf())?;
        store.list()?
    };

    println!("Probe mode: improvement-curation-read");
    println!("Identity: simard-improvement-curator");
    print_text("Selected base type", base_type);
    print_text("Topology", topology);
    print_display("State root", state_root.display());
    print_text("Latest review artifact", "<none>");
    print_text("Review id", "<none>");
    print_text("Review target", "<none>");
    println!("Review proposals: 0");
    println!("Approved proposals: 0");
    println!("Approved proposals: <none>");
    println!("Deferred proposals: 0");
    println!("Deferred proposals: <none>");
    print_goal_section(&goal_records, GoalStatus::Active, "Active");
    print_goal_section(&goal_records, GoalStatus::Proposed, "Proposed");
    print_text("Latest improvement record", "<none>");
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

    // ───────────────────────────────────────────────────────────────────
    // Issue #1909: `improvement-curation read` without an explicit
    // `[state-root]` argument must fall back to the canonical daemon
    // state root (`$SIMARD_STATE_ROOT` or `$HOME/.simard/state`) and
    // render an explicit empty report when no persisted review artifact
    // exists yet — instead of hard-failing on a fresh machine.
    // ───────────────────────────────────────────────────────────────────

    fn with_simard_state_root<R>(path: &std::path::Path, run: impl FnOnce() -> R) -> R {
        let prev = std::env::var("SIMARD_STATE_ROOT").ok();
        // SAFETY: gated on `serial_test::serial(simard_state_root)`.
        unsafe {
            std::env::set_var("SIMARD_STATE_ROOT", path);
        }
        let result = run();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("SIMARD_STATE_ROOT", v),
                None => std::env::remove_var("SIMARD_STATE_ROOT"),
            }
        }
        result
    }

    #[test]
    #[serial_test::serial(simard_state_root)]
    fn improvement_curation_read_probe_falls_back_to_default_state_root_when_no_override() {
        let dir = TempDir::new().unwrap();
        // Empty state root — no review-artifacts/, no memory_records.json.
        // Pre-fix this hard-failed with `expected persisted review artifact`.
        // Post-fix it falls back and renders empty.
        let result = with_simard_state_root(dir.path(), || {
            run_improvement_curation_read_probe("local-harness", "single-process", None)
        });
        assert!(
            result.is_ok(),
            "improvement-curation read should succeed via daemon fallback when \
             no override is given and the daemon store is empty: {:?}",
            result.err()
        );
    }

    #[test]
    #[serial_test::serial(simard_state_root)]
    fn improvement_curation_read_probe_fallback_rejects_bogus_topology() {
        let dir = TempDir::new().unwrap();
        let result = with_simard_state_root(dir.path(), || {
            run_improvement_curation_read_probe("local-harness", "totally-bogus-topology", None)
        });
        assert!(
            result.is_err(),
            "bogus topology should fail validation even on the daemon-fallback path"
        );
    }
}
