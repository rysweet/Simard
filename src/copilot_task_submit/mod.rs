mod orchestration;
mod transcript;
mod types;

use std::path::Path;

pub(crate) use types::{CopilotSubmitOutcome, CopilotSubmitReport, CopilotSubmitRun};

use orchestration::{
    PersistReportInputs, ensure_copilot_submit_is_launchable, finalize_session, observe_startup,
    observe_submit, persist_report,
};
use types::{
    COPILOT_SUBMIT_ACTION, COPILOT_SUBMIT_BASE_TYPE, CopilotSubmitFlowAsset, StartupStatus,
    SubmitStatus,
};

use crate::error::{SimardError, SimardResult};
use crate::runtime::RuntimeTopology;
use crate::terminal_session::{PtyTerminalSession, resolve_working_directory};

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
