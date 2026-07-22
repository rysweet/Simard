---
title: Steer a standing research goal toward novelty
description: How to read, verify, and revise the #4347 novelty-first steer for Simard's standing cognition-research goal — the prompt directive and thin is_standing_research_goal() hook that make it prefer a novel benchmarked direction over an incremental refinement — via durable prompt assets and code, not a runtime CLI tweak the daemon would clobber. See also the stronger #4399 never-idle mandate.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: how-to
status: implemented
related:
  - ../concepts/novelty-first-standing-research-steering.md
  - ../reference/standing-research-goal-novelty-directive-api.md
  - ../concepts/research-goal-never-idle.md
  - ./keep-the-research-goal-never-idle.md
---

# How-To: Steer a Standing Research Goal Toward Novelty

Simard's standing cognition-research goal is durably steered to **first survey
genuinely-new research directions each cycle** and **prefer a novel, benchmarked
improvement over another incremental parse-site/dedup refinement** (#4347). This
guide shows how to read, verify, and revise that steer — **without a runtime CLI
tweak**, which the daemon would clobber.

> **See also #4399 (never idle).** This guide covers the novelty *steer* (prefer a
> novel direction when the goal acts). The stronger **never-idle** mandate — the
> goal must produce a NEW source or NEW experiment **every** cycle, and an idle is
> a fault, not a disclosed incremental fallback — is documented in
> [How to keep the research goal never idle](keep-the-research-goal-never-idle.md).

For the rationale, see
[Novelty-first steering for standing research/cognition goals](../concepts/novelty-first-standing-research-steering.md).
For the exact predicate and injection point, see the
[novelty-directive API reference](../reference/standing-research-goal-novelty-directive-api.md).

## Why you cannot fix this from the CLI

Do **not** try to steer this goal with `simard goal set-priority`,
`simard goal add`, or a description mutation. The running daemon owns the
authoritative in-memory goal board and re-persists it every cycle
(`save_goal_board`), overwriting any external write to `goal_board.json`. See
[Authoritative goal-board store](../concepts/authoritative-goal-board-store.md).
The durable steer lives in the daemon's **prompt assets and code**, read fresh
each cycle.

## TL;DR

1. Edit the canonical directive prose in
   `prompt_assets/simard/goal_session_objective.md` (the G1/G2 "Cognition &
   self-improvement goals" section).
2. Optionally adjust the one-line reinforcement in
   `prompt_assets/simard/ooda_orient.md` and
   `prompt_assets/simard/ooda_decide.md`.
3. If you changed which goals qualify, edit the predicate in
   `src/goal_curation/types.rs` (`RESEARCH_DESCRIPTION_MARKERS` /
   `is_standing_research_goal`) and the static string in
   `src/ooda_actions/goal_session/input.rs`.
4. Rebuild: `cargo build --release -p simard`
   (prompts are compiled in via `include_str!`; a rebuild is required).
5. Restart the daemon (see [run-ooda-daemon](run-ooda-daemon.md)).
6. Confirm from the next cycle report / journal that the goal opens with a
   **novelty survey** before any incremental work.

## What the steer does

When Simard advances a goal for which
`ActiveGoal::is_standing_research_goal()` holds
(**standing/perpetual AND cognition/research-worded**), her per-goal reasoning
context is directed to, each cycle:

1. **FIRST** survey NOVEL, unexplored cognition-research directions she has not
   yet tried — new graph-memory retrieval strategies, memory-consolidation
   techniques, reasoner-reliability approaches, ranking/embedding ideas,
   benchmarked experiments — drawing on her own memory and recent PRs to avoid
   repeating work.
2. **PREFER** a genuinely new direction: prototype + benchmark against the current
   recall-precision / fact-yield baseline, and ship either a durable PR
   implementing the technique **or** a memory-recorded, reasoned NEGATIVE result —
   in preference to another incremental parse-site / dedup refinement.
3. **Fall back** to incremental maintenance **only** when no novel direction is
   currently viable, and **say so** explicitly.

## Verify the steer is active

The steer applies to a goal iff **both** predicates hold. Check a goal's
description:

- **Standing?** It durably marks itself standing — e.g. the phrase
  `STANDING PERPETUAL goal`, or the `[standing]` sentinel
  (`description_marks_standing` / `is_perpetual()`).
- **Research?** Its description names cognition/research subject matter — any of
  `cognition`, `recall`, `distillation`, `reasoner`, `memory`, `consolidation`,
  `retrieval`, `embedding` (leading-word-boundary match, so a description that
  merely contains a marker as a non-leading substring never falsely qualifies).

Both true → the novelty directive is injected. Confirm at runtime by reading the
goal-session input or the cycle report for that goal:

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
# Latest cycle report — the standing research goal should open with a novelty survey:
ls -t "$SIMARD_HOME"/cycle_reports/cycle_*.json | head -1 | xargs cat | less
# Daemon log around the goal's advance:
grep -n "continuously-research-and-improve-your-own-cogn" "$SIMARD_HOME/ooda.log" | tail
```

A healthy cycle for this goal reasons over unexplored directions first; a cycle
that genuinely finds none states so before falling back to maintenance.

## Revise the directive prose (most common)

The authoritative prose lives in
`prompt_assets/simard/goal_session_objective.md`. Edit the G1/G2 cognition block
that names the standing cognition/research goal class. Keep it **self-scoped**
("For any standing cognition/research goal…") — the asset is appended to *every*
goal's objective, so it must name the class, never a slug.

Then edit the short echoes in the `G1/G2/G3` engineering-guidelines blockquotes of
`ooda_orient.md` and `ooda_decide.md` so the orient/decide phases apply the same
bias when they pick work. Keep these to a single line each; the canonical prose
stays in `goal_session_objective.md` so the two do not drift.

Rebuild and restart (steps 4–5 above).

## Change which goals qualify (predicate change)

Only needed if you want a different set of goals steered. Edit
`src/goal_curation/types.rs`:

- Add/remove a marker in `RESEARCH_DESCRIPTION_MARKERS` (word-boundary matched).
- Do **not** add a goal-id / slug branch — keep the predicate general.

Keep the code-owned static string in
`src/ooda_actions/goal_session/input.rs` in sync with the prompt prose, and
**never interpolate** `goal.description`, the slug, or recalled memory into it
(prompt-injection guard). Update the unit tests in `types.rs` and the injection
tests in `src/ooda_actions/tests_goal_session.rs`.

## Validate

```bash
cargo fmt
cargo build -p simard
cargo test -p simard goal_curation::types
cargo test -p simard tests_goal_session
cargo clippy --all-targets --all-features -- -D warnings
```

> **Clippy note.** This repo denies warnings, including
> `clippy::doc_lazy_continuation`: indent multi-line `///` list continuations by
> 2+ spaces.

## What you must NOT regress

- **Standing-perpetual semantics.** Never make the goal completable; the steer
  changes *what work is preferred*, not the lifecycle. The completion-evidence
  gate stays untouched.
- **No-progress exemption.** Do not touch the
  [standing/perpetual no-progress exemption](../concepts/perpetual-goal-no-progress-exemption.md);
  a standing research goal that legitimately idles must stay exempt from the
  hard-block.
- **Anti-starvation.** The directive adds only a *disclosed fallback*. Never let
  it forbid progress or stall the goal waiting for a novel idea.

## See also

- [Novelty-first steering for standing research/cognition goals](../concepts/novelty-first-standing-research-steering.md)
- [Standing-research novelty-directive API reference](../reference/standing-research-goal-novelty-directive-api.md)
- [Edit the OODA Brain Prompt](edit-the-ooda-brain-prompt.md)
- [Keeping the OODA Daemon Steerable](../concepts/steerable-ooda-daemon.md)
- [Run the OODA daemon](run-ooda-daemon.md)
