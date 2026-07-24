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

/// Maximum number of `gh` spawn attempts when the kernel returns `ETXTBSY`
/// (`Text file busy`, errno 26). The cap bounds the transient exec-vs-write
/// fork/exec race so a genuinely persistent failure still surfaces rather than
/// spinning forever (security S4).
const ETXTBSY_MAX_ATTEMPTS: usize = 8;

/// Constant backoff between `ETXTBSY` retries. A short synchronous sleep is
/// enough for a racing writer's file descriptor to close; it never busy-spins.
const ETXTBSY_RETRY_BACKOFF: Duration = Duration::from_millis(5);

/// Classify an [`io::Error`] as the transient `ETXTBSY` ("Text file busy")
/// spawn race, strictly by numeric errno. String matching is deliberately
/// avoided so the predicate stays locale-independent (security S2).
fn is_etxtbsy(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::ETXTBSY)
}

/// Run `op`, retrying **only** on transient `ETXTBSY` spawn failures up to
/// [`ETXTBSY_MAX_ATTEMPTS`] times with a constant [`ETXTBSY_RETRY_BACKOFF`].
///
/// Any `Ok`, and any `Err` that is not `ETXTBSY`, returns immediately — the
/// helper never masks real failures such as `ENOENT`/`EACCES` (security S3).
/// The retry path logs only the attempt index and numeric errno via
/// `tracing::debug!`; it never logs the command, args, body, or token
/// (security S1).
fn retry_on_etxtbsy<T, F>(mut op: F) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
{
    let mut attempt = 1usize;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(err) if is_etxtbsy(&err) && attempt < ETXTBSY_MAX_ATTEMPTS => {
                tracing::debug!(
                    attempt,
                    max_attempts = ETXTBSY_MAX_ATTEMPTS,
                    errno = err.raw_os_error(),
                    "retrying `gh` spawn after transient ETXTBSY"
                );
                attempt += 1;
                std::thread::sleep(ETXTBSY_RETRY_BACKOFF);
            }
            Err(err) => return Err(err),
        }
    }
}

fn execute_create_issue(
    executable: &OsStr,
    args: &[&OsStr],
    body: &[u8],
) -> Result<Output, CreateIssueExecutionError> {
    let mut child = retry_on_etxtbsy(|| {
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
        GhIssue, IssueListQuery, RECENT_OPEN_ISSUE_SCAN_LIMIT, is_etxtbsy, issue_list_args,
        merge_issue_candidates, parse_issue_list, resolve_dedup_candidates, retry_on_etxtbsy,
    };
    use crate::error::SimardError;
    use std::cell::{Cell, RefCell};

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

    // --- ETXTBSY classification + bounded-retry contract (PR #4523) ---------
    //
    // These tests specify the deterministic, hermetic fix for the `Text file
    // busy (os error 26)` fork/exec-vs-write race that flakes the `fake_gh`
    // spawn tests under parallel load. They are pure and subprocess-free: no
    // real `gh`, no real backoff observed by the assertions.

    /// The retry cap contracted by the design spec: 8 attempts total.
    const EXPECTED_MAX_ATTEMPTS: usize = 8;

    /// `is_etxtbsy` must classify strictly by numeric errno, never by string.
    #[test]
    fn is_etxtbsy_true_only_for_errno_26() {
        let etxtbsy = io::Error::from_raw_os_error(libc::ETXTBSY);
        assert_eq!(libc::ETXTBSY, 26, "ETXTBSY is errno 26 on Linux");
        assert!(is_etxtbsy(&etxtbsy), "errno 26 must classify as ETXTBSY");
    }

    /// Neighbouring spawn errnos must NOT be treated as ETXTBSY — they are real
    /// failures that must surface immediately (fail-loud, security S3).
    #[test]
    fn is_etxtbsy_false_for_other_spawn_errnos() {
        for errno in [libc::ENOENT, libc::EACCES, libc::EPERM, libc::ENOMEM] {
            let err = io::Error::from_raw_os_error(errno);
            assert!(
                !is_etxtbsy(&err),
                "errno {errno} must NOT classify as ETXTBSY"
            );
        }
    }

    /// A synthesized error with no OS errno (e.g. `io::Error::other`) must not
    /// be mistaken for ETXTBSY — guards against `raw_os_error() == None`.
    #[test]
    fn is_etxtbsy_false_for_non_os_error() {
        let err = io::Error::other("no raw os errno here");
        assert!(err.raw_os_error().is_none());
        assert!(
            !is_etxtbsy(&err),
            "non-OS error must not classify as ETXTBSY"
        );
    }

    /// On first-try success the op is invoked exactly once — no spurious
    /// retries, no latency on the happy path.
    #[test]
    fn retry_on_etxtbsy_returns_ok_without_retrying() {
        let calls = Cell::new(0usize);
        let result: io::Result<u32> = retry_on_etxtbsy(|| {
            calls.set(calls.get() + 1);
            Ok(42)
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.get(), 1, "success path must not retry");
    }

    /// Transient ETXTBSY failures are retried until the op succeeds; the return
    /// value of the successful attempt is surfaced.
    #[test]
    fn retry_on_etxtbsy_retries_transient_then_succeeds() {
        let calls = Cell::new(0usize);
        let result: io::Result<&str> = retry_on_etxtbsy(|| {
            let n = calls.get() + 1;
            calls.set(n);
            if n < 3 {
                Err(io::Error::from_raw_os_error(libc::ETXTBSY))
            } else {
                Ok("spawned")
            }
        });
        assert_eq!(result.unwrap(), "spawned");
        assert_eq!(calls.get(), 3, "should retry exactly until success");
    }

    /// A non-ETXTBSY error is surfaced immediately on the first attempt — the
    /// helper must never mask ENOENT/EACCES/etc. behind retries.
    #[test]
    fn retry_on_etxtbsy_surfaces_other_errors_immediately() {
        let calls = Cell::new(0usize);
        let result: io::Result<()> = retry_on_etxtbsy(|| {
            calls.set(calls.get() + 1);
            Err(io::Error::from_raw_os_error(libc::ENOENT))
        });
        let err = result.unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ENOENT));
        assert_eq!(calls.get(), 1, "non-ETXTBSY must not be retried");
    }

    /// Persistent ETXTBSY is bounded: the op is attempted exactly
    /// `EXPECTED_MAX_ATTEMPTS` times, then the last ETXTBSY error is returned
    /// (no infinite loop, no busy-spin — security S4).
    #[test]
    fn retry_on_etxtbsy_respects_attempt_cap() {
        let calls = Cell::new(0usize);
        let result: io::Result<()> = retry_on_etxtbsy(|| {
            calls.set(calls.get() + 1);
            Err(io::Error::from_raw_os_error(libc::ETXTBSY))
        });
        let err = result.unwrap_err();
        assert!(
            is_etxtbsy(&err),
            "final error must remain the ETXTBSY error"
        );
        assert_eq!(
            calls.get(),
            EXPECTED_MAX_ATTEMPTS,
            "persistent ETXTBSY must be attempted exactly {EXPECTED_MAX_ATTEMPTS} times"
        );
    }

    /// The retry wrapper is transparent to arbitrary success payloads and does
    /// not require `Clone`/`Copy` on the returned value.
    #[test]
    fn retry_on_etxtbsy_passes_through_owned_values() {
        let calls = RefCell::new(0usize);
        let result: io::Result<String> = retry_on_etxtbsy(|| {
            *calls.borrow_mut() += 1;
            if *calls.borrow() == 1 {
                Err(io::Error::from_raw_os_error(libc::ETXTBSY))
            } else {
                Ok(String::from("owned-output"))
            }
        });
        assert_eq!(result.unwrap(), "owned-output");
        assert_eq!(*calls.borrow(), 2);
    }
}
