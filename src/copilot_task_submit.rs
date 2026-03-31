use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::base_types::BaseTypeId;
use crate::copilot_status_probe::{CopilotStatusProbeResult, probe_local_copilot_status};
use crate::error::{SimardError, SimardResult};
use crate::evidence::{EvidenceRecord, EvidenceSource, EvidenceStore, FileBackedEvidenceStore};
use crate::handoff::{CopilotSubmitAudit, RuntimeHandoffSnapshot};
use crate::identity::OperatingMode;
use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::prompt_assets::{FilePromptAssetStore, PromptAssetRef, PromptAssetStore};
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};
use crate::terminal_engineer_bridge::{ScopedHandoffMode, persist_handoff_artifacts};
use crate::terminal_session::{
    PtyTerminalSession, TerminalSessionCapture, TerminalStep, compact_terminal_evidence_value,
    render_terminal_step, resolve_working_directory, terminal_last_output_line, transcript_preview,
    transcript_visible_content_lines_iter,
};

const COPILOT_SUBMIT_ACTION: &str = "copilot-submit";
const COPILOT_SUBMIT_BASE_TYPE: &str = "terminal-shell";
const COPILOT_SUBMIT_FLOW_ASSET_ID: &str = "copilot-submit-flow";
const COPILOT_SUBMIT_FLOW_ASSET_PATH: &str = "simard/terminal_recipes/copilot-submit.json";
const COPILOT_SUBMIT_ADAPTER_IDENTITY: &str = "terminal-shell::local-pty";
const COPILOT_SUBMIT_SHELL_LABEL: &str = "pty-direct-command";
const COPILOT_SUBMIT_RUNTIME_NODE: &str = "node-local";
const COPILOT_SUBMIT_MEMORY_KEY: &str = "copilot-submit-summary";
const POLL_INTERVAL: Duration = Duration::from_millis(50);

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
struct CopilotSubmitFlowAsset {
    launch_command: String,
    working_directory: PathBuf,
    wait_timeout_seconds: u64,
    startup_banner: String,
    guidance_checkpoint: String,
    submit_hint: String,
    #[serde(default)]
    post_submit_checkpoint: Option<String>,
    #[serde(default)]
    trust_prompt: Option<String>,
    #[serde(default)]
    wrapper_error_signal: Option<String>,
    #[serde(default)]
    workflow_noise_signals: Vec<String>,
    payload_id: String,
    payload: String,
}

impl CopilotSubmitFlowAsset {
    fn load() -> SimardResult<Self> {
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

    fn wait_timeout(&self) -> Duration {
        Duration::from_secs(self.wait_timeout_seconds)
    }

    fn launch_step(&self) -> String {
        format!("launch: {}", self.launch_command)
    }

    fn startup_banner_step(&self) -> String {
        render_terminal_step(&TerminalStep::WaitFor(self.startup_banner.clone()))
    }

    fn guidance_step(&self) -> String {
        render_terminal_step(&TerminalStep::WaitFor(self.guidance_checkpoint.clone()))
    }

    fn payload_step(&self) -> String {
        render_terminal_step(&TerminalStep::Input(self.payload.clone()))
    }

    fn submit_hint_step(&self) -> String {
        render_terminal_step(&TerminalStep::WaitFor(self.submit_hint.clone()))
    }

    fn post_submit_step(&self) -> Option<String> {
        self.post_submit_checkpoint
            .as_ref()
            .map(|checkpoint| render_terminal_step(&TerminalStep::WaitFor(checkpoint.clone())))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupStatus {
    Ready,
    Wait,
    Unsupported(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubmitStatus {
    Success,
    Wait,
    Unsupported(&'static str),
}

pub(crate) fn run_copilot_submit(
    topology: RuntimeTopology,
    state_root: &Path,
) -> SimardResult<CopilotSubmitRun> {
    ensure_copilot_submit_is_launchable()?;
    let flow = CopilotSubmitFlowAsset::load()?;
    let working_directory = resolve_working_directory(
        Some(flow.working_directory.as_path()),
        COPILOT_SUBMIT_BASE_TYPE,
    )?;
    let mut session = PtyTerminalSession::launch_command(
        COPILOT_SUBMIT_BASE_TYPE,
        &flow.launch_command,
        &working_directory,
    )?;

    let startup = observe_startup(&mut session, &flow)?;
    if let StartupStatus::Unsupported(reason_code) = startup.status {
        let capture = finalize_session(session, startup.terminate)?;
        let report = persist_report(PersistReportInputs {
            state_root,
            topology,
            flow: &flow,
            ordered_steps: startup.ordered_steps,
            observed_checkpoints: startup.observed_checkpoints,
            transcript: &capture.transcript,
            outcome: CopilotSubmitOutcome::Unsupported,
            reason_code: Some(reason_code.to_string()),
            working_directory: &working_directory,
        })?;
        return Ok(CopilotSubmitRun::Unsupported(report));
    }

    session.send_input(&flow.payload)?;
    let submit = observe_submit(&mut session, &flow)?;
    let capture = finalize_session(session, submit.terminate)?;
    let outcome = match submit.status {
        SubmitStatus::Success => CopilotSubmitOutcome::Success,
        SubmitStatus::Unsupported(_) => CopilotSubmitOutcome::Unsupported,
        SubmitStatus::Wait => {
            return Err(SimardError::ActionExecutionFailed {
                action: COPILOT_SUBMIT_ACTION.to_string(),
                reason: "runtime-failure: local PTY observation ended before copilot-submit classified the result".to_string(),
            });
        }
    };
    let reason_code = match submit.status {
        SubmitStatus::Unsupported(reason_code) => Some(reason_code.to_string()),
        SubmitStatus::Success | SubmitStatus::Wait => None,
    };
    let report = persist_report(PersistReportInputs {
        state_root,
        topology,
        flow: &flow,
        ordered_steps: submit.ordered_steps,
        observed_checkpoints: submit.observed_checkpoints,
        transcript: &capture.transcript,
        outcome,
        reason_code,
        working_directory: &working_directory,
    })?;

    Ok(match report.outcome {
        CopilotSubmitOutcome::Success => CopilotSubmitRun::Success(report),
        CopilotSubmitOutcome::Unsupported => CopilotSubmitRun::Unsupported(report),
    })
}

struct StartupObservation {
    status: StartupStatus,
    ordered_steps: Vec<String>,
    observed_checkpoints: Vec<String>,
    terminate: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TranscriptCheckpointScan {
    observed_checkpoints: Vec<ObservedCheckpoint>,
    has_trust_prompt: bool,
    has_wrapper_error: bool,
    has_workflow_noise: bool,
    has_other_lines: bool,
    has_startup_sequence_drift: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PositiveCheckpointKind {
    StartupBanner,
    Guidance,
    SubmitHint,
    PostSubmitCheckpoint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservedCheckpoint {
    kind: PositiveCheckpointKind,
    line: String,
    index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupCheckpointState {
    ExpectBanner,
    ExpectGuidance,
    Complete,
}

impl TranscriptCheckpointScan {
    fn record_checkpoint(&mut self, kind: PositiveCheckpointKind, line: &str, index: usize) {
        self.observed_checkpoints.push(ObservedCheckpoint {
            kind,
            line: line.to_string(),
            index,
        });
    }

    fn has_checkpoint(&self, kind: PositiveCheckpointKind) -> bool {
        self.observed_checkpoints
            .iter()
            .any(|checkpoint| checkpoint.kind == kind)
    }

    fn checkpoint_index(&self, kind: PositiveCheckpointKind) -> Option<usize> {
        self.observed_checkpoints
            .iter()
            .find_map(|checkpoint| (checkpoint.kind == kind).then_some(checkpoint.index))
    }

    fn has_banner(&self) -> bool {
        self.has_checkpoint(PositiveCheckpointKind::StartupBanner)
    }

    fn has_guidance(&self) -> bool {
        self.has_checkpoint(PositiveCheckpointKind::Guidance)
    }

    fn has_submit_hint(&self) -> bool {
        self.has_checkpoint(PositiveCheckpointKind::SubmitHint)
    }

    fn has_post_submit_checkpoint(&self) -> bool {
        self.has_checkpoint(PositiveCheckpointKind::PostSubmitCheckpoint)
    }

    fn has_non_startup_checkpoints(&self) -> bool {
        self.has_submit_hint() || self.has_post_submit_checkpoint()
    }

    fn has_ordered_startup_sequence(&self) -> bool {
        matches!(
            (
                self.checkpoint_index(PositiveCheckpointKind::StartupBanner),
                self.checkpoint_index(PositiveCheckpointKind::Guidance),
            ),
            (Some(banner_index), Some(guidance_index)) if banner_index < guidance_index
        )
    }

    fn has_visible_startup_evidence(&self) -> bool {
        !self.observed_checkpoints.is_empty()
            || self.has_trust_prompt
            || self.has_wrapper_error
            || self.has_other_lines
    }

    fn observed_checkpoints(&self) -> Vec<String> {
        self.observed_checkpoints
            .iter()
            .map(|checkpoint| checkpoint.line.clone())
            .collect()
    }

    fn observed_banner_before_guidance(&self) -> bool {
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

    fn startup_ordered_steps(&self, flow: &CopilotSubmitFlowAsset) -> Vec<String> {
        let mut steps = vec![flow.launch_step(), flow.startup_banner_step()];
        if self.observed_banner_before_guidance() {
            steps.push(flow.guidance_step());
        }
        steps
    }

    fn submit_ordered_steps(&self, flow: &CopilotSubmitFlowAsset) -> Vec<String> {
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

fn observe_startup(
    session: &mut PtyTerminalSession,
    flow: &CopilotSubmitFlowAsset,
) -> SimardResult<StartupObservation> {
    let start = Instant::now();
    let timeout = flow.wait_timeout();
    loop {
        let transcript = session.read_transcript()?;
        let scan = scan_transcript_lines(transcript_visible_content_lines_iter(&transcript), flow);
        let exited = session.status()?.is_some();
        let status = classify_startup(&scan, exited);
        let ordered_steps = scan.startup_ordered_steps(flow);
        let observed_checkpoints = scan.observed_checkpoints();
        match status {
            StartupStatus::Ready => {
                return Ok(StartupObservation {
                    status,
                    ordered_steps,
                    observed_checkpoints,
                    terminate: false,
                });
            }
            StartupStatus::Unsupported(reason_code) => {
                return Ok(StartupObservation {
                    status: StartupStatus::Unsupported(reason_code),
                    ordered_steps,
                    observed_checkpoints,
                    terminate: !exited,
                });
            }
            StartupStatus::Wait => {
                if start.elapsed() >= timeout {
                    if let Some(reason_code) = classify_startup_timeout(&scan) {
                        return Ok(StartupObservation {
                            status: StartupStatus::Unsupported(reason_code),
                            ordered_steps,
                            observed_checkpoints,
                            terminate: !exited,
                        });
                    }
                    return Err(SimardError::ActionExecutionFailed {
                        action: COPILOT_SUBMIT_ACTION.to_string(),
                        reason: format!(
                            "runtime-failure: local PTY observation timed out after {}s before copilot-submit reached a classified startup state",
                            flow.wait_timeout_seconds
                        ),
                    });
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

struct SubmitObservation {
    status: SubmitStatus,
    ordered_steps: Vec<String>,
    observed_checkpoints: Vec<String>,
    terminate: bool,
}

fn observe_submit(
    session: &mut PtyTerminalSession,
    flow: &CopilotSubmitFlowAsset,
) -> SimardResult<SubmitObservation> {
    let start = Instant::now();
    let timeout = flow.wait_timeout();
    loop {
        let transcript = session.read_transcript()?;
        let scan = scan_transcript_lines(transcript_visible_content_lines_iter(&transcript), flow);
        let exited = session.status()?.is_some();
        let ordered_steps = scan.submit_ordered_steps(flow);
        let observed_checkpoints = scan.observed_checkpoints();
        match classify_submit(&scan, exited) {
            SubmitStatus::Success => {
                return Ok(SubmitObservation {
                    status: SubmitStatus::Success,
                    ordered_steps,
                    observed_checkpoints,
                    terminate: true,
                });
            }
            SubmitStatus::Unsupported(reason_code) => {
                return Ok(SubmitObservation {
                    status: SubmitStatus::Unsupported(reason_code),
                    ordered_steps,
                    observed_checkpoints,
                    terminate: !exited,
                });
            }
            SubmitStatus::Wait => {
                if start.elapsed() >= timeout {
                    return Ok(SubmitObservation {
                        status: SubmitStatus::Unsupported(classify_submit_timeout(&scan, flow)),
                        ordered_steps,
                        observed_checkpoints,
                        terminate: true,
                    });
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

fn classify_startup(scan: &TranscriptCheckpointScan, exited: bool) -> StartupStatus {
    if scan.has_wrapper_error {
        return StartupStatus::Unsupported("copilot-wrapper-error");
    }

    if scan.has_trust_prompt {
        return StartupStatus::Unsupported("trust-confirmation-required");
    }

    if scan.has_non_startup_checkpoints() {
        return StartupStatus::Unsupported("unexpected-startup-text");
    }

    if scan.has_startup_sequence_drift
        || (scan.has_banner() && scan.has_guidance() && !scan.has_ordered_startup_sequence())
    {
        return StartupStatus::Unsupported("unexpected-startup-text");
    }

    if scan.has_other_lines {
        if scan.has_guidance() && !scan.has_banner() {
            return StartupStatus::Unsupported("missing-startup-banner");
        }
        if scan.has_banner() && !scan.has_guidance() {
            return StartupStatus::Unsupported("missing-guidance-checkpoint");
        }
        return StartupStatus::Unsupported("unexpected-startup-text");
    }

    if scan.has_ordered_startup_sequence() {
        return if exited {
            StartupStatus::Unsupported("process-exited-early")
        } else {
            StartupStatus::Ready
        };
    }

    if exited {
        if scan.has_guidance() && !scan.has_banner() && !scan.has_other_lines {
            return StartupStatus::Unsupported("missing-startup-banner");
        }
        if scan.has_banner() && !scan.has_guidance() && !scan.has_other_lines {
            return StartupStatus::Unsupported("missing-guidance-checkpoint");
        }
        return StartupStatus::Unsupported("process-exited-early");
    }

    StartupStatus::Wait
}

fn classify_startup_timeout(scan: &TranscriptCheckpointScan) -> Option<&'static str> {
    if scan.has_wrapper_error {
        return Some("copilot-wrapper-error");
    }

    if scan.has_trust_prompt {
        return Some("trust-confirmation-required");
    }

    if scan.has_non_startup_checkpoints() {
        return Some("unexpected-startup-text");
    }

    if scan.has_startup_sequence_drift
        || (scan.has_banner() && scan.has_guidance() && !scan.has_ordered_startup_sequence())
    {
        return Some("unexpected-startup-text");
    }

    if scan.has_banner() && !scan.has_guidance() {
        return Some("missing-guidance-checkpoint");
    }

    if scan.has_guidance() && !scan.has_banner() {
        return Some("missing-startup-banner");
    }

    if scan.has_workflow_noise && !scan.has_visible_startup_evidence() {
        return Some("workflow-wrapper-noise");
    }

    if scan.has_visible_startup_evidence() {
        return Some("unexpected-startup-text");
    }

    None
}

fn classify_submit(scan: &TranscriptCheckpointScan, exited: bool) -> SubmitStatus {
    if scan.has_wrapper_error {
        return SubmitStatus::Unsupported("copilot-wrapper-error");
    }

    if scan.has_trust_prompt {
        return SubmitStatus::Unsupported("trust-confirmation-required");
    }

    if scan.has_post_submit_checkpoint() {
        return SubmitStatus::Success;
    }

    if scan.has_submit_hint() && exited {
        return SubmitStatus::Unsupported("submit-hotkey-required");
    }

    if exited {
        return SubmitStatus::Unsupported("missing-post-submit-checkpoint");
    }

    SubmitStatus::Wait
}

fn classify_submit_timeout(
    scan: &TranscriptCheckpointScan,
    flow: &CopilotSubmitFlowAsset,
) -> &'static str {
    if scan.has_wrapper_error {
        return "copilot-wrapper-error";
    }

    if scan.has_trust_prompt {
        return "trust-confirmation-required";
    }

    if scan.has_submit_hint() {
        return "submit-hotkey-required";
    }

    if flow.post_submit_checkpoint.is_some() {
        return "missing-post-submit-checkpoint";
    }

    "submit-flow-unsupported"
}

struct PersistReportInputs<'a> {
    state_root: &'a Path,
    topology: RuntimeTopology,
    flow: &'a CopilotSubmitFlowAsset,
    ordered_steps: Vec<String>,
    observed_checkpoints: Vec<String>,
    transcript: &'a str,
    outcome: CopilotSubmitOutcome,
    reason_code: Option<String>,
    working_directory: &'a Path,
}

fn persist_report(inputs: PersistReportInputs<'_>) -> SimardResult<CopilotSubmitReport> {
    let PersistReportInputs {
        state_root,
        topology,
        flow,
        ordered_steps,
        observed_checkpoints,
        transcript,
        outcome,
        reason_code,
        working_directory,
    } = inputs;
    let session_ids = UuidSessionIdGenerator;
    let mut session = SessionRecord::new(
        OperatingMode::Engineer,
        flow.payload.clone(),
        BaseTypeId::new(COPILOT_SUBMIT_BASE_TYPE),
        &session_ids,
    );
    for phase in [
        SessionPhase::Preparation,
        SessionPhase::Planning,
        SessionPhase::Execution,
        SessionPhase::Reflection,
        SessionPhase::Persistence,
        SessionPhase::Complete,
    ] {
        session.advance(phase)?;
    }

    let last_meaningful_output_line =
        terminal_last_output_line(transcript, &[TerminalStep::Input(flow.payload.clone())]);
    let transcript_preview = transcript_preview(transcript);
    let audit = CopilotSubmitAudit {
        flow_asset: COPILOT_SUBMIT_FLOW_ASSET_PATH.to_string(),
        payload_id: flow.payload_id.clone(),
        outcome: outcome.as_str().to_string(),
        reason_code: reason_code.clone(),
        ordered_steps: ordered_steps.clone(),
        observed_checkpoints: observed_checkpoints.clone(),
        last_meaningful_output_line: last_meaningful_output_line.clone(),
        transcript_preview: transcript_preview.clone(),
    };
    let report = CopilotSubmitReport {
        selected_base_type: COPILOT_SUBMIT_BASE_TYPE.to_string(),
        flow_asset: audit.flow_asset.clone(),
        outcome,
        reason_code,
        payload_id: audit.payload_id.clone(),
        ordered_steps: audit.ordered_steps.clone(),
        observed_checkpoints: audit.observed_checkpoints.clone(),
        last_meaningful_output_line: audit.last_meaningful_output_line.clone(),
        transcript_preview: audit.transcript_preview.clone(),
    };

    let runtime_node = RuntimeNodeId::new(COPILOT_SUBMIT_RUNTIME_NODE);
    let runtime_address = RuntimeAddress::local(&runtime_node);
    let memory_record = MemoryRecord {
        key: format!("{}-{COPILOT_SUBMIT_MEMORY_KEY}", session.id),
        scope: MemoryScope::SessionSummary,
        value: format!(
            "copilot-submit outcome={} payload_id={} reason_code={}",
            report.outcome.as_str(),
            report.payload_id,
            report.reason_code.as_deref().unwrap_or("<none>")
        ),
        session_id: session.id.clone(),
        recorded_in: SessionPhase::Complete,
    };
    session.attach_memory(memory_record.key.clone());

    let evidence_records = build_evidence_records(
        &session,
        flow,
        &ordered_steps,
        &audit,
        working_directory,
        &runtime_address,
    );
    for record in &evidence_records {
        session.attach_evidence(record.id.clone());
    }

    FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?
        .put(memory_record.clone())?;
    let evidence_store =
        FileBackedEvidenceStore::try_new(state_root.join("evidence_records.json"))?;
    for record in &evidence_records {
        evidence_store.record(record.clone())?;
    }

    let snapshot = RuntimeHandoffSnapshot {
        exported_state: RuntimeState::Stopped,
        identity_name: "simard-engineer".to_string(),
        selected_base_type: BaseTypeId::new(COPILOT_SUBMIT_BASE_TYPE),
        topology,
        source_runtime_node: runtime_node,
        source_mailbox_address: runtime_address,
        session: Some(session.redacted_for_handoff()),
        memory_records: vec![memory_record],
        evidence_records: evidence_records.clone(),
        copilot_submit_audit: Some(audit),
    };
    persist_handoff_artifacts(state_root, ScopedHandoffMode::Terminal, &snapshot)?;

    Ok(report)
}

fn build_evidence_records(
    session: &SessionRecord,
    flow: &CopilotSubmitFlowAsset,
    ordered_steps: &[String],
    audit: &CopilotSubmitAudit,
    working_directory: &Path,
    runtime_address: &RuntimeAddress,
) -> Vec<EvidenceRecord> {
    let mut details = vec![
        format!("selected-base-type={COPILOT_SUBMIT_BASE_TYPE}"),
        format!("backend-implementation={COPILOT_SUBMIT_ADAPTER_IDENTITY}"),
        format!("shell={COPILOT_SUBMIT_SHELL_LABEL}"),
        format!(
            "terminal-working-directory={}",
            compact_terminal_evidence_value(&working_directory.display().to_string(), 160)
        ),
        "terminal-command-count=1".to_string(),
        format!(
            "terminal-wait-count={}",
            3 + usize::from(flow.post_submit_checkpoint.is_some())
        ),
        format!(
            "terminal-wait-timeout-seconds={}",
            flow.wait_timeout_seconds
        ),
        format!("terminal-step-count={}", ordered_steps.len()),
        format!("terminal-transcript-preview={}", audit.transcript_preview),
        format!("runtime-node={COPILOT_SUBMIT_RUNTIME_NODE}"),
        format!("mailbox-address={runtime_address}"),
        format!("copilot-flow-asset={}", audit.flow_asset),
        format!("copilot-submit-outcome={}", audit.outcome),
        format!("copilot-payload-id={}", audit.payload_id),
    ];
    if let Some(reason_code) = &audit.reason_code {
        details.push(format!("copilot-reason-code={reason_code}"));
    }
    if let Some(last_meaningful_output_line) = &audit.last_meaningful_output_line {
        details.push(format!(
            "terminal-last-output-line={last_meaningful_output_line}"
        ));
    }
    for (index, step) in ordered_steps.iter().enumerate() {
        details.push(format!(
            "terminal-step-{}={}",
            index + 1,
            compact_terminal_evidence_value(step, 160)
        ));
    }
    for (index, checkpoint) in audit.observed_checkpoints.iter().enumerate() {
        details.push(format!(
            "terminal-checkpoint-{}={}",
            index + 1,
            compact_terminal_evidence_value(checkpoint, 160)
        ));
    }

    details
        .into_iter()
        .enumerate()
        .map(|(index, detail)| EvidenceRecord {
            id: format!("{}-copilot-submit-evidence-{}", session.id, index + 1),
            session_id: session.id.clone(),
            phase: SessionPhase::Complete,
            detail,
            source: EvidenceSource::BaseType(BaseTypeId::new(COPILOT_SUBMIT_BASE_TYPE)),
        })
        .collect()
}

fn scan_transcript_lines<I, S>(lines: I, flow: &CopilotSubmitFlowAsset) -> TranscriptCheckpointScan
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut scan = TranscriptCheckpointScan::default();
    let mut startup_state = StartupCheckpointState::ExpectBanner;
    let mut saw_guidance_before_banner = false;
    for (index, line) in lines.into_iter().enumerate() {
        let line = line.as_ref();
        if line == flow.startup_banner {
            scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, line, index);
            if startup_state != StartupCheckpointState::ExpectBanner {
                scan.has_startup_sequence_drift = true;
            } else if saw_guidance_before_banner {
                scan.has_startup_sequence_drift = true;
                startup_state = StartupCheckpointState::ExpectGuidance;
            } else {
                startup_state = StartupCheckpointState::ExpectGuidance;
            }
        } else if line == flow.guidance_checkpoint {
            scan.record_checkpoint(PositiveCheckpointKind::Guidance, line, index);
            if startup_state == StartupCheckpointState::ExpectBanner {
                saw_guidance_before_banner = true;
            } else if startup_state != StartupCheckpointState::ExpectGuidance {
                scan.has_startup_sequence_drift = true;
            } else {
                startup_state = StartupCheckpointState::Complete;
            }
        } else if line == flow.submit_hint {
            scan.record_checkpoint(PositiveCheckpointKind::SubmitHint, line, index);
        } else if flow
            .post_submit_checkpoint
            .as_ref()
            .is_some_and(|checkpoint| line == checkpoint)
        {
            scan.record_checkpoint(PositiveCheckpointKind::PostSubmitCheckpoint, line, index);
        } else if flow
            .trust_prompt
            .as_ref()
            .is_some_and(|checkpoint| line.contains(checkpoint))
        {
            scan.has_trust_prompt = true;
        } else if flow
            .wrapper_error_signal
            .as_ref()
            .is_some_and(|signal| line.contains(signal))
        {
            scan.has_wrapper_error = true;
        } else if flow
            .workflow_noise_signals
            .iter()
            .any(|signal| line.contains(signal))
        {
            scan.has_workflow_noise = true;
        } else {
            scan.has_other_lines = true;
        }
    }
    scan
}

fn finalize_session(
    mut session: PtyTerminalSession,
    terminate: bool,
) -> SimardResult<TerminalSessionCapture> {
    if terminate {
        session.terminate()?;
    }
    session.finish()
}

fn ensure_copilot_submit_is_launchable() -> SimardResult<()> {
    match probe_local_copilot_status() {
        CopilotStatusProbeResult::Available { .. } => Ok(()),
        CopilotStatusProbeResult::Unavailable {
            reason_code,
            detail,
        }
        | CopilotStatusProbeResult::Unsupported {
            reason_code,
            detail,
        } => Err(SimardError::ActionExecutionFailed {
            action: COPILOT_SUBMIT_ACTION.to_string(),
            reason: format!("runtime-failure: {reason_code}: {detail}"),
        }),
    }
}

fn prompt_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        CopilotSubmitFlowAsset, StartupStatus, SubmitStatus, classify_startup,
        classify_startup_timeout, classify_submit, scan_transcript_lines,
    };

    fn flow() -> CopilotSubmitFlowAsset {
        CopilotSubmitFlowAsset {
            launch_command: "amplihack copilot".to_string(),
            working_directory: PathBuf::from("."),
            wait_timeout_seconds: 5,
            startup_banner: "Describe a task to get started.".to_string(),
            guidance_checkpoint:
                "Type @ to mention files, # for issues/PRs, / for commands, or ? for shortcuts"
                    .to_string(),
            submit_hint: "ctrl+s run command".to_string(),
            post_submit_checkpoint: Some("READY".to_string()),
            trust_prompt: Some("Do you trust the files in this folder?".to_string()),
            wrapper_error_signal: Some(
                "unknown option '--dangerously-skip-permissions'".to_string(),
            ),
            workflow_noise_signals: vec!["✅ Copied".to_string(), "GitHub Copilot v".to_string()],
            payload_id: "simard-local-task-submit-ready-v1".to_string(),
            payload: "fixed payload".to_string(),
        }
    }

    #[test]
    fn classify_startup_uses_explicit_reason_codes() {
        let flow = flow();
        assert!(matches!(
            classify_startup(
                &scan_transcript_lines(
                    [
                        flow.startup_banner.as_str(),
                        flow.guidance_checkpoint.as_str(),
                    ],
                    &flow,
                ),
                false,
            ),
            StartupStatus::Ready
        ));
        assert!(matches!(
            classify_startup(
                &scan_transcript_lines(
                    [
                        flow.guidance_checkpoint.as_str(),
                        flow.startup_banner.as_str(),
                    ],
                    &flow,
                ),
                false,
            ),
            StartupStatus::Unsupported("unexpected-startup-text")
        ));
        assert!(matches!(
            classify_startup(
                &scan_transcript_lines([flow.guidance_checkpoint.as_str()], &flow,),
                true,
            ),
            StartupStatus::Unsupported("missing-startup-banner")
        ));
        assert!(matches!(
            classify_startup(
                &scan_transcript_lines([flow.startup_banner.as_str(), "Still warming up",], &flow,),
                true,
            ),
            StartupStatus::Unsupported("missing-guidance-checkpoint")
        ));
        assert!(matches!(
            classify_startup(
                &scan_transcript_lines(
                    [
                        flow.startup_banner.as_str(),
                        flow.startup_banner.as_str(),
                        flow.guidance_checkpoint.as_str(),
                    ],
                    &flow,
                ),
                false,
            ),
            StartupStatus::Unsupported("unexpected-startup-text")
        ));
    }

    #[test]
    fn classify_submit_requires_post_submit_checkpoint() {
        let flow = flow();
        assert!(matches!(
            classify_submit(
                &scan_transcript_lines(
                    [flow
                        .post_submit_checkpoint
                        .as_deref()
                        .expect("flow should include a post-submit checkpoint")],
                    &flow,
                ),
                true,
            ),
            SubmitStatus::Success
        ));
        assert!(matches!(
            classify_submit(
                &scan_transcript_lines([flow.submit_hint.as_str()], &flow),
                true,
            ),
            SubmitStatus::Unsupported("submit-hotkey-required")
        ));
    }

    #[test]
    fn classify_startup_timeout_preserves_explicit_reason_codes() {
        let flow = flow();

        assert_eq!(
            classify_startup_timeout(&scan_transcript_lines(
                [
                    flow.guidance_checkpoint.as_str(),
                    flow.startup_banner.as_str(),
                ],
                &flow,
            )),
            Some("unexpected-startup-text")
        );
        assert_eq!(
            classify_startup_timeout(&scan_transcript_lines(
                [flow.startup_banner.as_str()],
                &flow,
            )),
            Some("missing-guidance-checkpoint")
        );
        assert_eq!(
            classify_startup_timeout(&scan_transcript_lines(
                [flow.guidance_checkpoint.as_str()],
                &flow,
            )),
            Some("missing-startup-banner")
        );
        assert_eq!(
            classify_startup_timeout(&scan_transcript_lines(["✅ Copied bin"], &flow,)),
            Some("workflow-wrapper-noise")
        );
        assert_eq!(
            classify_startup_timeout(&scan_transcript_lines(std::iter::empty::<&str>(), &flow)),
            None
        );
    }
}
