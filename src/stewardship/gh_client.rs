//! `gh` CLI abstraction. The trait keeps stewardship logic testable; the
//! [`RealGhClient`] subprocess implementation is the only network-touching
//! surface in this module.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::process::{Command, Output, Stdio};

use crate::error::{SimardError, SimardResult};

use super::dedup::find_existing;

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

// ── Issue #4474: `ooda-stuck` label self-heal ──────────────────────────────
//
// The OODA no-progress breaker files a tracking issue via
// `gh issue create --label ooda-stuck` when a goal is Blocked. When the
// `ooda-stuck` label does not exist in the repo, `gh` exits non-zero and the
// whole escalation silently fails — a blocked goal can never reach the
// operator. The self-heal below idempotently ensures the label exists first
// (`gh label create`), and — crucially — never fails: a label that cannot be
// created (e.g. a token with issue-write but not label-write) DEGRADES to
// filing the issue *without* the label, so the escalation always proceeds.

/// The GitHub label the OODA no-progress breaker attaches to the tracking
/// issues it files for stalled / Blocked goals. Filing historically failed when
/// this label was absent from the repo; [`ensure_label`] self-heals it first.
pub(crate) const OODA_STUCK_LABEL: &str = "ooda-stuck";

/// Upper bound (bytes) on the `gh` stderr folded into a degrade reason or
/// failure note, so a hostile or runaway stderr cannot flood the logs.
pub(crate) const GH_STDERR_LOG_LIMIT: usize = 2048;

/// Truncate `text` to at most [`GH_STDERR_LOG_LIMIT`] bytes (on a UTF-8 char
/// boundary), appending a marker when truncated, so decoded `gh` stderr is
/// safe to fold into a log field without flooding the journal.
pub(crate) fn truncate_for_log(text: &str) -> String {
    if text.len() <= GH_STDERR_LOG_LIMIT {
        return text.to_string();
    }
    let mut end = GH_STDERR_LOG_LIMIT;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (truncated)", &text[..end])
}

/// Whether the `ooda-stuck` label may be attached to a `gh issue create`.
///
/// Returned by [`ensure_label`]; it is *never* an error, so the escalation
/// always proceeds — either with the label ([`Attach`](Self::Attach)) or,
/// when the label could not be ensured, without it
/// ([`Omit`](Self::Omit)) while surfacing the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LabelDisposition {
    /// The label exists (created just now, or already present): safe to pass
    /// `--label <name>`.
    Attach,
    /// The label could not be ensured; file the issue *without* it and surface
    /// `reason`. Degraded, but the escalation still succeeds.
    Omit { reason: String },
}

impl LabelDisposition {
    /// The `--label <name>` argv fragment when the label is ensured; empty when
    /// degraded — so a single argv builder can splice it unconditionally and
    /// stay the one source of truth for label handling across all filer sites.
    pub(crate) fn label_args<'a>(&self, label: &'a str) -> Vec<&'a str> {
        match self {
            LabelDisposition::Attach => vec!["--label", label],
            LabelDisposition::Omit { .. } => Vec::new(),
        }
    }
}

/// Injectable executor for `gh label create`, mirroring [`CreateIssueExecutor`].
/// Real code binds [`execute_ensure_label`]; tests inject subprocess outcomes.
/// `.output()` collapses spawn+wait into a single `io::Error`, so — unlike the
/// multi-stage [`CreateIssueExecutor`] — no dedicated error enum is warranted.
type LabelEnsureExecutor = fn(&OsStr, &[&OsStr]) -> Result<Output, io::Error>;

/// Core, executor-injected label self-heal: run `gh label create <label>` and
/// map the outcome to a [`LabelDisposition`]. NEVER returns an error — a
/// failure to create the label degrades to [`LabelDisposition::Omit`] so the
/// caller still files the escalation issue. A non-zero exit whose stderr says
/// the label already exists is the *idempotent success* path (case-insensitive,
/// since the `gh`/GitHub wording is not a stable contract).
fn ensure_label_with(
    executable: &OsStr,
    executor: LabelEnsureExecutor,
    label: &str,
) -> LabelDisposition {
    let args = [OsStr::new("label"), OsStr::new("create"), OsStr::new(label)];
    match executor(executable, &args) {
        Ok(output) if output.status.success() => LabelDisposition::Attach,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.to_lowercase().contains("already exists") {
                LabelDisposition::Attach
            } else {
                LabelDisposition::Omit {
                    reason: format!(
                        "`gh label create {label}` exited {}: {}",
                        output.status,
                        truncate_for_log(stderr.trim())
                    ),
                }
            }
        }
        Err(error) => LabelDisposition::Omit {
            reason: format!("failed to run `gh label create`: {error}"),
        },
    }
}

/// Real `gh label create` executor: spawn `gh`, capture stdout/stderr, no stdin
/// (a label create carries no body). `.output()` folds spawn+wait errors.
fn execute_ensure_label(executable: &OsStr, args: &[&OsStr]) -> Result<Output, io::Error> {
    Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
}

/// Idempotently ensure `label` exists in the ambient repo before a tracking
/// issue is filed, self-healing the missing-label failure that broke OODA
/// no-progress escalation (issue #4474). Never errors: an un-creatable label
/// degrades to [`LabelDisposition::Omit`], letting the caller file the issue
/// without the label so the blocked goal is still escalated.
///
/// Scoping is intentionally *ambient* (no `-R`): the label is created in the
/// same repo context the sibling `gh issue create` runs in, so the two can
/// never target different repos. See
/// `docs/concepts/ooda-stuck-label-self-heal.md`.
pub(crate) fn ensure_label(label: &str) -> LabelDisposition {
    ensure_label_with(OsStr::new("gh"), execute_ensure_label, label)
}

/// How many of the newest open issues to scan directly — via the
/// **strongly-consistent** `gh issue list` (no `--search`) — when the
/// eventually-consistent search index has not yet surfaced a just-filed
/// tracking issue. Bounds the cost of the lag-defeating fallback while
/// comfortably covering the tracking issues a single governed repo accrues
/// between two stewardship sweeps.
const RECENT_OPEN_ISSUE_SCAN_LIMIT: usize = 100;

/// A `gh issue list` variant used while resolving dedup candidates.
enum IssueListQuery {
    /// Full-text search of open-issue bodies for the stewardship signature.
    /// Fast and exact, but backed by GitHub's **eventually-consistent** issue
    /// search index — a tracking issue filed seconds/minutes ago may not be
    /// indexed yet.
    Signature(String),
    /// The newest open issues (up to the given limit), fetched via the
    /// **strongly-consistent** REST list (no `--search`). Used to catch a
    /// signed tracking issue the search index has not indexed yet.
    RecentOpen(usize),
}

/// Build the `gh issue list` argv for a dedup [`IssueListQuery`].
fn issue_list_args(repo: &str, query: &IssueListQuery) -> Vec<String> {
    let mut args = vec![
        "issue".to_string(),
        "list".to_string(),
        "-R".to_string(),
        repo.to_string(),
        "--state".to_string(),
        "open".to_string(),
    ];
    match query {
        IssueListQuery::Signature(signature) => {
            args.push("--search".to_string());
            args.push(format!("stewardship-signature:{signature} in:body"));
        }
        IssueListQuery::RecentOpen(limit) => {
            args.push("--limit".to_string());
            args.push(limit.to_string());
        }
    }
    args.push("--json".to_string());
    args.push("number,url,title,body".to_string());
    args
}

/// Parse `gh issue list --json number,url,title,body` output.
fn parse_issue_list(stdout: &[u8]) -> SimardResult<Vec<GhIssue>> {
    #[derive(serde::Deserialize)]
    struct RawIssue {
        number: u64,
        url: String,
        title: String,
        body: String,
    }
    let raws: Vec<RawIssue> =
        serde_json::from_slice(stdout).map_err(|e| SimardError::StewardshipGhCommandFailed {
            reason: format!("failed to parse `gh issue list` JSON: {e}"),
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

/// Union two issue-candidate lists, de-duplicating by issue number while
/// preserving order (search hits first, then any newest-open-issue hits the
/// search index had not yet indexed).
fn merge_issue_candidates(searched: Vec<GhIssue>, recent: Vec<GhIssue>) -> Vec<GhIssue> {
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    let mut merged = Vec::with_capacity(searched.len() + recent.len());
    for issue in searched.into_iter().chain(recent) {
        if seen.insert(issue.number) {
            merged.push(issue);
        }
    }
    merged
}

/// Resolve the open issues to dedup a signature against, resilient to GitHub's
/// eventually-consistent issue search index.
///
/// Strategy: run the fast full-text [`IssueListQuery::Signature`] search first.
/// If it already surfaces the signed issue, return its hits unchanged (the
/// common, already-indexed path — one `gh` call). Otherwise the tracking issue
/// may exist but not be indexed yet, so complement the (possibly empty) search
/// hits with a **strongly-consistent** [`IssueListQuery::RecentOpen`] scan and
/// union the two. Without this fallback, two stewardship sweeps within the
/// multi-minute search-index window each see an empty search and file a
/// duplicate, breaking the "one issue per distinct failure" guarantee.
///
/// `list` performs one `gh issue list` per query; any error it returns
/// propagates (fail-loud — a degraded search never silently yields "no match").
fn resolve_dedup_candidates<F>(mut list: F, signature: &str) -> SimardResult<Vec<GhIssue>>
where
    F: FnMut(&IssueListQuery) -> SimardResult<Vec<GhIssue>>,
{
    let searched = list(&IssueListQuery::Signature(signature.to_string()))?;
    if find_existing(&searched, signature).is_some() {
        return Ok(searched);
    }
    let recent = list(&IssueListQuery::RecentOpen(RECENT_OPEN_ISSUE_SCAN_LIMIT))?;
    Ok(merge_issue_candidates(searched, recent))
}

/// Shell out to `gh issue list` for one dedup [`IssueListQuery`] and parse it.
fn run_gh_issue_list(repo: &str, query: &IssueListQuery) -> SimardResult<Vec<GhIssue>> {
    let args = issue_list_args(repo, query);
    let output = Command::new("gh").args(&args).output().map_err(|e| {
        SimardError::StewardshipGhCommandFailed {
            reason: format!("failed to spawn `gh issue list`: {e}"),
        }
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
    parse_issue_list(&output.stdout)
}

impl GhClient for RealGhClient {
    fn search_issues(&self, repo: &str, signature: &str) -> SimardResult<Vec<GhIssue>> {
        resolve_dedup_candidates(|query| run_gh_issue_list(repo, query), signature)
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
    use super::{
        GhIssue, IssueListQuery, RECENT_OPEN_ISSUE_SCAN_LIMIT, issue_list_args,
        merge_issue_candidates, parse_issue_list, resolve_dedup_candidates,
    };
    use crate::error::SimardError;
    use std::cell::RefCell;

    fn issue(number: u64, signature: &str) -> GhIssue {
        GhIssue {
            number,
            url: format!("https://github.com/o/r/issues/{number}"),
            title: format!("[ci-health] wf-{number} failing"),
            body: format!("filed-by: simard-stewardship\nstewardship-signature: {signature}\nbody",),
        }
    }

    #[test]
    fn issue_list_args_uses_search_for_signature_query() {
        let args = issue_list_args("o/r", &IssueListQuery::Signature("cafef00dcafef00d".into()));
        assert!(args.windows(2).any(|w| w
            == [
                "--search".to_string(),
                "stewardship-signature:cafef00dcafef00d in:body".to_string()
            ]));
        assert!(!args.iter().any(|a| a == "--limit"));
        assert_eq!(
            args[0..6],
            ["issue", "list", "-R", "o/r", "--state", "open"]
        );
    }

    #[test]
    fn issue_list_args_uses_limit_and_no_search_for_recent_open_query() {
        let args = issue_list_args("o/r", &IssueListQuery::RecentOpen(42));
        assert!(!args.iter().any(|a| a == "--search"));
        assert!(
            args.windows(2)
                .any(|w| w == ["--limit".to_string(), "42".to_string()])
        );
    }

    #[test]
    fn merge_issue_candidates_dedups_by_number_preserving_search_first_order() {
        let searched = vec![issue(10, "aaaaaaaaaaaaaaaa")];
        let recent = vec![issue(10, "aaaaaaaaaaaaaaaa"), issue(9, "bbbbbbbbbbbbbbbb")];
        let merged = merge_issue_candidates(searched, recent);
        assert_eq!(
            merged.iter().map(|i| i.number).collect::<Vec<_>>(),
            vec![10, 9],
            "issue #10 must appear once (search hit), #9 appended from recent scan"
        );
    }

    #[test]
    fn parse_issue_list_reads_gh_json() {
        let json = br#"[{"number":7,"url":"u","title":"t","body":"b"}]"#;
        let issues = parse_issue_list(json).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 7);
    }

    /// A canned list executor recording which queries it was asked to run.
    struct FakeList {
        by_signature: Vec<GhIssue>,
        recent: Vec<GhIssue>,
        recent_err: bool,
        queries: RefCell<Vec<String>>,
    }

    impl FakeList {
        fn run(&self, query: &IssueListQuery) -> Result<Vec<GhIssue>, SimardError> {
            match query {
                IssueListQuery::Signature(_) => {
                    self.queries.borrow_mut().push("signature".into());
                    Ok(self.by_signature.clone())
                }
                IssueListQuery::RecentOpen(limit) => {
                    assert_eq!(*limit, RECENT_OPEN_ISSUE_SCAN_LIMIT);
                    self.queries.borrow_mut().push("recent".into());
                    if self.recent_err {
                        return Err(SimardError::StewardshipGhCommandFailed {
                            reason: "recent scan failed".into(),
                        });
                    }
                    Ok(self.recent.clone())
                }
            }
        }
    }

    #[test]
    fn resolve_skips_recent_scan_when_search_already_finds_signed_issue() {
        let sig = "cafef00dcafef00d";
        let fake = FakeList {
            by_signature: vec![issue(5, sig)],
            recent: vec![],
            recent_err: false,
            queries: RefCell::new(Vec::new()),
        };
        let out = resolve_dedup_candidates(|q| fake.run(q), sig).unwrap();
        assert_eq!(out.iter().map(|i| i.number).collect::<Vec<_>>(), vec![5]);
        assert_eq!(
            *fake.queries.borrow(),
            vec!["signature".to_string()],
            "recent scan must be skipped once search surfaces the signed issue"
        );
    }

    /// Regression for the search-index-lag duplicate-filing bug: a just-filed
    /// tracking issue is not yet in GitHub's search index, so the full-text
    /// search returns empty — but the strongly-consistent recent-open-issue
    /// scan surfaces it, so dedup finds the match instead of filing a duplicate.
    #[test]
    fn resolve_finds_signed_issue_via_recent_scan_when_search_index_lags() {
        let sig = "cafef00dcafef00d";
        let fake = FakeList {
            by_signature: vec![], // search index has not indexed it yet
            recent: vec![issue(9, "otherotherother0"), issue(8, sig)],
            recent_err: false,
            queries: RefCell::new(Vec::new()),
        };
        let out = resolve_dedup_candidates(|q| fake.run(q), sig).unwrap();
        assert!(
            super::find_existing(&out, sig).is_some(),
            "the unindexed signed issue must be found via the recent scan"
        );
        assert_eq!(
            *fake.queries.borrow(),
            vec!["signature".to_string(), "recent".to_string()],
            "an empty search must trigger the strongly-consistent recent scan"
        );
    }

    #[test]
    fn resolve_returns_no_match_when_neither_query_has_signature() {
        let sig = "cafef00dcafef00d";
        let fake = FakeList {
            by_signature: vec![],
            recent: vec![issue(9, "otherotherother0")],
            recent_err: false,
            queries: RefCell::new(Vec::new()),
        };
        let out = resolve_dedup_candidates(|q| fake.run(q), sig).unwrap();
        assert!(super::find_existing(&out, sig).is_none());
    }

    #[test]
    fn resolve_propagates_recent_scan_error_fail_loud() {
        let sig = "cafef00dcafef00d";
        let fake = FakeList {
            by_signature: vec![],
            recent: vec![],
            recent_err: true,
            queries: RefCell::new(Vec::new()),
        };
        let err = resolve_dedup_candidates(|q| fake.run(q), sig).unwrap_err();
        assert!(err.to_string().contains("recent scan failed"));
    }

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

    // ── ooda-stuck label self-heal (issue #4474) ───────────────────────────
    //
    // The no-progress breaker escalation filed `gh issue create --label
    // ooda-stuck`, but that label does not exist in rysweet/Simard, so `gh`
    // exited non-zero and the escalation silently failed. These tests pin the
    // contract of the fix: before filing, idempotently ensure the label exists
    // (`gh label create`), and if it cannot be ensured, DEGRADE to filing
    // without the label rather than failing — the escalation must always
    // succeed. Mirrors the injectable `CreateIssueExecutor` fn-pointer seam.

    use super::{LabelDisposition, ensure_label_with, execute_ensure_label};
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    fn label_create_succeeds(_executable: &OsStr, _args: &[&OsStr]) -> Result<Output, io::Error> {
        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }

    fn label_already_exists(_executable: &OsStr, _args: &[&OsStr]) -> Result<Output, io::Error> {
        // Mixed-case wording forces the idempotency match to be case-insensitive
        // (the `gh`/GitHub message casing is not guaranteed stable).
        Ok(Output {
            status: ExitStatus::from_raw(256),
            stdout: Vec::new(),
            stderr: b"HTTP 422: Validation Failed. Label \"ooda-stuck\" Already EXISTS".to_vec(),
        })
    }

    fn label_create_unauthorized(
        _executable: &OsStr,
        _args: &[&OsStr],
    ) -> Result<Output, io::Error> {
        Ok(Output {
            status: ExitStatus::from_raw(256),
            stdout: Vec::new(),
            stderr: b"HTTP 403: Resource not accessible by integration".to_vec(),
        })
    }

    fn label_spawn_fails(_executable: &OsStr, _args: &[&OsStr]) -> Result<Output, io::Error> {
        Err(io::Error::new(io::ErrorKind::NotFound, "no gh binary"))
    }

    fn label_huge_stderr(_executable: &OsStr, _args: &[&OsStr]) -> Result<Output, io::Error> {
        Ok(Output {
            status: ExitStatus::from_raw(256),
            stdout: Vec::new(),
            stderr: vec![b'x'; 100_000],
        })
    }

    #[test]
    fn ooda_stuck_label_constant_is_ooda_stuck() {
        assert_eq!(super::OODA_STUCK_LABEL, "ooda-stuck");
    }

    #[test]
    fn ensure_label_attaches_when_create_succeeds() {
        let disp = ensure_label_with(OsStr::new("gh"), label_create_succeeds, "ooda-stuck");
        assert_eq!(disp, LabelDisposition::Attach);
    }

    #[test]
    fn ensure_label_attaches_idempotently_when_label_already_exists() {
        // A non-zero exit whose stderr says the label already exists is the
        // idempotent success case — the label is present, so keep attaching it.
        let disp = ensure_label_with(OsStr::new("gh"), label_already_exists, "ooda-stuck");
        assert_eq!(
            disp,
            LabelDisposition::Attach,
            "an `already exists` failure must be treated as Attach (case-insensitive)",
        );
    }

    #[test]
    fn ensure_label_omits_with_reason_when_unauthorized() {
        // A token that can file issues may lack repo-write to create labels.
        // The fail-safe path degrades to Omit so the escalation still proceeds.
        let disp = ensure_label_with(OsStr::new("gh"), label_create_unauthorized, "ooda-stuck");
        match disp {
            LabelDisposition::Omit { reason } => assert!(
                reason.contains("403") || reason.to_lowercase().contains("not accessible"),
                "the degrade reason must surface the gh stderr, got {reason:?}",
            ),
            LabelDisposition::Attach => {
                panic!("unauthorized label creation must degrade to Omit, never Attach")
            }
        }
    }

    #[test]
    fn ensure_label_omits_when_spawn_fails_and_never_errs() {
        // No silent fallback, no propagated error: a spawn failure becomes an
        // observable Omit so the caller still files the issue without the label.
        let disp = ensure_label_with(OsStr::new("gh"), label_spawn_fails, "ooda-stuck");
        assert!(
            matches!(disp, LabelDisposition::Omit { .. }),
            "a spawn failure must degrade to Omit, not panic or Err",
        );
    }

    #[test]
    fn ensure_label_omit_reason_is_bounded_for_huge_stderr() {
        // Truncate the degrade reason so a hostile/huge stderr cannot flood logs.
        let disp = ensure_label_with(OsStr::new("gh"), label_huge_stderr, "ooda-stuck");
        match disp {
            LabelDisposition::Omit { reason } => assert!(
                reason.len() <= 4096,
                "omit reason must be bounded to prevent log flooding, was {}",
                reason.len(),
            ),
            LabelDisposition::Attach => panic!("a hard failure must degrade to Omit"),
        }
    }

    #[test]
    fn label_args_present_on_attach_absent_on_omit() {
        assert_eq!(
            LabelDisposition::Attach.label_args("ooda-stuck"),
            vec!["--label", "ooda-stuck"],
        );
        assert!(
            LabelDisposition::Omit {
                reason: "unauthorized".into(),
            }
            .label_args("ooda-stuck")
            .is_empty(),
        );
    }

    #[test]
    fn ensure_label_runs_gh_label_create_argv_via_real_executor() {
        // The real subprocess path must invoke `gh label create <label>` with no
        // issue body, and map a clean exit to Attach.
        let script = r#"
dir=${0%/*}
printf '%s\n' "$@" > "$dir/argv"
exit 0
"#;
        let (dir, executable) = fake_gh(script);
        let disp = ensure_label_with(executable.as_os_str(), execute_ensure_label, "ooda-stuck");
        assert_eq!(disp, LabelDisposition::Attach);
        let argv = fs::read_to_string(dir.path().join("argv")).unwrap();
        assert!(argv.contains("label\ncreate\n"), "argv was: {argv:?}");
        assert!(argv.contains("ooda-stuck"), "argv was: {argv:?}");
        assert!(
            !argv.contains("--body"),
            "`gh label create` must not carry an issue body",
        );
    }

    #[test]
    fn ensure_label_treats_real_already_exists_stderr_as_attach() {
        // End-to-end through the real executor: a non-zero exit whose stderr
        // reports the label already exists is idempotent success.
        let script = r#"
printf '%s\n' 'label already exists' >&2
exit 1
"#;
        let (_dir, executable) = fake_gh(script);
        let disp = ensure_label_with(executable.as_os_str(), execute_ensure_label, "ooda-stuck");
        assert_eq!(
            disp,
            LabelDisposition::Attach,
            "`already exists` from the real subprocess path is idempotent success",
        );
    }

    #[test]
    fn ensure_label_degrades_when_real_executor_reports_other_failure() {
        let script = r#"
printf '%s\n' 'HTTP 403: Resource not accessible by integration' >&2
exit 1
"#;
        let (_dir, executable) = fake_gh(script);
        let disp = ensure_label_with(executable.as_os_str(), execute_ensure_label, "ooda-stuck");
        assert!(
            matches!(disp, LabelDisposition::Omit { .. }),
            "a genuine failure from the real path must degrade to Omit",
        );
    }
}
