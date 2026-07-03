---
title: "Concept: keeping the OODA daemon steerable — read-your-writes, the done-gate, the no-progress breaker, and distillation banner-stripping"
description: The retcon narrative for the systemic goal-board + OODA-livelock + distillation incident — why the daemon became un-steerable (operator goal edits appeared not to stick), why it livelocked re-selecting already-done supply-chain goals, and why distillation extracted zero facts — and how four coordinated fixes across src/goal_curation, src/goal_curation/completion_gate, the OODA advance-goal path, and src/recipe_output restore read-your-writes, evidence-gated completion, forward progress, and fact extraction.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ./goal-board-persistence.md
  - ./goal-board-corruption-guards.md
  - ./deploy-aware-done-gate.md
  - ./ooda-loop-self-detection.md
  - ./copilot-launcher-preamble-stripping.md
  - ../reference/goal-board-api.md
  - ../reference/completion-evidence-gate-api.md
  - ../reference/distill-recipe-output-capture.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../howto/recover-goal-board.md
  - ../howto/diagnose-a-rejected-goal-completion.md
  - ../howto/diagnose-decide-orient-parse-failures.md
---

# Concept: keeping the OODA daemon steerable

This document is the **single coherent narrative** for a systemic incident in
which the autonomous OODA daemon became *un-steerable and stuck*. It ties
together four fixes that each already have their own focused reference and
concept pages, and it reconciles the **operator-observed symptoms** with the
**implementation that actually shipped** — the "retcon" that explains why the
shipped design resolves what was seen in production.

If you are here to change one subsystem, jump straight to its authoritative
page:

| Symptom | Root cause | Authoritative doc |
|---|---|---|
| Operator `goal add` / `goal remove` "didn't stick"; the next `goal list` and the next curation cycle read an old board | Stale, unordered snapshot read + a load/save asymmetry + un-serialized cross-process writes | [Goal board persistence](./goal-board-persistence.md) · [Goal board API](../reference/goal-board-api.md) |
| Goals with objectively-complete evidence (merged PR, filed/closed issue) stayed active at 0% and were re-litigated every cycle | No evidence-driven auto-completion; completion required a subjective LLM claim | [Deploy-aware done-gate](./deploy-aware-done-gate.md) · [Completion-evidence gate API](../reference/completion-evidence-gate-api.md) |
| Every cycle re-selected the same done goals and the brain emitted "I'll break the loop by verifying concretely…" no-action prose forever | No bounded, per-goal escalation from repeated no-action to a definitive resolution | [No-progress breaker](#the-no-progress-breaker-fix-3) (below) · [OODA loop self-detection](./ooda-loop-self-detection.md) |
| `distill: 50 episodes -> 0 facts, 0 procedures` | The Copilot CLI launch-log banner polluted the transcript the distill parser reads | [Copilot launch-log preamble stripping](./copilot-launcher-preamble-stripping.md) · [Distill recipe-output capture](../reference/distill-recipe-output-capture.md) |

---

## The incident

A production OODA daemon exhibited three failure modes at once, and they
compounded each other:

1. **Un-steerable.** An operator ran `simard goal remove <ids>` and
   `simard goal add …`. Both reported success, yet the next
   `simard goal list` still showed the *old* goals. Operator intent — the
   whole point of steering the daemon — was silently lost.

2. **Livelocked.** Every OODA cycle re-selected the **same four** supply-chain
   goals — all objectively **done** (two merged hardening PRs, one
   out-of-scope issue filed) — and the brain, cycle after cycle, emitted
   variations of *"I'll break the loop by verifying concretely…"* while taking
   **no shippable action**. The goals stayed `not-started` at 0% forever.

3. **Blind.** Memory distillation reported
   `distill: 50 episodes -> 0 facts, 0 procedures, 50 marked (100% reduction)`.
   Fifty episodes went in; nothing came out. The daemon could not learn from
   its own history, so it kept repeating it.

The three fed one loop: the daemon could not be redirected (1), so it kept
grinding on stale work (2), and it could not distill the experience into
memory that would let it notice (3).

---

## Fix 1 — Goal-board read-your-writes

**Symptom:** operator edits didn't stick; `goal list` and the next curation
cycle read an old board.

**Root cause.** The board was persisted as a cognitive-memory
`goal-board:snapshot` fact, but the *read* did not select the **latest**
snapshot. A newly-written snapshot became just one more fact node among many;
an unordered, limited search could return an older revision. A separate
load/save asymmetry (the loader ignoring what the saver had just written) and
un-serialized cross-process writes (daemon vs. dashboard vs. `goal`-CLI IPC)
turned "I added a goal" into "my goal vanished on the next cycle." A tight
multi-client race could even delete production goals between cycles (issue
[#1915](https://github.com/rysweet/Simard/issues/1915)).

**What shipped.** The board has a **single source of truth** — the
`goal-board:snapshot` fact in cognitive memory — and three properties make
writes observable by the very next read:

- **Newest-snapshot selection.** `load_goal_board` reads a *window* of
  candidate snapshots and picks the newest deterministically
  (`search_facts("goal-board:snapshot", 64, 0.0)` → filter →
  `max_by(node_id)`), instead of trusting an unordered limit-1 result. The
  loader and saver now share this one `read_latest_snapshot` helper, closing
  the load/save asymmetry.
- **Merge-on-write.** `save_goal_board` re-reads the latest persisted snapshot
  and unions it by goal `id` with the in-flight board (in-flight wins on
  collision) before storing. An operator- or meeting-added goal is therefore
  **merged**, never clobbered, by a concurrent curation write — curation can
  regenerate its own set without deleting operator intent.
- **Serialized cross-process writes.** When the daemon is running, all writes
  flow through its IPC socket; when it is not, the writer takes an advisory
  **`flock`** over the board lock file
  ([#2514](https://github.com/rysweet/Simard/pull/2514)) so the daemon, the
  dashboard, and the `goal` CLI never interleave a read-modify-write.

A one-shot bootstrap migration imports any legacy
`$SIMARD_STATE_ROOT/goal_records.json` on first startup and then removes it, so
there is exactly one authoritative store afterward.

> **On the operator's "make disk authoritative" diagnosis.** The observed
> root cause was correct — the board read was returning a stale snapshot and
> writes were racing. The shipped resolution keeps **cognitive memory** as the
> authoritative store (issue
> [#1590](https://github.com/rysweet/Simard/issues/1590)) rather than promoting
> the disk file, but it delivers the property the operator actually needed:
> **read-your-writes**. `add` → reload shows it; `remove` → reload omits it;
> and the next curation cycle observes the same latest board. See
> [Goal board persistence](./goal-board-persistence.md) for the full consumer
> matrix and the [`save_goal_board` merge rule](../reference/goal-board-api.md#save_goal_board),
> and [Goal board corruption guards](./goal-board-corruption-guards.md) for the
> pre-write validity gate that keeps a hallucinated Decide-phase board from
> overwriting the snapshot.

**Tests.** `tests_operations.rs`, `tests_snapshot_dedup.rs`, and
`tests_save_with_removals.rs` cover write→load round-trips, newest-snapshot
selection over multiple snapshots, add/remove reload behavior, and
merge-on-write under a concurrent writer/reader.

---

## Fix 2 — The evidence-gated done-gate

**Symptom:** goals whose work was objectively finished (a merged PR, a filed
or closed issue) were never marked done — they stayed active at 0% and were
re-selected every cycle.

**Root cause.** Completion was a *claim*, not a *verification*. Nothing turned
"the referenced PR is merged" or "the referenced issue is filed/closed" into an
automatic `done` transition, so objectively-complete goals lingered on the
active board and kept feeding the livelock.

**What shipped.** The **deploy-aware done-gate**
(`src/goal_curation/completion_gate.rs`) makes completion a function of **hard
evidence** gathered through an injected `EvidenceSource` (so the logic is pure
and hermetic in tests):

- `CompletionEvidence { pr_merged, issue_closed, self_affecting, deployed }`.
- A goal is complete only with a **merged PR**, a **closed linked issue**, and
  — for changes to Simard's own running code — a **verified deploy**
  (`!DeployDrift::needs_deploy`).
- Anything short records a specific `MissingEvidence` blocker
  (`PrNotMerged` / `IssueOpen` / `NotDeployed`) instead of silently archiving.
- `has_derivable_signal` — true when the goal references a PR, an issue, or is
  self-affecting — is the trigger that lets the gate auto-resolve a goal from
  external state rather than re-litigating it at 0%.

Applied to the incident, the four ladybug supply-chain goals each carried a
derivable signal (a merged hardening PR, or a filed out-of-scope issue), so the
gate would **auto-complete or auto-drop** them instead of returning them to the
active set. See [Deploy-aware done-gate](./deploy-aware-done-gate.md) and the
[Completion-evidence gate API](../reference/completion-evidence-gate-api.md);
operators diagnosing a *rejected* completion use the
[rejected-completion runbook](../howto/diagnose-a-rejected-goal-completion.md).

**Tests.** The completion-gate suite exercises the merged-PR / closed-issue /
self-affecting-not-deployed matrix, including a merged-PR-plus-filed-issue
fixture that maps directly to the four stuck supply-chain goals.

---

## The no-progress breaker (Fix 3)

**Symptom:** the brain kept emitting *"I'll break the loop by verifying
concretely…"* indefinitely — a healthy brain confidently producing **no
action** on the same goal, forever.

This is the fix with the least prior documentation, so it is documented in full
here. It is deliberately layered so that **no single layer has to be perfect**,
and — per the incident's coordination constraint — it lives in the
goal-selection / progress path and the OODA advance-goal dispatch, **not** deep
inside the brain reasoners.

### Why livelock is distinct from a brain failure

A brain *failure* is a transport error, a JSON parse error, or an empty
response — the brain did not produce a usable decision. A *livelock* is the
opposite: the brain **succeeds** and emits a well-formed decision whose
*content* is "take no action, I'll verify later." The daemon looks busy and
makes zero shippable progress. The two need different breakers.

### The three layers, and the definitive-resolution ladder

1. **Prompt-level self-detection.** The Observe/Orient/Decide prompts carry an
   explicit *"am I looping?"* judgment: a cycle that only re-triages the same
   PRs, re-reads the same issue, or re-records the same percentage is **not
   progress**. This is the first line of defense and requires no rebuild — see
   [OODA loop self-detection](./ooda-loop-self-detection.md).

2. **No-action classification.** When a decision resolves to a `NO ACTION` /
   `NO_ACTION` marker on its own line, the goal-session parser
   (`parse_orchestrator_response` → `has_no_action_marker` in
   `src/ooda_actions/goal_session/mod.rs`, reached through the advance-goal
   dispatch) routes it to `GoalAction::NoAction { reason }` and records a no-op
   cycle via `assess_only_outcome` (rather than spawning an engineer). This
   gives the progress path a **countable, structured** no-progress signal
   instead of having to pattern-match free prose.

3. **Bounded escalation to a definitive resolution.** Repeated no-progress on
   the *same* goal must terminate in a single decisive outcome — never another
   "I'll verify" cycle. The daemon already ships the analogous safeguard for
   brain *failures*: after **3 consecutive** failing cycles,
   `dispatch_spawn_engineer` writes a sentinel-tagged
   `GoalProgress::Blocked` reason
   (`BRAIN_FAILURE_BLOCKED_PREFIX` … `BRAIN_FAILURE_BLOCKED_SUFFIX`), files a
   tracking issue for human review, and heals automatically after one healthy
   cycle. The **no-progress breaker** applies the same shape to *no-action*
   cycles: after a small number (N ≈ 2–3) of consecutive no-progress cycles on
   one goal, force **one** definitive verification, then resolve it exactly
   once via the ladder below — and stop re-queuing the goal for another
   "verify" cycle.

```
consecutive no-progress cycles on goal G reaches N
        │
        ▼
run the concrete verification ONCE (not "I'll verify later")
        │
        ├─ evidence present  ──►  mark DONE via the done-gate (Fix 2)
        ├─ goal obsolete     ──►  DROP from the active board
        └─ neither           ──►  ESCALATE: file a GitHub issue for
                                   human review and Block the goal
```

The verification is the **done-gate** from Fix 2: "concretely verify" means
"ask the `EvidenceSource` whether the referenced PR is merged / the issue is
closed / the self-change is deployed," and then commit to the answer. The four
stuck supply-chain goals reach the first branch (evidence present → DONE) or
the second (out-of-scope issue filed → DROP); they can never reach a fourth
"I'll verify again" branch, because that branch does not exist.

### Status and boundaries

Layer 1 (prompt self-detection) and the brain-*failure* escalation of layer 3
are shipped and documented
([OODA loop self-detection](./ooda-loop-self-detection.md),
[Unblock OODA goals stuck after a brain-failure lockout](../howto/unblock-stuck-ooda-goals.md)).
Layer 2's `NO ACTION` classification is shipped in the goal-session parser
(`src/ooda_actions/goal_session/`), reached through the advance-goal dispatch.
The **no-action** counterpart of the layer-3 escalation — a per-goal
consecutive-no-progress counter that forces the resolution ladder above — is
the concept this page defines; it reuses the existing sentinel-`Blocked` +
file-an-issue machinery and the done-gate rather than introducing a new
state-machine in the reasoners, to keep the change inside
`goal_curation` / the advance-goal path (the naming-cleanup rename owns the
`ooda_brain` / reasoner / bridge files, so those are left untouched).

**Test shape.** A goal that yields `GoalAction::NoAction` N times in a row
triggers the breaker exactly once and terminates in DONE / DROP / ESCALATE — it
must **not** produce an (N+1)th no-action cycle.

---

## Fix 4 — Distillation banner-stripping

**Symptom:** `distill: 50 episodes -> 0 facts, 0 procedures`.

**Root cause.** The transcript the distillation parser reads was prefixed with
the **Copilot CLI launch-log banner** — the
`… launching copilot binary=… version="GitHub Copilot CLI …"` line and the
`ℹ … NODE_OPTIONS=… (saved preference)` info marker (the exact banner visible
in this session's terminal heartbeat). That preamble sits in front of the
agent's real `{ "facts": …, "procedures": … }` payload, so the parser sees
noise and extracts nothing.

**What shipped.** The banner and ANSI noise are stripped at the **single
shared `recipe_output` chokepoint** (`strip_recipe_noise` /
`is_copilot_launcher_line` in `src/recipe_output/extract.rs`). Because every
recipe-backed brain phase **and** the distillation capture path read their
agent output through this one function, the strip that originally fixed the
decide/orient deadlock now also cleans the distill transcript — no per-caller
duplication. `is_copilot_launcher_line` anchors on the launcher-line markers
and requires *both* `NODE_OPTIONS=` and `(saved preference)` on the info-marker
line, so genuine prose that merely mentions `NODE_OPTIONS` is never eaten. See
[Copilot launch-log preamble stripping](./copilot-launcher-preamble-stripping.md)
and [Distill recipe-output capture](../reference/distill-recipe-output-capture.md);
operators diagnosing a parse failure use the
[decide/orient parse-failure runbook](../howto/diagnose-decide-orient-parse-failures.md).

**Tests.** A regression fixture feeds a **banner-prefixed** distill transcript
through the shared chokepoint and asserts **> 0 facts** are extracted, so a
re-introduced banner leak fails CI at the distill path, not only at
decide/orient.

---

## Why these four ship together

Each fix removes one leg of the same stool:

- **Fix 1** makes the board **steerable** — operator and meeting intent survive
  and are visible on the next read.
- **Fix 2** lets objectively-finished goals **leave** the active board on
  evidence, instead of being re-litigated at 0%.
- **Fix 3** drives a goal that repeatedly produces no progress toward a
  **definitive** resolution instead of an endless "I'll verify" loop — shipped
  today for brain *failures*, and by design (see
  [Status and boundaries](#status-and-boundaries)) for the *no-action* livelock
  counterpart.
- **Fix 4** restores **learning**, so distillation turns episodes back into the
  facts and procedures that let the daemon notice it is repeating itself.

Steerable input, evidence-based exit, bounded escalation, and working memory:
remove any one and the daemon can slide back toward the un-steerable, stuck
state this incident captured.

---

## Guarantees and non-guarantees

**Guaranteed**

- **Read-your-writes** for goal-board edits under the daemon IPC path or the
  advisory `flock`: an `add`/`remove` is observed by the next `goal list` and
  the next curation cycle (Fix 1).
- **No evidence-free completion** and **no evidence-free perpetual re-litigation**:
  a goal with a derivable signal is auto-resolved by the done-gate rather than
  returned to the active set at 0% (Fix 2).
- **Bounded no-progress** *(design defined here; the no-action breaker is not
  yet shipped — see [Status and boundaries](#status-and-boundaries))*: a goal
  must not emit unbounded consecutive no-action cycles; it terminates in DONE /
  DROP / ESCALATE (Fix 3). Layer 1 (prompt self-detection) and the
  brain-*failure* escalation are shipped today; the per-goal
  consecutive-no-action counter that closes this guarantee is the concept this
  page defines.
- **Banner-immune distillation**: a launch-banner-prefixed transcript still
  yields facts, verified by regression fixture (Fix 4).

**Not guaranteed**

- **Strict linearizability** of `save_goal_board` across separate IPC clients —
  merge-on-write prevents goal *disappearance* in the common race, but a tight
  read-read-write-write interleaving can still drop the earlier writer's most
  recent *field* edit on the same `id`. Callers needing strict serializability
  route through the daemon IPC socket. See the
  [`save_goal_board` non-guarantees](../reference/goal-board-api.md#save_goal_board).
- **Escalation cannot manufacture evidence.** If neither completion evidence
  nor an obsolescence signal exists, the breaker files an issue and blocks the
  goal for human review — it does not guess.
- **Prompt-level self-detection is best-effort**; the layer-3 escalation exists
  precisely because layer 1 can be fooled by a confidently-looping brain.

---

## See also

- [Goal board persistence](./goal-board-persistence.md) — the single-source-of-truth
  design, consumer matrix, and cycle-startup sequence.
- [Goal board corruption guards](./goal-board-corruption-guards.md) — the
  pre-write validity gate.
- [Deploy-aware done-gate](./deploy-aware-done-gate.md) ·
  [Completion-evidence gate API](../reference/completion-evidence-gate-api.md).
- [OODA loop self-detection](./ooda-loop-self-detection.md) ·
  [Unblock OODA goals stuck after a brain-failure lockout](../howto/unblock-stuck-ooda-goals.md).
- [Copilot launch-log preamble stripping](./copilot-launcher-preamble-stripping.md) ·
  [Distill recipe-output capture](../reference/distill-recipe-output-capture.md).
- [How to recover a corrupted or missing goal board](../howto/recover-goal-board.md).
