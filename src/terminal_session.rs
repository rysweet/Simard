use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeOutcome, BaseTypeSessionRequest, BaseTypeTurnInput,
};
use crate::error::{SimardError, SimardResult};
use crate::sanitization::{objective_metadata, sanitize_terminal_text};

const DEFAULT_SHELL: &str = "/usr/bin/bash";
const PTY_LAUNCHER: &str = "script";
const WAIT_STEP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, PartialEq)]
enum TerminalStep {
    Input(String),
    WaitFor(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TerminalTurnSpec {
    shell: String,
    working_directory: Option<PathBuf>,
    wait_timeout: Duration,
    steps: Vec<TerminalStep>,
}

struct TranscriptGuard {
    path: PathBuf,
}

impl TranscriptGuard {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TranscriptGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl TerminalTurnSpec {
    fn parse(raw: &str, base_type: &str) -> SimardResult<Self> {
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
                    working_directory = Some(PathBuf::from(value))
                }
                "wait-timeout-seconds" | "wait_timeout_seconds" | "wait-timeout" => {
                    wait_timeout = parse_wait_timeout(value, base_type)?
                }
                "command" | "input" => steps.push(TerminalStep::Input(value.to_string())),
                "wait-for" | "wait_for" | "expect" => {
                    steps.push(TerminalStep::WaitFor(value.to_string()))
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

    fn input_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| matches!(step, TerminalStep::Input(_)))
            .count()
    }

    fn wait_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| matches!(step, TerminalStep::WaitFor(_)))
            .count()
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

fn parse_wait_timeout(value: &str, base_type: &str) -> SimardResult<Duration> {
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
    let transcript_path = unique_transcript_path("transcript");
    let _transcript_guard = TranscriptGuard::new(transcript_path.clone());
    let _transcript_file = open_exclusive_temp_file(&transcript_path, base_type)?;
    let mut child = Command::new(PTY_LAUNCHER)
        .arg("-qefc")
        .arg(&launch_command)
        .arg(&transcript_path)
        .current_dir(working_directory)
        .env("TERM", "dumb")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
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
        for step in &spec.steps {
            match step {
                TerminalStep::Input(command) => {
                    writeln!(stdin, "{command}").map_err(|error| {
                        SimardError::AdapterInvocationFailed {
                            base_type: base_type.to_string(),
                            reason: format!("failed to write terminal command input: {error}"),
                        }
                    })?;
                    stdin
                        .flush()
                        .map_err(|error| SimardError::AdapterInvocationFailed {
                            base_type: base_type.to_string(),
                            reason: format!("failed to flush terminal command input: {error}"),
                        })?;
                }
                TerminalStep::WaitFor(expected) => wait_for_transcript(
                    base_type,
                    &mut child,
                    &transcript_path,
                    expected,
                    spec.wait_timeout,
                )?,
            }
        }
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

    fs::read_to_string(&transcript_path).map_err(|error| SimardError::AdapterInvocationFailed {
        base_type: base_type.to_string(),
        reason: format!(
            "failed to read terminal transcript '{}': {error}",
            transcript_path.display()
        ),
    })
}

fn wait_for_transcript(
    base_type: &str,
    child: &mut std::process::Child,
    transcript_path: &Path,
    expected: &str,
    timeout: Duration,
) -> SimardResult<()> {
    let start = Instant::now();
    loop {
        if fs::read_to_string(transcript_path)
            .map(|transcript| transcript.contains(expected))
            .unwrap_or(false)
        {
            return Ok(());
        }

        if let Some(status) =
            child
                .try_wait()
                .map_err(|error| SimardError::AdapterInvocationFailed {
                    base_type: base_type.to_string(),
                    reason: format!("failed to poll terminal-shell session state: {error}"),
                })?
        {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: format!(
                    "terminal-shell session exited with status {status} before expected output '{expected}' appeared"
                ),
            });
        }

        if start.elapsed() >= timeout {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: format!(
                    "terminal-shell did not emit expected output '{expected}' within {}s",
                    timeout.as_secs()
                ),
            });
        }

        thread::sleep(Duration::from_millis(50));
    }
}

fn unique_transcript_path(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "simard-terminal-shell-{label}-{}-{nanos}.log",
        std::process::id()
    ))
}

fn open_exclusive_temp_file(path: &Path, base_type: &str) -> SimardResult<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!(
                "failed to create terminal transcript '{}': {error}",
                path.display()
            ),
        })
}

fn transcript_preview(transcript: &str) -> String {
    let mut normalized = transcript_content_lines(transcript).join(" | ");

    if normalized.len() > 512 {
        normalized.truncate(512);
        normalized.push_str("...");
    }

    normalized
}

fn terminal_step_evidence(steps: &[TerminalStep]) -> Vec<String> {
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            format!(
                "terminal-step-{}={}",
                index + 1,
                compact_terminal_evidence_value(&render_terminal_step(step), 160)
            )
        })
        .collect()
}

fn terminal_checkpoint_evidence(steps: &[TerminalStep]) -> Vec<String> {
    steps
        .iter()
        .filter_map(|step| match step {
            TerminalStep::WaitFor(expected) => Some(expected.as_str()),
            TerminalStep::Input(_) => None,
        })
        .enumerate()
        .map(|(index, expected)| {
            format!(
                "terminal-checkpoint-{}={}",
                index + 1,
                compact_terminal_evidence_value(expected, 160)
            )
        })
        .collect()
}

fn terminal_last_output_line(transcript: &str, steps: &[TerminalStep]) -> Option<String> {
    let input_commands = steps
        .iter()
        .filter_map(|step| match step {
            TerminalStep::Input(command) => Some(sanitize_terminal_text(command)),
            TerminalStep::WaitFor(_) => None,
        })
        .collect::<Vec<_>>();
    transcript_content_lines(transcript)
        .into_iter()
        .rev()
        .map(sanitize_terminal_text)
        .find(|line| is_meaningful_terminal_output(line, &input_commands))
        .map(|line| compact_terminal_evidence_value(&line, 160))
}

fn is_meaningful_terminal_output(line: &str, input_commands: &[String]) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed == "exit"
        || trimmed.ends_with("$ exit")
        || trimmed.ends_with("# exit")
    {
        return false;
    }

    !input_commands.iter().any(|command| {
        trimmed == command
            || trimmed.ends_with(&format!("$ {command}"))
            || trimmed.ends_with(&format!("# {command}"))
    })
}

fn transcript_content_lines(transcript: &str) -> Vec<&str> {
    transcript
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("Script started on ")
                && !line.starts_with("Script done on ")
        })
        .collect()
}

fn render_terminal_step(step: &TerminalStep) -> String {
    match step {
        TerminalStep::Input(command) => format!("input: {command}"),
        TerminalStep::WaitFor(expected) => format!("wait-for: {expected}"),
    }
}

fn compact_terminal_evidence_value(raw: &str, limit: usize) -> String {
    let mut normalized = sanitize_terminal_text(raw)
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    if normalized.len() > limit {
        normalized.truncate(limit);
        normalized.push_str("...");
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalStep, TerminalTurnSpec, compact_terminal_evidence_value, normalize_shell,
        terminal_checkpoint_evidence, terminal_last_output_line, terminal_step_evidence,
    };
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

    #[test]
    fn terminal_step_and_checkpoint_evidence_preserve_operator_visible_flow() {
        let steps = vec![
            TerminalStep::Input("printf \"ready\\n\"".to_string()),
            TerminalStep::WaitFor("ready".to_string()),
            TerminalStep::Input("/status".to_string()),
        ];

        assert_eq!(
            terminal_step_evidence(&steps),
            vec![
                "terminal-step-1=input: printf \"ready\\n\"".to_string(),
                "terminal-step-2=wait-for: ready".to_string(),
                "terminal-step-3=input: /status".to_string(),
            ]
        );
        assert_eq!(
            terminal_checkpoint_evidence(&steps),
            vec!["terminal-checkpoint-1=ready".to_string()]
        );
    }

    #[test]
    fn terminal_last_output_line_ignores_script_preamble_and_sanitizes_control_text() {
        let transcript = "Script started on 2025-03-29 12:00:00+00:00 [COMMAND=\"/usr/bin/bash --noprofile --norc -i\" <not executed on terminal>]\nterminal-ready\n\u{1b}[32mterminal-ok\u{1b}[0m\nScript done on 2025-03-29 12:00:01+00:00 [COMMAND_EXIT_CODE=\"0\"]";
        assert_eq!(
            terminal_last_output_line(transcript, &[]),
            Some("terminal-ok".to_string())
        );
    }

    #[test]
    fn terminal_last_output_line_ignores_prompt_wrapped_inputs_and_exit() {
        let transcript = "pwd\nprintf \"terminal-foundation-ok\\n\"\nbash-5.2$ pwd\n/home/azureuser/src/Simard\nbash-5.2$ printf \"terminal-foundation-ok\\n\"\nterminal-foundation-ok\nbash-5.2$ exit";
        let steps = vec![
            TerminalStep::Input("pwd".to_string()),
            TerminalStep::Input("printf \"terminal-foundation-ok\\n\"".to_string()),
        ];
        assert_eq!(
            terminal_last_output_line(transcript, &steps),
            Some("terminal-foundation-ok".to_string())
        );
    }

    #[test]
    fn compact_terminal_evidence_value_replaces_newlines_and_truncates() {
        let raw = "line1\nline2\tline3";
        assert_eq!(compact_terminal_evidence_value(raw, 12), "line1\\nline2...");
    }
}
