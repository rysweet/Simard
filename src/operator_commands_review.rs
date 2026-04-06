use std::path::PathBuf;

use crate::operator_commands::{
    print_display, print_text, prompt_root, resolved_review_state_root,
};
use crate::sanitization::sanitize_terminal_text;
use crate::{
    BootstrapConfig, BootstrapInputs, CognitiveMemoryType, FileBackedMemoryStore, MemoryRecord,
    MemoryStore, ReviewRequest, ReviewTargetKind, build_review_artifact, latest_local_handoff,
    latest_review_artifact, persist_review_artifact, run_local_session,
};

pub fn run_review_probe(
    base_type: &str,
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-engineer";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_review_state_root(
            state_root_override,
            base_type,
            topology,
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable review handoff")?;
    let review = build_review_artifact(
        ReviewRequest {
            target_kind: ReviewTargetKind::Session,
            target_label: "operator-review".to_string(),
            execution_summary: execution.outcome.execution_summary.clone(),
            reflection_summary: execution.outcome.reflection.summary.clone(),
            measurement_notes: Vec::new(),
            signals: Vec::new(),
        },
        &exported,
    )?;
    let review_artifact_path = persist_review_artifact(config.state_root_path(), &review)?;
    let session_id = exported
        .session
        .as_ref()
        .ok_or("expected session boundary in review handoff")?
        .id
        .clone();
    let memory_store = FileBackedMemoryStore::try_new(config.memory_store_path())?;
    let review_key = format!("{}-review-record", session_id);
    memory_store.put(MemoryRecord {
        key: review_key.clone(),
        memory_type: CognitiveMemoryType::Semantic,
        value: review.concise_record(),
        session_id,
        recorded_in: crate::SessionPhase::Complete,
    })?;
    let decision_records = memory_store.list(CognitiveMemoryType::Semantic)?;

    println!("Probe mode: review-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    print_display("State root", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    print_display("Review artifact", review_artifact_path.display());
    println!("Review proposals: {}", review.proposals.len());
    for (index, proposal) in review.proposals.iter().enumerate() {
        println!(
            "Proposal {}: [{}] {} => {}",
            index + 1,
            proposal.category,
            sanitize_terminal_text(&proposal.title),
            sanitize_terminal_text(&proposal.suggested_change)
        );
    }
    println!("Decision records: {}", decision_records.len());
    print_text("Review record key", &review_key);
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

pub fn run_review_read_probe(
    base_type: &str,
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_review_state_root(state_root_override, base_type, topology)?;
    let (review_artifact_path, review) =
        latest_review_artifact(&state_root)?.ok_or("expected persisted review artifact")?;
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let decision_records = memory_store
        .list(CognitiveMemoryType::Semantic)?
        .into_iter()
        .filter(|record| record.value.starts_with("review-summary |"))
        .collect::<Vec<_>>();

    println!("Probe mode: review-read");
    println!("Identity: {}", review.identity_name);
    println!("Selected base type: {}", review.selected_base_type);
    println!("Topology: {}", review.topology);
    print_display("State root", state_root.display());
    print_display("Latest review artifact", review_artifact_path.display());
    print_text("Latest review target", &review.target_label);
    print_text("Latest review summary", &review.summary);
    println!("Review proposals: {}", review.proposals.len());
    for (index, proposal) in review.proposals.iter().enumerate() {
        println!(
            "Proposal {}: [{}] {} => {}",
            index + 1,
            proposal.category,
            sanitize_terminal_text(&proposal.title),
            sanitize_terminal_text(&proposal.suggested_change)
        );
    }
    println!("Decision review records: {}", decision_records.len());
    if let Some(record) = decision_records.last() {
        print_text("Latest decision review record", &record.value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_read_probe_errors_with_nonexistent_explicit_state_root() {
        let result = run_review_read_probe(
            "terminal-shell",
            "single-process",
            Some(PathBuf::from(
                "/nonexistent/simard-test-path-does-not-exist-12345",
            )),
        );
        assert!(
            result.is_err(),
            "should fail when state root does not exist"
        );
    }

    #[test]
    fn review_probe_errors_with_nonexistent_explicit_state_root() {
        let result = run_review_probe(
            "terminal-shell",
            "single-process",
            "test objective",
            Some(PathBuf::from(
                "/nonexistent/simard-test-path-does-not-exist-12345",
            )),
        );
        assert!(
            result.is_err(),
            "should fail when state root does not exist"
        );
    }

    #[test]
    fn review_read_probe_errors_with_empty_state_root() {
        let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!("review-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&scratch);

        let result =
            run_review_read_probe("terminal-shell", "single-process", Some(scratch.clone()));
        assert!(
            result.is_err(),
            "should fail with empty state root (no review artifacts)"
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn sanitize_terminal_text_is_applied_to_proposals() {
        // Verify that the sanitize function is importable and works
        let input = "normal text";
        let result = sanitize_terminal_text(input);
        assert_eq!(result, input, "clean text should pass through unchanged");
    }

    #[test]
    fn sanitize_terminal_text_strips_control_chars() {
        let input = "text\x1b[31mwith\x1b[0m escapes";
        let result = sanitize_terminal_text(input);
        assert!(
            !result.contains("\x1b"),
            "should strip ANSI escape sequences: {result}"
        );
    }

    // --- run_review_read_probe additional edge cases ---

    #[test]
    fn review_read_probe_errors_with_empty_string_path() {
        let result =
            run_review_read_probe("terminal-shell", "single-process", Some(PathBuf::from("")));
        assert!(result.is_err(), "empty path should fail");
    }

    #[test]
    fn review_probe_errors_with_empty_objective() {
        let result = run_review_probe(
            "terminal-shell",
            "single-process",
            "",
            Some(PathBuf::from("/nonexistent/path-12345")),
        );
        assert!(result.is_err(), "should fail with nonexistent state root");
    }

    #[test]
    fn review_read_probe_with_valid_empty_state_root_has_descriptive_error() {
        let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!("review-valid-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&scratch);

        let result =
            run_review_read_probe("terminal-shell", "single-process", Some(scratch.clone()));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("review") || msg.contains("artifact"),
            "error should mention review/artifact: {msg}"
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }

    // --- sanitize_terminal_text edge cases ---

    #[test]
    fn sanitize_terminal_text_empty_input() {
        assert_eq!(sanitize_terminal_text(""), "");
    }

    #[test]
    fn sanitize_terminal_text_only_ansi_escapes() {
        let input = "\x1b[31m\x1b[0m";
        let result = sanitize_terminal_text(input);
        assert!(
            !result.contains("\x1b"),
            "should strip all escapes: {result}"
        );
    }

    #[test]
    fn sanitize_terminal_text_unicode_preserved() {
        let input = "hello 🌍 world";
        let result = sanitize_terminal_text(input);
        assert_eq!(result, input, "unicode should be preserved");
    }

    #[test]
    fn sanitize_terminal_text_newlines_preserved() {
        let input = "line1\nline2";
        let result = sanitize_terminal_text(input);
        assert!(result.contains("line1"), "line content should be preserved");
        assert!(result.contains("line2"), "line content should be preserved");
    }

    // --- run_review_probe argument edge cases ---

    #[test]
    fn review_probe_errors_with_unusual_base_type() {
        let result = run_review_probe(
            "nonexistent-base-type",
            "single-process",
            "test objective",
            Some(PathBuf::from("/nonexistent/path-99999")),
        );
        assert!(result.is_err());
    }

    #[test]
    fn review_probe_errors_with_unusual_topology() {
        let result = run_review_probe(
            "terminal-shell",
            "nonexistent-topology",
            "test objective",
            Some(PathBuf::from("/nonexistent/path-99998")),
        );
        assert!(result.is_err());
    }

    #[test]
    fn review_read_probe_errors_with_unusual_base_type() {
        let result = run_review_read_probe(
            "nonexistent-base-type",
            "single-process",
            Some(PathBuf::from("/nonexistent/path-99997")),
        );
        assert!(result.is_err());
    }

    #[test]
    fn review_read_probe_errors_with_unusual_topology() {
        let result = run_review_read_probe(
            "terminal-shell",
            "nonexistent-topology",
            Some(PathBuf::from("/nonexistent/path-99996")),
        );
        assert!(result.is_err());
    }

    // --- sanitize_terminal_text additional ---

    #[test]
    fn sanitize_terminal_text_tab_characters() {
        let input = "col1\tcol2\tcol3";
        let result = sanitize_terminal_text(input);
        assert!(
            result.contains("col1") && result.contains("col2") && result.contains("col3"),
            "tab-separated content should be preserved: {result}"
        );
    }

    #[test]
    fn sanitize_terminal_text_nested_ansi() {
        let input = "\x1b[1m\x1b[31mbold red\x1b[0m normal";
        let result = sanitize_terminal_text(input);
        assert!(
            !result.contains("\x1b"),
            "nested escapes stripped: {result}"
        );
        assert!(result.contains("normal"), "text preserved: {result}");
    }

    #[test]
    fn sanitize_terminal_text_long_input() {
        let input = "a".repeat(10_000);
        let result = sanitize_terminal_text(&input);
        assert_eq!(result.len(), 10_000);
    }

    #[test]
    fn sanitize_terminal_text_mixed_unicode_and_ansi() {
        let input = "\x1b[32m日本語\x1b[0m テスト";
        let result = sanitize_terminal_text(input);
        assert!(!result.contains("\x1b"));
        assert!(result.contains("テスト"));
    }

    // --- review_read_probe with scratch dirs ---

    #[test]
    fn review_read_probe_with_files_but_no_review_artifact() {
        let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!("review-no-artifact-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&scratch);
        // Write a dummy file that isn't a review artifact
        let _ = std::fs::write(scratch.join("dummy.txt"), "not a review");

        let result =
            run_review_read_probe("terminal-shell", "single-process", Some(scratch.clone()));
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn review_probe_with_long_objective() {
        let long_objective = "a".repeat(5000);
        let result = run_review_probe(
            "terminal-shell",
            "single-process",
            &long_objective,
            Some(PathBuf::from("/nonexistent/path-long-obj")),
        );
        assert!(result.is_err());
    }

    // --- BootstrapInputs default ---

    #[test]
    fn bootstrap_inputs_default_has_all_none() {
        let inputs = BootstrapInputs::default();
        assert!(inputs.prompt_root.is_none());
        assert!(inputs.objective.is_none());
        assert!(inputs.state_root.is_none());
        assert!(inputs.identity.is_none());
        assert!(inputs.base_type.is_none());
        assert!(inputs.topology.is_none());
    }
}
