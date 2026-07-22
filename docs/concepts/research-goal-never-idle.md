---
title: The standing research goal never idles — an idle cycle is a fault
description: Why Simard's STANDING cognition-research goal must produce a concrete NOVEL action every cycle — a new external source ingestion OR a new measurable experiment (dedup'd against recent directions) — and why an idle cycle for THIS goal is a fault the daemon actively re-orients out of, rather than the benign perpetual-idle exemption granted to other standing goals; prompt-first (charter + directive), reinforced by a thin fail-closed breaker rail keyed on ActiveGoal::is_standing_research_goal() (#4399).
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./novelty-first-standing-research-steering.md
  - ./perpetual-goal-no-progress-exemption.md
  - ./steerable-ooda-daemon.md
  - ./hybrid-cognition-measurement.md
  - ./semantic-creative-ideas-dedup.md
  - ../reference/research-goal-never-idle-rail-api.md
  - ../reference/standing-research-goal-novelty-directive-api.md
  - ../reference/no-progress-breaker-api.md
  - ../howto/keep-the-research-goal-never-idle.md
  - ../howto/steer-a-standing-research-goal-toward-novelty.md
  - ../../prompt_assets/simard/goal_session_objective.md
  - ../../prompt_assets/simard/ooda_orient.md
  - ../../prompt_assets/simard/ooda_decide.md
  - ../../prompt_assets/simard/creative_idea_dedup.md
---

# The standing research goal never idles — an idle cycle is a fault

> **Status: implemented.** The standing cognition-research goal is **directed** to
> produce a concrete **novel** action on **every** cycle: it should either discover
> and ingest a genuinely **new external source**, or design and run a **new
> measurable experiment** (hypothesis + metric + method), de-duplicated against its
> own recent directions. That novelty and non-repetition are a **prompt directive**,
> not a code-enforced guarantee — whether a given cycle's LLM output is genuinely
> new (and not a re-tweak) is prompt-hoped, not verified in code; this is "arguably
> the most code can do for LLM output". An idle cycle for **this** goal is a
> **fault**, not the benign
> [perpetual-idle exemption](./perpetual-goal-no-progress-exemption.md) granted to
> other standing goals. The primary lever is the goal charter + directive prose in
> [`prompt_assets/simard/goal_session_objective.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/goal_session_objective.md),
> [`prompt_assets/simard/ooda_orient.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/ooda_orient.md),
> and [`prompt_assets/simard/ooda_decide.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/ooda_decide.md);
> a thin, **reactive** fail-closed rail in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs),
> keyed on
> [`ActiveGoal::is_standing_research_goal()`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs),
> catches a *slipped* idle after the fact and re-orients the **next** cycle instead
> of exempting it. See the
> [never-idle rail API reference](../reference/research-goal-never-idle-rail-api.md)
> for the exact types and injection points.

## The defect this fixes (#4399)

The standing goal
`continuously-research-and-improve-your-own-cogn-70ab8541`
("Continuously research and improve your own cognition: graph memory, recall
quality, distillation fact-yield, and reasoner reliability. STANDING PERPETUAL
goal — durable improvements only") was **running but stagnant**. Two prior fixes
each solved part of the story and, together, left a gap:

| Fix | What it gave the goal | The residual gap |
| --- | --- | --- |
| [Perpetual-idle exemption (#2589)](./perpetual-goal-no-progress-exemption.md) | It is never hard-blocked for idling — a bursty standing goal may legitimately idle. | It treats **every** idle as *normal*, including for the research goal, so idling was rewarded, not corrected. |
| [Novelty-first steering (#4347)](./novelty-first-standing-research-steering.md) | When it *does* act, prefer a novel benchmarked direction over an incremental refinement. | It still permitted a **disclosed fallback to incremental maintenance** — and, upstream of that, an outright **idle** cycle counted as fine. |

The observed symptom over many cycles: the goal produced only ~15 near-identical
recall/keyword micro-tweak PRs, or **nothing at all**, while the journal logged,
cycle after cycle:

```
no-progress breaker: standing/perpetual goal idled this cycle (normal, not a fault)
```

For a self-research goal whose entire charter is *continuous exploration*, that
is exactly wrong. An idle cycle is not bursty rest — it is the goal failing to do
the one thing it exists to do. The operator's directive: **this goal must never
idle. Every cycle it must actively seek a new source or design a new experiment
that improves Simard's metacognition, and it must not repeat a recent
direction.**

## The mandate

> **Every cycle, the standing research goal should yield either a NEW external
> source-ingestion OR a NEW measurable experiment — not idle, not a repeat.**

This is the **directive target**, not a property the code proves each cycle.
"New" is **steered** by de-duplication against the goal's own recent PRs and
experiments (reusing the existing
[creative-idea dedup](./semantic-creative-ideas-dedup.md) semantics) — a
prompt-level nudge, not a code-enforced check on the LLM's chosen action.
"Measurable" inherits the [hybrid cognition measurement](./hybrid-cognition-measurement.md)
discipline: an experiment carries a hypothesis, a metric, and a method, and a
reasoned **negative result** is a first-class, memory-recorded outcome — an
experiment that disproves a hypothesis is *progress*, not an idle. What the code
**does** enforce is narrower and reactive: a research goal that *did* idle (and is
not holding a live in-flight artifact) is recorded as a fault and re-oriented so
the **next** cycle re-enters work generation.

This is **not** a blanket removal of the perpetual-idle exemption. Other standing
goals (e.g. a CI-stewardship perpetual goal) remain legitimately bursty and keep
the benign exemption. The never-idle mandate is **scoped to the research goal via
its charter**, through the `is_standing_research_goal()` predicate — not a
hardcoded goal id, and not a global change.

## The fix: prompt-first, thin-rail-reinforced

Per the house philosophy (accomplish via prompts/recipes/charter over code —
engineer guideline `G3`), the novelty expectation lives in the **prompt and
charter** (it is an expectation of the LLM, not something the code can guarantee);
the Rust is a **thin, reactive fail-closed safety net**, not the mechanism.

### Lever A — work generation (primary): close the "fall back to idle" loophole

The goal's per-cycle reasoning context already carried the novelty-first directive
(#4347). Its **step 3 fallback** was the loophole: *"fall back to incremental
maintenance only when no novel direction is viable — and say so."* That sentence
permitted an idle/repeat micro-tweak to be self-justified as acceptable. #4399
**rewrites that fallback** so the floor is no longer "incremental maintenance" but
"**design and run a NEW measurable experiment**":

> **Never idle. Each cycle you MUST produce one concrete novel action that
> advances your metacognition:**
> 1. **Prefer** discovering and ingesting a genuinely **NEW external source** —
>    a paper, repo, technique, or dataset relevant to metacognition / memory /
>    recall / reasoning-reliability — that you have not already used.
> 2. **Otherwise** design and run a **NEW measurable experiment** — a hypothesis,
>    a metric it moves (recall precision/latency, distillation yield, reasoner
>    reliability, novelty of learnings), and a method — run it locally and record
>    the result (a reasoned negative result counts).
> 3. **De-duplicate** the chosen source/experiment against your own recent PRs and
>    experiments (creative-idea dedup) so it is a **new direction**, never a
>    re-tweak of a seam you already worked.
> 4. There is **no idle fallback.** If no external source is reachable this cycle,
>    **degrade to a LOCAL experiment** — never to doing nothing and never to a
>    repeat.

This is encoded durably in the goal charter (G0 of
[`goal_session_objective.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/goal_session_objective.md))
and reinforced at the decision point in
[`ooda_orient.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/ooda_orient.md)
and
[`ooda_decide.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/ooda_decide.md)
with a "next-source / next-experiment" step, so the bias is present when the cycle
*chooses* work, not only once the goal session is underway. The static code-owned
copy injected by
[`build_goal_advance_input`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/goal_session/input.rs)
is updated in lockstep so the prompt and the reinforcement never drift.

### Lever B — breaker semantics (safety net): idle = fault, not exemption

The [no-progress breaker](../reference/no-progress-breaker-api.md) runs at two
sites in the cycle driver. Before #4399, both sites granted **every** standing
goal the benign `perpetual_idled` exemption. #4399 introduces a single shared
classifier, `classify_standing_idle()`, consumed by **both** sites (so they can
never drift), that splits the standing-idle case three ways:

- **Non-research standing goal idles** → unchanged: the benign
  `perpetual_idled` exemption. Counter reset, goal kept active, framed as *normal
  for a bursty goal*.
- **Research goal holding a LIVE in-flight artifact** (an open, unmerged PR /
  working branch / engineer session — `has_live_in_flight_ref()` holds) → **not a
  fault, not idle**: it is genuine in-flight progress. The counter is reset and the
  goal stays active, but it is recorded in **neither** `research_idle_faults` nor
  `perpetual_idled`, and it is **not** re-oriented — so its load-bearing `wip_refs`
  are preserved (see [In-flight progress is not idle](#in-flight-progress-is-not-idle-crusty-finding-1)).
- **Research goal idles with NO live artifact** (`is_standing_research_goal()`
  holds, `wip_refs` empty/dead) → a **fault**:
  1. record the goal id in the new report field
     **`research_idle_faults: Vec<String>`** (the fixed-vocabulary fault category
     is carried in the warn log, not folded into the entry),
  2. emit a distinct `tracing::warn!` — *"research goal idled — fault:
     re-orienting to generate a novel action"* — so a slipped idle is loud, not
     silent,
  3. **reset the counter** and re-dispatch the goal through the existing
    [`ActiveGoal::roll_to_new_cycle()`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs)
    re-orient primitive (the same one the
    [completion gate](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
    calls for a non-completable standing goal), so the next cycle re-enters
    Lever A work generation.

The research-idle fault **never** enters the hard-block / "needs human review"
escalation path — it is **fail-closed**: the goal is kept active and
re-dispatchable, never blocked, killed, or parked. Lever B is **reactive**: it
observes an idle cycle *after the fact* and forces the **next** cycle back into
work generation. It exists only to catch the case where Lever A slipped; when
Lever A holds, Lever B does not fire.

## In-flight progress is not idle (crusty finding 1)

A research goal that opened a durable PR — a genuine novel action — and then
produces a no-action cycle **while that PR is still open and unmerged** is NOT
meaningfully idle. Its `wip_refs` are **load-bearing**, not cosmetic: the
Overseer/Orient dedup set
([`overseer::sensor::in_flight_from_board`](https://github.com/rysweet/Simard/blob/main/src/overseer/sensor.rs))
"so the Overseer never fights an engineer already on a case", engineer-admission
control
([`ooda_brain::depended_on`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/mod.rs)),
and the completion gate's merge/close-verification signal all read them.
Classifying such a goal as a fault and calling `roll_to_new_cycle` would
`wip_refs.clear()` those refs — dropping the open PR from dedup, losing merge
tracking, and letting the next cycle spawn an **overlapping** engineer on the same
seam. The classifier therefore treats a live-in-flight research goal as
**progress** (`ResearchInFlight`): counter reset, goal active, but no fault and no
re-orient, so the refs survive. Only a research goal with **no** live artifact is
faulted and re-oriented.

## Why this is safe

- **Scoped by charter, not by id.** Both the never-idle mandate and the breaker
  fault path key on the structured
  [`ActiveGoal::is_standing_research_goal()`](../reference/standing-research-goal-novelty-directive-api.md#activegoalis_standing_research_goal)
  predicate (standing **and** cognition/research), which reuses the existing
  `is_perpetual()` flag. There is **no `70ab8541` slug branch**, and no other
  standing goal is affected.
- **No new prompt-injection surface.** The injected directive is a **static,
  code-owned string**; ingested external sources are treated as **untrusted
  data/evidence, never instructions** (the orient/decide/dedup prompts keep the
  XPIA posture). No goal description, slug, or source content is interpolated into
  the reasoning directive.
- **Fail-closed, never fail-open.** A research-idle fault re-orients and keeps the
  goal active; it can never disable the breaker for non-research goals and can
  never route the research goal into block/kill/park.
- **No log injection.** Every log line uses only the fixed `ResearchIdleFault`
  vocabulary category plus the goal-board slug id; the `research_idle_faults`
  entry is that same controlled id — no free text is ever folded in.
- **Bounded loop rate.** The re-orient respects the existing cycle cadence and
  backoff and caps at **one** experiment per cycle, so "never idle" cannot become a
  tight fetch/spawn loop.
- **Standing-perpetual semantics preserved.** The goal remains non-completable
  (the [completion gate](../reference/completion-evidence-gate-api.md) is
  untouched). #4399 changes *what the goal must do each cycle*, never its
  lifecycle.
- **Single source of truth for "idle is disruptive."** `research_idle_faults` is
  additive and, like `perpetual_idled`, is **excluded from `fired()`** — a
  research-idle fault is a re-orient, not a hard breaker action, but it is always
  visible via the per-goal `warn` and the report field.

## What an operator sees

Nothing to configure. The journal line

```
no-progress breaker: standing/perpetual goal idled this cycle (normal, not a fault)
```

**no longer applies to the research goal.** In steady state the research goal's
cycle report opens with a **next-source / next-experiment** action — a newly
ingested source or a newly designed experiment with its hypothesis, metric, and
method — distinct from its recent directions. On the rare cycle where Lever A
slipped and the goal would have idled, the log instead shows the fault + re-orient:

```
WARN simard::ooda: research goal idled — fault: re-orienting to generate a novel action goal=continuously-research-and-improve-your-own-cogn-70ab8541 category=no-novel-action-produced
```

and the goal is re-dispatched on the next cycle rather than sitting idle. Other
standing goals' legitimate bursty-idle behaviour is unchanged.

To read, verify, or revise the mandate, see
[How to keep the research goal never idle](../howto/keep-the-research-goal-never-idle.md).

## Related

- [Novelty-first steering for standing research/cognition goals](./novelty-first-standing-research-steering.md)
  — the #4347 predecessor; #4399 closes its residual "fall back to idle/incremental"
  gap and reuses its `is_standing_research_goal()` predicate.
- [Standing/perpetual goals are exempt from the no-progress hard-block](./perpetual-goal-no-progress-exemption.md)
  — the benign exemption #4399 keeps for **non-research** standing goals and
  supersedes for the research goal.
- [Keeping the OODA daemon steerable](./steerable-ooda-daemon.md) — the
  prompt-owns-policy / thin-Rust-rails split this feature follows.
- [Hybrid cognition measurement (benchmark + live)](./hybrid-cognition-measurement.md)
  — the measurement discipline every new experiment must satisfy; negative results
  are first-class.
- [Semantic creative-ideas dedup](./semantic-creative-ideas-dedup.md) — the dedup
  semantics reused to guarantee each source/experiment is a genuinely new direction.
- [Never-idle rail API reference](../reference/research-goal-never-idle-rail-api.md)
  — `classify_standing_idle`, the `research_idle_faults` field, the fault
  vocabulary, and both breaker sites.
- [Standing-research novelty-directive API reference](../reference/standing-research-goal-novelty-directive-api.md)
  — the `is_standing_research_goal()` predicate and the directive injection point.
- [How to keep the research goal never idle](../howto/keep-the-research-goal-never-idle.md).
