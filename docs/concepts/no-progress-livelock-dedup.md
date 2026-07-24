---
title: The no-progress breaker escalates once and halts, instead of livelocking on re-orient
description: >
  How the OODA no-progress / re-orientation breaker stopped spamming near-duplicate
  `ooda-stuck` tracking issues for a still-blocked goal. Dedup is now sourced from a
  remote, restart-durable signature (`ooda-signature:<sig>` embedded in the issue body,
  computed per-goal over the un-redacted `goal_id` via a dedicated
  `no_progress::breaker_signature` helper), backed by an IO-free complement — the
  breaker's tracking-issue `wip_ref` is now PRESERVED across `roll_to_new_cycle`
  (issue #4509), so an in-process re-orient dedups from memory without a `gh` call.
  After a guided retry still leaves the goal Blocked, the breaker escalates exactly once and
  the goal — still standing Blocked with the no-progress sentinel — is skipped on subsequent
  ticks (no re-orientation, no duplicate issue) until its block is lifted, repairing the
  broken escalation + issue-filing path.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
issue: 4497
related:
  - ./blocked-goal-escalation-backoff.md
  - ./no-progress-root-cause-resolution.md
  - ./no-progress-terminal-investigation.md
  - ./steerable-ooda-daemon.md
  - ../reference/whisper-gate-backoff-api.md
---

# The no-progress breaker escalates once and halts

> **Status: implemented (issues #4497, #4499, #4504, #4508, #4509, #4474, #4472).**
> A still-blocked goal now produces **at most one open `ooda-stuck` tracking
> issue**, and a goal that stays blocked after a guided retry is **escalated once
> and then skipped** (its re-orientation is suppressed while it stands Blocked
> with the no-progress sentinel) instead of re-firing on every Overseer tick.
> Primary source:
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs)
> (`breaker_signature`, `escalate_with_tracking_issue`, `NoProgressIssueFiler`,
> `GhIssueFiler`, and the `goal_is_sentinel_blocked` skip guard).

## The defect this fixes

The OODA no-progress / re-orientation breaker was **livelocking**. In one ~6h
window (07:05–12:45Z) it auto-filed five near-duplicate operator-facing tracking
issues for the *same* two still-blocked goals:

- `#4497`, `#4499`, `#4504`, `#4508` — all titled
  *"OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)"*
- `#4509` — *"higher-order re-orientation livelock — standing"*

while goals `4d27c91a` and `7f5afcca` stayed Blocked and never converged. Two
companion issues showed the breaker's own escalation machinery was defective:

- `#4474` — *"no-progress breaker escalation is broken"*
- `#4472` — *"breaker cannot file its operator-facing tracking issue"*

The root cause was two-fold:

1. **Dedup was in-memory only.** `escalate_with_tracking_issue` deduped by
   scanning the goal's in-memory `wip_refs` for a breaker-authored tracking ref
   (`is_breaker_tracking_ref`). But `roll_to_new_cycle` **cleared `wip_refs`** on
   every re-orient, so the dedup memory was wiped each cycle. The next tick saw a
   goal with empty `wip_refs`, concluded "no tracking issue yet", and filed
   another one. Restarts had the same effect. The remote issue list — the actual
   source of truth — was never consulted.

   The fix attacks this on **two** independent fronts (issue #4509): the remote
   signature search below makes dedup durable even when the in-memory link is
   genuinely gone (process restart), **and** `roll_to_new_cycle` now
   *preserves* the breaker's tracking-issue ref (a durable `issue` RECORD, not
   live work — see `WipRef::is_no_progress_tracking` in `src/goal_curation/types.rs`) so an **in-process**
   re-orient keeps deduping from memory, IO-free, with no dependence on `gh`
   availability.

2. **No terminal boundary.** After a guided retry still left a goal Blocked, the
   breaker kept **re-orienting the same goal every tick** with no backoff and no
   terminal state, so it re-entered the escalation path indefinitely.

The result: one perpetually-blocked goal produced an unbounded stream of
duplicate `ooda-stuck` issues, burying the operator and never converging — a
self-inflicted denial-of-service on the issue tracker.

## The fix, in one sentence

Make dedup **remote and restart-durable** (a signature the issue body carries)
**and preserve the breaker's tracking-issue ref across `roll_to_new_cycle`** so
an in-process re-orient dedups IO-free from memory, and make a post-guided-retry
stall **skip**: escalate exactly once, then skip that goal while it stands
Blocked with the no-progress sentinel — until its block is lifted.

## Part 1 — remote, restart-durable dedup

### The signature

Each blocked goal gets a deterministic **per-goal** dedup signature, computed with
the **existing** stewardship hashing convention (16-hex first-8-bytes-of-SHA-256),
so no new hashing primitive is introduced:

```rust
// src/ooda_loop/no_progress.rs
let signature = breaker_signature(goal_id);
```

`breaker_signature` returns a stable 16-hex-character key
(`sha256("ooda-no-progress\n" || goal_id)[..8]`). It is:

- **Deterministic** — the same `goal_id` always yields the same signature.
- **Per-goal-distinct** — two *different* `goal_id`s yield two *different*
  signatures.
- **Stable across re-orient and restart** — it depends only on `goal_id`, not on
  volatile `wip_refs` or process memory.

> **Design constraint — do NOT reuse `failure_signature` verbatim here.**
> `stewardship::dedup::failure_signature` first runs its argument through
> `normalize_for_signature` → `redact_token`, which deliberately folds volatile
> identifiers: any all-hex token of length ≥ 7 collapses to `<HEX>` and any
> canonical UUID collapses to `<UUID>`. Goal IDs are exactly such tokens (the
> real ones observed in this livelock — `4d27c91a`, `7f5afcca` — are 8-char hex),
> so passing a `goal_id` through `failure_signature` would redact **every** goal
> to the *same* placeholder and therefore hash **all goals to one shared
> signature** — collapsing the "per-goal" guarantee and over-deduping a second
> genuinely-stuck goal against the first goal's issue. The signature here must be
> computed over the **un-redacted** `goal_id`. `breaker_signature` reuses the
> *hashing format and marker convention* directly over `goal_id` — **not**
> `failure_signature`'s redaction pipeline.

The signature is embedded in the tracking issue's body as a machine-readable
marker (mirroring stewardship's `stewardship-signature:` convention, but as a
single whitespace-delimited token so it survives body reflow):

```text
ooda-signature:3f9a1c77b0e42d18
```

### Search-before-create

`NoProgressIssueFiler` gains a **read-only** lookup that consults the remote
issue list before creating anything:

```rust
pub(crate) trait NoProgressIssueFiler {
    fn file_issue(&self, title: &str, body: &str) -> Option<FiledIssue>;

    /// Return the open `ooda-stuck` tracking issue whose body embeds
    /// `ooda-signature:<sig>`, if one already exists. Read-only; fail-closed
    /// (returns `None` on any error, and the caller then treats the goal as
    /// still Blocked without filing — never files a duplicate on uncertainty).
    fn find_open_tracking_issue(&self, signature: &str) -> Option<FiledIssue>;
}
```

The production `GhIssueFiler` implements it with an **argv-only** listing — never
a shell string — then scans each returned body for the marker locally (a
substring search is used rather than `--search` so the match is exact and
independent of GitHub's search tokenizer):

```bash
gh issue list --state open --label ooda-stuck --limit 200 \
  --json number,url,body
```

If the list command fails, `GhIssueFiler` logs the failure with
`tracing::error!(target: "simard::ooda", …)` and returns `None`; it **never
aborts the cycle** (this is the direct repair of `#4472` / `#4474`).

### The escalation check order

`escalate_with_tracking_issue` applies its checks in this order, guaranteeing
**≤ 1 open `ooda-stuck` issue per blocked goal**:

1. **Live `wip_ref` check** — if the current in-memory goal still carries a
   breaker-authored tracking ref, reuse it (no remote call). Because
   `roll_to_new_cycle` now **preserves** that ref (issue #4509), this fast path
   survives an in-process re-orient — the remote search is only needed after a
   true process restart, when in-memory state is genuinely gone.
2. **Remote signature search** — call `find_open_tracking_issue(signature)`. If a
   matching open issue exists, re-link it to the goal and file nothing. This
   survives process restarts (and any path where the in-memory ref was lost).
3. **File** — only when both of the above miss, `file_issue` creates one and
   embeds the `ooda-signature:` marker; the result is linked back to the goal via
   `link_tracking_issue`.

The *skip-once* boundary (Part 2) is enforced one level up, in the breaker loop,
**before** `escalate_with_tracking_issue` is called at all: a goal already
standing Blocked with the no-progress sentinel is `continue`d past, so an
already-escalated goal never re-enters the escalation path.

> **Migration note.** Tracking issues filed *before* this change do not carry the
> `ooda-signature:` marker, so they will not be matched by the remote search on
> the first cycle after upgrade. The result is at most **one** additional
> "file-new-once" per affected goal; from then on the new issue's marker makes the
> goal idempotent. This bounded one-time cost is acceptable and requires no
> back-fill.

## Part 2 — skip-once after guided retry

`NoProgressBreakerReport` gains an additive field recording which goals the
breaker escalated-and-skipped this tick (for observability and tests):

```rust
pub(crate) struct NoProgressBreakerReport {
    // … existing fields …
    /// Goal IDs the breaker escalated exactly once this tick and will now skip
    /// while they stand Blocked with the no-progress sentinel — populated at each
    /// escalation site so a still-blocked goal is not re-oriented next tick.
    pub halted: Vec<String>,
}
```

The mechanism is deliberately keyed on the goal's **live block status**, not a
persisted "halted forever" flag. Before it does any work for a goal, the breaker
loop calls `goal_is_sentinel_blocked(state, goal_id)` — true when the goal
currently stands `Blocked(reason)` with a reason that `is_no_progress_marker`
recognizes as the breaker's own sentinel — and if so `continue`s past the goal
entirely (no counter bump, no re-orient, no escalation):

```text
Blocked (no guided retry yet)
   │  breaker fires → re-orient once, set guided_retry_used
   ▼
Blocked (guided_retry_used = true)
   │  still Blocked next visit → escalate ONCE
   │  (file/reuse ooda-stuck issue), report.halted.push(goal)
   │  the goal is left Blocked with the no-progress sentinel
   ▼
Sentinel-Blocked
   │  subsequent ticks: goal_is_sentinel_blocked → continue (skip):
   │  NO re-orient, NO new issue
   ▼
(re-admitted the moment the goal leaves sentinel-Blocked — an operator
 unblock, or the agentic reasoner's roll_to_new_cycle lifting the block)
```

**Why status-based, not a persisted flag.** Keying on the live block status means
the skip lifts automatically the instant the goal is unblocked — whether an
operator re-scopes it or the agentic reasoner re-orients it — with no separate
"clear the halt" bookkeeping to get wrong. Dedup (Part 1) is what still prevents
a *re-stall* from filing a **duplicate** issue: the preserved in-memory tracking
ref covers an in-process re-orient IO-free, and the remote `ooda-signature:`
marker covers a restart — so an unblocked-then-re-stalled goal re-escalates
idempotently either way.

Key properties:

- **Escalate exactly once.** The transition files or reuses a single tracking
  issue, then the goal is skipped while sentinel-Blocked; it does not re-escalate
  on later ticks.
- **No re-orient thrash.** A sentinel-Blocked goal is skipped *before*
  `roll_to_new_cycle`, so the breaker stops wiping `wip_refs` and stops
  re-entering the escalation path — killing the livelock at its source.
- **Reuses existing accounting.** The skip reuses the existing `guided_retry_used`
  / `surfaced_failures` accounting rather than introducing a parallel timer, and
  composes with the per-signature
  [blocked-goal escalation backoff](./blocked-goal-escalation-backoff.md).
- **Not permanent silence.** The skip is gated on the *live* sentinel-Blocked
  status: the moment the goal leaves that status (operator unblock or agentic
  re-orient), it becomes actionable again.

## API summary

| Symbol | Location | Role |
| --- | --- | --- |
| `breaker_signature(goal_id)` | `src/ooda_loop/no_progress.rs` | Deterministic, **per-goal**, restart-durable dedup key over the **un-redacted** `goal_id` (reuses the SHA-256 format, not `failure_signature`'s redaction). |
| `ooda-signature:<sig>` | issue body marker | Remote dedup source of truth (single whitespace-delimited token). |
| `NoProgressIssueFiler::find_open_tracking_issue` | `src/ooda_loop/no_progress.rs` | Read-only, fail-closed remote list-and-scan before create. |
| `GhIssueFiler` | `src/ooda_loop/no_progress.rs` | argv-only `gh issue list … --json number,url,body`, marker matched locally; `tracing::error!` on failure, never aborts. |
| `escalate_with_tracking_issue` | `src/ooda_loop/no_progress.rs` | live-ref → remote-search → file. |
| `NoProgressBreakerReport.halted` | `src/ooda_loop/no_progress.rs` | Goal IDs escalated-once-and-skipped this tick. |
| `goal_is_sentinel_blocked` | `src/ooda_loop/no_progress.rs` | Skip guard: `continue`s past a goal still standing Blocked with the no-progress sentinel, before any escalation. |
| `is_no_progress_marker` | `src/goal_curation/no_progress_breaker.rs` | Recognizes the breaker's own Blocked-reason sentinel that the skip guard keys on. |
| `WipRef::is_no_progress_tracking` | `src/goal_curation/types.rs` | Marks the breaker's tracking-issue ref; `roll_to_new_cycle` **preserves** exactly this ref so in-memory dedup survives an in-process re-orient (IO-free, `gh`-independent). Prefix constant `NO_PROGRESS_TRACKING_LABEL_PREFIX` lives here (single home shared with `no_progress.rs`). |

## Configuration

This feature is **on by default** and has no new environment variables or config
keys. It reuses the existing `gh` authentication and the standard OODA daemon
setup (see [Keeping the OODA daemon steerable](./steerable-ooda-daemon.md) for
`SIMARD_HOME` and related settings). The `ooda-stuck` label must exist in the
target repository, exactly as before this change.

## Examples

### Healthy behavior — one issue, then quiet

A goal that becomes and stays blocked:

```text
tick N    : goal 4d27c91a Blocked, guided retry used → escalate
            no open ooda-signature:3f9a1c77b0e42d18 found → file #4600
tick N+1  : goal 4d27c91a still sentinel-Blocked → skip (no-op)
tick N+2  : goal 4d27c91a still sentinel-Blocked → skip (no-op)
…
operator closes #4600 and unblocks (re-scopes the goal)
tick N+k  : no longer sentinel-Blocked → goal re-orients normally
```

Expected log lines (structured tracing; no `print!`/`println!`):

```text
[simard::ooda] no-progress breaker: tracking issue filed for stuck goal issue=4600
[simard::ooda] no-progress breaker: stuck after guided retry — BLOCKED WITH why + issue filed and linked (halted, escalated once) goal=4d27c91a
[simard::ooda] no-progress breaker: goal already escalated (sentinel-blocked) — skipping (escalated once) goal=4d27c91a
```

### Dedup after a re-orient / restart

```text
tick N    : file #4600 (marker ooda-signature:3f9a1c77b0e42d18)
--- in-process re-orient: roll_to_new_cycle PRESERVES the tracking ref ---
tick N+1  : live wip_ref check HITS (ref survived the roll) → re-link
            in memory, NO remote call, NO new issue
--- daemon restart; wip_refs empty ---
tick N+2  : live wip_ref check MISSES (memory cleared by restart)
            remote search HITS #4600 → re-link, NO new issue
```

This is exactly the path that previously produced `#4499`/`#4504`/`#4508` and no
longer does. Note the two distinct dedup sources: an **in-process** re-orient is
handled IO-free by the preserved in-memory ref (issue #4509); a **restart** — the
only path that genuinely loses the in-memory ref — falls back to the remote
signature search.

## Verifying the behavior

Unit tests (inline `#[cfg(test)]` in `no_progress.rs` and `types.rs` plus the
`tests_no_progress*` / `tests_no_progress_breaker` suites) assert:

- **Dedup survives an in-process re-orient (in-memory)** — the real
  `roll_to_new_cycle` preserves the breaker tracking ref, so a re-stall dedups
  from memory **without** consulting the remote
  (`roll_to_new_cycle_preserves_in_memory_dedup_across_reorient`, plus
  `roll_to_new_cycle_preserves_breaker_tracking_ref_but_drops_live_refs` in
  `types.rs`).
- **Dedup survives loss of the in-memory link (remote)** — when `wip_refs` are
  gone (process restart), the remote signature search matches the existing issue,
  not a second file.
- **Dedup survives restart** — a fresh filer whose remote list already contains
  the marker re-links the existing issue instead of filing.
- **Distinct goals get distinct signatures** — two different `goal_id`s (e.g. the
  8-char-hex `4d27c91a` and `7f5afcca`) produce two *different* `ooda-signature`
  markers, so a second stuck goal is never deduped against the first goal's issue.
  (Regression guard for the `failure_signature`-redaction collision pitfall.)
- **Single escalation + skip** — a goal still Blocked after a guided retry
  escalates once (recorded in `report.halted`) and later ticks are skipped while
  it stands sentinel-Blocked.
- **Filer-failure path** — when `gh` fails, the filer returns `None`, logs at
  `error`, and the cycle continues (goal stays Blocked). No panic (repairs
  `#4472`/`#4474`).
- **Re-admit on unblock** — a still-blocked goal that an operator or the agentic
  reasoner unblocks becomes re-orientable again; a re-stall re-escalates
  idempotently against its existing `ooda-signature:` marker (no duplicate issue).

Quality gates that must stay green: `scan_no_stray_prints` (structured tracing +
OpenTelemetry only) and `scan_no_bridge_naming`.

## Why this is safe

- **Additive / non-breaking.** New trait method, new signature helper, new report
  field, and a status-based skip guard; existing filing behavior is preserved for
  the common case. The PRD is unchanged.
- **Fail-closed dedup.** On any `gh` list error the breaker does *not* file a
  duplicate; the goal simply stays Blocked, no worse than before the search
  existed.
- **Bounded one-time migration cost.** Pre-existing unmarked issues cost at most
  one extra "file-new-once" per goal, then are bounded by the marker.
- **No permanent silence.** The skip re-admits on a concrete state change (the
  goal leaves sentinel-Blocked via operator unblock or agentic re-orient), so a
  goal that becomes actionable is never stranded.
- **No command injection.** All `gh` invocations use `Command::new("gh").args([…])`
  argv vectors — never `sh -c` or string-built commands.

## See also

- [Blocked-goal escalation backoff](./blocked-goal-escalation-backoff.md) — the
  per-signature backoff this composes with.
- [No-progress breaker explains WHY and self-resolves](./no-progress-root-cause-resolution.md)
- [Terminal no-progress stall never parks empty evidence](./no-progress-terminal-investigation.md)
- [Keeping the OODA daemon steerable](./steerable-ooda-daemon.md)
