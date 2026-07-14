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
    fn repo(&self) -> &str;
    fn validate_repo_target(&self) -> SimardResult<()> {
        Ok(())
    }
    /// Search **open** issues whose body embeds the dedup signature.
    fn search_issues(&self, signature: &str) -> SimardResult<Vec<GhIssue>>;
    /// File a tracking issue and return its handle (with URL + number).
    fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> SimardResult<GhIssue>;
    /// `cargo update -p <crate> --precise <to>` — the minimal bump. `--locked`
    /// is deliberately NOT passed: it forbids Cargo from touching `Cargo.lock`,
    /// which is exactly what `--precise` must do, so the combination aborts with
    /// exit 101 on every real bump.
    fn cargo_update_precise(&self, crate_name: &str, to: &str) -> SimardResult<()>;
    /// Reset the working tree to the scan's pristine base commit so each
    /// advisory's remediation branch and commit contains ONLY its own change.
    /// The base (the default-branch checkout HEAD) is captured on the first
    /// call; subsequent calls discard any prior advisory's un-pushed local
    /// state. Idempotent.
    fn reset_to_scan_base(&self) -> SimardResult<()>;
    /// Prepare the local branch and commit. This does not contact GitHub.
    fn prepare_remediation_pr(&self, spec: &PrSpec) -> SimardResult<()>;
    /// Push the prepared branch to GitHub.
    fn push_remediation_branch(&self, branch: &str) -> SimardResult<()>;
    /// Create the GitHub PR for an already-pushed branch.
    fn create_remediation_pr(&self, spec: &PrSpec) -> SimardResult<OpenedPr>;
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
    /// The scan's pristine base commit (default-branch HEAD), captured lazily on
    /// the first `reset_to_scan_base` call — before any remediation commit — so
    /// every advisory branches from the same clean point.
    base: std::cell::OnceCell<String>,
}

impl RealSupplyChainGh {
    /// Construct against `repo` (an `owner/name` slug).
    pub fn new(repo: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            base: std::cell::OnceCell::new(),
        }
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

    /// The scan's pristine base commit SHA, captured once from `HEAD`. Because
    /// this is first read before any remediation commit is made, it pins the
    /// original default-branch checkout regardless of later branch/commit state.
    fn scan_base(&self) -> SimardResult<String> {
        if let Some(base) = self.base.get() {
            return Ok(base.clone());
        }
        let output = Self::run("git", &["rev-parse", "HEAD"], "scan_base")?;
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if sha.is_empty() {
            return Err(SimardError::SupplyChainRemediationFailed {
                reason: "scan_base: `git rev-parse HEAD` returned no SHA".to_string(),
            });
        }
        // First writer wins; single-threaded here, and any redundant set would
        // just re-store the same HEAD, so returning our own read is correct.
        let _ = self.base.set(sha.clone());
        Ok(sha)
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

    /// Run `gh <args>` and interpret its stdout as the URL of a freshly created
    /// issue or PR, returning `(url, trailing-number)`. `gh issue/pr create`
    /// prints the resource URL; its final path segment is the issue/PR number.
    fn gh_url_and_number<T>(args: &[String], step: &str) -> SimardResult<(String, T)>
    where
        T: std::str::FromStr,
    {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = Self::run("gh", &arg_refs, step)?;
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let number = url
            .rsplit('/')
            .next()
            .and_then(|n| n.parse::<T>().ok())
            .ok_or_else(|| SimardError::SupplyChainRemediationFailed {
                reason: format!("{step}: `gh` returned no trailing numeric id: {url:?}"),
            })?;
        Ok((url, number))
    }
}

/// Append `--label <l>` for each label to an already-built `gh` argument list.
fn push_labels(args: &mut Vec<String>, labels: &[String]) {
    for label in labels {
        args.push("--label".to_string());
        args.push(label.clone());
    }
}

pub(crate) fn remote_matches_repo(remote: &str, repo: &str) -> bool {
    let normalized = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    normalized == format!("https://github.com/{repo}")
        || normalized == format!("git@github.com:{repo}")
        || normalized == format!("ssh://git@github.com/{repo}")
}

impl SupplyChainGh for RealSupplyChainGh {
    fn repo(&self) -> &str {
        &self.repo
    }

    fn validate_repo_target(&self) -> SimardResult<()> {
        let remote = Self::run("git", &["remote", "get-url", "origin"], "verify-origin")?;
        let remote = String::from_utf8_lossy(&remote.stdout);
        if remote_matches_repo(&remote, &self.repo) {
            Ok(())
        } else {
            Err(SimardError::SupplyChainRemediationFailed {
                reason: format!(
                    "refusing GitHub mutation because origin does not match guarded repo {}",
                    self.repo
                ),
            })
        }
    }

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
        push_labels(&mut args, labels);
        let (url, number) = Self::gh_url_and_number::<u64>(&args, "create_issue")?;
        Ok(GhIssue {
            number,
            url,
            title: title.to_string(),
            body: body.to_string(),
        })
    }

    fn cargo_update_precise(&self, crate_name: &str, to: &str) -> SimardResult<()> {
        // NB: no `--locked` — it forbids the very `Cargo.lock` edit `--precise`
        // performs (cargo aborts exit 101 otherwise). This matches the
        // non-mutating `--dry-run` resolvability probe in the driver.
        Self::run(
            "cargo",
            &["update", "-p", crate_name, "--precise", to],
            "cargo_update_precise",
        )
        .map(|_| ())
    }

    fn reset_to_scan_base(&self) -> SimardResult<()> {
        let base = self.scan_base()?;
        Self::run("git", &["reset", "--hard", &base], "reset_to_scan_base")?;
        Ok(())
    }

    fn prepare_remediation_pr(&self, spec: &PrSpec) -> SimardResult<()> {
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
        Ok(())
    }

    fn push_remediation_branch(&self, branch: &str) -> SimardResult<()> {
        self.validate_repo_target()?;
        Self::run(
            "git",
            &["push", "-u", "origin", branch, "--force-with-lease"],
            "open_pr:push",
        )?;
        Ok(())
    }

    fn create_remediation_pr(&self, spec: &PrSpec) -> SimardResult<OpenedPr> {
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
        push_labels(&mut args, &spec.labels);
        let (url, number) = Self::gh_url_and_number::<u32>(&args, "open_pr:create")?;
        Ok(OpenedPr {
            number,
            url,
            ci_will_run: self.has_ci_trigger_token(),
        })
    }

    fn self_merge_if_green(&self, pr_number: u32) -> SimardResult<MergeOutcome> {
        Ok(MergeOutcome::Refused {
            pr_number,
            reason:
                "production supply-chain self-merge is disabled until approval is head-SHA-bound"
                    .to_string(),
        })
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
    fn repo(&self) -> &str {
        "rysweet/Simard"
    }

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

    fn reset_to_scan_base(&self) -> SimardResult<()> {
        self.log.borrow_mut().push("reset_to_scan_base".to_string());
        Ok(())
    }

    fn prepare_remediation_pr(&self, spec: &PrSpec) -> SimardResult<()> {
        self.log
            .borrow_mut()
            .push(format!("prepare_pr:{}", spec.branch));
        Ok(())
    }

    fn push_remediation_branch(&self, branch: &str) -> SimardResult<()> {
        self.log.borrow_mut().push(format!("push_branch:{branch}"));
        Ok(())
    }

    fn create_remediation_pr(&self, spec: &PrSpec) -> SimardResult<OpenedPr> {
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
