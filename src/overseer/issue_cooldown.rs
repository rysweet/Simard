//! Durable **issue-cooldown ledger** (issue #4930 / Problem 1 — the OODA-core
//! auto-issue storm).
//!
//! The OODA-core self-monitoring re-filed the SAME finding once per OODA cycle
//! because its per-path dedup was in-memory only (reset on the daemon's periodic
//! exec-reload) and its durable marker could be clobbered by a cross-client
//! goal-board merge. Over ~24h that produced ~20 duplicate auto-issues from just
//! three findings.
//!
//! [`IssueCooldownLedger`] is the single **durable** dedup layer shared by every
//! auto-issue filer. It guarantees that a given `(finding_kind, subject)` opens
//! **at most one** tracking issue per cooldown window — a window whose floor is
//! `>= 1` full OODA cycle — and the guarantee **survives daemon exec-reload,
//! process restart, and cross-client goal-board merges** because the state lives
//! in a standalone cognitive-memory fact namespace (`overseer:issue-cooldown`),
//! not in any in-process struct or the goal-board snapshot.
//!
//! The window math is [`WhisperGate`]'s exponential backoff reused verbatim
//! (`6h → 12h → 24h`, capped): the ledger is "a `WhisperGate` window with a
//! durable, mergeable backing store". See
//! `docs/reference/issue-cooldown-ledger-api.md` for the full contract.
//!
//! Security / invariants: `subject` is canonicalized to a charset-restricted
//! slug so untrusted goal/gap text cannot inject `gh --search` qualifiers
//! (SR-V3); the durable fact holds only `{ last_emit_secs, strikes,
//! issue_number }` — never an issue body or token (SR-D1/D2); a memory-read
//! error fails **open** (`Emit`) so a storage hiccup can never permanently
//! silence a genuinely new finding (SR-D3). Observability is `tracing` at
//! `debug` only — no `print!`/`println!`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::overseer::guardrails::WhisperGate;
use crate::stewardship::gh_client::{GhClient, GhIssue};

/// The isolated cognitive-memory namespace every cooldown fact lives under, kept
/// separate from other facts so a search/prune scan is exact and the ledger can
/// never be clobbered by the goal-board snapshot merge.
const COOLDOWN_NAMESPACE: &str = "overseer:issue-cooldown";

/// Stable identifier for the class of finding being filed. Serialized as a
/// fixed, charset-restricted slug so keys are stable across cycles and safe to
/// embed in a `gh` search signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FindingKind {
    /// No-progress breaker "goal stuck after guided retry" (`ooda-stuck`).
    OodaStuck,
    /// Overseer stewardship "goal re-blocked after prior escalation".
    RecurringGoalReblock,
    /// Overseer workstream gap-scan "uncovered backlog workstream".
    WorkstreamGapIssue,
}

impl FindingKind {
    /// Fixed slug used in the cooldown key and the issue-body signature. Never
    /// derived from untrusted input, so it is always charset-safe.
    pub fn slug(self) -> &'static str {
        match self {
            FindingKind::OodaStuck => "ooda-stuck",
            FindingKind::RecurringGoalReblock => "recurring-goal-reblock",
            FindingKind::WorkstreamGapIssue => "workstream-gap-issue",
        }
    }
}

/// Reduce an untrusted `subject` to a stable, injection-safe slug.
///
/// Lower-cases, keeps only `[a-z0-9]`, maps every other byte to `_`, collapses
/// runs of `_`, and trims leading/trailing `_`. This is deliberately STRICTER
/// than the documented `[a-z0-9:_-]` concept charset: dropping `:` and `-` from
/// the *subject* means untrusted goal/gap text can never contribute a
/// `gh --search` qualifier such as `is:issue` or `label:...` (SR-V3), while the
/// `:`/`-` that DO appear in a [`CooldownKey::fact_concept`] come only from the
/// fixed namespace and slug that this module controls. Total and pure; an empty
/// or all-separator subject collapses to a single `_`.
fn canonicalize_subject(subject: &str) -> String {
    let mut out = String::with_capacity(subject.len());
    let mut last_was_sep = false;
    for c in subject.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_lowercase() || lc.is_ascii_digit() {
            out.push(lc);
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('_');
            last_was_sep = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Durable, canonicalized dedup key: `(finding_kind, subject)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CooldownKey {
    pub kind: FindingKind,
    pub subject: String,
}

impl CooldownKey {
    /// Build a key from a finding kind and a raw subject (e.g. a goal id or a
    /// gap signature). Canonicalizes `subject`. Total and pure.
    pub fn new(kind: FindingKind, raw_subject: &str) -> Self {
        Self {
            kind,
            subject: canonicalize_subject(raw_subject),
        }
    }

    /// The stable cognitive-memory fact concept:
    /// `overseer:issue-cooldown:<slug>:<canonical-subject>`. The only `:`/`-`
    /// come from the fixed namespace and slug; the subject segment is
    /// `[a-z0-9_]` only.
    pub fn fact_concept(&self) -> String {
        format!(
            "{}:{}:{}",
            COOLDOWN_NAMESPACE,
            self.kind.slug(),
            self.subject
        )
    }
}

/// Result of consulting the ledger before filing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CooldownDecision {
    /// No prior emit, or the cooldown window has elapsed → FILE a new issue,
    /// then call [`IssueCooldownLedger::record_emit`].
    Emit,
    /// Inside the cooldown window → DO NOT file; comment-and-throttle on the
    /// existing open issue via [`IssueCooldownLedger::note_still_observed`].
    Throttle,
}

/// The small, non-sensitive JSON persisted per key. Deliberately carries NO
/// issue body and NO token (SR-D1/D2) — only the metadata the backoff needs.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CooldownRecord {
    /// Unix seconds of the most recent emit.
    last_emit_secs: i64,
    /// Number of in-window emits so far; drives the exponential window.
    strikes: u32,
    /// The one canonical tracking issue's number (for comment-and-throttle).
    issue_number: u64,
}

/// Durable cooldown ledger backed by a standalone cognitive-memory fact
/// namespace (`overseer:issue-cooldown`), keyed by `(finding_kind, subject)`.
///
/// The ledger lives OUTSIDE the goal-board snapshot, so `merge_boards`
/// last-writer-wins can never clobber it, and it is reconstructed from durable
/// memory on daemon exec-reload / restart, so the in-memory [`WhisperGate`]
/// reset that caused the storm no longer re-opens the loop.
pub struct IssueCooldownLedger {
    memory: Arc<dyn CognitiveMemoryOps>,
    window: WhisperGate,
}

impl IssueCooldownLedger {
    /// Construct with the durable memory backing and the exponential window.
    /// `window` supplies only the backoff math (base/cap/per-hour); the ledger
    /// holds the durable `(last_emit, strikes)` in cognitive memory rather than
    /// in the gate's in-memory maps.
    pub fn new(memory: Arc<dyn CognitiveMemoryOps>, window: WhisperGate) -> Self {
        Self { memory, window }
    }

    /// Best-effort durable read of the record for `key`. Returns `Ok(None)` when
    /// no fact exists and `Err(..)` only on a genuine memory read error (so
    /// callers can implement the fail-open contract explicitly).
    fn read_record(&self, key: &CooldownKey) -> SimardResult<Option<CooldownRecord>> {
        let concept = key.fact_concept();
        let facts = self.memory.search_facts(&concept, 8, 0.0)?;
        let record = facts
            .into_iter()
            .find(|f| f.concept == concept)
            .and_then(|f| serde_json::from_str::<CooldownRecord>(&f.content).ok());
        Ok(record)
    }

    /// Decide whether a filer may open a new issue for this finding right now.
    /// Reads the durable last-emit timestamp for `key` and applies the backoff
    /// window. Fail-OPEN: a memory read error returns [`CooldownDecision::Emit`]
    /// (never permanently suppresses a genuinely new finding, SR-D3). Does not
    /// mutate durable state.
    pub fn allow_emit(&self, key: &CooldownKey, now_secs: i64) -> CooldownDecision {
        let record = match self.read_record(key) {
            Ok(Some(r)) => r,
            // No prior durable emit → admit.
            Ok(None) => return CooldownDecision::Emit,
            // Fail OPEN on a read error: a storage hiccup must never permanently
            // suppress a genuinely new finding.
            Err(e) => {
                tracing::debug!(
                    target: "overseer::issue_cooldown",
                    key = %key.fact_concept(),
                    error = %e,
                    "issue-cooldown read failed; failing open (emit)"
                );
                return CooldownDecision::Emit;
            }
        };

        let window = self.window.window_for_strikes(record.strikes);
        let elapsed = now_secs.saturating_sub(record.last_emit_secs);
        let decision = if elapsed < window {
            CooldownDecision::Throttle
        } else {
            CooldownDecision::Emit
        };
        tracing::debug!(
            target: "overseer::issue_cooldown",
            key = %key.fact_concept(),
            strikes = record.strikes,
            window_secs = window,
            elapsed_secs = elapsed,
            decision = ?decision,
            "issue-cooldown decision"
        );
        decision
    }

    /// Record that an issue was filed for `key` at `now_secs`. Upsert-idempotent:
    /// re-recording the same key advances the last-emit timestamp and grows the
    /// backoff strike count **in place** (no duplicate fact) by writing through
    /// [`CognitiveMemoryOps::store_fact_with_caller_key`] with the deterministic
    /// key. Persists a fact under `key.fact_concept()`.
    pub fn record_emit(
        &self,
        key: &CooldownKey,
        issue: &GhIssue,
        now_secs: i64,
    ) -> SimardResult<()> {
        // Read the prior strike count best-effort so the window grows across
        // re-emits. A read error here is non-fatal — treat as no prior record.
        let prior_strikes = self
            .read_record(key)
            .ok()
            .flatten()
            .map(|r| r.strikes)
            .unwrap_or(0);

        let record = CooldownRecord {
            last_emit_secs: now_secs,
            strikes: prior_strikes.saturating_add(1),
            issue_number: issue.number,
        };
        // Only non-sensitive metadata is serialized — never the issue body/token.
        let content = serde_json::to_string(&record).unwrap_or_else(|_| {
            format!(
                "{{\"last_emit_secs\":{},\"strikes\":{},\"issue_number\":{}}}",
                record.last_emit_secs, record.strikes, record.issue_number
            )
        });

        let concept = key.fact_concept();
        let tags = [
            "overseer".to_string(),
            "issue-cooldown".to_string(),
            key.kind.slug().to_string(),
        ];
        self.memory.store_fact_with_caller_key(
            &concept, // caller_key: deterministic, one row per (kind, subject)
            &concept, // concept
            &content, // content: metadata JSON only
            1.0,      // confidence
            &tags,    // tags for recall/prune scans
            &concept, // source_id: provenance
        )?;
        tracing::debug!(
            target: "overseer::issue_cooldown",
            key = %concept,
            strikes = record.strikes,
            issue_number = record.issue_number,
            "issue-cooldown recorded emit"
        );
        Ok(())
    }

    /// Comment-and-throttle: add a short "still observed" annotation to the
    /// existing open issue for `key` instead of filing a new one. Fail-OPEN on
    /// any `gh`/memory error (a lost comment is not a storm — `record_emit`
    /// already prevented the duplicate). Never files.
    pub fn note_still_observed(
        &self,
        key: &CooldownKey,
        gh: &dyn GhClient,
        repo: &str,
        now_secs: i64,
    ) -> SimardResult<()> {
        let issue_number = match self.read_record(key).ok().flatten() {
            Some(r) => r.issue_number,
            None => {
                // Nothing recorded to comment on — nothing to do (fail open).
                return Ok(());
            }
        };
        let body = format!(
            "Still observed at t={now_secs} for `{}` (issue-cooldown throttle; \
             not re-filing — this is the single canonical tracking issue).",
            key.fact_concept()
        );
        if let Err(e) = gh.comment_on_issue(repo, issue_number, &body) {
            // A dropped annotation is benign; the duplicate was already avoided.
            tracing::debug!(
                target: "overseer::issue_cooldown",
                key = %key.fact_concept(),
                issue_number,
                error = %e,
                "issue-cooldown comment-and-throttle failed (non-fatal, fail-open)"
            );
        }
        Ok(())
    }

    /// Count cooldown facts not touched for `> cap_secs` (bounded-memory
    /// hygiene), consistent with the [`WhisperGate`] stale-entry pruning. A key
    /// past the cap is already admitted immediately by [`allow_emit`] (the
    /// window can never exceed the cap), so pruning is purely a memory-bounding
    /// pass; actual node reclamation of the durable fact is delegated to the
    /// memory backend's retention pass
    /// ([`CognitiveMemoryOps::forget_low_value_facts`] /
    /// [`CognitiveMemoryOps::prune_superseded`]). Returns the number of stale
    /// keys observed.
    pub fn prune(&self, now_secs: i64) -> SimardResult<usize> {
        let cap = self.window.cap_secs();
        let facts = self.memory.search_facts(COOLDOWN_NAMESPACE, 10_000, 0.0)?;
        let stale = facts
            .into_iter()
            .filter(|f| f.concept.starts_with(COOLDOWN_NAMESPACE))
            .filter_map(|f| serde_json::from_str::<CooldownRecord>(&f.content).ok())
            .filter(|r| now_secs.saturating_sub(r.last_emit_secs) > cap)
            .count();
        Ok(stale)
    }
}
