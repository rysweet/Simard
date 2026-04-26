use super::types::*;
use std::path::PathBuf;
use std::time::Duration;

fn test_flow() -> CopilotSubmitFlowAsset {
    CopilotSubmitFlowAsset {
        launch_command: "copilot".into(),
        working_directory: PathBuf::from("/test"),
        wait_timeout_seconds: 60,
        startup_banner: "Welcome".into(),
        guidance_checkpoint: "Ready".into(),
        submit_hint: "Submit now".into(),
        post_submit_checkpoint: Some("Done".into()),
        trust_prompt: None,
        wrapper_error_signal: None,
        workflow_noise_signals: vec![],
        payload_id: "p1".into(),
        payload: "task".into(),
    }
}

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

#[test]
fn copilot_submit_outcome_as_str() {
    assert_eq!(CopilotSubmitOutcome::Success.as_str(), "success");
    assert_eq!(CopilotSubmitOutcome::Unsupported.as_str(), "unsupported");
}

#[test]
fn copilot_submit_outcome_clone_eq() {
    let a = CopilotSubmitOutcome::Success;
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn flow_wait_timeout() {
    let flow = test_flow();
    assert_eq!(flow.wait_timeout(), Duration::from_secs(60));
}

#[test]
fn flow_launch_step() {
    let flow = test_flow();
    assert!(flow.launch_step().contains("copilot"));
}

#[test]
fn flow_post_submit_step_present() {
    let flow = test_flow();
    assert!(flow.post_submit_step().is_some());
}

#[test]
fn flow_post_submit_step_none() {
    let mut flow = test_flow();
    flow.post_submit_checkpoint = None;
    assert!(flow.post_submit_step().is_none());
}

#[test]
fn startup_status_eq() {
    assert_eq!(StartupStatus::Ready, StartupStatus::Ready);
    assert_eq!(StartupStatus::Wait, StartupStatus::Wait);
    assert_eq!(
        StartupStatus::Unsupported("x"),
        StartupStatus::Unsupported("x")
    );
    assert_ne!(StartupStatus::Ready, StartupStatus::Wait);
}

#[test]
fn submit_status_eq() {
    assert_eq!(SubmitStatus::Success, SubmitStatus::Success);
    assert_eq!(SubmitStatus::Wait, SubmitStatus::Wait);
    assert_ne!(SubmitStatus::Success, SubmitStatus::Wait);
}

#[test]
fn scan_default_is_empty() {
    let scan = TranscriptCheckpointScan::default();
    assert!(!scan.has_banner());
    assert!(!scan.has_guidance());
    assert!(!scan.has_submit_hint());
    assert!(!scan.has_post_submit_checkpoint());
    assert!(!scan.has_ordered_startup_sequence());
    assert!(!scan.has_visible_startup_evidence());
    assert!(scan.observed_checkpoints().is_empty());
}

#[test]
fn scan_record_and_query_checkpoint() {
    let mut scan = TranscriptCheckpointScan::default();
    scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "banner", 0);
    assert!(scan.has_banner());
    assert!(!scan.has_guidance());
    assert_eq!(
        scan.checkpoint_index(PositiveCheckpointKind::StartupBanner),
        Some(0)
    );
}

#[test]
fn scan_ordered_startup_sequence() {
    let mut scan = TranscriptCheckpointScan::default();
    scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "b", 0);
    scan.record_checkpoint(PositiveCheckpointKind::Guidance, "g", 1);
    assert!(scan.has_ordered_startup_sequence());
}

#[test]
fn scan_reversed_startup_not_ordered() {
    let mut scan = TranscriptCheckpointScan::default();
    scan.record_checkpoint(PositiveCheckpointKind::Guidance, "g", 0);
    scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "b", 1);
    assert!(!scan.has_ordered_startup_sequence());
}

#[test]
fn scan_has_visible_startup_evidence_with_trust() {
    let scan = TranscriptCheckpointScan {
        has_trust_prompt: true,
        ..Default::default()
    };
    assert!(scan.has_visible_startup_evidence());
}

#[test]
fn scan_has_visible_startup_evidence_with_other_lines() {
    let scan = TranscriptCheckpointScan {
        has_other_lines: true,
        ..Default::default()
    };
    assert!(scan.has_visible_startup_evidence());
}

#[test]
fn scan_observed_checkpoints_returns_lines() {
    let mut scan = TranscriptCheckpointScan::default();
    scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "hello", 0);
    scan.record_checkpoint(PositiveCheckpointKind::Guidance, "world", 1);
    let lines = scan.observed_checkpoints();
    assert_eq!(lines, vec!["hello", "world"]);
}

#[test]
fn scan_observed_banner_before_guidance_banner_only() {
    let mut scan = TranscriptCheckpointScan::default();
    scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "b", 0);
    assert!(scan.observed_banner_before_guidance());
}

#[test]
fn scan_startup_ordered_steps() {
    let flow = test_flow();
    let mut scan = TranscriptCheckpointScan::default();
    scan.record_checkpoint(PositiveCheckpointKind::StartupBanner, "b", 0);
    scan.record_checkpoint(PositiveCheckpointKind::Guidance, "g", 1);
    let steps = scan.startup_ordered_steps(&flow);
    assert!(steps.len() >= 2);
}

#[test]
fn scan_submit_ordered_steps_with_post_submit() {
    let flow = test_flow();
    let mut scan = TranscriptCheckpointScan::default();
    scan.record_checkpoint(PositiveCheckpointKind::PostSubmitCheckpoint, "done", 5);
    let steps = scan.submit_ordered_steps(&flow);
    assert!(steps.len() >= 5);
}

#[test]
fn scan_submit_ordered_steps_no_post_submit() {
    let flow = test_flow();
    let scan = TranscriptCheckpointScan::default();
    let steps = scan.submit_ordered_steps(&flow);
    assert_eq!(steps.len(), 5);
}
