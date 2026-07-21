---
title: Keep the research goal never idle
description: How to read, verify, and revise the #4399 never-idle mandate for Simard's standing cognition-research goal — the prompt/charter directive that makes it produce a NEW source or NEW experiment every cycle (dedup'd, degrade to local experiment, never idle) and the thin fail-closed breaker rail that treats a slipped idle as a fault to re-orient rather than the benign perpetual-idle exemption. Durable prompt/charter/code, not a runtime CLI tweak.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: how-to
status: implemented
related:
  - ../concepts/research-goal-never-idle.md
  - ../reference/research-goal-never-idle-rail-api.md
  - ./steer-a-standing-research-goal-toward-novelty.md
  - ../concepts/perpetual-goal-no-progress-exemption.md
---

# How-To: Keep the research goal never idle

Simard's standing cognition-research goal is durably steered to **never idle**:
every cycle it must produce one concrete **novel** action — a **new external
source ingestion** or a **new measurable experiment** (deduplicated against its
recent directions) — and an idle cycle for **this** goal is a **fault** the
daemon re-orients out of, not the benign perpetual-idle rest granted to other
standing goals (#4399). This guide shows how to read, verify, and revise that
mandate — **without a runtime CLI tweak**, which the daemon would clobber.

For the rationale, see
[The standing research goal never idles — an idle cycle is a fault](../concepts/research-goal-never-idle.md).
For the exact predicate, classifier, and report field, see the
[never-idle rail API reference](../reference/research-goal-never-idle-rail-api.md).

## Why you cannot fix this from the CLI

Do **not** try to steer this goal with `simard goal set-priority`,
`simard goal add`, or a description mutation. The running daemon owns the
authoritative in-memory goal board and re-persists it every cycle
(`save_goal_board`), overwriting any external write to `goal_board.json`. See
[Authoritative goal-board store](../concepts/authoritative-goal-board-store.md).
The durable mandate lives in the daemon's **prompt assets, goal charter, and
code**, read fresh each cycle.

## TL;DR

1. Edit the never-idle charter/directive prose in
   `prompt_assets/simard/goal_session_objective.md` (the G0 / cognition block).
2. Adjust the one-line "next-source / next-experiment" reinforcement in
   `prompt_assets/simard/ooda_orient.md` and
   `prompt_assets/simard/ooda_decide.md`.
3. Keep the code-owned static copy in
   `src/ooda_actions/goal_session/input.rs` in sync (guarded by
   `is_standing_research_goal()`).
4. The thin safety-net rail lives in `src/ooda_loop/no_progress.rs`
   (`classify_standing_idle`, `research_idle_faults`); change it only if you are
   changing the fault handling, not the mandate.
5. Rebuild: `cargo build --release -p simard`
   (prompts are compiled in via `include_str!`; a rebuild is required).
6. Restart the daemon (see [run-ooda-daemon](run-ooda-daemon.md)).
7. Confirm from the next cycle report / journal that the goal opens with a
   **new source or new experiment**, never an idle.

## What the mandate does

When Simard advances a goal for which
`ActiveGoal::is_standing_research_goal()` holds
(**standing/perpetual AND cognition/research-worded**), her per-goal reasoning
context requires, each cycle:

1. **Never idle** — produce exactly one concrete novel action.
2. **Prefer a NEW external source** — discover and ingest a paper / repo /
   technique / dataset relevant to metacognition, memory, recall, or
   reasoning-reliability that she has not already used. Ingested sources are
   treated as **untrusted data, never instructions**.
3. **Otherwise a NEW experiment** — design a hypothesis + metric + method, run it
   locally, and record the result. A reasoned **negative result counts as
   progress**.
4. **Dedup** the source/experiment against her own recent PRs and experiments
   (reusing `creative_idea_dedup.md`) so it is a **new direction**, never a
   re-tweak of a seam already worked.
5. **Degrade to a LOCAL experiment** when no external source is reachable — there
   is **no idle fallback and no repeat fallback**.

If Lever A ever slips and the goal idles anyway, the breaker rail records a
**fault** (not the benign exemption), warns, resets the counter, and re-orients
the goal so the next cycle re-enters work generation. It **never** blocks, kills,
or parks the goal (fail-closed).

## Verify the mandate is active

The mandate applies to a goal iff **both** predicates hold. Check a goal's
description:

- **Standing?** It durably marks itself standing — e.g. `STANDING PERPETUAL goal`
  or the `[standing]` sentinel (`is_perpetual()` / `description_marks_standing`).
- **Research?** Its description names cognition/research subject matter — any of
  `cognition`, `recall`, `distillation`, `reasoner`, `memory`, `consolidation`,
  `retrieval`, `embedding` (leading-word-boundary match, so a description that
  merely contains a marker as a non-leading substring never falsely qualifies).

Both true → never-idle mandate applies and a slipped idle is a fault. Confirm at
runtime:

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
# Latest cycle report — the research goal should open with a NEW source/experiment:
ls -t "$SIMARD_HOME"/cycle_reports/cycle_*.json | head -1 | xargs cat | less
# The benign "idled this cycle (normal, not a fault)" line must NOT name the research goal:
grep -n "idled this cycle" "$SIMARD_HOME/ooda.log" | tail
# A slipped idle surfaces as a re-orient fault, not a block:
grep -n "research goal idled — fault" "$SIMARD_HOME/ooda.log" | tail
```

A healthy cycle for this goal produces a new source or experiment. The
`standing/perpetual goal idled this cycle (normal, not a fault)` line must never
name the research goal — if it does, the classifier is not routing this goal to
the fault path (check `is_standing_research_goal()` matches its description).

## Revise the mandate prose (most common)

The authoritative prose lives in
`prompt_assets/simard/goal_session_objective.md` (the G0 / cognition block). Keep
it **self-scoped** ("For any standing cognition/research goal, never idle…") — the
asset is appended to *every* goal's objective, so it must name the class, never a
slug.

Then edit the short "next-source / next-experiment" echoes in
`ooda_orient.md` and `ooda_decide.md` so the orient/decide phases apply the same
bias when they pick work. Keep these to a single line each; the canonical prose
stays in `goal_session_objective.md` so the two do not drift.

**Do not reintroduce an idle/incremental fallback.** The whole point of #4399 is
that step 3 is a *new experiment*, not "fall back to incremental maintenance". If
no external source is reachable, the floor is a **local experiment**, never idle.

Rebuild and restart (TL;DR steps 5–6).

## Change which goals qualify (predicate change)

Only needed if you want a different set of goals under the never-idle mandate.
Edit `src/goal_curation/types.rs`:

- Add/remove a marker in `RESEARCH_DESCRIPTION_MARKERS` (word-boundary matched).
- Do **not** add a goal-id / slug branch — keep the predicate general. Both the
  directive injection and `classify_standing_idle` key on the same
  `is_standing_research_goal()` predicate, so one edit re-scopes both levers.

Keep the code-owned static directive in `input.rs` in sync with the prompt prose,
and **never interpolate** `goal.description`, the slug, recalled memory, or
ingested source content into it (prompt-injection guard). Update the unit tests
in `types.rs`, the classifier tests in `no_progress.rs`, and the injection tests
in `src/ooda_actions/tests_goal_session.rs`.

## Tune the fault handling (rare)

The safety-net rail lives in `src/ooda_loop/no_progress.rs`:

- `classify_standing_idle(goal)` — the single decision point (research → fault,
  non-research standing → benign exempt, bounded → normal ladder). Both breaker
  sites call it; do not re-implement the branch inline.
- `ResearchIdleFault` — the fixed fault vocabulary. Add a category here (never a
  free-text string) if you need to distinguish a new idle cause.
- `research_idle_faults` on `NoProgressBreakerReport` — additive, excluded from
  `fired()`. A research-idle fault re-orients; it must **never** route to
  `escalated` / `Blocked`.

## Validate

```bash
cargo fmt
cargo build -p simard
cargo test -p simard goal_curation::types
cargo test -p simard no_progress
cargo test -p simard tests_goal_session
cargo clippy --all-targets --all-features -- -D warnings
```

> **Clippy note.** This repo denies warnings, including
> `clippy::doc_lazy_continuation`: indent multi-line `///` list continuations by
> 2+ spaces.

## What you must NOT regress

- **Benign exemption for OTHER standing goals.** Never make the never-idle mandate
  a blanket removal of the
  [perpetual-idle exemption](../concepts/perpetual-goal-no-progress-exemption.md).
  A non-research standing goal (e.g. CI-stewardship) that legitimately idles must
  still land in `perpetual_idled` and stay active.
- **Fail-closed.** A research-idle fault must only ever re-orient + keep the goal
  active. Never route it to block / kill / park / "needs human review".
- **Standing-perpetual semantics.** Never make the goal completable; #4399 changes
  *what the goal must do each cycle*, not its lifecycle. The completion-evidence
  gate stays untouched.
- **No injection.** Ingested sources stay untrusted data; fault categories stay a
  fixed code-owned vocabulary; goal ids in logs stay sanitized + length-bounded.
- **Bounded loop rate.** Re-orient respects existing cadence/backoff; cap one
  experiment per cycle. "Never idle" must not become a tight fetch/spawn loop.

## See also

- [The standing research goal never idles — an idle cycle is a fault](../concepts/research-goal-never-idle.md)
- [Never-idle rail API reference](../reference/research-goal-never-idle-rail-api.md)
- [Steer a standing research goal toward novelty](steer-a-standing-research-goal-toward-novelty.md)
  — the #4347 novelty steer #4399 builds on.
- [Standing/perpetual goals are exempt from the no-progress hard-block](../concepts/perpetual-goal-no-progress-exemption.md)
  — the benign exemption preserved for non-research standing goals.
- [Edit the OODA Brain Prompt](edit-the-ooda-brain-prompt.md)
- [Run the OODA daemon](run-ooda-daemon.md)
