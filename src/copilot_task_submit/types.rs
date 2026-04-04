use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::prompt_assets::{FilePromptAssetStore, PromptAssetRef, PromptAssetStore};
use crate::terminal_session::{TerminalStep, render_terminal_step};

pub(super) const COPILOT_SUBMIT_ACTION: &str = "copilot-submit";
pub(super) const COPILOT_SUBMIT_BASE_TYPE: &str = "terminal-shell";
pub(super) const COPILOT_SUBMIT_FLOW_ASSET_ID: &str = "copilot-submit-flow";
pub(super) const COPILOT_SUBMIT_FLOW_ASSET_PATH: &str =
    "simard/terminal_recipes/copilot-submit.json";
pub(super) const COPILOT_SUBMIT_ADAPTER_IDENTITY: &str = "terminal-shell::local-pty";
pub(super) const COPILOT_SUBMIT_SHELL_LABEL: &str = "pty-direct-command";
pub(super) const COPILOT_SUBMIT_RUNTIME_NODE: &str = "node-local";
pub(super) const COPILOT_SUBMIT_MEMORY_KEY: &str = "copilot-submit-summary";
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CopilotSubmitOutcome {
    Success,
    Unsupported,
}

impl CopilotSubmitOutcome {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct CopilotSubmitReport {
    pub selected_base_type: String,
    pub flow_asset: String,
    pub outcome: CopilotSubmitOutcome,
    pub reason_code: Option<String>,
    pub payload_id: String,
    pub ordered_steps: Vec<String>,
    pub observed_checkpoints: Vec<String>,
    pub last_meaningful_output_line: Option<String>,
    pub transcript_preview: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CopilotSubmitRun {
    Success(CopilotSubmitReport),
    Unsupported(CopilotSubmitReport),
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub(super) struct CopilotSubmitFlowAsset {
    pub(super) launch_command: String,
    pub(super) working_directory: PathBuf,
    pub(super) wait_timeout_seconds: u64,
    pub(super) startup_banner: String,
    pub(super) guidance_checkpoint: String,
    pub(super) submit_hint: String,
    #[serde(default)]
    pub(super) post_submit_checkpoint: Option<String>,
    #[serde(default)]
    pub(super) trust_prompt: Option<String>,
    #[serde(default)]
    pub(super) wrapper_error_signal: Option<String>,
    #[serde(default)]
    pub(super) workflow_noise_signals: Vec<String>,
    pub(super) payload_id: String,
    pub(super) payload: String,
}

impl CopilotSubmitFlowAsset {
    pub(super) fn load() -> SimardResult<Self> {
        let asset_ref = PromptAssetRef::new(
            COPILOT_SUBMIT_FLOW_ASSET_ID,
            PathBuf::from(COPILOT_SUBMIT_FLOW_ASSET_PATH),
        );
        let asset = FilePromptAssetStore::new(prompt_root()).load(&asset_ref)?;
        serde_json::from_str(&asset.contents).map_err(|error| SimardError::PromptAssetRead {
            path: prompt_root().join(COPILOT_SUBMIT_FLOW_ASSET_PATH),
            reason: format!(
                "failed to deserialize '{}' as a copilot-submit flow contract: {error}",
                COPILOT_SUBMIT_FLOW_ASSET_PATH
            ),
        })
    }

    pub(super) fn wait_timeout(&self) -> Duration {
        Duration::from_secs(self.wait_timeout_seconds)
    }

    pub(super) fn launch_step(&self) -> String {
        format!("launch: {}", self.launch_command)
    }

    pub(super) fn startup_banner_step(&self) -> String {
        render_terminal_step(&TerminalStep::WaitFor(self.startup_banner.clone()))
    }

    pub(super) fn guidance_step(&self) -> String {
        render_terminal_step(&TerminalStep::WaitFor(self.guidance_checkpoint.clone()))
    }

    pub(super) fn payload_step(&self) -> String {
        render_terminal_step(&TerminalStep::Input(self.payload.clone()))
    }

    pub(super) fn submit_hint_step(&self) -> String {
        render_terminal_step(&TerminalStep::WaitFor(self.submit_hint.clone()))
    }

    pub(super) fn post_submit_step(&self) -> Option<String> {
        self.post_submit_checkpoint
            .as_ref()
            .map(|checkpoint| render_terminal_step(&TerminalStep::WaitFor(checkpoint.clone())))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StartupStatus {
    Ready,
    Wait,
    Unsupported(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SubmitStatus {
    Success,
    Wait,
    Unsupported(&'static str),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct TranscriptCheckpointScan {
    pub(super) observed_checkpoints: Vec<ObservedCheckpoint>,
    pub(super) has_trust_prompt: bool,
    pub(super) has_wrapper_error: bool,
    pub(super) has_workflow_noise: bool,
    pub(super) has_other_lines: bool,
    pub(super) has_startup_sequence_drift: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PositiveCheckpointKind {
    StartupBanner,
    Guidance,
    SubmitHint,
    PostSubmitCheckpoint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ObservedCheckpoint {
    pub(super) kind: PositiveCheckpointKind,
    pub(super) line: String,
    pub(super) index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StartupCheckpointState {
    ExpectBanner,
    ExpectGuidance,
    Complete,
}

impl TranscriptCheckpointScan {
    pub(super) fn record_checkpoint(
        &mut self,
        kind: PositiveCheckpointKind,
        line: &str,
        index: usize,
    ) {
        self.observed_checkpoints.push(ObservedCheckpoint {
            kind,
            line: line.to_string(),
            index,
        });
    }

    pub(super) fn has_checkpoint(&self, kind: PositiveCheckpointKind) -> bool {
        self.observed_checkpoints
            .iter()
            .any(|checkpoint| checkpoint.kind == kind)
    }

    pub(super) fn checkpoint_index(&self, kind: PositiveCheckpointKind) -> Option<usize> {
        self.observed_checkpoints
            .iter()
            .find_map(|checkpoint| (checkpoint.kind == kind).then_some(checkpoint.index))
    }

    pub(super) fn has_banner(&self) -> bool {
        self.has_checkpoint(PositiveCheckpointKind::StartupBanner)
    }

    pub(super) fn has_guidance(&self) -> bool {
        self.has_checkpoint(PositiveCheckpointKind::Guidance)
    }

    pub(super) fn has_submit_hint(&self) -> bool {
        self.has_checkpoint(PositiveCheckpointKind::SubmitHint)
    }

    pub(super) fn has_post_submit_checkpoint(&self) -> bool {
        self.has_checkpoint(PositiveCheckpointKind::PostSubmitCheckpoint)
    }

    pub(super) fn has_ordered_startup_sequence(&self) -> bool {
        matches!(
            (
                self.checkpoint_index(PositiveCheckpointKind::StartupBanner),
                self.checkpoint_index(PositiveCheckpointKind::Guidance),
            ),
            (Some(banner_index), Some(guidance_index)) if banner_index < guidance_index
        )
    }

    pub(super) fn has_visible_startup_evidence(&self) -> bool {
        !self.observed_checkpoints.is_empty()
            || self.has_trust_prompt
            || self.has_wrapper_error
            || self.has_other_lines
    }

    pub(super) fn observed_checkpoints(&self) -> Vec<String> {
        self.observed_checkpoints
            .iter()
            .map(|checkpoint| checkpoint.line.clone())
            .collect()
    }

    pub(super) fn observed_banner_before_guidance(&self) -> bool {
        matches!(
            (
                self.checkpoint_index(PositiveCheckpointKind::StartupBanner),
                self.checkpoint_index(PositiveCheckpointKind::Guidance),
            ),
            (Some(banner_index), Some(guidance_index)) if banner_index < guidance_index
        ) || matches!(
            (
                self.checkpoint_index(PositiveCheckpointKind::StartupBanner),
                self.checkpoint_index(PositiveCheckpointKind::Guidance),
            ),
            (Some(_), None)
        )
    }

    pub(super) fn startup_ordered_steps(&self, flow: &CopilotSubmitFlowAsset) -> Vec<String> {
        let mut steps = vec![flow.launch_step(), flow.startup_banner_step()];
        if self.observed_banner_before_guidance() {
            steps.push(flow.guidance_step());
        }
        steps
    }

    pub(super) fn submit_ordered_steps(&self, flow: &CopilotSubmitFlowAsset) -> Vec<String> {
        let mut steps = vec![
            flow.launch_step(),
            flow.startup_banner_step(),
            flow.guidance_step(),
            flow.payload_step(),
            flow.submit_hint_step(),
        ];
        if (self.has_submit_hint() || self.has_post_submit_checkpoint())
            && let Some(post_submit_step) = flow.post_submit_step()
        {
            steps.push(post_submit_step);
        }
        steps
    }
}

fn prompt_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")
}
