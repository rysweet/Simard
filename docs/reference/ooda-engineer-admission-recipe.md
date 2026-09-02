---
title: "Reference: OODA engineer-admission recipe and prompt schema"
description: >
  The ooda-engineer-admission.yaml recipe and its prompt schema — the single
  source of truth for the overlap-aware admission reasoning. Context variables,
  the record-tool call (simard ooda record-admission with {admit, defer,
  serialize_after}), the variant-owned-field contract for defer/serialize_after,
  few-shot examples anchored on the real collisions (goals_status.rs, PRs
  #2698/#2696, the Adapter-rename broken-main), hot-reload resolution order, the
  "daemon fails OPEN, does NOT default on your behalf" contract, versioning, and
  tests.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/dependency-overlap-aware-scheduling.md
  - ./engineer-admission-api.md
  - ./ooda-record-admission-cli.md
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
version: "2.0.0"
author: "Simard"
tags: ["simard", "ooda", "act", "engineer-admission", "scheduling"]
# Output: NONE scraped from stdout. The agent RECORDS its verdict by calling
# `simard ooda record-admission`; RecipeBrain reads the typed record.

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
      hold work back when there is a clear file-level collision. RECORD your
      verdict by calling the tool (below).

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

      Pick exactly one `--choice`. The daemon maps each to a concrete effect:

      - `admit` — Independent work (or trivial, acceptable overlap). Spawn now.
        Default when in doubt.
      - `defer` — A live engineer holds files this goal needs; starting now would
        collide at merge. Skip THIS cycle (Simard retries next cycle). Provide
        `--blocked-by` (the goal ids in the way).
      - `serialize_after` — Overlap exists but the candidate can proceed if it
        rebases onto the named engineer's work first. Spawn now WITH a rebase
        hint. Provide `--after-goal-id` and `--overlap-files`.

      ## HOW TO RECORD — call the typed admission tool

      Record your verdict by calling the tool EXACTLY ONCE. Do NOT print JSON:
      stdout is ignored.

      ```bash
      "{{simard_bin}}" ooda record-admission \
        --choice <admit|defer|serialize_after> \
        --rationale "<short reason naming the overlapping files/goals>" \
        [--blocked-by <csv-goal-ids>] [--retry-after-secs <int>] \
        [--after-goal-id <goal-id>] [--overlap-files <csv-paths>] \
        --record-path "{{record_path}}" \
        --goal-id "{{goal_id}}" \
        --cycle-number "{{cycle_number}}"
      ```

      Supply only the flags your `--choice` owns (`--blocked-by` /
      `--retry-after-secs` for `defer`; `--after-goal-id` / `--overlap-files` for
      `serialize_after`) — a non-owned flag is rejected. A genuine "these are
      independent, parallelize" answer is a REAL decision: call with
      `--choice admit` explicitly. If you never call the tool the daemon does NOT
      default on your behalf — the record is absent, the read is an `Err`, and the
      daemon FAILS OPEN to `admit` (scheduling is an optimization, never a stall
      gate), auditing the fallback. A CERTAIN collision (the candidate's exact
      target paths are already held by one live engineer) is blocked by a Rust
      rail regardless of what you record — you cannot override it.

      ## EXAMPLES

      ```bash
      "{{simard_bin}}" ooda record-admission --choice defer \
        --blocked-by render-goals-status \
        --rationale "live engineer render-goals-status is rewriting src/operator_commands_ooda/goals_status.rs, the only file this goal edits; parallel PRs would collide (cf. #2698/#2696)" \
        --record-path "{{record_path}}" --goal-id "{{goal_id}}" --cycle-number "{{cycle_number}}"
      ```

      ```bash
      "{{simard_bin}}" ooda record-admission --choice serialize_after \
        --after-goal-id rename-adapter-symbol \
        --overlap-files src/agent_supervisor/adapter.rs \
        --rationale "candidate edits call sites that engineer rename-adapter-symbol is moving; rebase after it to avoid a broken-main union" \
        --record-path "{{record_path}}" --goal-id "{{goal_id}}" --cycle-number "{{cycle_number}}"
      ```

      ```bash
      "{{simard_bin}}" ooda record-admission --choice admit \
        --rationale "candidate scope is docs/ only; no live engineer touches docs — independent, parallelize" \
        --record-path "{{record_path}}" --goal-id "{{goal_id}}" --cycle-number "{{cycle_number}}"
      ```
```

The recipe is a single `agent` step. `recipe-runner-rs` renders the prompt and
invokes the agent, which calls `simard ooda record-admission`. That tool writes a
typed [`AdmissionDecisionRecord`](ooda-record-admission-cli.md#admissiondecisionrecord)
via the shared `EngineerAdmissionDecision::from_choice_fields` chokepoint;
`RecipeBrain::decide_engineer_admission` then reads it with `read_verified_admission`.
The agent's **stdout is ignored** — see
[Reference: `simard ooda record-admission`](ooda-record-admission-cli.md).

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
| `{{record_path}}` | string (absolute path) | per-cycle temp dir + `admission.json`; where the tool writes the typed record and `read_verified_admission` reads it |
| `{{simard_bin}}` | string (absolute path) | `std::env::current_exe()` — the binary the tool call invokes |
| `{{goal_id}}` | string | `ctx.candidate.id` — embedded in the record, re-verified on read (R6) |
| `{{cycle_number}}` | u32 | `REASONER_RECORD_CYCLE = 0` sentinel — embedded in the record, re-verified on read (R7) |

The overlap-context vars (`candidate_*`, `live_engineers`, `repo_root`) are the
untrusted, model-facing inputs and pass through `sanitize_context_var`. The
tool-plumbing vars (`record_path`, `simard_bin`, `goal_id`, `cycle_number`) are
daemon-controlled identity/path values, not model text.

Special rendering: an empty `predicted_scope` or empty `live_engineers` renders
to an explicit `"<unknown>"` / `"<none>"` sentinel so the model can distinguish
"no overlap" from a rendering bug. Both sentinels correspond to fail-open `admit`
paths that the Rust seam would take anyway.

## Choice → Rust mapping

The agent calls `simard ooda record-admission --choice <c>` with the flags that
variant owns; the shared `EngineerAdmissionDecision::from_choice_fields`
chokepoint (called by both the CLI writer and `read_verified_admission`)
constructs the `choice`-tagged enum directly. There is no prose scraper and no
`decision`→variant mapper:

| `--choice` | Constructs | Variant-owned flags |
| --- | --- | --- |
| `admit` | `EngineerAdmissionDecision::Admit` | _(none)_ |
| `defer` | `EngineerAdmissionDecision::Defer` | `--blocked-by` (CSV → `Vec<String>`), `--retry-after-secs` (→ `Option<u64>`) |
| `serialize_after` | `EngineerAdmissionDecision::SerializeAfter` | `--after-goal-id` (→ `String`), `--overlap-files` (CSV → `Vec<String>`) |

> **Variant-owned fields are enforced, not defaulted.** `from_choice_fields`
> applies the [field-ownership matrix](ooda-record-admission-cli.md#field-ownership-matrix):
> a flag a variant does not own is a hard **rejection** (no file written), not a
> silently-dropped default. A `defer` with an empty `--blocked-by` still defers
> (the rationale carries the reason); a `serialize_after` requires a non-empty
> `--after-goal-id` (there is no engineer to rebase after otherwise). Because the
> writer and the reader share the one chokepoint, the load-bearing `blocked_by` /
> `after_goal_id` / `overlap_files` fields can never drift between write and read.

## Missing-record behavior (fail-open)

Unlike the [outcome verifier](outcome-verification-api.md) (which fails **closed**
on a missing/invalid record), the admission brain fails **open**:

- If the agent never calls the tool, or the record is malformed / mismatched
  (R1–R7), `read_verified_admission` returns `Err` → the daemon returns `Admit`
  via `engineer_admission_fallback`, with a loud `tracing::warn` and a
  `BrainJudgmentRecord` (`fallback = true`).
- This is never silent: the fallback is audited exactly like a normal decision,
  so an operator can see the scheduler degraded to collision-blind admission and
  fix the recipe.

The **only** non-admit path that survives a missing/invalid record is the Rust
exact-path rail (a certain collision) — the prompt cannot suppress it, and a
missing record cannot bypass it.

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

1. Add the variant to `EngineerAdmissionDecision` in `src/ooda_brain/mod.rs`, its
   `from_choice_fields` arm (including the field-ownership rule for any new
   variant-owned flags).
2. Accept the new keyword in `EngineerAdmissionDecision::from_choice_fields`; the
   CLI writer (`dispatch_record_admission`) and `read_verified_admission` pick it
   up through the shared chokepoint automatically.
3. Add the `choice` (and any owned flags) to the `OPTIONS` / `HOW TO RECORD`
   sections in the recipe YAML.
4. Add an example to the `EXAMPLES` section.
5. Handle the variant in the `dispatch_spawn_engineer` apply match.
6. Add a round-trip + field-ownership + fail-closed `read_verified_admission`
   test covering the new decision and its seam effect.
7. Update the variant table in
   [engineer-admission API reference](engineer-admission-api.md#engineeradmissiondecision)
   and the [record-admission CLI reference](ooda-record-admission-cli.md).

Cosmetic edits (rationale guidance, examples, ROLE/overlap-heuristic phrasing)
are safe to ship alone — and take effect without a rebuild.

## Tests

- **Record round-trip** — each of `admit` / `defer` / `serialize_after` written
  by `record-admission` reads back through `read_verified_admission` bit-for-bit
  incl. its owned fields; every variant rejects a non-owned flag; an
  absent/malformed/mismatched record → `Err` → `Admit` fallback (fail-open). See
  [the record-admission CLI regression tests](ooda-record-admission-cli.md#regression-tests).
- **Recipe resolution & content** — a test asserts the in-tree
  `ooda-engineer-admission.yaml` resolves via `resolve_recipe_path`, and a content
  test (mirroring the existing checks in `tests/typed_ooda_recipe_assets.rs`)
  asserts the recipe text **calls `simard ooda record-admission`**, documents
  `Output: NONE scraped from stdout`, and carries no JSON output envelope.
- **Sanitization** — a goal title / path containing newlines, ANSI, or injection
  markers is neutralized before it reaches the recipe.

## See also

- [Engineer-admission API reference](engineer-admission-api.md) — the typed surface this recipe maps to.
- [`simard ooda record-admission` (typed admission tool)](ooda-record-admission-cli.md) — the tool this recipe calls and the fail-closed record reader.
- [Dependency/overlap-aware engineer scheduling (concept)](../concepts/dependency-overlap-aware-scheduling.md) — the rationale.
- [OODA engineer-lifecycle recipe](ooda-engineer-lifecycle-recipe.md) — the sibling Act-phase recipe this mirrors.
- [Recipe brain API](recipe-brain-api.md) — the `RecipeBrain` invocation path.
- [Recipe context variable sanitization](recipe-context-var-sanitization.md) — the `sanitize_context_var` boundary.
- [How to edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — the hot-reload editing workflow.
- [How to diagnose a deferred engineer spawn](../howto/diagnose-a-deferred-engineer-spawn.md) — the operator runbook.
