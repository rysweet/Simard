use std::path::{Path, PathBuf};

use simard::{
    BootstrapConfig, BootstrapInputs, FileBackedMemoryStore, MemoryRecord, MemoryScope,
    MemoryStore, ReflectiveRuntime, ReviewRequest, ReviewTargetKind, RuntimeTopology,
    assemble_local_runtime_from_handoff, build_review_artifact, latest_local_handoff,
    latest_review_artifact, persist_review_artifact, render_review_context_directives,
    run_local_engineer_loop,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("expected a probe command")?;

    match command.as_str() {
        "bootstrap-run" => {
            let identity = args.next().ok_or("expected identity")?;
            let base_type = args.next().ok_or("expected base type")?;
            let topology = args.next().ok_or("expected topology")?;
            let objective = args.next().ok_or("expected objective")?;
            run_bootstrap_probe(&identity, &base_type, &topology, &objective)?;
        }
        "handoff-roundtrip" => {
            let identity = args.next().ok_or("expected identity")?;
            let base_type = args.next().ok_or("expected base type")?;
            let topology = args.next().ok_or("expected topology")?;
            let objective = args.next().ok_or("expected objective")?;
            run_handoff_probe(&identity, &base_type, &topology, &objective)?;
        }
        "meeting-run" => {
            let base_type = args.next().ok_or("expected base type")?;
            let topology = args.next().ok_or("expected topology")?;
            let objective = args.next().ok_or("expected objective")?;
            let state_root = args.next().map(PathBuf::from);
            run_meeting_probe(&base_type, &topology, &objective, state_root)?;
        }
        "goal-curation-run" => {
            let base_type = args.next().ok_or("expected base type")?;
            let topology = args.next().ok_or("expected topology")?;
            let objective = args.next().ok_or("expected objective")?;
            let state_root = args.next().map(PathBuf::from);
            run_goal_curation_probe(&base_type, &topology, &objective, state_root)?;
        }
        "terminal-run" => {
            let topology = args.next().ok_or("expected topology")?;
            let objective = args.next().ok_or("expected objective")?;
            run_terminal_probe(&topology, &objective)?;
        }
        "engineer-loop-run" => {
            let topology = args.next().ok_or("expected topology")?;
            let workspace_root = args.next().ok_or("expected workspace root")?;
            let objective = args.next().ok_or("expected objective")?;
            let state_root = args.next().map(PathBuf::from);
            run_engineer_loop_probe(
                &topology,
                Path::new(&workspace_root),
                &objective,
                state_root,
            )?;
        }
        "review-run" => {
            let base_type = args.next().ok_or("expected base type")?;
            let topology = args.next().ok_or("expected topology")?;
            let objective = args.next().ok_or("expected objective")?;
            let state_root = args.next().map(PathBuf::from);
            run_review_probe(&base_type, &topology, &objective, state_root)?;
        }
        "review-read" => {
            let base_type = args.next().ok_or("expected base type")?;
            let topology = args.next().ok_or("expected topology")?;
            let state_root = args.next().map(PathBuf::from);
            run_review_read_probe(&base_type, &topology, state_root)?;
        }
        "improvement-curation-run" => {
            let base_type = args.next().ok_or("expected base type")?;
            let topology = args.next().ok_or("expected topology")?;
            let objective = args.next().ok_or("expected objective")?;
            let state_root = args.next().map(PathBuf::from);
            run_improvement_curation_probe(&base_type, &topology, &objective, state_root)?;
        }
        other => return Err(format!("unsupported probe command '{other}'").into()),
    }

    Ok(())
}

fn prompt_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")
}

fn state_root(identity: &str, base_type: &str, topology: &str, probe: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/operator-probe-state")
        .join(probe)
        .join(identity)
        .join(base_type)
        .join(topology)
}

fn resolved_state_root(
    explicit: Option<PathBuf>,
    identity: &str,
    base_type: &str,
    topology: &str,
    probe: &str,
) -> PathBuf {
    explicit.unwrap_or_else(|| state_root(identity, base_type, topology, probe))
}

fn resolved_review_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> PathBuf {
    resolved_state_root(
        explicit,
        "simard-engineer",
        base_type,
        topology,
        "review-run",
    )
}

fn run_bootstrap_probe(
    identity: &str,
    base_type: &str,
    topology: &str,
    objective: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(state_root(identity, base_type, topology, "bootstrap-run")),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = simard::run_local_session(&config)?;
    println!("Probe mode: bootstrap-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Identity components: {}",
        if execution.snapshot.identity_components.is_empty() {
            "<none>".to_string()
        } else {
            execution.snapshot.identity_components.join(", ")
        }
    );
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    println!(
        "Adapter implementation: {}",
        execution.snapshot.adapter_backend.identity
    );
    println!(
        "Topology backend: {}",
        execution.snapshot.topology_backend.identity
    );
    println!(
        "Transport backend: {}",
        execution.snapshot.transport_backend.identity
    );
    println!("State root: {}", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!("Shutdown: {}", execution.stopped_snapshot.runtime_state);
    println!("Execution summary: {}", execution.outcome.execution_summary);
    println!(
        "Reflection summary: {}",
        execution.outcome.reflection.summary
    );
    Ok(())
}

fn run_handoff_probe(
    identity: &str,
    base_type: &str,
    topology: &str,
    objective: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(state_root(
            identity,
            base_type,
            topology,
            "handoff-roundtrip",
        )),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = simard::run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable handoff snapshot")?;
    let restored = assemble_local_runtime_from_handoff(&config, exported.clone())?;
    let restored_snapshot = restored.snapshot()?;

    println!("Probe mode: handoff-roundtrip");
    println!("State root: {}", config.state_root_path().display());
    println!("Identity: {}", restored_snapshot.identity_name);
    println!(
        "Identity components: {}",
        if restored_snapshot.identity_components.is_empty() {
            "<none>".to_string()
        } else {
            restored_snapshot.identity_components.join(", ")
        }
    );
    println!(
        "Selected base type: {}",
        restored_snapshot.selected_base_type
    );
    println!("Topology: {}", restored_snapshot.topology);
    println!("Runtime node: {}", restored_snapshot.runtime_node);
    println!("Mailbox address: {}", restored_snapshot.mailbox_address);
    println!("Exported memory records: {}", exported.memory_records.len());
    println!(
        "Exported evidence records: {}",
        exported.evidence_records.len()
    );
    println!("Restored state: {}", restored_snapshot.runtime_state);
    println!(
        "Restored session phase: {}",
        restored_snapshot
            .session_phase
            .map(|phase| phase.to_string())
            .unwrap_or_else(|| "<none>".to_string())
    );
    println!(
        "Restored adapter implementation: {}",
        restored_snapshot.adapter_backend.identity
    );
    println!(
        "Restored topology backend: {}",
        restored_snapshot.topology_backend.identity
    );
    println!(
        "Restored transport backend: {}",
        restored_snapshot.transport_backend.identity
    );
    println!("Execution summary: {}", execution.outcome.execution_summary);
    Ok(())
}

fn run_meeting_probe(
    base_type: &str,
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-meeting";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_state_root(
            state_root_override,
            identity,
            base_type,
            topology,
            "meeting-run",
        )),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = simard::run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable meeting handoff")?;
    let decision_records = exported
        .memory_records
        .iter()
        .filter(|record| record.scope == MemoryScope::Decision)
        .map(|record| record.value.clone())
        .collect::<Vec<_>>();

    println!("Probe mode: meeting-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    println!("State root: {}", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!("Decision records: {}", decision_records.len());
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        println!("Active goal {}: {}", index + 1, goal);
    }
    for (index, value) in decision_records.iter().enumerate() {
        println!("Decision record {}: {}", index + 1, value);
    }
    println!("Execution summary: {}", execution.outcome.execution_summary);
    println!(
        "Reflection summary: {}",
        execution.outcome.reflection.summary
    );
    Ok(())
}

fn run_goal_curation_probe(
    base_type: &str,
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-goal-curator";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_state_root(
            state_root_override,
            identity,
            base_type,
            topology,
            "goal-curation-run",
        )),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = simard::run_local_session(&config)?;
    println!("Probe mode: goal-curation-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    println!("State root: {}", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        println!("Active goal {}: {}", index + 1, goal);
    }
    println!("Execution summary: {}", execution.outcome.execution_summary);
    println!(
        "Reflection summary: {}",
        execution.outcome.reflection.summary
    );
    Ok(())
}

fn run_terminal_probe(topology: &str, objective: &str) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-engineer";
    let base_type = "terminal-shell";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(state_root(identity, base_type, topology, "terminal-run")),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = simard::run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable terminal handoff")?;
    let terminal_evidence = exported
        .evidence_records
        .iter()
        .filter(|record| {
            record.detail.starts_with("shell=")
                || record.detail.starts_with("terminal-")
                || record.detail.starts_with("backend-implementation=")
        })
        .map(|record| record.detail.clone())
        .collect::<Vec<_>>();

    println!("Probe mode: terminal-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    println!(
        "Adapter implementation: {}",
        execution.snapshot.adapter_backend.identity
    );
    println!(
        "Adapter capabilities: {}",
        execution.snapshot.adapter_capabilities.join(", ")
    );
    println!("State root: {}", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!("Terminal evidence lines: {}", terminal_evidence.len());
    for detail in terminal_evidence {
        println!("Terminal evidence: {detail}");
    }
    println!("Execution summary: {}", execution.outcome.execution_summary);
    println!(
        "Reflection summary: {}",
        execution.outcome.reflection.summary
    );
    Ok(())
}

fn run_engineer_loop_probe(
    topology: &str,
    workspace_root: &Path,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_topology = parse_runtime_topology(topology)?;
    let state_root = resolved_state_root(
        state_root_override,
        "simard-engineer",
        "terminal-shell",
        topology,
        "engineer-loop-run",
    );
    let run = run_local_engineer_loop(workspace_root, objective, runtime_topology, &state_root)
        .map_err(|error| format!("{error}"))?;

    println!("Probe mode: engineer-loop-run");
    println!("Repo root: {}", run.inspection.repo_root.display());
    println!("Repo branch: {}", run.inspection.branch);
    println!("Repo head: {}", run.inspection.head);
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
        println!("Active goal {}: {}", index + 1, goal.concise_label());
    }
    println!(
        "Carried meeting decisions: {}",
        run.inspection.carried_meeting_decisions.len()
    );
    for (index, decision) in run.inspection.carried_meeting_decisions.iter().enumerate() {
        println!("Carried meeting decision {}: {}", index + 1, decision);
    }
    println!("Gap summary: {}", run.inspection.architecture_gap_summary);
    println!("Execution scope: {}", run.execution_scope);
    println!("Selected action: {}", run.action.selected.label);
    println!("Action plan: {}", run.action.selected.plan_summary);
    println!(
        "Verification steps: {}",
        run.action.selected.verification_steps.join(" || ")
    );
    println!("Action rationale: {}", run.action.selected.rationale);
    println!("Action command: {}", run.action.selected.argv.join(" "));
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
    println!("Verification summary: {}", run.verification.summary);
    println!("State root: {}", run.state_root.display());
    Ok(())
}

fn run_review_probe(
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
        )),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = simard::run_local_session(&config)?;
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
        scope: MemoryScope::Decision,
        value: review.concise_record(),
        session_id,
        recorded_in: simard::SessionPhase::Complete,
    })?;
    let decision_records = memory_store.list(MemoryScope::Decision)?;

    println!("Probe mode: review-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    println!("State root: {}", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!("Review artifact: {}", review_artifact_path.display());
    println!("Review proposals: {}", review.proposals.len());
    for (index, proposal) in review.proposals.iter().enumerate() {
        println!(
            "Proposal {}: [{}] {} => {}",
            index + 1,
            proposal.category,
            proposal.title,
            proposal.suggested_change
        );
    }
    println!("Decision records: {}", decision_records.len());
    println!("Review record key: {}", review_key);
    println!("Execution summary: {}", execution.outcome.execution_summary);
    println!(
        "Reflection summary: {}",
        execution.outcome.reflection.summary
    );
    Ok(())
}

fn run_review_read_probe(
    base_type: &str,
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_review_state_root(state_root_override, base_type, topology);
    let (review_artifact_path, review) =
        latest_review_artifact(&state_root)?.ok_or("expected persisted review artifact")?;
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let decision_records = memory_store
        .list(MemoryScope::Decision)?
        .into_iter()
        .filter(|record| record.value.starts_with("review-summary |"))
        .collect::<Vec<_>>();

    println!("Probe mode: review-read");
    println!("Identity: {}", review.identity_name);
    println!("Selected base type: {}", review.selected_base_type);
    println!("Topology: {}", review.topology);
    println!("State root: {}", state_root.display());
    println!("Latest review artifact: {}", review_artifact_path.display());
    println!("Latest review target: {}", review.target_label);
    println!("Latest review summary: {}", review.summary);
    println!("Review proposals: {}", review.proposals.len());
    for (index, proposal) in review.proposals.iter().enumerate() {
        println!(
            "Proposal {}: [{}] {} => {}",
            index + 1,
            proposal.category,
            proposal.title,
            proposal.suggested_change
        );
    }
    println!("Decision review records: {}", decision_records.len());
    if let Some(record) = decision_records.last() {
        println!("Latest decision review record: {}", record.value);
    }
    Ok(())
}

fn run_improvement_curation_probe(
    base_type: &str,
    topology: &str,
    operator_objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_review_state_root(state_root_override, base_type, topology);
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

    let execution = simard::run_local_session(&config)?;
    let plan = simard::ImprovementPromotionPlan::parse(&objective)?;
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
    println!("State root: {}", config.state_root_path().display());
    println!("Review artifact: {}", review_artifact_path.display());
    println!("Review id: {}", review.review_id);
    println!("Review target: {}", review.target_label);
    println!("Review proposals: {}", review.proposals.len());
    println!("Approved proposals: {}", plan.approvals.len());
    for (index, approval) in plan.approvals.iter().enumerate() {
        println!(
            "Approved proposal {}: p{} [{}] {}",
            index + 1,
            approval.priority,
            approval.status,
            approval.title
        );
    }
    println!("Deferred proposals: {}", plan.deferrals.len());
    for (index, deferral) in plan.deferrals.iter().enumerate() {
        println!(
            "Deferred proposal {}: {} ({})",
            index + 1,
            deferral.title,
            deferral.rationale
        );
    }
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        println!("Active goal {}: {}", index + 1, goal);
    }
    println!(
        "Proposed goals count: {}",
        execution.snapshot.proposed_goal_count
    );
    for (index, goal) in execution.snapshot.proposed_goals.iter().enumerate() {
        println!("Proposed goal {}: {}", index + 1, goal);
    }
    println!("Decision records: {}", improvement_records.len());
    if let Some(record) = improvement_records.last() {
        println!("Latest improvement record: {}", record.value);
    }
    println!("Execution summary: {}", execution.outcome.execution_summary);
    println!(
        "Reflection summary: {}",
        execution.outcome.reflection.summary
    );
    Ok(())
}

fn parse_runtime_topology(value: &str) -> Result<RuntimeTopology, Box<dyn std::error::Error>> {
    match value {
        "single-process" => Ok(RuntimeTopology::SingleProcess),
        "multi-process" => Ok(RuntimeTopology::MultiProcess),
        "distributed" => Ok(RuntimeTopology::Distributed),
        other => Err(format!(
            "unsupported runtime topology '{other}'; expected single-process, multi-process, or distributed"
        )
        .into()),
    }
}
