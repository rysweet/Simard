---
title: Creative-ideas semantic dedup-gate API reference
description: >
  Rust API reference for the Creative Ideas semantic dedup + enhance gate
  (#2925) — the IdeaDedupCtx / ExistingIdeaView / IdeaDedupDecision types, the
  OodaBrain::decide_idea_dedup trait method and its RecipeBrain override, the
  two-stage plan_candidate seam (coarse Jaccard shortlist → agentic judge) with
  its fail-closed rails, the PlannedAction outcome and apply_enhance append-only
  merge, the SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP kill-switch and
  SIMARD_CREATIVE_IDEAS_DEDUP_SHORTLIST_K knob, the typed-record fail-closed
  reader (read_verified_idea_dedup, #4719 Group C), the consolidation entrypoint,
  observability (metric + judgment record + tracing counts), module layout, and
  the hermetic test matrix.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/semantic-creative-ideas-dedup.md
  - ./creative-idea-dedup-recipe.md
  - ./ooda-record-idea-dedup-consolidation-cli.md
  - ./creative-ideas-api.md
  - ./resource-admission-api.md
  - ./recipe-brain-api.md
  - ./recipe-context-var-sanitization.md
  - ../howto/configure-creative-ideas-semantic-dedup.md
  - ../operations/creative-ideas-semantic-dedup-kill-switch.md
  - ../../src/creative_ideas/dedup_gate.rs
  - ../../src/creative_ideas/dedup.rs
  - ../../src/cognitive_threads/threads/creative_ideas.rs
  - ../../src/ooda_brain/mod.rs
  - ../../src/ooda_brain/recipe_brain.rs
---

# Creative-ideas semantic dedup-gate API reference

> **Status: implemented (#2925).** The typed surface below is live. The seam +
> rails + apply live in
> `src/creative_ideas/dedup_gate.rs`,
> with `IdeaDedupCtx` / `IdeaDedupDecision` / `ExistingIdeaView` and the
> `OodaBrain::decide_idea_dedup` method in
> [`src/ooda_brain/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/mod.rs),
> the production override + parser in
> [`src/ooda_brain/recipe_brain.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/recipe_brain.rs),
> the tick wiring in
> [`src/cognitive_threads/threads/creative_ideas.rs`](https://github.com/rysweet/Simard/blob/main/src/cognitive_threads/threads/creative_ideas.rs),
> and the reasoning asset at
> `prompt_assets/simard/recipes/creative-idea-dedup.yaml`.
>
> **One behavioural refinement vs. the original sketch:** a reasoner `Err` or an
> out-of-shortlist ENHANCE target does **not** fall back to the deterministic
> Jaccard backstop — it **fails closed by dropping the candidate this cycle**
> (never a silent duplicate), surfacing the error, and retrying next run
> ([`PlannedAction::FailClosed`](#plannedaction)). The Jaccard backstop is used
> only on the kill-switch-off path. This is the stronger "no silent duplicate"
> guarantee the issue requires.

This reference specifies the API of the **semantic dedup + enhance gate**. For
the rationale and safety model, see
[semantic dedup + enhance-existing gate for Creative Ideas](../concepts/semantic-creative-ideas-dedup.md).
The gate is another instance of the brain-seam pattern used by
[`decide_resource_admission`](resource-admission-api.md),
[`decide_engineer_admission`](engineer-admission-api.md), and
[`decide_engineer_lifecycle`](ooda-engineer-lifecycle-recipe.md):
`Ctx → OodaBrain method → RecipeBrain(recipe.yaml) → typed Decision → apply`.

Modules:
`simard::creative_ideas::dedup_gate`,
`simard::creative_ideas::dedup` (the reused coarse ranker),
`simard::ooda_brain` (the types + trait method),
`simard::cognitive_threads::threads::creative_ideas` (the tick seam).

## Contents

- [`ExistingIdeaView`](#existingideaview)
- [`IdeaDedupCtx`](#ideadedupctx)
- [`IdeaDedupDecision`](#ideadedupdecision)
- [`OodaBrain::decide_idea_dedup`](#oodabraindecide_idea_dedup)
- [The two-stage seam: `plan_candidate`](#the-two-stage-seam-plan_candidate)
- [`PlannedAction`](#plannedaction)
- [The fail-closed rails](#the-fail-closed-rails)
- [`apply_enhance` (append-only merge)](#apply_enhance-append-only-merge)
- [Tick wiring (`run_tick`)](#tick-wiring-run_tick)
- [`RecipeBrain::decide_idea_dedup` and the fail-closed reader](#recipebraindecide_idea_dedup-and-the-fail-closed-reader)
- [Consolidation entrypoint](#consolidation-entrypoint)
- [Configuration](#configuration)
- [Observability](#observability)
- [Kill-switch](#kill-switch)
- [Module layout](#module-layout)
- [Test matrix](#test-matrix)

## `ExistingIdeaView`

One existing idea, rendered as advisory context for the dedup brain. The
`node_id` is the ENHANCE target handle. Every string field is sanitized and
length-capped by the seam before it becomes a recipe `-c` variable — pool
content is **untrusted** (see
[context-var sanitization](recipe-context-var-sanitization.md)).

```rust
// src/ooda_brain/mod.rs (beside ResourceAdmissionCtx)
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ExistingIdeaView {
    /// The memory node id — the handle an ENHANCE decision names.
    pub node_id: String,
    /// The stable idea id (revisions share it).
    pub idea_id: String,
    /// The idea text (sanitized before templating).
    pub idea: String,
    /// The idea's stored rationale (sanitized, capped).
    pub rationale: String,
}
```

## `IdeaDedupCtx`

The structured context the brain reasons over for **one** candidate. Assembled
per candidate by the Simard-side seam. `existing_shortlist` is the bounded set of
nearest existing ideas produced by the coarse pre-filter (see
[`plan_candidate`](#the-two-stage-seam-plan_candidate)), so the prompt stays
small regardless of pool size.

```rust
// src/ooda_brain/mod.rs
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct IdeaDedupCtx {
    /// The candidate idea's text.
    pub candidate_idea: String,
    /// The candidate idea's rationale/context.
    pub candidate_rationale: String,
    /// The nearest existing ideas (coarse pre-filtered, bounded to K).
    pub existing_shortlist: Vec<ExistingIdeaView>,
}
```

## `IdeaDedupDecision`

What the brain decided for one candidate. serde-tagged on `choice` (snake_case)
so an **unknown tag fails to parse** → the seam fails closed. There is **no
`Default`**: the fail-closed path is chosen explicitly by the seam, never by
defaulting a decision on the brain's behalf.

```rust
// src/ooda_brain/mod.rs
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum IdeaDedupDecision {
    /// Genuinely novel — persist as a new `New` idea (today's default path).
    CreateNew { rationale: String },
    /// True duplicate that adds nothing — drop the candidate.
    Skip { rationale: String },
    /// Merge into the existing idea identified by `target_node_id`.
    EnhanceExisting { target_node_id: String, rationale: String },
}
```

- `choice: "create_new"` → `CreateNew`
- `choice: "skip"` → `Skip`
- `choice: "enhance_existing"` → `EnhanceExisting` (requires `target_node_id`)

`target_node_id` **must** be validated by the seam against the shortlist's
`node_id`s. An `EnhanceExisting` naming an unknown node degrades to the
fail-closed rail — it never blindly creates and never mutates an unrelated idea.

## `OodaBrain::decide_idea_dedup`

A **defaulted** trait method, so every existing brain and test-double compiles
unchanged and an un-migrated brain preserves today's behaviour (the deterministic
Jaccard rail remains the actual dedup). The production `RecipeBrain`
[overrides it](#recipebraindecide_idea_dedup-and-the-fail-closed-reader).

```rust
// src/ooda_brain/mod.rs, in `trait OodaBrain`
/// Decide SKIP / ENHANCE-EXISTING / CREATE-NEW for one candidate idea (#2925).
/// Defaulted to `CreateNew` so an un-migrated brain never silently *drops* an
/// idea; novelty then falls to the deterministic rail (identical to today).
fn decide_idea_dedup(
    &self,
    _ctx: &IdeaDedupCtx,
) -> SimardResult<IdeaDedupDecision> {
    Ok(IdeaDedupDecision::CreateNew {
        rationale: "semantic idea-dedup not implemented by this brain".into(),
    })
}
```

The default is `CreateNew` (not `Skip`): an un-migrated brain must never
silently *drop* ideas. This mirrors resource-admission defaulting to the
no-op-preserving `Admit`.

## The two-stage seam: `plan_candidate`

`plan_candidate` is the **pure** decision core — no IO, no store writes, no
metric emission — so the rail is trivially testable. It returns a
[`PlannedAction`](#plannedaction) the caller applies. It is two-stage to bound
prompt cost as the pool grows past ~104 ideas:

**Stage 1 — coarse shortlist (cheap, deterministic).** Rank the pool by the same
word-set similarity used by
[`dedup::is_near_duplicate`](https://github.com/rysweet/Simard/blob/main/src/creative_ideas/dedup.rs)
— its internal `jaccard` scorer, exposed `pub(crate)` as a similarity value for
ranking — and keep the top-`K` (default `12`). This is the allowed "cheap
similarity as a coarse filter"; Jaccard is the v1 primitive (no network, hermetic
tests). If an
embedding similarity is ever added at the store layer (in `amplihack-memory-lib`)
this stage swaps its ranker and the contract is unchanged.

**Stage 2 — agentic judge.** Build `IdeaDedupCtx { candidate, shortlist }` and
call `brain.decide_idea_dedup`, then apply the [rails](#the-fail-closed-rails).

```rust
// src/creative_ideas/dedup_gate.rs
/// PURE: no IO, no store writes, no metric. Returns the plan the caller applies.
pub(crate) fn plan_candidate(
    candidate: &RawIdea,
    pool: &[CreativeIdea],
    brain: &dyn OodaBrain,
    enabled: bool,          // kill-switch: false ⇒ deterministic rail only
    shortlist_k: usize,     // Stage-1 shortlist size (default DEFAULT_SHORTLIST_K)
    jaccard_threshold: f64, // reused DEFAULT_DEDUP_THRESHOLD for the backstop
) -> PlannedAction;
```

## `PlannedAction`

```rust
// src/creative_ideas/dedup_gate.rs
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlannedAction {
    /// Persist a new idea + review/route it (today's byte-for-byte path).
    Create,
    /// Drop the candidate; nothing is persisted.
    Skip { rationale: String },
    /// Merge the candidate into an existing idea (validated target).
    Enhance { target_node_id: String, rationale: String },
    /// Fail-CLOSED: the reasoner errored (or named a bad target). The candidate
    /// is dropped this cycle — never a silent duplicate — surfaced and retried
    /// next run.
    FailClosed { reason: String },
}
```

## The fail-closed rails

`plan_candidate` maps a brain result to a `PlannedAction` under these rails, in
order:

| Rail | Guard | Result |
| --- | --- | --- |
| **Kill-switch** | `enabled == false` | Deterministic rail only: a Jaccard match (≥ threshold) ⇒ `Skip`, else `Create`. Brain is **not** consulted. |
| **Empty pool** | `pool` is empty | `Create` (nothing to dedup against). Brain is not consulted. |
| **Empty shortlist** | nothing lexically near | `Create` (v1 lexical pre-filter limitation). Brain is not consulted. |
| **Brain error (fail-closed)** | `decide_idea_dedup` returns `Err` | [`PlannedAction::FailClosed`] + a loud `error!`. The candidate is **dropped this cycle** (not persisted) and retried next run. Never a blind `Create` on a broken reasoner; never an `Enhance` on a guess; never a silent duplicate. |
| **Bad ENHANCE target** | `target_node_id` ∉ shortlist | `FailClosed` (as above). Never mutate an unrelated idea, never a silent duplicate. |
| **Valid brain result** | otherwise | `CreateNew → Create`, `Skip → Skip`, `EnhanceExisting → Enhance`. |

The kill-switch-off path is the **only** place the deterministic Jaccard
backstop runs; a brain *error* while the semantic layer is ON fails closed by
dropping (the strongest "no silent duplicate" guarantee, per #2925).

Observability (the per-tick telemetry counts) is applied by the **caller**, not
inside `plan_candidate`, so the pure-rail tests write no files (mirroring
`ResourceAdmissionEvaluation` keeping observability out of the evaluator).

## `apply_enhance` (append-only merge)

Applies an `Enhance` plan against the `CreativeIdeaStore`. It is **append-only
and status-preserving**: it loads the target, merges the candidate's rationale
and evidence links, and writes back with `update`, which appends a **new
revision** under the same `idea_id`. It does **not** call `try_transition` — the
lifecycle status is preserved (there is no `New → New` edge, and enhancing is not
a lifecycle event).

```rust
// src/creative_ideas/dedup_gate.rs
pub(crate) fn apply_enhance(
    store: &dyn CreativeIdeaStore,
    target_node_id: &str,
    candidate: &RawIdea,
    rationale: &str,
    dry_run: bool,
) -> SimardResult<bool>;
```

Returns `Ok(false)` when the target node is gone (the caller then degrades to
`Create`, never losing the idea); `Ok(true)` when the merge was applied.

Behaviour:

1. `let mut existing = store.get(target_node_id)?;` — if the node is missing,
   returns `Ok(false)` so the caller degrades to `Create` (fail-closed; never a
   wrong-node write).
2. `existing.context.rationale = merge(existing rationale, candidate.rationale, decision rationale)` — appended behind an audit marker and length-capped.
3. `existing.links.extend(candidate.links)` — deduped by `(kind, node_id)`; this
   is how ENHANCE accretes evidence.
4. `priority()` is **not** bumped (it is review-derived; out of v1 scope).
5. `if !dry_run { store.update(&existing)?; }` — appends a revision at the same
   `idea_id`. **No new node** ⇒ the pool count does not grow for a merge.

Because `list()` collapses to the latest revision per idea (via
`latest_revision_per_idea`), the dashboard shows the strengthened idea **once**.

## Tick wiring (`run_tick`)

The generation tick replaces the old batch `reject_duplicates` call with a
per-candidate plan-and-apply loop. `select_balanced` still bounds the batch
**before** the gate, so the number of brain calls per tick is bounded by
`SIMARD_CREATIVE_IDEAS_BATCH`. The comparison pool is `inputs.previous_ideas`,
which is already the full trigger-scoped pool (see
[trigger-scoped read](creative-ideas-trigger-scoped-read.md)).

```text
selected = select_balanced(raw, cfg.batch)              // bound brain calls
for cand in selected:
    plan = dedup_gate::plan_candidate(cand, &inputs.previous_ideas,
                                      brain, semantic_enabled,
                                      shortlist_k, DEFAULT_DEDUP_THRESHOLD)
    match plan {
        Create        => { store.store(New); review_and_route(...); report.persisted += 1 }
        Skip { .. }   => { report.skipped += 1 }         // drop; nothing persisted
        Enhance { target, rationale } => {
            match apply_enhance(store, &target, cand, &rationale, ctx.dry_run)? {
                true  => report.enhanced += 1,           // 0 new nodes
                false => create(cand),                   // target vanished ⇒ never lose it
            }
        }
        FailClosed { reason } => {                        // reasoner error / bad target
            report.dedup_errors += 1                      // dropped this cycle, surfaced
            tracing::error!(reason, "…fail-closed…")      // retried next run
        }
    }
// one [simard] tracing line per tick with generated/skipped/enhanced/created
```

`GenerationReport` gains `skipped`, `enhanced`, and `dedup_errors` counters
(alongside the existing `persisted` = created). A `FailClosed` candidate is
**not** persisted (fail-closed, no silent duplicate) and is surfaced via
`tracing::error!` — it is regenerated and retried on the next tick. Each tick
emits one `[simard]`-prefixed structured tracing summary
(`generated`/`skipped`/`enhanced`/`created`).

**Brain access:** the `CreativeIdeasThread` gains a `Box<dyn OodaBrain>`
constructor seam (`with_pipeline_and_brain`), wired at `register()` /
`from_env()` from a `RecipeBrain` bound to `creative-idea-dedup.yaml` (falling
back **loudly** to a deterministic word-set brain with the semantic layer OFF
when recipe-runner-rs is unavailable), and a stub in tests. The legacy
`with_pipeline` constructor keeps today's deterministic behaviour (semantic
layer off) so existing tests are unchanged.

## `RecipeBrain::decide_idea_dedup` and the fail-closed reader

The production override mirrors
[`RecipeBrain::decide_engineer_admission` / `decide_resource_admission`](ooda-record-admission-cli.md)
after the [#4719](https://github.com/rysweet/Simard/issues/4719) Group C
typed-record rework — the recipe **acts via a tool**, and the shim **reads a
typed record fail-closed** instead of scraping stdout:

- Consts: `IDEA_DEDUP_RECIPE_FILENAME = "creative-idea-dedup.yaml"`, the prompt-store
  name `IDEA_DEDUP_PROMPT_NAME = "creative_idea_dedup.md"` for versioned judgment
  stamping, the schema pin `IDEA_DEDUP_SCHEMA = "simard.creative.idea_dedup.v1"`,
  and the fixed synthetic seam id `"creative-idea-dedup"` (with
  `REASONER_RECORD_CYCLE = 0`).
- `decide_idea_dedup` allocates a fresh per-call temp dir, resolves the recipe
  path (hot-reload order: `~/.simard/...` then the repo asset), renders each
  `IdeaDedupCtx` field through `sanitize_context_var`, and spawns
  `recipe-runner-rs` with `-c` variables — the untrusted DATA (`candidate_idea`,
  `candidate_rationale`, the sanitized `existing_shortlist` block) **plus** the
  tool-wiring vars `record_path`, `simard_bin` (`current_exe()`), `goal_id`, and
  `cycle_number`. The agent **records** its verdict by calling the
  [`simard ooda record-idea-dedup`](ooda-record-idea-dedup-consolidation-cli.md)
  tool; the agent's stdout is ignored.
- The shim reads the result with `read_verified_idea_dedup(record_path,
  "creative-idea-dedup", REASONER_RECORD_CYCLE)`, which returns
  `Ok(IdeaDedupDecision)` only when the typed record exists, pins the schema,
  matches goal/cycle, and re-validates through the shared
  `IdeaDedupDecision::from_choice_fields` chokepoint (non-empty rationale;
  `target_node_id` required on `enhance_existing`, rejected otherwise). Any other
  outcome (absent/malformed/wrong-schema/unknown-choice/missing-target/mismatch)
  is an `Err` → the shim returns `Err(AdapterInvocationFailed)` → the seam **fails
  closed** (the candidate is dropped this cycle). **No stdout scraping.**

See the [record tool reference](ooda-record-idea-dedup-consolidation-cli.md) for
the on-disk record shape and the R1–R7 read matrix, and the
[recipe & prompt schema](creative-idea-dedup-recipe.md) for the tool-call the
agent makes.

## Consolidation entrypoint

The one-time cleanup of the pre-existing duplicates is a separate,
operator-invoked entrypoint (not the daily tick). It reuses the same gate over
the *existing* pool:

```rust
// src/creative_ideas/dedup_gate.rs
pub struct ConsolidationReport {
    pub clusters: usize,
    pub canonical: usize,
    pub rejected: usize,     // redundant ideas transitioned New -> Rejected
    pub dry_run: bool,
}

/// Cluster the existing pool by semantic duplication (via the consolidation
/// recipe), enhance each cluster's canonical idea, and transition the redundant
/// ideas to `Rejected`. Dry-run first: with `apply == false` it computes and
/// reports the plan and writes nothing.
pub fn consolidate_existing(
    store: &dyn CreativeIdeaStore,
    brain: &dyn OodaBrain,
    apply: bool,
) -> SimardResult<ConsolidationReport>;
```

Mechanics: load the pool via `store.list(u32::MAX)`; the
[consolidation recipe](creative-idea-dedup-recipe.md#consolidation-recipe)'s
agent writes its cluster list to a `clusters_path` file and **records** it by
calling the
[`simard ooda record-idea-consolidation`](ooda-record-idea-dedup-consolidation-cli.md#simard-ooda-record-idea-consolidation)
tool; `decide_idea_consolidation` reads the typed `IdeaConsolidationRecord`
fail-closed via `read_verified_idea_consolidation`, returning
`Ok(Vec<IdeaCluster>)` — each `{ canonical_id, redundant_ids, merged_rationale,
evidence }` re-validated through the shared `IdeaCluster::sanitized` chokepoint. A
present-but-empty list is a valid `Ok(vec![])` ("nothing to consolidate");
absent/malformed/mismatched is an `Err` (the applier writes nothing). The applier
`apply_enhance`s the canonical and, for each redundant id,
`try_transition(New → Rejected)` with rationale `"merged into <canonical>"`.
`New → Rejected` is a valid edge; ideas that cannot transition (already terminal)
are skipped. **No hard deletes.** Re-running after apply is idempotent
(`Rejected` ideas can no longer transition, so a second pass finds no collapsible
members). Surfaced to operators as a CLI trigger —
`simard creative-ideas consolidate [--apply]` — see the
[how-to](../howto/configure-creative-ideas-semantic-dedup.md#consolidate-the-existing-duplicate-pool).

## Configuration

| Variable | Default | Effect |
| --- | --- | --- |
| `SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP` | (unset ⇒ ON) | Kill switch. Only the exact value `off` (case-insensitive) disables the **agentic** layer and reverts to deterministic Jaccard. Read once at daemon start. See [kill-switch](#kill-switch). |
| `SIMARD_CREATIVE_IDEAS_DEDUP_SHORTLIST_K` | `12` | Stage-1 coarse-shortlist size — how many nearest existing ideas the reasoner sees per candidate. Clamped to `1..=64`; out-of-range/unparseable falls back to the default with a `WARN`. |

The deterministic backstop reuses `DEFAULT_DEDUP_THRESHOLD` (`0.6`) from
`dedup.rs`. All values are consts exposed for tuning against the metric stream.

## Observability

Per tick, the caller emits one `[simard]`-prefixed structured tracing summary
(target `creative_ideas`):

```
[simard] creative_ideas dedup: generated=10 skipped=4 enhanced=3 created=3
```

with structured fields `generated`, `skipped`, `enhanced`, `created` (plus
`dedup_errors` for fail-closed drops). There are no stray `println!` /
`eprintln!` — all output is `[simard]`-prefixed / `tracing`. A per-candidate
`creative_idea_dedup_decision` metric line and a versioned `CreativeIdeaDedup`
judgment record (mirroring the `ResourceAdmission` record) are a possible future
addition; the shipped rail keeps observability to the per-tick counts to stay
minimal.

## Kill-switch

The **semantic (agentic)** layer is disabled at daemon boot by
`SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP=off` (only the exact value `off`,
case-insensitive; any other value keeps it ON — read once at startup). When off,
`plan_candidate` is called with `enabled == false` and the brain is **not**
consulted: each candidate is judged by the deterministic Jaccard backstop
(`Skip`-or-`Create`). Deduplication is never disabled — only the reasoning and
the `enhance` capability. This is the same secure-default discipline as the
[resource-admission kill-switch](resource-admission-api.md#kill-switch).
See the [operations runbook](../operations/creative-ideas-semantic-dedup-kill-switch.md).

## Module layout

| Path | Role |
| --- | --- |
| `src/creative_ideas/dedup_gate.rs` | **New brick.** Public studs: `plan_candidate`, `PlannedAction`, `apply_enhance`, `consolidate_existing`, `semantic_dedup_enabled`, `shortlist_k_from_env`. Depends only on `dedup`'s word-set similarity, the `OodaBrain` trait, and the `CreativeIdeaStore` trait. |
| `src/creative_ideas/dedup.rs` | **Reused.** Its `jaccard` scorer (exposed `pub(crate)` as `similarity`) becomes the Stage-1 ranker + the backstop; `is_near_duplicate` / `reject_duplicates` retained. |
| `src/creative_ideas/mod.rs` | `pub mod dedup_gate;` |
| `src/ooda_brain/mod.rs` | `+ IdeaDedupCtx`, `ExistingIdeaView`, `IdeaDedupDecision`, `IdeaConsolidationCtx`, `IdeaCluster`, `OodaBrain::decide_idea_dedup` + `decide_idea_consolidation` (defaulted). |
| `src/ooda_brain/recipe_brain.rs` | `+ RecipeBrain::decide_idea_dedup` / `decide_idea_consolidation` (temp-dir → run-recipe → `read_verified_*`), consts. The former `parse_idea_dedup_decision` / `parse_idea_consolidation` scrapers are **deleted** (#4719 Group C). |
| `src/cognitive_threads/threads/creative_ideas.rs` | Thread gains the brain seam (`with_pipeline_and_brain`); `run_tick` rewired; `GenerationReport` counters. |
| `src/operator_cli/creative_ideas.rs` | `simard creative-ideas consolidate [--apply]` trigger. |
| `prompt_assets/simard/creative_idea_dedup.md` | The reasoning prompt (prompt-store registered). |
| `prompt_assets/simard/recipes/creative-idea-dedup.yaml` | The per-candidate reasoning recipe. |
| `prompt_assets/simard/recipes/creative-ideas-consolidation.yaml` | The consolidation clustering recipe. |

`pipeline.rs`, the memory-lib model types, and the `Create` path are unchanged.

## Test matrix

All tests are hermetic — a stub `OodaBrain` and an in-memory `CreativeIdeaStore`
fake; no network, no recipe runner in the seam tests.

| Test | Asserts |
| --- | --- |
| `plan_candidate` — each variant | Stub brain returning `CreateNew` / `Skip` / `EnhanceExisting` maps to `Create` / `Skip` / `Enhance`. |
| `plan_candidate` — kill-switch off | Brain **not** called; Jaccard-only `Skip`-or-`Create`. |
| `plan_candidate` — brain `Err` | Fail-closed **drop** (`FailClosed`); never a blind `Create`, never `Enhance`, never a silent duplicate. |
| `plan_candidate` — empty pool | `Create` without consulting the brain. |
| `plan_candidate` — bad ENHANCE target | Fail-closed **drop** (`FailClosed`); no wrong-node write. |
| `apply_enhance` | `update` appended a revision at the **same** `idea_id`, status preserved, rationale + links merged, **pool count unchanged (0 new nodes)**; `dry_run` writes nothing; missing target ⇒ `Ok(false)`. |
| `read_verified_idea_dedup` (table) | Valid variants read back; unknown `choice` → `Err`; missing/rejected `target_node_id` on enhance → `Err`; wrong schema / goal / cycle → `Err`; empty rationale → `Err`. (See the [record tool reference](ooda-record-idea-dedup-consolidation-cli.md#regression-tests).) |
| `read_verified_idea_consolidation` (table) | Reads the `clusters` array re-sanitized via `IdeaCluster::sanitized`; drops headless clusters; present-empty is `Ok(vec![])`; absent/malformed/mismatched → `Err`. |
| `run_tick` integration | Report `persisted` / `skipped` / `enhanced` / `dedup_errors` counts; a stubbed SKIP drops (0 nodes); a stubbed ENHANCE updates the matched idea and creates **0** nodes; a stubbed CREATE persists; a brain error is fail-closed (candidate not persisted, no duplicate). |
| `consolidate_existing` | Dry-run produces a plan and writes nothing; `apply` strengthens canonicals and transitions redundant ideas `New → Rejected`; second run is idempotent. |
| Recipe/prompt content-pin | The recipe + prompt assets expose the documented `-c` vars, **call the `simard ooda record-idea-*` tool** with `--record-path {{record_path}} --goal-id {{goal_id}} --cycle-number {{cycle_number}}` (consolidation also `--clusters-path {{clusters_path}}`), and document `Output: NONE scraped from stdout` (`tests/creative_ideas_dedup_assets.rs`); full validation by the recipe runner in CI. |
