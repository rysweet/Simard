//! Guarded `gh` issue mutation transport. [`RealGhClient`] is the only
//! network-touching surface in this module.

use crate::error::{SimardError, SimardResult};
use crate::stewardship::types::IssueMutationIdentity;

/// A GitHub issue as observed via `gh issue list` / `gh issue view`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GhIssue {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
}

/// Legacy search seam retained only for signature-normalization tests.
#[cfg(test)]
pub trait GhClient {
    /// Search **open** issues in `repo` whose body contains
    /// `stewardship-signature:<signature>`.
    fn search_issues(&self, repo: &str, signature: &str) -> SimardResult<Vec<GhIssue>>;
}

/// Low-level issue mutation transport. Production callers cannot use this
/// directly; [`crate::stewardship::mutation_guard::MutationGuard`] is the
/// authorization, persistence, idempotency, and budget boundary.
pub(crate) trait IssueMutationTransport {
    fn create_issue(
        &self,
        repo: &str,
        identity: &IssueMutationIdentity,
        title: &str,
        body: &str,
        labels: &[String],
        assignees: &[String],
    ) -> SimardResult<GhIssue>;
    fn edit_issue(
        &self,
        _repo: &str,
        _number: u64,
        _title: Option<&str>,
        _body: Option<&str>,
    ) -> SimardResult<GhIssue> {
        Err(SimardError::StewardshipGhCommandFailed {
            reason: "issue edit transport is not implemented".to_string(),
        })
    }
    fn close_issue(&self, _repo: &str, _number: u64) -> SimardResult<GhIssue> {
        Err(SimardError::StewardshipGhCommandFailed {
            reason: "issue close transport is not implemented".to_string(),
        })
    }
    fn reopen_issue(&self, _repo: &str, _number: u64) -> SimardResult<GhIssue> {
        Err(SimardError::StewardshipGhCommandFailed {
            reason: "issue reopen transport is not implemented".to_string(),
        })
    }
}

pub(crate) trait StewardshipGh: IssueMutationTransport {}

impl<T> StewardshipGh for T where T: IssueMutationTransport {}

/// Production implementation that shells out to the `gh` binary.
#[derive(Default)]
pub struct RealGhClient;

impl RealGhClient {
    pub fn new() -> Self {
        Self
    }
}

impl RealGhClient {
    fn run(args: &[String], action: &str) -> SimardResult<std::process::Output> {
        let output = std::process::Command::new("gh")
            .args(args)
            .output()
            .map_err(|e| SimardError::StewardshipGhCommandFailed {
                reason: format!("failed to spawn `gh {action}`: {e}"),
            })?;
        if !output.status.success() {
            let detail = if matches!(action, "issue create" | "issue edit") {
                "request details withheld".to_string()
            } else {
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            };
            return Err(SimardError::StewardshipGhCommandFailed {
                reason: format!("`gh {action}` exited {}: {detail}", output.status),
            });
        }
        Ok(output)
    }

    fn view_issue(repo: &str, number: u64) -> SimardResult<GhIssue> {
        let output = Self::run(
            &[
                "issue".to_string(),
                "view".to_string(),
                number.to_string(),
                "-R".to_string(),
                repo.to_string(),
                "--json".to_string(),
                "number,url,title,body".to_string(),
            ],
            "issue view",
        )?;
        serde_json::from_slice(&output.stdout).map_err(|e| {
            SimardError::StewardshipGhCommandFailed {
                reason: format!("failed to parse `gh issue view` JSON: {e}"),
            }
        })
    }
}

impl IssueMutationTransport for RealGhClient {
    fn create_issue(
        &self,
        repo: &str,
        identity: &IssueMutationIdentity,
        title: &str,
        body: &str,
        labels: &[String],
        assignees: &[String],
    ) -> SimardResult<GhIssue> {
        let body = format!(
            "{body}\n\nsimard-mutation-id: {}\nsimard-provenance: stewardship\n",
            identity.as_str()
        );
        let mut args = vec![
            "issue".to_string(),
            "create".to_string(),
            "-R".to_string(),
            repo.to_string(),
            "--title".to_string(),
            title.to_string(),
            "--body".to_string(),
            body.clone(),
        ];
        for label in labels {
            args.extend(["--label".to_string(), label.clone()]);
        }

        for assignee in assignees {
            args.extend(["--assignee".to_string(), assignee.clone()]);
        }
        let output = Self::run(&args, "issue create")?;
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

    fn edit_issue(
        &self,
        repo: &str,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
    ) -> SimardResult<GhIssue> {
        let mut args = vec![
            "issue".to_string(),
            "edit".to_string(),
            number.to_string(),
            "-R".to_string(),
            repo.to_string(),
        ];
        if let Some(title) = title {
            args.extend(["--title".to_string(), title.to_string()]);
        }
        if let Some(body) = body {
            args.extend(["--body".to_string(), body.to_string()]);
        }
        Self::run(&args, "issue edit")?;
        Self::view_issue(repo, number)
    }

    fn close_issue(&self, repo: &str, number: u64) -> SimardResult<GhIssue> {
        Self::run(
            &[
                "issue".to_string(),
                "close".to_string(),
                number.to_string(),
                "-R".to_string(),
                repo.to_string(),
            ],
            "issue close",
        )?;
        Self::view_issue(repo, number)
    }

    fn reopen_issue(&self, repo: &str, number: u64) -> SimardResult<GhIssue> {
        Self::run(
            &[
                "issue".to_string(),
                "reopen".to_string(),
                number.to_string(),
                "-R".to_string(),
                repo.to_string(),
            ],
            "issue reopen",
        )?;
        Self::view_issue(repo, number)
    }
}
