use std::path::{Path, PathBuf};

use crate::operator_commands::{
    load_terminal_objective_file, optional_terminal_evidence_value, print_display, print_text,
    prompt_root, render_redacted_objective_metadata, required_terminal_evidence_value,
    resolved_state_root, resolved_terminal_read_state_root, terminal_evidence_values,
    validated_terminal_read_artifacts,
};
use crate::terminal_engineer_bridge::{
    SHARED_DEFAULT_STATE_ROOT_SOURCE, SHARED_EXPLICIT_STATE_ROOT_SOURCE, ScopedHandoffMode,
    TERMINAL_HANDOFF_FILE_NAME, TERMINAL_MODE_BOUNDARY, load_runtime_handoff_snapshot,
    persist_handoff_artifacts, scoped_handoff_path,
};
use crate::{
    BootstrapConfig, BootstrapInputs, CopilotSubmitAudit, FileBackedEvidenceStore,
    FileBackedMemoryStore, RuntimeHandoffSnapshot, latest_local_handoff, run_local_session,
};

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
    let recipes = crate::operator_commands::list_terminal_recipe_descriptors()?;
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
    let recipe = crate::operator_commands::load_terminal_recipe(recipe_name)?;
    crate::operator_commands::print_terminal_recipe(recipe_name, &recipe);
    Ok(())
}

pub fn run_terminal_recipe_probe(
    topology: &str,
    recipe_name: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::operator_commands::ensure_terminal_recipe_is_runnable(recipe_name)?;
    let recipe = crate::operator_commands::load_terminal_recipe(recipe_name)?;
    run_terminal_probe(topology, &recipe.contents, state_root_override)
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
