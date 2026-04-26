use super::base_type_copilot::*;
use crate::base_types::BaseTypeSessionRequest;
use crate::base_types::{BaseTypeCapability, BaseTypeFactory, BaseTypeTurnInput};
use crate::identity::OperatingMode;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::session::SessionId;

fn test_request() -> BaseTypeSessionRequest {
    BaseTypeSessionRequest {
        session_id: SessionId::from_uuid(uuid::Uuid::now_v7()),
        mode: OperatingMode::Engineer,
        topology: RuntimeTopology::SingleProcess,
        prompt_assets: vec![],
        runtime_node: RuntimeNodeId::new("node-1"),
        mailbox_address: RuntimeAddress::new("addr-1"),
    }
}

#[test]
fn copilot_adapter_creates_session() {
    let adapter = CopilotSdkAdapter::registered("copilot-test").unwrap();
    assert_eq!(adapter.descriptor().id.as_str(), "copilot-test");
    assert!(
        adapter
            .descriptor()
            .capabilities
            .contains(&BaseTypeCapability::TerminalSession)
    );
}

#[test]
fn copilot_session_lifecycle() {
    let adapter = CopilotSdkAdapter::registered("copilot-lifecycle").unwrap();
    let request = test_request();
    let mut session = adapter.open_session(request).unwrap();

    session.open().unwrap();
    assert!(session.open().is_err()); // Double open
    session.close().unwrap();
    assert!(session.close().is_err()); // Double close
}

#[test]
fn copilot_session_rejects_turn_before_open() {
    let adapter = CopilotSdkAdapter::registered("copilot-pre-open").unwrap();
    let request = test_request();
    let mut session = adapter.open_session(request).unwrap();

    let result = session.run_turn(BaseTypeTurnInput::objective_only("test"));
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("must be opened"));
}

#[test]
fn copilot_adapter_rejects_unsupported_topology() {
    let adapter = CopilotSdkAdapter::registered("copilot-topo").unwrap();
    let mut request = test_request();
    request.topology = RuntimeTopology::MultiProcess;
    let result = adapter.open_session(request);
    assert!(result.is_err());
}

#[test]
fn copilot_adapter_rejects_command_with_shell_metacharacters() {
    let config = CopilotAdapterConfig {
        command: "echo; rm -rf /".to_string(),
        working_directory: None,
    };
    let result = CopilotSdkAdapter::with_config("copilot-inject", config);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("forbidden"));
}

#[test]
fn copilot_adapter_rejects_empty_command() {
    let config = CopilotAdapterConfig {
        command: "  ".to_string(),
        working_directory: None,
    };
    let result = CopilotSdkAdapter::with_config("copilot-empty", config);
    assert!(result.is_err());
}

#[test]
fn build_copilot_objective_includes_command_and_exit() {
    let config = CopilotAdapterConfig {
        command: "my-copilot run".to_string(),
        working_directory: Some("/tmp/work".to_string()),
    };
    let prompt = "Do the thing.";
    let objective = build_copilot_terminal_objective(&config, prompt);
    assert!(objective.contains("my-copilot run"));
    assert!(objective.contains("working-directory: /tmp/work"));
    assert!(objective.contains("Do the thing."));
    assert!(
        objective.contains("; exit"),
        "objective must chain exit to end the shell naturally"
    );
    assert!(
        !objective.contains("wait-for:"),
        "no wait-for — process exits naturally"
    );
    assert!(
        !objective.contains("wait-timeout"),
        "no timeout — process runs to completion"
    );
}

#[test]
fn build_copilot_objective_without_working_directory() {
    let config = CopilotAdapterConfig::default();
    let prompt = "Hello world";
    let objective = build_copilot_terminal_objective(&config, prompt);
    assert!(!objective.contains("working-directory:"));
    assert!(objective.contains("; exit"));
    assert!(objective.contains("amplihack copilot"));
}

#[test]
fn build_copilot_objective_escapes_single_quotes() {
    let config = CopilotAdapterConfig::default();
    let prompt = "it's a test with 'quotes'";
    let objective = build_copilot_terminal_objective(&config, prompt);
    assert!(!objective.contains("it's"), "single quotes must be escaped");
}

#[test]
fn extract_response_from_transcript_isolates_copilot_output() {
    // Full transcript format: newline-delimited lines from `script`
    let transcript = "\
Script started on 2025-04-07 02:55:00+00:00
bash-5.2$ SIMARD_PROMPT_FILE=$(mktemp) && printf '%s' 'Hello' > \"$SIMARD_PROMPT_FILE\" && amplihack copilot -p \"$(cat \"$SIMARD_PROMPT_FILE\")\" ; rm -f \"$SIMARD_PROMPT_FILE\" ; exit
I'm Simard, your engineering agent. How can I help?
Total usage est: 0.012
bash-5.2$ exit
Script done on 2025-04-07 02:56:00+00:00";
    let response = extract_response_from_transcript(transcript);
    assert!(
        response.contains("I'm Simard"),
        "should extract copilot response: got '{response}'"
    );
    assert!(
        !response.contains("exit"),
        "exit command should be stripped"
    );
    assert!(
        !response.contains("Total usage"),
        "usage stats should be stripped"
    );
}

#[test]
fn extract_response_from_transcript_pipe_delimited_secondary() {
    // Preview format: pipe-delimited
    let transcript =
        "SIMARD_PROMPT_FILE=x && amplihack copilot -p test | Hello from Simard | bash-5.2$ exit";
    let response = extract_response_from_transcript(transcript);
    assert!(
        response.contains("Hello from Simard"),
        "should handle pipe-delimited: got '{response}'"
    );
}

#[test]
fn extract_response_from_transcript_handles_no_command_marker() {
    let transcript = "some output\nactual response text\nmore text";
    let response = extract_response_from_transcript(transcript);
    assert!(!response.is_empty(), "should return all content lines");
}

#[test]
fn extract_response_from_transcript_handles_empty() {
    let response = extract_response_from_transcript("");
    assert!(response.is_empty() || response.trim().is_empty());
}

#[test]
fn extract_response_strips_new_copilot_billing_footer() {
    // Regression for issue #1062 / dashboard chat bug: the new Copilot CLI
    // emits a billing-summary footer that the parser previously leaked
    // into the chat as if it were the assistant's reply.
    let transcript = "\
Script started on 2026-04-21 03:00:00+00:00
bash-5.2$ SIMARD_PROMPT_FILE=$(mktemp) && amplihack copilot -p \"$(cat \"$SIMARD_PROMPT_FILE\")\" ; exit
Hello from the assistant.
Changes   +0 -0
Requests  7.5 Premium (10s)
bash-5.2$ exit
Script done on 2026-04-21 03:00:17+00:00";
    let response = extract_response_from_transcript(transcript);
    assert!(
        response.contains("Hello from the assistant"),
        "should extract assistant content: got '{response}'"
    );
    assert!(
        !response.contains("Changes"),
        "Changes telemetry must be stripped: got '{response}'"
    );
    assert!(
        !response.contains("Premium"),
        "Requests/Premium telemetry must be stripped: got '{response}'"
    );
}

#[test]
fn extract_response_returns_empty_when_only_telemetry_present() {
    // When the Copilot CLI exits without producing conversational
    // content (auth fail, rate limit, etc.) the transcript contains
    // only the billing footer. Parser must NOT emit that footer as
    // the response — it must return empty so the caller can fail loud.
    let transcript = "\
Script started on 2026-04-21 03:00:00+00:00
bash-5.2$ amplihack copilot -p \"hi\" ; exit
Changes   +0 -0
Requests  7.5 Premium (17s)
bash-5.2$ exit
Script done on 2026-04-21 03:00:17+00:00";
    let response = extract_response_from_transcript(transcript);
    assert!(
        response.trim().is_empty(),
        "telemetry-only transcript must yield empty response, got '{response}'"
    );
}

#[test]
fn copilot_footer_classifier_recognizes_legacy_and_new_formats() {
    assert!(is_copilot_footer_line("Total usage est: 0.012"));
    assert!(is_copilot_footer_line("API time spent: 1.2s"));
    assert!(is_copilot_footer_line("Total session time: 17s"));
    assert!(is_copilot_footer_line("Changes   +0 -0"));
    assert!(is_copilot_footer_line("Changes +12 -3"));
    assert!(is_copilot_footer_line("Requests  7.5 Premium (10s)"));
    assert!(is_copilot_footer_line("Requests  3 Premium (8m 28s)"));
    assert!(!is_copilot_footer_line("Hello from the assistant"));
    assert!(!is_copilot_footer_line(""));
}

#[test]
fn extract_copilot_response_from_evidence_prefers_full_transcript() {
    let evidence = vec![
        "selected-base-type=copilot-test".to_string(),
        "terminal-transcript-preview=SIMARD_PROMPT_FILE=x && amplihack copilot -p test | Preview only | bash-5.2$ exit".to_string(),
        "terminal-transcript-full=Script started on 2025-04-07\nbash-5.2$ amplihack copilot -p test\nFull response from Simard\nTotal usage est: 0.01\nbash-5.2$ exit\nScript done on 2025-04-07".to_string(),
    ];
    let response = extract_copilot_response_from_evidence(&evidence);
    assert!(
        response.contains("Full response from Simard"),
        "should prefer full transcript: got '{response}'"
    );
    assert!(
        !response.contains("Preview only"),
        "should not use preview when full is available"
    );
}

#[test]
fn extract_copilot_response_from_evidence_falls_back_to_preview() {
    let evidence = vec![
        "selected-base-type=copilot-test".to_string(),
        "terminal-transcript-preview=SIMARD_PROMPT_FILE=x && amplihack copilot -p test | Hello from Simard | bash-5.2$ exit".to_string(),
    ];
    let response = extract_copilot_response_from_evidence(&evidence);
    assert!(
        response.contains("Hello from Simard"),
        "should use preview: got '{response}'"
    );
}

#[test]
fn extract_copilot_response_from_evidence_handles_missing_preview() {
    let evidence = vec!["selected-base-type=copilot-test".to_string()];
    let response = extract_copilot_response_from_evidence(&evidence);
    assert!(response.is_empty());
}

#[test]
fn extract_response_from_transcript_filters_shell_timing_and_hooks() {
    let transcript = "\
Script started on 2025-04-07 02:55:00+00:00
bash-5.2$ SIMARD_PROMPT_FILE=$(mktemp) && cat \"$SIMARD_PROMPT_FILE\" | amplihack copilot --subprocess-safe ; exit
Staged pre-tool hook: validate.sh
Loaded hook: post_response
Hook fired: telemetry
Created file /tmp/foo.txt
Modified file src/lib.rs
Wrote file: README.md
Hello, I am the actual response.
real\t0m1.234s
user\t0m0.123s
sys\t0m0.456s
Total usage est: 0.012
bash-5.2$ exit
Script done on 2025-04-07 02:56:00+00:00";
    let response = extract_response_from_transcript(transcript);
    assert!(
        response.contains("Hello, I am the actual response."),
        "real response must survive: got {response:?}"
    );
    for noise in [
        "Staged pre-tool hook",
        "Loaded hook",
        "Hook fired",
        "Created file",
        "Modified file",
        "Wrote file",
        "real\t0m",
        "user\t0m",
        "sys\t0m",
    ] {
        assert!(
            !response.contains(noise),
            "noise marker {noise:?} should be filtered, got: {response:?}"
        );
    }
}

#[test]
fn extract_response_from_transcript_strips_amplihack_banners() {
    let transcript = "\
Script started on 2026-04-21 04:21:00+00:00
bash-5.2$ SIMARD_PROMPT_FILE=$(mktemp) && cat \"$SIMARD_PROMPT_FILE\" | amplihack copilot --subprocess-safe ; exit
\u{1b}[33mA newer version of amplihack is available (v0.7.61). Run 'amplihack update' to upgrade.\u{1b}[0m
ℹ NODE_OPTIONS=--max-old-space-size=8192
{\"steps\":[{\"action\":\"ReadOnlyScan\",\"description\":\"inspect repo\"}]}
Total usage est: 0.012
bash-5.2$ exit
Script done on 2026-04-21 04:22:00+00:00";
    let response = extract_response_from_transcript(transcript);
    assert!(
        response.contains("\"steps\""),
        "JSON payload must survive: got {response:?}"
    );
    assert!(
        !response.contains("amplihack is available"),
        "version-update banner should be filtered, got: {response:?}"
    );
    assert!(
        !response.contains("NODE_OPTIONS"),
        "NODE_OPTIONS info line should be filtered, got: {response:?}"
    );
}

#[test]
fn strip_ansi_removes_csi_color_codes() {
    assert_eq!(strip_ansi("\u{1b}[33mhello\u{1b}[0m"), "hello");
    assert_eq!(strip_ansi("plain"), "plain");
    assert_eq!(
        strip_ansi("\u{1b}[1;31mbold red\u{1b}[0m end"),
        "bold red end"
    );
}
