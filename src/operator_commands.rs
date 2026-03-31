use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::base_types::BaseTypeId;
use crate::bootstrap::validate_state_root;
use crate::copilot_status_probe::{
    CopilotStatusProbeResult, is_copilot_guarded_recipe, probe_local_copilot_status,
};
use crate::copilot_task_submit::{CopilotSubmitRun, run_copilot_submit};
use crate::goals::{FileBackedGoalStore, GoalRecord, GoalStatus, GoalStore};
use crate::improvements::PersistedImprovementRecord;
use crate::meetings::PersistedMeetingRecord;
use crate::prompt_assets::{FilePromptAssetStore, PromptAsset, PromptAssetRef, PromptAssetStore};
use crate::sanitization::sanitize_terminal_text;
use crate::terminal_engineer_bridge::{
    ENGINEER_HANDOFF_FILE_NAME, ENGINEER_MODE_BOUNDARY, SHARED_DEFAULT_STATE_ROOT_SOURCE,
    SHARED_EXPLICIT_STATE_ROOT_SOURCE, ScopedHandoffMode, TERMINAL_HANDOFF_FILE_NAME,
    TERMINAL_MODE_BOUNDARY, TerminalBridgeContext, load_runtime_handoff_snapshot,
    persist_handoff_artifacts, scoped_handoff_path, select_handoff_artifact_for_read,
};
use crate::{
    BootstrapConfig, BootstrapInputs, BuiltinIdentityLoader, CopilotSubmitAudit, EvidenceRecord,
    FileBackedEvidenceStore, FileBackedMemoryStore, Freshness, IdentityLoadRequest, IdentityLoader,
    ManifestContract, MemoryRecord, MemoryScope, MemoryStore, Provenance, ReflectiveRuntime,
    ReviewRequest, ReviewTargetKind, RuntimeHandoffSnapshot, RuntimeTopology,
    assemble_local_runtime_from_handoff, benchmark_scenarios, build_review_artifact,
    builtin_base_type_registry_for_manifest, compare_latest_benchmark_runs, default_output_root,
    latest_local_handoff, latest_review_artifact, persist_review_artifact,
    render_review_context_directives, review_artifacts_dir, run_benchmark_scenario,
    run_benchmark_suite, run_local_engineer_loop, run_local_session,
};

pub fn dispatch_operator_probe<I>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let command = args.next().ok_or("expected a probe command")?;

    match command.as_str() {
        "bootstrap-run" => {
            let identity = next_required(&mut args, "identity")?;
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_bootstrap_probe(&identity, &base_type, &topology, &objective, state_root)?;
        }
        "handoff-roundtrip" => {
            let identity = next_required(&mut args, "identity")?;
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            reject_extra_args(args)?;
            run_handoff_probe(&identity, &base_type, &topology, &objective)?;
        }
        "meeting-run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_meeting_probe(&base_type, &topology, &objective, state_root)?;
        }
        "meeting-read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_meeting_read_probe(&base_type, &topology, state_root)?;
        }
        "goal-curation-run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_goal_curation_probe(&base_type, &topology, &objective, state_root)?;
        }
        "terminal-run" => {
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_probe(&topology, &objective, state_root)?;
        }
        "terminal-run-file" => {
            let topology = next_required(&mut args, "topology")?;
            let objective_path = next_required(&mut args, "objective file")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_probe_from_file(&topology, Path::new(&objective_path), state_root)?;
        }
        "terminal-read" => {
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_read_probe(&topology, state_root)?;
        }
        "terminal-recipe-list" => {
            reject_extra_args(args)?;
            run_terminal_recipe_list_probe()?;
        }
        "terminal-recipe-show" => {
            let recipe_name = next_required(&mut args, "recipe name")?;
            reject_extra_args(args)?;
            run_terminal_recipe_show_probe(&recipe_name)?;
        }
        "terminal-recipe-run" => {
            let topology = next_required(&mut args, "topology")?;
            let recipe_name = next_required(&mut args, "recipe name")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_recipe_probe(&topology, &recipe_name, state_root)?;
        }
        "engineer-loop-run" => {
            let topology = next_required(&mut args, "topology")?;
            let workspace_root = next_required(&mut args, "workspace root")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_engineer_loop_probe(
                &topology,
                Path::new(&workspace_root),
                &objective,
                state_root,
            )?;
        }
        "engineer-read" => {
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_engineer_read_probe(&topology, state_root)?;
        }
        "review-run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_review_probe(&base_type, &topology, &objective, state_root)?;
        }
        "review-read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_review_read_probe(&base_type, &topology, state_root)?;
        }
        "improvement-curation-run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_improvement_curation_probe(&base_type, &topology, &objective, state_root)?;
        }
        "improvement-curation-read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_improvement_curation_read_probe(&base_type, &topology, state_root)?;
        }
        other => return Err(format!("unsupported probe command '{other}'").into()),
    }

    Ok(())
}

pub fn dispatch_legacy_gym_cli<I>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let command = args.next().ok_or(gym_usage())?;

    match command.as_str() {
        "list" => {
            reject_extra_args(args)?;
            run_gym_list()?;
        }
        "run" => {
            let scenario_id = next_required(&mut args, "scenario id")?;
            reject_extra_args(args)?;
            run_gym_scenario(&scenario_id)?;
        }
        "compare" => {
            let scenario_id = next_required(&mut args, "scenario id")?;
            reject_extra_args(args)?;
            run_gym_compare(&scenario_id)?;
        }
        "run-suite" => {
            let suite_id = next_required(&mut args, "suite id")?;
            reject_extra_args(args)?;
            run_gym_suite(&suite_id)?;
        }
        _ => return Err(gym_usage().into()),
    }

    Ok(())
}

pub fn gym_usage() -> &'static str {
    "usage: simard-gym <list|run <scenario-id>|compare <scenario-id>|run-suite <suite-id>>"
}

pub fn run_bootstrap_probe(
    identity: &str,
    base_type: &str,
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_state_root(
            state_root_override,
            identity,
            base_type,
            topology,
            "bootstrap-run",
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
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
    print_display("State root", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!("Shutdown: {}", execution.stopped_snapshot.runtime_state);
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

pub fn run_handoff_probe(
    identity: &str,
    base_type: &str,
    topology: &str,
    objective: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_state_root(
            None,
            identity,
            base_type,
            topology,
            "handoff-roundtrip",
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable handoff snapshot")?;
    let restored = assemble_local_runtime_from_handoff(&config, exported.clone())?;
    let restored_snapshot = restored.snapshot()?;

    println!("Probe mode: handoff-roundtrip");
    print_display("State root", config.state_root_path().display());
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
            .map(|phase: crate::SessionPhase| phase.to_string())
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
    print_text("Execution summary", &execution.outcome.execution_summary);
    Ok(())
}

pub fn run_meeting_probe(
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
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
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
    print_display("State root", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!("Decision records: {}", decision_records.len());
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal);
    }
    for (index, value) in decision_records.iter().enumerate() {
        print_text(&format!("Decision record {}", index + 1), value);
    }
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

pub fn run_meeting_read_probe(
    base_type: &str,
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_meeting_read_state_root(state_root_override, base_type, topology)?;
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let meeting_records = memory_store
        .list(MemoryScope::Decision)?
        .into_iter()
        .filter(|record| crate::looks_like_persisted_meeting_record(&record.value))
        .collect::<Vec<_>>();
    let latest_record = meeting_records
        .last()
        .ok_or("expected persisted meeting decision record")?;
    let parsed_record =
        PersistedMeetingRecord::parse(&latest_record.value).map_err(|error| format!("{error}"))?;

    println!("Probe mode: meeting-read");
    println!("Identity: simard-meeting");
    print_text("Selected base type", base_type);
    print_text("Topology", topology);
    print_display("State root", state_root.display());
    println!("Meeting records: {}", meeting_records.len());
    print_text("Latest agenda", &parsed_record.agenda);
    print_string_section("Updates", &parsed_record.updates);
    print_string_section("Decisions", &parsed_record.decisions);
    print_string_section("Risks", &parsed_record.risks);
    print_string_section("Next steps", &parsed_record.next_steps);
    print_string_section("Open questions", &parsed_record.open_questions);
    print_meeting_goal_section(&parsed_record.goals);
    print_text("Latest meeting record", &latest_record.value);
    Ok(())
}

pub fn run_goal_curation_probe(
    base_type: &str,
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-goal-curator";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_goal_curation_state_root(
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
    println!("Probe mode: goal-curation-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    print_display("State root", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal);
    }
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

pub fn run_goal_curation_read_probe(
    base_type: &str,
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_goal_curation_state_root(state_root_override, base_type, topology)?;
    let goal_store = FileBackedGoalStore::try_new(state_root.join("goal_records.json"))?;
    let goal_records = goal_store.list()?;
    let register = GoalRegisterView::from_records(goal_records);

    println!("Goal register: durable");
    print_text("Selected base type", base_type);
    print_text("Topology", topology);
    print_display("State root", state_root.display());
    register.print();
    Ok(())
}

pub fn run_terminal_probe(
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-engineer";
    let base_type = "terminal-shell";
    let state_root_was_explicit = state_root_override.is_some();
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_state_root(
            state_root_override,
            identity,
            base_type,
            topology,
            "terminal-run",
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable terminal handoff")?;
    persist_handoff_artifacts(
        config.state_root_path(),
        ScopedHandoffMode::Terminal,
        &exported,
    )?;
    let handoff_source = scoped_handoff_path(config.state_root_path(), ScopedHandoffMode::Terminal)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("latest_terminal_handoff.json")
        .to_string();
    let continuity_source = if state_root_was_explicit {
        SHARED_EXPLICIT_STATE_ROOT_SOURCE
    } else {
        SHARED_DEFAULT_STATE_ROOT_SOURCE
    };
    let view = TerminalReadView::from_handoff(
        config.state_root_path().to_path_buf(),
        exported,
        handoff_source,
        Some(continuity_source.to_string()),
    )?;
    view.print_terminal_run(
        &execution.snapshot.adapter_capabilities,
        &execution.outcome.execution_summary,
        &execution.outcome.reflection.summary,
    );
    Ok(())
}

pub fn run_terminal_probe_from_file(
    topology: &str,
    objective_path: &Path,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let objective = load_terminal_objective_file(objective_path)?;
    run_terminal_probe(topology, &objective, state_root_override)
}

pub fn run_terminal_read_probe(
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_terminal_read_state_root(state_root_override, topology)?;
    let view = TerminalReadView::load(state_root)?;
    view.print();
    Ok(())
}

pub fn run_terminal_recipe_list_probe() -> Result<(), Box<dyn std::error::Error>> {
    let recipes = list_terminal_recipe_descriptors()?;
    println!("Terminal recipes: {}", recipes.len());
    for (index, recipe) in recipes.iter().enumerate() {
        print_text(
            &format!("Terminal recipe {}", index + 1),
            format!(
                "{} ({})",
                recipe.name,
                recipe.reference.relative_path.display()
            ),
        );
    }
    Ok(())
}

pub fn run_terminal_recipe_show_probe(recipe_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let recipe = load_terminal_recipe(recipe_name)?;
    print_terminal_recipe(recipe_name, &recipe);
    Ok(())
}

pub fn run_terminal_recipe_probe(
    topology: &str,
    recipe_name: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    ensure_terminal_recipe_is_runnable(recipe_name)?;
    let recipe = load_terminal_recipe(recipe_name)?;
    run_terminal_probe(topology, &recipe.contents, state_root_override)
}

pub fn run_copilot_submit_probe(
    topology: &str,
    state_root_override: Option<PathBuf>,
    json_output: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_topology = parse_runtime_topology(topology)?;
    let state_root = resolved_state_root(
        state_root_override,
        "simard-engineer",
        "terminal-shell",
        topology,
        "terminal-run",
    )?;
    match run_copilot_submit(runtime_topology, &state_root)? {
        CopilotSubmitRun::Success(report) => {
            print_copilot_submit_report(&state_root, topology, &report, json_output)?;
            Ok(())
        }
        CopilotSubmitRun::Unsupported(report) => {
            print_copilot_submit_report(&state_root, topology, &report, json_output)?;
            Err(crate::SimardError::ActionExecutionFailed {
                action: "copilot-submit".to_string(),
                reason: format!(
                    "unsupported: {}",
                    report.reason_code.as_deref().unwrap_or("unknown-reason")
                ),
            }
            .into())
        }
    }
}

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
        scope: MemoryScope::Decision,
        value: review.concise_record(),
        session_id,
        recorded_in: crate::SessionPhase::Complete,
    })?;
    let decision_records = memory_store.list(MemoryScope::Decision)?;

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
        .list(MemoryScope::Decision)?
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

pub fn run_improvement_curation_probe(
    base_type: &str,
    topology: &str,
    operator_objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_review_state_root(state_root_override, base_type, topology)?;
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
    let state_root =
        resolved_improvement_curation_read_state_root(state_root_override, base_type, topology)?;
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

pub fn run_gym_list() -> Result<(), Box<dyn std::error::Error>> {
    println!("Simard benchmark scenarios:");
    for scenario in benchmark_scenarios() {
        println!(
            "- {} | class={} | identity={} | base_type={} | topology={}",
            scenario.id, scenario.class, scenario.identity, scenario.base_type, scenario.topology
        );
        println!("  {}", scenario.title);
    }
    Ok(())
}

pub fn run_gym_scenario(scenario_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let report = run_benchmark_scenario(scenario_id, default_output_root())?;
    print_text("Scenario", report.scenario.id);
    print_text("Suite", &report.suite_id);
    print_text("Session", &report.session_id);
    print_display("Passed", report.passed);
    print_display(
        "Checks passed",
        format!(
            "{}/{}",
            report.scorecard.correctness_checks_passed, report.scorecard.correctness_checks_total
        ),
    );
    print_display(
        "Unnecessary actions",
        crate::gym::render_benchmark_count(report.scorecard.unnecessary_action_count),
    );
    print_display(
        "Retry count",
        crate::gym::render_benchmark_count(report.scorecard.retry_count),
    );
    print_text("Artifact report", &report.artifacts.report_json);
    print_text("Artifact summary", &report.artifacts.report_txt);
    print_text("Review artifact", &report.artifacts.review_json);
    Ok(())
}

pub fn run_gym_compare(scenario_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let report = compare_latest_benchmark_runs(scenario_id, default_output_root())?;
    print_text("Scenario", &report.scenario_id);
    print_display("Comparison status", report.status);
    print_text("Comparison summary", &report.summary);
    print_text("Current session", &report.current.session_id);
    print_display("Current passed", report.current.passed);
    print_display(
        "Current checks passed",
        format!(
            "{}/{}",
            report.current.correctness_checks_passed, report.current.correctness_checks_total
        ),
    );
    print_text("Current report", &report.current.report_json);
    print_display(
        "Current unnecessary actions",
        crate::gym::render_benchmark_count(report.current.unnecessary_action_count),
    );
    print_display(
        "Current retry count",
        crate::gym::render_benchmark_count(report.current.retry_count),
    );
    print_text("Previous session", &report.previous.session_id);
    print_display("Previous passed", report.previous.passed);
    print_display(
        "Previous checks passed",
        format!(
            "{}/{}",
            report.previous.correctness_checks_passed, report.previous.correctness_checks_total
        ),
    );
    print_text("Previous report", &report.previous.report_json);
    print_display(
        "Previous unnecessary actions",
        crate::gym::render_benchmark_count(report.previous.unnecessary_action_count),
    );
    print_display(
        "Previous retry count",
        crate::gym::render_benchmark_count(report.previous.retry_count),
    );
    print_display(
        "Delta correctness checks passed",
        format!("{:+}", report.delta.correctness_checks_passed),
    );
    print_display(
        "Delta unnecessary actions",
        crate::gym::render_benchmark_delta(report.delta.unnecessary_action_count),
    );
    print_display(
        "Delta retry count",
        crate::gym::render_benchmark_delta(report.delta.retry_count),
    );
    print_display(
        "Delta exported memory records",
        format!("{:+}", report.delta.exported_memory_records),
    );
    print_display(
        "Delta exported evidence records",
        format!("{:+}", report.delta.exported_evidence_records),
    );
    print_text(
        "Comparison artifact report",
        &report.artifact_paths.report_json,
    );
    print_text(
        "Comparison artifact summary",
        &report.artifact_paths.report_txt,
    );
    Ok(())
}

pub fn run_gym_suite(suite_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let report = run_benchmark_suite(suite_id, default_output_root())?;
    println!("Suite: {}", report.suite_id);
    println!("Suite passed: {}", report.passed);
    for scenario in &report.scenarios {
        println!(
            "- {}: {} ({})",
            scenario.scenario_id,
            if scenario.passed { "passed" } else { "failed" },
            scenario.report_json
        );
    }
    println!("Suite artifact report: {}", report.artifact_path);
    Ok(())
}

fn prompt_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")
}

fn ensure_terminal_recipe_is_runnable(recipe_name: &str) -> crate::SimardResult<()> {
    if !is_copilot_guarded_recipe(recipe_name) {
        return Ok(());
    }

    match probe_local_copilot_status() {
        CopilotStatusProbeResult::Available { .. } => Ok(()),
        CopilotStatusProbeResult::Unavailable {
            reason_code,
            detail,
        }
        | CopilotStatusProbeResult::Unsupported {
            reason_code,
            detail,
        } => Err(crate::SimardError::ActionExecutionFailed {
            action: recipe_name.to_string(),
            reason: format!("{reason_code}: {detail}"),
        }),
    }
}

const TERMINAL_RECIPE_DIRECTORY: &str = "simard/terminal_recipes";
const TERMINAL_RECIPE_EXTENSION: &str = "simard-terminal";

#[derive(Clone, Debug, Eq, PartialEq)]
struct TerminalRecipeDescriptor {
    name: String,
    reference: PromptAssetRef,
}

fn list_terminal_recipe_descriptors() -> crate::SimardResult<Vec<TerminalRecipeDescriptor>> {
    let recipe_root = prompt_root().join(TERMINAL_RECIPE_DIRECTORY);
    let entries =
        fs::read_dir(&recipe_root).map_err(|error| crate::SimardError::PromptAssetRead {
            path: recipe_root.clone(),
            reason: error.to_string(),
        })?;
    let mut recipes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| crate::SimardError::PromptAssetRead {
            path: recipe_root.clone(),
            reason: error.to_string(),
        })?;
        let entry_path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| crate::SimardError::PromptAssetRead {
                path: entry_path.clone(),
                reason: error.to_string(),
            })?;
        if !file_type.is_file()
            || entry_path.extension() != Some(OsStr::new(TERMINAL_RECIPE_EXTENSION))
        {
            continue;
        }
        let Some(stem) = entry_path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        recipes.push(TerminalRecipeDescriptor {
            name: stem.to_string(),
            reference: terminal_recipe_reference(stem)?,
        });
    }
    recipes.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(recipes)
}

fn load_terminal_recipe(recipe_name: &str) -> crate::SimardResult<PromptAsset> {
    let reference = terminal_recipe_reference(recipe_name)?;
    FilePromptAssetStore::new(prompt_root()).load(&reference)
}

fn terminal_recipe_reference(recipe_name: &str) -> crate::SimardResult<PromptAssetRef> {
    if recipe_name.is_empty()
        || !recipe_name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(crate::SimardError::InvalidPromptAssetPath {
            asset_id: format!("terminal-recipe:{recipe_name}"),
            path: PathBuf::from(recipe_name),
            reason: "recipe names may only use lowercase ASCII letters, digits, and hyphens"
                .to_string(),
        });
    }
    Ok(PromptAssetRef::new(
        format!("terminal-recipe:{recipe_name}"),
        PathBuf::from(TERMINAL_RECIPE_DIRECTORY)
            .join(format!("{recipe_name}.{TERMINAL_RECIPE_EXTENSION}")),
    ))
}

fn print_terminal_recipe(recipe_name: &str, recipe: &PromptAsset) {
    print_text("Terminal recipe", recipe_name);
    print_display("Recipe asset", recipe.relative_path.display());
    println!("Recipe contents:");
    for line in sanitize_terminal_text(&recipe.contents).lines() {
        println!("{line}");
    }
}

fn state_root(
    identity: &str,
    base_type: &BaseTypeId,
    topology: RuntimeTopology,
    probe: &str,
) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/operator-probe-state")
        .join(probe)
        .join(identity)
        .join(base_type.as_str())
        .join(topology.to_string())
}

fn resolved_goal_curation_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    resolved_state_root(
        explicit,
        "simard-goal-curator",
        base_type,
        topology,
        "goal-curation-run",
    )
}

struct ValidatedRuntimeSegments {
    base_type: BaseTypeId,
    topology: RuntimeTopology,
}

fn validated_runtime_segments(
    identity: &str,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<ValidatedRuntimeSegments> {
    let topology = parse_runtime_topology(topology)?;
    let contract = ManifestContract::new(
        concat!(module_path!(), "::validated_runtime_segments"),
        "operator-cli -> identity-loader -> base-type-registry",
        vec![
            format!("identity:{identity}"),
            format!("base-type:{base_type}"),
            format!("topology:{topology}"),
        ],
        Provenance::runtime(format!("operator-cli/default-state-root/{identity}")),
        Freshness::now()?,
    )?;
    let manifest = BuiltinIdentityLoader.load(&IdentityLoadRequest::new(
        identity,
        env!("CARGO_PKG_VERSION"),
        contract,
    ))?;
    let base_types = builtin_base_type_registry_for_manifest(&manifest)?;
    let requested_base_type = BaseTypeId::new(base_type);
    let factory = base_types.get(&requested_base_type).ok_or_else(|| {
        crate::SimardError::AdapterNotRegistered {
            base_type: base_type.to_string(),
        }
    })?;
    if !factory.descriptor().supports_topology(topology) {
        return Err(crate::SimardError::UnsupportedTopology {
            base_type: base_type.to_string(),
            topology,
        });
    }

    Ok(ValidatedRuntimeSegments {
        base_type: factory.descriptor().id.clone(),
        topology,
    })
}

fn resolved_state_root(
    explicit: Option<PathBuf>,
    identity: &str,
    base_type: &str,
    topology: &str,
    probe: &str,
) -> crate::SimardResult<PathBuf> {
    match explicit {
        Some(path) => validate_state_root(path),
        None => {
            let segments = validated_runtime_segments(identity, base_type, topology)?;
            validate_state_root(state_root(
                identity,
                &segments.base_type,
                segments.topology,
                probe,
            ))
        }
    }
}

fn resolved_review_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    resolved_state_root(
        explicit,
        "simard-engineer",
        base_type,
        topology,
        "review-run",
    )
}

fn resolved_improvement_curation_read_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    let state_root = resolved_review_state_root(explicit, base_type, topology)?;
    validate_improvement_curation_read_state_root(&state_root)?;
    Ok(state_root)
}

fn resolved_engineer_read_state_root(
    explicit: Option<PathBuf>,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    let state_root = resolved_state_root(
        explicit,
        "simard-engineer",
        "terminal-shell",
        topology,
        "engineer-loop-run",
    )?;
    validate_engineer_read_state_root(&state_root)?;
    Ok(state_root)
}

fn resolved_terminal_read_state_root(
    explicit: Option<PathBuf>,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    let state_root = resolved_state_root(
        explicit,
        "simard-engineer",
        "terminal-shell",
        topology,
        "terminal-run",
    )?;
    validate_terminal_read_state_root(&state_root)?;
    Ok(state_root)
}

fn resolved_meeting_read_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    let state_root = resolved_state_root(
        explicit,
        "simard-meeting",
        base_type,
        topology,
        "meeting-run",
    )?;
    validate_meeting_read_state_root(&state_root)?;
    Ok(state_root)
}

fn validate_meeting_read_state_root(state_root: &Path) -> crate::SimardResult<()> {
    validate_existing_read_state_root_root("meeting read", state_root)?;
    require_existing_read_file_for_mode(
        "meeting read",
        state_root,
        &state_root.join("memory_records.json"),
    )?;
    Ok(())
}

fn validate_engineer_read_state_root(state_root: &Path) -> crate::SimardResult<()> {
    validated_engineer_read_artifacts(state_root)?;
    Ok(())
}

fn validate_terminal_read_state_root(state_root: &Path) -> crate::SimardResult<()> {
    validated_terminal_read_artifacts(state_root)?;
    Ok(())
}

fn validate_improvement_curation_read_state_root(state_root: &Path) -> crate::SimardResult<()> {
    validate_existing_read_state_root_root("improvement-curation read", state_root)?;

    require_existing_read_directory_for_mode(
        "improvement-curation read",
        state_root,
        &review_artifacts_dir(state_root),
        "review-artifacts/",
    )?;
    require_existing_read_file_for_mode(
        "improvement-curation read",
        state_root,
        &state_root.join("memory_records.json"),
    )?;
    require_existing_read_file_for_mode(
        "improvement-curation read",
        state_root,
        &state_root.join("goal_records.json"),
    )?;
    Ok(())
}

fn validate_existing_read_state_root_root(
    mode_label: &str,
    state_root: &Path,
) -> crate::SimardResult<()> {
    let root_metadata =
        fs::symlink_metadata(state_root).map_err(|error| crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!("{mode_label} requires an existing state root directory: {error}"),
        })?;
    if root_metadata.file_type().is_symlink() {
        return Err(crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!("{mode_label} requires state-root to be a directory, not a symlink"),
        });
    }
    if root_metadata.is_dir() {
        return Ok(());
    }

    Err(crate::SimardError::InvalidStateRoot {
        path: state_root.to_path_buf(),
        reason: format!("{mode_label} requires state-root to resolve to a directory"),
    })
}

fn require_existing_read_directory_for_mode(
    mode_label: &str,
    state_root: &Path,
    path: &Path,
    label: &str,
) -> crate::SimardResult<()> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!("{mode_label} requires {label} to exist as a directory: {error}"),
        })?;
    if metadata.file_type().is_symlink() {
        return Err(crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!("{mode_label} requires {label} to exist as a directory, not a symlink"),
        });
    }
    if metadata.is_dir() {
        return Ok(());
    }

    Err(crate::SimardError::InvalidStateRoot {
        path: state_root.to_path_buf(),
        reason: format!("{mode_label} requires {label} to exist as a directory"),
    })
}

fn require_existing_read_file_for_mode(
    mode_label: &str,
    state_root: &Path,
    path: &Path,
) -> crate::SimardResult<PathBuf> {
    let file_name = artifact_name(path);
    let metadata =
        fs::symlink_metadata(path).map_err(|error| crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!(
                "{mode_label} requires {file_name} to exist as a regular file: {error}"
            ),
        })?;
    if metadata.file_type().is_symlink() {
        return Err(crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!(
                "{mode_label} requires {file_name} to exist as a regular file, not a symlink"
            ),
        });
    }
    if metadata.is_file() {
        return Ok(path.to_path_buf());
    }

    Err(crate::SimardError::InvalidStateRoot {
        path: state_root.to_path_buf(),
        reason: format!("{mode_label} requires {file_name} to exist as a regular file"),
    })
}

fn artifact_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file")
}

struct EngineerReadArtifacts {
    handoff_path: PathBuf,
    handoff_file_name: String,
    memory_path: PathBuf,
    evidence_path: PathBuf,
}

fn validated_engineer_read_artifacts(
    state_root: &Path,
) -> crate::SimardResult<EngineerReadArtifacts> {
    validate_existing_read_state_root_root("engineer read", state_root)?;
    let selected_handoff =
        select_handoff_artifact_for_read(state_root, ScopedHandoffMode::Engineer, "engineer read")?;
    Ok(EngineerReadArtifacts {
        handoff_path: selected_handoff.path,
        handoff_file_name: selected_handoff.file_name.to_string(),
        memory_path: require_existing_read_file_for_mode(
            "engineer read",
            state_root,
            &state_root.join("memory_records.json"),
        )?,
        evidence_path: require_existing_read_file_for_mode(
            "engineer read",
            state_root,
            &state_root.join("evidence_records.json"),
        )?,
    })
}

fn validated_terminal_read_artifacts(
    state_root: &Path,
) -> crate::SimardResult<EngineerReadArtifacts> {
    validate_existing_read_state_root_root("terminal read", state_root)?;
    let selected_handoff =
        select_handoff_artifact_for_read(state_root, ScopedHandoffMode::Terminal, "terminal read")?;
    Ok(EngineerReadArtifacts {
        handoff_path: selected_handoff.path,
        handoff_file_name: selected_handoff.file_name.to_string(),
        memory_path: require_existing_read_file_for_mode(
            "terminal read",
            state_root,
            &state_root.join("memory_records.json"),
        )?,
        evidence_path: require_existing_read_file_for_mode(
            "terminal read",
            state_root,
            &state_root.join("evidence_records.json"),
        )?,
    })
}

fn parse_runtime_topology(value: &str) -> crate::SimardResult<RuntimeTopology> {
    match value {
        "single-process" => Ok(RuntimeTopology::SingleProcess),
        "multi-process" => Ok(RuntimeTopology::MultiProcess),
        "distributed" => Ok(RuntimeTopology::Distributed),
        other => Err(crate::SimardError::InvalidConfigValue {
            key: "SIMARD_RUNTIME_TOPOLOGY".to_string(),
            value: other.to_string(),
            help: "expected 'single-process', 'multi-process', or 'distributed'".to_string(),
        }),
    }
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

struct TerminalReadView {
    state_root: PathBuf,
    handoff_source: String,
    identity: String,
    selected_base_type: String,
    topology: String,
    session_phase: String,
    objective_metadata: String,
    adapter_implementation: String,
    shell: String,
    working_directory: String,
    command_count: String,
    wait_count: String,
    wait_timeout_seconds: String,
    step_count: usize,
    steps: Vec<String>,
    checkpoints: Vec<String>,
    last_output_line: Option<String>,
    transcript_preview: String,
    continuity_source: Option<String>,
    copilot_submit_audit: Option<CopilotSubmitAudit>,
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
                    )
                    .to_string(),
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

impl TerminalReadView {
    fn load(state_root: PathBuf) -> crate::SimardResult<Self> {
        let artifacts = validated_terminal_read_artifacts(&state_root)?;
        let handoff = load_runtime_handoff_snapshot(
            &crate::terminal_engineer_bridge::SelectedHandoffArtifact {
                path: artifacts.handoff_path.clone(),
                file_name: match artifacts.handoff_file_name.as_str() {
                    TERMINAL_HANDOFF_FILE_NAME => TERMINAL_HANDOFF_FILE_NAME,
                    _ => crate::terminal_engineer_bridge::COMPATIBILITY_HANDOFF_FILE_NAME,
                },
            },
            "terminal read",
        )?;

        FileBackedMemoryStore::try_new(artifacts.memory_path)?;
        FileBackedEvidenceStore::try_new(artifacts.evidence_path)?;

        Self::from_handoff(state_root, handoff, artifacts.handoff_file_name, None)
    }

    fn from_handoff(
        state_root: PathBuf,
        handoff: RuntimeHandoffSnapshot,
        handoff_source: String,
        continuity_source: Option<String>,
    ) -> crate::SimardResult<Self> {
        let handoff_source_label = handoff_source.clone();
        let session = handoff.session.as_ref().ok_or_else(|| {
            crate::SimardError::InvalidHandoffSnapshot {
                field: "session".to_string(),
                reason: format!(
                    "terminal read requires {handoff_source} to contain a persisted session snapshot"
                )
                    .to_string(),
            }
        })?;

        Ok(Self {
            state_root,
            handoff_source,
            identity: handoff.identity_name,
            selected_base_type: handoff.selected_base_type.to_string(),
            topology: handoff.topology.to_string(),
            session_phase: session.phase.to_string(),
            objective_metadata: render_redacted_objective_metadata(&session.objective)?,
            adapter_implementation: required_terminal_evidence_value(
                &handoff.evidence_records,
                "backend-implementation=",
                &handoff_source_label,
            )?
            .to_string(),
            shell: required_terminal_evidence_value(
                &handoff.evidence_records,
                "shell=",
                &handoff_source_label,
            )?
            .to_string(),
            working_directory: required_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-working-directory=",
                &handoff_source_label,
            )?
            .to_string(),
            command_count: required_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-command-count=",
                &handoff_source_label,
            )?
            .to_string(),
            wait_count: optional_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-wait-count=",
            )
            .unwrap_or("0")
            .to_string(),
            wait_timeout_seconds: optional_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-wait-timeout-seconds=",
            )
            .unwrap_or("5")
            .to_string(),
            step_count: terminal_evidence_values(&handoff.evidence_records, "terminal-step-").len(),
            steps: terminal_evidence_values(&handoff.evidence_records, "terminal-step-"),
            checkpoints: terminal_evidence_values(
                &handoff.evidence_records,
                "terminal-checkpoint-",
            ),
            last_output_line: optional_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-last-output-line=",
            )
            .map(ToOwned::to_owned),
            transcript_preview: required_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-transcript-preview=",
                &handoff_source_label,
            )?
            .to_string(),
            continuity_source,
            copilot_submit_audit: handoff.copilot_submit_audit,
            memory_record_count: handoff.memory_records.len(),
            evidence_record_count: handoff.evidence_records.len(),
        })
    }

    fn print(&self) {
        println!("Probe mode: terminal-read");
        self.print_terminal_audit_header();
        self.print_terminal_audit_body();
    }

    fn print_terminal_run(
        &self,
        adapter_capabilities: &[String],
        execution_summary: &str,
        reflection_summary: &str,
    ) {
        println!("Probe mode: terminal-run");
        self.print_terminal_audit_header();
        print_text("Adapter capabilities", adapter_capabilities.join(", "));
        self.print_terminal_audit_body();
        print_text("Execution summary", execution_summary);
        print_text("Reflection summary", reflection_summary);
    }

    fn print_terminal_audit_header(&self) {
        print_text("Terminal handoff source", &self.handoff_source);
        print_text("Mode boundary", TERMINAL_MODE_BOUNDARY);
        print_text("Identity", &self.identity);
        print_text("Selected base type", &self.selected_base_type);
        print_text("Topology", &self.topology);
        print_display("State root", self.state_root.display());
        print_text("Session phase", &self.session_phase);
        print_text("Objective metadata", &self.objective_metadata);
        print_text("Adapter implementation", &self.adapter_implementation);
    }

    fn print_terminal_audit_body(&self) {
        print_text("Shell", &self.shell);
        print_text("Working directory", &self.working_directory);
        print_text("Terminal command count", &self.command_count);
        print_text("Terminal wait count", &self.wait_count);
        print_text("Terminal wait timeout seconds", &self.wait_timeout_seconds);
        println!("Terminal steps count: {}", self.step_count);
        if self.steps.is_empty() {
            println!("Terminal steps: <none>");
        } else {
            for (index, step) in self.steps.iter().enumerate() {
                print_text(&format!("Terminal step {}", index + 1), step);
            }
        }
        println!("Terminal checkpoints count: {}", self.checkpoints.len());
        if self.checkpoints.is_empty() {
            println!("Terminal checkpoints: <none>");
        } else {
            for (index, checkpoint) in self.checkpoints.iter().enumerate() {
                print_text(&format!("Terminal checkpoint {}", index + 1), checkpoint);
            }
        }
        if let Some(last_output_line) = &self.last_output_line {
            print_text("Terminal last output line", last_output_line);
        }
        print_text("Terminal transcript preview", &self.transcript_preview);
        if let Some(audit) = &self.copilot_submit_audit {
            print_text("Copilot flow asset", &audit.flow_asset);
            print_text("Copilot submit outcome", &audit.outcome);
            if let Some(reason_code) = &audit.reason_code {
                print_text("Copilot reason code", reason_code);
            }
            print_text("Copilot payload id", &audit.payload_id);
        }
        if let Some(continuity_source) = &self.continuity_source {
            print_text("Next step source", continuity_source);
        }
        println!("Next steps count: 1");
        println!(
            "Next step 1: run 'simard engineer run <topology> <workspace-root> <objective> {}' to start the separate repo-grounded bounded loop",
            self.state_root.display()
        );
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

fn required_terminal_evidence_value<'a>(
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
                "terminal read requires {handoff_source} to carry persisted terminal evidence '{}' for operator output",
                prefix.trim_end_matches('=')
            ),
        })
}

fn optional_terminal_evidence_value<'a>(
    evidence_records: &'a [EvidenceRecord],
    prefix: &str,
) -> Option<&'a str> {
    evidence_records
        .iter()
        .rev()
        .find_map(|record| record.detail.strip_prefix(prefix))
}

fn terminal_evidence_values(evidence_records: &[EvidenceRecord], prefix: &str) -> Vec<String> {
    evidence_records
        .iter()
        .filter_map(|record| record.detail.split_once('='))
        .filter(|(label, _)| {
            label.starts_with(prefix)
                && label[prefix.len()..]
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_digit())
        })
        .map(|(_, value)| value.to_string())
        .collect()
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

fn render_redacted_objective_metadata(value: &str) -> crate::SimardResult<String> {
    crate::sanitization::normalize_objective_metadata(value).ok_or_else(|| {
        crate::SimardError::InvalidHandoffSnapshot {
            field: "session.objective".to_string(),
            reason: "engineer read requires a trusted handoff artifact to persist objective metadata as objective-metadata(chars=<n>, words=<n>, lines=<n>)".to_string(),
        }
    })
}

fn print_terminal_bridge_section(
    terminal_bridge_context: Option<&TerminalBridgeContext>,
    default_source: &str,
) {
    match terminal_bridge_context {
        Some(context) => {
            print_text("Mode boundary", TERMINAL_MODE_BOUNDARY);
            print_text("Terminal continuity available", "yes");
            print_text("Terminal continuity source", &context.continuity_source);
            print_text("Terminal continuity handoff", &context.handoff_file_name);
            print_text(
                "Terminal continuity working directory",
                &context.working_directory,
            );
            print_text("Terminal continuity command count", &context.command_count);
            print_text("Terminal continuity wait count", &context.wait_count);
            if let Some(last_output_line) = &context.last_output_line {
                print_text("Terminal continuity last output line", last_output_line);
            } else {
                print_text("Terminal continuity last output line", "<none>");
            }
        }
        None => {
            print_text("Terminal continuity available", "no");
            print_text("Terminal continuity source", default_source);
        }
    }
}

fn print_copilot_submit_report(
    state_root: &Path,
    topology: &str,
    report: &crate::copilot_task_submit::CopilotSubmitReport,
    json_output: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }

    println!("Probe mode: copilot-submit");
    print_text("Selected base type", &report.selected_base_type);
    print_text("Topology", topology);
    print_display("State root", state_root.display());
    print_text("Copilot flow asset", &report.flow_asset);
    print_text("Copilot submit outcome", report.outcome.as_str());
    if let Some(reason_code) = &report.reason_code {
        print_text("Copilot reason code", reason_code);
    }
    print_text("Copilot payload id", &report.payload_id);
    println!(
        "Copilot ordered steps count: {}",
        report.ordered_steps.len()
    );
    for (index, step) in report.ordered_steps.iter().enumerate() {
        print_text(&format!("Copilot step {}", index + 1), step);
    }
    println!(
        "Copilot observed checkpoints count: {}",
        report.observed_checkpoints.len()
    );
    for (index, checkpoint) in report.observed_checkpoints.iter().enumerate() {
        print_text(
            &format!("Copilot observed checkpoint {}", index + 1),
            checkpoint,
        );
    }
    if let Some(last_output_line) = &report.last_meaningful_output_line {
        print_text("Terminal last output line", last_output_line);
    }
    print_text("Terminal transcript preview", &report.transcript_preview);
    Ok(())
}

fn next_required(
    args: &mut impl Iterator<Item = String>,
    label: &'static str,
) -> Result<String, Box<dyn std::error::Error>> {
    args.next()
        .ok_or_else(|| format!("expected {label}").into())
}

fn next_optional_path(args: &mut impl Iterator<Item = String>) -> Option<PathBuf> {
    args.next().map(PathBuf::from)
}

fn load_terminal_objective_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "terminal objective file '{}' could not be inspected: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "terminal objective file '{}' must be a regular file, not a symlink",
            path.display()
        )
        .into());
    }
    if !metadata.is_file() {
        return Err(format!(
            "terminal objective file '{}' must be a regular file",
            path.display()
        )
        .into());
    }

    fs::read_to_string(path).map_err(|error| {
        format!(
            "terminal objective file '{}' could not be read as UTF-8 text: {error}",
            path.display()
        )
        .into()
    })
}

fn reject_extra_args(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(extra) = args.next() {
        let mut extras = vec![extra];
        extras.extend(args);
        return Err(format!("unexpected trailing arguments: {}", extras.join(" ")).into());
    }
    Ok(())
}

fn print_text(label: &str, value: impl AsRef<str>) {
    println!("{label}: {}", sanitize_terminal_text(value.as_ref()));
}

fn print_display(label: &str, value: impl std::fmt::Display) {
    println!("{label}: {}", sanitize_terminal_text(&value.to_string()));
}

fn print_string_section(label: &str, values: &[String]) {
    println!("{label} count: {}", values.len());
    if values.is_empty() {
        println!("{label}: <none>");
        return;
    }

    let singular = label.strip_suffix('s').unwrap_or(label);
    for (index, value) in values.iter().enumerate() {
        print_text(&format!("{singular} {}", index + 1), value);
    }
}

fn print_meeting_goal_section(goals: &[crate::PersistedMeetingGoalUpdate]) {
    println!("Goal updates count: {}", goals.len());
    if goals.is_empty() {
        println!("Goal updates: <none>");
        return;
    }

    for (index, goal) in goals.iter().enumerate() {
        print_text(&format!("Goal update {}", index + 1), goal.concise_label());
    }
}

fn print_goal_section(records: &[GoalRecord], status: GoalStatus, heading: &'static str) {
    let mut matching = records
        .iter()
        .filter(|record| record.status == status)
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then(left.title.cmp(&right.title))
            .then(left.slug.cmp(&right.slug))
    });
    println!("{} goals count: {}", heading, matching.len());
    if matching.is_empty() {
        println!("{} goals: <none>", heading);
        return;
    }

    for (index, goal) in matching.iter().enumerate() {
        print_text(
            &format!("{heading} goal {}", index + 1),
            goal.concise_label(),
        );
    }
}

struct GoalRegisterView {
    sections: [GoalRegisterSection; 4],
}

impl GoalRegisterView {
    fn from_records(records: Vec<GoalRecord>) -> Self {
        let mut active = Vec::new();
        let mut proposed = Vec::new();
        let mut paused = Vec::new();
        let mut completed = Vec::new();

        for record in records {
            match record.status {
                GoalStatus::Active => active.push(record),
                GoalStatus::Proposed => proposed.push(record),
                GoalStatus::Paused => paused.push(record),
                GoalStatus::Completed => completed.push(record),
            }
        }

        Self {
            sections: [
                GoalRegisterSection::new(GoalStatus::Active, active),
                GoalRegisterSection::new(GoalStatus::Proposed, proposed),
                GoalRegisterSection::new(GoalStatus::Paused, paused),
                GoalRegisterSection::new(GoalStatus::Completed, completed),
            ],
        }
    }

    fn print(&self) {
        for section in &self.sections {
            section.print();
        }
    }
}

struct GoalRegisterSection {
    heading: &'static str,
    label: &'static str,
    goals: Vec<GoalRecord>,
}

impl GoalRegisterSection {
    fn new(status: GoalStatus, mut goals: Vec<GoalRecord>) -> Self {
        goals.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then(left.title.cmp(&right.title))
                .then(left.slug.cmp(&right.slug))
        });
        let (heading, label) = match status {
            GoalStatus::Active => ("Active", "Active goals"),
            GoalStatus::Proposed => ("Proposed", "Proposed goals"),
            GoalStatus::Paused => ("Paused", "Paused goals"),
            GoalStatus::Completed => ("Completed", "Completed goals"),
        };

        Self {
            heading,
            label,
            goals,
        }
    }

    fn print(&self) {
        println!("{} count: {}", self.label, self.goals.len());
        if self.goals.is_empty() {
            println!("{}: <none>", self.label);
            return;
        }

        for (index, goal) in self.goals.iter().enumerate() {
            print_text(
                &format!("{} goal {}", self.heading, index + 1),
                goal.concise_label(),
            );
        }
    }
}
