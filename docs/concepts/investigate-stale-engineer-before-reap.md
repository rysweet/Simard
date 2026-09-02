---
title: "Investigate a Quiet/Idle Engineer Before Reaping It"
description: >
  How the stale-engineer-claim reaper investigates a HeartbeatStale engineer
  before reclaiming its claim, instead of reaping on idle-mtime alone. A
  heartbeat-stale claim is never reaped until a completed investigation concludes
  the engineer is Dead; the reaper fails closed on every other verdict. The
  investigation is de-duplicated across Overseer ticks by a stable per-claim
  signature so a still-stale claim maps to exactly one evidence dir and exactly
  one investigation recipe until it concludes or its freshness window lapses, and
  the journal capture it performs is bounded by a subprocess deadline so a slow
  journalctl can never stall the meta-OODA tick.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ./stale-engineer-claim-reaper.md
  - ./engineer-claim-liveness-lease.md
  - ../reference/investigate-stale-engineer-api.md
  - ../reference/claim-reaper-api.md
  - ../reference/overseer-recipe-launch-idempotency.md
  - ../reference/engineer-worktree-sweep-safety.md
  - ../operations/claim-reaper-kill-switch.md
---

# Investigate a Quiet/Idle Engineer Before Reaping It

> **Status: implemented.** This page describes shipped behaviour in present
> tense. The investigate-before-reap path lives in
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs)
> and runs synchronously on the Overseer tick beside
> `reconcile_inflight_investigations`
> ([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)).
> The dedup/idempotency and journal-timeout contracts described here are the
> current, enforced behaviour. This is the concept companion of the
> [Investigate-Before-Reap API](../reference/investigate-stale-engineer-api.md).

## What this adds on top of the reaper

The [stale-engineer-claim reaper](./stale-engineer-claim-reaper.md) reclaims a
claim when its engineer is **provably dead**. That base sweep treats a worktree
whose newest-file mtime is older than the stale threshold as
`Dead { HeartbeatStale }` and reclaims it directly.

A quiet-but-alive engineer — one in a long compile, a slow test run, or simply
thinking between file writes — has the *same* idle-mtime signature as a genuinely
hung one. Reaping on idle-mtime **alone** risks killing live work. The
investigate-before-reap path closes that gap: a `HeartbeatStale` verdict is **not
sufficient** to reclaim. It is only a *trigger to investigate*. The claim is
reclaimed only after a **completed investigation concludes the engineer is
`Dead`**.

| Verdict from the base probe | Investigate-before-reap behaviour |
|---|---|
| `Dead { NoWorktree }` | **Reap immediately** — a concluded investigation already removed the worktree; there is nothing to protect (pre-existing [#4099](https://github.com/rysweet/Simard/issues/4099), unchanged). |
| `Dead { HeartbeatStale }` | **Do not reap yet.** Run an investigation. Reap **iff** it concludes `Dead`. Otherwise keep the claim. |
| `Live` | Keep the claim (unchanged). |

The reaper **fails closed** on every non-`Dead` investigation verdict:
`StillAlive`, `Blocked`, `Recoverable`, and `Pending` all keep the claim. Only an
investigation that positively concludes `Dead` reaps a heartbeat-stale claim.
Evidence is always preserved before any cleanup.

## The investigation, one sweep at a time

When the reaper sees a `HeartbeatStale` claim it hands the claim to an
`investigate()` seam that, on the **first** sweep for that stale claim:

1. **Archives evidence** — copies the engineer's worktree artifacts and a bounded
   slice of the daemon journal into an owner-restricted evidence directory under
   `reaped-engineers/` (mode `0700` dirs, `0600` files).
2. **Dispatches one investigation recipe** — emits a single `LaunchRecipe`
   intervention whose brief points an agentic reasoner at the preserved evidence,
   and returns the verdict `Pending`.

`Pending.should_reap() == false`, so the claim is **kept** this sweep. A later
sweep observes the concluded investigation's outcome (the investigation removes
the worktree when it concludes the engineer is dead, which the base probe then
reports as `NoWorktree` → immediate reap).

## Why de-duplication matters

The Overseer tick runs continuously. Without de-duplication, **every** tick that
still sees the same heartbeat-stale claim would archive evidence *again* and
launch *another* investigation. That is exactly the failure this feature is
built to avoid:

- **Unbounded disk growth** — a new timestamped `reaped-engineers/<key>-<ts>/`
  directory minted on every tick.
- **Unbounded / duplicate agentic spawn** — a fresh investigation recipe admitted
  every cycle for a claim already under investigation.
- **A TOCTOU race** — N concurrent duplicate investigations all racing to release
  the same claim and remove the same worktree.

De-duplication is therefore a **correctness** property, not an optimisation.

### The dedup contract

> **Dedup contract.** Two Overseer ticks for the **same still-stale claim** map
> to the **same dedup key** and together produce **exactly one** evidence
> directory and **exactly one** admitted investigation recipe, until that
> investigation completes or the evidence directory ages out of the freshness
> window. Later ticks return `Pending` with **no** new archive and **no** new
> launch, and the claim is **not** reaped.

Two independent guards, either of which alone suppresses a duplicate, together
make this hold under concurrency:

1. **Disk-derived idempotency (primary).** `archive_stale_engineer_evidence()`
   is idempotent *per claim*. Instead of always minting a fresh
   `<sanitized_claim_key>-<ts>/` directory, it **reuses the most-recent existing
   directory for that claim** when one exists within the
   `ARCHIVE_FRESHNESS_WINDOW` (1 hour), returning it **as-is without re-writing**.
   Only when no within-window directory exists is a new one minted (and its
   evidence written). `investigate()` short-circuits to `Pending { interventions:
   [] }` — no re-archive, no re-dispatch — whenever the archive step **reused**
   (rather than minted) a directory. This makes the evidence path **stable per
   claim** while an investigation is outstanding, so no pile of `<key>-<ts>/`
   siblings accrues and the bounded journal capture runs at most once per epoch.

2. **Stable dedup signature (belt-and-suspenders).** The investigation brief
   carries a single-line, reserved dedup token
   `stale-investigation:<sanitized_claim_key>`. `recipe_dedup_key()`
   ([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs))
   keys on this token when present, so two ticks for the same claim produce the
   **same** key even though the human-readable prose (evidence dir path, idle age)
   differs. The Overseer's in-flight-investigation guard then suppresses a second
   `LaunchRecipe` while one is outstanding. This is the guarantee that makes the
   code comment "deduped by the in-flight-investigation guard" **true**.

The stable token is a **distinct** reserved prefix. It deliberately does **not**
reuse `OVERSEER_OBS_PREFIX` (`overseer-obs:`), which carries separate write-back /
self-referential-loop semantics ([#4128](https://github.com/rysweet/Simard/issues/4128)).
Volatile prose — the timestamped `evidence_dir` and the monotonically-growing
`idle_age_secs` — is kept **out** of the portion `recipe_dedup_key` keys on, so it
can never destabilise the signature.

### Evidence-directory naming and the freshness window

Evidence directories are named `<sanitized_claim_key>-<unix_ts>`:

- `sanitized_claim_key` is the claim key passed through a strict allowlist
  sanitizer (`[A-Za-z0-9_-]`, single line, no `..`, no `/`, `\`, NUL, CR/LF, and
  no `:` collision with the dedup token — every other byte, including `.`, folds
  to `_`). The **same** sanitizer builds both the directory name and the dedup
  token, so the two can never diverge.
- `<unix_ts>` is the mint time of the directory.

On each investigation sweep, the reaper scans the direct children of
`reaped-engineers/` for the most-recent `<sanitized_claim_key>-<ts>` directory
(the `<ts>` is parsed from the **name**, so the freshness window is derived from
the archive epoch itself):

| Directory state | Action |
|---|---|
| A matching dir exists **within** `ARCHIVE_FRESHNESS_WINDOW` (1 h) | **Reuse it as-is** — no re-write, no re-archive; the dir keeps its `0700`/`0600` perms. `investigate()` short-circuits to `Pending` — no re-dispatch. |
| No matching dir, or the newest is **older** than the window | **Mint** a new `<key>-<now_ts>/`, archive into it (`0700`/`0600`), dispatch one investigation. |

The 1-hour window is a documented heuristic: it comfortably spans a single
outstanding investigation across many meta-OODA ticks, while a genuinely-new
recurrence hours later is a new claim epoch that legitimately earns fresh
evidence. The window is the **disk-side backstop**; the stable dedup token is the
**primary** correctness guarantee.

## No permanent wedge

De-duplication suppresses duplicates while an investigation is *outstanding* — it
never permanently prevents future investigation of a recurring problem:

- The in-flight token guard is **transient**: `reconcile_inflight_investigations`
  clears it when the investigation handle finishes.
- The evidence directory **ages out** of the freshness window.

So a completed-then-recurring stale event for the **same** claim can be
investigated again later (fresh dir + new launch). And a genuinely-dead engineer
whose worktree the concluded investigation removed is reported as `NoWorktree` and
reaped immediately. Nothing is permanently suppressed.

## Bounded journal capture

The investigation archives a slice of the daemon journal so the reasoner can see
what the engineer was doing before it went quiet. That capture shells out to
`journalctl`. Because the reaper runs **synchronously on the meta-OODA tick**
(the module contract promises the sweep "adds no new thread"), an unbounded
`journalctl` call would block **all** Overseer supervision if journalctl is slow
or hung.

`capture_journal_slice()` therefore runs `journalctl` through a bounded
subprocess helper, `run_with_deadline`, with a `JOURNAL_CAPTURE_TIMEOUT` of **5
seconds**:

- The child is spawned, polled to a deadline, and **killed and reaped** on expiry
  (no zombie, no leaked reader thread).
- On timeout the helper emits a `tracing::warn!` and returns a best-effort
  empty/partial slice — it **never panics and never blocks past the deadline**.
- Output is capped to avoid unbounded memory growth.

This bound is scoped to the **journalctl subprocess only**. It is **not** a
wall-clock timeout on any agentic/recipe step — a healthy long-running
investigation recipe is never killed by elapsed time, matching the operator rule
that agentic steps must never be time-capped. The idempotent archive already
ensures this capture runs at most once per claim rather than every tick; the
deadline bounds the per-call block that remains.

## Invariants at a glance

| Situation | Behaviour |
|---|---|
| `HeartbeatStale` claim, first sweep | Archive once + dispatch one investigation; verdict `Pending`; claim **kept** |
| Same still-stale claim, later sweeps within the window | **Reuse** dir; **no** re-archive/re-launch; verdict `Pending`; claim **kept** |
| Investigation concludes `Dead` (worktree removed) | Next sweep sees `NoWorktree` → **reap immediately** |
| Investigation concludes `StillAlive`/`Blocked`/`Recoverable`/`Pending` | Claim **kept** (fail-closed) |
| Genuinely-new recurrence after window lapses / prior investigation completes | Fresh dir + new investigation (**no permanent wedge**) |
| `journalctl` slow or hung | Bounded at 5 s: `warn` + partial/empty slice; tick **never stalls** |
| Evidence directories | `0700` dirs / `0600` files, set on **mint**; a reused dir retains them (reuse re-writes nothing) |

## Related

- [Stale-Engineer-Claim Reaper](./stale-engineer-claim-reaper.md) — the base
  independent claim-reclaim sweep this path extends.
- [Investigate-Before-Reap API](../reference/investigate-stale-engineer-api.md) —
  the `investigate()` seam, `ArchiveOutcome`, the dedup token contract, the
  freshness window, and the `run_with_deadline` bound.
- [Claim-Reaper API](../reference/claim-reaper-api.md) — the base sweep,
  liveness probe seam, cleanup seam, and config resolvers.
- [Overseer Recipe-Launch Idempotency](../reference/overseer-recipe-launch-idempotency.md)
  — how `recipe_dedup_key` and the in-flight guard suppress duplicate launches.
- [Worktree Reaping Safety Guards](../reference/engineer-worktree-sweep-safety.md)
- [Engineer-Claim Liveness Lease](./engineer-claim-liveness-lease.md)
- [Claim-Reaper Kill Switch & Tuning](../operations/claim-reaper-kill-switch.md)
