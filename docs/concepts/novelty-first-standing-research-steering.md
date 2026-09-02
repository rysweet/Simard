---
title: Novelty-first steering for standing research/cognition goals
description: Why Simard's STANDING cognition-research goal is durably steered to FIRST survey genuinely-new research directions each cycle and PREFER a novel, benchmarked improvement over yet another incremental parse-site/dedup refinement — a prompt-asset directive reinforced by a thin, code-owned predicate hook, durable across daemon goal-board re-persist (not a runtime CLI tweak) (#4347).
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./steerable-ooda-daemon.md
  - ./perpetual-goal-no-progress-exemption.md
  - ./research-goal-never-idle.md
  - ./hybrid-cognition-measurement.md
  - ./authoritative-goal-board-store.md
  - ../reference/standing-research-goal-novelty-directive-api.md
  - ../reference/research-goal-never-idle-rail-api.md
  - ../reference/no-progress-breaker-api.md
  - ../howto/steer-a-standing-research-goal-toward-novelty.md
  - ../howto/keep-the-research-goal-never-idle.md
  - ../../prompt_assets/simard/goal_session_objective.md
  - ../../prompt_assets/simard/ooda_orient.md
  - ../../prompt_assets/simard/ooda_decide.md
---

# Novelty-first steering for standing research/cognition goals

> **Status: implemented.** When Simard advances a **standing cognition/research**
> goal, her per-goal reasoning context durably directs her to *first* survey
> genuinely-new research directions and *prefer* a novel, benchmarked improvement
> over another incremental refinement of an already-worked seam. The directive is
> owned by the prompt assets
> [`prompt_assets/simard/goal_session_objective.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/goal_session_objective.md),
> [`prompt_assets/simard/ooda_orient.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/ooda_orient.md),
> and [`prompt_assets/simard/ooda_decide.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/ooda_decide.md),
> and reinforced by a thin code-owned hook keyed on the
> [`ActiveGoal::is_standing_research_goal()`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs)
> predicate. See the
> [novelty-directive API reference](../reference/standing-research-goal-novelty-directive-api.md)
> for the exact predicate and injection point.

> **Extended by #4399 (never idle).** This page describes the novelty-first
> *steer* — when the goal acts, prefer a novel benchmarked direction. It permitted
> a **disclosed fallback to incremental maintenance** and, upstream, an outright
> **idle** cycle. [#4399](./research-goal-never-idle.md) closes both gaps: the goal
> must produce a NEW source or NEW experiment **every** cycle (dedup'd), the
> incremental/idle fallback is replaced by a **local-experiment** floor, and an
> idle cycle is now a **fault** the breaker re-orients out of (not the benign
> `perpetual_idled` exemption). Where this page says "still exempt from the
> hard-block" or "fall back to incremental only when no novel direction is viable",
> read the [never-idle concept](./research-goal-never-idle.md) as the current
> behaviour for the research goal.

## The defect this fixes (#4347)

The standing goal
`continuously-research-and-improve-your-own-cogn-70ab8541`
("Continuously research and improve your own cognition: graph memory, recall
quality, distillation fact-yield, and reasoner reliability. STANDING PERPETUAL
goal — durable improvements only") *was* running and shipping PRs — but they were
consistently **narrow incremental refinements of already-worked seams**:

| PR | What it refined |
| --- | --- |
| #4347-era recall precision | tuned an existing recall path |
| ranked-recall forwarding | forwarded an existing ranked-recall signal |
| trailing-comma JSON recovery | a parse-site robustness fix |
| distill dedup | a distillation dedup refinement |

Each is a legitimate maintenance improvement, but taken together they were the
goal repeatedly grabbing the *nearest* incremental fix instead of **surveying and
pursuing NOVEL, unexplored cognition-research directions** — new retrieval,
consolidation, reasoning, or ranking/embedding techniques, run as benchmarked
experiments. The operator's directive: when this goal runs, it should **first**
survey what it has *not yet* tried and **prefer** a genuinely new, benchmarked
direction over yet another parse-site refinement.

## Why a runtime CLI nudge does not stick

The obvious "fix" — `simard goal set-priority` or a `goal add`/description
mutation — **does not work here**, and the fix deliberately avoids it. The running
daemon owns the **authoritative in-memory goal board** and re-persists it every
cycle (`save_goal_board` in
[`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs)
and
[`src/ooda_actions/advance_goal/subordinate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/subordinate.rs)),
clobbering any external CLI write to `goal_board.json`. See
[Authoritative goal-board store](./authoritative-goal-board-store.md).

So a durable steer **must live inside the daemon** — in the prompt assets it reads
each cycle and/or in code — not in a CLI priority tweak that the next
`save_goal_board` overwrites. That is the load-bearing design constraint behind
this feature.

## The steer: prompt-first, thin-code-reinforced

Per the house philosophy (prompts/recipes over code — engineer guideline `G3`),
the primary lever is a **prompt directive**, reinforced by a **thin deterministic
hook** so it reliably targets *this* class of goal.

### 1. Prompt directive (primary) — self-scoped novelty-first mandate

[`goal_session_objective.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/goal_session_objective.md)
is appended to **every** goal's session objective, so the directive cannot target
one goal by id — it **self-scopes** by naming the goal *class*, exactly like the
existing G1/G2 "Cognition & self-improvement goals" block does. The directive adds
a novelty-first mandate that reads, in effect:

> **For any standing cognition/research goal, novelty-seeking is the primary
> directive.** Each cycle, **first** survey the space of NOVEL, unexplored
> cognition-research directions you have not yet tried — new graph-memory
> retrieval strategies, memory-consolidation techniques, reasoner-reliability
> approaches, ranking/embedding ideas, benchmarked experiments — drawing on your
> own memory and recent PRs so you do not repeat work already done. **Prefer**
> pursuing a genuinely new direction (prototype + benchmark against the current
> recall-precision / fact-yield baseline, delivering either a durable PR that
> implements the novel technique **or** a memory-recorded, reasoned NEGATIVE
> result explaining why it does not beat the baseline) **over** another
> incremental refinement of an already-worked seam (parse-site fix, dedup tweak).
> Fall back to incremental maintenance **only** when no novel direction is
> currently viable — and **say so** explicitly.

This slots into the existing G1/G2 cognition section and inherits its benchmark
+ live-self-measurement discipline (see
[Hybrid cognition measurement](./hybrid-cognition-measurement.md)): a novel
direction is not "done" on a benchmark number alone, and a negative result is a
first-class, memory-recorded outcome.

### 2. Orient/decide reinforcement — echo at the decision point

The orient and decide phases pick *what to pursue* before the goal-session prompt
runs. A one-line reinforcement in the `G1/G2/G3` engineering-guidelines
blockquotes of
[`ooda_orient.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/ooda_orient.md)
and
[`ooda_decide.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/ooda_decide.md)
echoes the same steering — "for a standing research goal, prefer a novel,
benchmarked direction over an incremental parse-site refinement" — so the bias is
present at the moment the cycle chooses work, not only once the goal session is
already underway.

### 3. Thin code hook — reliable targeting the prompt cannot guarantee

Because the objective asset is appended unconditionally, a prompt directive can
only *self-scope*; it cannot *deterministically* guarantee the mandate is present
for this goal and absent for ordinary goals. A thin, code-owned hook closes that
gap. When Simard builds the per-goal advance input
([`build_goal_advance_input`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/goal_session/input.rs)),
she checks
[`ActiveGoal::is_standing_research_goal()`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs)
and, only when it holds, appends a **static, code-owned** copy of the
novelty-first directive to the reasoning context. The predicate is:

```text
is_standing_research_goal  ==  description_marks_standing  AND  description_marks_research
```

- `description_marks_standing` — the **existing** standing-goal predicate
  (`is_perpetual()`), reused verbatim.
- `description_marks_research` — a new sibling predicate that matches
  cognition/research markers on a **leading word boundary**: `cognition`,
  `recall`, `distillation`, `reasoner`, `memory`, `consolidation`, `retrieval`,
  `embedding`. Leading-boundary matching keeps a description that merely contains
  a marker as a non-leading substring from qualifying; the motivating goal's
  charter ("graph memory, recall quality, …") matches via
  `memory`/`recall`/`distillation`/`reasoner`.

There is **no `70ab8541` slug branch**. Any standing goal whose description marks
it as research/cognition gets the steer; ordinary goals never do. See the
[novelty-directive API reference](../reference/standing-research-goal-novelty-directive-api.md).

## Why this is durable and safe

- **Durable across re-persist.** The steer lives in the daemon's compiled-in
  prompt assets and code, read fresh every cycle — it is not a `goal_board.json`
  field, so `save_goal_board` cannot clobber it. This is the whole point of not
  using a CLI nudge.
- **General, not a one-off.** The steer keys on the *standing-research nature* of
  the goal (a general predicate), not on the `70ab8541` slug. A future standing
  research goal inherits the same novelty bias automatically.
- **No prompt-injection surface.** The injected directive is a **static,
  code-owned string.** The hook never interpolates `goal.description`, the slug,
  or recalled memory text into the prompt, so a crafted goal description cannot
  smuggle instructions into the reasoning context. The predicate itself is pure,
  total, and panic-free over arbitrary/Unicode/very-long input.
- **Standing-perpetual semantics preserved.** The steer changes *what kind of
  work is preferred*, never the goal's lifecycle. The goal is still
  non-completable (the completion gate is untouched), and the
  [no-progress breaker exemption](./perpetual-goal-no-progress-exemption.md) is
  untouched — a standing research goal that legitimately idles is still exempt
  from the hard-block.
- **Anti-starvation intact.** The directive adds only a **disclosed fallback**
  ("fall back to incremental only when no novel direction is viable, and say
  so"). It never forbids progress and never lets the goal stall waiting for a
  novel idea — if nothing novel is viable this cycle, incremental maintenance is
  still available, just explicitly acknowledged.

## What an operator sees

Nothing to configure. When the standing cognition-research goal is advanced, its
cycle report and journal narrative now open with a **novelty survey** — a short
reasoned list of unexplored directions and why the chosen one was (or was not)
pursued — rather than jumping straight to "triage PRs / fix parse site". A cycle
that legitimately finds no viable novel direction states so explicitly before
falling back to maintenance. Over successive cycles the PR mix shifts from
near-duplicate incremental refinements toward benchmarked experiments and
memory-recorded negative results.

To read or revise the directive text, see
[How to steer a standing research goal toward novelty](../howto/steer-a-standing-research-goal-toward-novelty.md).

## Related

- [Keeping the OODA daemon steerable](./steerable-ooda-daemon.md) — the
  prompt-owns-policy / thin-Rust-rails split this feature follows.
- [Standing/perpetual goals are exempt from the no-progress hard-block](./perpetual-goal-no-progress-exemption.md)
  — the safeguard this steer must not regress.
- [Hybrid cognition measurement (benchmark + live)](./hybrid-cognition-measurement.md)
  — the benchmark/live-metric discipline a novel direction must satisfy.
- [Authoritative goal-board store](./authoritative-goal-board-store.md) — why a
  runtime CLI mutation is clobbered and the steer must live in the daemon.
- [Novelty-directive API reference](../reference/standing-research-goal-novelty-directive-api.md)
  — the predicate and injection point.
- [How to steer a standing research goal toward novelty](../howto/steer-a-standing-research-goal-toward-novelty.md).
