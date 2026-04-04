use std::path::{Path, PathBuf};

use crate::meetings::PersistedMeetingRecord;
use crate::operator_commands::{
    parse_runtime_topology, print_display, print_terminal_bridge_section, print_text,
    render_redacted_objective_metadata, resolved_engineer_read_state_root, resolved_state_root,
    validated_engineer_read_artifacts,
};
use crate::terminal_engineer_bridge::{
    ENGINEER_HANDOFF_FILE_NAME, ENGINEER_MODE_BOUNDARY, SHARED_DEFAULT_STATE_ROOT_SOURCE,
    SHARED_EXPLICIT_STATE_ROOT_SOURCE, TerminalBridgeContext, load_runtime_handoff_snapshot,
};
use crate::{
    EvidenceRecord, FileBackedEvidenceStore, FileBackedMemoryStore, run_local_engineer_loop,
};

pub fn run_engineer_loop_probe(
    topology: &str,
    workspace_root: &Path,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_topology = parse_runtime_topology(topology)?;
    let state_root_was_explicit = state_root_override.is_some();
    let state_root = resolved_state_root(
        state_root_override,
        "simard-engineer",
        "terminal-shell",
        topology,
        "engineer-loop-run",
    )?;
    let run = run_local_engineer_loop(workspace_root, objective, runtime_topology, &state_root)
        .map_err(|error| format!("{error}"))?;

    println!("Probe mode: engineer-loop-run");
    print_text("Mode boundary", ENGINEER_MODE_BOUNDARY);
    print_display("Repo root", run.inspection.repo_root.display());
    print_text("Repo branch", &run.inspection.branch);
    print_text("Repo head", &run.inspection.head);
    println!("Worktree dirty: {}", run.inspection.worktree_dirty);
    println!(
        "Changed files: {}",
        if run.inspection.changed_files.is_empty() {
            "<none>".to_string()
        } else {
            run.inspection.changed_files.join(", ")
        }
    );
    println!("Active goals count: {}", run.inspection.active_goals.len());
    for (index, goal) in run.inspection.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal.concise_label());
    }
    println!(
        "Carried meeting decisions: {}",
        run.inspection.carried_meeting_decisions.len()
    );
    for (index, decision) in run.inspection.carried_meeting_decisions.iter().enumerate() {
        print_text(&format!("Carried meeting decision {}", index + 1), decision);
    }
    print_terminal_bridge_section(
        run.terminal_bridge_context.as_ref(),
        if state_root_was_explicit {
            SHARED_EXPLICIT_STATE_ROOT_SOURCE
        } else {
            SHARED_DEFAULT_STATE_ROOT_SOURCE
        },
    );
    print_text("Gap summary", &run.inspection.architecture_gap_summary);
    print_text("Execution scope", &run.execution_scope);
    print_text("Selected action", &run.action.selected.label);
    print_text("Action plan", &run.action.selected.plan_summary);
    print_text(
        "Verification steps",
        run.action.selected.verification_steps.join(" || "),
    );
    print_text("Action rationale", &run.action.selected.rationale);
    print_text("Action command", run.action.selected.argv.join(" "));
    println!("Action status: success");
    println!(
        "Changed files after action: {}",
        if run.action.changed_files.is_empty() {
            "<none>".to_string()
        } else {
            run.action.changed_files.join(", ")
        }
    );
    println!("Verification status: {}", run.verification.status);
    print_text("Verification summary", &run.verification.summary);
    println!("Elapsed duration: {:?}", run.elapsed_duration);
    println!("Phase traces: {}", run.phase_traces.len());
    for trace in &run.phase_traces {
        println!(
            "  Phase: {} | duration={:?} | outcome={:?}",
            trace.name, trace.duration, trace.outcome
        );
    }
    print_display("State root", run.state_root.display());
    Ok(())
}

pub fn run_engineer_read_probe(
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_engineer_read_state_root(state_root_override, topology)?;
    let view = EngineerReadView::load(state_root)?;
    view.print();
    Ok(())
}

struct EngineerReadView {
    state_root: PathBuf,
    handoff_source: String,
    identity: String,
    selected_base_type: String,
    topology: String,
    session_phase: String,
    objective_metadata: String,
    repo_root: PathBuf,
    repo_branch: String,
    repo_head: String,
    worktree_dirty: String,
    changed_files: String,
    active_goals: Vec<String>,
    carried_meeting_decisions: Vec<String>,
    selected_action: String,
    action_plan: String,
    verification_steps: String,
    action_status: String,
    changed_files_after_action: String,
    verification_status: String,
    verification_summary: String,
    terminal_bridge_context: Option<TerminalBridgeContext>,
    memory_record_count: usize,
    evidence_record_count: usize,
}

impl EngineerReadView {
    fn load(state_root: PathBuf) -> crate::SimardResult<Self> {
        let artifacts = validated_engineer_read_artifacts(&state_root)?;
        let handoff_source = artifacts.handoff_file_name.clone();
        let handoff = load_runtime_handoff_snapshot(
            &crate::terminal_engineer_bridge::SelectedHandoffArtifact {
                path: artifacts.handoff_path.clone(),
                file_name: match handoff_source.as_str() {
                    ENGINEER_HANDOFF_FILE_NAME => ENGINEER_HANDOFF_FILE_NAME,
                    _ => crate::terminal_engineer_bridge::COMPATIBILITY_HANDOFF_FILE_NAME,
                },
            },
            "engineer read",
        )?;
        let session =
            handoff
                .session
                .as_ref()
                .ok_or_else(|| crate::SimardError::InvalidHandoffSnapshot {
                    field: "session".to_string(),
                    reason: format!(
                        "engineer read requires {} to contain a persisted session snapshot",
                        artifacts.handoff_file_name
                    ),
                })?;

        FileBackedMemoryStore::try_new(artifacts.memory_path)?;
        FileBackedEvidenceStore::try_new(artifacts.evidence_path)?;

        Ok(Self {
            state_root,
            handoff_source: handoff_source.clone(),
            identity: handoff.identity_name,
            selected_base_type: handoff.selected_base_type.to_string(),
            topology: handoff.topology.to_string(),
            session_phase: session.phase.to_string(),
            objective_metadata: render_redacted_objective_metadata(&session.objective)?,
            repo_root: PathBuf::from(required_engineer_evidence_value(
                &handoff.evidence_records,
                "repo-root=",
                &handoff_source,
            )?),
            repo_branch: required_engineer_evidence_value(
                &handoff.evidence_records,
                "repo-branch=",
                &handoff_source,
            )?
            .to_string(),
            repo_head: required_engineer_evidence_value(
                &handoff.evidence_records,
                "repo-head=",
                &handoff_source,
            )?
            .to_string(),
            worktree_dirty: required_engineer_evidence_value(
                &handoff.evidence_records,
                "worktree-dirty=",
                &handoff_source,
            )?
            .to_string(),
            changed_files: required_engineer_evidence_value(
                &handoff.evidence_records,
                "changed-files=",
                &handoff_source,
            )?
            .to_string(),
            active_goals: parse_engineer_summary_list(
                required_engineer_evidence_value(
                    &handoff.evidence_records,
                    "active-goals=",
                    &handoff_source,
                )?,
                ", ",
            ),
            carried_meeting_decisions: parse_carried_meeting_decisions(
                required_engineer_evidence_value(
                    &handoff.evidence_records,
                    "carried-meeting-decisions=",
                    &handoff_source,
                )?,
            )?,
            selected_action: required_engineer_evidence_value(
                &handoff.evidence_records,
                "selected-action=",
                &handoff_source,
            )?
            .to_string(),
            action_plan: required_engineer_evidence_value(
                &handoff.evidence_records,
                "action-plan=",
                &handoff_source,
            )?
            .to_string(),
            verification_steps: required_engineer_evidence_value(
                &handoff.evidence_records,
                "action-verification-steps=",
                &handoff_source,
            )?
            .to_string(),
            action_status: required_engineer_evidence_value(
                &handoff.evidence_records,
                "action-status=",
                &handoff_source,
            )?
            .to_string(),
            changed_files_after_action: required_engineer_evidence_value(
                &handoff.evidence_records,
                "changed-files-after-action=",
                &handoff_source,
            )?
            .to_string(),
            verification_status: required_engineer_evidence_value(
                &handoff.evidence_records,
                "verification-status=",
                &handoff_source,
            )?
            .to_string(),
            verification_summary: required_engineer_evidence_value(
                &handoff.evidence_records,
                "verification-summary=",
                &handoff_source,
            )?
            .to_string(),
            terminal_bridge_context: TerminalBridgeContext::from_engineer_evidence(
                &handoff.evidence_records,
            )?,
            memory_record_count: handoff.memory_records.len(),
            evidence_record_count: handoff.evidence_records.len(),
        })
    }

    fn print(&self) {
        println!("Probe mode: engineer-read");
        print_text("Engineer handoff source", &self.handoff_source);
        print_text("Mode boundary", ENGINEER_MODE_BOUNDARY);
        print_text("Identity", &self.identity);
        print_text("Selected base type", &self.selected_base_type);
        print_text("Topology", &self.topology);
        print_display("State root", self.state_root.display());
        print_text("Session phase", &self.session_phase);
        print_text("Objective metadata", &self.objective_metadata);
        print_display("Repo root", self.repo_root.display());
        print_text("Repo branch", &self.repo_branch);
        print_text("Repo head", &self.repo_head);
        print_text("Worktree dirty", &self.worktree_dirty);
        print_text("Changed files", &self.changed_files);
        println!("Active goals count: {}", self.active_goals.len());
        for (index, goal) in self.active_goals.iter().enumerate() {
            print_text(&format!("Active goal {}", index + 1), goal);
        }
        println!(
            "Carried meeting decisions: {}",
            self.carried_meeting_decisions.len()
        );
        for (index, decision) in self.carried_meeting_decisions.iter().enumerate() {
            print_text(&format!("Carried meeting decision {}", index + 1), decision);
        }
        print_terminal_bridge_section(
            self.terminal_bridge_context.as_ref(),
            self.terminal_bridge_context
                .as_ref()
                .map(|context| context.continuity_source.as_str())
                .unwrap_or(SHARED_DEFAULT_STATE_ROOT_SOURCE),
        );
        print_text("Selected action", &self.selected_action);
        print_text("Action plan", &self.action_plan);
        print_text("Verification steps", &self.verification_steps);
        print_text("Action status", &self.action_status);
        print_text(
            "Changed files after action",
            &self.changed_files_after_action,
        );
        print_text("Verification status", &self.verification_status);
        print_text("Verification summary", &self.verification_summary);
        println!("Memory records: {}", self.memory_record_count);
        println!("Evidence records: {}", self.evidence_record_count);
    }
}

fn required_engineer_evidence_value<'a>(
    evidence_records: &'a [EvidenceRecord],
    prefix: &str,
    handoff_source: &str,
) -> crate::SimardResult<&'a str> {
    evidence_records
        .iter()
        .rev()
        .find_map(|record| record.detail.strip_prefix(prefix))
        .ok_or_else(|| crate::SimardError::InvalidHandoffSnapshot {
            field: prefix.trim_end_matches('=').to_string(),
            reason: format!(
                "engineer read requires {handoff_source} to carry persisted engineer evidence '{}' for operator output",
                prefix.trim_end_matches('=')
            ),
        })
}

fn parse_engineer_summary_list(raw: &str, separator: &str) -> Vec<String> {
    if raw == "<none>" {
        return Vec::new();
    }

    raw.split(separator)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_carried_meeting_decisions(raw: &str) -> crate::SimardResult<Vec<String>> {
    if raw == "<none>" {
        return Ok(Vec::new());
    }

    let persisted_records = raw
        .split(" || ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if persisted_records.is_empty() {
        return Err(crate::SimardError::InvalidHandoffSnapshot {
            field: "carried-meeting-decisions".to_string(),
            reason: format!(
                "engineer read requires {ENGINEER_HANDOFF_FILE_NAME} or {} to carry at least one persisted meeting record or '<none>' for carried-meeting-decisions",
                crate::terminal_engineer_bridge::COMPATIBILITY_HANDOFF_FILE_NAME
            ),
        });
    }

    let mut decisions = Vec::new();
    for persisted_record in persisted_records {
        let record = PersistedMeetingRecord::parse(persisted_record).map_err(|error| {
            crate::SimardError::InvalidHandoffSnapshot {
                field: "carried-meeting-decisions".to_string(),
                reason: format!(
                    "engineer read requires valid persisted meeting records for carried-meeting-decisions: {error}"
                ),
            }
        })?;
        decisions.extend(record.decisions);
    }
    Ok(decisions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::EvidenceSource;
    use crate::session::{SessionId, SessionPhase};

    fn make_evidence(detail: &str) -> EvidenceRecord {
        EvidenceRecord {
            id: "ev-test".to_string(),
            session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
            phase: SessionPhase::Execution,
            detail: detail.to_string(),
            source: EvidenceSource::Runtime,
        }
    }

    // --- parse_engineer_summary_list ---

    #[test]
    fn summary_list_returns_empty_for_none_marker() {
        assert!(parse_engineer_summary_list("<none>", ", ").is_empty());
    }

    #[test]
    fn summary_list_parses_comma_separated() {
        let result = parse_engineer_summary_list("goal-a, goal-b, goal-c", ", ");
        assert_eq!(result, vec!["goal-a", "goal-b", "goal-c"]);
    }

    #[test]
    fn summary_list_trims_whitespace() {
        let result = parse_engineer_summary_list("  alpha ,  beta  ", ",");
        assert_eq!(result, vec!["alpha", "beta"]);
    }

    #[test]
    fn summary_list_filters_empty_entries() {
        let result = parse_engineer_summary_list("a, , b", ", ");
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn summary_list_single_item() {
        let result = parse_engineer_summary_list("only-one", ", ");
        assert_eq!(result, vec!["only-one"]);
    }

    #[test]
    fn summary_list_with_pipe_separator() {
        let result = parse_engineer_summary_list("x | y | z", " | ");
        assert_eq!(result, vec!["x", "y", "z"]);
    }

    #[test]
    fn summary_list_all_empty_after_split() {
        let result = parse_engineer_summary_list(", , ", ", ");
        assert!(result.is_empty());
    }

    // --- parse_carried_meeting_decisions ---

    #[test]
    fn carried_decisions_returns_empty_for_none_marker() {
        let result = parse_carried_meeting_decisions("<none>").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn carried_decisions_errors_on_invalid_record_format() {
        let result = parse_carried_meeting_decisions("not-a-valid-record");
        assert!(result.is_err());
    }

    #[test]
    fn carried_decisions_parses_valid_record_with_decisions() {
        let record = "agenda=Sprint review; updates=[Updated A]; decisions=[Use strategy X | Defer Y]; risks=[Risk 1]; next_steps=[Step 1]; open_questions=[Question 1]; goals=[p1:active:Goal title:Goal rationale]";
        let result = parse_carried_meeting_decisions(record).unwrap();
        assert_eq!(result, vec!["Use strategy X", "Defer Y"]);
    }

    #[test]
    fn carried_decisions_multiple_records_merged() {
        let record_a = "agenda=Review A; updates=[U1]; decisions=[D1]; risks=[R1]; next_steps=[N1]; open_questions=[Q1]; goals=[p1:active:G1:Rationale]";
        let record_b = "agenda=Review B; updates=[U2]; decisions=[D2 | D3]; risks=[R2]; next_steps=[N2]; open_questions=[Q2]; goals=[p2:active:G2:Rationale]";
        let combined = format!("{record_a} || {record_b}");
        let result = parse_carried_meeting_decisions(&combined).unwrap();
        assert_eq!(result, vec!["D1", "D2", "D3"]);
    }

    // --- required_engineer_evidence_value ---

    #[test]
    fn required_evidence_returns_value_for_matching_prefix() {
        let records = vec![make_evidence("repo-root=/home/user/project")];
        let result =
            required_engineer_evidence_value(&records, "repo-root=", "test-handoff").unwrap();
        assert_eq!(result, "/home/user/project");
    }

    #[test]
    fn required_evidence_errors_when_no_match() {
        let records = vec![make_evidence("repo-root=/home/user/project")];
        let result =
            required_engineer_evidence_value(&records, "nonexistent-prefix=", "test-handoff");
        assert!(result.is_err());
    }

    #[test]
    fn required_evidence_returns_last_matching_value_via_rev() {
        let records = vec![
            make_evidence("repo-branch=main"),
            make_evidence("repo-branch=feature/new"),
        ];
        let result =
            required_engineer_evidence_value(&records, "repo-branch=", "test-handoff").unwrap();
        assert_eq!(result, "feature/new");
    }

    #[test]
    fn required_evidence_ignores_non_matching_records() {
        let records = vec![
            make_evidence("other-key=value1"),
            make_evidence("repo-head=abc123"),
            make_evidence("another-key=value2"),
        ];
        let result =
            required_engineer_evidence_value(&records, "repo-head=", "test-handoff").unwrap();
        assert_eq!(result, "abc123");
    }

    #[test]
    fn required_evidence_empty_records_errors() {
        let records: Vec<EvidenceRecord> = vec![];
        let result = required_engineer_evidence_value(&records, "repo-root=", "test-handoff");
        assert!(result.is_err());
    }

    #[test]
    fn required_evidence_error_message_contains_field() {
        let records: Vec<EvidenceRecord> = vec![];
        let err =
            required_engineer_evidence_value(&records, "repo-root=", "test-handoff").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("repo-root"),
            "error should mention field name: {msg}"
        );
    }

    #[test]
    fn required_evidence_error_message_contains_handoff_source() {
        let records: Vec<EvidenceRecord> = vec![];
        let err = required_engineer_evidence_value(&records, "repo-root=", "my-handoff.json")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("my-handoff.json"),
            "error should mention handoff source: {msg}"
        );
    }

    #[test]
    fn required_evidence_strips_prefix_correctly() {
        let records = vec![make_evidence("worktree-dirty=false")];
        let result =
            required_engineer_evidence_value(&records, "worktree-dirty=", "test-handoff").unwrap();
        assert_eq!(result, "false");
    }

    #[test]
    fn required_evidence_handles_empty_value_after_prefix() {
        let records = vec![make_evidence("changed-files=")];
        let result =
            required_engineer_evidence_value(&records, "changed-files=", "test-handoff").unwrap();
        assert_eq!(result, "");
    }

    // --- parse_engineer_summary_list extended ---

    #[test]
    fn summary_list_preserves_internal_whitespace() {
        let result = parse_engineer_summary_list("goal with spaces, another goal", ", ");
        assert_eq!(result, vec!["goal with spaces", "another goal"]);
    }

    #[test]
    fn summary_list_exact_none_marker() {
        // Only exactly "<none>" should return empty
        let result = parse_engineer_summary_list("<NONE>", ", ");
        assert_eq!(result, vec!["<NONE>"]);
    }

    #[test]
    fn summary_list_single_separator_only() {
        let result = parse_engineer_summary_list(", ", ", ");
        assert!(result.is_empty());
    }

    // --- parse_carried_meeting_decisions extended ---

    #[test]
    fn carried_decisions_rejects_empty_after_split() {
        // A string that splits into only empty pieces after trim
        let result = parse_carried_meeting_decisions(" || ");
        assert!(result.is_err(), "should reject empty records after split");
    }

    #[test]
    fn carried_decisions_single_valid_record_no_decisions() {
        let record = "agenda=Review; updates=[U]; decisions=[]; risks=[R]; next_steps=[N]; open_questions=[Q]; goals=[p1:active:G:Rationale]";
        let result = parse_carried_meeting_decisions(record).unwrap();
        assert!(
            result.is_empty(),
            "empty decisions list should yield empty vec"
        );
    }

    #[test]
    fn carried_decisions_error_mentions_field() {
        let err = parse_carried_meeting_decisions("garbage input").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("carried-meeting-decisions"),
            "error should mention field: {msg}"
        );
    }

    // --- required_engineer_evidence_value with special characters ---

    #[test]
    fn required_evidence_value_with_equals_in_value() {
        let records = vec![make_evidence("repo-root=/home/user=special")];
        let result =
            required_engineer_evidence_value(&records, "repo-root=", "test-handoff").unwrap();
        assert_eq!(result, "/home/user=special");
    }

    #[test]
    fn required_evidence_value_with_spaces_in_value() {
        let records = vec![make_evidence("repo-branch=feature/my branch")];
        let result =
            required_engineer_evidence_value(&records, "repo-branch=", "test-handoff").unwrap();
        assert_eq!(result, "feature/my branch");
    }

    #[test]
    fn required_evidence_prefix_partial_match_does_not_match() {
        let records = vec![make_evidence("repo-root-extra=/value")];
        let result = required_engineer_evidence_value(&records, "repo-root=", "test-handoff");
        assert!(result.is_err(), "partial prefix match should not succeed");
    }

    // --- parse_engineer_summary_list: more edge cases ---

    #[test]
    fn summary_list_empty_string() {
        let result = parse_engineer_summary_list("", ", ");
        assert_eq!(result, Vec::<String>::new());
    }

    #[test]
    fn summary_list_separator_at_start() {
        let result = parse_engineer_summary_list(", alpha, beta", ", ");
        assert_eq!(result, vec!["alpha", "beta"]);
    }

    #[test]
    fn summary_list_separator_at_end() {
        let result = parse_engineer_summary_list("alpha, beta, ", ", ");
        assert_eq!(result, vec!["alpha", "beta"]);
    }

    #[test]
    fn summary_list_multiple_separators_in_row() {
        let result = parse_engineer_summary_list("a, , , b", ", ");
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn summary_list_none_marker_case_sensitive() {
        // "<None>" is not the same as "<none>"
        let result = parse_engineer_summary_list("<None>", ", ");
        assert_eq!(result, vec!["<None>"]);
    }

    // --- parse_carried_meeting_decisions: more patterns ---

    #[test]
    fn carried_decisions_none_marker_case_sensitive() {
        let result = parse_carried_meeting_decisions("<None>");
        assert!(
            result.is_err(),
            "<None> is not <none>, should be treated as invalid"
        );
    }

    #[test]
    fn carried_decisions_empty_input_errors() {
        let result = parse_carried_meeting_decisions("");
        assert!(result.is_err());
    }

    #[test]
    fn carried_decisions_valid_record_many_decisions() {
        let record = "agenda=Sprint; updates=[U]; decisions=[D1 | D2 | D3 | D4]; risks=[R]; next_steps=[N]; open_questions=[Q]; goals=[p1:active:G:Rationale]";
        let result = parse_carried_meeting_decisions(record).unwrap();
        assert_eq!(result.len(), 4);
    }

    // --- required_engineer_evidence_value: more patterns ---

    #[test]
    fn required_evidence_multiple_different_prefixes() {
        let records = vec![
            make_evidence("repo-root=/home/user"),
            make_evidence("repo-branch=main"),
            make_evidence("repo-head=abc123"),
            make_evidence("worktree-dirty=true"),
        ];
        assert_eq!(
            required_engineer_evidence_value(&records, "repo-root=", "h").unwrap(),
            "/home/user"
        );
        assert_eq!(
            required_engineer_evidence_value(&records, "repo-branch=", "h").unwrap(),
            "main"
        );
        assert_eq!(
            required_engineer_evidence_value(&records, "repo-head=", "h").unwrap(),
            "abc123"
        );
        assert_eq!(
            required_engineer_evidence_value(&records, "worktree-dirty=", "h").unwrap(),
            "true"
        );
    }

    #[test]
    fn required_evidence_error_mentions_engineer_read() {
        let records: Vec<EvidenceRecord> = vec![];
        let err =
            required_engineer_evidence_value(&records, "selected-action=", "engineer-handoff.json")
                .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("selected-action"),
            "error should mention field: {msg}"
        );
        assert!(
            msg.contains("engineer-handoff.json"),
            "error should mention source: {msg}"
        );
        assert!(
            msg.contains("engineer read"),
            "error should mention context: {msg}"
        );
    }

    // --- run_engineer_read_probe: error paths ---

    #[test]
    fn engineer_read_probe_rejects_nonexistent_state_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        let result = run_engineer_read_probe("single-process", Some(missing));
        assert!(result.is_err(), "should fail for nonexistent state root");
    }

    #[test]
    fn engineer_read_probe_rejects_empty_state_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = run_engineer_read_probe("single-process", Some(dir.path().to_path_buf()));
        assert!(
            result.is_err(),
            "should fail when handoff artifacts missing"
        );
    }

    #[test]
    fn engineer_read_probe_invalid_topology() {
        let result = run_engineer_read_probe("invalid-topology", None);
        assert!(result.is_err(), "should fail for invalid topology");
    }

    // --- run_engineer_loop_probe: error paths ---

    #[test]
    fn engineer_loop_probe_invalid_topology() {
        let result = run_engineer_loop_probe(
            "invalid-topology",
            std::path::Path::new("/nonexistent"),
            "test objective",
            None,
        );
        assert!(result.is_err(), "should fail for invalid topology");
    }

    #[test]
    fn engineer_loop_probe_nonexistent_workspace() {
        let result = run_engineer_loop_probe(
            "single-process",
            std::path::Path::new("/nonexistent/workspace/path"),
            "test objective",
            None,
        );
        assert!(result.is_err(), "should fail for nonexistent workspace");
    }
}
