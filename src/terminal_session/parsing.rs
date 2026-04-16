use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{SimardError, SimardResult};

use super::types::{DEFAULT_SHELL, TerminalStep, TerminalTurnSpec, WAIT_STEP_TIMEOUT};

impl TerminalTurnSpec {
    pub(crate) fn parse(raw: &str, base_type: &str) -> SimardResult<Self> {
        let mut shell = None;
        let mut working_directory = None;
        let mut wait_timeout = WAIT_STEP_TIMEOUT;
        let mut steps = Vec::new();

        for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let Some((label, value)) = line.split_once(':') else {
                steps.push(TerminalStep::Input(line.to_string()));
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
                    working_directory = Some(PathBuf::from(value));
                }
                "wait-timeout-seconds" | "wait_timeout_seconds" | "wait-timeout" => {
                    wait_timeout = parse_wait_timeout(value, base_type)?;
                }
                "command" | "input" => steps.push(TerminalStep::Input(value.to_string())),
                "wait-for" | "wait_for" | "expect" => {
                    steps.push(TerminalStep::WaitFor(value.to_string()));
                }
                _ => steps.push(TerminalStep::Input(line.to_string())),
            }
        }

        if !steps
            .iter()
            .any(|step| matches!(step, TerminalStep::Input(_)))
        {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: "terminal-shell requires at least one input line".to_string(),
            });
        }

        Ok(Self {
            shell: shell.unwrap_or_else(|| DEFAULT_SHELL.to_string()),
            working_directory,
            wait_timeout,
            steps,
        })
    }

    pub(crate) fn input_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| matches!(step, TerminalStep::Input(_)))
            .count()
    }

    pub(crate) fn wait_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| matches!(step, TerminalStep::WaitFor(_)))
            .count()
    }
}

pub(crate) fn parse_wait_timeout(value: &str, base_type: &str) -> SimardResult<Duration> {
    let seconds = value
        .parse::<u64>()
        .map_err(|error| SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!("terminal-shell wait timeout '{value}' is invalid: {error}"),
        })?;
    if !(1..=60).contains(&seconds) {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!(
                "terminal-shell wait timeout '{value}' must be between 1 and 60 seconds"
            ),
        });
    }
    Ok(Duration::from_secs(seconds))
}

pub(crate) fn normalize_shell(value: &str, base_type: &str) -> SimardResult<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    enum TempPathKind {
        File,
        Directory,
    }

    struct TempPathGuard {
        path: PathBuf,
        kind: TempPathKind,
    }

    impl TempPathGuard {
        fn directory(path: PathBuf) -> Self {
            Self {
                path,
                kind: TempPathKind::Directory,
            }
        }

        fn file(path: PathBuf) -> Self {
            Self {
                path,
                kind: TempPathKind::File,
            }
        }
    }

    impl Drop for TempPathGuard {
        fn drop(&mut self) {
            match self.kind {
                TempPathKind::File => {
                    let _ = fs::remove_file(&self.path);
                }
                TempPathKind::Directory => {
                    let _ = fs::remove_dir(&self.path);
                }
            }
        }
    }

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
        let _guard = TempPathGuard::directory(directory.clone());
        let result = normalize_shell(directory.to_string_lossy().as_ref(), "terminal-shell");

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
        let _guard = TempPathGuard::file(file.clone());

        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&file, permissions).unwrap();

        let result = normalize_shell(file.to_string_lossy().as_ref(), "terminal-shell");

        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("must be an executable file"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn parse_terminal_turn_supports_wait_for_steps() {
        let spec = TerminalTurnSpec::parse(
            "working-directory: .\ncommand: printf \"ready\\n\"\nwait-for: ready\ninput: exit",
            "terminal-shell",
        )
        .expect("terminal turn should parse");

        assert_eq!(
            spec.steps,
            vec![
                TerminalStep::Input("printf \"ready\\n\"".to_string()),
                TerminalStep::WaitFor("ready".to_string()),
                TerminalStep::Input("exit".to_string()),
            ]
        );
        assert_eq!(spec.input_count(), 2);
        assert_eq!(spec.wait_count(), 1);
    }

    #[test]
    fn parse_terminal_turn_supports_wait_timeout_override() {
        let spec = TerminalTurnSpec::parse(
            "working-directory: .\nwait-timeout-seconds: 30\ncommand: printf \"ready\\n\"\nwait-for: ready",
            "terminal-shell",
        )
        .expect("terminal turn should parse");

        assert_eq!(spec.wait_timeout, std::time::Duration::from_secs(30));
    }
}
