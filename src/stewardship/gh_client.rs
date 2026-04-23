//! `gh` CLI abstraction. The trait keeps stewardship logic testable; the
//! [`RealGhClient`] subprocess implementation is the only network-touching
//! surface in this module.

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

impl RealGhClient {
    pub fn new() -> Self {
        Self
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
        let output = std::process::Command::new("gh")
            .args([
                "issue", "create", "-R", repo, "--title", title, "--body", body,
            ])
            .output()
            .map_err(|e| SimardError::StewardshipGhCommandFailed {
                reason: format!("failed to spawn `gh issue create`: {e}"),
            })?;
        if !output.status.success() {
            return Err(SimardError::StewardshipGhCommandFailed {
                reason: format!(
                    "`gh issue create -R {repo}` exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
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
}
