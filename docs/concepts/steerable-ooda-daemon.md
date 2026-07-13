---
title: "Concept: prompt-owned OODA semantics and thin Rust rails"
description: Intended behavior for the steerable OODA daemon: judgment lives in prompt and recipe assets while Rust provides trusted loading, orchestration, validation, subprocess execution, and explicit failure surfacing.
last_updated: 2026-07-13
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../howto/run-ooda-daemon.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../reference/simard-cli.md
  - ../reference/ooda-coverage-parallelism-ceiling.md
  - ../../prompt_assets/simard/ooda_decide.md
  - ../../prompt_assets/simard/goal_session_objective.md
  - ../../prompt_assets/simard/recipes/ooda-decide.yaml
  - ../../prompt_assets/simard/recipes/ooda-orient.yaml
  - ../../prompt_assets/simard/recipes/ooda-no-progress-why.yaml
---

# [PLANNED - Implementation Pending] Concept: prompt-owned OODA semantics and thin Rust rails

This document describes the intended feature behavior. The OODA daemon is
steerable because its judgment lives in assets operators can read and revise:
prompts under `prompt_assets/simard/` and recipes under
`prompt_assets/simard/recipes/`. Rust owns the rails around those assets. It
loads trusted prompts, builds structured inputs, invokes recipes, validates the
small response contracts, records outcomes, and surfaces failures.

Rust does **not** own OODA policy. It must not grow hard-coded decision trees,
keyword taxonomies, semantic scoring rules, or code-owned "brain" judgment.

## Why this split exists

OODA behavior changes often: which goal to advance, when to fan out work, when a
PR is ready to merge, when a goal is looping, and when a no-action cycle is
honest. Those are semantic judgments. Encoding them as Rust parsers makes the
daemon brittle and expensive to steer.

The daemon therefore treats prompts and recipes as the policy layer:

| Layer | Owns | Examples |
| --- | --- | --- |
| Prompt and recipe assets | Semantics, judgment, decision policy, examples, output instructions | `ooda_decide.md`, `goal_session_objective.md`, `recipes/ooda-decide.yaml`, `recipes/ooda-orient.yaml`, `recipes/ooda-no-progress-why.yaml` |
| Rust rails | Trusted asset resolution, context construction, subprocess boundaries, response-contract validation, state mutation, logs, errors | `src/ooda_brain/recipe_brain.rs`, `src/operator_commands_ooda/daemon/brains.rs`, `src/ooda_actions/goal_session/*` |

This makes normal behavior changes prompt edits instead of rebuilds, while still
giving operators deterministic safety at the IO and state boundaries.

## The daemon cycle

```text
Observe
  |
  v
Orient
  prompt/recipe judges priority and demotion semantics
  Rust validates numeric urgency and records parse errors loudly
  |
  v
Decide
  prompt/recipe selects one action kind
  Rust accepts only known action variants and rejects ambiguous output
  |
  v
Act / advance goal
  goal-session prompt decides whether to spawn an engineer or take no action
  Rust enforces the explicit response contract and performs side effects
```

The recipes can interpret meaning. The rails can only decide whether the recipe
returned a valid contract.

## What prompts and recipes own

### OODA Orient

`prompt_assets/simard/recipes/ooda-orient.yaml` owns the semantic judgment for
failure-aware priority adjustment. It receives the current goal id, base urgency,
base reason, failure count, and any repair prompt text. It decides the adjusted
urgency and rationale.

Rust owns only the bounds:

- input fields are rendered as structured recipe context, not shell text;
- the returned urgency must be numeric and within the documented range;
- missing, malformed, or out-of-range output is a brain-output failure, not a
  quiet deterministic default.

### OODA Decide

`prompt_assets/simard/ooda_decide.md` and
`prompt_assets/simard/recipes/ooda-decide.yaml` own action-kind routing:

- ordinary goal slugs usually route to `advance_goal`;
- reserved synthetic ids route to their dedicated action kinds;
- loop signals are named in the rationale so the goal-session brain can change
  strategy;
- merge and self-update policy remain prompt-owned gate descriptions.

Rust accepts only the known action variants. It may keep a compatibility reader
for older first-token output, but the canonical contract is the recipe-owned
structured decision envelope:

```json
{"decision": "advance_goal", "rationale": "ordinary goal slug, default routing"}
```

If no valid decision can be recovered after bounded repair, the daemon records a
visible parse error. It must not silently pretend the brain chose a default.

### Goal-session advance

`prompt_assets/simard/goal_session_objective.md` owns the judgment for one
active goal in one cycle:

- whether to spawn an engineer;
- whether to take no action because work is already in flight or externally
  blocked;
- what progress percentage is honest;
- whether the goal should be decomposed, finished, merged, closed, or retired;
- how to avoid repeated non-progress loops.

Rust owns only the response contract:

```text
ACTION: SPAWN_ENGINEER
TASK:
Drive PR #4042 to merge-readiness: fix confirmed quality-audit findings,
update evidence, wait for green checks, then merge through `simard merge-pr`.
PROGRESS: 75
```

or:

```text
NO ACTION
REASON: engineer simard-4042-finalizer is already repairing the PR branch.
PROGRESS: 80
```

The response is still prose-first. The `TASK:` body is handed to the engineer as
natural language. The markers are small rails so the daemon can reject ambiguous
or conflicting output instead of guessing. Until the strict contract parser
lands, the current compatibility path may still dispatch non-empty free-form
prose as an engineer task; that compatibility behavior is not the final
contract.

### No-progress explanation

`prompt_assets/simard/recipes/ooda-no-progress-why.yaml` owns the human-readable
explanation for a no-progress escalation. The deterministic breaker decides that
the goal has produced no progress; the recipe explains why in operator language.

The recipe does not grant itself authority to suppress the breaker. It can enrich
the evidence narrative; it cannot make a looping goal healthy by narration.

## What Rust owns

Rust is deliberately boring. It owns:

1. **Trusted asset resolution.** Prompt and recipe paths are resolved from the
   packaged Simard install or the repository root. Operator input is never used
   as an arbitrary prompt path.
2. **Context construction.** Goal ids, reasons, failure counts, urgency scores,
   progress state, PR references, and WIP summaries are assembled from known
   daemon state into structured recipe context.
3. **Subprocess boundaries.** Recipe invocations use explicit executable and
   argument vectors, including `-c key=value` context arguments. They do not use
   shell-interpolated command strings.
4. **Output capture.** The recipe-runner JSON envelope is decoded and the final
   step output is extracted. Status, stdout, and stderr are preserved enough to
   diagnose failures without turning logs into a secret sink.
5. **Contract enforcement.** The daemon accepts only documented markers and
   action variants. Missing, unknown, duplicate, or conflicting markers are
   invalid.
6. **State mutation.** Rust updates goal progress, records no-action outcomes,
   spawns engineers, files tracking issues through existing rails, and writes
   cycle journals.
7. **Loud failures.** Invalid recipe output, subprocess failure, missing assets,
   missing tools, and malformed response contracts surface as explicit failures.
   They are not converted into success-shaped no-ops.

## Planned goal-session module boundary

The goal-session action should be split by responsibility before this planned
contract is considered implemented:

| Planned module | Responsibility |
| --- | --- |
| `src/ooda_actions/goal_session/mod.rs` | Public module boundary and narrow exports for input, advance, and outcome rails. |
| `src/ooda_actions/goal_session/input.rs` | Build deterministic prompt and recipe input from known goal, board, WIP, and state data. No semantic scoring. |
| `src/ooda_actions/goal_session/advance.rs` | Run one goal-session turn, call the brain, and route only from explicit output contracts. |
| `src/ooda_actions/goal_session/outcome.rs` | Validate `ACTION: SPAWN_ENGINEER`, `NO ACTION`, `REASON:`, `TASK:`, and `PROGRESS: NN`; reject ambiguous or conflicting output. |

This boundary is the guardrail against policy creep. If a change needs to decide
what work matters, it belongs in a prompt or recipe. If it only validates a
marker or moves bytes between trusted components, it belongs in Rust.

## Response contracts

### Decide contract

Canonical:

```json
{"decision": "advance_goal", "rationale": "ordinary goal slug, default routing"}
```

`decision` must be one known variant:

- `advance_goal`
- `consolidate_memory`
- `run_improvement`
- `poll_developer_activity`
- `extract_ideas`
- `safe_update`
- `research_query`
- `run_gym_eval`
- `build_skill`
- `launch_session`

Unknown variants are invalid. Compatibility readers may accept older first-token
output only as an explicit migration rail; new prompt assets should emit the JSON
envelope.

### Orient contract

Canonical:

```json
{"adjusted_urgency": 0.6, "confidence": 0.82, "rationale": "one recent failure; light demotion"}
```

`adjusted_urgency` must be a number in the allowed range. It must not inflate a
goal above the base urgency passed to the recipe.

### Goal-session contract

This is the target contract for the feature. It is stricter than the current
compatibility parser and requires matching prompt-asset changes before the
planned marker is removed.

Spawn an engineer:

```text
ACTION: SPAWN_ENGINEER
TASK:
Check out PR #4042, fix confirmed quality-audit findings only, update the PR
body with evidence, and merge through `simard merge-pr` after all gates pass.
PROGRESS: 70
```

Take no action:

```text
NO ACTION
REASON: PR #4042 is waiting on required checks; spawning another engineer would duplicate work.
PROGRESS: 85
```

Rules:

- `ACTION: SPAWN_ENGINEER` and `NO ACTION` are mutually exclusive.
- `TASK:` is required for `ACTION: SPAWN_ENGINEER`.
- `REASON:` is required for `NO ACTION`.
- `PROGRESS: NN` is optional and must be a single integer in `0..=100`; out-of-range values are invalid and must not be clamped.
- Duplicate progress markers with different values are invalid.
- Empty output, prose with no action marker, unknown action markers, and
  conflicting markers are invalid.

## Configuration

| Setting | Default | Purpose |
| --- | --- | --- |
| `SIMARD_HOME` | `$HOME/.simard` | Install root for the binary, prompt assets, recipe assets, logs, and systemd working directory. |
| `SIMARD_LLM_PROVIDER` | config file value | Overrides the provider used by prompt-driven brains. |
| `SIMARD_ENGINEER_AGENT` | `copilot` | Selects the subordinate engineer agent. Valid values are `copilot` and `rustyclawd`. |
| `SIMARD_OODA_MAX_CONCURRENT` | `24` | Preferred per-cycle goal coverage ceiling, range `1..=64`. |
| `SIMARD_MAX_CONCURRENT_ACTIONS` | `24` | Legacy fallback used only when `SIMARD_OODA_MAX_CONCURRENT` is unset. |

The generated user systemd units include a deterministic `PATH` that contains
`$SIMARD_HOME/bin`, `$HOME/.local/bin`, `$HOME/.cargo/bin`, and standard system
paths. See [How to run the OODA daemon](../howto/run-ooda-daemon.md).

## Healthy behavior

A healthy daemon reports recipe-backed brains in `ooda.log`, advances distinct
goals up to the resource ceiling, records no-action outcomes when work is
already in flight, and fails visibly when a prompt or recipe returns invalid
output.

Example health signals:

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
journalctl --user -u simard-ooda.service -n 100 --no-pager
grep "RecipeBrain" "$SIMARD_HOME/ooda.log"
```

Expected log lines name recipe-backed phases such as:

```text
[simard] OODA daemon: decide_brain = RecipeBrain (recipe-runner-rs backed, decide)
[simard] OODA daemon: orient_brain = RecipeBrain (recipe-runner-rs backed, orient)
```

`DEGRADED` fallback lines are not healthy. They mean a provider, recipe asset,
or recipe-runner dependency is missing and the daemon cannot honestly claim the
agentic OODA architecture is active.

## Anti-patterns

Do not implement OODA judgment by adding:

- Rust keyword scanners for semantic phrases such as "merge-ready", "stuck",
  "blocked", "safe", or "done";
- deterministic routing trees that duplicate `ooda_decide.md`;
- code-owned scoring for whether a goal is important or looping;
- silent defaults after malformed recipe output;
- arbitrary prompt path configuration supplied by operator input;
- shell-joined recipe commands.

The right fix for loose or ambiguous brain output is a clearer prompt/recipe
contract plus stricter contract validation, not a larger parser.

## See also

- [How to run the OODA daemon](../howto/run-ooda-daemon.md)
- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md)
- [Simard CLI reference](../reference/simard-cli.md)
- [OODA coverage parallelism ceiling](../reference/ooda-coverage-parallelism-ceiling.md)
