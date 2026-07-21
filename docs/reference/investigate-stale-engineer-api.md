---
title: "Reference: Investigate-Before-Reap API"
description: >
  The API contract for investigating a stale engineer BEFORE reaping it: the
  StaleEngineerInvestigator seam, the InvestigationVerdict / InvestigationCause /
  InvestigationOutcome types and their should_reap()/label() routing, the widened
  reap_stale_claims signature, the ReapSummary.pending_interventions field, the
  durable reaped-engineers/ evidence archive (path sanitization + assert_under_root
  containment), the production RecipeStaleEngineerInvestigator, the
  investigate_stale_engineer.md prompt asset, the Overseer wiring
  (ClaimReaperSeams / reap_stale_engineer_claims), and the regression test list.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/investigate-stale-engineer-before-reap.md
  - ./claim-reaper-api.md
  - ./overseer-root-cause-why-api.md
  - ./terminal-failure-diagnosis-api.md
  - ./engineer-worktree-sweep-safety.md
  - ../operations/claim-reaper-kill-switch.md
  - ../howto/investigate-a-stale-engineer-before-reap.md
---

# Reference: Investigate-Before-Reap API

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary source:
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs).
> Conceptual overview:
> [Investigate-Before-Reap](../concepts/investigate-stale-engineer-before-reap.md).
> This page documents ONLY the investigate-before-reap surface added on top of
> the [Stale-Engineer-Claim Reaper API](./claim-reaper-api.md); the ledger,
> liveness-probe, cleanup, and config surfaces are unchanged.

## Overview

The reaper is still a pure orchestrator over injectable seams, now with a
**fourth** seam: a `StaleEngineerInvestigator`. Only the
`Dead { HeartbeatStale, age > stale_secs }` branch changes — it no longer
releases + cleans directly. It:

1. asks the investigator for a verdict (the investigator archives evidence
   **before** returning any terminal verdict), then
2. routes **mechanically**: reclaim **iff** `verdict.should_reap()`, and
3. **always** surfaces the returned `interventions` for gated dispatch.

Injecting the investigator keeps the whole sweep hermetically testable with
fakes — no real filesystem, process, `gh`, or model call.

## Verdict types

```rust
/// Why a genuinely-dead engineer died. Carried only by `InvestigationVerdict::Dead`.
/// Every variant still reaps (the process is gone, evidence archived); the cause
/// drives self-improvement signalling, not the reap decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvestigationCause {
    Panic,
    Oom,
    E2big,
    LockContention,
    /// A defect in Simard itself killed the engineer. Reaps AND always carries
    /// self-improvement interventions (FileIssue / LaunchRecipe / Escalate).
    SimardBug,
    /// The engineer genuinely finished but never reported its result.
    FinishedUnreported,
    /// Provably gone, no known signature matched. Reaps; FileIssue captures it.
    Unknown,
}

impl InvestigationCause {
    /// Stable, log-safe label for the fail-visible reclaim line.
    pub fn label(self) -> &'static str; // "panic" | "oom" | "e2big" |
                                        // "lock-contention" | "simard-bug" |
                                        // "finished-unreported" | "unknown"
}

/// The investigation's conclusion about a would-be-stale engineer.
///
/// `should_reap()` is the ONLY decision Rust makes; all WHY-nuance lives behind
/// the seam. Every non-`Dead` variant fails closed (keeps the claim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvestigationVerdict {
    /// False positive: evidence shows the engineer is (or may be) still working.
    /// Never reaped; the reaper logs the false positive.
    StillAlive,
    /// Stuck on a missing precondition. Not reaped; escalate/surface the block.
    Blocked,
    /// Died from a transient/retryable condition. Not reaped; relaunch to resume.
    Recoverable,
    /// Investigation is in flight (agentic recipe launched). Not reaped THIS
    /// sweep; a later sweep of the same still-stale claim resolves it. Only the
    /// production investigator (which owns the inflight map) can construct this —
    /// it is NOT part of the model's parseable output schema (see below).
    Pending,
    /// Genuinely dead AND unrecoverable. Reap permitted (evidence already
    /// archived). `cause` names why it died.
    Dead { cause: InvestigationCause },
}

impl InvestigationVerdict {
    /// The mechanical router. `true` ONLY for `Dead`. Every other variant —
    /// StillAlive, Blocked, Recoverable, Pending — returns `false` (fail-closed).
    pub fn should_reap(self) -> bool;
    /// Stable, log-safe label named in the extended reclaim line.
    pub fn label(self) -> &'static str; // "still-alive" | "blocked" |
                                        // "recoverable" | "pending" | "dead"
}

/// What the investigator returns: the routing verdict plus any interventions to
/// dispatch. Interventions are surfaced REGARDLESS of the verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InvestigationOutcome {
    pub verdict: InvestigationVerdict,
    /// Actions for the Overseer's gated Act path. Reuses the existing
    /// `overseer::intervention::Intervention` set — no parallel plumbing.
    pub interventions: Vec<Intervention>,
}
```

> `InvestigationVerdict::default()` is `StillAlive` and
> `InvestigationOutcome::default()` carries it with no interventions — so any
> `Default`/fail-closed path keeps the claim and never fabricates a `Dead`
> verdict.

## Investigation seam: `StaleEngineerInvestigator`

```rust
/// Agentic-investigation injection seam. The production impl archives evidence
/// and dispatches the `investigate_stale_engineer.md` prompt as a gated
/// `smart-orchestrator` workstream; tests inject fakes that return a terminal
/// verdict.
///
/// CONTRACT: `investigate` MUST archive the engineer's diagnostic evidence to a
/// durable location BEFORE returning any terminal (`Dead`) verdict, so evidence
/// is structurally preserved before the pure fn is ever allowed to clean the
/// worktree. On ANY internal fault (spawn error, timeout, unparseable output),
/// it MUST fail closed with `StillAlive` — never fabricate `Dead`.
pub trait StaleEngineerInvestigator: Send + Sync {
    fn investigate(&self, claim_key: &str, idle_age_secs: u64) -> InvestigationOutcome;
}
```

`Send + Sync` (like the probe/cleanup seams) so it is stored across Overseer
ticks.

## Sweep: widened `reap_stale_claims`

The orchestrator gains one parameter — the investigator seam:

```rust
/// Sweep ALL engineer claims and reclaim those whose engineer is provably dead
/// AND investigated. Fail-closed, fail-visible, per-claim errors contained.
///
/// New invariant: a `HeartbeatStale` claim is NEVER reclaimed without a completed
/// (terminal) investigation whose verdict `should_reap()`. Evidence is archived
/// by the investigator before any terminal verdict is returned.
pub fn reap_stale_claims(
    ledger: &dyn ClaimLedger,
    probe: &dyn ClaimLivenessProbe,
    cleanup: &dyn OrphanWorktreeCleanup,
    investigator: &dyn StaleEngineerInvestigator,   // NEW seam
    enabled: bool,
    stale_secs: u64,
) -> ReapSummary;
```

Algorithm (only the `HeartbeatStale, age > stale_secs` branch is new):

```text
if !enabled: return ReapSummary::default()          // kill switch: total no-op

for claim_key in ledger.list_engineer_claims():
    match probe.assess(&claim_key):
        Live                                  => skip
        Dead { NoWorktree, .. }               => reclaim(no investigation; nothing to protect)
        Dead { HeartbeatStale, age <= T }     => skip                       // fail-closed, no wall-clock kill
        Dead { HeartbeatStale, age  > T }     =>
            outcome = investigator.investigate(&claim_key, age)             // archives evidence first
            summary.pending_interventions.extend(outcome.interventions)     // ALWAYS surfaced
            if outcome.verdict.should_reap():                               // == matches Dead{..} only
                reclaim(claim_key, reason="heartbeat-stale", age, verdict=outcome.verdict.label())
            else:
                summary.skipped += 1                                        // StillAlive/Blocked/Recoverable/Pending
    // reclaim = release_engineer_claim + cleanup.cleanup, one [simard] fail-visible line
```

- **`NoWorktree` is unchanged** — reclaimed immediately, no investigation.
- **Interventions are surfaced regardless of the verdict** (even for a
  `StillAlive` false positive).
- **Reclaim chokepoints only:** `release_engineer_claim` + the
  `OrphanWorktreeCleanup` seam. The investigator has NO direct release path. No
  hand-rolled SQL, no `--admin`.
- **Per-claim containment** and the release/cleanup ordering (row released before
  best-effort worktree removal) are unchanged.

## `ReapSummary.pending_interventions`

`ReapSummary` gains one field; the existing counters keep their meaning
(`skipped` now also counts `StillAlive`/`Blocked`/`Recoverable`/`Pending`
outcomes):

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReapSummary {
    pub reclaimed: Vec<String>,   // released + worktree cleaned this sweep
    pub skipped: usize,           // kept (live/fresh/unknown + non-reaping verdicts)
    pub errors: usize,            // contained per-claim errors
    /// NEW: interventions the investigator returned this sweep, drained by the
    /// caller into the existing gated Act path. Never dispatched by the pure fn.
    pub pending_interventions: Vec<Intervention>,
}
```

> **Claim association is intentionally flat.** `pending_interventions` is a single
> `Vec<Intervention>` for the whole sweep, not grouped per claim. This is
> sufficient because each `Intervention` is self-describing (e.g. `FileIssue`
> carries its own title/body and `FileIssue` dedup keys off content), and the
> gated Act path drains the vector uniformly. If per-claim telemetry is ever
> needed (e.g. attributing an escalation back to a specific `claim_key`), it must
> be added deliberately — either by enriching the `Intervention` payloads or by
> switching to a `Vec<(String, Intervention)>`. Do not assume ordering encodes
> the originating claim.

## Evidence archive

Performed by the production investigator **before** it returns any terminal
verdict, so the pure fn only ever cleans a worktree whose evidence is already
durable.

- **Location:** `<state_root>/reaped-engineers/<sanitized_claim_key>-<unix_ts>/`.
- **Containment:** the archive dir is `sanitize → canonicalize → assert_under_root`
  guarded against `<state_root>/reaped-engineers`, the SAME
  `assert_under_root` (canonicalize + `starts_with`) guard the worktree cleanup
  uses (see [Worktree Reaping Safety Guards](./engineer-worktree-sweep-safety.md)).
  A path-traversal `claim_key` (`../../etc/x`, absolute, NUL, control chars,
  embedded `:` / `/`) can never escape the archive root.
- **`sanitized_claim_key`:** `/`, `:`, `..`, NUL and control characters are
  replaced for the directory name; the **raw** `claim_key` is stored verbatim in
  `manifest.json` inside the archive.
- **Contents:** the worktree's newest logs / transcript / recipe-runner output,
  the captured exit status, a narrow `journalctl` slice for the goal/unit, and a
  `manifest.json` (raw `claim_key`, `goal_id`, `idle_age_secs`, timestamp,
  verdict when known). Copies are size-capped, symlink-safe (never follow a link
  out of the worktree/state_root), and secret-scrubbed (`ghp_`, `github_pat_`,
  AWS keys, `Authorization:`, `*_TOKEN=`). Dirs are `0700`, files `0600`.
  The `journalctl` slice is best-effort: when `journalctl` is unavailable or
  exits non-zero, `journal.txt` is omitted and the reason is emitted at `debug`
  level (`target = "overseer::claim_reaper"`); archiving otherwise proceeds.

## Production investigator: `RecipeStaleEngineerInvestigator`

The production seam is a **thin agentic rail** that reuses Simard's EXISTING
remediation machinery — exactly the [`self_diagnose`](./terminal-failure-diagnosis-api.md)
/ `escalation_triage` pattern — rather than adding a parallel investigation
pipeline. Per would-be-stale engineer, `investigate` does three things:

1. **Archive evidence FIRST** to `reaped-engineers/…` (above). If the archive
   fails, it folds to the fail-closed `StillAlive` (no launch, claim kept) — the
   whole point is to never destroy the evidence, so it never reaps blind.
2. **Dispatch the agentic WHY** as an `Intervention::LaunchRecipe` whose
   `task_description` points a `smart-orchestrator` workstream at
   [`investigate_stale_engineer.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/investigate_stale_engineer.md)
   with the archived `evidence_dir`, `goal_id`, `claim_key` (carried as untrusted
   DATA), and `idle_age_secs`. It is returned on `interventions`, so
   `reap_stale_engineer_claims` drains it through the SAME gate → plan → `act()`
   rail every other remediation uses — deduped by the existing
   `inflight_investigations` guard (no parallel plumbing, per the brief).
3. **Return `InvestigationVerdict::Pending`** — the reaper does NOT reap this
   sweep. The claim and its archived evidence persist while the investigation
   runs; the investigation files issues / escalates / dispatches a fix via its
   own gated tools when a Simard bug is implicated, and tears down a genuinely-
   dead worktree.

```rust
pub struct RecipeStaleEngineerInvestigator {
    state_root: PathBuf,
    target_repo: String,
}
impl RecipeStaleEngineerInvestigator {
    pub fn new(state_root: impl Into<PathBuf>, target_repo: impl Into<String>) -> Self;
}
```

`state_root` roots the evidence archive (and worktree correlation); `target_repo`
(`rysweet/Simard`) targets the dispatched fix workstream. No generic runner
parameter and no bespoke JSON parser: the agentic verdict + self-improvement
actions are produced by the dispatched workstream through the gated Act path, not
re-parsed in Rust — keeping the rail thin and the routing mechanical.

### `Pending` and cross-tick resolution

`Pending` is the production verdict on every sweep of a still-present stale
worktree: staleness ALONE never reaps (the defect this closes). The loop still
resolves — through the EXISTING plumbing, not a new inflight map:

1. **First sight** — archive evidence, dispatch the investigation `LaunchRecipe`,
   return `Pending` (skip the reap). The dispatched launch registers in the
   Overseer's `inflight_investigations` set.
2. **Still stale, investigation in flight** — the same claim produces the same
   brief → the same `recipe_dedup_key` → the in-flight guard suppresses a second
   launch; the reaper returns `Pending` again and keeps the claim.
3. **Investigation concludes** — when it finds the engineer genuinely dead it
   releases the claim + removes the worktree via its gated tools. The NEXT sweep
   then sees `Dead { NoWorktree }` and reclaims the leaked slot **immediately**
   (the `NoWorktree` branch needs no investigation). A false-positive / blocked /
   recoverable engineer keeps working; nothing is destroyed.

So a genuinely-dead engineer is still reclaimed — post-investigation, with
evidence preserved — but the reclaim rides the unambiguous `NoWorktree` path once
the investigation has torn the dead worktree down, never a silent staleness kill.
Fakes return terminal verdicts directly, so the pure-function tests exercise the
`Dead`-reaps-now routing without the multi-tick production path.

## Prompt asset

[`prompt_assets/simard/overseer/investigate_stale_engineer.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/investigate_stale_engineer.md)
is the single agentic prompt behind the seam — JSON in, JSON out, modeled on
[`self_diagnose.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/self_diagnose.md)
but adding the false-positive / `still-alive` category central to this feature.
Inputs: `claim_key`, `goal_id`, `idle_age_secs`, `evidence_dir`, evidence tails,
`exit_status`, `journal_slice`, `prior_signature_recall`. Output: `verdict`,
`cause`, `why`, `interventions`, `escalate`.

## Overseer wiring

- **`ClaimReaperSeams`** (in
  [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs))
  carries the boxed `StaleEngineerInvestigator` alongside the ledger handle,
  probe, and cleanup, injected via `with_claim_reaper` and stored across ticks.
- **`reap_stale_engineer_claims`** (mod.rs) is the thin caller: it calls
  `reap_stale_claims(...)`, stages `summary.pending_interventions` on the
  Overseer, and then — after `health_review` in `run_cycle` —
  **`dispatch_reaper_interventions`** drains them through the SAME gate → plan →
  `act()` rail agentic health review uses: the investigation `LaunchRecipe`
  registers in `inflight_investigations`, an `EscalateBlockedGoal` routes via
  `act_escalate_blocked_goal`, a `FileIssue` via `IssueFiler::file`. The sweep
  itself runs synchronously on the tick beside `reconcile_inflight_investigations`
  — **no new thread**.
- **`build_claim_reaper_seams`** (in
  [`src/overseer/wiring.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/wiring.rs))
  constructs the production `RecipeStaleEngineerInvestigator` alongside the
  ledger, probe, and cleanup, rooted at the same `state_root`.

## Configuration

**No new knob.** Investigation is always-on whenever the reaper is enabled; it
reuses `SIMARD_CLAIM_REAP_ENABLED` and `SIMARD_CLAIM_REAP_STALE_SECS` (see the
[kill switch](../operations/claim-reaper-kill-switch.md)). Disabling the reaper
(`SIMARD_CLAIM_REAP_ENABLED=off`) is still a total no-op — no sweep, no
investigation, no archive.

## Fail-visible logging

The single reclaim line is **extended** to name the verdict (the `claim-reaper:`,
`reason=`, `age=`, and new `verdict=` substrings are stable):

```
[simard] claim-reaper: reclaimed rysweet/Simard:goal-improve-tests (reason=heartbeat-stale, age=5142s, verdict=dead:panic)
```

A `still-alive` false positive is logged and NOT reaped:

```
[simard] claim-reaper: kept rysweet/Simard:goal-long-compile (reason=heartbeat-stale, age=5142s, verdict=still-alive — investigation says engineer still working)
```

`NoWorktree` reclaims are unchanged (no verdict; nothing was investigated):

```
[simard] claim-reaper: reclaimed rysweet/Simard:g1 (reason=no-worktree, age=n/a)
```

## Regression coverage

New hermetic tests (fake investigator + fake seams) live inline in
`src/overseer/claim_reaper.rs` (`#[cfg(test)]`), alongside the existing T1–T6 /
fail-closed suite which **still passes** — every existing call site injects a
fake investigator whose default terminal verdict is `Dead { Unknown }`, so
`t3_stale_worktree_is_reaped` and the rest keep identical assertions:

| Test | Asserts |
|---|---|
| Evidence-before-cleanup | The investigator archives evidence **before** any `cleanup.cleanup` call (ordering pinned); the archive exists after a `Dead` reclaim. |
| No-reap-while-pending | A `Pending` verdict ⇒ **not** reaped this sweep (no `release`, no `cleanup`); the claim persists for `reconcile`. |
| Still-alive false positive | `StillAlive` ⇒ **not** reaped; the claim is kept and the false positive is logged. |
| Blocked / recoverable | `Blocked` / `Recoverable` ⇒ **not** reaped; the block/relaunch interventions are surfaced. |
| Simard-bug surfaces self-improvement | `Dead { SimardBug }` ⇒ reaped **and** `FileIssue`/`Escalate` interventions land in `pending_interventions`. |
| Dead reaps with verdict logged | `Dead { cause }` ⇒ reaped via `release_engineer_claim` + cleanup, with the verdict in the log line. |
| Interventions always surfaced | Interventions are drained for dispatch regardless of the verdict. |
| Path-traversal rejected | A `../../etc/x` / absolute / NUL `claim_key` never escapes `reaped-engineers/` (archive stays under root). |
| Fail-closed on fault | An investigator that faults resolves to `StillAlive` ⇒ claim kept; never a fabricated `Dead`. |
| Existing T1–T6 + fail-closed probe | Unchanged assertions (fake investigator returns `Dead{Unknown}`). |

Required gates (merge blockers): `cargo fmt`, `cargo clippy -D warnings`, and
`cargo test` (overseer/claim_reaper + diagnosis) must pass; no `unwrap`/`expect`
on any I/O or subprocess path; never `--no-verify` / `--admin`.

## Related

- [Investigate-Before-Reap (concept)](../concepts/investigate-stale-engineer-before-reap.md)
- [Stale-Engineer-Claim Reaper API](./claim-reaper-api.md) — the base sweep this extends.
- [Overseer Root-Cause WHY API](./overseer-root-cause-why-api.md)
- [Terminal-Failure Diagnosis API](./terminal-failure-diagnosis-api.md)
- [Worktree Reaping Safety Guards](./engineer-worktree-sweep-safety.md)
- [Claim-Reaper Kill Switch & Tuning](../operations/claim-reaper-kill-switch.md)
- [How to investigate a stale engineer before reap](../howto/investigate-a-stale-engineer-before-reap.md)
