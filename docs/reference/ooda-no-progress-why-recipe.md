---
title: "Reference: the ooda-no-progress-why recipe and prompt schema"
description: >
  The optional ooda-no-progress-why.yaml recipe — the agentic ENRICHMENT layer
  that turns the breaker's deterministic root-cause classification and its
  gathered evidence into a human-readable WHY narrative for the escalation issue
  and block reason. Context variables, the single-string narrative output
  contract, the "enrichment NEVER changes routing" invariant, hot-reload
  resolution order, the NO-FALLBACK fail-closed contract, versioning, and tests.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./no-progress-root-cause-resolution-api.md
  - ./no-progress-breaker-api.md
  - ./recipe-brain-api.md
  - ./ooda-engineer-admission-recipe.md
  - ../concepts/no-progress-root-cause-resolution.md
  - ../howto/edit-the-ooda-brain-prompt.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../../prompt_assets/simard/recipes/ooda-no-progress-why.yaml
  - ../../src/goal_curation/no_progress_why.rs
---

# Reference: the `ooda-no-progress-why` recipe and prompt schema

> **Status: implemented (issue #16).** The recipe lives at
> `prompt_assets/simard/recipes/ooda-no-progress-why.yaml`. It is the **optional
> agentic-narrative** layer: production routing is done by the deterministic
> `DeterministicNoProgressReasoner` (see the
> [root-cause resolution API reference](./no-progress-root-cause-resolution-api.md#noprogresswhyreasoner)),
> and this recipe supplies the human-readable WHY narrative for the escalation
> issue. It never changes which ladder rung is taken.

## What this recipe is for

This recipe is the **optional agentic-enrichment** layer of the no-progress
root-cause resolution. When the breaker escalates a stall to a human, this recipe
turns the deterministic classification (`NoProgressClass`) and the structured
`Evidence` behind it into a **one-paragraph, human-readable WHY narrative** that
travels on the escalation issue and, appended, on the block reason.

It exists so the human who reads the block gets prose ("This goal's three
referenced issues are closed and its two PRs are merged, but the goal was never
marked complete — it appears **already done**; recommend verifying the deploy
signal and completing it") rather than only a token.

### What this recipe is NOT

The load-bearing decisions live in **Rust**, not here:

- **This recipe never chooses the ladder rung.** Routing
  (auto-complete / drop / heal / defer / guided-retry / escalate) is decided
  deterministically from evidence signals *before* this recipe runs. Editing this
  prompt changes narrative **quality**, never *which action is taken*.
- **This recipe never authorises a completion, a clone, a defer, or a spawn.**
  Those are performed by the adapter from the deterministic resolution.
- **Every `detail` field in the evidence is UNTRUSTED text.** The `verified`
  state of an artifact is set by an authenticated adapter (`gh`/reconcile), never
  by anything this prompt reads or writes.

Mirrors the framing of the
[closed-loop outcome-verification](../concepts/closed-loop-outcome-verification.md)
gate and the
[engineer-admission recipe](./ooda-engineer-admission-recipe.md): the prompt is
the reasoning surface; the safety rails are in Rust.

## Location and hot-reload resolution order

The reasoner resolves the recipe through the standard hot-reload path (same
order as every other OODA recipe), so operators can edit the narrative without a
rebuild:

1. `~/.simard/prompt_assets/simard/recipes/ooda-no-progress-why.yaml` (deployed /
   operator-editable copy), else
2. the repository copy at
   `prompt_assets/simard/recipes/ooda-no-progress-why.yaml`.

Resolution is **NO-FALLBACK**: if the reasoner is wired in but the recipe cannot
be resolved or the run errors, the breaker **fails closed** to the deterministic
narrative (it does not silently proceed with an empty or fabricated WHY, and it
does not change the routing). See
[fail-closed error handling](./no-progress-root-cause-resolution-api.md#fail-closed-error-handling).

> The reasoner is **optional**. The daemon may run with no reasoner wired
> (`None`), in which case the deterministic `NoProgressWhy::narrative` is used and
> this recipe is never consulted. The self-resolving and escalation behaviour is
> complete without it.

## Context variables

Passed via `-c key=value` (context-file transport, so large values never hit the
argv `E2BIG` limit):

| Variable | Meaning |
| --- | --- |
| `goal_id` | The stalled goal's id. |
| `goal_title` | The goal's title / description. |
| `success_criteria` | The goal's declared done-criteria, verbatim. |
| `class_token` | The deterministic classification token (e.g. `ALREADY-COMPLETE`, `GENUINELY-STUCK`). **Input, not a decision** — the recipe explains it, it does not re-derive it. |
| `evidence_json` | The gathered `Evidence[]` as JSON (kind, link, detail). UNTRUSTED `detail`. |
| `consecutive_cycles` | The no-action cycle count that tripped the breaker. |

## Output contract

The recipe emits a single JSON object with one required field:

```json
{ "narrative": "<one paragraph, plain language, references the evidence links>" }
```

The narrative is **display/log only** — it is quoted into the escalation issue
body. It is never parsed for control decisions. (The block reason itself is
rendered deterministically by
`no_progress_blocked_reason_with_why(consecutive, why)` as
`why=<TOKEN> evidence=[…]`, independent of this recipe.)

## Prompt shape

The prompt instructs the agent to:

1. Read `class_token` as the **given** diagnosis (do not second-guess routing).
2. Read `success_criteria` and `evidence_json` and write one plain-language
   paragraph explaining, for a human operator, *why* the goal stalled under that
   classification and what the linked artifacts show.
3. For `ALREADY-COMPLETE`, state which artifacts satisfy which criteria. For
   `UPSTREAM-DEPENDENCY`, name the blocking ref. For `MISSING-PRECONDITION`, name
   the absent precondition. For `UNCLEAR-CRITERIA` / `GENUINELY-STUCK`, state what
   an engineer should investigate first.
4. Emit only the JSON envelope.

## Versioning and tests

- The recipe carries a `version` in its header; bump it on any schema-affecting
  edit, exactly as the sibling OODA recipes do.
- Hermetic tests inject a **fake** `NoProgressWhyReasoner` (and, for the
  parse-shim tests, canned recipe output) — the recipe is **never** run against a
  live agent in CI, and no `gh`/subprocess is invoked. A parse-failure test
  asserts the fail-closed downgrade to the deterministic narrative.

## See also

- [Root-cause resolution API reference](./no-progress-root-cause-resolution-api.md) — the `NoProgressWhyReasoner` trait and the types this recipe feeds.
- [Concept: the breaker explains WHY and self-resolves before escalating](../concepts/no-progress-root-cause-resolution.md)
- [Recipe-brain API reference](./recipe-brain-api.md) — the shared runner and verdict-parse conventions this reasoner reuses.
- [Edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — how prompt hot-reload works in general.
