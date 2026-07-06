//! The bounded, durable "recent Overseer activity" feed (#2419).
//!
//! Simard's cognition runs on two clocks: the **engineer** OODA loop (already
//! visible on the dashboard and TUI) and the **steward** side — the acting
//! [`Overseer`](crate::overseer) meta-loop that runs alongside it on its own
//! cadence, quietly observing the whole system and intervening (filing issues,
//! launching fix workstreams, verifying + merging green PRs, guarded deploys,
//! escalations) or *holding* when a gate says "not yet".
//!
//! Until now that steward activity was invisible outside the daemon log. This
//! module makes it a first-class, queryable surface: the last [`MAX_RECORDS`]
//! Overseer ticks and their outcomes, plus per-thread status.
//!
//! ## Why a durable file (not an in-RAM ring)
//!
//! The daemon (which *runs* the Overseer), the dashboard server, and the TUI are
//! **separate processes**. A pure in-memory ring in the daemon would be
//! unreachable by the other two. So the feed keeps a bounded in-memory
//! [`VecDeque`] (cap [`MAX_RECORDS`]) as the write surface **and** persists the
//! whole capped feed atomically to `<state_root>/overseer/activity.json` each
//! tick — the same tmp+rename, `0600`, degrade-to-`None` pattern the
//! [telemetry snapshot](crate::telemetry::snapshot) already uses. That file is
//! the cross-process seam every reader consumes.
//!
//! This module only *records and surfaces* Overseer activity; it never changes
//! what the Overseer decides or does. The only producer-side touch is
//! [`record_tick`] at the tick boundary.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cognitive_threads::ThreadHealth;
use crate::overseer::OverseerTickReport;
use crate::overseer::intervention::Remediation;
use crate::overseer::signal::RootCause;

/// Bumped only on an INCOMPATIBLE shape change. Additive fields do not bump it.
pub const SCHEMA_VERSION: u32 = 1;

/// The ring buffer retains the last `N` Overseer ticks, evicting oldest-first.
pub const MAX_RECORDS: usize = 100;

/// Hard cap on the on-disk feed we will read into memory (bytes). A
/// pathologically large file degrades to `None` instead of exhausting memory.
pub const MAX_FEED_BYTES: u64 = 8 * 1024 * 1024;

/// Cumulative outcome counts across the retained records (a rolling window over
/// the last [`MAX_RECORDS`] ticks — **not** an all-time counter).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverseerTotals {
    pub problems: u64,
    pub issues_filed: u64,
    pub recipes_launched: u64,
    pub prs_merged: u64,
    pub deploys: u64,
    pub escalations: u64,
    /// False-parked perpetual goals self-healed (auto-unblocked + reactivated).
    pub goals_unblocked: u64,
    /// Genuinely-blocked "needs human review" goals escalated to the operator.
    pub goals_escalated: u64,
    /// Backlog-coverage gaps flagged by the recurring gap-scan (operator
    /// notified + deduped issue filed).
    pub workstream_gaps_detected: u64,
    /// Backlog-coverage gaps suppressed as recurring (within the dedup window).
    pub workstream_gaps_suppressed: u64,
    /// Problems for which a structured root-cause WHY was produced (issue #2635).
    pub root_cause_analyses: u64,
    /// Actions labelled symptom-mitigation (root cause left unaddressed). A
    /// deliberate block (`Acknowledged`) is NOT counted here.
    pub symptom_mitigations: u64,
    /// Actions that addressed the root cause — class `RootCause` or `Acknowledged`
    /// (`remediation.root_cause_addressed == true`).
    pub root_causes_addressed: u64,
    pub held: u64,
    pub errors: u64,
}

/// One Overseer tick: the outcome report plus time + gate context.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverseerActivityRecord {
    /// RFC3339 timestamp of tick completion.
    pub timestamp: String,
    /// The acting-Overseer gate state at this tick.
    pub enabled: bool,
    /// The verbatim outcome tally (never interpreted).
    pub report: OverseerTickReport,
    /// Per-problem rows for this tick (issue #2635): problem + WHY + action +
    /// root-cause/symptom. Additive (`#[serde(default)]`), so a feed written
    /// before this field existed deserializes to an empty vector. If a
    /// concurrent overseer-log-detail change adds its own per-problem type,
    /// `ProblemEntry` merges into it (keep both sides).
    pub problem_entries: Vec<ProblemEntry>,
}

/// Per cognitive thread, derived from [`ThreadHealth`] (epochs → RFC3339).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverseerThreadStatus {
    /// Stable thread id, e.g. `"overseer"`.
    pub id: String,
    pub enabled: bool,
    /// From `last_run_epoch`, or `None`.
    pub last_run: Option<String>,
    /// From `next_run_epoch`, or `None`.
    pub next_due: Option<String>,
    pub last_success: Option<bool>,
    pub consecutive_errors: u32,
    pub backoff_until: Option<String>,
    /// Derived label — see [`derive_health`].
    pub health: String,
}

impl OverseerThreadStatus {
    /// Build the synthetic status row for the acting Overseer meta-thread (which
    /// is driven directly by the daemon loop, not the [`Mind`](crate::cognitive_threads::Mind)
    /// scheduler). `last_success` reflects the just-completed tick.
    pub fn overseer_meta(cadence_secs: u64, last_success: bool) -> Self {
        let now = chrono::Utc::now();
        let consecutive_errors = u32::from(!last_success);
        let last_run = Some(rfc3339(now));
        let next_due = Some(rfc3339(
            now + chrono::Duration::seconds(cadence_secs as i64),
        ));
        let health = derive_health(true, None, consecutive_errors, last_run.as_deref());
        Self {
            id: "overseer".to_string(),
            enabled: true,
            last_run,
            next_due,
            last_success: Some(last_success),
            consecutive_errors,
            backoff_until: None,
            health,
        }
    }

    /// Convert a scheduler [`ThreadHealth`] heartbeat into a feed row, mapping
    /// unix epochs to RFC3339 and deriving the plain-word `health` label.
    pub fn from_thread_health(h: &ThreadHealth) -> Self {
        let last_run = h.last_run_epoch.map(epoch_to_rfc3339);
        let backoff_until = h.backoff_until_epoch.map(epoch_to_rfc3339);
        let health = derive_health(
            h.enabled,
            backoff_until.as_deref(),
            h.consecutive_errors,
            last_run.as_deref(),
        );
        Self {
            id: h.id.clone(),
            enabled: h.enabled,
            last_run,
            next_due: h.next_run_epoch.map(epoch_to_rfc3339),
            last_success: h.last_success,
            consecutive_errors: h.consecutive_errors,
            backoff_until,
            health,
        }
    }
}

/// Pure, testable `health` label (first match wins), per the feed reference.
pub fn derive_health(
    enabled: bool,
    backoff_until: Option<&str>,
    consecutive_errors: u32,
    last_run: Option<&str>,
) -> String {
    if !enabled {
        "disabled"
    } else if backoff_until.is_some() {
        "backoff"
    } else if consecutive_errors > 0 {
        "erroring"
    } else if last_run.is_none() {
        "idle"
    } else {
        "ok"
    }
    .to_string()
}

/// The whole feed: top-level Overseer status + per-thread status + recent ticks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverseerActivity {
    pub schema_version: u32,
    /// `overseer_acting_enabled()` at the last write.
    pub enabled: bool,
    /// `overseer_interval_secs()` — drives the `live`/`stale` window (`2×`).
    pub cadence_secs: u64,
    /// The Overseer's distinct git identity.
    pub author_login: String,
    /// RFC3339 of the most recent tick, or `None`.
    pub last_tick_at: Option<String>,
    /// Summed over the records currently retained.
    pub totals: OverseerTotals,
    /// From `Mind::health()` plus the synthetic overseer meta-thread.
    pub threads: Vec<OverseerThreadStatus>,
    /// Newest-first, capped at [`MAX_RECORDS`].
    pub recent: VecDeque<OverseerActivityRecord>,
}

impl Default for OverseerActivity {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            enabled: true,
            cadence_secs: crate::overseer::config::DEFAULT_OVERSEER_INTERVAL_SECS,
            author_login: crate::overseer::config::DEFAULT_OVERSEER_AUTHOR_LOGIN.to_string(),
            last_tick_at: None,
            totals: OverseerTotals::default(),
            threads: Vec::new(),
            recent: VecDeque::new(),
        }
    }
}

impl OverseerActivity {
    /// Front-push a tick, evict oldest beyond [`MAX_RECORDS`], refresh
    /// `last_tick_at`, and recompute `totals` over the retained window.
    pub fn push_record(&mut self, record: OverseerActivityRecord) {
        self.recent.push_front(record);
        while self.recent.len() > MAX_RECORDS {
            self.recent.pop_back();
        }
        self.last_tick_at = self.recent.front().map(|r| r.timestamp.clone());
        self.recompute_totals();
    }

    fn recompute_totals(&mut self) {
        let mut t = OverseerTotals::default();
        for r in &self.recent {
            let rep = &r.report;
            t.problems += rep.problems as u64;
            t.issues_filed += rep.issues_filed as u64;
            t.recipes_launched += rep.recipes_launched as u64;
            t.prs_merged += rep.prs_merged as u64;
            t.deploys += rep.deploys as u64;
            t.escalations += rep.escalations as u64;
            t.goals_unblocked += rep.goals_unblocked as u64;
            t.goals_escalated += rep.goals_escalated as u64;
            t.workstream_gaps_detected += rep.workstream_gaps_detected as u64;
            t.workstream_gaps_suppressed += rep.workstream_gaps_suppressed as u64;
            t.root_cause_analyses += rep.root_cause_analyses as u64;
            t.symptom_mitigations += rep.symptom_mitigations as u64;
            t.root_causes_addressed += rep.root_causes_addressed as u64;
            t.held += rep.held as u64;
            t.errors += rep.errors as u64;
        }
        self.totals = t;
    }

    /// Count of *actions taken* over the retained window (issues filed, fix
    /// workstreams launched, PRs merged, deploys, escalations, goal-board
    /// self-heals + escalations, backlog-coverage gaps flagged). `held` is
    /// deliberately excluded: holding is observing-and-waiting, not an action.
    pub fn interventions(&self) -> u64 {
        let t = &self.totals;
        t.issues_filed
            + t.recipes_launched
            + t.prs_merged
            + t.deploys
            + t.escalations
            + t.goals_unblocked
            + t.goals_escalated
            + t.workstream_gaps_detected
    }

    /// The honest one-line status summary rendered on every surface.
    ///
    /// Distinguishes *disabled* from *enabled-but-idle* — "observing, 0
    /// interventions" is a real, truthful outcome, never a blank list.
    pub fn status_summary(&self) -> String {
        if !self.enabled {
            return "disabled".to_string();
        }
        let n = self.interventions();
        if n == 0 {
            "enabled, observing, 0 interventions".to_string()
        } else {
            format!("enabled, {n} interventions")
        }
    }
}

/// Canonical on-disk path of the feed under a state root:
/// `<state_root>/overseer/activity.json`.
pub fn activity_path(state_root: &Path) -> PathBuf {
    state_root.join("overseer").join("activity.json")
}

/// Atomically and privately write `activity` to `path`.
///
/// Creates the parent `0700`, writes a `0600` temp file, `fsync`s it, then
/// `rename`s over the target — so `path` is never briefly world-readable and
/// readers never see a torn document. The `recent` ring is clamped to
/// [`MAX_RECORDS`] before serialize so the file can never grow unbounded.
pub fn write_atomic(path: &Path, activity: &OverseerActivity) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    // Clamp defensively on write, even though `push_record` already bounds it.
    let capped;
    let to_write = if activity.recent.len() > MAX_RECORDS {
        let mut c = activity.clone();
        c.recent.truncate(MAX_RECORDS);
        capped = c;
        &capped
    } else {
        activity
    };

    let body = serde_json::to_vec_pretty(to_write)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let tmp = path.with_extension("json.tmp");
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp)?;
        file.write_all(&body)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read a feed from `path`, degrading to `None` on any problem.
///
/// Returns `None` when the file is missing, larger than [`MAX_FEED_BYTES`],
/// unreadable, unparseable, or carries an unknown-higher `schema_version` —
/// **never** panics, never fabricates. Freshness is a judgement the caller
/// makes from `last_tick_at`; this only materializes the document.
pub fn read(path: &Path) -> Option<OverseerActivity> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FEED_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let activity = serde_json::from_slice::<OverseerActivity>(&bytes).ok()?;
    if activity.schema_version > SCHEMA_VERSION {
        return None;
    }
    Some(activity)
}

/// Durably append one tick to the feed under `state_root` and return the new
/// feed. Read-modify-write: reads the current file (or starts fresh), pushes the
/// record, refreshes the top-level status fields, and writes atomically.
///
/// Concurrency is last-writer-wins (no cross-process lock), but the atomic
/// tmp+rename guarantees the file is never torn — every reader always sees a
/// valid, bounded feed. This is the single producer-side touch the daemon makes,
/// at the tick boundary, and it is non-fatal to the caller on write error.
pub fn record_tick(
    state_root: &Path,
    record: OverseerActivityRecord,
    threads: Vec<OverseerThreadStatus>,
    enabled: bool,
    cadence_secs: u64,
    author_login: &str,
) -> std::io::Result<OverseerActivity> {
    let path = activity_path(state_root);
    let mut feed = read(&path).unwrap_or_default();
    feed.schema_version = SCHEMA_VERSION;
    feed.enabled = enabled;
    feed.cadence_secs = cadence_secs;
    feed.author_login = author_login.to_string();
    feed.threads = threads;
    feed.push_record(record);
    write_atomic(&path, &feed)?;
    Ok(feed)
}

/// RFC3339 (UTC, second precision) for a chrono instant.
fn rfc3339(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Convert a unix-epoch-seconds heartbeat into an RFC3339 string. An
/// out-of-range epoch degrades to the unix origin rather than panicking.
fn epoch_to_rfc3339(epoch_secs: u64) -> String {
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(epoch_secs as i64, 0)
        .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap());
    rfc3339(dt)
}

/// A single per-problem feed entry for one Overseer tick (issue #2635): the
/// problem, its root-cause **WHY**, the action taken, and whether that action
/// addressed the root cause or only mitigated the symptom. Rendered so an
/// operator sees, for every tick entry, *problem + WHY + action + root/symptom*.
///
/// This is the self-contained per-problem surface the root-cause work owns; it
/// composes with (does not depend on) the concurrent overseer-log-detail feed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProblemEntry {
    /// The problem's stable dedup key.
    pub key: String,
    /// The problem's one-line summary.
    pub summary: String,
    /// The structured root-cause analysis (the WHY).
    pub why: RootCause,
    /// The chosen action's short label (e.g. `"unblock_goal"`, `"escalate"`).
    pub action: String,
    /// Whether the action targeted the root cause or only the symptom.
    pub remediation: Remediation,
}

impl ProblemEntry {
    /// A compact one-line render: `{summary} — WHY: {why} — {action} [class]`,
    /// with the surfaced unaddressed-cause note appended for a symptom mitigation.
    pub fn humanize(&self) -> String {
        let mut line = format!(
            "{} — WHY: {} — {} [{}]",
            self.summary,
            self.why,
            self.action,
            self.remediation.class_label(),
        );
        if let Some(note) = &self.remediation.unaddressed_note {
            line.push_str(&format!(" — {note}"));
        }
        line
    }
}

/// A plain-language one-liner for one Overseer tick: what it **saw** and what it
/// **did** — or, when nothing needed doing, that it held or simply observed.
/// Shared by the `simard status` terminal render and the TUI Overseer pane so
/// both surfaces stay identical (the dashboard mirrors it in JS).
pub fn humanize_tick(r: &OverseerTickReport) -> String {
    fn plural(n: usize) -> &'static str {
        if n == 1 { "" } else { "s" }
    }
    let mut did: Vec<String> = Vec::new();
    if r.issues_filed > 0 {
        did.push(format!(
            "filed {} issue{}",
            r.issues_filed,
            plural(r.issues_filed)
        ));
    }
    if r.recipes_launched > 0 {
        did.push(format!(
            "launched {} workstream{}",
            r.recipes_launched,
            plural(r.recipes_launched)
        ));
    }
    if r.prs_merged > 0 {
        did.push(format!(
            "merged {} PR{}",
            r.prs_merged,
            plural(r.prs_merged)
        ));
    }
    if r.deploys > 0 {
        did.push(format!("ran {} deploy{}", r.deploys, plural(r.deploys)));
    }
    if r.escalations > 0 {
        did.push(format!("escalated {} to the operator", r.escalations));
    }
    if r.goals_unblocked > 0 {
        did.push(format!(
            "self-healed {} blocked goal{}",
            r.goals_unblocked,
            plural(r.goals_unblocked)
        ));
    }
    if r.goals_escalated > 0 {
        did.push(format!(
            "escalated {} blocked goal{} for human review",
            r.goals_escalated,
            plural(r.goals_escalated)
        ));
    }
    if r.memory_writes > 0 {
        did.push(format!(
            "recorded {} memory note{}",
            r.memory_writes,
            plural(r.memory_writes)
        ));
    }
    if r.workstream_gaps_detected > 0 {
        did.push(format!(
            "flagged {} workstream gap{}",
            r.workstream_gaps_detected,
            plural(r.workstream_gaps_detected)
        ));
    }

    let saw = format!("saw {} problem{}", r.problems, plural(r.problems));
    let action = if !did.is_empty() {
        did.join(", ")
    } else if r.held > 0 {
        format!("held {} (waiting on a gate)", r.held)
    } else if r.panicked {
        "tick panicked — isolated".to_string()
    } else {
        "observing, no action needed".to_string()
    };
    let mut line = format!("{saw}  ·  {action}");
    // Root-cause honesty (issue #2635): when the Overseer could only mitigate a
    // symptom, surface that the underlying cause was left live — never a silent
    // patch. Absent when no symptom mitigation occurred.
    if r.symptom_mitigations > 0 {
        line.push_str(&format!(
            "  ·  ({} symptom-mitigation{}, root cause unaddressed)",
            r.symptom_mitigations,
            plural(r.symptom_mitigations)
        ));
    }
    line
}

/// The per-tick DETAIL lines (issue #21) rendered beneath the [`humanize_tick`]
/// one-liner: WHAT the Overseer observed (concrete problem + evidence values)
/// and WHAT it did (each action/hold and its outcome). Observed lines are
/// prefixed `observed:`; action lines already self-prefix (`did:` / `held:`).
///
/// Additive companion to [`humanize_tick`], which stays a byte-identical
/// summary. Returns empty when the report carries no structured details (older
/// records, or a tick that observed nothing), so the summary one-liner stands
/// alone. Shared by the terminal `simard status` render, the TUI Overseer pane,
/// and mirrored in the dashboard SPA.
pub fn humanize_tick_details(r: &OverseerTickReport) -> Vec<String> {
    let mut out: Vec<String> =
        Vec::with_capacity(r.observed_details.len() + r.action_details.len());
    for o in &r.observed_details {
        out.push(format!("observed: {o}"));
    }
    for a in &r.action_details {
        out.push(a.clone());
    }
    out
}

/// Humanize a cadence in seconds to "15 min" / "2 h" / "45 s". Shared across the
/// status render and the TUI pane.
pub fn human_cadence(secs: u64) -> String {
    if secs == 0 {
        "—".to_string()
    } else if secs.is_multiple_of(3600) {
        format!("{} h", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(problems: usize, issues: usize, held: usize) -> OverseerTickReport {
        OverseerTickReport {
            problems,
            issues_filed: issues,
            held,
            ..OverseerTickReport::default()
        }
    }

    fn record(ts: &str, r: OverseerTickReport) -> OverseerActivityRecord {
        OverseerActivityRecord {
            timestamp: ts.to_string(),
            enabled: true,
            report: r,
            problem_entries: Vec::new(),
        }
    }

    #[test]
    fn default_feed_uses_schema_and_config_defaults() {
        let a = OverseerActivity::default();
        assert_eq!(a.schema_version, SCHEMA_VERSION);
        assert!(a.enabled);
        assert_eq!(
            a.cadence_secs,
            crate::overseer::config::DEFAULT_OVERSEER_INTERVAL_SECS
        );
        assert!(a.recent.is_empty());
    }

    #[test]
    fn status_summary_is_honest_for_each_state() {
        let disabled = OverseerActivity {
            enabled: false,
            ..OverseerActivity::default()
        };
        assert_eq!(disabled.status_summary(), "disabled");

        let mut idle = OverseerActivity::default();
        idle.push_record(record("2026-07-05T15:00:00Z", rep(2, 0, 1)));
        assert_eq!(idle.status_summary(), "enabled, observing, 0 interventions");

        let mut acting = OverseerActivity::default();
        acting.push_record(record("2026-07-05T15:00:00Z", rep(2, 3, 0)));
        assert_eq!(acting.status_summary(), "enabled, 3 interventions");
    }

    #[test]
    fn derive_health_first_match_wins() {
        assert_eq!(derive_health(false, None, 0, Some("t")), "disabled");
        assert_eq!(derive_health(true, Some("t"), 0, Some("t")), "backoff");
        assert_eq!(derive_health(true, None, 2, Some("t")), "erroring");
        assert_eq!(derive_health(true, None, 0, None), "idle");
        assert_eq!(derive_health(true, None, 0, Some("t")), "ok");
    }

    #[test]
    fn from_thread_health_maps_epochs_and_health() {
        let h = ThreadHealth {
            id: "maintenance".to_string(),
            enabled: true,
            last_run_epoch: Some(0),
            next_run_epoch: Some(900),
            last_success: Some(true),
            consecutive_errors: 0,
            backoff_until_epoch: None,
        };
        let s = OverseerThreadStatus::from_thread_health(&h);
        assert_eq!(s.id, "maintenance");
        assert_eq!(s.last_run.as_deref(), Some("1970-01-01T00:00:00Z"));
        assert_eq!(s.health, "ok");
    }

    #[test]
    fn overseer_meta_thread_reflects_tick_success() {
        let ok = OverseerThreadStatus::overseer_meta(900, true);
        assert_eq!(ok.id, "overseer");
        assert_eq!(ok.health, "ok");
        assert_eq!(ok.consecutive_errors, 0);

        let bad = OverseerThreadStatus::overseer_meta(900, false);
        assert_eq!(bad.health, "erroring");
        assert_eq!(bad.consecutive_errors, 1);
    }
}

#[cfg(test)]
mod detail_tests {
    //! Contract for the issue-#21 informative detail lines: the durable feed
    //! must carry structured, human-readable `observed_details` / `action_details`
    //! and expose them via `humanize_tick_details`, while the one-liner
    //! `humanize_tick` stays byte-identical (so existing needle tests hold) and
    //! the on-disk schema stays backward-compatible.
    use super::*;

    fn report_with_details() -> OverseerTickReport {
        OverseerTickReport {
            problems: 2,
            issues_filed: 1,
            observed_details: vec![
                "distillation parse-failure rate 34%".to_string(),
                "PR rysweet/Simard#42 is green and merge-ready".to_string(),
            ],
            action_details: vec![
                "did: filed issue https://github.com/rysweet/Simard/issues/9".to_string(),
                "held: verify-and-merge rysweet/Simard#42 — opt-in".to_string(),
            ],
            ..OverseerTickReport::default()
        }
    }

    #[test]
    fn humanize_tick_details_surfaces_observed_and_action_lines() {
        let lines = humanize_tick_details(&report_with_details());
        let joined = lines.join("\n");
        // Observed lines are prefixed with an "observed:" marker.
        assert!(
            joined.contains("observed:"),
            "observed detail lines must be marked 'observed:': {joined:?}"
        );
        assert!(
            joined.contains("34%"),
            "observed details must carry concrete values: {joined:?}"
        );
        // Action lines are already self-prefixed ("did:" / "held:").
        assert!(
            joined.contains("did: filed issue https://github.com/rysweet/Simard/issues/9"),
            "action details must surface the concrete action + outcome: {joined:?}"
        );
        assert!(
            joined.contains("held: verify-and-merge rysweet/Simard#42 — opt-in"),
            "a held action must explain itself in the details: {joined:?}"
        );
    }

    #[test]
    fn humanize_tick_details_is_empty_when_there_are_no_details() {
        let bare = OverseerTickReport {
            problems: 1,
            ..OverseerTickReport::default()
        };
        assert!(
            humanize_tick_details(&bare).is_empty(),
            "no structured details → no detail lines (the summary one-liner stands alone)"
        );
    }

    #[test]
    fn humanize_tick_one_liner_stays_byte_identical() {
        // The detail work is strictly ADDITIVE: the existing summary one-liner
        // (asserted by dashboard/status/TUI needle tests) must not drift.
        let r = OverseerTickReport {
            problems: 3,
            ..OverseerTickReport::default()
        };
        assert_eq!(
            humanize_tick(&r),
            "saw 3 problems  ·  observing, no action needed"
        );
    }

    #[test]
    fn report_details_round_trip_through_serde() {
        let r = report_with_details();
        let json = serde_json::to_string(&r).expect("serialize report");
        let back: OverseerTickReport = serde_json::from_str(&json).expect("deserialize report");
        assert_eq!(
            back, r,
            "the detail vecs must survive a serde round-trip verbatim"
        );
        // And the concrete strings are actually present in the wire form.
        assert!(json.contains("observed_details"));
        assert!(json.contains("action_details"));
        assert!(json.contains("34%"));
    }

    #[test]
    fn legacy_report_json_without_detail_fields_deserializes_to_empty_vecs() {
        // A record written by the CURRENT production build (deploy #21) has no
        // observed_details / action_details keys. It must still parse, with the
        // new fields defaulting to empty — never a hard reject on rolling deploy.
        let legacy = r#"{
            "problems": 3, "issues_filed": 1, "recipes_launched": 0,
            "prs_merged": 0, "deploys": 0, "escalations": 0, "held": 1,
            "whispers": 0, "whispers_suppressed": 0, "goals_unblocked": 0,
            "goals_escalated": 0, "goals_health_suppressed": 0, "errors": 0,
            "panicked": false, "duration_ms": 42
        }"#;
        let r: OverseerTickReport =
            serde_json::from_str(legacy).expect("legacy report must remain parseable");
        assert_eq!(r.problems, 3);
        assert_eq!(r.held, 1);
        assert!(
            r.observed_details.is_empty(),
            "missing observed_details must default to empty"
        );
        assert!(
            r.action_details.is_empty(),
            "missing action_details must default to empty"
        );
    }

    #[test]
    fn schema_version_stays_one_for_a_backward_compatible_additive_change() {
        // Additive `#[serde(default)]` fields do NOT bump the schema version —
        // bumping it would make older readers reject the newer feed.
        assert_eq!(SCHEMA_VERSION, 1);
    }
}
