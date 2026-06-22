use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeOutcome, BaseTypeSessionRequest, BaseTypeTurnInput,
};
use crate::error::{SimardError, SimardResult};
use crate::sanitization::objective_metadata;

use super::evidence::{
    terminal_checkpoint_evidence, terminal_failure_hint, terminal_last_output_line,
    terminal_step_evidence, transcript_preview,
};
use super::session::PtyTerminalSession;
use super::types::{TerminalStep, TerminalTurnSpec, TerminalWaitStatus};
use super::workflow_guard::capture_workflow_restore_guards_for_steps;

pub fn execute_terminal_turn(
    descriptor: &BaseTypeDescriptor,
    request: &BaseTypeSessionRequest,
    input: &BaseTypeTurnInput,
) -> SimardResult<BaseTypeOutcome> {
    let spec = TerminalTurnSpec::parse(&input.objective, descriptor.id.as_str())?;
    let working_directory =
        resolve_working_directory(spec.working_directory.as_deref(), descriptor.id.as_str())?;
    let transcript = run_terminal_script(descriptor.id.as_str(), &spec, &working_directory)?;
    let transcript_preview = transcript_preview(&transcript);
    let objective_summary = objective_metadata(&input.objective);
    let input_count = spec.input_count();
    let wait_count = spec.wait_count();
    let step_evidence = terminal_step_evidence(&spec.steps);
    let checkpoint_evidence = terminal_checkpoint_evidence(&spec.steps);
    let last_output_line = terminal_last_output_line(&transcript, &spec.steps);
    let mut evidence = vec![
        format!("selected-base-type={}", descriptor.id),
        format!("backend-implementation={}", descriptor.backend.identity),
        format!("shell={}", spec.shell),
        format!("terminal-working-directory={}", working_directory.display()),
        format!("terminal-command-count={input_count}"),
        format!("terminal-wait-count={wait_count}"),
        format!(
            "terminal-wait-timeout-seconds={}",
            spec.wait_timeout.as_secs()
        ),
        format!("terminal-step-count={}", spec.steps.len()),
        format!("terminal-transcript-preview={transcript_preview}"),
        format!("runtime-node={}", request.runtime_node),
        format!("mailbox-address={}", request.mailbox_address),
    ];
    evidence.extend(step_evidence);
    evidence.extend(checkpoint_evidence);
    if let Some(last_output_line) = last_output_line {
        evidence.push(format!("terminal-last-output-line={last_output_line}"));
    }
    // Include the full transcript so adapters (e.g. copilot) can extract
    // the actual LLM response instead of relying on the truncated preview.
    evidence.push(format!("terminal-transcript-full={transcript}"));

    Ok(BaseTypeOutcome {
        plan: format!(
            "Open local PTY shell '{}' in '{}' and run {} terminal input line(s) with {} wait checkpoint(s) and a {}s wait timeout for '{}' on '{}'.",
            spec.shell,
            working_directory.display(),
            input_count,
            wait_count,
            spec.wait_timeout.as_secs(),
            request.mode,
            request.topology,
        ),
        execution_summary: format!(
            "Terminal shell session executed {} via selected base type '{}' on implementation '{}' from node '{}' at '{}' with shell '{}' in '{}' across {} terminal input line(s), {} wait checkpoint(s), and a {}s wait timeout.",
            objective_summary,
            descriptor.id,
            descriptor.backend.identity,
            request.runtime_node,
            request.mailbox_address,
            spec.shell,
            working_directory.display(),
            input_count,
            wait_count,
            spec.wait_timeout.as_secs(),
        ),
        evidence,
    })
}

fn run_terminal_script(
    base_type: &str,
    spec: &TerminalTurnSpec,
    working_directory: &Path,
) -> SimardResult<String> {
    let _workflow_restore_guards =
        capture_workflow_restore_guards_for_steps(base_type, &spec.steps, working_directory)?;
    let mut session = PtyTerminalSession::launch(base_type, &spec.shell, working_directory)?;
    for step in &spec.steps {
        match step {
            TerminalStep::Input(command) => session.send_input(command)?,
            TerminalStep::WaitFor(expected) => {
                match session.wait_for_output(expected, spec.wait_timeout)? {
                    TerminalWaitStatus::Satisfied => {}
                    TerminalWaitStatus::ExitedEarly(status) => {
                        return Err(SimardError::AdapterInvocationFailed {
                            base_type: base_type.to_string(),
                            reason: format!(
                                "terminal-shell session exited with status {status} before expected output '{expected}' appeared"
                            ),
                        });
                    }
                    TerminalWaitStatus::TimedOut => {
                        return Err(SimardError::AdapterInvocationFailed {
                            base_type: base_type.to_string(),
                            reason: format!(
                                "terminal-shell did not emit expected output '{expected}' within {}s",
                                spec.wait_timeout.as_secs()
                            ),
                        });
                    }
                }
            }
        }
    }

    let capture = session.finish()?;
    if !capture.exit_status.success() {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: describe_terminal_failure(
                &capture.exit_status,
                &capture.transcript,
                &spec.steps,
            ),
        });
    }

    Ok(capture.transcript)
}

/// Build an actionable failure message for a non-zero terminal-shell exit.
///
/// A bare `exit status: 127` tells an operator nothing. This explains what the
/// well-known shell exit codes mean (127 = command not found, 126 = not
/// executable) and surfaces the shell's own diagnostic line (e.g.
/// `bash: say: command not found`) — or the last terminal output — so the
/// failure can be diagnosed without re-running the session.
fn describe_terminal_failure(
    exit_status: &ExitStatus,
    transcript: &str,
    steps: &[TerminalStep],
) -> String {
    let mut reason = format!("terminal-shell session exited with status {exit_status}");
    match exit_status.code() {
        Some(127) => reason.push_str(
            " — exit code 127 means a command in the terminal session could not be found on PATH; \
             verify the command is installed and on PATH, or invoke it with an absolute path",
        ),
        Some(126) => reason.push_str(
            " — exit code 126 means a command was found but is not executable; \
             check the file permissions",
        ),
        _ => {}
    }
    if let Some(hint) = terminal_failure_hint(transcript) {
        reason.push_str(&format!(" (shell reported: {hint})"));
    } else if let Some(last) = terminal_last_output_line(transcript, steps) {
        reason.push_str(&format!(" (last terminal output: {last})"));
    }
    reason
}

pub(crate) fn resolve_working_directory(
    path: Option<&Path>,
    base_type: &str,
) -> SimardResult<PathBuf> {
    let cwd = match path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => std::env::current_dir()
            .map_err(|error| SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: format!("failed to resolve current working directory: {error}"),
            })?
            .join(path),
        None => std::env::current_dir().map_err(|error| SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!("failed to resolve current working directory: {error}"),
        })?,
    };

    if !cwd.is_dir() {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!(
                "terminal-shell working directory '{}' does not exist",
                cwd.display()
            ),
        });
    }

    Ok(cwd)
}

#[cfg(test)]
mod tests {
    use super::super::types::TerminalTurnSpec;
    use super::*;

    // -- run_terminal_script: failure diagnostics (regression for #2077) --

    /// Regression for #2077: `engineer terminal "say hello"` exited 127 with a
    /// bare `terminal-shell session exited with status exit status: 127` that
    /// gave the operator no clue what failed. A missing-binary turn must now
    /// surface an actionable message: the 127 meaning *and* the shell's own
    /// "command not found" diagnostic naming the offending command.
    #[test]
    fn run_terminal_script_surfaces_command_not_found_for_missing_binary() {
        let spec =
            TerminalTurnSpec::parse("definitely-not-a-real-binary-2077 hello", "terminal-shell")
                .unwrap();
        let cwd = std::env::current_dir().unwrap();

        let error = run_terminal_script("terminal-shell", &spec, &cwd)
            .expect_err("a missing binary must fail the terminal turn");
        let message = error.to_string();

        assert!(
            message.contains("127"),
            "error must report the exit code: {message}"
        );
        assert!(
            message.contains("could not be found on PATH"),
            "error must explain exit code 127: {message}"
        );
        assert!(
            message.to_ascii_lowercase().contains("command not found")
                || message.to_ascii_lowercase().contains("not found"),
            "error must surface the shell diagnostic: {message}"
        );
        assert!(
            message.contains("definitely-not-a-real-binary-2077"),
            "error must name the offending command: {message}"
        );
        assert!(
            !message.ends_with("exit status: 127"),
            "error must not be the bare opaque 127 message: {message}"
        );
    }

    /// The success path must remain intact: a valid command resolves on PATH
    /// and completes the terminal turn, returning its transcript.
    #[test]
    fn run_terminal_script_succeeds_for_valid_command() {
        let spec =
            TerminalTurnSpec::parse("echo simard-terminal-2077-ok", "terminal-shell").unwrap();
        let cwd = std::env::current_dir().unwrap();

        let transcript = run_terminal_script("terminal-shell", &spec, &cwd)
            .expect("a valid command should complete the terminal turn");
        assert!(
            transcript.contains("simard-terminal-2077-ok"),
            "transcript should contain the echoed output: {transcript}"
        );
    }

    // -- describe_terminal_failure --

    fn exit_status_with_code(code: i32) -> ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            // Encode the code into the wait-status byte (code << 8).
            ExitStatus::from_raw(code << 8)
        }
        #[cfg(not(unix))]
        {
            let _ = code;
            unimplemented!("exit_status_with_code is only used on unix test targets");
        }
    }

    #[test]
    fn describe_terminal_failure_explains_127_with_shell_hint() {
        let transcript = "bash-5.2$ say hello\nbash: say: command not found\nbash-5.2$ exit";
        let steps = vec![TerminalStep::Input("say hello".to_string())];

        let reason = describe_terminal_failure(&exit_status_with_code(127), transcript, &steps);

        assert!(reason.contains("status"), "{reason}");
        assert!(reason.contains("could not be found on PATH"), "{reason}");
        assert!(reason.contains("bash: say: command not found"), "{reason}");
    }

    #[test]
    fn describe_terminal_failure_explains_126_not_executable() {
        let transcript = "bash: ./blocked.sh: Permission denied";
        let reason = describe_terminal_failure(&exit_status_with_code(126), transcript, &[]);

        assert!(reason.contains("not executable"), "{reason}");
        assert!(
            reason.to_ascii_lowercase().contains("permission denied"),
            "{reason}"
        );
    }

    #[test]
    fn describe_terminal_failure_falls_back_to_last_output_without_marker() {
        let transcript = "bash-5.2$ run-thing\nthing failed with code 3\nbash-5.2$ exit";
        let steps = vec![TerminalStep::Input("run-thing".to_string())];

        let reason = describe_terminal_failure(&exit_status_with_code(3), transcript, &steps);

        assert!(reason.contains("last terminal output"), "{reason}");
        assert!(reason.contains("thing failed with code 3"), "{reason}");
    }

    // -- resolve_working_directory --

    #[test]
    fn resolve_working_directory_uses_cwd_when_none() {
        let result = resolve_working_directory(None, "test-bt").unwrap();
        assert!(result.is_dir());
        assert!(result.is_absolute());
    }

    #[test]
    fn resolve_working_directory_returns_absolute_path_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let result = resolve_working_directory(Some(dir.path()), "test-bt").unwrap();
        assert_eq!(result, dir.path().to_path_buf());
    }

    #[test]
    fn resolve_working_directory_resolves_relative_against_cwd() {
        let result = resolve_working_directory(Some(Path::new(".")), "test-bt").unwrap();
        assert!(result.is_absolute());
        assert!(result.is_dir());
    }

    #[test]
    fn resolve_working_directory_errors_on_nonexistent_path() {
        let result =
            resolve_working_directory(Some(Path::new("/nonexistent_path_12345")), "test-bt");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("does not exist"), "{msg}");
    }

    #[test]
    fn resolve_working_directory_errors_on_file_not_dir() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a_file.txt");
        std::fs::write(&file, "").unwrap();
        let result = resolve_working_directory(Some(file.as_path()), "test-bt");
        assert!(result.is_err());
    }
}
