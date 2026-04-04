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
