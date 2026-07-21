//! Problem 6 — **PR Backlog Triage** for `rysweet/Simard`.
//!
//! This module implements the least-privilege, poll-before-mutate triage
//! workflow specified in the `pr-triage-docs.md` design artifact and pinned by
//! `tests/pr_triage.rs`. It operates only on an explicit allow-list of
//! conflicting (DIRTY) PRs and assigns each one exactly one of three
//! dispositions:
//!
//!   * `rebased-and-green`     — conflicts resolved, required checks pass.
//!   * `closed-with-rationale` — obsolete/superseded; closed with a comment.
//!   * `triage-note`           — relevance unclear; comment posted, PR left open.
//!
//! Design constraints honoured here: additive / non-breaking; least privilege
//! (never `--admin` / `--force`, act only on the allow-list); poll live merge
//! state before every mutation (never trust a stale DIRTY label); serialize
//! shared OODA-core / overseer done-gate surfaces one-at-a-time; and emit
//! structured `tracing` (never stdout macros) so each critical step is
//! observable via spans/events.

use std::path::PathBuf;
use std::str::FromStr;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{SimardError, SimardResult};

/// The nine documented conflicting PRs, in requirement order.
pub const DEFAULT_ALLOWLIST: &[u64] = &[4351, 4346, 4334, 4324, 4319, 4303, 4296, 4269, 4230];

/// Output rendering for the triage report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(OutputFormat::Text),
            "json" => Ok(OutputFormat::Json),
            other => Err(format!(
                "unknown output format '{other}' (expected 'text' or 'json')"
            )),
        }
    }
}

/// Live GitHub `mergeable` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mergeable {
    Mergeable,
    Conflicting,
    Unknown,
}

/// Live GitHub `mergeStateStatus` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStateStatus {
    Clean,
    Dirty,
    Blocked,
    Behind,
    Unstable,
    Unknown,
}

/// Configuration for a triage run.
#[derive(Debug, Clone)]
pub struct TriageConfig {
    pub repo: String,
    pub prs: Vec<u64>,
    pub dry_run: bool,
    pub format: OutputFormat,
    pub serialize_globs: Vec<String>,
    pub worktrees_root: PathBuf,
}

impl Default for TriageConfig {
    fn default() -> Self {
        TriageConfig {
            repo: "rysweet/Simard".to_string(),
            prs: DEFAULT_ALLOWLIST.to_vec(),
            dry_run: false,
            format: OutputFormat::Text,
            // Done-gate surfaces rebased one-at-a-time: ooda_* and overseer.
            serialize_globs: vec!["src/ooda_*".to_string(), "src/overseer/*".to_string()],
            worktrees_root: PathBuf::from("./worktrees/"),
        }
    }
}

impl TriageConfig {
    /// Reject structurally invalid PR allow-lists (a PR number is 1-based).
    pub fn validate(&self) -> Result<(), String> {
        if let Some(bad) = self.prs.iter().find(|&&n| n == 0) {
            return Err(format!("invalid PR number {bad}: PR numbers are positive"));
        }
        Ok(())
    }

    /// Least privilege: the set to act on is EXACTLY the configured allow-list.
    /// Any DIRTY PR discovered outside the allow-list is surfaced as an
    /// operator-review candidate, never auto-promoted into the action set.
    pub fn action_set(&self, discovered: &[u64]) -> ActionSet {
        let to_act = self.prs.clone();
        let candidates: Vec<u64> = discovered
            .iter()
            .copied()
            .filter(|n| !self.prs.contains(n))
            .collect();
        debug!(
            allowlist = self.prs.len(),
            candidates = candidates.len(),
            "computed triage action set (least-privilege)"
        );
        ActionSet { to_act, candidates }
    }

    /// True when ANY of the given changed paths is a shared done-gate surface
    /// (ooda_* / overseer), which forces one-at-a-time serialization.
    pub fn is_serialized(&self, paths: &[&str]) -> bool {
        paths
            .iter()
            .any(|p| self.serialize_globs.iter().any(|g| glob_match(g, p)))
    }

    /// Whether this configuration will perform mutations. A dry run reports
    /// only and never mutates.
    pub fn will_mutate(&self) -> bool {
        !self.dry_run
    }
}

/// Minimal prefix/glob matcher: a `*` denotes "match the remaining suffix".
/// Patterns without `*` match on exact-or-prefix. Sufficient for the
/// done-gate surface set (`src/ooda_*`, `src/overseer/*`).
fn glob_match(pattern: &str, path: &str) -> bool {
    match pattern.find('*') {
        Some(idx) => path.starts_with(&pattern[..idx]),
        None => path == pattern || path.starts_with(pattern),
    }
}

/// Result of resolving the action set against discovered DIRTY PRs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionSet {
    pub to_act: Vec<u64>,
    pub candidates: Vec<u64>,
}

/// A live-polled view of a single PR's merge state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrState {
    pub number: u64,
    pub mergeable: Mergeable,
    pub merge_state_status: MergeStateStatus,
    pub checks_green: bool,
}

impl PrState {
    /// Needs a rebase/merge rescue when the LIVE state is conflicting/dirty.
    pub fn needs_rescue(&self) -> bool {
        matches!(self.mergeable, Mergeable::Conflicting)
            || matches!(self.merge_state_status, MergeStateStatus::Dirty)
    }

    /// Merge only when clean, mergeable, and all required checks are green.
    pub fn merge_eligible(&self) -> bool {
        matches!(self.mergeable, Mergeable::Mergeable)
            && matches!(self.merge_state_status, MergeStateStatus::Clean)
            && self.checks_green
    }
}

/// Poll-before-every-mutation guard: decisions must be taken from a freshly
/// polled state, never from a stale label cached earlier in the run.
#[derive(Debug, Clone, Copy)]
pub struct FreshPoll(PrState);

impl FreshPoll {
    /// Record a fresh live poll of the PR's merge state.
    pub fn poll(state: PrState) -> Self {
        debug!(
            pr = state.number,
            "fresh merge-state poll taken before mutation"
        );
        FreshPoll(state)
    }

    pub fn needs_rescue(&self) -> bool {
        self.0.needs_rescue()
    }

    pub fn merge_eligible(&self) -> bool {
        self.0.merge_eligible()
    }
}

/// A lightweight PR reference used for supersession grouping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    pub number: u64,
    pub title: String,
    pub created: NaiveDate,
}

/// Given a group of PRs, return `(close, favor)` pairs where every older PR
/// sharing an identical title is closed in favour of the newest one with that
/// title. Distinct titles never form a supersession group.
pub fn supersession_closures(group: &[PrRef]) -> Vec<(u64, u64)> {
    use std::collections::BTreeMap;

    let mut by_title: BTreeMap<&str, Vec<&PrRef>> = BTreeMap::new();
    for pr in group {
        by_title.entry(pr.title.as_str()).or_default().push(pr);
    }

    let mut closures = Vec::new();
    for (_title, mut prs) in by_title {
        if prs.len() < 2 {
            continue;
        }
        // Newest by creation date wins; break ties on the higher PR number.
        prs.sort_by(|a, b| a.created.cmp(&b.created).then(a.number.cmp(&b.number)));
        // Guarded by `prs.len() < 2` above, so `split_last` always yields `Some`;
        // pattern-matching keeps this panic-free per the module's zero-panic guarantee.
        if let Some((newest, older_prs)) = prs.split_last() {
            for older in older_prs {
                closures.push((older.number, newest.number));
            }
        }
    }
    closures.sort();
    debug!(pairs = closures.len(), "computed supersession closures");
    closures
}

/// Build a least-privilege merge command for a PR. Never bypasses branch
/// protection (`--admin`) and never force-merges (`-f`/`--force`).
pub fn merge_command(pr: u64) -> Vec<String> {
    debug!(pr, "building non-privileged merge command");
    vec![
        "gh".to_string(),
        "pr".to_string(),
        "merge".to_string(),
        pr.to_string(),
        "--squash".to_string(),
    ]
}

/// Build a close command that always records an auditable, non-empty rationale
/// comment. Closing without a rationale is rejected.
pub fn close_command(pr: u64, rationale: &str) -> Result<Vec<String>, String> {
    if rationale.trim().is_empty() {
        return Err(format!(
            "refusing to close PR #{pr} without a rationale comment (audit requirement)"
        ));
    }
    debug!(pr, "building close command with auditable rationale");
    Ok(vec![
        "gh".to_string(),
        "pr".to_string(),
        "close".to_string(),
        pr.to_string(),
        "--comment".to_string(),
        rationale.to_string(),
    ])
}

/// The single disposition assigned to a triaged PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    RebasedAndGreen,
    ClosedWithRationale {
        superseded_by: Option<u64>,
        reason: String,
    },
    TriageNote {
        reason: String,
    },
}

impl Disposition {
    /// Canonical action string used in both text and JSON output.
    pub fn action_str(&self) -> &'static str {
        match self {
            Disposition::RebasedAndGreen => "rebased-and-green",
            Disposition::ClosedWithRationale { .. } => "closed-with-rationale",
            Disposition::TriageNote { .. } => "triage-note",
        }
    }
}

/// One row of the triage report: a PR, its disposition, and a human detail.
#[derive(Debug, Clone)]
pub struct DispositionRecord {
    pub pr: u64,
    pub disposition: Disposition,
    pub detail: String,
}

impl Serialize for DispositionRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("DispositionRecord", 3)?;
        st.serialize_field("pr", &self.pr)?;
        st.serialize_field("action", self.disposition.action_str())?;
        st.serialize_field("detail", &self.detail)?;
        st.end()
    }
}

/// A non-code operator escalation recorded (never dropped) by the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Escalation {
    pub id: String,
    pub kind: String,
    pub detail: String,
}

/// The two operational escalations that this triage cannot fix in code:
/// Problem 3 (release adoption) and Problem 4 (overseer cadence).
pub fn default_escalations() -> Vec<Escalation> {
    vec![
        Escalation {
            id: "problem-3".to_string(),
            kind: "release-adoption".to_string(),
            detail: "Simard runs 0.31.0 while 0.33.1 is released; upgrade the running \
                     deployment (honour the operator's 'Auto Update: ask' preference)."
                .to_string(),
        },
        Escalation {
            id: "problem-4".to_string(),
            kind: "overseer-cadence".to_string(),
            detail: "Overseer cadence appears stalled (missed ticks); a scheduler/liveness \
                     action is required on the running Overseer, not a repo change."
                .to_string(),
        },
    ]
}

/// The full triage report: per-PR dispositions plus recorded escalations.
#[derive(Debug, Clone, Serialize)]
pub struct TriageReport {
    pub repo: String,
    pub dispositions: Vec<DispositionRecord>,
    pub escalations: Vec<Escalation>,
}

impl TriageReport {
    /// Render the report as a human-readable text table. The text and JSON
    /// forms represent the same feature, so escalations appear in both.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("PR Backlog Triage — {}\n", self.repo));
        out.push_str("PR       ACTION                  DETAIL\n");
        for rec in &self.dispositions {
            out.push_str(&format!(
                "#{:<6}  {:<22}  {}\n",
                rec.pr,
                rec.disposition.action_str(),
                rec.detail
            ));
        }
        out.push_str("\nEscalations (operator action required):\n");
        for esc in &self.escalations {
            out.push_str(&format!("  [{}] {} — {}\n", esc.id, esc.kind, esc.detail));
        }
        out
    }

    /// Render according to the configured output format.
    pub fn render(&self, format: OutputFormat) -> Result<String, String> {
        match format {
            OutputFormat::Text => Ok(self.to_text()),
            OutputFormat::Json => {
                serde_json::to_string_pretty(self).map_err(|e| format!("JSON render failed: {e}"))
            }
        }
    }
}

// ===========================================================================
// External service integration — live GitHub (`gh` CLI) adapter.
//
// The pure logic above is hermetic; this section is the ONLY network-touching
// surface of the triage feature. It mirrors the repo's established pattern
// (`stewardship::gh_client`, `stewardship::merge_authority`): a small trait for
// testability, a subprocess implementation, per-subsystem error variant, and
// bounded retry with linear backoff for *idempotent reads only* — mutations
// never retry so a squash-merge or close can never be double-applied.
// ===========================================================================

/// Max retry attempts for *transient* `gh` read failures (network blips,
/// GitHub 5xx, secondary rate limits). Mutations are excluded on purpose.
const GH_READ_MAX_RETRIES: u32 = 3;

/// Base backoff (milliseconds) between transient `gh` read retries, scaled
/// linearly by attempt number so repeated rate-limit hits back off further.
const GH_RETRY_BACKOFF_MS: u64 = 500;

impl Mergeable {
    /// Map GitHub's `mergeable` enum string. Unknown / missing values are the
    /// conservative `Unknown` so callers never treat an unresolved PR as safe.
    pub fn from_gh(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "MERGEABLE" => Mergeable::Mergeable,
            "CONFLICTING" => Mergeable::Conflicting,
            _ => Mergeable::Unknown,
        }
    }
}

impl MergeStateStatus {
    /// Map GitHub's `mergeStateStatus` enum string. Anything unrecognised
    /// (`HAS_HOOKS`, `DRAFT`, future states) maps to `Unknown`.
    pub fn from_gh(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "CLEAN" => MergeStateStatus::Clean,
            "DIRTY" => MergeStateStatus::Dirty,
            "BLOCKED" => MergeStateStatus::Blocked,
            "BEHIND" => MergeStateStatus::Behind,
            "UNSTABLE" => MergeStateStatus::Unstable,
            _ => MergeStateStatus::Unknown,
        }
    }
}

/// One `statusCheckRollup` row. Check runs carry `status`/`conclusion`; status
/// contexts carry `state`. All three are optional so either shape deserializes.
#[derive(Debug, Clone, Deserialize)]
struct RollupEntry {
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    state: String,
}

impl RollupEntry {
    /// Normalise to a single terminal-state string, preferring the completed
    /// `conclusion`, then a status context's `state`, then the run `status`.
    fn normalized(&self) -> &str {
        for candidate in [&self.conclusion, &self.state, &self.status] {
            if !candidate.trim().is_empty() {
                return candidate.trim();
            }
        }
        ""
    }

    /// A check is passing when it terminated SUCCESS / NEUTRAL / SKIPPED.
    /// Pending, in-progress, failed, or unknown states are all non-passing.
    fn is_passing(&self) -> bool {
        matches!(
            self.normalized().to_ascii_uppercase().as_str(),
            "SUCCESS" | "NEUTRAL" | "SKIPPED"
        )
    }
}

/// Raw `gh pr view --json ...` document (only the fields we consume).
#[derive(Debug, Clone, Deserialize)]
struct RawPrView {
    number: u64,
    #[serde(default)]
    mergeable: String,
    #[serde(rename = "mergeStateStatus", default)]
    merge_state_status: String,
    #[serde(rename = "statusCheckRollup", default)]
    status_check_rollup: Vec<RollupEntry>,
}

impl RawPrView {
    /// Green iff no rollup entry is non-passing. An empty rollup is green
    /// (nothing failing) — `merge_eligible` still requires a CLEAN state.
    fn checks_green(&self) -> bool {
        self.status_check_rollup.iter().all(RollupEntry::is_passing)
    }
}

/// Parse a `gh pr view --json number,mergeable,mergeStateStatus,statusCheckRollup`
/// payload into a live [`PrState`]. Malformed JSON is a hard error.
pub fn parse_pr_state(json: &[u8]) -> SimardResult<PrState> {
    let raw: RawPrView =
        serde_json::from_slice(json).map_err(|e| SimardError::PrTriageGhCommandFailed {
            reason: format!("failed to parse `gh pr view` JSON: {e}"),
        })?;
    Ok(PrState {
        number: raw.number,
        mergeable: Mergeable::from_gh(&raw.mergeable),
        merge_state_status: MergeStateStatus::from_gh(&raw.merge_state_status),
        checks_green: raw.checks_green(),
    })
}

/// Abstract live-GitHub operations the triage runtime needs. Kept behind a
/// trait so the poll-before-mutate orchestration is testable without a network.
pub trait TriageGhClient {
    /// Freshly poll a PR's live merge state (idempotent read; retried on
    /// transient failures by the real implementation).
    fn poll_pr_state(&self, repo: &str, pr: u64) -> SimardResult<PrState>;

    /// Execute a pre-built least-privilege mutation command (as produced by
    /// [`merge_command`] / [`close_command`]). Returns stdout on success. Never
    /// retried — a squash-merge or close must not be double-applied.
    fn run_mutation(&self, argv: &[String]) -> SimardResult<String>;
}

/// Heuristic classifier: should a failed `gh` invocation be retried? Returns
/// `true` only for transient network / availability failures that typically
/// clear after a short backoff. Deterministic failures (auth, not-found,
/// malformed args) return `false` so they surface immediately.
fn is_transient_gh_failure(reason: &str) -> bool {
    const TRANSIENT_NEEDLES: [&str; 14] = [
        "429",
        "rate limit",
        "secondary rate",
        "502",
        "503",
        "504",
        "timed out",
        "timeout",
        "connection reset",
        "could not resolve host",
        "temporary failure",
        "try again",
        "tls handshake",
        "server error",
    ];
    let lower = reason.to_ascii_lowercase();
    TRANSIENT_NEEDLES
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Run an idempotent `gh` read closure, retrying transient failures with a
/// bounded linear backoff. Deterministic failures and the exhausted-retry case
/// both return the underlying error.
fn retry_transient_read<T>(op: &str, f: impl FnMut() -> SimardResult<T>) -> SimardResult<T> {
    retry_transient_read_inner(op, GH_READ_MAX_RETRIES, GH_RETRY_BACKOFF_MS, f)
}

/// Backoff-parameterized core of [`retry_transient_read`], split out so tests
/// can exercise the retry/give-up logic with a zero backoff (no real sleeping).
fn retry_transient_read_inner<T>(
    op: &str,
    max_retries: u32,
    backoff_ms: u64,
    mut f: impl FnMut() -> SimardResult<T>,
) -> SimardResult<T> {
    let mut attempt = 0u32;
    loop {
        match f() {
            Ok(value) => return Ok(value),
            Err(err) => {
                let transient = matches!(
                    &err,
                    SimardError::PrTriageGhCommandFailed { reason }
                        if is_transient_gh_failure(reason)
                );
                if !transient || attempt >= max_retries {
                    return Err(err);
                }
                attempt += 1;
                let delay = backoff_ms.saturating_mul(u64::from(attempt));
                tracing::warn!(
                    op,
                    attempt,
                    max_retries,
                    delay_ms = delay,
                    error = %err,
                    "pr-triage: transient gh read failure, backing off"
                );
                if delay > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                }
            }
        }
    }
}

/// Production [`TriageGhClient`] that shells out to the `gh` binary. This is the
/// single network-touching surface of the triage feature.
#[derive(Debug, Default, Clone)]
pub struct RealTriageGh;

impl RealTriageGh {
    pub fn new() -> Self {
        Self
    }

    /// Spawn `gh <args>` and return its stdout bytes on success, mapping every
    /// failure (spawn error, non-zero exit) onto [`SimardError::PrTriageGhCommandFailed`]
    /// so the transient classifier can inspect the reason string.
    fn run_gh(label: &str, args: &[&str]) -> SimardResult<Vec<u8>> {
        let output = std::process::Command::new("gh")
            .args(args)
            .output()
            .map_err(|e| SimardError::PrTriageGhCommandFailed {
                reason: format!("failed to spawn `{label}`: {e}"),
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::PrTriageGhCommandFailed {
                reason: format!("`{label}` exited {} with stderr:\n{stderr}", output.status),
            });
        }
        Ok(output.stdout)
    }
}

impl TriageGhClient for RealTriageGh {
    fn poll_pr_state(&self, repo: &str, pr: u64) -> SimardResult<PrState> {
        let pr_str = pr.to_string();
        let label = format!("gh pr view {pr} --repo {repo}");
        let stdout = retry_transient_read(&label, || {
            RealTriageGh::run_gh(
                &label,
                &[
                    "pr",
                    "view",
                    &pr_str,
                    "--repo",
                    repo,
                    "--json",
                    "number,mergeable,mergeStateStatus,statusCheckRollup",
                ],
            )
        })?;
        parse_pr_state(&stdout)
    }

    fn run_mutation(&self, argv: &[String]) -> SimardResult<String> {
        let mut parts = argv.iter();
        match parts.next().map(String::as_str) {
            Some("gh") => {}
            other => {
                return Err(SimardError::PrTriageGhCommandFailed {
                    reason: format!(
                        "refusing to run non-`gh` mutation command (first arg was {other:?})"
                    ),
                });
            }
        }
        // Least-privilege guard: never allow branch-protection bypass or force.
        // Normalise each arg to its flag name (before any `=value`) so the
        // `--admin=true` / `--force=true` forms cannot slip past exact-match.
        const DENIED_FLAGS: [&str; 3] = ["--admin", "-f", "--force"];
        if argv.iter().any(|a| {
            let flag = a.split('=').next().unwrap_or(a.as_str());
            DENIED_FLAGS.contains(&flag)
        }) {
            return Err(SimardError::PrTriageGhCommandFailed {
                reason: format!("refusing privileged/forced mutation: {argv:?}"),
            });
        }
        let rest: Vec<&str> = parts.map(String::as_str).collect();
        let label = format!("gh {}", rest.join(" "));
        debug!(command = %label, "executing least-privilege triage mutation (no retry)");
        let stdout = RealTriageGh::run_gh(&label, &rest)?;
        Ok(String::from_utf8_lossy(&stdout).trim().to_string())
    }
}

/// Outcome of a poll-before-mutate step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollThenMutate {
    /// Live state no longer justified the mutation; it was skipped safely.
    Skipped { state: PrState },
    /// Dry-run mode: the mutation was validated and logged but not applied.
    DryRun { state: PrState },
    /// The mutation ran; carries the fresh state it was taken from and stdout.
    Mutated { state: PrState, output: String },
}

/// Poll-before-mutate resilience wrapper: re-poll the PR's LIVE merge state,
/// then run `argv` only if `should_mutate` still agrees given the fresh poll.
/// This is what prevents acting on a stale DIRTY/CLEAN label cached earlier in
/// a run. In `dry_run` mode the mutation is validated but never applied.
pub fn poll_before_mutate<C, F>(
    client: &C,
    repo: &str,
    pr: u64,
    argv: &[String],
    dry_run: bool,
    should_mutate: F,
) -> SimardResult<PollThenMutate>
where
    C: TriageGhClient,
    F: Fn(&FreshPoll) -> bool,
{
    let state = client.poll_pr_state(repo, pr)?;
    let fresh = FreshPoll::poll(state);
    if !should_mutate(&fresh) {
        debug!(pr, "live state no longer justifies mutation; skipping");
        return Ok(PollThenMutate::Skipped { state });
    }
    if dry_run {
        debug!(pr, "dry-run: mutation validated but not applied");
        return Ok(PollThenMutate::DryRun { state });
    }
    let output = client.run_mutation(argv)?;
    Ok(PollThenMutate::Mutated { state, output })
}

#[cfg(test)]
mod external_integration_tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn parses_conflicting_dirty_pr_state() {
        let json = br#"{
            "number": 4351,
            "mergeable": "CONFLICTING",
            "mergeStateStatus": "DIRTY",
            "statusCheckRollup": [{"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"}]
        }"#;
        let state = parse_pr_state(json).expect("valid json");
        assert_eq!(state.number, 4351);
        assert_eq!(state.mergeable, Mergeable::Conflicting);
        assert_eq!(state.merge_state_status, MergeStateStatus::Dirty);
        assert!(state.checks_green);
        assert!(state.needs_rescue());
        assert!(!state.merge_eligible());
    }

    #[test]
    fn parses_clean_mergeable_green_state() {
        let json = br#"{
            "number": 4230,
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "CLEAN",
            "statusCheckRollup": [
                {"context": "ci/legacy", "state": "SUCCESS"},
                {"name": "lint", "status": "COMPLETED", "conclusion": "NEUTRAL"}
            ]
        }"#;
        let state = parse_pr_state(json).expect("valid json");
        assert!(state.merge_eligible());
        assert!(!state.needs_rescue());
    }

    #[test]
    fn pending_check_is_not_green() {
        let json = br#"{
            "number": 1,
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "UNSTABLE",
            "statusCheckRollup": [{"name": "build", "status": "IN_PROGRESS", "conclusion": ""}]
        }"#;
        let state = parse_pr_state(json).expect("valid json");
        assert!(!state.checks_green);
        assert!(!state.merge_eligible());
    }

    #[test]
    fn unknown_enum_strings_map_conservatively() {
        assert_eq!(Mergeable::from_gh("WHATEVER"), Mergeable::Unknown);
        assert_eq!(
            MergeStateStatus::from_gh("HAS_HOOKS"),
            MergeStateStatus::Unknown
        );
    }

    #[test]
    fn malformed_json_is_a_hard_error() {
        let err = parse_pr_state(b"not json").unwrap_err();
        assert!(matches!(err, SimardError::PrTriageGhCommandFailed { .. }));
    }

    #[test]
    fn transient_failures_are_retried_then_succeed() {
        let calls = RefCell::new(0u32);
        let result = retry_transient_read_inner("gh pr view", 3, 0, || {
            let mut n = calls.borrow_mut();
            *n += 1;
            if *n < 3 {
                Err(SimardError::PrTriageGhCommandFailed {
                    reason: "HTTP 503 server error".to_string(),
                })
            } else {
                Ok(42u32)
            }
        });
        assert_eq!(result.expect("eventually succeeds"), 42);
        assert_eq!(*calls.borrow(), 3);
    }

    #[test]
    fn deterministic_failures_are_not_retried() {
        let calls = RefCell::new(0u32);
        let result: SimardResult<u32> = retry_transient_read_inner("gh pr view", 5, 0, || {
            *calls.borrow_mut() += 1;
            Err(SimardError::PrTriageGhCommandFailed {
                reason: "authentication required".to_string(),
            })
        });
        assert!(result.is_err());
        assert_eq!(*calls.borrow(), 1, "deterministic failure must not loop");
    }

    /// In-memory client that returns a scripted state and records mutations.
    struct FakeTriageGh {
        state: PrState,
        mutations: RefCell<Vec<Vec<String>>>,
    }

    impl TriageGhClient for FakeTriageGh {
        fn poll_pr_state(&self, _repo: &str, _pr: u64) -> SimardResult<PrState> {
            Ok(self.state)
        }
        fn run_mutation(&self, argv: &[String]) -> SimardResult<String> {
            self.mutations.borrow_mut().push(argv.to_vec());
            Ok("merged".to_string())
        }
    }

    #[test]
    fn stale_dirty_but_live_clean_pr_merges_from_live_state() {
        let client = FakeTriageGh {
            state: PrState {
                number: 4230,
                mergeable: Mergeable::Mergeable,
                merge_state_status: MergeStateStatus::Clean,
                checks_green: true,
            },
            mutations: RefCell::new(Vec::new()),
        };
        let argv = merge_command(4230);
        let outcome = poll_before_mutate(&client, "rysweet/Simard", 4230, &argv, false, |f| {
            f.merge_eligible()
        })
        .expect("poll ok");
        assert!(matches!(outcome, PollThenMutate::Mutated { .. }));
        assert_eq!(client.mutations.borrow().len(), 1);
    }

    #[test]
    fn live_conflicting_pr_is_not_merged() {
        let client = FakeTriageGh {
            state: PrState {
                number: 4351,
                mergeable: Mergeable::Conflicting,
                merge_state_status: MergeStateStatus::Dirty,
                checks_green: false,
            },
            mutations: RefCell::new(Vec::new()),
        };
        let argv = merge_command(4351);
        let outcome = poll_before_mutate(&client, "rysweet/Simard", 4351, &argv, false, |f| {
            f.merge_eligible()
        })
        .expect("poll ok");
        assert!(matches!(outcome, PollThenMutate::Skipped { .. }));
        assert!(
            client.mutations.borrow().is_empty(),
            "no mutation on conflict"
        );
    }

    #[test]
    fn dry_run_validates_but_does_not_mutate() {
        let client = FakeTriageGh {
            state: PrState {
                number: 4230,
                mergeable: Mergeable::Mergeable,
                merge_state_status: MergeStateStatus::Clean,
                checks_green: true,
            },
            mutations: RefCell::new(Vec::new()),
        };
        let argv = merge_command(4230);
        let outcome = poll_before_mutate(&client, "rysweet/Simard", 4230, &argv, true, |f| {
            f.merge_eligible()
        })
        .expect("poll ok");
        assert!(matches!(outcome, PollThenMutate::DryRun { .. }));
        assert!(client.mutations.borrow().is_empty());
    }

    #[test]
    fn real_client_rejects_privileged_and_non_gh_commands() {
        let real = RealTriageGh::new();
        let forced = vec![
            "gh".to_string(),
            "pr".to_string(),
            "merge".to_string(),
            "1".to_string(),
            "--admin".to_string(),
        ];
        assert!(
            real.run_mutation(&forced).is_err(),
            "--admin must be refused"
        );
        let not_gh = vec!["curl".to_string(), "https://evil".to_string()];
        assert!(
            real.run_mutation(&not_gh).is_err(),
            "non-gh must be refused"
        );
        // The `--admin=true` flag form must not slip past the guard.
        let forced_kv = vec![
            "gh".to_string(),
            "pr".to_string(),
            "merge".to_string(),
            "1".to_string(),
            "--admin=true".to_string(),
        ];
        assert!(
            real.run_mutation(&forced_kv).is_err(),
            "--admin=true must be refused"
        );
    }
}
