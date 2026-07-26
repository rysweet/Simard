---
title: Overseer workstream gap-scan reference
description: >
  The Overseer's recurring "WHAT WORKSTREAMS ARE WE MISSING?" gap-scan — the
  additive Observe→Orient→Act step that surveys the whole work picture each tick
  and flags BACKLOG-COVERAGE GAPS: high-priority goals, high-signal GitHub
  issues, and live anomalies that SHOULD have an active workstream but do not.
  Covers the Signal::WorkstreamGap / ProblemKind::WorkstreamCoverage data model,
  the coverage-set detection contract, the deduped NotifyOperator act
  path, the SIMARD_OVERSEER_GAP_SCAN configuration, the additive
  OverseerTickReport / ObservedState fields, and how gaps render on the Overseer
  activity surfaces.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./overseer-activity-feed.md
  - ./overseer-gap-durable-dedup.md
  - ./overseer-self-observation-stability.md
  - ../design/overseer.md
  - ../howto/review-overseer-workstream-gaps.md
  - ../howto/diagnose-recurring-cognitive-memory-signature.md
  - ../howto/watch-overseer-activity.md
  - ./stewardship-api.md
  - ./no-progress-breaker-api.md
  - ./status-snapshot-api.md
  - ../concepts/operational-autonomy-model.md
---

# Overseer workstream gap-scan reference

> **Retired as the observation SOURCE (#2419).** The single-repo Rust
> survey-and-parse described here (`OVERSEER_SURVEY_REPO`,
> `survey_high_signal_open_issues`, `issue_coverage_from_open_prs`,
> `detect_workstream_gaps → Vec<GapItem> → FlagWorkstreamGaps`) has been replaced
> as the way the Overseer discovers work. Observation is now the agentic,
> multi-repo [ecosystem-observe chain](../design/ecosystem-observe.md): an agent
> runs `gh` across the stewarded roster and reasons to a deduped Problem list, with
> Rust reduced to a thin cadence/routing rail. The `SIMARD_OVERSEER_GAP_SCAN` /
> `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` cadence knobs are preserved and now gate the
> ecosystem-observe pass. This page is retained for historical context.

The acting **Overseer** already runs its own Observe/Orient/Decide/Act loop
alongside Simard's engineer OODA — filing stewardship issues, launching fix
workstreams, verifying and merging green PRs, and escalating genuine blocks (see
the [Overseer design](../design/overseer.md) and the
[activity feed](./overseer-activity-feed.md)). What it did **not** do before was
step back and ask the question an operator asks out loud:

> **"What workstreams are we missing?"**

The **workstream gap-scan** makes that a recurring, first-class part of the
Overseer's normal loop. On each (or every *Nth*) tick, the Overseer surveys the
**whole** work picture — the goal board, open GitHub issues, and live telemetry
anomalies — correlates it against everything already **in flight**, and flags the
**backlog-coverage gaps**: important work that *should* have an active workstream
but does not. It then acts on those gaps through the Overseer's **existing**
escalation plumbing: it notifies the operator (email + Signal), so a genuine
gap reaches a person exactly once without creating recursive tracking work.

> **This is additive.** The gap-scan adds one Observe→Orient→Act step to the
> Overseer loop. It does not change how existing interventions decide or act, and
> it reuses the same notification and dedup machinery as `goal_health`. Every
> new field on `ObservedState` and `OverseerTickReport` is
> additive and `#[serde(default)]`, so older readers tolerate a newer file.

> **Modules:** detector `src/overseer/sensor.rs`
> (`detect_workstream_gaps`); model `src/overseer/signal.rs`
> (`Signal::WorkstreamGap`, `GapItem`, `GapCategory`, `ProblemKind::WorkstreamCoverage`)
> + `src/overseer/capabilities.rs` (`ObservedState.workstream_gaps`); act path
> `src/overseer/mod.rs` (`act_flag_workstream_gaps`) + `src/overseer/notify.rs`
> (`OperatorNotification::workstream_gap`); wiring + counters
> `src/overseer/wiring.rs` (`OverseerTickReport.workstream_gaps_detected`);
> config `src/overseer/config.rs` (`SIMARD_OVERSEER_GAP_SCAN`); rendering
> `src/overseer/activity.rs` (`humanize_tick`) +
> `src/operator_commands_dashboard/overseer.rs`. Hermetic tests
> `src/overseer/tests_gap_scan.rs`.

## At a glance

| You want to… | Use |
|---|---|
| See the gaps the Overseer flagged | Dashboard **Overseer** tab / TUI **Overseer** pane / `simard status` → **OVERSEER** — each tick's line reads e.g. `flagged 2 workstream gaps` |
| Read the gaps as JSON | `GET /api/overseer` → `data.recent[].report.workstream_gaps_detected` / `…_suppressed` |
| Get told when a genuine gap appears | The deduped operator notification (email + Signal), kind `workstream-gap` |
| See what covers a gap | The gated coverage `LaunchRecipe` (`WORKSTREAM_COVERAGE_GROUP`) the gap-scan decides to since #4128 — the gap scan itself files **no** GitHub issue |
| Turn the scan up, down, or off | `SIMARD_OVERSEER_GAP_SCAN` + `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` (see [Configuration](#configuration)) |

## What counts as a gap

A gap is **genuine work that is currently uncovered**. Concretely, the gap-scan
looks at three sources and, for each candidate, keeps it **only if it is not
already being worked**:

| Source | Candidate | Kept as a gap when… |
|---|---|---|
| **Goal board** | A **p1/p2** (high-priority) active goal | It has **no** assigned engineer **and no** open PR — nobody is actually driving it. |
| **Open GitHub issues** (`rysweet/Simard`) | A **high-signal** issue (label `bug`, `P1`, or `workflow:default`) | It has **no** open PR **and no** active Overseer/engineer workstream referencing it. |
| **Live anomalies** (telemetry / `ObservedState`) | A standing anomaly (e.g. distill parse-fail rate high, restart churn, ladder exhausted) | It has **no** fix in flight — no open PR or workstream is addressing it. |

The rule, uniformly: **a candidate is a gap ⟺ no active workstream AND no open PR
AND (for anomalies) no fix in flight.**

### Coverage set (dedupe against in-flight work)

To decide "already being worked", the detector builds a **coverage set** once per
scan and tests every candidate against it:

```
coverage = { refs of currently-active workstreams / engineers }
         ∪ { refs of open PRs }
```

- **In-flight workstreams / engineers** come from the goal board's `wip_refs` and
  active-goal assignments (`goal_curation::load_goal_board`), plus the Overseer's
  own active-workstream handles.
- **Open PRs** come from the same `PrGhClient` read the merge path already uses
  (`merge_authority.rs`).

A candidate whose ref is in the coverage set is **covered** and is dropped. This
is what keeps the scan from re-flagging something an engineer already picked up,
and from fighting Simard's own OODA.

### Boundary with goal-board health (no double-notify)

The goal board also produces `Blocked` / "needs human review" /
consecutive-failure signals — but those are **already owned** by the
[`goal_health`](../design/overseer.md#capability-action-set-existing-simard-modules)
path (self-heal false parks, escalate genuine blocks). To avoid double-notifying
the operator, the gap-scan **delegates** those goals to `goal_health` and does
**not** re-flag them. The gap-scan **owns** only the *uncovered-but-not-blocked*
cases:

- an **idle p1/p2 goal** (active, high-priority, no engineer, no PR),
- a **high-signal open issue** with no PR/workstream,
- an **unaddressed live anomaly**.

A goal that is `Blocked` (including the `🔒 [OODA-SAFEGUARD] … needs human
review` no-progress marker, detected via `no_progress_breaker`) flows through
`goal_health`, not the gap-scan.

## Data model

Defined in `src/overseer/signal.rs` and `src/overseer/capabilities.rs`. All
additions are additive; the new `OverseerTickReport` / `OverseerTotals` counters
are `#[serde(default)]` so the activity-feed schema grows without a version bump.

### `GapItem` and `GapCategory`

One `GapItem` is the structured, human-readable description of a single uncovered
piece of work. **Structured data + templated rendering** (per guideline **G3**)
— never brittle string parsing:

```rust
/// One backlog-coverage gap: a specific piece of work that SHOULD have an active
/// workstream but does not. Structured so every surface renders it from fields,
/// never by re-parsing a blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GapItem {
    /// Which survey source produced this gap.
    pub category: GapCategory,
    /// The thing that is uncovered, as a stable reference:
    ///   goal id (`"g-1234"`), `owner/repo#N` for an issue, or an anomaly kind.
    pub ref_id: String,
    /// A short human-readable title (goal title / issue title / anomaly label).
    /// External/untrusted text is escaped + truncated before it reaches a
    /// notification, an issue body, or a log line.
    pub title: String,
    /// Plain-language "why this matters" — why an operator should care that this
    /// is uncovered (e.g. "p1 goal with no engineer and no PR").
    pub why_it_matters: String,
    /// Stable dedup signature for this gap (see the signature grammar below).
    /// Used to de-duplicate operator notifications (and coverage launches) across
    /// ticks.
    pub signature: String,
}

/// The survey source a gap came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapCategory {
    /// A high-priority goal with no engineer and no PR.
    GoalUncovered,
    /// A high-signal open GitHub issue with no PR and no workstream.
    IssueUncovered,
    /// A live telemetry anomaly with no fix in flight.
    AnomalyUnaddressed,
}
```

### `Signal::WorkstreamGap` and `ProblemKind::WorkstreamCoverage`

The scan emits **one** consolidated signal per tick carrying the whole (possibly
empty) gap list. (Unlike `goal_health`, which emits one `GoalBlocked` signal
*per* blocked goal, the gap-scan batches its whole Observe pass into a single
signal — the operator wants one "here is everything uncovered" view — while each
gap is still deduped independently by its `signature` inside the act.)

```rust
pub enum Signal {
    // … existing variants …
    /// Backlog-coverage gaps found this Observe pass: important work that SHOULD
    /// have an active workstream but does not (after dedupe against in-flight
    /// work). Carries the specific gaps so Orient/Act render them verbatim.
    /// From `ObservedState.workstream_gaps`.
    WorkstreamGap { gaps: Vec<GapItem> },
}

pub enum ProblemKind {
    // … existing variants …
    /// One or more backlog-coverage gaps — uncovered high-priority goals,
    /// high-signal issues, or unaddressed anomalies. Acted on by
    /// `act_flag_workstream_gaps`: notify the operator without filing an issue.
    WorkstreamCoverage,
}
```

Orient classifies a non-empty `WorkstreamGap` into a `Problem` of kind
`WorkstreamCoverage` at `High` priority (uncovered high-priority goals,
high-signal issues, and live anomalies all rank `High`), with the stable
`dedup_key` `"workstream-gap"`. The per-gap `signature`s drive the act-path dedup
gate (below), not the problem key.

### `ObservedState.workstream_gaps`

The Observe pass populates one additive field on `ObservedState`
(`src/overseer/capabilities.rs`), exactly like `blocked_goals`:

```rust
pub struct ObservedState {
    // … existing fields …
    /// Backlog-coverage gaps detected this Observe pass — high-priority goals,
    /// high-signal issues, and live anomalies that have NO active workstream and
    /// NO open PR (deduped against the coverage set). Populated by
    /// `sensor::detect_workstream_gaps`. Empty when everything important is
    /// covered, or when a survey source is unreadable (degrade-to-empty, never a
    /// panic).
    pub workstream_gaps: Vec<GapItem>,
}
```

`signal::signals_from` emits a single `Signal::WorkstreamGap { gaps }` whenever
`workstream_gaps` is non-empty (an empty list emits nothing — no gap, no signal,
no noise).

### The detector — `sensor::detect_workstream_gaps`

The detector is a **pure projection** (no I/O of its own; it is handed the
surveyed inputs), so it is unit-testable against a hand-built picture. Its
contract:

- **Inputs:** the active goal board, the surveyed high-signal open issues, the
  live anomalies (from `ObservedState`), and the **coverage set** (in-flight refs
  ∪ open-PR refs).
- **Output:** `Vec<GapItem>` — one item per genuine, uncovered candidate.
- **Delegation:** blocked / needs-review / consecutive-failure goals are skipped
  (owned by `goal_health`).
- **Bounds:** the number of gaps per tick and the length of every rendered field
  are bounded (input-validation guideline **V4**), and each `signature` is a
  sanitized slug (below), so a hostile issue title can never inflate a
  notification, an issue body, or a `gh` argument.
- **Degrade-to-empty:** if a survey source is missing or unreadable, that source
  contributes zero gaps and the scan continues — it never blocks the tick and
  never fabricates a gap.

## Signature grammar and de-duplication

Every gap carries a **stable signature** so the same recurring gap is acted on
(notified, or covered by a launch) **at most once** per window within a running
daemon, matching the existing M1 dedup behaviour. The
signature is a restricted slug — only `[A-Za-z0-9_\-#/:]`, never raw titles,
quotes, or newlines (input-validation guideline **V3**):

| Category | `signature` shape | Example |
|---|---|---|
| `GoalUncovered` | `goal:<goal_id>` | `goal:g-1873` |
| `IssueUncovered` | `issue:<owner>/<repo>#<number>` | `issue:rysweet/Simard#2630` |
| `AnomalyUnaddressed` | `anomaly:<kind>` | `anomaly:distill_parse_fail` |

The dedup key committed to the gate is `workstream-gap:<signature>`. De-duplication
is **in-process** on both act paths, so a fast tick cadence does not flood the
operator or the backlog:

1. **Notify path (`act_flag_workstream_gaps`).** Before notifying, it **peeks** the
   in-process `WhisperGate` (`gap_gate`) for `workstream-gap:<signature>`; if
   already committed within the window it records a *suppressed* outcome and
   skips. On success it **commits** the key. The gate **fails closed** on an
   identity/read error (guardrail **A2**) — an indeterminate gate suppresses
   rather than risks a duplicate. This path **only notifies the operator**; it
   does **not** file or upsert a GitHub issue.
2. **Coverage path (`LaunchRecipe`, the #4128 default).** An equivalent covering
   launch is **held** while one is already in flight (`inflight_investigations`)
   and, after it completes, suppressed within a growing bounded window by the
   in-process exponential `coverage_backoff` gate (keyed by `recipe_dedup_key`).

Both gates are **in-memory**, so their state is per-process. A durable,
cross-process GitHub open-issue equivalence check that would survive a daemon
restart is **future work**, tracked in
[#4717](https://github.com/rysweet/Simard/issues/4717); see the
[gap-filing dedup reference](./overseer-gap-durable-dedup.md). The result today is
**one deduped item per recurring gap signature within a running daemon**, not one
per tick.

## Act path — the coverage closing edge (issue #4128, D3b)

> **Update (issue #4128).** A `WorkstreamCoverage` problem now `decide`s to a
> **closing edge that actually covers the gap** — an
> `Intervention::LaunchRecipe` tagged with the `WORKSTREAM_COVERAGE_GROUP`
> sequence group — instead of the old **notify-only** `FlagWorkstreamGaps`
> routing. The notify-only path left the gap **uncovered**, so it re-surfaced
> every window as the recurring `workstream-gap` signature (the exact incident in
> #4128, A2). The launch is routed through the **same gate** as every other
> launch and additionally **fails closed** without a distinct steward identity
> (anti-recursion), is **held** while a coverage launch for the same gap set is
> already in flight (the `inflight_investigations` dedup guard, keyed by the
> per-gap `workstream-gap:<sorted sigs>` brief), is held when the gap-scan is
> disabled (the `SIMARD_OVERSEER_GAP_SCAN` opt-out now matches the coverage
> `sequence_group`), and carries a `task_description` **templated from structured
> data only** (gap category + restricted-slug signature), never raw issue text.
> The `Problem` `dedup_key` is keyed **per-gap** (`workstream-gap:<sig>`) rather
> than the bare `workstream-gap` constant, so distinct gap sets no longer collapse
> onto one key (which had starved the closing edge to a single gap).
>
> The `FlagWorkstreamGaps` intervention, its `act_flag_workstream_gaps` handler,
> and the `WorkstreamGapsFlagged` outcome **still exist** and are still admitted by
> the gate — only the `decide` **routing** for `WorkstreamCoverage` changed. The
> notify-centric description below documents that retained machinery; it is no
> longer the path a coverage gap takes by default.

When it is invoked directly, `act_flag_workstream_gaps`
(`src/overseer/mod.rs`) acts through the Overseer's **existing** escalation
machinery — the same notifier `goal_health` and M1 use — with no new bypass:

1. **De-dupe** each gap via the in-process `WhisperGate` (above); suppressed gaps
   are counted, not acted on. A gap whose signature is not a valid bounded slug is
   dropped and counted as suppressed.
2. **NotifyOperator.** Emit **one consolidated**
   `OperatorNotification::workstream_gap(count, top_gaps)` on **both** channels
   (email + Signal) through the `DualChannelNotifier` — a notification is never
   silently dropped; an unconfigured channel is `Queued` and logged.

That is the **entire** side effect of this act: it **notifies the operator only**.
It does **not** file or upsert a GitHub issue, and it does **not** call
`stewardship::process_orchestrator_run` / `find_existing`. (An `IssueFiler` /
`FileIssue` path does exist on the Overseer, but it is reached by the
`QualityRegression` CI-failure-cluster problem, **not** by the gap scan.) The act
then returns **one** summarising
`ActOutcome::WorkstreamGapsFlagged { flagged, suppressed }` (see
[Tick counters and totals](#tick-counters-and-totals)); it does **not** return a
separate `Escalated` / `IssueFiled` outcome, so gap activity is counted only on
the gap-scan's own dedicated counters, never on the generic ones.

**Since issue #4128 (D3b) the gap-scan closes the gap with a gated launch.** A
`WorkstreamCoverage` problem decides to a `LaunchRecipe` tagged
`WORKSTREAM_COVERAGE_GROUP` that launches a workstream to **cover** the uncovered
work, so the gap stops recurring. This launch is **not** a new bypass: it is
admitted through the **same gate** as every other launch (autonomy, per-cycle
launch cap, budget, and the conflict sequencer), it **fails closed** without a
distinct steward identity (the same anti-recursion guard the notify path
enforced), it is **held** while an identical coverage launch is already in flight,
and its `task_description` stays **Simard-templated from structured data** (gap
category + restricted-slug signature), never raw external issue text (guideline
**C2**). Underlying anomaly fixes still ride the Overseer's pre-existing
anomaly→`LaunchRecipe` path, counted normally under `recipes_launched`.

### `OperatorNotification::workstream_gap`

A factory mirroring `goal_blocked(...)`, kind `"workstream-gap"`. It renders a
short, plain-language, **provenance-labelled** summary — each gap says *what* is
uncovered and *why it matters*, and every external string is escaped and
truncated before it reaches email/Signal (content-safety guideline **C1**):

```rust
impl OperatorNotification {
    /// Build a workstream-gap notification: a consolidated, deduped summary of
    /// backlog-coverage gaps the Overseer found — uncovered high-priority goals,
    /// high-signal issues, or unaddressed anomalies — so a real gap reaches a
    /// person exactly once. Sent on BOTH channels (email + Signal).
    pub fn workstream_gap(count: usize, top: &[GapItem]) -> Self { /* … */ }
}
```

The **subject** reuses the shared `subject()` unchanged
(`[Overseer] {kind}: {headline}`): with `kind = "workstream-gap"` and
`headline = "N uncovered workstream(s)"` it reads
`[Overseer] workstream-gap: N uncovered workstream(s)` — no renderer change.

The **body** does *not* reuse the generic `plain_text()` template verbatim.
That template renders `"{who} performed a {kind} in {repo}. Problem solved:
{problem}"`, whose "performed a workstream-gap" / "Problem solved:" wording is
wrong for a gap (nothing was *solved*). So `plain_text()` gains one small
**kind-aware branch** for `"workstream-gap"` — still additive (a new match arm;
no field removed, no other kind's output changed) — that renders the
consolidated, provenance-labelled list the operator actually sees:

```text
The Overseer autonomously flagged backlog-coverage gaps in rysweet/Simard.

Uncovered work:
  • <category> <ref_id> — <title>: <why_it_matters>
  • …
```

Every external `title` is escaped and truncated before it reaches this body
(content-safety guideline **C1**). The per-line micro-format is an
implementation detail, but the intro line and the `Uncovered work:` heading are
fixed so the operator how-to's rendered example and this reference stay in
lockstep — see
[Review the Overseer's workstream gaps](../howto/review-overseer-workstream-gaps.md).

## Tick counters and totals

`OverseerTickReport` (`src/overseer/wiring.rs`) and `OverseerTotals`
(`src/overseer/activity.rs`) each gain **two** additive, `#[serde(default)]`
counters — mirroring the dedicated `goals_escalated` / `goals_health_suppressed`
pair `goal_health` already added. Because they default, the activity-feed
`SCHEMA_VERSION` **stays 1** (additive change, forward/backward tolerant):

| Field | Type | Meaning |
|---|---|---|
| `OverseerTickReport.workstream_gaps_detected` | `usize` | Genuine, deduped backlog-coverage gaps **flagged** this tick — operator notified after coverage-set dedupe and gate suppression. |
| `OverseerTickReport.workstream_gaps_suppressed` | `usize` | Gaps whose signature was already committed within the dedup window this tick — not re-notified. |
| `OverseerTotals.workstream_gaps_detected` / `…_suppressed` | `u64` | The same two, summed over the retained activity window. |

Here is the part the counter flow **must** get right. In the Overseer,
`overseer_tick` calls `act` **once per admitted intervention**, each `act`
returns **exactly one** `ActOutcome`, and `tally_outcome` bumps **one** counter
for it (`wiring.rs`). So a single act **cannot** emit both an `Escalated` and an
`IssueFiled` outcome — and gap activity therefore does **not** ride the generic
`escalations` / `issues_filed` counters. This is exactly how `goal_health`
behaves: `act_escalate_blocked_goal` notifies **both** channels yet returns a
single `GoalEscalated`, bumping the dedicated `goals_escalated`, never the
generic `escalations`.

The consolidated gap act does the same. It performs its notification side
effect and returns **one** summarising outcome carrying the batch
counts, which a **new** `tally_outcome` arm sums into the two dedicated counters.
`tally_outcome`'s `match` is **exhaustive** (no wildcard arm), so the compiler
**forces** that new arm — nothing rides an existing counter "for free":

```rust
pub enum ActOutcome {
    // … existing variants …
    /// The consolidated result of one gap-scan act: `flagged` genuine gaps were
    /// surfaced (operator notified on both channels), and `suppressed` gaps
    /// matched an already-committed signature within the dedup window (not
    /// re-notified). Mirrors how the
    /// per-goal `GoalEscalated` / `GoalHealthSuppressed` feed dedicated goal
    /// counters — here batched, because one gap act handles the whole pass.
    WorkstreamGapsFlagged { flagged: usize, suppressed: usize },
}
```

```rust
// The new, compiler-forced arm in `tally_outcome`:
ActOutcome::WorkstreamGapsFlagged { flagged, suppressed } => {
    report.workstream_gaps_detected += flagged;
    report.workstream_gaps_suppressed += suppressed;
}
```

A tick whose gaps were **all** duplicates returns
`WorkstreamGapsFlagged { flagged: 0, suppressed: N }` — no notification,
only `workstream_gaps_suppressed` moves.

## Rendering on the Overseer surfaces

The gap-scan lights up the **same** Overseer surfaces as every other
intervention (see the [activity feed reference](./overseer-activity-feed.md)) —
dashboard **Overseer** tab, TUI **Overseer** pane, `simard status`, and
`GET /api/overseer` — because it writes through the same `OverseerTickReport`.

`activity::humanize_tick` gains one clause driven by the **dedicated**
`workstream_gaps_detected` counter — alongside the existing `goals_escalated`
("escalated N blocked goals for human review") and `goals_unblocked` clauses, and
**separate** from the generic `filed N issues` / `escalated N to the operator`
clauses (which gaps do not bump). A tick that flagged gaps reads, for example:

> saw 3 problems, flagged 2 workstream gaps

When the scan runs and finds nothing uncovered, it adds **no** clause — a clean
board is the honest "observing, 0 interventions" state, never a fabricated line.

### `GET /api/overseer` JSON

The new counter rides the existing auth-gated response (`GET /api/overseer`, the
`simard_session` cookie or `Authorization: ****** — **no new route and no new
auth surface**. It appears in both `totals` and each `recent[].report`:

```jsonc
{
  "section": {
    "data": {
      "totals": {
        "problems": 14, "issues_filed": 5, "recipes_launched": 2,
        "escalations": 3, "held": 4, "errors": 0,
        "workstream_gaps_detected": 3,         // ← additive, serde-default
        "workstream_gaps_suppressed": 1        // ← additive, serde-default
      },
      "recent": [
        {
          "timestamp": "2026-07-06T03:00:00Z",
          "enabled": true,
          "report": {
            // gap-only tick: the 2 gaps are counted on the dedicated gap
            // counters, NOT on issues_filed / escalations (a single act
            // returns a single outcome — see "Tick counters and totals").
            "problems": 2, "issues_filed": 0, "recipes_launched": 0,
            "escalations": 0, "held": 0, "errors": 0,
            "workstream_gaps_detected": 2,     // ← additive, serde-default
            "workstream_gaps_suppressed": 0,   // ← additive, serde-default
            "panicked": false, "duration_ms": 912
          }
        }
      ]
    }
  }
}
```

An older reader that does not know `workstream_gaps_detected` ignores it; an
older file that lacks it defaults to `0`.

## Configuration

The gap-scan is **on by default whenever the acting Overseer runs** (opt-**out**),
consistent with `goal_health` and the whisperer. Two additive knobs in
`src/overseer/config.rs`:

| Env var | Effect | Default |
|---|---|---|
| `SIMARD_OVERSEER_GAP_SCAN` | Kill-switch. An explicit falsey value (`0`/`false`/`no`/`off`, case-insensitive) **disables** the gap-scan; unset or truthy keeps it on. A disabled acting Overseer (`SIMARD_OVERSEER_ENABLED` falsey) forces it off — a gap-scan only makes sense while the Overseer runs. | **on** |
| `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` | Run the scan every *Nth* Overseer tick (e.g. `4` ≈ hourly at the default 15-minute cadence). Unset/empty/unparseable → `1` (every tick). **Clamped to a floor of `1`** so a `0`/garbage value can never disable the scan by stealth or divide-by-zero. | `1` |

Resolved by opt-out helpers that mirror `goal_health_enabled_from`:

```rust
pub fn gap_scan_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool;
pub fn gap_scan_enabled() -> bool;
pub fn gap_scan_every_n_from(lookup: impl Fn(&str) -> Option<String>) -> u64; // clamped ≥ 1
pub fn gap_scan_every_n() -> u64;
```

The scan honours the Overseer's shared cadence (`SIMARD_OVERSEER_INTERVAL_SECS`,
15-minute default, clamped to a 60 s floor) — `EVERY_N` multiplies that interval
rather than introducing a second clock. There is no per-scan state file: notify
-path dedup lives in the in-process `WhisperGate` and coverage-path dedup in the
in-process in-flight / exponential-backoff guards; durable findings are issues or
code, never committed snapshot docs. A durable GitHub-side open-issue check is
future work ([#4717](https://github.com/rysweet/Simard/issues/4717)).

## Using cognitive memory (best-effort)

Where available, the gap-scan reads Simard's **cognitive memory** to recall prior
gaps and their outcomes — e.g. "this same anomaly was flagged and fixed last
week" — so a recently-resolved gap is not re-surfaced as new. This recall is
**best-effort and read-only**: if cognitive memory is unavailable it degrades to
an empty recall (logged once), and detection proceeds on the live picture alone.
It never blocks a tick and never writes.

## Guarantees

- **Additive.** New signal/problem/outcome variants, new `ObservedState` and
  `OverseerTickReport` fields, new config knobs — no existing behaviour changes,
  no `pub` item removed, `SCHEMA_VERSION` unchanged (serde-default).
- **Genuine gaps only.** Every candidate is deduped against the coverage set
  (in-flight refs ∪ open PRs); blocked goals are delegated to `goal_health`, so
  the scan never re-flags work already in motion.
- **Deduped delivery.** In-process dedup on the notify path (the `WhisperGate`
  `gap_gate`) yields **one** operator notification per recurring gap signature,
  and the coverage path's in-flight + exponential-backoff guards yield **one**
  covering launch per signature — never a flood. Neither path files a GitHub
  issue today; a durable cross-process check is future work
  ([#4717](https://github.com/rysweet/Simard/issues/4717)).
- **Reuses existing plumbing.** The notify path goes through the same
  `DualChannelNotifier` `goal_health` / M1 use — same escalation, same gates, no
  `--admin`, no `--no-verify`, no new bypass. Since
  issue #4128 a coverage gap decides to a **gated closing-edge** `LaunchRecipe`
  (tagged `WORKSTREAM_COVERAGE_GROUP`): it is admitted only through the existing
  launch gate, **fails closed** without a distinct steward identity, and is
  in-flight-deduped — it opens no unguarded auto-launch path. Any other anomaly
  fix still rides the Overseer's pre-existing `LaunchRecipe` path and its counter,
  unchanged.
- **Fails closed and never panics.** The gate fails closed on identity errors;
  the detector degrades every unreadable source to empty; a panicking tick is
  isolated and recorded, never swallowed.
- **Honest surfaces.** Gaps render on the existing Overseer surfaces from
  structured fields (G3); a clean board adds no line and fabricates no `0`.
- **Bounded and safe.** Gaps-per-tick and field lengths are bounded (V4); every
  signature is a restricted slug (V3); external issue/PR text is escaped,
  truncated, and provenance-labelled before it reaches a notification, an issue
  body, or a `gh` argument (C1).

## Security notes

External GitHub issue and PR text is **untrusted input** that flows into
notifications, launched coverage `task_description`s, and `gh` reads. The gap-scan therefore:

- **A1/A2:** exposes the new counter only on the already `require_auth`-gated
  `/api/overseer` (no new/unauthenticated route); `act_flag_workstream_gaps` **fails
  closed** on an identity/gate error.
- **A3:** uses no `--admin` / `--no-verify` / bypass flag; least privilege only.
- **V1/V2:** preserves the no-shell `Command::args` pattern for every `gh` call;
  validates `owner/repo` and numeric issue numbers, and only passes untrusted
  refs **after** a fixed flag or `--`.
- **V3/V4:** restricts dedup signatures to `[A-Za-z0-9_\-#/:]`; bounds
  gaps-per-tick, field lengths, and any `gh issue list --limit`.
- **C1/C2:** renders external titles as escaped, truncated, provenance-labelled
  plaintext; templates any launched `task_description` from structured data,
  never raw issue text.
- **D1/D2:** never emits tokens, `SIMARD_DASHBOARD_TOKEN`, internal paths, or
  stack traces into notifications, issues, or logs; keeps `gh` credentials
  ambient.
- **S1/S2/S3:** the only launch the gap-scan adds — the issue #4128 coverage
  closing edge — stays **behind the existing launch gate** plus a fail-closed
  steward-identity guard and in-flight dedup (no unguarded auto-launch path);
  in-process dedup prevents notification and duplicate-launch floods; the `SIMARD_OVERSEER_GAP_SCAN`
  kill-switch is honoured (its opt-out holds the coverage launch too).

## Testing

The gap-scan ships with **hermetic** tests (`src/overseer/tests_gap_scan.rs`)
built on synthetic pictures — no network, no real `gh`, no clock dependence:

- **Detects a real gap.** Given a picture with an **uncovered p1 goal** (active,
  high-priority, no engineer, no PR) **and** an **unaddressed anomaly**, the scan
  emits a `Signal::WorkstreamGap` whose `GapItem`s carry those specific `ref_id`s,
  titles, and `why_it_matters`, classified into a `WorkstreamCoverage` problem.
- **Dedupes on repeat.** Re-running the scan on the same picture suppresses both
  gaps (gate hit) — no second notification, no second issue — the act returns
  `WorkstreamGapsFlagged { flagged: 0, suppressed: 2 }`, so only
  `workstream_gaps_suppressed` moves.
- **Ignores covered work.** A p1 goal that *has* an open PR, or an issue with an
  in-flight workstream, produces **no** gap.
- **Delegates blocked goals.** A `Blocked` / "needs human review" goal is left to
  `goal_health` and is **not** re-flagged as a gap (no double-notify).
- **Honours the kill-switch.** `SIMARD_OVERSEER_GAP_SCAN=0` yields zero gaps
  regardless of the picture; `EVERY_N` clamps to `1`.
- **Neutralises hostile input.** A gap whose source title contains shell/markup
  metacharacters renders inert (escaped/truncated) and produces a sanitized
  signature slug.

## See also

- [Review the Overseer's workstream gaps](../howto/review-overseer-workstream-gaps.md)
  — the operator walkthrough with rendered output.
- [Overseer activity feed reference](./overseer-activity-feed.md) — the tick
  report, totals, and `GET /api/overseer` surface this scan writes through.
- [Overseer design](../design/overseer.md) — the meta-OODA loop, the
  capability/guardrail model, and the `goal_health` pattern this scan mirrors.
- [Stewardship API](./stewardship-api.md) — the deduped issue-filing path
  (`failure_signature` / `find_existing`) used by the Overseer's
  `QualityRegression` CI-cluster path; the gap scan does **not** file issues but
  shares the same signature-stability philosophy.
- [No-progress breaker API](./no-progress-breaker-api.md) — the "needs human
  review" marker delegated to `goal_health` rather than re-flagged as a gap.
