//! GitHub / cargo / git side-effect surface for the supply-chain steward
//! (issue #2741).
//!
//! Every network- or filesystem-mutating operation the execution layer needs is
//! behind the [`SupplyChainGh`] trait, so [`super::execute`] is fully
//! unit-testable against [`FakeSupplyChainGh`]. [`RealSupplyChainGh`] is the only
//! surface that shells out to `gh`, `cargo`, and `git`.
//!
//! Issue de-dup reuses [`crate::stewardship::GhIssue`]; green-CI-only self-merge
//! reuses [`crate::stewardship::merge_pr_if_merge_ready`] — the steward never
//! force-merges and never uses `--admin`/`--no-verify`.

use crate::error::{SimardError, SimardResult};
use crate::stewardship::{GhIssue, MergeOutcome};

/// Everything needed to open one remediation pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrSpec {
    /// Deterministic branch name (`chore/advisory-<id>`), so a re-run updates
    /// the same branch instead of opening a duplicate PR.
    pub branch: String,
    pub title: String,
    pub body: String,
    /// Labels to apply (e.g. `needs-CI-trigger` when no bot token is present).
    pub labels: Vec<String>,
}

/// The result of opening a remediation PR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenedPr {
    pub number: u32,
    pub url: String,
    /// Whether the PR was opened with a token that will trigger required CI. A
    /// PR whose CI cannot run must never be self-merged (fail-safe).
    pub ci_will_run: bool,
}

/// Abstract GitHub / cargo / git operations the remediation execution layer
/// drives. Kept minimal and mockable.
pub trait SupplyChainGh {
    /// Search **open** issues whose body embeds the dedup signature.
    fn search_issues(&self, signature: &str) -> SimardResult<Vec<GhIssue>>;
    /// File a tracking issue and return its handle (with URL + number).
    fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> SimardResult<GhIssue>;
    /// `cargo update -p <crate> --precise <to> --locked` — the minimal bump.
    fn cargo_update_precise(&self, crate_name: &str, to: &str) -> SimardResult<()>;
    /// Commit the working-tree changes on a branch, push, and open a PR.
    fn open_remediation_pr(&self, spec: &PrSpec) -> SimardResult<OpenedPr>;
    /// Green-CI-only self-merge of the steward's **own** PR. Refuses unless
    /// every required check passes (via the merge-authority rail).
    fn self_merge_if_green(&self, pr_number: u32) -> SimardResult<MergeOutcome>;
    /// Whether a bot token that triggers downstream CI is configured. When
    /// false the steward still opens the PR but never self-merges.
    fn has_ci_trigger_token(&self) -> bool;
}

/// Production implementation: shells out to `gh`, `cargo`, and `git`.
pub struct RealSupplyChainGh {
    repo: String,
}

impl RealSupplyChainGh {
    /// Construct against `repo` (an `owner/name` slug).
    pub fn new(repo: impl Into<String>) -> Self {
        Self { repo: repo.into() }
    }

    /// Resolve the repo slug from `STEWARD_REPO` (set by the workflow to
    /// `github.repository`), falling back to the Simard repo.
    pub fn from_env() -> Self {
        let repo = std::env::var("STEWARD_REPO")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "rysweet/Simard".to_string());
        Self::new(repo)
    }

    fn run(cmd: &str, args: &[&str], step: &str) -> SimardResult<std::process::Output> {
        let output = std::process::Command::new(cmd)
            .args(args)
            .output()
            .map_err(|e| SimardError::SupplyChainRemediationFailed {
                reason: format!("{step}: failed to spawn `{cmd}`: {e}"),
            })?;
        if !output.status.success() {
            return Err(SimardError::SupplyChainRemediationFailed {
                reason: format!(
                    "{step}: `{cmd} {}` exited {}: {}",
                    args.join(" "),
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        Ok(output)
    }
}

impl SupplyChainGh for RealSupplyChainGh {
    fn search_issues(&self, signature: &str) -> SimardResult<Vec<GhIssue>> {
        let search = format!("{signature} in:body");
        let output = Self::run(
            "gh",
            &[
                "issue",
                "list",
                "-R",
                &self.repo,
                "--state",
                "open",
                "--search",
                &search,
                "--json",
                "number,url,title,body",
            ],
            "search_issues",
        )?;
        #[derive(serde::Deserialize)]
        struct RawIssue {
            number: u64,
            url: String,
            title: String,
            body: String,
        }
        let raws: Vec<RawIssue> = serde_json::from_slice(&output.stdout).map_err(|e| {
            SimardError::SupplyChainRemediationFailed {
                reason: format!("search_issues: failed to parse `gh issue list` JSON: {e}"),
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

    fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> SimardResult<GhIssue> {
        let mut args = vec![
            "issue".to_string(),
            "create".to_string(),
            "-R".to_string(),
            self.repo.clone(),
            "--title".to_string(),
            title.to_string(),
            "--body".to_string(),
            body.to_string(),
        ];
        for label in labels {
            args.push("--label".to_string());
            args.push(label.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = Self::run("gh", &arg_refs, "create_issue")?;
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let number: u64 = url
            .rsplit('/')
            .next()
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| SimardError::SupplyChainRemediationFailed {
                reason: format!("create_issue: `gh issue create` returned non-URL output: {url:?}"),
            })?;
        Ok(GhIssue {
            number,
            url,
            title: title.to_string(),
            body: body.to_string(),
        })
    }

    fn cargo_update_precise(&self, crate_name: &str, to: &str) -> SimardResult<()> {
        Self::run(
            "cargo",
            &["update", "-p", crate_name, "--precise", to, "--locked"],
            "cargo_update_precise",
        )
        .map(|_| ())
    }

    fn open_remediation_pr(&self, spec: &PrSpec) -> SimardResult<OpenedPr> {
        Self::run("git", &["checkout", "-B", &spec.branch], "open_pr:branch")?;
        Self::run("git", &["add", "-A"], "open_pr:add")?;
        Self::run(
            "git",
            &[
                "-c",
                "user.name=simard-supply-chain-steward",
                "-c",
                "user.email=simard-steward@users.noreply.github.com",
                "commit",
                "-m",
                &spec.title,
            ],
            "open_pr:commit",
        )?;
        Self::run(
            "git",
            &["push", "-u", "origin", &spec.branch, "--force"],
            "open_pr:push",
        )?;

        let mut args = vec![
            "pr".to_string(),
            "create".to_string(),
            "-R".to_string(),
            self.repo.clone(),
            "--head".to_string(),
            spec.branch.clone(),
            "--title".to_string(),
            spec.title.clone(),
            "--body".to_string(),
            spec.body.clone(),
        ];
        for label in &spec.labels {
            args.push("--label".to_string());
            args.push(label.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = Self::run("gh", &arg_refs, "open_pr:create")?;
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let number: u32 = url
            .rsplit('/')
            .next()
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| SimardError::SupplyChainRemediationFailed {
                reason: format!("open_pr: `gh pr create` returned non-URL output: {url:?}"),
            })?;
        Ok(OpenedPr {
            number,
            url,
            ci_will_run: self.has_ci_trigger_token(),
        })
    }

    fn self_merge_if_green(&self, pr_number: u32) -> SimardResult<MergeOutcome> {
        let client = crate::stewardship::RealPrGhClient;
        crate::stewardship::merge_pr_if_merge_ready(pr_number, &self.repo, &client)
    }

    fn has_ci_trigger_token(&self) -> bool {
        std::env::var("STEWARD_GH_TOKEN")
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false)
    }
}

// ─────────────────────────── test fake ───────────────────────────

/// In-memory [`SupplyChainGh`] for unit tests. Records an ordered operation log
/// so tests can assert the hard-rail ordering (issue **before** ignore write),
/// and returns caller-configured canned responses.
#[cfg(test)]
pub struct FakeSupplyChainGh {
    /// Issues returned by `search_issues` (dedup-match simulation).
    pub existing_issues: Vec<GhIssue>,
    /// Whether a CI-triggering token is present.
    pub has_token: bool,
    /// Whether `cargo_update_precise` succeeds.
    pub cargo_update_ok: bool,
    /// Outcome returned by `self_merge_if_green`.
    pub merge_outcome: MergeOutcome,
    /// Ordered log of operations, tagged by kind.
    pub log: std::cell::RefCell<Vec<String>>,
    /// Monotonic issue/PR number source.
    pub next_number: std::cell::Cell<u32>,
}

#[cfg(test)]
impl Default for FakeSupplyChainGh {
    fn default() -> Self {
        Self {
            existing_issues: Vec::new(),
            has_token: true,
            cargo_update_ok: true,
            merge_outcome: MergeOutcome::Merged {
                pr_number: 0,
                repo: "rysweet/Simard".to_string(),
            },
            log: std::cell::RefCell::new(Vec::new()),
            next_number: std::cell::Cell::new(100),
        }
    }
}

#[cfg(test)]
impl FakeSupplyChainGh {
    fn next(&self) -> u32 {
        let n = self.next_number.get();
        self.next_number.set(n + 1);
        n
    }
}

#[cfg(test)]
impl SupplyChainGh for FakeSupplyChainGh {
    fn search_issues(&self, signature: &str) -> SimardResult<Vec<GhIssue>> {
        self.log.borrow_mut().push(format!("search:{signature}"));
        Ok(self.existing_issues.clone())
    }

    fn create_issue(&self, title: &str, _body: &str, labels: &[String]) -> SimardResult<GhIssue> {
        self.log
            .borrow_mut()
            .push(format!("create_issue:{title}:[{}]", labels.join(",")));
        let number = self.next() as u64;
        Ok(GhIssue {
            number,
            url: format!("https://github.com/rysweet/Simard/issues/{number}"),
            title: title.to_string(),
            body: _body.to_string(),
        })
    }

    fn cargo_update_precise(&self, crate_name: &str, to: &str) -> SimardResult<()> {
        self.log
            .borrow_mut()
            .push(format!("cargo_update:{crate_name}@{to}"));
        if self.cargo_update_ok {
            Ok(())
        } else {
            Err(SimardError::SupplyChainRemediationFailed {
                reason: format!("cargo update -p {crate_name} --precise {to} failed (test)"),
            })
        }
    }

    fn open_remediation_pr(&self, spec: &PrSpec) -> SimardResult<OpenedPr> {
        self.log.borrow_mut().push(format!(
            "open_pr:{}:[{}]",
            spec.branch,
            spec.labels.join(",")
        ));
        let number = self.next();
        Ok(OpenedPr {
            number,
            url: format!("https://github.com/rysweet/Simard/pull/{number}"),
            ci_will_run: self.has_token,
        })
    }

    fn self_merge_if_green(&self, pr_number: u32) -> SimardResult<MergeOutcome> {
        self.log
            .borrow_mut()
            .push(format!("self_merge:{pr_number}"));
        Ok(self.merge_outcome.clone())
    }

    fn has_ci_trigger_token(&self) -> bool {
        self.has_token
    }
}
