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
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(prompt_file.path(), "Do the thing.").unwrap();
    let objective = build_copilot_terminal_objective(&config, prompt_file.path());
    assert!(objective.contains("my-copilot run"));
    assert!(objective.contains("working-directory: /tmp/work"));
    // Issue #1871: the prompt body is now read from the temp file, not inlined.
    assert!(
        objective.contains(prompt_file.path().to_str().unwrap()),
        "objective should reference the prompt file path"
    );
    assert!(
        !objective.contains("Do the thing."),
        "prompt body must NOT be inlined into the shell command (issue #1871)"
    );
    assert!(
        objective.contains("; exit"),
        "objective must chain exit to end the shell naturally"
    );
    assert!(
        objective.contains("-p"),
        "must use -p flag for non-interactive copilot execution"
    );
    assert!(
        objective.contains("--allow-all-tools"),
        "must include --allow-all-tools for non-interactive mode"
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
    let prompt_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(prompt_file.path(), "Hello world").unwrap();
    let objective = build_copilot_terminal_objective(&config, prompt_file.path());
    assert!(!objective.contains("working-directory:"));
    assert!(objective.contains("; exit"));
    assert!(objective.contains("amplihack copilot"));
}

/// Regression for issue #1871: prompts containing both single and double
/// quotes (plus a large body) must round-trip through the adapter without
/// any shell-escaping ambiguity. With the new file-based approach, the
/// objective shell command is short and does not embed the prompt body at
/// all — the prompt is delivered to the copilot subprocess via
/// `cat 'PATH'` reading a file written out-of-band from Rust.
#[test]
fn build_copilot_objective_handles_apostrophes_and_double_quotes() {
    let config = CopilotAdapterConfig::default();
    // Prompt with the classic failure patterns: apostrophes ("author's",
    // "Simard's"), backslashes, double quotes, and a $-prefixed token that
    // would have undergone shell expansion if inlined.
    let prompt = "author's Simard's judge's response: she said \"hello $USER\" and \\escaped\\";
    let temp_file = write_prompt_to_tempfile(prompt).expect("write tempfile");
    let objective = build_copilot_terminal_objective(&config, temp_file.path());

    // The prompt body must NEVER appear in the shell command line — that
    // was the root cause of #1871.
    for needle in [
        "author's",
        "Simard's",
        "judge's",
        "hello $USER",
        "\\escaped\\",
    ] {
        assert!(
            !objective.contains(needle),
            "prompt fragment {needle:?} leaked into shell command: {objective:?}"
        );
    }

    // The file on disk must contain the prompt verbatim.
    let written = std::fs::read_to_string(temp_file.path()).expect("read prompt file");
    assert_eq!(
        written, prompt,
        "tempfile contents must be byte-for-byte identical to the input prompt"
    );

    // The objective must reference the temp file path.
    assert!(
        objective.contains(temp_file.path().to_str().unwrap()),
        "objective must reference the temp file path"
    );
}

/// Regression for issue #1871 (PTY line-buffer overflow): a 128 KB+ prompt
/// containing apostrophes must produce a short shell command line and a
/// faithful on-disk prompt file. The reporter observed the failure with a
/// ~12 KB prompt; we lock the larger 128 KB threshold to leave headroom.
#[test]
fn build_copilot_objective_handles_prompts_over_128kb() {
    let config = CopilotAdapterConfig::default();

    // Build a > 128 KB prompt mixing apostrophes, double quotes, backslashes,
    // and shell metacharacters. The repeated unit is 64 bytes so 3000 reps
    // ≈ 187 KB — well over the threshold.
    let unit = "author's judge's \"merge\" `pwned` ${injected} \\nope\\ end-of-line\n";
    assert_eq!(unit.len(), 64);
    let prompt: String = unit.repeat(3000);
    assert!(prompt.len() > 128 * 1024, "prompt should exceed 128 KB");

    let temp_file = write_prompt_to_tempfile(&prompt).expect("write tempfile");
    let objective = build_copilot_terminal_objective(&config, temp_file.path());

    // The objective shell-command must stay small — comfortably under the
    // canonical PTY line buffer (~4 KB on Linux). A few hundred bytes is
    // typical: command + path + flags.
    assert!(
        objective.len() < 1024,
        "objective length {} exceeded 1 KB; large prompt may have leaked into command line",
        objective.len()
    );

    // The on-disk file must contain the prompt byte-for-byte.
    let written = std::fs::read_to_string(temp_file.path()).expect("read prompt file");
    assert_eq!(written.len(), prompt.len(), "tempfile size mismatch");
    assert_eq!(written, prompt, "tempfile content mismatch");

    // The command must reference the file via `cat 'PATH'`, not inline it.
    assert!(
        objective.contains("$(cat '"),
        "objective must read prompt via $(cat 'PATH'): {objective:?}"
    );

    // Sanity: no fragment of the inlined prompt body leaked through.
    assert!(
        !objective.contains("author's"),
        "prompt body must not appear in shell command"
    );
}

/// `write_prompt_to_tempfile` returns a `NamedTempFile` whose path is safe
/// to single-quote in shell. The `NamedTempFile` API guarantees ASCII
/// alphanumerics, `/`, `.`, `_`, and `-`; this test pins that contract so
/// any future change that broke it would surface here, not as a production
/// shell-injection bug.
#[test]
fn write_prompt_tempfile_path_is_shell_safe() {
    let file = write_prompt_to_tempfile("ok").expect("write tempfile");
    let path = file.path().to_string_lossy();
    for ch in path.chars() {
        assert!(
            ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-'),
            "tempfile path {path:?} contains unexpected character {ch:?}; \
             single-quoting in build_copilot_terminal_objective would no longer be safe"
        );
    }
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
    assert!(is_copilot_footer_line(
        "Tokens    \u{2191} 29.9k \u{2022} \u{2193} 5 \u{2022} 12.7k (cached)"
    ));
    assert!(is_copilot_footer_line(
        "Tokens  \u{2191} 64.7k \u{2193} 12k"
    ));
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
