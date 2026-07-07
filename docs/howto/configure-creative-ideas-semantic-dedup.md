---
title: "How to configure and operate Creative-Ideas semantic dedup"
description: >
  Operator + developer guide for the Creative Ideas semantic dedup + enhance
  gate (#2925) — how it plugs into the generation tick, tuning the coarse
  shortlist size (SIMARD_CREATIVE_IDEAS_DEDUP_SHORTLIST_K), switching the
  agentic layer off (SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP), editing the
  hot-reloadable dedup prompt to change SKIP/ENHANCE/CREATE quality, reading the
  per-tick telemetry counts and the per-candidate metric + judgment records,
  diagnosing an idea that keeps getting skipped or enhanced onto the wrong node,
  and running the one-time dry-run-first consolidation pass over the existing
  ~104-idea pool.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
status: draft
related:
  - ../concepts/semantic-creative-ideas-dedup.md
  - ../reference/creative-ideas-dedup-gate-api.md
  - ../reference/creative-idea-dedup-recipe.md
  - ../operations/creative-ideas-semantic-dedup-kill-switch.md
  - ./configure-creative-ideas-thread.md
  - ../operator-dashboard/creative-ideas-operator-controls.md
  - ../howto/edit-the-ooda-brain-prompt.md
---

# How to configure and operate Creative-Ideas semantic dedup

> **Status: implemented (#2925).** The gate this page describes lives in
> `src/creative_ideas/dedup_gate.rs`
> and reasons through
> `prompt_assets/simard/recipes/creative-idea-dedup.yaml`.
> For the rationale see
> [semantic dedup + enhance-existing gate](../concepts/semantic-creative-ideas-dedup.md);
> for the typed surface, the [API reference](../reference/creative-ideas-dedup-gate-api.md).

Before the Creative Ideas thread persists a generated candidate, it asks an
agentic reasoner whether the idea is **new** (`create`), a **duplicate**
(`skip`), or the **same idea as one already on the board** that should be
strengthened (`enhance`). This page is the operator guide: what it does per tick,
how to tune it, how to read what it decided, and how to clean up the duplicates
that already exist.

## Where it sits in the tick

The gate is part of the normal [Creative Ideas generation tick](./configure-creative-ideas-thread.md).
Per tick:

1. Generate a batch of candidate ideas (bounded by `SIMARD_CREATIVE_IDEAS_BATCH`).
2. For each candidate, the gate builds a **coarse shortlist** of the nearest
   existing ideas (cheap word-set Jaccard ranking), then asks the reasoner for
   `create` / `skip` / `enhance <node_id>`.
3. Apply: **create** persists a new idea and routes it as before; **skip** drops
   it; **enhance** strengthens the named existing idea in place (append rationale
   + evidence, **no new node**).
4. Emit one summary tracing line and a per-candidate metric + judgment record.

Nothing here changes the reviewer/routing pipeline — the **create** path is
byte-for-byte the pre-#2925 behaviour.

## Turn the agentic layer on/off

The semantic layer is **on by default**. To fall back to the old deterministic
Jaccard-only behaviour (no brain call), set the kill switch:

```bash
# Revert to deterministic Jaccard dedup only (no agentic reasoning).
SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP=off simard daemon
```

Only the exact value `off` (case-insensitive) disables it; any other value
(including a typo) leaves it **on**. The variable is read once at daemon start —
restart to change it. Turning it off never disables dedup entirely; it drops to
`skip`-or-`create` on the Jaccard threshold. Full details and the systemd recipe
are on the [kill-switch page](../operations/creative-ideas-semantic-dedup-kill-switch.md).

## Tune the coarse shortlist

The reasoner only sees the top-`K` nearest existing ideas per candidate, so the
prompt stays small as the pool grows. Tune `K` with:

| Variable | Default | Range | Effect |
| --- | --- | --- | --- |
| `SIMARD_CREATIVE_IDEAS_DEDUP_SHORTLIST_K` | `12` | clamped to `1..=64` | How many nearest existing ideas the reasoner compares against per candidate. |

```bash
# Show the reasoner a wider comparison set (more thorough, larger prompt).
SIMARD_CREATIVE_IDEAS_DEDUP_SHORTLIST_K=24 simard daemon
```

Raise it if genuine duplicates are slipping past because the true match ranked
just outside the shortlist; lower it to shrink prompt cost. Out-of-range or
unparseable values fall back to `12` with a `WARN`, so a typo is visible rather
than silently neutralizing the setting.

## Tune the reasoning (optional, no rebuild)

The **quality** of the skip/enhance/create judgment lives in the hot-reloadable
prompt, not in code. To change how aggressively Simard dedups, or to bias toward
enhancing rather than skipping, edit the prompt and the **next tick** uses it —
no restart, no rebuild:

- Operator override (preferred for iteration):
  `~/.simard/prompt_assets/simard/recipes/creative-idea-dedup.yaml`
- In-repo asset:
  `prompt_assets/simard/recipes/creative-idea-dedup.yaml`
  (prompt body in `prompt_assets/simard/creative_idea_dedup.md`).

The resolution order and the editing workflow are the same as every other brain
recipe — see [edit the OODA brain prompt](./edit-the-ooda-brain-prompt.md). The
[recipe & prompt schema](../reference/creative-idea-dedup-recipe.md) documents the
context variables and the required JSON envelope; keep the envelope shape when
editing, or the shim fails closed to the Jaccard backstop.

## Read what it decided

**Per-tick summary** — one `[simard]` tracing line (target `creative_ideas`):

```
[simard] creative_ideas dedup: generated=10 skipped=4 enhanced=3 created=3
```

`generated` is the batch size; `skipped + enhanced + created` accounts for every
candidate. A healthy board shows `skipped`/`enhanced` rising and `created`
falling over time as the pool saturates.

**Per-candidate** — a `creative_idea_dedup_decision` metric line in
`metrics.jsonl` (tagged `create` / `skip` / `enhance`, with the
`target_node_id` on enhance) and a `CreativeIdeaDedup` judgment record stamped
with the prompt-asset version and the decision rationale. Read them the same way
as any other judgment record (see the
[API reference — observability](../reference/creative-ideas-dedup-gate-api.md#observability)).

## Diagnose a bad decision

| Symptom | Likely cause | What to do |
| --- | --- | --- |
| A genuinely new idea keeps getting **skipped** | Prompt is over-aggressive, or the shortlist is too small so a superficially-similar idea dominates | Read the `CreativeIdeaDedup` rationale; widen or sharpen the rubric in the prompt; check `SIMARD_CREATIVE_IDEAS_DEDUP_SHORTLIST_K`. |
| Two obvious duplicates both persist as **created** | The true match ranked outside the coarse shortlist | Raise `SIMARD_CREATIVE_IDEAS_DEDUP_SHORTLIST_K`; the semantic judge only sees the shortlist. |
| An **enhance** merged onto an unrelated idea | Reasoner named a plausible-but-wrong `node_id` | The seam only accepts a `target_node_id` **from the shortlist**; a target outside it already fails closed. Tighten the prompt's "same underlying change to the same target" rubric. |
| Every candidate is **created**, counts never move | Kill switch is `off`, or the brain is erroring | Check `SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP`; look for a loud `error!` from the gate (a brain error fails closed to Jaccard, which under an empty-ish pool creates). |
| A tick logs a dedup **error** and creates nothing for that candidate | Recipe run/parse failed | This is the fail-closed path: the candidate is **not** silently persisted; it is retried next tick. Fix the recipe/prompt; verify with the [recipe schema](../reference/creative-idea-dedup-recipe.md). |

> **Fail-closed, by design.** If the reasoner errors, the gate never mints a
> silent duplicate and never enhances on a guess — it drops to the deterministic
> Jaccard backstop and surfaces the error. The worst case is "no worse than the
> old Jaccard behaviour," never "a wrong-node write."

## Consolidate the existing duplicate pool

The gate prevents *new* duplication, but the ~104 ideas already on the board need
a one-time cleanup. That is a separate, **operator-invoked, dry-run-first**
consolidation pass — also recipe-driven (it clusters the pool by meaning), not a
code heuristic.

**Step 1 — dry run (default).** Report the proposed merges; write nothing:

```bash
simard creative-ideas consolidate
```

Or, from the operator dashboard, the **Creative Ideas** tab's **Consolidate**
control (dry-run preview). The output is a plan of clusters: for each, the
**canonical** idea to keep and the **redundant** ideas that would be merged into
it and marked `Rejected`.

**Step 2 — apply.** After reviewing the plan, perform the writes:

```bash
simard creative-ideas consolidate --apply
```

This enhances each canonical idea (appending the merged rationale + evidence) and
transitions each redundant idea to `Rejected` (a valid `New → Rejected`
transition) with rationale `"merged into <canonical>"`. **No idea is deleted** —
collapsed ideas remain auditable in the terminal `Rejected` state, and the
dashboard's default view filters them out. Re-running is idempotent.

> **Always dry-run first.** Consolidation is the one operation that rewrites the
> existing board in bulk. Review the cluster plan before `--apply`. Because it
> only *rejects* (never deletes), a mistaken merge is recoverable by inspecting
> the `Rejected` ideas.

## See also

- [Concept: semantic dedup + enhance-existing gate](../concepts/semantic-creative-ideas-dedup.md)
- [Dedup-gate API reference](../reference/creative-ideas-dedup-gate-api.md)
- [Dedup recipe & prompt schema](../reference/creative-idea-dedup-recipe.md)
- [Semantic-dedup kill switch](../operations/creative-ideas-semantic-dedup-kill-switch.md)
- [Configure and operate the Creative Ideas thread](./configure-creative-ideas-thread.md)
- [Creative Ideas tab — operator controls](../operator-dashboard/creative-ideas-operator-controls.md)
