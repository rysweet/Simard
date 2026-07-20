---
title: Overseer BackoffGate & gap-scan dedup reference
description: >
  The typed surface of the Overseer's gap-scan duplicate-suppression rail
  (#4186, meta bugs #4255 / #4126): the `BackoffGate` exponential-backoff
  suppression primitive and its `BackoffDecision` enum in
  `src/overseer/guardrails.rs`, the `SIMARD_OVERSEER_BACKOFF_*` configuration
  accessors and their fail-safe clamps in `src/overseer/config.rs`, and how the
  gate is wired into the Overseer `gate()` / `act()` gap-scan `LaunchRecipe`
  path so an already-covered coverage gap is not re-launched every tick.
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/gap-scan-backoff-dedup.md
  - ../howto/configure-overseer-gap-scan-backoff.md
  - ./overseer-workstream-gap-scan.md
  - ./overseer-recipe-launch-idempotency.md
  - ./overseer-memory-recall-api.md
  - ./overseer-self-observation-stability.md
  - ../howto/diagnose-recurring-cognitive-memory-signature.md
  - ../howto/review-overseer-workstream-gaps.md
  - ../concepts/operational-autonomy-model.md
  - ../design/overseer.md
---

# Overseer BackoffGate & gap-scan dedup reference

> **Status: implemented (#4186).** The `BackoffGate` primitive and its
> `BackoffDecision` enum live in
> [`src/overseer/guardrails.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/guardrails.rs),
> the `SIMARD_OVERSEER_BACKOFF_*` accessors in
> [`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs),
> and the wiring into the gap-scan `gate()` / `act()` path in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs).
> This rail closes the dedup/backoff gap tracked by meta bugs
> [#4255](https://github.com/rysweet/Simard/issues/4255) and
> [#4126](https://github.com/rysweet/Simard/issues/4126). For the rationale see
> [gap-scan dedup & backoff](../concepts/gap-scan-backoff-dedup.md); for the
> operator knobs, the [configure how-to](../howto/configure-overseer-gap-scan-backoff.md).

## What this fixes

Before this rail the Overseer's gap-scan re-emitted the **same** coverage gap
every tick. A single uncovered backlog workstream produced a stream of
byte-identical *"Cover uncovered backlog workstream(s)"* issues
([#4186](https://github.com/rysweet/Simard/issues/4186),
[#4190](https://github.com/rysweet/Simard/issues/4190),
[#4191](https://github.com/rysweet/Simard/issues/4191),
[#4198](https://github.com/rysweet/Simard/issues/4198),
[#4201](https://github.com/rysweet/Simard/issues/4201),
[#4203](https://github.com/rysweet/Simard/issues/4203),
[#4206](https://github.com/rysweet/Simard/issues/4206)) and re-surfaced an
already-seen `RecurringSignature`
([#4108](https://github.com/rysweet/Simard/issues/4108),
[#4124](https://github.com/rysweet/Simard/issues/4124)) on a fixed cadence.

The gap-scan act path had **no suppression between an already-emitted gap and
its next re-emission**. The [`WhisperGate`](./overseer-memory-recall-api.md) and
the [recipe-launch idempotency rail](./overseer-recipe-launch-idempotency.md)
each cover a *different* seam (whisper delivery; in-flight recipe processes) and
do not stop a **completed** gap-cover launch from re-firing a tick later. This
reference documents the primitive that closes that seam.

## Two-layer suppression contract

Duplicate gap-cover work is suppressed by the in-process `BackoffGate` (layer 1),
which fails toward surfacing — a suppressed action is only ever *not taken*.

| Layer | Scope | Fails toward | Source | Status |
|-------|-------|--------------|--------|--------|
| **1. `BackoffGate`** | In-process, per `dedup_key`, exponential window on an injected clock | **Surface** (a suppressed action is only ever *not taken*) | `guardrails.rs` | **Implemented (#4186)** |
| **2. Open-issue equivalence check** | Cross-process, best-effort GitHub query for an already-open equivalent issue | **Surface** (API error ⇒ treat as "no duplicate", fall back to layer 1) | `stewardship::dedup` + `WorkstreamCoverage` routing | **Implemented** — catches the cold-start / cross-process case layer 1 cannot |

Layer 1 survives across ticks **within one daemon process**; it is reset by a
daemon restart. Layer 2 is the **cross-process** guard that catches the
cold-start case — an equivalent gap-cover issue is **already open on GitHub**
when the in-memory `BackoffGate` is empty (fresh daemon, or a different
process/host opened it) — which layer 1 alone cannot see. Together they close the
duplicate-issue pile: layer 1 rate-limits within a process, layer 2 refuses to
open a second issue when GitHub already carries an equivalent open one.

## `BackoffDecision`

```rust
/// A BackoffGate's decision: admit the action (surface the gap), or suppress it
/// because an equivalent action fired within the current exponential window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackoffDecision {
    /// The gap is new, or its backoff window has elapsed — surface it.
    Admit,
    /// An equivalent gap fired within the current window — suppress it.
    Suppress,
}
```

`BackoffDecision` is a parallel enum to
[`WhisperDecision`](./overseer-memory-recall-api.md); it is intentionally **not**
merged into `WhisperDecision` so the two gates' semantics stay independent and
the change is additive (existing `WhisperGate` callers are untouched).

## `BackoffGate`

```rust
/// Exponential-backoff duplicate-suppression gate for the gap-scan act path
/// (mirrors the WhisperGate peek/commit/admit shape). Keyed by an opaque
/// `dedup_key`, timed on an INJECTED clock (`now_secs`) so the daemon uses
/// wall-clock while tests drive a virtual clock. The clock type is `i64` to
/// match the existing `WhisperGate` and the `now_secs() -> i64` helper in
/// `mod.rs`, so no lossy `i64 -> u64` cast sits between the act path and the
/// gate (such a cast would turn a regressed clock into a huge positive delta
/// and defeat the clock-regression guard below).
///
/// For each key the gate tracks: the last time an action was admitted, and the
/// current suppression window. On each admit the window GROWS
/// (`window = min(window * multiplier, max_window_secs)`); after a silence of
/// `>= 2 * current_window` the key RESETS to `base_window_secs`, so a genuinely
/// recurring gap is never permanently silenced — only rate-limited.
///
/// Only ADMITTED actions advance a key's window; a suppressed action records
/// nothing. `admit` is the combined decide+commit used in unit tests; the act
/// path peeks then commits only after a successful `LaunchRecipe` so a
/// failed/panicking launch does not consume the backoff slot.
///
/// Per-key state: `(last_admit_secs, current_window_secs)`.
pub struct BackoffGate { /* private: base/mult/cap + per-key state map */ }

impl BackoffGate {
    /// Construct from resolved config. `base_window_secs` is the first window,
    /// `multiplier` (> 1) grows it, `max_window_secs` caps it.
    pub fn new(base_window_secs: i64, multiplier: i64, max_window_secs: i64) -> Self;

    /// Decide WITHOUT recording — the act path uses this so it can commit only
    /// after a successful launch. `Suppress` iff an equivalent key was admitted
    /// within its current window; a clock regression (`now_secs < last_admit`)
    /// is treated as "window elapsed" (fails toward `Admit`) via an explicit
    /// guard, not merely `saturating_sub`.
    pub fn peek(&self, dedup_key: &str, now_secs: i64) -> BackoffDecision;

    /// Record a successful admit of `dedup_key` at `now_secs`: grow the key's
    /// window (`saturating_mul`, hard-capped at `max_window_secs`), or reset it
    /// to `base_window_secs` after a silence of `>= 2 * current_window`.
    pub fn commit(&mut self, dedup_key: &str, now_secs: i64);

    /// Decide and, on `Admit`, record in one call. Returns the decision.
    pub fn admit(&mut self, dedup_key: &str, now_secs: i64) -> BackoffDecision;
}
```

### Window schedule

With the defaults (`base = 900`, `multiplier = 2`, `cap = 86400`) a key that
keeps re-firing walks this window schedule (seconds since its previous admit
below which the next occurrence is suppressed):

| Admit # | Window before next admit |
|---------|--------------------------|
| 1st (new key) | 900 (15 min) |
| 2nd | 1 800 (30 min) |
| 3rd | 3 600 (1 h) |
| 4th | 7 200 (2 h) |
| … | ×2 each time |
| capped | 86 400 (24 h) — never longer |

After `2 × current_window` of silence the key resets to `900`. A brand-new key
always returns `Admit` on its first `peek`.

### Safety invariants

- **Suppression can only *reduce* actions.** The gate never triggers a launch or
  an issue write; a wrong decision can at worst delay surfacing a real gap, never
  fabricate one. Fail-safe by construction.
- **Saturating math only.** Window growth uses `saturating_mul` hard-capped at
  `max_window_secs`; there is no `unwrap()`/`panic!` on the `i64` arithmetic.
- **Clock-regression safe.** A non-monotonic injected `now_secs` (i.e.
  `now_secs < last_admit`) is treated as "window elapsed" → `Admit` (surface, do
  not silence). This is an **explicit `now_secs < last_admit` guard**, not a
  reliance on `saturating_sub` — with a signed clock a regressed delta is
  negative and would otherwise read as "within window" (the latent
  `WhisperGate` behaviour this gate deliberately corrects).
- **Bounded in practice.** Per-key state is a small `(i64, i64)` tuple keyed by
  the coverage `dedup_key`; the number of live keys is bounded by the number of
  distinct active gap signatures (a handful), so it does not grow without bound
  in normal operation.
- **State is in-memory only.** A daemon restart resets the gate. The cold-start
  case (an equivalent issue already open on GitHub when the in-memory gate is
  empty) is caught by the **layer-2 open-issue equivalence check** described
  below, which queries GitHub before creating and so does not depend on
  in-memory state.

## Dedup key

The gate is keyed by **`recipe_dedup_key(brief)`** — the *same* value the
[recipe-launch idempotency rail](./overseer-recipe-launch-idempotency.md)
already keys its `inflight_investigations` map on (see `mod.rs`,
`recipe_dedup_key`, used at the `gate()` in-flight check and the `act()` insert).
Standardising on this one function keeps the BackoffGate and the idempotency rail
keyed off an **identical** signature, so a duplicate cannot slip through a key
mismatch (and the layer-2 open-issue equivalence check reuses the same key).

This is consistent by construction, not by coincidence: `recipe_dedup_key`
extracts the stable `overseer-obs:` token from the brief's task description, and
that token is derived from `problem.dedup_key`. So `recipe_dedup_key(brief)` and
the originating `problem.dedup_key` resolve to the same stable signature for the
coverage path — no separate reconciliation step is required, and the gate must
**not** introduce a third key. For the `RecurringSignature` emit path that
signature is composed as:

```
dedup_key = "{signal_kind}:{stable_signature}"
```

where `stable_signature` is the `failure_signature`-style **stable hash**, not
the human-readable count string. This is deliberate: *"seen 3×"* and *"seen 4×"*
collapse to **one** key, so an incrementing recurrence count does not defeat
suppression.

## Open-issue equivalence check (layer 2)

> **Implemented.** Layer 1 (`BackoffGate`) rate-limits within a process; layer 2
> is the cross-process guard wired into the `WorkstreamCoverage` routing in
> `src/overseer/mod.rs` / `src/overseer/sensor.rs`. Together they stop the
> duplicate *"Cover uncovered backlog workstream(s)"* pile
> ([#4190](https://github.com/rysweet/Simard/issues/4190)…[#4338](https://github.com/rysweet/Simard/issues/4338)).

Before launching a gap-cover recipe (and before the `coverage_backoff` re-admit
window is consulted), the act path performs a **best-effort** GitHub query using
the stewardship helpers, keyed on the **canonical gap signature**, to see
whether an equivalent gap-cover issue is **already open**:

```
stewardship::dedup::failure_signature(kind, text) -> String  // stable signature
gh_client.search_issues(repo, &signature) -> SimardResult<Vec<GhIssue>>  // open-issue query
stewardship::dedup::find_existing(&issues, &signature) -> Option<&GhIssue>  // match (by signature)
```

Note `search_issues` is a method on the stewardship gh-client trait
(`src/stewardship/gh_client.rs`), taking `(repo, signature)` — it is **not** a
free function under `stewardship::dedup`. Only `failure_signature` and
`find_existing` live in `stewardship::dedup`.

Behaviour:

- **Equivalent open issue found** → **skip** the launch and emit a structured
  trace referencing the existing issue number. The open issue is **not** mutated
  (reuse/skip-only, to avoid churn and stay non-breaking) — the existing issue
  *is* the coverage, so no duplicate is created.
- **No match** → fall through to the layer-1 `coverage_backoff` re-admit check,
  then proceed to launch if admitted.
- **GitHub API error** → treat as "no duplicate" (fail toward surfacing) and fall
  back to layer 1 alone. It reuses the existing GitHub client/credentials and
  introduces **no new token or scope**.

> **Out of scope: existing backlog cleanup.** Bulk-closing the ~13 duplicate
> gap-scan issues that were opened before this guard existed is **operator**
> work, not a code path here. Layer 2 is the *creation-side* guard — it stops new
> duplicates, it does not close old ones. Their recurrence root cause (missing
> issue-level dedup) is exactly what this layer removes going forward.

## Wiring into the gap-scan path

The `Overseer` struct gains one field, `coverage_backoff: BackoffGate`,
initialised in the constructor from the resolved config, plus an injected
`clock` (defaulting to `now_secs`, overridable in tests via `with_clock`). The
gate is threaded through the existing decide/act seam:

1. **`gate()`** — after the `inflight_investigations` check and **before** the
   cost gate, `peek(recipe_dedup_key, now_secs)` is consulted **only** for
   coverage briefs (`brief.sequence_group == Some(WORKSTREAM_COVERAGE_GROUP)`).
   On `BackoffDecision::Suppress` the gap-cover plan is held with the reason
   *"held: an equivalent coverage was launched recently (backoff window)"* and
   the tick continues with other work.
2. **`act()`** — on a **successful** coverage `LaunchRecipe` (the
   `Intervention::LaunchRecipe { brief } => { … }` act arm in `mod.rs`, near the
   existing `recipe_dedup_key(brief)` handling; anchor on the match arm, not a
   line number), `commit(recipe_dedup_key, now_secs)` advances the key's window.
   Commit happens **only** on success, so a failed launch leaves the slot free
   to retry next tick.

This mirrors the `WhisperGate` peek-then-commit-on-success discipline exactly.

> **Related intra-cycle fix.** `ConflictSequencer` is a *per-cycle* planning
> accumulator; `run_cycle` now calls `self.sequencer.reset()` at the top of each
> cycle so a launched coverage group does not permanently lock out later cycles
> (which would have masked the backoff behaviour entirely). Cross-cycle dedup is
> the job of the in-flight guard and this `BackoffGate`; the sequencer only
> serialises within a single tick.

## Observability

Suppression is surfaced through the tick's structured plan output — **never**
`print!`/`println!`. When a coverage plan is suppressed it is recorded as a
**held plan** whose reason string appears in the tick's `action_details`:

> `held: an equivalent coverage was launched recently (backoff window)`

This reuses the existing held-plan channel the Overseer already uses for other
gates (e.g. the in-flight guard and cost gate), so no new trace target or metric
is introduced by the layer-1 suppression path. The **layer-2** skip emits a
structured trace referencing the already-open issue number (below). A dedicated
`overseer::gap_scan` counter and the over-silence signal remain **future work**:

| Signal | Status |
|--------|--------|
| Held-plan reason in `action_details` on suppression | **Implemented** |
| Open-issue equivalence skip trace (references the existing issue number) | **Implemented** (layer 2) |
| `overseer::gap_scan` structured counter/trace per suppression | Future work |
| Over-silence alert (window saturated at cap and still suppressing) | Future work |

**Log hygiene:** only opaque signature/dedup keys are surfaced; GitHub tokens,
full issue bodies, and repo secrets are never logged.

## Configuration accessors

Resolved in `src/overseer/config.rs` with the standard `*_from(lookup)` +
fail-safe pattern (mirroring `overseer_interval_from`). See the
[configure how-to](../howto/configure-overseer-gap-scan-backoff.md) for operator
guidance.

| Env var | Accessor | Default | Fail-safe |
|---------|----------|---------|-----------|
| `SIMARD_OVERSEER_BACKOFF_BASE_SECS` | `overseer_backoff_base_secs_from` | `900` | `> 0`; unset/empty/zero/negative/unparseable ⇒ default |
| `SIMARD_OVERSEER_BACKOFF_MULTIPLIER` | `overseer_backoff_multiplier_from` | `2` | `> 1`; `<= 1`/unparseable ⇒ default |
| `SIMARD_OVERSEER_BACKOFF_MAX_SECS` | `overseer_backoff_max_secs_from` | `86400` | `> 0`; zero/negative/unparseable ⇒ default |

Production entrypoints `overseer_backoff_{base_secs,multiplier,max_secs}()` wrap
these accessors over the real environment lookup. There is **no** enable/disable
flag: the gate is always active (it can only ever *reduce* actions, so there is
no safety reason to disable it). Each accessor validates its **own** field
independently and falls back to that field's default on any invalid value; there
is no cross-field clamp (e.g. `max` is not coerced relative to `base`), keeping
each knob's fail-safe simple and predictable.

## Test surface

- **Unit** (`src/overseer/tests_whisper.rs`): virtual-clock coverage of
  suppress-within-window, exponential growth, reset-after-silence, key
  independence, and overflow/clock-regression safety.
- **Integration** (`src/overseer/tests_gap_scan.rs`): a `run_cycle` that
  completes a `WorkstreamCoverage` launch does **not** create a second issue on
  the next tick; a relaunch **is** admitted after the window elapses; and
  distinct gap signatures are unaffected by each other's suppression.

## Related reading

- [Gap-scan dedup & backoff](../concepts/gap-scan-backoff-dedup.md) — the concept
  and rationale.
- [Configure Overseer gap-scan backoff](../howto/configure-overseer-gap-scan-backoff.md)
  — operator knobs, tuning, and verification.
- [Overseer workstream gap-scan](./overseer-workstream-gap-scan.md) — the
  gap-scan step this rail guards.
- [Overseer recipe-launch idempotency](./overseer-recipe-launch-idempotency.md) —
  the sibling rail that dedups *in-flight* recipe processes (different seam).
- [Overseer verify-and-merge escalation convergence](./overseer-merge-escalation-convergence.md) —
  the sibling `BackoffGate`-based rail that converges the `VerifyAndMergePr`
  escalation, keyed on `repo#pr`.
- [Diagnose a recurring cognitive-memory signature](../howto/diagnose-recurring-cognitive-memory-signature.md)
  — the operator-facing symptom this rail resolves.
- [Operational autonomy model](../concepts/operational-autonomy-model.md) — why a
  suppression rail is a preserved safety gate, not a weakening of autonomy.
