//! `gh` CLI abstraction. The trait keeps stewardship logic testable; the
//! [`RealGhClient`] subprocess implementation is the only network-touching
//! surface in this module.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

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

/// Bounded retry budget for the `gh` fork+exec race (`ETXTBSY`). Kept
/// deliberately small: an unbounded retry on a genuinely-stuck exec would hang
/// the create-issue path (a self-inflicted DoS surface).
const SPAWN_MAX_ATTEMPTS: usize = 5;

/// Run `spawn` up to [`SPAWN_MAX_ATTEMPTS`] times, retrying **only** on
/// `ETXTBSY` ("text file busy", os error 26).
///
/// This closes the fork+exec race in which a sibling thread's
/// `write(executable)` is momentarily visible to `exec()` while another handle
/// to that just-written binary is still open. Every other error — missing
/// binary, `EACCES`, PATH hijack — surfaces immediately and unchanged on the
/// first attempt, so real faults are never masked or delayed. On exhaustion the
/// last (original) `ETXTBSY` error is returned verbatim: no fabricated error,
/// no silent success (fail-closed with the real cause).
fn spawn_with_etxtbsy_retry<T, F>(mut spawn: F) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
{
    let mut attempt = 1usize;
    loop {
        match spawn() {
            Ok(value) => return Ok(value),
            Err(error) => {
                let is_etxtbsy = error.raw_os_error() == Some(libc::ETXTBSY);
                if !is_etxtbsy || attempt >= SPAWN_MAX_ATTEMPTS {
                    return Err(error);
                }
                tracing::warn!(
                    attempt,
                    max_attempts = SPAWN_MAX_ATTEMPTS,
                    "gh spawn hit ETXTBSY (text file busy); retrying after backoff"
                );
                std::thread::sleep(Duration::from_millis(5 * attempt as u64));
                attempt += 1;
            }
        }
    }
}

fn execute_create_issue(
    executable: &OsStr,
    args: &[&OsStr],
    body: &[u8],
) -> Result<Output, CreateIssueExecutionError> {
    let mut child = spawn_with_etxtbsy_retry(|| {
        Command::new(executable)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    })
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
    #[serial_test::serial(gh_exec)]
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
    #[serial_test::serial(gh_exec)]
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

    // ── issue #4536: fork+exec ETXTBSY race — bounded, errno-gated spawn retry ──
    //
    // When sibling threads race a `write(executable)` + `Command::spawn()` of a
    // just-written binary, the exec can transiently fail with ETXTBSY ("text
    // file busy", os error 26) while another handle to that file is still open.
    // The production fix is a bounded retry that fires ONLY for ETXTBSY and
    // surfaces every other error immediately, so real faults — missing binary,
    // PATH hijack, EACCES — are never masked or delayed. These tests pin that
    // contract against `super::spawn_with_etxtbsy_retry` /
    // `super::SPAWN_MAX_ATTEMPTS`.
    use super::{SPAWN_MAX_ATTEMPTS, spawn_with_etxtbsy_retry};

    fn etxtbsy() -> io::Error {
        io::Error::from_raw_os_error(libc::ETXTBSY)
    }

    #[test]
    fn spawn_retry_bound_is_small_and_fixed() {
        // Documents the chosen budget. It MUST stay small: an unbounded retry on
        // a genuinely-stuck exec would hang the create-issue path (DoS surface).
        assert_eq!(SPAWN_MAX_ATTEMPTS, 5);
    }

    #[test]
    fn spawn_retry_succeeds_on_first_attempt_without_extra_calls() {
        // The happy path must not add any retries or delay: exactly one call.
        let mut calls = 0usize;
        let out: u32 = spawn_with_etxtbsy_retry(|| {
            calls += 1;
            Ok(42)
        })
        .expect("a clean spawn must return Ok");
        assert_eq!(out, 42);
        assert_eq!(calls, 1, "a clean spawn must not retry");
    }

    #[test]
    fn spawn_retry_recovers_after_transient_etxtbsy() {
        // ETXTBSY on every attempt but the last-allowed one, then success:
        // the helper must ride out the transient race and return Ok, having
        // used exactly the full budget.
        let mut calls = 0usize;
        let out: u32 = spawn_with_etxtbsy_retry(|| {
            calls += 1;
            if calls < SPAWN_MAX_ATTEMPTS {
                Err(etxtbsy())
            } else {
                Ok(7)
            }
        })
        .expect("a transient ETXTBSY that clears within budget must recover");
        assert_eq!(out, 7);
        assert_eq!(calls, SPAWN_MAX_ATTEMPTS);
    }

    #[test]
    fn spawn_retry_exhausts_and_returns_original_etxtbsy_error() {
        // Persistent ETXTBSY → exhaust the budget and surface the LAST error
        // unchanged (errno preserved). No fabricated/substituted error, no
        // silent success: fail-closed with the real cause.
        let mut calls = 0usize;
        let err = spawn_with_etxtbsy_retry::<u32, _>(|| {
            calls += 1;
            Err(etxtbsy())
        })
        .expect_err("persistent ETXTBSY must exhaust and surface an error");
        assert_eq!(
            calls, SPAWN_MAX_ATTEMPTS,
            "must attempt exactly SPAWN_MAX_ATTEMPTS times — no more, no fewer"
        );
        assert_eq!(
            err.raw_os_error(),
            Some(libc::ETXTBSY),
            "the surfaced error must be the original ETXTBSY, unchanged"
        );
    }

    #[test]
    fn spawn_retry_does_not_retry_non_etxtbsy_error() {
        // Correctness + security: a real failure (missing binary → ENOENT) must
        // surface on the FIRST attempt. Retrying it would both waste the budget
        // and mask/delay a genuine fault.
        let mut calls = 0usize;
        let err = spawn_with_etxtbsy_retry::<u32, _>(|| {
            calls += 1;
            Err(io::Error::from_raw_os_error(libc::ENOENT))
        })
        .expect_err("a non-ETXTBSY error must propagate immediately");
        assert_eq!(calls, 1, "non-ETXTBSY errors must never be retried");
        assert_eq!(
            err.raw_os_error(),
            Some(libc::ENOENT),
            "the propagated error must be the original, unmasked failure"
        );
    }

    #[test]
    fn spawn_retry_bound_is_an_upper_limit_not_off_by_one() {
        // ETXTBSY on the first SPAWN_MAX_ATTEMPTS calls, then a would-be success
        // on attempt MAX+1. The helper must have already given up: the budget is
        // an inclusive upper bound, guarding against an off-by-one that silently
        // allows an extra attempt.
        let mut calls = 0usize;
        let result = spawn_with_etxtbsy_retry::<u32, _>(|| {
            calls += 1;
            if calls <= SPAWN_MAX_ATTEMPTS {
                Err(etxtbsy())
            } else {
                Ok(99)
            }
        });
        assert!(
            result.is_err(),
            "helper must stop at SPAWN_MAX_ATTEMPTS, never reach the MAX+1 success"
        );
        assert_eq!(calls, SPAWN_MAX_ATTEMPTS);
    }

    /// Models `execute_create_issue` after the ETXTBSY retry budget is
    /// exhausted: the original spawn error is surfaced as `Spawn(..)`.
    fn etxtbsy_spawn_exhaustion(
        _executable: &OsStr,
        _args: &[&OsStr],
        _body: &[u8],
    ) -> Result<Output, CreateIssueExecutionError> {
        Err(CreateIssueExecutionError::Spawn(etxtbsy()))
    }

    #[test]
    fn create_issue_spawn_exhaustion_redacts_body_and_title() {
        // Even when spawn exhaustion surfaces as `Spawn`, the reported error
        // must carry the spawn-failure message and NEVER leak the issue body or
        // title (the existing redaction contract holds on the retry path too).
        let body = "SECRET_BODY_MUST_NOT_APPEAR";
        let title = "SECRET_TITLE_MUST_NOT_APPEAR";

        let error = create_issue_with(
            OsStr::new("gh"),
            etxtbsy_spawn_exhaustion,
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
}
