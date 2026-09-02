---
title: "Concept: semantic dedup + enhance-existing gate for Creative Ideas"
description: >
  Why the Creative Ideas thread judges each candidate idea for SEMANTIC
  duplication with an agentic reasoner (a hot-reloadable recipe + prompt), not a
  Rust similarity heuristic — the ~104-idea duplication incident (#2925), the
  SKIP / ENHANCE-EXISTING / CREATE-NEW decision, the word-set Jaccard demoted to
  a cheap coarse pre-filter and fail-closed backstop, why ENHANCE strengthens an
  existing idea instead of minting a near-duplicate node, and the one-time
  operator consolidation pass that collapses the pre-existing duplicates.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ../reference/creative-ideas-dedup-gate-api.md
  - ../reference/creative-idea-dedup-recipe.md
  - ../reference/ooda-record-idea-dedup-consolidation-cli.md
  - ../howto/configure-creative-ideas-semantic-dedup.md
  - ../operations/creative-ideas-semantic-dedup-kill-switch.md
  - ./resource-aware-engineer-admission.md
  - ../design/creative-ideas-thread.md
  - ../reference/creative-ideas-api.md
  - ../reference/creative-ideas-trigger-scoped-read.md
---

# Concept: semantic dedup + enhance-existing gate for Creative Ideas

> **Status: implemented (#2925).** The gate lives in
> `src/creative_ideas/dedup_gate.rs`,
> is wired into the generation tick in
> [`src/cognitive_threads/threads/creative_ideas.rs`](https://github.com/rysweet/Simard/blob/main/src/cognitive_threads/threads/creative_ideas.rs),
> and reasons through the hot-reloadable recipe at
> `prompt_assets/simard/recipes/creative-idea-dedup.yaml`.
> For the typed surface see the
> [dedup-gate API reference](../reference/creative-ideas-dedup-gate-api.md); to
> operate it see [how to configure semantic dedup](../howto/configure-creative-ideas-semantic-dedup.md).

Before the Creative Ideas thread persists a freshly generated candidate, it
asks one question: **is this really a new idea?** The answer is produced by an
**agentic reasoner** — a repeated, structured act of thought driven by a
hot-reloadable prompt — not by a Rust similarity threshold. The reasoner returns
exactly one of three outcomes for the candidate:

- **CREATE-NEW** — genuinely novel; persist it as today.
- **SKIP** — a true duplicate that adds nothing; drop it.
- **ENHANCE-EXISTING** — substantially the same underlying idea as one already
  in the pool, but with a new angle / rationale / evidence; **strengthen the
  existing idea** and create **no** new node.

This is the same brain-seam pattern as
[resource-aware engineer admission](./resource-aware-engineer-admission.md): a
structured context handed to the single Simard brain, a typed decision back, and
a thin, **fail-closed** Rust rail that applies it. The judgment lives in the
prompt; Rust is only the wiring.

## The incident that motivated it

The **Creative Ideas** tab was showing roughly **104 ideas with heavy semantic
overlap** — the same handful of underlying suggestions restated in different
words, over and over. Three things combined to produce that (#2925):

1. **The comparison pool was effectively empty.** Until the trigger-scoped read
   fix ([creative-ideas trigger-scoped read](../reference/creative-ideas-trigger-scoped-read.md)),
   each generation run compared against nothing, so every idea looked novel and
   was persisted.
2. **The only dedup signal was lexical.** The original filter is a deterministic
   **word-set Jaccard** (`dedup.rs`, threshold `0.6`). "Cache the goal-board
   reads" and "avoid re-reading `goal_board.json` every cycle" are the *same*
   idea, but they share few words, so Jaccard waves both through.
3. **There was no enhance path.** Even when the lexical filter *did* catch a
   near-duplicate, it simply **dropped** the candidate. The insight the
   candidate carried — a new rationale, a fresh piece of evidence — was thrown
   away instead of used to sharpen the idea already on the board.

The result was a low-signal, high-noise board: duplicates crowding out genuine
diversity, and no accumulation of supporting evidence on the good ideas.

## Why a recipe, not more Rust

Semantic equivalence — "same underlying idea, different words" — is a judgment,
not an arithmetic threshold. Encoding it as Rust cosine/Jaccard/threshold code
would be brittle (endless threshold-tuning that never captures paraphrase) and
would put the decision somewhere an operator cannot iterate on without a rebuild
and redeploy.

So the authority is a **prompt asset + recipe**:

- `prompt_assets/simard/creative_idea_dedup.md`
  — the reasoning, including a **semantic-equivalence rubric** (judge meaning,
  not shared tokens).
- `prompt_assets/simard/recipes/creative-idea-dedup.yaml`
  — the hot-reloadable recipe that runs it as a single agent step. The agent
  **records** its verdict by calling the gated `simard ooda record-idea-dedup`
  tool (a typed, owner-only `0o600`, freshness-checked record) — Rust reads that
  record fail-closed via `read_verified_idea_dedup`, never scraping the agent's
  stdout (epic [#4719](https://github.com/rysweet/Simard/issues/4719) Group C).

Editing the prompt changes dedup **quality** on the next tick — no rebuild. This
is the "repeated execution of structured thought" the issue asks for: the
reasoning is data, the Rust is a rail.

The old word-set Jaccard is **not deleted** — it is **demoted** to two humble
roles:

1. A **cheap coarse pre-filter** that ranks the pool and hands the reasoner a
   bounded *shortlist* of the nearest existing ideas, so the prompt stays small
   even as the pool grows.
2. A **fail-closed backstop** used only when the reasoner is unavailable or the
   semantic layer is switched off.

The **semantic judge is the authority**; Jaccard is never the decision-maker.

## What ENHANCE does

`ENHANCE-EXISTING <node_id>` is the new capability. Instead of minting a near-
duplicate, the gate loads the named existing idea and **strengthens it in
place**:

- appends the candidate's fresh **rationale** to the existing idea's rationale,
- merges in the candidate's **evidence links**,
- writes it back with `CreativeIdeaStore::update`, which appends a new
  **revision** under the same `idea_id`.

Because `list()` collapses to the latest revision per idea, the dashboard shows
the strengthened idea **once** — the pool count does **not** grow for a merge.
The idea's lifecycle **status is preserved**: ENHANCE never calls
`try_transition` (there is no `New → New` edge, and enhancing is not a lifecycle
event). Raising priority on enhance is deliberately **out of scope for v1** —
`priority()` is derived from review flags, not a settable field.

## Fail-closed by construction

A resource gate must never cause `ENOSPC`; this gate must never **silently
create a duplicate** — that is the failure it exists to prevent. So every
uncertain path resolves toward "do not blindly create, do not mutate a wrong
node":

| Condition | Behavior |
| --- | --- |
| Reasoner returns `Err` (recipe run / parse failed) | Deterministic Jaccard backstop (SKIP if a pool idea is over threshold, else CREATE) + a loud `error!`. Never a blind CREATE on a broken reasoner; never an ENHANCE on a guess. The candidate is naturally reconsidered next tick. |
| `ENHANCE-EXISTING` names a `node_id` **not** in the shortlist | Fall back to the deterministic backstop — never mutate an unrelated idea. |
| Unparseable / unknown decision tag | Treated as an error → backstop (above). |
| Semantic layer switched **off** (kill switch) | Deterministic Jaccard only (SKIP-or-CREATE). Reverts to *today's* pre-#2925 behaviour — never disables dedup entirely. |
| Empty pool | CREATE (nothing to compare against). |

The one deliberate difference from resource-admission: admission's Rust rail is
a *hard safety override* (the disk ceiling). Dedup has no irreversible
catastrophe, so the deterministic Jaccard here is a **fail-closed backstop**
(used on error / when disabled), not an always-on override. That keeps the change
**additive** and **never worse than today**: with the reasoner off or broken,
you get exactly the old Jaccard behaviour plus the new ENHANCE-safety of never
writing to a wrong node.

## Consolidating the ideas already on the board

The gate stops *new* duplication going forward, but it does not, by itself, clean
up the ~104 duplicates already persisted. That is a separate, **operator-invoked
consolidation pass** — also recipe-driven, not a code heuristic:

- A maintenance recipe
  (`prompt_assets/simard/recipes/creative-ideas-consolidation.yaml`)
  **clusters** the existing pool by semantic duplication and picks one
  **canonical** idea per cluster.
- The thin Rust applier **enhances the canonical** idea (merging rationale +
  evidence from the cluster) and transitions each **redundant** idea to
  `Rejected` via the `IdeaStatus` state machine (`New → Rejected` is a valid
  edge). **No silent deletes** — every collapsed idea remains auditable in a
  terminal `Rejected` state.
- It is **dry-run first**: by default it reports the proposed merges and writes
  nothing; an explicit `--apply` confirmation performs the writes. Re-running
  after apply is idempotent.

See [how to consolidate duplicate Creative Ideas](../howto/configure-creative-ideas-semantic-dedup.md#consolidate-the-existing-duplicate-pool).

## Where the boundaries sit

- **Decision authority** — the prompt + recipe. The reasoner judges semantic
  equivalence; its structured result is the outcome.
- **Orchestration / rails** — Simard, in `src/creative_ideas/dedup_gate.rs`
  (the "decision logic" the issue scopes to `src/creative_ideas`): shortlist
  building, the fail-closed rail, and the apply step. No type or function here
  contains the word `Bridge` (operator preference).
- **The memory model** — the `CreativeIdea` type, its `IdeaStatus` lifecycle,
  and the typed-link taxonomy — is owned **upstream** in
  `amplihack-memory-lib` and re-exported. If a real embedding *similarity
  primitive* is ever added, it lands there and only swaps the coarse-shortlist
  ranker; the gate contract is unchanged.

## Telemetry

Every generation tick emits one `[simard]`-prefixed tracing line summarising the
cycle:

```
[simard] creative_ideas dedup: generated=10 skipped=4 enhanced=3 created=3
```

and one `creative_idea_dedup_decision` metric plus a `CreativeIdeaDedup`
judgment record per candidate — so an operator can see, per cycle, how many
candidates were deduped, enhanced, and created. See the
[API reference — observability](../reference/creative-ideas-dedup-gate-api.md#observability).

## See also

- [Dedup-gate API reference](../reference/creative-ideas-dedup-gate-api.md) — the
  `IdeaDedupCtx` / `IdeaDedupDecision` types, `OodaBrain::decide_idea_dedup`, the
  `plan_candidate` seam and its rails, config, and the test matrix.
- [Creative-idea dedup recipe & prompt schema](../reference/creative-idea-dedup-recipe.md)
  — the recipe layout, context variables, the tool-call the agent makes, the
  semantic-equivalence rubric, and the consolidation recipe.
- [`simard ooda record-idea-dedup` / `record-idea-consolidation` tools](../reference/ooda-record-idea-dedup-consolidation-cli.md)
  — the typed records the recipes write and the fail-closed readers RecipeBrain
  uses instead of scraping prose.
- [How to configure and operate semantic dedup](../howto/configure-creative-ideas-semantic-dedup.md).
- [Semantic-dedup kill switch](../operations/creative-ideas-semantic-dedup-kill-switch.md).
- [Resource-aware engineer admission](./resource-aware-engineer-admission.md) —
  the sibling gate this one mirrors.
