# Secondary Investigation — Common Root-Cause of the Blocked-Goal Cluster & the Meaning/Repetition of `workstream-gap`

**Role:** SECONDARY (pattern / common-root-cause focus)
**HEAD:** `85b9398a` (one commit past validated `dea65df8`; delta is **docs-only** —
`git diff --name-only dea65df8..HEAD` touches only `ai_working/`, no `src/`).
**Verdict:** All prior root-cause conclusions **hold unchanged**. Every source
citation below re-verified live at this HEAD.

---

## 1. The one shared mechanism behind the whole cluster

The composite signature is the overseer's own write-back key
(`observation_signature`, `mod.rs:1068-1073`, re-verified): `overseer-obs:` +
sorted/deduped problem `dedup_key`s joined by `|`. It is a **faithful fingerprint
of a static problem set**, not a dedup/storage/replay artifact. So the pattern
question is not "why does the fingerprint repeat" but **"why does the problem set
never change."**

**Answer — one lever, four surface symptoms.** Every clustered goal funnels
through a single upstream mechanism: the **bare no-progress park with no `WHY`
token**. The corrective vocabulary already exists (`NoProgressClass` +
`resolution_for_why`, `no_progress_breaker.rs:384-417`) but is **double-gated and
fails open to bare-park** (`ooda_loop/cycle.rs:582-702`):

- **Gate A** — `completion_evidence.is_some()`; if `None`, the entire breaker
  block collapses to `Vec::new()` (no classification, no ladder).
- **Gate B** — `no_progress_investigation_enabled()` (default `true`; env-off
  downgrades to the legacy bare park).
- **No invariant** ties a `Blocked` reason to a `NoProgressClass`.

When the WHY reasoner is unwired/misclassifies, all six stall classes collapse to
the same bare park → the goal re-parks every window → the recurring `goal:blocked`
population. **This is the common root cause.** Not five independent goal bugs —
one unwired classification rung.

### Per-goal cause map (verified, matches `CONSOLIDATED_FINDINGS.md §4/§5a)

| Cluster | Class | Why it stays blocked |
|---|---|---|
| kgpacks-rs parity + #12/#17/#18/#23/#25 | **(a) false-park** `AlreadyComplete`/`MissingPrecondition` | work already done (issues CLOSED / PRs MERGED) misread as "stuck" — the canonical incident (`no_progress_why.rs` header) |
| Audit Simard coverage → 70% | **(b) missing-perpetual-tag** via `UnclearCriteria` | uncheckable done-gate → idles → re-parks |
| simard-identity personas (atelier/bursar/cartographer/concierge/gastronome) | **(c) starvation** `GoalUncovered` | p1/p2 with no assignee/workstream — under-resourced |
| coin benchmark harness | **genuine dependency block** `MissingPrecondition`/`UpstreamDependency` | absent precondition / unlanded upstream |

Three of four are *not* genuine blocks — they are false-park / uncheckable-gate /
starvation, all remediable at the **one** WHY-reasoner-wiring lever.

---

## 2. What `workstream-gap` means and why it repeats

**Meaning (verified `sensor.rs:288-320`):** a **backlog-coverage gap**, NOT
zero-workstream decomposition. `detect_workstream_gaps` flags an active,
non-blocked p1/p2 goal with no assignee/PR/branch/session (`GoalUncovered`), a
high-signal open issue with no PR (`IssueUncovered`), or a live anomaly with no
fix in flight (`AnomalyUnaddressed`). **Blocked goals are explicitly skipped**
(`sensor.rs:300-302`) — routed via `goal_health` to avoid double-notify.
(Decomposition producing `<2` sub-goals is a *separate, loud* path in
`decompose.rs`, `MIN_SUBGOALS = 2`, and emits **no** `workstream-gap`.)

**Why it only flags, never converges (verified `mod.rs:884-948`):**
`act_flag_workstream_gaps` peeks the `gap_gate`, sends **one consolidated operator
notification**, and commits the gate. It **files no issue and launches no
workstream.** `WorkstreamCoverage` is the only High-priority Decide arm with **no
edge into `launch.rs`** — its siblings (`ProcessHealth`, `CrossCutting`,
`StepFailure`) all reach `LaunchRecipe`. This is **"observe-and-flag without a
closing action"**: the condition is never removed, so it is re-detected and
re-flagged every window.

**Why `workstream-gap|workstream-gap` repeats in the signature:** these are
**multiple distinct `WorkstreamCoverage` problems/episodes** each carrying the
**bare family `dedup_key` `"workstream-gap"`** (`mod.rs:1371`), concatenated in the
recall stream. `dedup()` in `observation_signature` only collapses **adjacent
equal keys within one signature** — cross-episode repeats survive. It is **not**
an adjacent-dedup bug and **not** the "×2" recurrence (the ×2 counts the whole
aggregate snapshot via `RECURRING_SIGNATURE_THRESHOLD = 2`, `signal.rs:362`). The
bare family key also **destroys per-gap identity** — every persona gap looks
identical at the write boundary, so their count cannot be attributed.

---

## 3. The unifying pattern — "Two signatures, one root problem"

The two families are **one problem in two views** (verified `sensor.rs:300` +
breaker park path). One under-resourced important goal **oscillates**:

```
  active, no workstream  ──breaker parks──▶  Blocked
        │  emits                                   │  emits
        ▼                                          ▼
   workstream-gap  ◀──unblocked/reactivated──   goal:blocked
   (GoalUncovered)                             (skipped by gap-scan)
```

This is exactly why personas, the coverage audit, the coin harness, and kgpacks
appear in **both** recurring families and co-occur inside the same
`overseer-obs:…` composite. Treat gap + blocked for the same entity as **one
resourcing/convergence problem**, not two bugs.

---

## 4. Two compounding (but independent) anti-patterns

Confirmed and re-grounded — these amplify the root cause but are not it:

1. **Recurrence dead zone.** A signal with recurrence in `[2,3)` gets neither
   auto-remediation nor escalation. Coverage gaps have **no** cross-window
   escalation at all (`gap_gate = WhisperGate::new(900, 200)`, `mod.rs:304`
   forgets across windows); blocked-goal root causes escalate only at
   `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`). The `×2` sits below
   both bars. **Design already exists**: recurrence-aware gap rung at
   gap-threshold=2 (`tertiary_gap_routing_and_remediation_rung.md §4`).

2. **Self-observation feedback + missing storage idempotency.** Recall-derived
   `RecurringSignature` (`signal.rs:455-469`) is admitted as a `ProcessHealth`
   problem (`sanitize_recalled`, `mod.rs:1353-1359`) and written back by
   `write_back_observation(&cycle.problems)`, nesting prior `overseer-obs:` tokens
   into the next signature. The write-back is gated only by an in-process
   `WhisperGate` with **no signature-keyed idempotent upsert**, so the count is a
   *faithful* append, not a broken dedup. The `×2` is **honest**.

---

## 5. Meta-conclusion (secondary verdict)

**The recurrence count is honest — audit the closing action, not the counter.**
The signature is a deterministic, provably within-window-deduped fingerprint
(test `write_back_is_deduplicated_within_window`, `tests_memory_recall.rs:797-817`)
of a problem set that never changes because **two observe-and-flag loops never
close**:

- **Loop A (blocked goals):** bare no-progress park with the WHY-ladder gated off.
- **Loop B (workstream-gaps):** flag-only Act with no `launch.rs`/issue edge.

Both are the **same shape** — a persistent-signal loop with **no convergence
rung** — sitting in a **recurrence dead zone**. Fix the missing closing actions
(WHY-reasoner wiring as P0; a 2×-recurrence gap-remediation rung into the existing
`launch.rs`/`RecipeBrief` seam), **not** the counter.

---

## 6. Questions for the verification phase

1. **Do NOT** blindly apply the one-line `record_occurrence →
   store_fact_with_caller_key(root_cause_signature(...))` fix — per
   `secondary_dedup_recurrence_VALIDATION_HEAD.md §4` it collapses Lane B to
   idempotent and changes escalation-counting semantics. Verify in design first.
2. Confirm whether `completion_evidence` (Gate A) is actually `None` in the live
   daemon that produced these signatures — that determines whether the WHY ladder
   ever ran for the kgpacks cluster.
3. Confirm the escalation DECISION latches at recurrence≥3 via `blocked_goal_gate`
   and **never un-latches** — relevant to whether convergence is even reachable
   once a goal crosses the bar.
4. The bare `"workstream-gap"` family `dedup_key` (`mod.rs:1371`) erases per-gap
   identity — verify whether a per-signature key (`workstream-gap:<sig>`, already
   used at the gate `mod.rs:901`) should also key the *problem* for attributable
   counting.
