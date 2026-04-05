use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::error::{SimardError, SimardResult};

use super::types::TerminalStep;

enum WorkflowRestoreSnapshot {
    Missing,
    Present(Vec<u8>),
}

pub(crate) struct WorkflowRestoreGuard {
    path: PathBuf,
    snapshot: WorkflowRestoreSnapshot,
}

impl WorkflowRestoreGuard {
    pub(crate) fn capture(path: PathBuf, base_type: &str) -> SimardResult<Self> {
        let snapshot = match fs::read(&path) {
            Ok(contents) => WorkflowRestoreSnapshot::Present(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                WorkflowRestoreSnapshot::Missing
            }
            Err(error) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: base_type.to_string(),
                    reason: format!(
                        "failed to snapshot workflow-only file '{}' before terminal launch: {error}",
                        path.display()
                    ),
                });
            }
        };
        Ok(Self { path, snapshot })
    }

    fn restore(&self) {
        for _ in 0..5 {
            match &self.snapshot {
                WorkflowRestoreSnapshot::Present(contents) => {
                    if let Some(parent) = self.path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let current = fs::read(&self.path).ok();
                    if current.as_deref() != Some(contents.as_slice()) {
                        let _ = fs::write(&self.path, contents);
                    }
                }
                WorkflowRestoreSnapshot::Missing => {
                    if self.path.exists() {
                        let _ = fs::remove_file(&self.path);
                    }
                }
            }

            if self.matches_snapshot() {
                break;
            }

            thread::sleep(Duration::from_millis(100));
        }
    }

    fn matches_snapshot(&self) -> bool {
        match &self.snapshot {
            WorkflowRestoreSnapshot::Present(contents) => fs::read(&self.path)
                .map(|current| current == *contents)
                .unwrap_or(false),
            WorkflowRestoreSnapshot::Missing => !self.path.exists(),
        }
    }
}

impl Drop for WorkflowRestoreGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

pub(crate) fn capture_workflow_restore_guards(
    base_type: &str,
    launch_command: &str,
    working_directory: &Path,
) -> SimardResult<Vec<WorkflowRestoreGuard>> {
    if !is_amplihack_copilot_command(launch_command) {
        return Ok(Vec::new());
    }

    [
        ".claude/context/PROJECT.md",
        ".claude/context/PROJECT.md.bak",
    ]
    .into_iter()
    .map(|relative_path| {
        WorkflowRestoreGuard::capture(working_directory.join(relative_path), base_type)
    })
    .collect()
}

pub(crate) fn capture_workflow_restore_guards_for_steps(
    base_type: &str,
    steps: &[TerminalStep],
    working_directory: &Path,
) -> SimardResult<Vec<WorkflowRestoreGuard>> {
    if !steps.iter().any(|step| match step {
        TerminalStep::Input(command) => is_amplihack_copilot_command(command),
        TerminalStep::WaitFor(_) => false,
    }) {
        return Ok(Vec::new());
    }

    [
        ".claude/context/PROJECT.md",
        ".claude/context/PROJECT.md.bak",
    ]
    .into_iter()
    .map(|relative_path| {
        WorkflowRestoreGuard::capture(working_directory.join(relative_path), base_type)
    })
    .collect()
}

fn is_amplihack_copilot_command(launch_command: &str) -> bool {
    let mut parts = launch_command.split_whitespace();
    matches!(parts.next(), Some("amplihack")) && matches!(parts.next(), Some("copilot"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

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

        #[allow(dead_code)]
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
    fn detects_amplihack_copilot_commands_for_workflow_restore() {
        assert!(is_amplihack_copilot_command("amplihack copilot"));
        assert!(is_amplihack_copilot_command(
            "amplihack copilot -- --version"
        ));
        assert!(!is_amplihack_copilot_command("amplihack status"));
        assert!(!is_amplihack_copilot_command("printf 'amplihack copilot'"));
    }

    #[test]
    fn detects_amplihack_copilot_input_steps_for_workflow_restore() {
        let root = unique_test_path("workflow-restore-step-root");
        fs::create_dir(&root).unwrap();
        let _root_guard = TempPathGuard::directory(root.clone());

        let guards = capture_workflow_restore_guards_for_steps(
            "terminal-shell",
            &[
                TerminalStep::Input("amplihack copilot".to_string()),
                TerminalStep::Input("/exit".to_string()),
            ],
            &root,
        )
        .unwrap();
        assert_eq!(guards.len(), 2);

        let no_guards = capture_workflow_restore_guards_for_steps(
            "terminal-shell",
            &[TerminalStep::Input("printf ready\\n".to_string())],
            &root,
        )
        .unwrap();
        assert!(no_guards.is_empty());
    }

    #[test]
    fn workflow_restore_guard_restores_original_file_contents_on_drop() {
        let root = unique_test_path("workflow-restore-root");
        fs::create_dir(&root).unwrap();
        let _root_guard = TempPathGuard::directory(root.clone());
        let file = root.join("PROJECT.md");
        let _file_guard = TempPathGuard::file(file.clone());
        fs::write(&file, "original\n").unwrap();

        {
            let _guard = WorkflowRestoreGuard::capture(file.clone(), "terminal-shell").unwrap();
            fs::write(&file, "mutated\n").unwrap();
        }

        assert_eq!(fs::read_to_string(&file).unwrap(), "original\n");
    }

    #[test]
    fn workflow_restore_guard_removes_created_file_when_snapshot_was_missing() {
        let root = unique_test_path("workflow-restore-missing");
        fs::create_dir(&root).unwrap();
        let _root_guard = TempPathGuard::directory(root.clone());
        let file = root.join("PROJECT.md.bak");

        {
            let _guard = WorkflowRestoreGuard::capture(file.clone(), "terminal-shell").unwrap();
            fs::write(&file, "created-by-launcher\n").unwrap();
        }

        assert!(!file.exists());
    }
}
