use super::types::{
    CopilotSubmitFlowAsset, PositiveCheckpointKind, StartupCheckpointState, StartupStatus,
    SubmitStatus, TranscriptCheckpointScan,
};
use crate::sanitization::sanitize_terminal_text;
use crate::terminal_session::compact_terminal_evidence_value;

pub(super) fn classify_startup(scan: &TranscriptCheckpointScan, exited: bool) -> StartupStatus {
    if scan.has_wrapper_error {
        return StartupStatus::Unsupported("copilot-wrapper-error");
    }

    if scan.has_trust_prompt {
        return StartupStatus::Unsupported("trust-confirmation-required");
    }

    if scan.has_post_submit_checkpoint() {
        return StartupStatus::Unsupported("unexpected-startup-text");
    }

    if scan.has_startup_sequence_drift
        || (scan.has_banner() && scan.has_guidance() && !scan.has_ordered_startup_sequence())
    {
        return StartupStatus::Unsupported("unexpected-startup-text");
    }

    if scan.has_ordered_startup_sequence() {
        return if exited {
            StartupStatus::Unsupported("process-exited-early")
        } else {
            StartupStatus::Ready
        };
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

pub(super) fn classify_startup_timeout(scan: &TranscriptCheckpointScan) -> Option<&'static str> {
    if scan.has_wrapper_error {
        return Some("copilot-wrapper-error");
    }

    if scan.has_trust_prompt {
        return Some("trust-confirmation-required");
    }

    if scan.has_post_submit_checkpoint() {
        return Some("unexpected-startup-text");
    }

    if scan.has_startup_sequence_drift
        || (scan.has_banner() && scan.has_guidance() && !scan.has_ordered_startup_sequence())
    {
        return Some("unexpected-startup-text");
    }

    if scan.has_ordered_startup_sequence() {
        return None;
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

pub(super) fn classify_submit(scan: &TranscriptCheckpointScan, exited: bool) -> SubmitStatus {
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

pub(super) fn classify_submit_timeout(
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

pub(super) fn scan_transcript_lines<I, S>(
    lines: I,
    flow: &CopilotSubmitFlowAsset,
) -> TranscriptCheckpointScan
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

pub(super) fn scan_transcript(
    transcript: &str,
    flow: &CopilotSubmitFlowAsset,
) -> TranscriptCheckpointScan {
    scan_transcript_lines(copilot_visible_fragments(transcript, flow), flow)
}

pub(super) fn copilot_visible_fragments(
    transcript: &str,
    flow: &CopilotSubmitFlowAsset,
) -> Vec<String> {
    transcript
        .lines()
        .filter_map(|line| {
            let sanitized = sanitize_terminal_text(line);
            let trimmed = sanitized.trim();
            (!trimmed.is_empty()
                && !trimmed.starts_with("Script started on ")
                && !trimmed.starts_with("Script done on "))
            .then_some(sanitized)
        })
        .flat_map(|line| split_visible_fragment_candidates(&line))
        .filter_map(|fragment| canonicalize_visible_fragment(&fragment, flow))
        .collect()
}

fn split_visible_fragment_candidates(line: &str) -> Vec<String> {
    let mut normalized = String::with_capacity(line.len());
    for ch in line.chars() {
        if matches!(
            ch,
            '╭' | '╮'
                | '╰'
                | '╯'
                | '│'
                | '─'
                | '❯'
                | '●'
                | '◉'
                | '◎'
                | '○'
                | '·'
                | '█'
                | '▘'
                | '▝'
                | '▔'
        ) {
            normalized.push('\n');
        } else if ch == '\u{200b}' {
            normalized.push(' ');
        } else {
            normalized.push(ch);
        }
    }

    normalized
        .lines()
        .map(|segment| segment.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn canonicalize_visible_fragment(fragment: &str, flow: &CopilotSubmitFlowAsset) -> Option<String> {
    if fragment == flow.startup_banner {
        return Some(flow.startup_banner.clone());
    }
    if fragment == flow.guidance_checkpoint {
        return Some(flow.guidance_checkpoint.clone());
    }
    if fragment.contains(&flow.submit_hint) {
        return Some(flow.submit_hint.clone());
    }
    if flow
        .post_submit_checkpoint
        .as_ref()
        .is_some_and(|checkpoint| fragment == checkpoint)
    {
        return flow.post_submit_checkpoint.clone();
    }
    if flow
        .trust_prompt
        .as_ref()
        .is_some_and(|prompt| fragment == prompt)
    {
        return flow.trust_prompt.clone();
    }
    if flow
        .wrapper_error_signal
        .as_ref()
        .is_some_and(|signal| fragment == signal)
    {
        return flow.wrapper_error_signal.clone();
    }
    if is_ignorable_copilot_ui_fragment(fragment) {
        return None;
    }
    Some(fragment.to_string())
}

pub(super) fn is_ignorable_copilot_ui_fragment(fragment: &str) -> bool {
    fragment.eq("GitHub Copilot")
        || fragment.starts_with("GitHub Copilot v")
        || fragment.starts_with("itHub Copilot")
        || fragment.starts_with("Tip:")
        || fragment == "Copilot uses AI, so always check for mistakes."
        || fragment.contains("Loading environment:")
        || fragment.contains("Environment loaded:")
        || fragment.contains("Remote session disabled:")
        || fragment.contains("switch mode")
        || fragment.contains("Unlimited reqs.")
        || fragment == "GPT-5.4"
        || fragment == "GPT-5.4 (high)"
        || fragment.contains("[⎇ ")
}

pub(super) fn copilot_last_meaningful_output_line(
    visible_fragments: &[String],
    flow: &CopilotSubmitFlowAsset,
) -> Option<String> {
    visible_fragments
        .iter()
        .rev()
        .find(|fragment| !fragment.contains(&flow.payload))
        .map(|fragment| compact_terminal_evidence_value(fragment, 160))
}

pub(super) fn copilot_transcript_preview(
    visible_fragments: &[String],
    flow: &CopilotSubmitFlowAsset,
) -> String {
    let mut normalized = visible_fragments
        .iter()
        .filter(|fragment| !fragment.contains(&flow.payload))
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");

    if normalized.len() > 512 {
        normalized.truncate(512);
        normalized.push_str("...");
    }

    normalized
}
