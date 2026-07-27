---
title: Goal-reblock backoff & stewardship dedup — API reference
description: >
  The typed surface of the Overseer's goal-reblock suppression rail (#4817 /
  #4828): the `goal_reblock_backoff` `BackoffGate` field and its
  `overseer-obs:goal:blocked:{goal_id}` key, the peek/commit wiring in the
  Overseer `gate()` / `act()` goal-hygiene path, the stabilised `GoalBlocked`
  failure-signature text, the `normalize_for_signature` counter-redaction
  contract in `src/stewardship/dedup.rs`, and the goal_id normalization applied
  before keys and issue text.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/goal-reblock-backoff-dedup.md
  - ./overseer-backoff-gate-api.md
  - ./stewardship-api.md
  - ./overseer-recipe-launch-idempotency.md
  - ../concepts/gap-scan-backoff-dedup.md
  - ../howto/diagnose-recurring-goal-reblock-churn.md
---

# Goal-reblock backoff & stewardship dedup — API reference

> **Status: implemented (#4817 / #4828).** The `goal_reblock_backoff` gate field
> and its `gate()`/`act()` wiring, plus the stabilised `GoalBlocked` signature
> text, live in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs);
> the shared `BackoffGate` primitive in
> [`src/overseer/guardrails.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/guardrails.rs);
> and the counter-redaction rule in
> [`src/stewardship/dedup.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/dedup.rs).
> For the rationale see
> [goal-reblock backoff & stewardship dedup](../concepts/goal-reblock-backoff-dedup.md).

## The gate field

```rust
// in the Overseer struct (src/overseer/mod.rs)
/// Per-goal exponential-backoff suppression for still-blocked GoalHygiene
/// briefs, which have `sequence_group = None` and so escape the gap-scan /
/// coverage in-flight guards. Keyed on `overseer-obs:goal:blocked:{goal_id}`.
goal_reblock_backoff: BackoffGate,
```

It is the same [`BackoffGate`](./overseer-backoff-gate-api.md) primitive used by
the gap-scan rail (`peek` / `commit` / `admit`, bounded exponential window,
clock-regression-safe, reset-on-long-silence). It is constructed from the shared
`SIMARD_OVERSEER_BACKOFF_*` window configuration (see
[BackoffGate reference](./overseer-backoff-gate-api.md#configuration)).

### The dedup key

```rust
let key = format!("overseer-obs:goal:blocked:{}", normalize_goal_id(goal_id));
```

The `overseer-obs:` prefix namespaces it away from the gap-scan keys; the
per-goal suffix means each blocked goal backs off independently.

## Wiring: `gate()` peeks, `act()` commits

Mirroring the gap-scan idempotency pattern
([recipe-launch idempotency](./overseer-recipe-launch-idempotency.md)):

- **`gate()`** — before admitting a GoalHygiene relaunch, it
  `goal_reblock_backoff.peek(key, now_secs)`. On `BackoffDecision::Suppress` the
  relaunch is dropped for this cycle (the goal is still blocked / in-flight);
  on `Admit` it proceeds.
- **`act()`** — after a **successful** relaunch it
  `goal_reblock_backoff.commit(key, now_secs)`, arming/growing the window. A
  launch that is itself held (`held: per-cycle launch cap reached`) or fails does
  **not** commit, so it does not consume the dedup slot.

```rust
match self.goal_reblock_backoff.peek(&key, now_secs) {
    BackoffDecision::Suppress => { /* skip relaunch; goal still blocked */ }
    BackoffDecision::Admit => {
        // ... relaunch the covering workstream ...
        // on success only:
        self.goal_reblock_backoff.commit(&key, now_secs);
    }
}
```

Suppression applies **only to the relaunch**. The single stewardship issue for
the blocked goal is still filed (see below) so the human always has visibility.

## Stabilised `GoalBlocked` signature

The `GoalHygiene` problem's `dedup_key` is unchanged
(`goal:blocked:{goal_id}`), but the **signature-bearing error text** no longer
embeds the fluctuating counter:

```rust
// BEFORE (#4817/#4828): counter in the signature text → signature changed every tick
format!("goal {goal_id} blocked ({consecutive_no_action} no-action cycle(s))")

// AFTER: counter removed from the signature input; kept only in the issue body/title
format!("goal {goal_id} blocked{}",
        if needs_review { " — needs human review" } else { "" })
```

`failure_signature(ProblemKind::GoalHygiene, text)` therefore produces a
**stable** signature across cycles, so
[`find_existing()`](./stewardship-api.md) matches the already-open issue and the
overseer reuses it instead of filing a duplicate.

The `consecutive_no_action` count is still surfaced to the human — it is written
into the issue **body / title annotation**, just not into the hashed signature
input.

## `normalize_for_signature` counter redaction

Defense-in-depth in `src/stewardship/dedup.rs`: `normalize_for_signature` now
redacts residual counter patterns so any counter that leaks into a signature
input still collapses to a single signature. This sits alongside the existing
UUID/timestamp redaction (`uuid_redaction_tests`).

Redacted patterns (case-insensitive, whitespace-tolerant):

| Pattern | Normalized to |
| ------- | ------------- |
| `(<N> no-action cycle(s))` | `(<count> no-action cycle(s))` |
| `no progress for <N> cycles` | `no progress for <count> cycles` |

Two `GoalBlocked` failures for the same goal that differ **only** by the counter
now share one signature → `MatchedExisting` → one issue.

## `goal_id` normalization (untrusted input)

Before a `goal_id` is embedded in a dedup key or an issue title/body it is
normalized:

- **length-bounded** (truncated to a fixed max),
- **charset-restricted** to `[A-Za-z0-9._:-]` (other characters dropped/replaced).

This prevents a crafted goal id from colliding signatures or injecting content
into a stewardship issue body/title.

## Invariants (asserted by unit tests)

Tests live in
[`src/overseer/tests_goal_health.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/tests_goal_health.rs)
and the `dedup.rs` sibling of `uuid_redaction_tests`:

- **Relaunch suppressed:** a still-blocked goal is not relaunched on the next
  cycle while inside the window.
- **Re-admit on clear:** once the block clears (long silence), the goal
  re-admits promptly.
- **Never starves:** exactly **one** stewardship issue is still filed for a
  suppressed-relaunch goal.
- **Stable signature:** two `GoalBlocked` failures differing only by the
  no-action counter share one signature (`MatchedExisting`).
- **Hostile goal_id:** an oversized / non-charset goal id is normalized before
  it reaches any key or issue text.
- **Failed launch doesn't commit:** a held/failed relaunch does not consume the
  dedup slot (peek-then-commit-on-success).

## See also

- [Overseer BackoffGate reference](./overseer-backoff-gate-api.md) — the shared primitive + config.
- [Stewardship API](./stewardship-api.md) — `failure_signature` / `find_existing`.
- [Goal-reblock backoff & stewardship dedup](../concepts/goal-reblock-backoff-dedup.md) — the rationale.
- [Diagnose recurring goal-reblock churn](../howto/diagnose-recurring-goal-reblock-churn.md) — the runbook.
