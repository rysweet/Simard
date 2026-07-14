//! `gh` CLI abstraction. The trait keeps stewardship logic testable; the
//! [`RealGhClient`] subprocess implementation is the only network-touching
//! surface in this module.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::process::{Command, Output, Stdio};

use crate::error::{SimardError, SimardResult};

/// A GitHub issue as observed via `gh issue list` / `gh issue view`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhIssue {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
}

/// Abstract `gh` operations needed by the stewardship loop.
pub trait GhClient {
    /// Search **open** issues in `repo` whose body contains
    /// `stewardship-signature:<signature>`.
    fn search_issues(&self, repo: &str, signature: &str) -> SimardResult<Vec<GhIssue>>;
    /// Create a new issue in `repo`.
    fn create_issue(&self, repo: &str, title: &str, body: &str) -> SimardResult<GhIssue>;
}

/// Production implementation that shells out to the `gh` binary.
#[derive(Default)]
pub struct RealGhClient;

type CreateIssueExecutor =
    fn(&OsStr, &[&OsStr], &[u8]) -> Result<Output, CreateIssueExecutionError>;

#[derive(Debug)]
enum CreateIssueExecutionError {
    Spawn(io::Error),
    Write {
        source: io::Error,
        wait: Option<io::Error>,
    },
    Wait(io::Error),
}

impl RealGhClient {
    pub fn new() -> Self {
        Self
    }
}

fn create_issue_with(
    executable: &OsStr,
    executor: CreateIssueExecutor,
    repo: &str,
    title: &str,
    body: &str,
) -> SimardResult<GhIssue> {
    let args = [
        OsStr::new("issue"),
        OsStr::new("create"),
        OsStr::new("-R"),
        OsStr::new(repo),
        OsStr::new("--title"),
        OsStr::new(title),
        OsStr::new("--body-file"),
        OsStr::new("-"),
    ];
    let output = executor(executable, &args, body.as_bytes()).map_err(|error| {
        SimardError::StewardshipGhCommandFailed {
            reason: create_issue_execution_reason(error),
        }
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = if stderr.is_empty() {
            format!("`gh issue create -R {repo}` exited {}", output.status)
        } else {
            format!(
                "`gh issue create -R {repo}` exited {} with stderr:\n{stderr}",
                output.status
            )
        };
        return Err(SimardError::StewardshipGhCommandFailed { reason });
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let number: u64 = url
        .rsplit('/')
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| SimardError::StewardshipGhCommandFailed {
            reason: format!("`gh issue create` returned non-URL output: {url:?}"),
        })?;
    Ok(GhIssue {
        number,
        url,
        title: title.to_string(),
        body: body.to_string(),
    })
}

fn execute_create_issue(
    executable: &OsStr,
    args: &[&OsStr],
    body: &[u8],
) -> Result<Output, CreateIssueExecutionError> {
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(CreateIssueExecutionError::Spawn)?;

    let write_result = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(body),
        None => Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "piped stdin was unavailable",
        )),
    };
    if let Err(source) = write_result {
        let wait = child.wait_with_output().err();
        return Err(CreateIssueExecutionError::Write { source, wait });
    }

    child
        .wait_with_output()
        .map_err(CreateIssueExecutionError::Wait)
}

fn create_issue_execution_reason(error: CreateIssueExecutionError) -> String {
    match error {
        CreateIssueExecutionError::Spawn(error) => {
            format!("failed to spawn `gh issue create`: {error}")
        }
        CreateIssueExecutionError::Write { source, wait: None } => {
            format!("failed to write issue body to `gh issue create` stdin: {source}")
        }
        CreateIssueExecutionError::Write {
            source,
            wait: Some(wait),
        } => format!(
            "failed to write issue body to `gh issue create` stdin: {source}; \
             additionally failed to wait for `gh issue create`: {wait}"
        ),
        CreateIssueExecutionError::Wait(error) => {
            format!("failed to wait for `gh issue create`: {error}")
        }
    }
}

impl GhClient for RealGhClient {
    fn search_issues(&self, repo: &str, signature: &str) -> SimardResult<Vec<GhIssue>> {
        let search = format!("stewardship-signature:{signature} in:body");
        let output = std::process::Command::new("gh")
            .args([
                "issue",
                "list",
                "-R",
                repo,
                "--state",
                "open",
                "--search",
                &search,
                "--json",
                "number,url,title,body",
            ])
            .output()
            .map_err(|e| SimardError::StewardshipGhCommandFailed {
                reason: format!("failed to spawn `gh issue list`: {e}"),
            })?;
        if !output.status.success() {
            return Err(SimardError::StewardshipGhCommandFailed {
                reason: format!(
                    "`gh issue list -R {repo}` exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        #[derive(serde::Deserialize)]
        struct RawIssue {
            number: u64,
            url: String,
            title: String,
            body: String,
        }
        let raws: Vec<RawIssue> = serde_json::from_slice(&output.stdout).map_err(|e| {
            SimardError::StewardshipGhCommandFailed {
                reason: format!("failed to parse `gh issue list` JSON: {e}"),
            }
        })?;
        Ok(raws
            .into_iter()
            .map(|r| GhIssue {
                number: r.number,
                url: r.url,
                title: r.title,
                body: r.body,
            })
            .collect())
    }

    fn create_issue(&self, repo: &str, title: &str, body: &str) -> SimardResult<GhIssue> {
        create_issue_with(OsStr::new("gh"), execute_create_issue, repo, title, body)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Output;

    use super::{CreateIssueExecutionError, create_issue_with, execute_create_issue};

    fn fake_gh(script_body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("gh");
        fs::write(&executable, format!("#!/bin/sh\nset -eu\n{script_body}\n")).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        (dir, executable)
    }

    #[test]
    fn create_issue_sends_large_body_byte_for_byte_through_stdin_only() {
        let script = r#"
dir=${0%/*}
printf '%s\n' "$@" > "$dir/argv"
cat > "$dir/stdin"
printf '%s\n' 'https://github.com/rysweet/Simard/issues/321'
"#;
        let (dir, executable) = fake_gh(script);
        let body = format!(
            "large-body-start\n{}\nlarge-body-end",
            "0123456789abcdef".repeat(256 * 1024)
        );

        let issue = create_issue_with(
            executable.as_os_str(),
            execute_create_issue,
            "rysweet/Simard",
            "[stewardship] Orchestrator failure",
            &body,
        )
        .unwrap();

        assert_eq!(issue.number, 321);
        assert_eq!(fs::read(dir.path().join("stdin")).unwrap(), body.as_bytes());
        let argv = fs::read_to_string(dir.path().join("argv")).unwrap();
        assert!(argv.contains("--title\n[stewardship] Orchestrator failure\n"));
        assert!(argv.contains("--body-file\n-\n"));
        assert!(!argv.contains("large-body-start"));
        assert!(!argv.contains("large-body-end"));
    }

    #[test]
    fn create_issue_reports_spawn_failure_without_body_content() {
        let body = "SECRET_BODY_MUST_NOT_APPEAR";
        let title = "SECRET_TITLE_MUST_NOT_APPEAR";

        let error = create_issue_with(
            OsStr::new("/definitely/missing/simard-test-gh"),
            execute_create_issue,
            "rysweet/Simard",
            title,
            body,
        )
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("failed to spawn `gh issue create`"),
            "{error}"
        );
        assert!(!error.contains(body));
        assert!(!error.contains(title));
    }

    #[test]
    fn create_issue_reports_nonzero_exit_and_stderr_without_body_content() {
        let script = r#"
cat >/dev/null
printf '%s\n' 'fake gh rejected the request' >&2
exit 23
"#;
        let (_dir, executable) = fake_gh(script);
        let body = "SECRET_BODY_MUST_NOT_APPEAR";
        let title = "SECRET_TITLE_MUST_NOT_APPEAR";

        let error = create_issue_with(
            executable.as_os_str(),
            execute_create_issue,
            "rysweet/Simard",
            title,
            body,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("exited"), "{error}");
        assert!(error.contains("fake gh rejected the request"), "{error}");
        assert!(!error.contains(body));
        assert!(!error.contains(title));
    }

    fn write_failure(
        _executable: &OsStr,
        _args: &[&OsStr],
        _body: &[u8],
    ) -> Result<Output, CreateIssueExecutionError> {
        Err(CreateIssueExecutionError::Write {
            source: io::Error::new(io::ErrorKind::BrokenPipe, "injected write failure"),
            wait: Some(io::Error::other("injected reap failure")),
        })
    }

    #[test]
    fn create_issue_reports_write_and_reap_failures_without_body_content() {
        let body = "SECRET_BODY_MUST_NOT_APPEAR";
        let title = "SECRET_TITLE_MUST_NOT_APPEAR";

        let error = create_issue_with(
            OsStr::new("gh"),
            write_failure,
            "rysweet/Simard",
            title,
            body,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("failed to write issue body to `gh issue create` stdin"));
        assert!(error.contains("injected write failure"));
        assert!(error.contains("additionally failed to wait for `gh issue create`"));
        assert!(error.contains("injected reap failure"));
        assert!(!error.contains(body));
        assert!(!error.contains(title));
    }

    fn wait_failure(
        _executable: &OsStr,
        _args: &[&OsStr],
        _body: &[u8],
    ) -> Result<Output, CreateIssueExecutionError> {
        Err(CreateIssueExecutionError::Wait(io::Error::other(
            "injected wait failure",
        )))
    }

    #[test]
    fn create_issue_reports_wait_failure_without_body_content() {
        let body = "SECRET_BODY_MUST_NOT_APPEAR";
        let title = "SECRET_TITLE_MUST_NOT_APPEAR";

        let error = create_issue_with(
            OsStr::new("gh"),
            wait_failure,
            "rysweet/Simard",
            title,
            body,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("failed to wait for `gh issue create`"));
        assert!(error.contains("injected wait failure"));
        assert!(!error.contains(body));
        assert!(!error.contains(title));
    }
}
