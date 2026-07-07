---
title: "Reference: OODA engineer-admission recipe and prompt schema"
description: >
  The ooda-engineer-admission.yaml recipe and its prompt schema — the single
  source of truth for the overlap-aware admission reasoning. Context variables,
  the JSON decision envelope ({admit, defer, serialize_after}), the extra-field
  contract for defer/serialize_after, few-shot examples anchored on the real
  collisions (goals_status.rs, PRs #2698/#2696, the Adapter-rename broken-main),
  hot-reload resolution order, the "daemon fails OPEN, does NOT default on your
  behalf" contract, versioning, and tests.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/dependency-overlap-aware-scheduling.md
  - ./engineer-admission-api.md
  - ./ooda-engineer-lifecycle-recipe.md
  - ./recipe-brain-api.md
  - ./recipe-context-var-sanitization.md
  - ../howto/edit-the-ooda-brain-prompt.md
  - ../howto/diagnose-a-deferred-engineer-spawn.md
  - ../../prompt_assets/simard/recipes/ooda-engineer-admission.yaml
---

# Reference: OODA engineer-admission recipe and prompt schema

> **Status: implemented.** This page describes the shipped recipe
> in present tense. The recipe lives at
> [`prompt_assets/simard/recipes/ooda-engineer-admission.yaml`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/recipes/ooda-engineer-admission.yaml)
> and is invoked by `RecipeBrain::decide_engineer_admission` (adapter tag
> `recipe-engineer-admission-brain`). The typed surface it maps to is documented
> in the [engineer-admission API reference](engineer-admission-api.md).

Recipe: `prompt_assets/simard/recipes/ooda-engineer-admission.yaml`
Shim: `RecipeBrain::decide_engineer_admission` in
`src/ooda_brain/recipe_brain.rs`

This is the single source of truth for the **overlap-aware admission decision**
made at the spawn/admission point in `dispatch_spawn_engineer`. The admission
brain runs as a **recipe step** via `recipe-runner-rs`, mirroring
[`ooda-engineer-lifecycle.yaml`](ooda-engineer-lifecycle-recipe.md),
`ooda-goal-outcome-verification.yaml`, and `ooda-decide.yaml`.

## Recipe layout

```yaml
name: "ooda-engineer-admission"
description: "OODA spawn-admission brain — dependency/overlap-aware engineer scheduling (#2690)"
version: "1.0.0"
author: "Simard"
tags: ["simard", "ooda", "act", "engineer-admission", "scheduling"]

context: {}

steps:
  - id: "engineer-admission-decision"
    type: "agent"
    agent: "default"
    prompt: |
      # OODA Brain — Engineer Admission (Overlap-Aware Scheduling)

      ## ROLE

      You are the brain of Simard's OODA daemon. The Act phase is about to spawn
      a NEW engineer for a candidate goal. Simard already guarantees at most one
      engineer PER GOAL; your job is the DIFFERENT-goals case: decide whether the
      candidate's likely FILE FOOTPRINT overlaps any in-flight engineer's, and if
      so whether to PARALLELIZE, SERIALIZE, or DEFER. Bias toward `admit`: only
      hold work back when there is a clear file-level collision. Output a JSON
      decision envelope (below).

      ## CONTEXT

      - candidate_goal_id: {{candidate_goal_id}}
      - candidate_goal_title: {{candidate_goal_title}}
      - candidate_predicted_scope:
      ```
      {{candidate_predicted_scope}}
      ```
      - repo_root: {{repo_root}}
      - live engineers (other goals in flight):
      ```
      {{live_engineers}}
      ```

      ## WHAT OVERLAP MEANS

      Two engineers COLLIDE when they edit the same files. `predicted_scope` is
      the candidate's best-effort target paths; each live engineer lists its
      `changed_files` and the `overlap_with_candidate` intersection already
      computed for you. `depended_on: true` means the candidate explicitly builds
      on that engineer's branch/PR. Weigh:
      - Large or exact overlap on hot shared files (e.g.
        `src/operator_commands_ooda/goals_status.rs`) → collision is likely.
      - A rename/move an engineer is doing that the candidate also touches (the
        broken-main Adapter-rename class) → collision is likely even if line
        ranges differ.
      - Empty scope / empty overlap → parallelize; you cannot know a collision.

      ## OPTIONS

      Pick exactly one `choice`. The daemon maps each to a concrete effect:

      - `admit` — Independent work (or trivial, acceptable overlap). Spawn now.
        Default when in doubt.
      - `defer` — A live engineer holds files this goal needs; starting now would
        collide at merge. Skip THIS cycle (Simard retries next cycle). Provide
        `blocked_by` (the goal ids in the way).
      - `serialize_after` — Overlap exists but the candidate can proceed if it
        rebases onto the named engineer's work first. Spawn now WITH a rebase
        hint. Provide `after_goal_id` and `overlap_files`.

      ## OUTPUT FORMAT

      Respond with a single JSON object (a fenced ```json block is fine; the
      daemon strips any surrounding banner/prose before parsing):

      ```json
      {"decision": "<admit|defer|serialize_after>",
       "rationale": "<short reason naming the overlapping files/goals>",
       "blocked_by": ["<goal-id>", ...],
       "retry_after_secs": <int|null>,
       "after_goal_id": "<goal-id>",
       "overlap_files": ["<path>", ...]}
      ```

      Include only the extra fields relevant to your `decision` (`blocked_by`
      /`retry_after_secs` for `defer`; `after_goal_id`/`overlap_files` for
      `serialize_after`). A genuine "these are independent, parallelize" answer is
      a REAL decision: emit `admit` explicitly. If your output is unparseable the
      daemon does NOT default on your behalf — it records a `brain_parse_error`
      and FAILS OPEN to `admit` (scheduling is an optimization, never a stall
      gate), auditing the fallback. A CERTAIN collision (the candidate's exact
      target paths are already held by one live engineer) is blocked by a Rust
      rail regardless of what you say — you cannot override it.

      ## EXAMPLES

      ```json
      {"decision": "defer", "blocked_by": ["render-goals-status"], "rationale": "live engineer render-goals-status is rewriting src/operator_commands_ooda/goals_status.rs, the only file this goal edits; parallel PRs would collide (cf. #2698/#2696)"}
      ```

      ```json
      {"decision": "serialize_after", "after_goal_id": "rename-adapter-symbol", "overlap_files": ["src/agent_supervisor/adapter.rs"], "rationale": "candidate edits call sites that engineer rename-adapter-symbol is moving; rebase after it to avoid a broken-main union"}
      ```

      ```json
      {"decision": "admit", "rationale": "candidate scope is docs/ only; no live engineer touches docs — independent, parallelize"}
      ```
    output: "admission_result"
```

The recipe is a single `agent` step. `recipe-runner-rs` renders the prompt,
invokes the agent, and captures stdout. `RecipeBrain::decide_engineer_admission`
parses the JSON envelope into an
[`EngineerAdmissionDecision`](engineer-admission-api.md#engineeradmissiondecision).

## Placeholders (context variables)

recipe-runner-rs performs Handlebars `{{name}}` substitution from the context
variables passed by `RecipeBrain`. Each value is routed through
[`sanitize_context_var`](recipe-context-var-sanitization.md) first.

| Variable | Type | Source |
| --- | --- | --- |
| `{{candidate_goal_id}}` | string | `ctx.candidate.id` |
| `{{candidate_goal_title}}` | string | `ctx.candidate.title` (task text, capped 2000) |
| `{{candidate_predicted_scope}}` | string (rendered list) | `ctx.candidate.predicted_scope` — one path per line; `"<unknown>"` when empty |
| `{{repo_root}}` | string | `ctx.repo_root` |
| `{{live_engineers}}` | string (rendered blocks) | `ctx.live_engineers` — per engineer: `goal_id`, `changed_files`, `overlap_with_candidate`, `depended_on`; `"<none>"` when the set is empty |

Special rendering: an empty `predicted_scope` or empty `live_engineers` renders
to an explicit `"<unknown>"` / `"<none>"` sentinel so the model can distinguish
"no overlap" from a rendering bug. Both sentinels correspond to fail-open `admit`
paths that the Rust seam would take anyway.

## Decision envelope → Rust mapping

`RecipeBrain` extracts the JSON object (banner/prose stripped) and maps
`decision` to the `choice`-tagged enum via `admission_decision_from_variant`:

| `decision` | Maps to | Extra fields read |
| --- | --- | --- |
| `admit` | `EngineerAdmissionDecision::Admit` | _(none)_ |
| `defer` | `EngineerAdmissionDecision::Defer` | `blocked_by: Vec<String>`, `retry_after_secs: Option<u64>` |
| `serialize_after` | `EngineerAdmissionDecision::SerializeAfter` | `after_goal_id: String`, `overlap_files: Vec<String>` |

> **Extra fields are read explicitly.** The base `DecisionEnvelope` shim reads
> only `decision` + `rationale`. The admission parser extends it (or uses a
> dedicated `AdmissionEnvelope`) with `#[serde(default)]` fields for `blocked_by`,
> `retry_after_secs`, `after_goal_id`, and `overlap_files`. Missing fields default
> to empty/`None`. A `defer` with an empty `blocked_by` still defers (the
> rationale carries the reason); a `serialize_after` with an empty `after_goal_id`
> is treated as `admit` with a logged note, since there is no engineer to rebase
> after.

## Parse-failure behavior (fail-open)

Unlike the [outcome verifier](outcome-verification-api.md) (which fails **closed**
on a parse error), the admission brain fails **open**:

- A malformed / unparseable envelope → the daemon records a `brain_parse_error`
  and returns `Admit` via `engineer_admission_fallback`, with a loud
  `tracing::warn` and a `BrainJudgmentRecord` (`fallback = true`).
- This is never silent: the fallback is audited exactly like a normal decision,
  so an operator can see the scheduler degraded to collision-blind admission and
  fix the recipe.

The **only** non-admit path that survives a parse failure is the Rust exact-path
rail (a certain collision) — the prompt cannot suppress it, and a parse failure
cannot bypass it.

## Runtime loading (not compile-time)

`RecipeBrain` resolves the recipe path in this order (hot-reload first):

1. `~/.simard/prompt_assets/simard/recipes/ooda-engineer-admission.yaml`
2. `{repo_root}/prompt_assets/simard/recipes/ooda-engineer-admission.yaml`

Unlike the OODA `*.md` prompts (which keep an `include_str!` embedded fallback in
[`prompt_store.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/prompt_store.rs)),
recipe YAML is **loaded from disk only** — there is no compiled-in copy.
`RecipeBrain::new(repo_root, "ooda-engineer-admission.yaml", "recipe-engineer-admission-brain")`
(→ [`resolve_recipe_path`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/recipe_brain.rs))
returns `None` when neither path resolves (or when `recipe-runner-rs` is
unavailable); the daemon then runs a brain that inherits the **defaulted**
`decide_engineer_admission` — i.e. fail-open `Admit`, so a missing recipe degrades
to today's collision-blind spawn rather than a stall. The in-tree copy ships in the
source tree, so a normal checkout always has the asset. Prompt edits take effect on
the **next daemon cycle without a rebuild**.

## Versioning & compatibility

Adding a new admission variant (a new `EngineerAdmissionDecision` value) requires
a coordinated change:

1. Add the variant to `EngineerAdmissionDecision` in `src/ooda_brain/mod.rs`.
2. Add the mapping to `admission_decision_from_variant` in `recipe_brain.rs`.
3. Add the `choice` to the `OPTIONS` section in the recipe YAML.
4. Add an example to the `EXAMPLES` section.
5. Handle the variant in the `dispatch_spawn_engineer` apply match.
6. Add a test covering the new decision and its seam effect.
7. Update the variant table in
   [engineer-admission API reference](engineer-admission-api.md#engineeradmissiondecision).

Cosmetic edits (rationale guidance, examples, ROLE/overlap-heuristic phrasing)
are safe to ship alone — and take effect without a rebuild.

## Tests

- **Golden envelope parse** — each of `admit` / `defer` / `serialize_after` parses
  to the correct enum with its extra fields; a malformed envelope → `Admit`
  fallback (fail-open).
- **Recipe resolution & content** — a test asserts the in-tree
  `ooda-engineer-admission.yaml` resolves via `resolve_recipe_path`, and a content
  test (`include_str!`, mirroring the existing `ooda-decide.yaml` content check in
  `prompt_store_tests.rs`) asserts the recipe text carries the overlap-reasoning
  contract and the `{admit, defer, serialize_after}` schema.
- **Sanitization** — a goal title / path containing newlines, ANSI, or injection
  markers is neutralized before it reaches the recipe.

## See also

- [Engineer-admission API reference](engineer-admission-api.md) — the typed surface this recipe maps to.
- [Dependency/overlap-aware engineer scheduling (concept)](../concepts/dependency-overlap-aware-scheduling.md) — the rationale.
- [OODA engineer-lifecycle recipe](ooda-engineer-lifecycle-recipe.md) — the sibling Act-phase recipe this mirrors.
- [Recipe brain API](recipe-brain-api.md) — the `RecipeBrain` invocation path.
- [Recipe context variable sanitization](recipe-context-var-sanitization.md) — the `sanitize_context_var` boundary.
- [How to edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — the hot-reload editing workflow.
- [How to diagnose a deferred engineer spawn](../howto/diagnose-a-deferred-engineer-spawn.md) — the operator runbook.
