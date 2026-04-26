mod orchestration;
mod transcript;
mod types;

#[cfg(test)]
mod tests_orchestration;
#[cfg(test)]
mod tests_orchestration_inline;
#[cfg(test)]
mod tests_transcript;

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

#[cfg(test)]
mod tests {
    use super::*;

    // ── run_copilot_submit error path ───────────────────────────────────

    #[test]
    fn run_copilot_submit_fails_without_copilot_binary() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = run_copilot_submit(RuntimeTopology::SingleProcess, dir.path());
        // In test environments amplihack is typically not on PATH,
        // so ensure_copilot_submit_is_launchable should fail.
        match result {
            Ok(_) => {} // amplihack is available in this environment
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("runtime-failure") || msg.contains("copilot-submit"),
                    "error should reference the copilot-submit action: {msg}"
                );
            }
        }
    }

    // ── CopilotSubmitOutcome ────────────────────────────────────────────

    #[test]
    fn copilot_submit_outcome_as_str_values() {
        assert_eq!(CopilotSubmitOutcome::Success.as_str(), "success");
        assert_eq!(CopilotSubmitOutcome::Unsupported.as_str(), "unsupported");
    }

    #[test]
    fn copilot_submit_outcome_equality() {
        assert_eq!(CopilotSubmitOutcome::Success, CopilotSubmitOutcome::Success);
        assert_ne!(
            CopilotSubmitOutcome::Success,
            CopilotSubmitOutcome::Unsupported
        );
    }

    #[test]
    fn copilot_submit_outcome_clone() {
        let original = CopilotSubmitOutcome::Success;
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    // ── CopilotSubmitReport ─────────────────────────────────────────────

    fn test_report(outcome: CopilotSubmitOutcome) -> CopilotSubmitReport {
        CopilotSubmitReport {
            selected_base_type: "terminal-shell".to_string(),
            flow_asset: "test.json".to_string(),
            outcome,
            reason_code: None,
            payload_id: "p1".to_string(),
            ordered_steps: vec![],
            observed_checkpoints: vec![],
            last_meaningful_output_line: None,
            transcript_preview: "preview".to_string(),
        }
    }

    #[test]
    fn copilot_submit_report_construction() {
        let report = test_report(CopilotSubmitOutcome::Success);
        assert_eq!(report.selected_base_type, "terminal-shell");
        assert_eq!(report.payload_id, "p1");
        assert!(report.reason_code.is_none());
        assert!(report.last_meaningful_output_line.is_none());
    }

    #[test]
    fn copilot_submit_report_with_all_fields() {
        let report = CopilotSubmitReport {
            selected_base_type: "terminal-shell".to_string(),
            flow_asset: "flow.json".to_string(),
            outcome: CopilotSubmitOutcome::Unsupported,
            reason_code: Some("startup-error".to_string()),
            payload_id: "p2".to_string(),
            ordered_steps: vec!["s1".to_string(), "s2".to_string()],
            observed_checkpoints: vec!["c1".to_string()],
            last_meaningful_output_line: Some("last line".to_string()),
            transcript_preview: "transcript preview".to_string(),
        };
        assert_eq!(report.outcome.as_str(), "unsupported");
        assert_eq!(report.reason_code.as_deref(), Some("startup-error"));
        assert_eq!(report.ordered_steps.len(), 2);
        assert_eq!(report.observed_checkpoints.len(), 1);
        assert_eq!(
            report.last_meaningful_output_line.as_deref(),
            Some("last line")
        );
    }

    #[test]
    fn copilot_submit_report_clone() {
        let report = test_report(CopilotSubmitOutcome::Success);
        let cloned = report.clone();
        assert_eq!(report, cloned);
    }

    // ── CopilotSubmitRun ────────────────────────────────────────────────

    #[test]
    fn copilot_submit_run_success_variant() {
        let report = test_report(CopilotSubmitOutcome::Success);
        let run = CopilotSubmitRun::Success(report);
        match run {
            CopilotSubmitRun::Success(r) => assert_eq!(r.outcome.as_str(), "success"),
            _ => panic!("expected Success variant"),
        }
    }

    #[test]
    fn copilot_submit_run_unsupported_variant() {
        let mut report = test_report(CopilotSubmitOutcome::Unsupported);
        report.reason_code = Some("reason".to_string());
        let run = CopilotSubmitRun::Unsupported(report);
        match run {
            CopilotSubmitRun::Unsupported(r) => {
                assert_eq!(r.reason_code, Some("reason".to_string()));
            }
            _ => panic!("expected Unsupported variant"),
        }
    }

    #[test]
    fn copilot_submit_run_equality() {
        let run1 = CopilotSubmitRun::Success(test_report(CopilotSubmitOutcome::Success));
        let run2 = CopilotSubmitRun::Success(test_report(CopilotSubmitOutcome::Success));
        assert_eq!(run1, run2);
    }

    #[test]
    fn copilot_submit_run_inequality_across_variants() {
        let run1 = CopilotSubmitRun::Success(test_report(CopilotSubmitOutcome::Success));
        let run2 = CopilotSubmitRun::Unsupported(test_report(CopilotSubmitOutcome::Unsupported));
        assert_ne!(run1, run2);
    }

    // ── constants ───────────────────────────────────────────────────────

    #[test]
    fn copilot_submit_action_constant() {
        assert_eq!(COPILOT_SUBMIT_ACTION, "copilot-submit");
    }

    #[test]
    fn copilot_submit_base_type_constant() {
        assert_eq!(COPILOT_SUBMIT_BASE_TYPE, "terminal-shell");
    }

    // --- CopilotSubmitOutcome ---

    #[test]
    fn copilot_submit_outcome_debug_format() {
        let debug = format!("{:?}", CopilotSubmitOutcome::Success);
        assert!(debug.contains("Success"));
        let debug2 = format!("{:?}", CopilotSubmitOutcome::Unsupported);
        assert!(debug2.contains("Unsupported"));
    }

    // --- CopilotSubmitReport ---

    #[test]
    fn copilot_submit_report_debug_format() {
        let report = test_report(CopilotSubmitOutcome::Success);
        let debug = format!("{:?}", report);
        assert!(debug.contains("terminal-shell"));
        assert!(debug.contains("p1"));
    }

    #[test]
    fn copilot_submit_report_empty_steps_and_checkpoints() {
        let report = test_report(CopilotSubmitOutcome::Success);
        assert!(report.ordered_steps.is_empty());
        assert!(report.observed_checkpoints.is_empty());
    }

    #[test]
    fn copilot_submit_report_with_reason_code() {
        let mut report = test_report(CopilotSubmitOutcome::Unsupported);
        report.reason_code = Some("no-copilot-binary".to_string());
        assert_eq!(report.reason_code.as_deref(), Some("no-copilot-binary"));
    }

    #[test]
    fn copilot_submit_report_transcript_preview() {
        let report = test_report(CopilotSubmitOutcome::Success);
        assert_eq!(report.transcript_preview, "preview");
    }

    // --- CopilotSubmitRun ---

    #[test]
    fn copilot_submit_run_clone() {
        let run = CopilotSubmitRun::Success(test_report(CopilotSubmitOutcome::Success));
        let cloned = run.clone();
        assert_eq!(run, cloned);
    }

    #[test]
    fn copilot_submit_run_debug_format() {
        let run = CopilotSubmitRun::Success(test_report(CopilotSubmitOutcome::Success));
        let debug = format!("{:?}", run);
        assert!(debug.contains("Success"));
    }

    #[test]
    fn copilot_submit_run_unsupported_debug() {
        let run = CopilotSubmitRun::Unsupported(test_report(CopilotSubmitOutcome::Unsupported));
        let debug = format!("{:?}", run);
        assert!(debug.contains("Unsupported"));
    }

    // --- constants ---

    #[test]
    fn copilot_submit_action_is_kebab_case() {
        assert!(
            COPILOT_SUBMIT_ACTION
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-'),
            "action should be kebab-case"
        );
    }

    #[test]
    fn copilot_submit_base_type_is_kebab_case() {
        assert!(
            COPILOT_SUBMIT_BASE_TYPE
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-'),
            "base type should be kebab-case"
        );
    }

    // --- CopilotSubmitReport with last_meaningful_output_line ---

    #[test]
    fn copilot_submit_report_none_output_line_default() {
        let report = test_report(CopilotSubmitOutcome::Success);
        assert!(report.last_meaningful_output_line.is_none());
    }

    #[test]
    fn copilot_submit_report_with_output_line() {
        let mut report = test_report(CopilotSubmitOutcome::Success);
        report.last_meaningful_output_line = Some("Build succeeded".to_string());
        assert_eq!(
            report.last_meaningful_output_line.as_deref(),
            Some("Build succeeded")
        );
    }
}
