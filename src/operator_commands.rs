use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::base_types::BaseTypeId;
use crate::bootstrap::validate_state_root;
use crate::copilot_status_probe::{
    CopilotStatusProbeResult, is_copilot_guarded_recipe, probe_local_copilot_status,
};
use crate::copilot_task_submit::{CopilotSubmitRun, run_copilot_submit};
use crate::goals::{GoalRecord, GoalStatus};
use crate::prompt_assets::{FilePromptAssetStore, PromptAsset, PromptAssetRef, PromptAssetStore};
use crate::reflection::ReflectiveRuntime;
use crate::sanitization::sanitize_terminal_text;
use crate::terminal_engineer_bridge::{TERMINAL_MODE_BOUNDARY, TerminalBridgeContext};
use crate::{
    BootstrapConfig, BootstrapInputs, BuiltinIdentityLoader, Freshness, IdentityLoadRequest,
    IdentityLoader, ManifestContract, Provenance, RuntimeTopology,
    assemble_local_runtime_from_handoff, builtin_base_type_registry_for_manifest,
    latest_local_handoff, review_artifacts_dir, run_local_session,
};

// Re-export all public functions from sub-modules so callers don't break.
pub use crate::operator_commands_engineer::{run_engineer_loop_probe, run_engineer_read_probe};
pub use crate::operator_commands_gym::{
    run_gym_compare, run_gym_list, run_gym_scenario, run_gym_suite,
};
pub use crate::operator_commands_meeting::{
    run_goal_curation_probe, run_goal_curation_read_probe, run_improvement_curation_probe,
    run_improvement_curation_read_probe, run_meeting_probe, run_meeting_read_probe,
};
pub use crate::operator_commands_review::{run_review_probe, run_review_read_probe};
pub use crate::operator_commands_terminal::{
    run_terminal_probe, run_terminal_probe_from_file, run_terminal_read_probe,
    run_terminal_recipe_list_probe, run_terminal_recipe_probe, run_terminal_recipe_show_probe,
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

// --- Shared helpers used by sub-modules ---

pub(crate) fn prompt_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")
}

pub(crate) struct EngineerReadArtifacts {
    pub(crate) handoff_path: PathBuf,
    pub(crate) handoff_file_name: String,
    pub(crate) memory_path: PathBuf,
    pub(crate) evidence_path: PathBuf,
}

pub(crate) struct ValidatedRuntimeSegments {
    pub(crate) base_type: BaseTypeId,
    pub(crate) topology: RuntimeTopology,
}

pub(crate) fn validated_runtime_segments(
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

pub(crate) fn resolved_state_root(
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

pub(crate) fn resolved_goal_curation_state_root(
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

pub(crate) fn resolved_review_state_root(
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

pub(crate) fn resolved_improvement_curation_read_state_root(
    explicit: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    let state_root = resolved_review_state_root(explicit, base_type, topology)?;
    validate_improvement_curation_read_state_root(&state_root)?;
    Ok(state_root)
}

pub(crate) fn resolved_engineer_read_state_root(
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

pub(crate) fn resolved_terminal_read_state_root(
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

pub(crate) fn resolved_meeting_read_state_root(
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

pub(crate) fn validate_existing_read_state_root_root(
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

pub(crate) fn require_existing_read_file_for_mode(
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

pub(crate) fn parse_runtime_topology(value: &str) -> crate::SimardResult<RuntimeTopology> {
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

pub(crate) fn print_text(label: &str, value: impl AsRef<str>) {
    println!("{label}: {}", sanitize_terminal_text(value.as_ref()));
}

pub(crate) fn print_display(label: &str, value: impl std::fmt::Display) {
    println!("{label}: {}", sanitize_terminal_text(&value.to_string()));
}

pub(crate) fn print_terminal_bridge_section(
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

pub(crate) fn render_redacted_objective_metadata(value: &str) -> crate::SimardResult<String> {
    crate::sanitization::normalize_objective_metadata(value).ok_or_else(|| {
        crate::SimardError::InvalidHandoffSnapshot {
            field: "session.objective".to_string(),
            reason: "engineer read requires a trusted handoff artifact to persist objective metadata as objective-metadata(chars=<n>, words=<n>, lines=<n>)".to_string(),
        }
    })
}

pub(crate) fn required_terminal_evidence_value<'a>(
    evidence_records: &'a [crate::EvidenceRecord],
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

pub(crate) fn optional_terminal_evidence_value<'a>(
    evidence_records: &'a [crate::EvidenceRecord],
    prefix: &str,
) -> Option<&'a str> {
    evidence_records
        .iter()
        .rev()
        .find_map(|record| record.detail.strip_prefix(prefix))
}

pub(crate) fn terminal_evidence_values(
    evidence_records: &[crate::EvidenceRecord],
    prefix: &str,
) -> Vec<String> {
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

pub(crate) fn load_terminal_objective_file(
    path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
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

pub(crate) fn validated_terminal_read_artifacts(
    state_root: &Path,
) -> crate::SimardResult<EngineerReadArtifacts> {
    validate_existing_read_state_root_root("terminal read", state_root)?;
    let selected_handoff = crate::terminal_engineer_bridge::select_handoff_artifact_for_read(
        state_root,
        crate::terminal_engineer_bridge::ScopedHandoffMode::Terminal,
        "terminal read",
    )?;
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

pub(crate) fn validated_engineer_read_artifacts(
    state_root: &Path,
) -> crate::SimardResult<EngineerReadArtifacts> {
    validate_existing_read_state_root_root("engineer read", state_root)?;
    let selected_handoff = crate::terminal_engineer_bridge::select_handoff_artifact_for_read(
        state_root,
        crate::terminal_engineer_bridge::ScopedHandoffMode::Engineer,
        "engineer read",
    )?;
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

pub(crate) fn print_string_section(label: &str, values: &[String]) {
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

pub(crate) fn print_meeting_goal_section(goals: &[crate::PersistedMeetingGoalUpdate]) {
    println!("Goal updates count: {}", goals.len());
    if goals.is_empty() {
        println!("Goal updates: <none>");
        return;
    }

    for (index, goal) in goals.iter().enumerate() {
        print_text(&format!("Goal update {}", index + 1), goal.concise_label());
    }
}

pub(crate) fn print_goal_section(
    records: &[GoalRecord],
    status: GoalStatus,
    heading: &'static str,
) {
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

pub(crate) struct GoalRegisterView {
    sections: [GoalRegisterSection; 4],
}

impl GoalRegisterView {
    pub(crate) fn from_records(records: Vec<GoalRecord>) -> Self {
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

    pub(crate) fn print(&self) {
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

pub(crate) fn ensure_terminal_recipe_is_runnable(recipe_name: &str) -> crate::SimardResult<()> {
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
pub(crate) struct TerminalRecipeDescriptor {
    pub(crate) name: String,
    pub(crate) reference: PromptAssetRef,
}

pub(crate) fn list_terminal_recipe_descriptors()
-> crate::SimardResult<Vec<TerminalRecipeDescriptor>> {
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

pub(crate) fn load_terminal_recipe(recipe_name: &str) -> crate::SimardResult<PromptAsset> {
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

pub(crate) fn print_terminal_recipe(recipe_name: &str, recipe: &PromptAsset) {
    print_text("Terminal recipe", recipe_name);
    print_display("Recipe asset", recipe.relative_path.display());
    println!("Recipe contents:");
    for line in sanitize_terminal_text(&recipe.contents).lines() {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goals::{GoalRecord, GoalStatus};
    use crate::session::{SessionId, SessionPhase};

    fn s(value: &str) -> String {
        value.to_string()
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| s(v)).collect()
    }

    fn make_evidence(detail: &str) -> crate::EvidenceRecord {
        crate::EvidenceRecord {
            id: s("ev-1"),
            session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
            phase: SessionPhase::Execution,
            detail: s(detail),
            source: crate::evidence::EvidenceSource::Runtime,
        }
    }

    fn make_goal(title: &str, status: GoalStatus, priority: u8) -> GoalRecord {
        GoalRecord {
            slug: title.to_lowercase().replace(' ', "-"),
            title: s(title),
            rationale: s("test rationale"),
            status,
            priority,
            owner_identity: s("test-identity"),
            source_session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
            updated_in: SessionPhase::Execution,
        }
    }

    // --- gym_usage ---

    #[test]
    fn gym_usage_contains_expected_subcommands() {
        let usage = gym_usage();
        assert!(usage.contains("list"), "should mention 'list'");
        assert!(usage.contains("run"), "should mention 'run'");
        assert!(usage.contains("compare"), "should mention 'compare'");
        assert!(usage.contains("run-suite"), "should mention 'run-suite'");
        assert!(usage.contains("simard-gym"), "should mention binary name");
    }

    // --- parse_runtime_topology ---

    #[test]
    fn parse_runtime_topology_single_process() {
        assert_eq!(
            parse_runtime_topology("single-process").unwrap(),
            RuntimeTopology::SingleProcess
        );
    }

    #[test]
    fn parse_runtime_topology_multi_process() {
        assert_eq!(
            parse_runtime_topology("multi-process").unwrap(),
            RuntimeTopology::MultiProcess
        );
    }

    #[test]
    fn parse_runtime_topology_distributed() {
        assert_eq!(
            parse_runtime_topology("distributed").unwrap(),
            RuntimeTopology::Distributed
        );
    }

    #[test]
    fn parse_runtime_topology_invalid_value() {
        let err = parse_runtime_topology("bogus").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("SIMARD_RUNTIME_TOPOLOGY"),
            "error should reference the config key"
        );
        assert!(
            msg.contains("single-process"),
            "error should list valid values"
        );
    }

    // --- next_required / next_optional_path / reject_extra_args ---

    #[test]
    fn next_required_returns_value_when_present() {
        let mut iter = args(&["hello"]).into_iter();
        assert_eq!(next_required(&mut iter, "greeting").unwrap(), "hello");
    }

    #[test]
    fn next_required_errors_when_empty() {
        let mut iter = std::iter::empty::<String>();
        let err = next_required(&mut iter, "widget").unwrap_err();
        assert!(err.to_string().contains("expected widget"));
    }

    #[test]
    fn next_optional_path_returns_some() {
        let mut iter = args(&["/a/b"]).into_iter();
        assert_eq!(next_optional_path(&mut iter), Some(PathBuf::from("/a/b")));
    }

    #[test]
    fn next_optional_path_returns_none_when_empty() {
        let mut iter = std::iter::empty::<String>();
        assert_eq!(next_optional_path(&mut iter), None);
    }

    #[test]
    fn reject_extra_args_ok_when_empty() {
        reject_extra_args(std::iter::empty::<String>()).unwrap();
    }

    #[test]
    fn reject_extra_args_errors_on_trailing() {
        let err = reject_extra_args(args(&["extra1", "extra2"]).into_iter()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("extra1"));
        assert!(msg.contains("extra2"));
    }

    // --- artifact_name ---

    #[test]
    fn artifact_name_extracts_file_name() {
        assert_eq!(artifact_name(Path::new("/foo/bar/baz.json")), "baz.json");
    }

    #[test]
    fn artifact_name_returns_fallback_for_root() {
        assert_eq!(artifact_name(Path::new("/")), "file");
    }

    // --- terminal_recipe_reference ---

    #[test]
    fn terminal_recipe_reference_valid_name() {
        let reference = terminal_recipe_reference("my-recipe-1").unwrap();
        assert_eq!(
            reference.id,
            crate::prompt_assets::PromptAssetId::new("terminal-recipe:my-recipe-1")
        );
        assert_eq!(
            reference.relative_path,
            PathBuf::from("simard/terminal_recipes/my-recipe-1.simard-terminal")
        );
    }

    #[test]
    fn terminal_recipe_reference_rejects_empty() {
        assert!(terminal_recipe_reference("").is_err());
    }

    #[test]
    fn terminal_recipe_reference_rejects_uppercase() {
        assert!(terminal_recipe_reference("MyRecipe").is_err());
    }

    #[test]
    fn terminal_recipe_reference_rejects_spaces() {
        assert!(terminal_recipe_reference("my recipe").is_err());
    }

    // --- dispatch_operator_probe: argument validation ---

    #[test]
    fn dispatch_operator_probe_no_command() {
        let err = dispatch_operator_probe(std::iter::empty::<String>()).unwrap_err();
        assert!(err.to_string().contains("expected a probe command"));
    }

    #[test]
    fn dispatch_operator_probe_unknown_command() {
        let err = dispatch_operator_probe(args(&["nonexistent"])).unwrap_err();
        assert!(err.to_string().contains("unsupported probe command"));
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn dispatch_operator_probe_missing_required_args() {
        // bootstrap-run needs identity, base type, topology, objective
        let err = dispatch_operator_probe(args(&["bootstrap-run"])).unwrap_err();
        assert!(err.to_string().contains("expected identity"));
    }

    // --- dispatch_legacy_gym_cli: argument validation ---

    #[test]
    fn dispatch_legacy_gym_cli_no_command() {
        let err = dispatch_legacy_gym_cli(std::iter::empty::<String>()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("simard-gym"), "should show usage on no args");
    }

    #[test]
    fn dispatch_legacy_gym_cli_unknown_command() {
        let err = dispatch_legacy_gym_cli(args(&["bogus"])).unwrap_err();
        assert!(err.to_string().contains("simard-gym"));
    }

    #[test]
    fn dispatch_legacy_gym_cli_run_missing_scenario_id() {
        let err = dispatch_legacy_gym_cli(args(&["run"])).unwrap_err();
        assert!(err.to_string().contains("expected scenario id"));
    }

    // --- terminal evidence helpers ---

    #[test]
    fn required_terminal_evidence_value_found() {
        let records = vec![
            make_evidence("terminal-cwd=/home/user"),
            make_evidence("terminal-exit-code=0"),
        ];
        let val = required_terminal_evidence_value(&records, "terminal-exit-code=", "test-handoff")
            .unwrap();
        assert_eq!(val, "0");
    }

    #[test]
    fn required_terminal_evidence_value_not_found() {
        let records = vec![make_evidence("terminal-cwd=/home/user")];
        let err = required_terminal_evidence_value(&records, "terminal-exit-code=", "test-handoff")
            .unwrap_err();
        assert!(err.to_string().contains("terminal-exit-code"));
    }

    #[test]
    fn required_terminal_evidence_value_returns_last_match() {
        let records = vec![
            make_evidence("terminal-exit-code=1"),
            make_evidence("terminal-exit-code=0"),
        ];
        let val = required_terminal_evidence_value(&records, "terminal-exit-code=", "test-handoff")
            .unwrap();
        assert_eq!(val, "0", "should return last (most recent) match");
    }

    #[test]
    fn optional_terminal_evidence_value_found() {
        let records = vec![make_evidence("terminal-cwd=/workspace")];
        assert_eq!(
            optional_terminal_evidence_value(&records, "terminal-cwd="),
            Some("/workspace")
        );
    }

    #[test]
    fn optional_terminal_evidence_value_missing() {
        let records = vec![make_evidence("other-key=value")];
        assert_eq!(
            optional_terminal_evidence_value(&records, "terminal-cwd="),
            None
        );
    }

    #[test]
    fn terminal_evidence_values_collects_indexed_entries() {
        let records = vec![
            make_evidence("checkpoint1=first"),
            make_evidence("checkpoint2=second"),
            make_evidence("unrelated=skip"),
        ];
        let values = terminal_evidence_values(&records, "checkpoint");
        assert_eq!(values, vec!["first", "second"]);
    }

    #[test]
    fn terminal_evidence_values_empty_when_no_match() {
        let records = vec![make_evidence("other=value")];
        let values = terminal_evidence_values(&records, "checkpoint");
        assert!(values.is_empty());
    }

    // --- GoalRegisterView::from_records ---

    #[test]
    fn goal_register_view_partitions_by_status() {
        let records = vec![
            make_goal("Alpha", GoalStatus::Active, 1),
            make_goal("Beta", GoalStatus::Proposed, 2),
            make_goal("Gamma", GoalStatus::Paused, 3),
            make_goal("Delta", GoalStatus::Completed, 1),
            make_goal("Epsilon", GoalStatus::Active, 2),
        ];
        let view = GoalRegisterView::from_records(records);

        assert_eq!(view.sections[0].goals.len(), 2, "Active");
        assert_eq!(view.sections[1].goals.len(), 1, "Proposed");
        assert_eq!(view.sections[2].goals.len(), 1, "Paused");
        assert_eq!(view.sections[3].goals.len(), 1, "Completed");

        // Active section should be sorted by priority
        assert_eq!(view.sections[0].goals[0].title, "Alpha");
        assert_eq!(view.sections[0].goals[1].title, "Epsilon");
    }

    #[test]
    fn goal_register_view_empty_input() {
        let view = GoalRegisterView::from_records(vec![]);
        for section in &view.sections {
            assert!(section.goals.is_empty());
        }
    }

    // --- prompt_root ---

    #[test]
    fn prompt_root_ends_with_prompt_assets() {
        let root = prompt_root();
        assert!(
            root.ends_with("prompt_assets"),
            "prompt_root should end with 'prompt_assets', got: {}",
            root.display()
        );
    }

    // --- state_root path construction ---

    #[test]
    fn state_root_builds_expected_path() {
        let base_type = BaseTypeId::new("my-type");
        let path = state_root(
            "my-id",
            &base_type,
            RuntimeTopology::SingleProcess,
            "probe-x",
        );
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("target/operator-probe-state"));
        assert!(path_str.contains("probe-x"));
        assert!(path_str.contains("my-id"));
        assert!(path_str.contains("my-type"));
        assert!(path_str.contains("single-process"));
    }

    // --- render_redacted_objective_metadata ---

    #[test]
    fn render_redacted_objective_metadata_valid() {
        let result =
            render_redacted_objective_metadata("objective-metadata(chars=10, words=2, lines=1)");
        assert!(result.is_ok());
        let rendered = result.unwrap();
        assert!(rendered.contains("chars=10"));
        assert!(rendered.contains("words=2"));
        assert!(rendered.contains("lines=1"));
    }

    #[test]
    fn render_redacted_objective_metadata_invalid() {
        let result = render_redacted_objective_metadata("not a valid metadata string");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("objective"));
    }
}
