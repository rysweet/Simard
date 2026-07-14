//! Exemplar 2 — [`EngineerLogAnalysisThread`]: the improvement finder
//! (design §9, security SR-1..SR-4, SR-8..SR-11).
//!
//! On a cadence it scans **recent, bounded** engineer/OODA telemetry under the
//! state root for recurring failure signatures and files a **deduplicated**
//! GitHub issue via the existing deterministic stewardship path. Its durable
//! artifact is a dedup'd issue (or, when `gh` is unavailable/dry-run,
//! structured telemetry) — never a repo snapshot doc. Behaviour bodies are
//! `todo!()` stubs during TDD; the config/type surface and the security-
//! critical `build_issue_*` seams are pinned by tests in `super::super::tests`.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use serde_json::json;

use crate::sanitization::sanitize_terminal_text;
use crate::stewardship::gh_client::{RealGhClient, StewardshipGh};
use crate::stewardship::mutation_guard::MutationGuard;
#[cfg(test)]
use crate::stewardship::mutation_store::MutationStore;
use crate::stewardship::{
    ArtifactProvenance, CycleId, IssueMutationIdentity, IssueMutationLimit, IssueMutationOutcome,
    IssueMutationRequest, LineageId,
};

use super::super::thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};

/// Stable telemetry id.
const ENGINEER_LOG_ANALYSIS_ID: &str = "engineer_log_analysis";

/// Number of times a failure signature must recur within the window before it
/// is treated as a durable finding (bounds noise; internal — not env-tunable).
pub(crate) const MIN_RECURRENCE: u32 = 2;

/// Tunables for [`EngineerLogAnalysisThread`] (all bounded — SR-8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineerLogAnalysisConfig {
    /// Cadence (`SIMARD_ENGINEER_LOG_ANALYSIS_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Target repo for issue filing (e.g. `"rysweet/Simard"`).
    pub repo: String,
    /// Bounded scan window in seconds (older telemetry is ignored).
    pub window_secs: u64,
    /// Hard cap on records scanned per run (SR-8).
    pub max_records: usize,
    /// Hard cap on findings emitted per run (SR-8).
    pub max_findings: usize,
    /// Suppress issue creation; emit structured telemetry only.
    pub dry_run: bool,
}

impl Default for EngineerLogAnalysisConfig {
    fn default() -> Self {
        Self {
            interval_secs: 6 * 60 * 60,
            repo: "rysweet/Simard".to_string(),
            window_secs: 7 * 24 * 60 * 60,
            max_records: 500,
            max_findings: 10,
            dry_run: false,
        }
    }
}

/// The engineer-log-analysis cognitive thread (exemplar 2).
pub struct EngineerLogAnalysisThread {
    cfg: EngineerLogAnalysisConfig,
    gh: Box<dyn StewardshipGh + Send>,
    guard: MutationGuard,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl EngineerLogAnalysisThread {
    /// Build from the environment using the real `gh`-backed client.
    pub fn from_env() -> Self {
        let mut cfg = EngineerLogAnalysisConfig::default();
        if let Some(v) = read_u64_env("SIMARD_ENGINEER_LOG_ANALYSIS_INTERVAL_SECS") {
            cfg.interval_secs = super::super::schedule::clamp_interval_secs(v);
        }
        if let Some(v) = read_bool_env("SIMARD_ENGINEER_LOG_ANALYSIS_DRY_RUN") {
            cfg.dry_run = v;
        }
        Self::with_client(cfg, Box::new(RealGhClient::new()))
    }

    /// Build from an explicit config with an injected [`GhClient`] (test seam —
    /// a fake client keeps tests offline and credential-free). The client must
    /// be `Send` so the thread satisfies [`CognitiveThread`]'s `Send` bound.
    pub(crate) fn with_client(
        cfg: EngineerLogAnalysisConfig,
        gh: Box<dyn StewardshipGh + Send>,
    ) -> Self {
        Self {
            cfg,
            gh,
            guard: {
                #[cfg(test)]
                {
                    let path = std::env::temp_dir()
                        .join(format!(
                            "simard-engineer-analysis-test-{}",
                            uuid::Uuid::new_v4()
                        ))
                        .join("mutations.json");
                    MutationGuard::new(MutationStore::new(path))
                }
                #[cfg(not(test))]
                {
                    MutationGuard::from_default_store()
                }
            },
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
        }
    }
}

impl CognitiveThread for EngineerLogAnalysisThread {
    fn id(&self) -> &str {
        ENGINEER_LOG_ANALYSIS_ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::EngineerLogAnalysis
    }

    fn policy(&self) -> SchedulePolicy {
        SchedulePolicy::Interval(std::time::Duration::from_secs(self.cfg.interval_secs))
    }

    fn priority(&self) -> Priority {
        Priority::Low
    }

    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        let start = Instant::now();
        let dry_run = self.cfg.dry_run || ctx.dry_run;

        // 1. Gather bounded, deduplicated failure signatures from recent
        //    persisted cycle reports (SR-8 bounds work + cost).
        let findings = collect_findings(
            ctx.state_root,
            ctx.now_epoch,
            self.cfg.window_secs,
            self.cfg.max_records,
        );

        let mut emitted = 0usize;
        let mut created = 0usize;
        let mut deduped = 0usize;
        let mut errors: Vec<String> = Vec::new();
        let cycle_id = match CycleId::scheduled("engineer-log-analysis") {
            Ok(cycle_id) => cycle_id,
            Err(error) => {
                return ThreadOutcome::failed(
                    format!("engineer-log-analysis cycle identity failed: {error}"),
                    start.elapsed(),
                );
            }
        };
        let mutation_limit = match IssueMutationLimit::configured() {
            Ok(limit) => limit,
            Err(error) => {
                return ThreadOutcome::failed(
                    format!("engineer-log-analysis mutation configuration failed: {error}"),
                    start.elapsed(),
                );
            }
        };
        if let Err(error) = self.guard.begin_cycle(cycle_id.clone(), mutation_limit) {
            return ThreadOutcome::failed(
                format!("engineer-log-analysis mutation cycle failed: {error}"),
                start.elapsed(),
            );
        }

        for (sig, finding) in findings.iter() {
            // Only recurring signatures are durable findings (bounds noise).
            if finding.count < MIN_RECURRENCE {
                continue;
            }
            if emitted >= self.cfg.max_findings {
                break;
            }
            emitted += 1;

            if dry_run {
                // Durable telemetry instead of an issue — never a repo doc.
                tracing::info!(
                    metric = "simard.thread.engineer_log_analysis.finding",
                    signature = %sig,
                    recurrence = finding.count,
                    "engineer-log-analysis finding (dry-run: not filed)"
                );
                continue;
            }

            let title = build_issue_title(sig, &finding.failure_kind);
            // `linewise` isolates inline `key=secret` pairs onto their own line
            // so `sanitize_terminal_text` (inside build_issue_body) redacts them
            // even when they are not at the start of the raw log line (SR-2).
            let body = build_issue_body(sig, &linewise(&finding.excerpt));
            let request = match IssueMutationRequest::create(
                &self.cfg.repo,
                IssueMutationIdentity::new(format!("engineer-log-analysis:{sig}"))
                    .expect("hex failure signature is a valid mutation identity"),
                ArtifactProvenance::system(
                    LineageId::new("engineer-log-analysis")
                        .expect("static lineage identity is valid"),
                ),
                &title,
                &body,
            ) {
                Ok(request) => request,
                Err(error) => {
                    errors.push(error.to_string());
                    break;
                }
            };
            match self.guard.execute(&cycle_id, &request, self.gh.as_ref()) {
                Ok(IssueMutationOutcome::Completed { issue }) => {
                    created += 1;
                    tracing::info!(
                        metric = "simard.thread.engineer_log_analysis.issue_filed",
                        signature = %sig,
                        number = issue.number,
                        "engineer-log-analysis filed deduplicated issue"
                    );
                }
                Ok(IssueMutationOutcome::AlreadyCompleted { .. }) => {
                    deduped += 1;
                }
                Err(e) => {
                    errors.push(format!("create failed: {e}"));
                    break;
                }
            }
        }

        let success = errors.is_empty();
        self.last_run_epoch = Some(ctx.now_epoch);
        self.next_run_epoch = super::super::schedule::next_run_epoch(
            &self.policy(),
            self.last_run_epoch,
            ctx.now_epoch,
        );
        self.last_success = Some(success);
        if success {
            self.consecutive_errors = 0;
        } else {
            self.consecutive_errors = self.consecutive_errors.saturating_add(1);
        }

        let summary = format!(
            "engineer-log-analysis: {emitted} recurring finding(s), {created} filed, \
             {deduped} deduped, {} error(s)",
            errors.len()
        );
        let detail = json!({
            "dry_run": dry_run,
            "findings": emitted,
            "created": created,
            "deduped": deduped,
            "errors": errors,
        });
        if success {
            ThreadOutcome::ok(summary, start.elapsed()).with_detail(detail)
        } else {
            ThreadOutcome::failed(summary, start.elapsed()).with_detail(detail)
        }
    }

    fn health(&self) -> ThreadHealth {
        ThreadHealth {
            id: ENGINEER_LOG_ANALYSIS_ID.to_string(),
            enabled: true,
            last_run_epoch: self.last_run_epoch,
            next_run_epoch: self.next_run_epoch,
            last_success: self.last_success,
            consecutive_errors: self.consecutive_errors,
            backoff_until_epoch: None,
        }
    }
}

/// Build the deduplicated issue title for a finding (SR-2/SR-11).
///
/// Contract: length-bounded; the `signature` is embedded so the title is
/// stable; the human-readable `failure_kind` excerpt is redacted via
/// [`crate::sanitization::sanitize_terminal_text`] before inclusion.
pub(crate) fn build_issue_title(signature: &str, failure_kind: &str) -> String {
    // The failure_kind can carry excerpted (untrusted) text — redact + bound it
    // and keep the title stable via the trusted signature.
    let kind = sanitize_terminal_text(failure_kind);
    let kind = kind.lines().next().unwrap_or("").trim();
    let kind = truncate_chars(kind, 80);
    let kind = if kind.is_empty() {
        "engineer failure"
    } else {
        kind.as_str()
    };
    format!("[cognitive-thread] recurring {kind} ({signature})")
}

/// Build the deduplicated issue body (SR-2/SR-3): sanitize + neutralize +
/// fence the untrusted excerpt, then append the trusted dedup marker exactly
/// once in a controlled location.
pub(crate) fn build_issue_body(signature: &str, excerpt: &str) -> String {
    // 1. Redact secrets + strip terminal control sequences (SR-2).
    let sanitized = sanitize_terminal_text(excerpt);
    // 2. Neutralize any dedup marker smuggled inside the untrusted excerpt so a
    //    spoofed `stewardship-signature: <sig>` cannot poison dedup (SR-3).
    let neutralized = neutralize_markers(&sanitized);
    // 3. Bound length, then escape fence break-outs so the excerpt stays inside
    //    its code block (which also stops GitHub auto-linking @mentions/#refs).
    let bounded = truncate_chars(neutralized.trim(), 4_000);
    let fenced = bounded.replace("```", "'''");

    format!(
        "## Cognitive-thread finding: recurring engineer failure\n\
         \n\
         The `engineer_log_analysis` cognitive thread observed this failure \
         signature recur across recent OODA cycles. Representative sanitized \
         excerpt (secrets redacted, fenced to suppress auto-links):\n\
         \n\
         ```text\n\
         {fenced}\n\
         ```\n\
         \n\
         <!-- deduplication marker — do not edit -->\n\
         stewardship-signature: {signature}\n"
    )
}

/// One representative record for a recurring failure signature.
struct Finding {
    count: u32,
    failure_kind: String,
    excerpt: String,
}

/// Scan up to `max_records` failed outcomes from the newest persisted cycle
/// reports under `state_root/cycle_reports/`, grouping them by failure
/// signature. Best-effort: unreadable/unparseable reports are skipped. The
/// scan is bounded (SR-8); `window_secs` additionally drops reports that carry
/// an explicit `cycle_start_epoch` older than the window.
fn collect_findings(
    state_root: &Path,
    now_epoch: u64,
    window_secs: u64,
    max_records: usize,
) -> BTreeMap<String, Finding> {
    let mut findings: BTreeMap<String, Finding> = BTreeMap::new();
    let dir = state_root.join("cycle_reports");
    let read = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return findings,
    };

    // Newest reports first so a bounded scan sees the most recent failures.
    let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = read
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| {
            let m = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (e.path(), m)
        })
        .collect();
    files.sort_by_key(|entry| std::cmp::Reverse(entry.1));

    let mut scanned = 0usize;
    for (path, _) in files {
        if scanned >= max_records {
            break;
        }
        let value: serde_json::Value = match std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
        {
            Some(v) => v,
            None => continue,
        };

        // Optional window filter — only applies when the report timestamps
        // itself; otherwise the report is considered in-window.
        if window_secs > 0
            && let Some(epoch) = value.get("cycle_start_epoch").and_then(|v| v.as_u64())
            && epoch != 0
            && now_epoch.saturating_sub(epoch) > window_secs
        {
            continue;
        }

        let Some(outcomes) = value.get("outcomes").and_then(|v| v.as_array()) else {
            continue;
        };
        for o in outcomes {
            if scanned >= max_records {
                break;
            }
            scanned += 1;
            if o.get("success").and_then(|v| v.as_bool()).unwrap_or(true) {
                continue;
            }
            let detail = o
                .get("detail")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if detail.is_empty() {
                continue;
            }
            let action_kind = o
                .get("action_kind")
                .or_else(|| o.pointer("/action/kind"))
                .and_then(|v| v.as_str())
                .unwrap_or("action");
            let goal_id = o
                .get("goal_id")
                .or_else(|| o.pointer("/action/goal_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("no-goal");
            let failure_kind = format!("engineer_failure:{action_kind}");
            let sig = IssueMutationIdentity::from_source(
                "engineer-log-condition",
                &format!("{failure_kind}\0{goal_id}"),
            )
            .as_str()
            .to_string();
            let entry = findings.entry(sig).or_insert_with(|| Finding {
                count: 0,
                failure_kind: failure_kind.clone(),
                excerpt: detail.to_string(),
            });
            entry.count = entry.count.saturating_add(1);
        }
    }
    findings
}

/// Break an inline log line into one whitespace token per line so
/// `sanitize_terminal_text` can redact `key=secret` pairs that are not at the
/// start of the raw line (defense against inline secret leakage — SR-2).
fn linewise(detail: &str) -> String {
    detail.split_whitespace().collect::<Vec<_>>().join("\n")
}

/// Defeat any `stewardship-signature` sequence embedded in untrusted text by
/// inserting a benign marker after the phrase, so the exact dedup needle
/// `stewardship-signature: <sig>` can never be forged from an excerpt (SR-3).
fn neutralize_markers(s: &str) -> String {
    const NEEDLE: &str = "stewardship-signature";
    let lower = s.to_ascii_lowercase();
    if !lower.contains(NEEDLE) {
        return s.to_string();
    }
    // ASCII-lowercasing preserves byte length, so byte offsets align between
    // `s` and `lower`.
    let mut out = String::with_capacity(s.len() + 16);
    let mut i = 0;
    while i < s.len() {
        if lower[i..].starts_with(NEEDLE) {
            out.push_str(&s[i..i + NEEDLE.len()]);
            out.push_str("(excerpt)");
            i += NEEDLE.len();
        } else {
            let ch = s[i..].chars().next().expect("valid char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Truncate to at most `max` chars on a UTF-8 boundary (length bounding).
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

fn read_u64_env(key: &str) -> Option<u64> {
    std::env::var(key).ok().and_then(|s| s.trim().parse().ok())
}

fn read_bool_env(key: &str) -> Option<bool> {
    std::env::var(key)
        .ok()
        .map(|s| matches!(s.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
}
