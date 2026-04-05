use std::path::{Path, PathBuf};

use crate::copilot_task_submit::{CopilotSubmitRun, run_copilot_submit};
use crate::reflection::ReflectiveRuntime;
use crate::{
    BootstrapConfig, BootstrapInputs, assemble_local_runtime_from_handoff, latest_local_handoff,
    run_local_session,
};

use super::format::{print_display, print_text};
use super::state_root::{parse_runtime_topology, prompt_root, resolved_state_root};

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
