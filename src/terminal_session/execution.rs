use std::path::{Path, PathBuf};

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeOutcome, BaseTypeSessionRequest, BaseTypeTurnInput,
};
use crate::error::{SimardError, SimardResult};
use crate::sanitization::objective_metadata;

use super::evidence::{
    terminal_checkpoint_evidence, terminal_last_output_line, terminal_step_evidence,
    transcript_preview,
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
            reason: format!(
                "terminal-shell session exited with status {}",
                capture.exit_status
            ),
        });
    }

    Ok(capture.transcript)
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
    use super::*;

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
