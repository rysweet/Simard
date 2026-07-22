---
title: Agentic merge-queue reasoning API reference
description: >
  The observe/orient reasoning pass that populates ObservedState.reasoned_prs,
  triaged_issues, and merge_reasoning_status — the MergeQueueReasoner seam and
  its MergeQueueObserveRequest/Outcome DTOs on merge_queue_observe.rs, the
  fail-closed brief parse and bounded schema, the config::merge_reasoning_scope()
  three-state resolver (Roster default-ON / Explicit / Disabled loud), the
  ReasonedPr / TriagedIssue / MergeReasoningStatus value types, the
  StalePrDetected / DuplicatePrDetected / IssueNeedsWorkstream signals, the
  FlagStalePr / CloseDuplicatePr interventions (RiskClass::MergeAuthority,
  positional argv, never --admin/--no-verify), and the reasoned_prs -> ready_prs
  re-narrowing projection that keeps merge AUTHORIZATION unchanged.
last_updated: 2026-07-19
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/agentic-merge-queue-reasoning.md
  - ../design/agentic-observe-orient-merge-queue.md
  - ./cross-repo-merge-authority.md
  - ./ready-prs-sensor-api.md
  - ./ecosystem-roster-resolution.md
  - ../howto/configure-agentic-merge-queue-reasoning.md
---

# Agentic merge-queue reasoning API reference

This reference documents the observe/orient reasoning pass that surveys the open
merge queue and issue backlog agentically, populating new `ObservedState` fields
each Overseer cycle. For the *why* and the safety posture, see
[the concept](../concepts/agentic-merge-queue-reasoning.md); for the full design,
see [the design spec](../design/agentic-observe-orient-merge-queue.md).

The pass produces a **proposal only**. The authoritative merge decision stays in
[`merge_authority`](./cross-repo-merge-authority.md), reached through the
`reasoned_prs → ready_prs` re-narrowing projection.

## Data types

### `ObservedState` (additive fields)

Three additive fields on `ObservedState`
([`src/overseer/capabilities.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/capabilities.rs)),
all default-empty / default-unknown so existing constructors and the
side-effect-free `observed_from_snapshot` projection compile unchanged. Any
field serialized into the activity feed carries `#[serde(default)]` for
forward-compatible deserialization of older snapshots.

```rust
pub struct ObservedState {
    // ... existing fields ...

    /// Agentic reasoning over the whole open-PR queue across the reasoning
    /// scope. Non-empty even when SIMARD_AUTOMERGE_* are unset (default-ON).
    #[serde(default)]
    pub reasoned_prs: Vec<ReasonedPr>,

    /// Agentic triage of the open-issue backlog.
    #[serde(default)]
    pub triaged_issues: Vec<TriagedIssue>,

    /// Whether merge reasoning is active, and if not, WHY (loud disablement).
    #[serde(default)]
    pub merge_reasoning_status: MergeReasoningStatus,
}
```

### `ReasonedPr`

```rust
pub struct ReasonedPr {
    /// `owner/name` — MUST be in the reasoning scope (roster/explicit).
    pub repo: String,
    pub pr: u32,
    pub disposition: PrDisposition,
    /// One-line agent rationale (bounded; sanitized before display/notify).
    pub rationale: String,
    /// The original PR number when `disposition == Duplicate`, else `None`.
    pub duplicate_of: Option<u32>,
}

pub enum PrDisposition {
    /// Agent proposes this PR is ready — subject to the re-narrowing projection.
    ReadyForMerge,
    NeedsWork,
    Stale,
    Duplicate,
}
```

### `TriagedIssue`

```rust
pub struct TriagedIssue {
    pub repo: String,   // MUST be in the reasoning scope
    pub issue: u32,
    pub priority: IssuePriority,   // High | Medium | Low
    pub readiness: IssueReadiness, // Ready | Blocked | NeedsInfo
    /// The single next action the agent recommends (bounded, sanitized).
    pub next_action: String,
}
```

### `MergeReasoningStatus`

```rust
pub enum MergeReasoningStatus {
    /// Not yet resolved this pass (the additive default so existing constructors
    /// compile unchanged).
    #[default]
    Unknown,
    /// Reasoning ran over the full governed roster (SIMARD_MERGE_REASONING_SCOPE
    /// unset — default-ON).
    RosterWide,
    /// Reasoning ran over an operator-narrowed explicit scope.
    Narrowed { repos: Vec<String> },
    /// Reasoning is explicitly DISABLED — surfaced loudly. Carries the reason so
    /// the status line names WHY.
    Disabled { reason: String },
}
```

`Disabled` is what makes disablement loud: it is set from the `off`/`disabled`
env value, surfaced in `simard status` / the dashboard, and triggers a one-time
`NotifyOperator` note.

## Configuration resolver — `merge_reasoning_scope()`

Follows the repo's `*_from(lookup)` testable pattern in
[`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs):
a pure resolver takes an env lookup closure + the governed roster, and a thin
production entry reads the real environment.

```rust
pub const SIMARD_MERGE_REASONING_SCOPE_ENV: &str = "SIMARD_MERGE_REASONING_SCOPE";

pub enum MergeReasoningScope {
    /// Unset ⇒ reason over the governed-repos roster (DEFAULT-ON).
    Roster,
    /// Explicit comma-separated `owner/name` list.
    Explicit(Vec<String>),
    /// Explicit `off`/`disabled`/falsey ⇒ reasoning DISABLED (loud).
    Disabled,
}

/// Pure, unit-testable resolver. `roster` is the validated governed-roster slug
/// list (resolved from identity-scoped state), used as the default scope when the
/// env var is unset.
pub fn merge_reasoning_scope_from(
    lookup: impl Fn(&str) -> Option<String>,
    roster: &[String],
) -> MergeReasoningScope;

/// Production entry: reads SIMARD_MERGE_REASONING_SCOPE and the governed roster.
pub fn merge_reasoning_scope() -> MergeReasoningScope;
```

| `SIMARD_MERGE_REASONING_SCOPE` | Result | Reasoning |
|---|---|---|
| unset | `Roster` | ON over governed repos + Simard |
| `""` / whitespace | `Roster` | ON |
| `rysweet/Simard,rysweet/azlin` | `Explicit([…])` | ON, narrowed |
| `off` / `disabled` / `0` / `false` / `no` | `Disabled` | **OFF, LOUD** |

> **Unset ≠ disabled.** The old `SIMARD_AUTOMERGE_REPOS` conflated them (unset ⇒
> silent zero reasoning). Here only an explicit off value disables, and it is
> announced on every channel. Roster resolution uses the single
> [governed-roster loader](./ecosystem-roster-resolution.md) over Simard's
> identity-scoped state; an empty roster is a loud error.

## The reasoner seam

A single trait carries the agentic pass, mirroring the `ecosystem-observe`
`EcosystemObserver` seam. It returns an **opaque `String` brief** (or `None`),
so no Rust type ever encodes reasoning — the rail forwards the payload and parses
it fail-closed.

```rust
/// The thin rail. Invokes the `observe-merge-queue` recipe on the Overseer
/// cadence and returns its OPAQUE brief. Never runs `gh`, never reasons, never
/// merges/comments/closes.
pub trait MergeQueueReasoner {
    fn observe(
        &self,
        request: MergeQueueObserveRequest,
    ) -> SimardResult<Option<String>>;
}

pub struct MergeQueueObserveRequest {
    /// Reasoning scope resolved by `merge_reasoning_scope()`. Named `scope`
    /// (not `roster`, as in `EcosystemObserveRequest`) because it is the
    /// *resolved* reasoning scope: the governed roster by default, but possibly
    /// a narrower subset when `SIMARD_MERGE_REASONING_SCOPE` lists explicit
    /// slugs. The rail serializes it to the recipe's `roster_path` ContextFile.
    pub scope: Vec<String>,
    /// Simard's in-flight OODA refs, for the agent's dedup reasoning. A plain
    /// `Vec<String>`, mirroring `EcosystemObserveRequest::inflight_refs`.
    pub inflight_refs: Vec<String>,
    /// Empty on the base pass; rail-set on escalation-ladder retries. Rail-owned,
    /// never a caller parameter (mirrors `EcosystemObserveRequest`).
    pub escalation_note: String,
}

pub struct MergeQueueObserveOutcome {
    pub reasoned_prs: Vec<ReasonedPr>,
    pub triaged_issues: Vec<TriagedIssue>,
}
```

- **`Ok(Some(brief))`** — an opaque semantic brief string to parse fail-closed.
- **`Ok(None)`** — nothing produced this pass.
- **`Err(_)`** — infrastructure fault: caller logs a `WARN`, skips the pass, and
  fabricates **no** reasoned PRs/issues (fail-closed).

The production implementation is `SpawnMergeQueueRecipeRunner`
([`src/overseer/merge_queue_observe.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/merge_queue_observe.rs)),
registered in [`wiring.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/wiring.rs)
alongside `SpawnEcosystemRecipeRunner`. Tests use an in-crate fake that returns a
canned brief.

### Fail-closed brief parse

`parse_merge_queue_brief(brief, scope) -> MergeQueueObserveOutcome` normalizes
the bounded JSON schema (see the design spec) into typed values, **dropping**:

- any entry whose `repo` is **not in `scope`** (roster trust boundary),
- any entry missing a required field or with an unknown `disposition`/enum,
- a `Duplicate` PR with no `duplicate_of`.

A whole-brief parse error yields **empty** outcome + a `WARN`. There is no error
variant that maps to an action — the type system forces fail-to-empty.

### Recipe & context vars

The recipe `prompt_assets/simard/recipes/observe-merge-queue.yaml` runs two agent
steps (REASON → BRIEF) with a `ContextFile` semantic handoff — the same
convention as `ecosystem-observe.yaml`. `gh` runs **inside** the REASON step,
read-only, never in Rust.

| Var (`-c <key>_path`) | Meaning |
|---|---|
| `roster_path` | Resolved reasoning `scope` slugs (rail writes them to a `ContextFile`). |
| `inflight_refs_path` | Simard's in-flight OODA refs, for dedup. |
| `merge_queue_brief_path` | Shared handoff: REASON writes the bounded brief, BRIEF reads it. |
| `escalation_note` | Empty on base attempt; rail-set on escalation retries. |

All unbounded values ride `ContextFile` paths (`ARG_MAX`-safe); the rail is
supervised by idle/liveness only — **no wall-clock timeout** on the agentic step.

## Signals (`src/overseer/signal.rs`)

Derived from `reasoned_prs` / `triaged_issues`:

| Signal | Source | Downstream |
|---|---|---|
| `Signal::PrReadyToMerge { repo, pr }` | `ReadyForMerge` PR that survives the re-narrowing projection | existing `DeliveryReady → VerifyAndMergePr` chain |
| `Signal::StalePrDetected { repo, pr }` | `disposition == Stale` | `FlagStalePr` |
| `Signal::DuplicatePrDetected { repo, pr, duplicate_of }` | `disposition == Duplicate` | `CloseDuplicatePr` |
| `Signal::IssueNeedsWorkstream { repo, issue, next_action }` | `readiness == Ready` (+ priority) | existing workstream/brief launch |

## Interventions (`src/overseer/intervention.rs`)

Two new interventions, both `RiskClass::MergeAuthority` (the same opt-in autonomy
gate as `VerifyAndMergePr`; **notify-only** when the gate is off):

```rust
pub enum Intervention {
    // ... existing variants ...

    /// Post a triage comment on a stale engineer PR. `gh pr comment` only —
    /// never merges/closes. Positional argv; never --admin/--no-verify.
    FlagStalePr { repo: String, pr: u32, note: String },

    /// Close a duplicate engineer PR with a comment referencing the original.
    /// `gh pr close` only. Positional argv; never --admin/--no-verify.
    CloseDuplicatePr { repo: String, pr: u32, duplicate_of: u32 },
}
```

Guarantees (unit-tested):

- **argv never contains `--admin` or `--no-verify`** (mirrors the conflict-path
  refusal test).
- **positional argv only**, via `sanitize_context_var` — no shell, injection
  structurally impossible.
- **anti-recursion author guard + engineer-PR narrowing** applied: they act only
  on Simard's own engineer PRs, never an operator's review PR.
- **opt-in**: with the `MergeAuthority` autonomy gate off, they are notify-only.

## The re-narrowing projection (`src/overseer/mod.rs`)

The seam that keeps reasoning broad and authorization narrow. A `ReadyForMerge`
`ReasonedPr` is projected to `ObservedState.ready_prs` **only if** it
independently re-passes, in order:

1. the anti-recursion **author guard** (not `simard-overseer[bot]`; matches the
   engineer identity),
2. the **engineer-PR narrowing** — the `simard-autonomous` label **OR** an
   engineer-exclusive branch namespace (`is_engineer_branch`),
3. the **objective gates** — `evaluate_objective_gates(&snap, &self.base_allowlist)`:
   base-branch allowlist + `mergeable == "MERGEABLE"` + every check in
   `{SUCCESS, NEUTRAL, SKIPPED}`.

Survivors flow into the **existing, unchanged** chain:

| Stage | Location |
|---|---|
| `Signal::PrReadyToMerge` | `src/overseer/signal.rs` |
| `ProblemKind::DeliveryReady` | Orient |
| `Intervention::VerifyAndMergePr` (`allow_verify_merge`) | `src/overseer/mod.rs` |
| `caps.prs.merge()` → RecursionGuard → `verify()` → poll-until-green | `src/overseer/merge_ops.rs` |
| `merge_pr_if_merge_ready_with_judge` (objective gates + `MergeJudge`) → `gh pr merge --squash --delete-branch` | `src/stewardship/merge_authority.rs` |
| `NotifyOperator` (email + Signal) | on every merge |

The agent's `ReadyForMerge` disposition is a **proposal**; this projection is the
**authorization**. Broadening reasoning scope can never widen this gate.

## Error & edge-case matrix

| Condition | Result |
|---|---|
| `SIMARD_MERGE_REASONING_SCOPE` unset | reasoning ON over governed roster (`RosterWide`) |
| `SIMARD_MERGE_REASONING_SCOPE=off`/`disabled`/falsey | reasoning OFF, **loud** (`Disabled`, WARN, one-time notify) |
| recipe runner `Err` / empty roster / unusable brief | empty `reasoned_prs`/`triaged_issues` + WARN (fail-closed) |
| brief entry `repo` not in scope | dropped (roster trust boundary) |
| brief entry missing field / unknown enum | dropped |
| `ReadyForMerge` PR fails author guard / engineer-PR gate / objective gate | **not** projected to `ready_prs` (excluded) |
| operator review PR (shared login, shared branch prefix, no label) | never projected |
| `Stale` PR | `FlagStalePr` (comment) — never merge/close |
| `Duplicate` PR | `CloseDuplicatePr` (close + comment) referencing `duplicate_of` |
| any argv containing `--admin`/`--no-verify` | impossible — asserted by unit test + repo-wide grep guard |

## Invariants

- The reasoning pass **never merges, comments, or closes** — it only populates
  `ObservedState`. Actions happen through the gated interventions.
- Reasoning is **default-ON** (unset ⇒ roster); disablement is **loud**
  (`Disabled` + WARN + one-time notify). Never a silent OFF.
- Fail-closed & fail-visible: every recipe/parse fault yields empty sets + a log
  line, never a fabricated PR or a silent wrong action.
- The `reasoned_prs → ready_prs` projection **only narrows**: it can never make a
  foreign-authored or objectively-ineligible PR a merge candidate.
- The authoritative merge gate in `merge_authority` is **unchanged** and still
  **never** uses `--admin` or `--no-verify`.
- New interventions build **positional argv only** and are `RiskClass::MergeAuthority`
  opt-in.
- Every merge notifies `rysweet` on **email and Signal**.
