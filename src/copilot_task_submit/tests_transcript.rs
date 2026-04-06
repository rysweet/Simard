use std::path::PathBuf;

use super::transcript::{
    classify_startup, classify_startup_timeout, classify_submit, scan_transcript,
    scan_transcript_lines,
};
use super::types::{CopilotSubmitFlowAsset, StartupStatus, SubmitStatus};

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
        wrapper_error_signal: Some("unknown option '--dangerously-skip-permissions'".to_string()),
        workflow_noise_signals: vec![
            "✅ Copied".to_string(),
            "🔐 Set execute permissions".to_string(),
            "💾 Backup created at".to_string(),
            "📋 Found existing settings.json".to_string(),
            "🔒 XPIA security hooks directory found".to_string(),
            "🔒 XPIA security hooks configured".to_string(),
            "✅ Settings updated".to_string(),
            "✓ Rust recipe runner available".to_string(),
            "✓ Disabled GitHub MCP server to save context tokens - using gh CLI instead"
                .to_string(),
            "Using gh CLI with account:".to_string(),
            "To re-enable GitHub MCP, just ask:".to_string(),
            "✓ XPIA security defender ready".to_string(),
            "✓ Staged".to_string(),
            "✓ Registered amplihack as Copilot CLI plugin".to_string(),
            "INFO:amplihack.security.".to_string(),
            "GitHub Copilot v".to_string(),
        ],
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
        classify_startup_timeout(&scan_transcript_lines(
            [
                "💾 Backup created at /tmp/settings.json.backup.123",
                "📋 Found existing settings.json",
                "🔒 XPIA security hooks directory found",
                "🔒 XPIA security hooks configured (3 hooks)",
                "✅ Settings updated (4 hooks configured)",
                "✓ Rust recipe runner available",
                "✓ Disabled GitHub MCP server to save context tokens - using gh CLI instead",
                "Using gh CLI with account: rysweet",
                "To re-enable GitHub MCP, just ask: 'please use the GitHub MCP server'",
                "✓ XPIA security defender ready (/home/azureuser/.amplihack/bin/xpia-defend)",
                "✓ Registered amplihack as Copilot CLI plugin (~/.copilot/installed-plugins/amplihack@local/)",
            ],
            &flow,
        )),
        Some("workflow-wrapper-noise")
    );
    assert_eq!(
        classify_startup_timeout(&scan_transcript_lines(std::iter::empty::<&str>(), &flow)),
        None
    );
}

#[test]
fn scan_transcript_extracts_real_copilot_tui_checkpoints() {
    let flow = flow();
    let transcript = "\
INFO:amplihack.security.xpia_defender:XPIA Defender initialized with security level: SecurityLevel.MEDIUM\n\
✓ Registered amplihack as Copilot CLI plugin (~/.copilot/installed-plugins/amplihack@local/)\n\
itHub Copilot\u{7}╭────────────────╮│  ╰─╯╰─╯  GitHub Copilot v1.0.14-0 ││  █ ▘▝ █  Describe a task to get started. ││  Tip: /experimental Show available experimental features ││  Copilot uses AI, so always check for mistakes. │╰────────────────╯● Loading environment: 2 custom instructions, 1 plugin\n\
❯  Type @ to mention files, # for issues/PRs, / for commands, or ? for shortcuts──────────────── shift+tab switch mode · ctrl+s run command \u{200b} Unlimited reqs.\n";
    let scan = scan_transcript(transcript, &flow);

    assert!(scan.has_banner());
    assert!(scan.has_guidance());
    assert!(scan.has_submit_hint());
    assert!(!scan.has_wrapper_error);
    assert!(!scan.has_trust_prompt);
    assert!(!scan.has_other_lines);
    assert_eq!(
        scan.observed_checkpoints(),
        vec![
            "Describe a task to get started.".to_string(),
            "Type @ to mention files, # for issues/PRs, / for commands, or ? for shortcuts"
                .to_string(),
            "ctrl+s run command".to_string(),
        ]
    );
}
