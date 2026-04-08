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

#[cfg(test)]
mod tests {
    use super::*;

    // -- CopilotSubmitOutcome --

    #[test]
    fn outcome_as_str() {
        assert_eq!(CopilotSubmitOutcome::Success.as_str(), "success");
        assert_eq!(CopilotSubmitOutcome::Unsupported.as_str(), "unsupported");
    }

    #[test]
    fn outcome_serializes_to_kebab_case() {
        let json = serde_json::to_string(&CopilotSubmitOutcome::Success).unwrap();
        assert_eq!(json, r#""success""#);
        let json = serde_json::to_string(&CopilotSubmitOutcome::Unsupported).unwrap();
        assert_eq!(json, r#""unsupported""#);
    }

    // -- TranscriptCheckpointScan --

    #[test]
    fn empty_scan_has_no_checkpoints() {
        let scan = TranscriptCheckpointScan::default();
        assert!(!scan.has_banner());
        assert!(!scan.has_guidance());
        assert!(!scan.has_submit_hint());
        assert!(!scan.has_post_submit_checkpoint());
        assert!(!scan.has_visible_startup_evidence());
    }

    #[test]
    fn record_and_query_checkpoints() {
        let mut scan = TranscriptCheckpointScan::default();
        scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "banner line", 0);
        scan.record_checkpoint(PositiveCheckpointKind::Guidance, "guidance line", 1);

        assert!(scan.has_banner());
        assert!(scan.has_guidance());
        assert!(!scan.has_submit_hint());
        assert_eq!(
            scan.checkpoint_index(PositiveCheckpointKind::StartupBanner),
            Some(0)
        );
        assert_eq!(
            scan.checkpoint_index(PositiveCheckpointKind::Guidance),
            Some(1)
        );
    }

    #[test]
    fn has_ordered_startup_sequence() {
        let mut scan = TranscriptCheckpointScan::default();
        scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "banner", 0);
        scan.record_checkpoint(PositiveCheckpointKind::Guidance, "guidance", 5);
        assert!(scan.has_ordered_startup_sequence());
    }

    #[test]
    fn no_ordered_startup_when_guidance_before_banner() {
        let mut scan = TranscriptCheckpointScan::default();
        scan.record_checkpoint(PositiveCheckpointKind::Guidance, "guidance", 0);
        scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "banner", 5);
        assert!(!scan.has_ordered_startup_sequence());
    }

    #[test]
    fn observed_banner_before_guidance_when_banner_only() {
        let mut scan = TranscriptCheckpointScan::default();
        scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "banner", 0);
        // No guidance recorded — should still return true
        assert!(scan.observed_banner_before_guidance());
    }

    #[test]
    fn observed_checkpoints_collects_lines() {
        let mut scan = TranscriptCheckpointScan::default();
        scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "line-a", 0);
        scan.record_checkpoint(PositiveCheckpointKind::SubmitHint, "line-b", 1);
        let lines = scan.observed_checkpoints();
        assert_eq!(lines, vec!["line-a".to_string(), "line-b".to_string()]);
    }

    #[test]
    fn has_visible_startup_evidence_with_trust_prompt() {
        let scan = TranscriptCheckpointScan {
            has_trust_prompt: true,
            ..Default::default()
        };
        assert!(scan.has_visible_startup_evidence());
    }

    // -- StartupStatus / SubmitStatus equality --

    #[test]
    fn startup_status_equality() {
        assert_eq!(StartupStatus::Ready, StartupStatus::Ready);
        assert_eq!(StartupStatus::Wait, StartupStatus::Wait);
        assert_ne!(StartupStatus::Ready, StartupStatus::Wait);
        assert_eq!(
            StartupStatus::Unsupported("reason"),
            StartupStatus::Unsupported("reason")
        );
    }

    #[test]
    fn submit_status_equality() {
        assert_eq!(SubmitStatus::Success, SubmitStatus::Success);
        assert_eq!(SubmitStatus::Wait, SubmitStatus::Wait);
        assert_ne!(SubmitStatus::Success, SubmitStatus::Wait);
    }

    // -- CopilotSubmitFlowAsset helpers --

    #[test]
    fn flow_asset_wait_timeout() {
        let flow = CopilotSubmitFlowAsset {
            launch_command: "copilot".to_string(),
            working_directory: PathBuf::from("."),
            wait_timeout_seconds: 30,
            startup_banner: "Welcome".to_string(),
            guidance_checkpoint: "Ready".to_string(),
            submit_hint: "Submit".to_string(),
            post_submit_checkpoint: None,
            trust_prompt: None,
            wrapper_error_signal: None,
            workflow_noise_signals: vec![],
            payload_id: "test-payload".to_string(),
            payload: "task data".to_string(),
        };
        assert_eq!(flow.wait_timeout(), Duration::from_secs(30));
        assert_eq!(flow.launch_step(), "launch: copilot");
        assert!(flow.post_submit_step().is_none());
    }

    #[test]
    fn flow_asset_post_submit_step_some() {
        let flow = CopilotSubmitFlowAsset {
            launch_command: "copilot".to_string(),
            working_directory: PathBuf::from("."),
            wait_timeout_seconds: 10,
            startup_banner: "Welcome".to_string(),
            guidance_checkpoint: "Ready".to_string(),
            submit_hint: "Submit".to_string(),
            post_submit_checkpoint: Some("Done!".to_string()),
            trust_prompt: None,
            wrapper_error_signal: None,
            workflow_noise_signals: vec![],
            payload_id: "test".to_string(),
            payload: "data".to_string(),
        };
        assert!(flow.post_submit_step().is_some());
    }

    // -- StartupCheckpointState --

    #[test]
    fn startup_checkpoint_state_variants() {
        let states = [
            StartupCheckpointState::ExpectBanner,
            StartupCheckpointState::ExpectGuidance,
            StartupCheckpointState::Complete,
        ];
        // Ensure all variants exist and are distinct
        assert_ne!(states[0], states[1]);
        assert_ne!(states[1], states[2]);
    }
}
