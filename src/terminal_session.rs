use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeOutcome, BaseTypeSessionRequest, BaseTypeTurnInput,
};
use crate::error::{SimardError, SimardResult};
use crate::sanitization::objective_metadata;

const DEFAULT_SHELL: &str = "/usr/bin/bash";
const PTY_LAUNCHER: &str = "script";

#[derive(Clone, Debug, Eq, PartialEq)]
struct TerminalTurnSpec {
    shell: String,
    working_directory: Option<PathBuf>,
    commands: Vec<String>,
}

impl TerminalTurnSpec {
    fn parse(raw: &str, base_type: &str) -> SimardResult<Self> {
        let mut shell = None;
        let mut working_directory = None;
        let mut commands = Vec::new();

        for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let Some((label, value)) = line.split_once(':') else {
                commands.push(line.to_string());
                continue;
            };

            let label = label.trim().to_ascii_lowercase();
            let value = value.trim();
            if value.is_empty() {
                continue;
            }

            match label.as_str() {
                "shell" => shell = Some(normalize_shell(value, base_type)?),
                "working-directory" | "working_directory" | "cwd" => {
                    working_directory = Some(PathBuf::from(value))
                }
                "command" => commands.push(value.to_string()),
                _ => commands.push(line.to_string()),
            }
        }

        if commands.is_empty() {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: "terminal-shell requires at least one command line".to_string(),
            });
        }

        Ok(Self {
            shell: shell.unwrap_or_else(|| DEFAULT_SHELL.to_string()),
            working_directory,
            commands,
        })
    }
}

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

    Ok(BaseTypeOutcome {
        plan: format!(
            "Open local PTY shell '{}' in '{}' and run {} terminal command(s) for '{}' on '{}'.",
            spec.shell,
            working_directory.display(),
            spec.commands.len(),
            request.mode,
            request.topology,
        ),
        execution_summary: format!(
            "Terminal shell session executed {} via selected base type '{}' on implementation '{}' from node '{}' at '{}' with shell '{}' in '{}' across {} terminal command(s).",
            objective_summary,
            descriptor.id,
            descriptor.backend.identity,
            request.runtime_node,
            request.mailbox_address,
            spec.shell,
            working_directory.display(),
            spec.commands.len(),
        ),
        evidence: vec![
            format!("selected-base-type={}", descriptor.id),
            format!("backend-implementation={}", descriptor.backend.identity),
            format!("shell={}", spec.shell),
            format!("terminal-working-directory={}", working_directory.display()),
            format!("terminal-command-count={}", spec.commands.len()),
            format!("terminal-transcript-preview={transcript_preview}"),
            format!("runtime-node={}", request.runtime_node),
            format!("mailbox-address={}", request.mailbox_address),
        ],
    })
}

fn normalize_shell(value: &str, base_type: &str) -> SimardResult<String> {
    let shell = value.trim();
    let shell_path = Path::new(shell);
    if shell.is_empty()
        || shell.contains('\n')
        || shell.contains('\r')
        || shell.chars().any(char::is_whitespace)
        || !shell_path.is_absolute()
        || !shell
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-'))
    {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: "terminal-shell only accepts an absolute shell executable path using safe path characters"
                .to_string(),
        });
    }

    let metadata =
        fs::metadata(shell_path).map_err(|error| SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!(
                "terminal-shell shell path '{}' could not be inspected: {error}",
                shell_path.display()
            ),
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: format!(
                    "terminal-shell shell path '{}' must be an executable file",
                    shell_path.display()
                ),
            });
        }
    }
    #[cfg(not(unix))]
    if !metadata.is_file() {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!(
                "terminal-shell shell path '{}' must be a file",
                shell_path.display()
            ),
        });
    }

    Ok(shell.to_string())
}

fn resolve_working_directory(path: Option<&Path>, base_type: &str) -> SimardResult<PathBuf> {
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

fn run_terminal_script(
    base_type: &str,
    spec: &TerminalTurnSpec,
    working_directory: &Path,
) -> SimardResult<String> {
    let launch_command = format!("{} --noprofile --norc -i", spec.shell);
    let mut child = Command::new(PTY_LAUNCHER)
        .arg("-qefc")
        .arg(&launch_command)
        .arg("/dev/null")
        .current_dir(working_directory)
        .env("TERM", "dumb")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!("failed to launch local PTY shell via '{PTY_LAUNCHER}': {error}"),
        })?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: "terminal-shell session did not expose stdin".to_string(),
            })?;
        for command in &spec.commands {
            writeln!(stdin, "{command}").map_err(|error| SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: format!("failed to write terminal command input: {error}"),
            })?;
        }
        writeln!(stdin, "exit").map_err(|error| SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!("failed to finalize terminal session input: {error}"),
        })?;
    }

    let output =
        child
            .wait_with_output()
            .map_err(|error| SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: format!("terminal-shell session failed while waiting for output: {error}"),
            })?;

    if !output.status.success() {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!(
                "terminal-shell session exited with status {}",
                output.status
            ),
        });
    }

    let mut transcript = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !transcript.ends_with('\n') {
            transcript.push('\n');
        }
        transcript.push_str(&stderr);
    }

    Ok(transcript)
}

fn transcript_preview(transcript: &str) -> String {
    let mut normalized = transcript
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");

    if normalized.len() > 240 {
        normalized.truncate(240);
        normalized.push_str("...");
    }

    normalized
}

#[cfg(test)]
mod tests {
    use super::normalize_shell;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn assert_invalid_shell(shell: &str, expected: &str) {
        let error = normalize_shell(shell, "terminal-shell").unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "unexpected error for {shell:?}: {error}"
        );
    }

    fn unique_test_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "simard-terminal-shell-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn normalize_shell_accepts_known_safe_absolute_shell() {
        assert_eq!(
            normalize_shell("/usr/bin/bash", "terminal-shell").unwrap(),
            "/usr/bin/bash"
        );
    }

    #[test]
    fn normalize_shell_rejects_metacharacters() {
        for shell in [
            "/usr/bin/bash$(printf-pwned)",
            "/usr/bin/bash;whoami",
            "/usr/bin/bash&",
            "/usr/bin/bash|cat",
            "/usr/bin/bash>file",
            "/usr/bin/bash`whoami`",
        ] {
            assert_invalid_shell(
                shell,
                "only accepts an absolute shell executable path using safe path characters",
            );
        }
    }

    #[test]
    fn normalize_shell_rejects_relative_paths() {
        assert_invalid_shell(
            "bash",
            "only accepts an absolute shell executable path using safe path characters",
        );
    }

    #[test]
    fn normalize_shell_rejects_empty_or_whitespace_only_values() {
        for shell in ["", "   ", "\t", "/usr/bin/bash whoami"] {
            assert_invalid_shell(
                shell,
                "only accepts an absolute shell executable path using safe path characters",
            );
        }
    }

    #[test]
    fn normalize_shell_rejects_missing_files() {
        let missing = unique_test_path("missing");
        assert_invalid_shell(missing.to_string_lossy().as_ref(), "could not be inspected");
    }

    #[test]
    fn normalize_shell_rejects_directories() {
        let directory = unique_test_path("dir");
        fs::create_dir(&directory).unwrap();
        let result = normalize_shell(directory.to_string_lossy().as_ref(), "terminal-shell");
        fs::remove_dir(&directory).unwrap();

        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("must be an executable file"),
            "unexpected error: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn normalize_shell_rejects_non_executable_files() {
        let file = unique_test_path("file");
        fs::write(&file, "#!/bin/sh\nexit 0\n").unwrap();

        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&file, permissions).unwrap();

        let result = normalize_shell(file.to_string_lossy().as_ref(), "terminal-shell");
        fs::remove_file(&file).unwrap();

        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("must be an executable file"),
            "unexpected error: {error}"
        );
    }
}
