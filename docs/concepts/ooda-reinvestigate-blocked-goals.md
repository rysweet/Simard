---
title: Re-investigating already-blocked OODA goals
description: Why the OODA no-progress safeguard re-investigates goals that are ALREADY sitting in a bare "[OODA-SAFEGUARD] … needs human review" state — not just goals crossing the block threshold — so no goal is ever stranded with an unexplained block, and how the population-driven re-investigation pass attaches a concrete WHY (already-complete / obsolete / missing-precondition / upstream-dependency / unclear-criteria / genuinely-stuck) and, when actionable, completes, drops, heals, defers, or spawns a fixer (#17).
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./perpetual-goal-no-progress-exemption.md
  - ./ooda-loop-self-detection.md
  - ../reference/no-progress-breaker-api.md
  - ../reference/no-progress-reinvestigation-api.md
  - ../howto/reinvestigate-bare-blocked-goals.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
---

# Re-investigating already-blocked OODA goals

> **Status: implemented.** The OODA no-progress safeguard investigates the WHY of
> a block for **two** populations: goals crossing the no-progress threshold on this
> cycle (the on-transition path, PR #2960) **and** goals that are *already* parked
> in a bare `[OODA-SAFEGUARD] … needs human review` state (the re-investigation
> pass, issue #17). The deterministic rail
> [`is_bare_no_progress_block`](../reference/no-progress-reinvestigation-api.md#the-thin-deterministic-rail)
> and the re-investigation pass `reinvestigate_bare_blocked_goals` live in
> [`src/goal_curation/no_progress_breaker.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/no_progress_breaker.rs)
> and
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs);
> the WHY vocabulary (`NoProgressClass`) is reused unchanged from
> [`src/goal_curation/no_progress_why.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/no_progress_why.rs).
> The exact types and functions are in the
> [no-progress re-investigation API reference](../reference/no-progress-reinvestigation-api.md).

## The gap this closes (#17)

The OODA daemon carries a deterministic **no-progress safeguard**: when a single
goal produces `NO_PROGRESS_BREAKER_THRESHOLD` (3) consecutive *no-action* cycles
and the done-gate cannot certify it complete or obsolete, the breaker hard-blocks
it — setting `GoalProgress::Blocked` with a sentinel reason.

PR #2960 improved that block from a bare "needs human review" note into a
**WHY-investigated** block: at the moment a goal *crosses* the threshold, an
agentic reasoner classifies *why* it is stuck and the safeguard resolves it along
a ladder (complete it, drop it, heal a precondition, defer behind an upstream
dependency, or spawn a guided fixer). That is the **on-transition** path.

But the on-transition path only fires **on the transition**. It is driven by
*this cycle's* action outcomes: a goal is investigated exactly once, at the cycle
where its consecutive-no-action counter reaches the threshold. That leaves a real
gap:

- A goal that was blocked by an **older** daemon build — before the
  WHY-investigation reasoner shipped — still carries the *bare* marker. It never
  re-crosses the threshold, so it is never investigated.
- A goal blocked on a cycle where the reasoner **erred** (fail-closed keeps the
  bare marker) is never re-examined.
- More fundamentally, once a goal is `Blocked`, the brain stops selecting it →
  it emits no fresh no-action outcome → its counter is already reset → it can
  **never re-cross the threshold**. Nothing scans the already-blocked population.

The symptom, observed live (deploy #41): several goals stuck at the *old* bare
message

```
🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 3 consecutive no-action cycles; needs human review
```

with no explanation and no path forward —
`advance-rysweet-agent-kgpacks-rs-to-full-parity`, `fix-agent-kgpacks-rs-issue-18`
(WS3), `-issue-21` (WS6), `-issue-22` (WS7), and `-issue-23` (WS8) — while a peer,
`fix-agent-kgpacks-rs-issue-17`, *did* get a proper WHY (an
`UPSTREAM-DEPENDENCY` block naming the open eval-baseline issue `#16`). That rich,
actionable format is what **every** blocked goal must have.

The operator directive is binding:

> whatever recipe is marking things as blocked needs to be updated to always
> investigate and clearly state the *why* and if possible spawn an engineer to go
> fix the why.

**No goal should ever sit with a bare "needs human review."**

## The fix: a population-driven re-investigation pass

The on-transition path is *outcome-driven* — it looks at what happened this cycle.
Issue #17 adds a complementary *population-driven* pass that looks at the **board
state** itself, exactly like the sibling auto-clear scan:

1. Each cycle, after the on-transition breaker runs, scan the active board for
   every goal whose status is a **bare** no-progress block — carrying the
   `[OODA-SAFEGUARD]` sentinel marker but *no* WHY classification — that is not a
   [standing/perpetual goal](./perpetual-goal-no-progress-exemption.md).
2. Run each such goal through the **same** injected reasoner the on-transition
   path uses. There is no second reasoner and no new prompt.
3. **Rewrite** the block reason to embed the concrete WHY (class token + cited
   evidence), so the goal is no longer bare.
4. **Resolve** it along the same ladder: complete it, drop it, heal a precondition,
   defer it behind the named upstream dependency, or spawn exactly one guided
   fixer — using the same shared resolution logic (`apply_resolution`) as the
   on-transition path, so the two can never diverge.

The result: a goal that was stranded with a bare marker is, on the next cycle,
either re-blocked **with a concrete WHY** (upstream dependency named), **completed**
(if the work was actually already done), **dropped** (if obsolete), **un-blocked
and healed/retried** (if a precondition was missing), or handed to **exactly one
fixer engineer**. None are left as a bare "needs human review."

### Agentic classification behind a thin deterministic rail

The classification is **agentic** — the reasoner decides *why* a goal is stuck by
looking at evidence (open/closed issues, merged/absent PRs, named dependencies).
The safeguard itself does **no** brittle parsing of that narrative. The only
deterministic string check in the whole feature is the rail that decides whether a
block is *bare*:

```
bare(reason)  ≡  is_no_progress_marker(reason)
              ∧  reason contains NO CLASS_* token
```

Everything downstream consumes the reasoner's **structured** result
(`NoProgressClass` + narrative), never a re-parse of the rendered text. This keeps
the intelligence in the reasoner and the safeguard as a thin, testable rail — the
[agentic-behind-a-thin-rail](./ooda-loop-self-detection.md) posture the daemon uses
everywhere.

## Why the two paths share one resolution ladder

Both the on-transition path and the re-investigation pass funnel every
classification through a single `apply_resolution` helper and a single
`resolution_for_why` mapping. This is deliberate: the ladder (which WHY maps to
complete / drop / heal / defer / spawn / escalate) is defined **once**, so the two
populations cannot drift apart. The only per-site difference is whether a healed or
fixer-assigned goal is left `Blocked` or un-blocked to `NotStarted` — see the
[resolution ladder](../reference/no-progress-reinvestigation-api.md#resolution-ladder)
in the API reference.

| WHY classification | Resolution | Post-state |
| --- | --- | --- |
| `ALREADY-COMPLETE` | mark done | ✅ Completed |
| `OBSOLETE` | drop from board | ✅ removed |
| `MISSING-PRECONDITION` | heal precondition, retry | ✅ NotStarted (un-blocked) |
| `UPSTREAM-DEPENDENCY` | defer behind the named upstream | ✅ Paused (auto-clears) |
| `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` (guided retry unused) | spawn **one** guided fixer | ✅ NotStarted (un-blocked) |
| `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` (guided retry spent) | block **with WHY** + file issue | ✅ Blocked-with-why |

Every arm produces a **non-bare** post-state. That is the safeguard's core
invariant: after a cycle in which the reasoner did not error for a goal, that goal
is never bare again.

## Idempotency: exactly one fixer, ever

Re-investigation runs every cycle, so it must be **idempotent** — running it twice
with no intervening change must not spawn a second fixer or take a second terminal
action. Two guards enforce this:

1. **Primary — the rewrite removes the goal from the population.** The moment a
   goal's reason is rewritten to embed a `CLASS_*` token, `is_bare_no_progress_block`
   returns `false` for it, so the *next* cycle's scan skips it. The bare predicate
   is self-excluding.
2. **Belt-and-suspenders — a persisted dedupe set.** `NoProgressTracker` carries a
   persisted `reinvestigated: HashSet<(goal_id, class_token)>`. A terminal action
   (spawn / mark-done / drop / defer) is skipped if `(goal, class)` is already in
   the set; the pair is inserted **only on success**. This survives a daemon
   restart between the board rewrite and the tracker persist, and it is the
   security rate-limit that prevents runaway N-spawns per cycle.

The dedupe key is a **string token**, not the `NoProgressClass` enum — a deliberate
availability choice. The goal board read is *fail-to-empty*: a single unparseable
value discards the whole board. A string token can never fail to deserialize on an
older or rolled-back binary; an on-disk enum could. See the
[persisted-data contract](../reference/no-progress-reinvestigation-api.md#persisted-data-contract).

## Fail-closed, loudly

If the reasoner errors for a goal, the pass **keeps the bare marker**, takes **no**
terminal action, records an investigation error, logs at `error`, and retries the
goal next cycle. It never guesses a classification, never silently completes or
drops a goal, and never inserts into the dedupe set on an error. There are **no
wall-clock timeouts** on the agentic step — liveness is governed by idle detection,
never a stopwatch. A transient reasoner failure simply means the goal is
re-examined next cycle, still visible as a bare block until a real classification
lands.

## What this is *not*

- It does **not** touch [standing/perpetual goals](./perpetual-goal-no-progress-exemption.md).
  A perpetual goal is exempt from the safeguard entirely and self-heals its stale
  markers; the re-investigation scan skips it, mirroring the on-transition exemption.
- It does **not** change the threshold, the sentinel marker contract, or the
  on-transition behavior. Every rewritten reason keeps the
  `NO_PROGRESS_BLOCKED_PREFIX` and a parseable leading count, so the overseer
  root-cause sensor and the load-time self-heal keep recognizing the marker. The
  existing on-transition tests pass byte-for-byte.
- It adds **no** new capability, no new subprocess, and no memory-engine change. It
  re-triggers existing trusted seams over a wider population behind the same
  dispatcher.

## See also

- [No-progress re-investigation API reference](../reference/no-progress-reinvestigation-api.md) — the exact types, functions, resolution ladder, and serde contract.
- [No-progress breaker API reference](../reference/no-progress-breaker-api.md) — the base breaker, sentinel marker, and load-time self-heal.
- [Re-investigate bare-blocked OODA goals](../howto/reinvestigate-bare-blocked-goals.md) — operator runbook to observe and verify re-investigation.
- [Standing/perpetual goals are exempt from the no-progress hard-block](./perpetual-goal-no-progress-exemption.md).
- [OODA loop self-detection, reflectiveness, and proactivity](./ooda-loop-self-detection.md).
- [Spawn engineers from the OODA daemon](../howto/spawn-engineers-from-ooda-daemon.md).
