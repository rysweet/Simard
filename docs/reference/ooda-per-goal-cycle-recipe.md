# Reference: OODA Per-Goal-Cycle Recipe & Prompt Schema

Recipe: `prompt_assets/simard/recipes/ooda-per-goal-cycle.yaml`
Prompt source: `prompt_assets/simard/ooda_per_goal_cycle.md`
Shim: `RecipeBrain::decide_per_goal_cycle` (`src/ooda_brain/recipe_brain.rs`)

This is the single source of truth for the **per-goal, per-cycle decision** — the
reasoning step run once for **every** goal on `board.active`, every cycle
([#4453](https://github.com/rysweet/Simard/issues/4453)). It is the sibling of
[`ooda-engineer-lifecycle.yaml`](ooda-engineer-lifecycle-recipe.md).

> **Changed in #4720 (typed decision tool).** The recipe no longer prints a
> prose JSON envelope for the Rust layer to scrape with
> `recipe_output::extract_json_payload`. Its single `agent` step now calls the
> zero-privilege tool
> [`simard ooda record-decision`](ooda-record-decision-cli.md) **exactly once**,
> which validates the choice against the closed `PerGoalAction` enum and
> atomically writes a typed `PerGoalDecisionRecord`. `RecipeBrain` reads that
> record with `read_verified` — the agent's **stdout is ignored**. This removes
> the forbidden "recipe emits JSON → Rust scrapes prose → Rust acts" pattern
> from the core decision path.

For the Rust types and driver loop, see
[Reference: OODA Per-Goal-Cycle API](ooda-per-goal-cycle-api.md). For the
motivation, see
[Concept: Agentic Per-Goal, Per-Cycle Decision](../concepts/agentic-per-goal-per-cycle.md).

## Recipe Layout

```yaml
name: ooda-per-goal-cycle
description: "OODA per-goal per-cycle decision — one reasoned next-action per active goal"
version: "1.0.0"
author: "Simard"
tags: ["simard", "ooda", "per-goal-cycle"]

context: {}   # all vars supplied via -c key=value by RecipeBrain

steps:
  - id: "per-goal-cycle-decision"
    type: "agent"
    agent: "default"
    prompt: |
      # OODA Brain — Per-Goal, Per-Cycle Decision
      ## ROLE ...
      ## CONTEXT
      - goal_id: {{goal_id}}
      - goal_description: {{goal_description}}
      - goal_status: {{goal_status}}
      - cycle_number: {{cycle_number}}
      - history_summary: {{history_summary}}
      - effect_jobs_in_flight: {{effect_jobs_in_flight}}
      - open_pr_refs: {{open_pr_refs}}
      - last_outcomes: {{last_outcomes}}
      - wip_ref_count: {{wip_ref_count}}
      - worker_present: {{worker_present}}
      - standing_idle_signal: {{standing_idle_signal}}
      - stale_claim_secs: {{stale_claim_secs}}
      - effect_board_missed: {{effect_board_missed}}
      - worker log tail:
      ```
      {{worker_log_tail}}
      ```
      ## OPTIONS  ...(6 actions)...
      ## RULES    ...(investigate-before-destructive)...
      ## HOW TO RECORD ...(call `simard ooda record-decision`)...
      ## EXAMPLES ...
    output: "per_goal_cycle_result"
```

Two context vars drive the tool call: `-c record_path=<per-cycle tempdir>/decision.json`
(where `RecipeBrain` reads the typed record) and `-c simard_bin=<current_exe>`
(the absolute path the sandbox uses to resolve the tool). See
[Reference: `simard ooda record-decision`](ooda-record-decision-cli.md).

The recipe is loaded at **runtime** by recipe-runner-rs (edit the YAML / prompt,
take effect next cycle, no rebuild), resolved in this order:

1. `~/.simard/prompt_assets/simard/recipes/ooda-per-goal-cycle.yaml` (hot-reload)
2. `{repo_root}/prompt_assets/simard/recipes/ooda-per-goal-cycle.yaml` (in-tree)

## Placeholders (Context Variables)

Handlebars `{{name}}` substitution from the context variables `RecipeBrain`
passes via `-c key=value` (one per argv token, argv-only — never `sh -c`). Every
free-text value is run through `sanitize_context_var` and length/count-capped
before substitution.

| Variable | Type | Source (`PerGoalCycleCtx` field) |
|---|---|---|
| `{{goal_id}}` | string | `goal_id` |
| `{{goal_description}}` | string | `goal_description` |
| `{{goal_status}}` | string | `goal_status` |
| `{{cycle_number}}` | string (u32) | `cycle_number` |
| `{{history_summary}}` | string | `history_summary` |
| `{{effect_jobs_in_flight}}` | string (u32) | `effect_jobs_in_flight` |
| `{{open_pr_refs}}` | string (joined) | `open_pr_refs` |
| `{{last_outcomes}}` | string (joined) | `last_outcomes` |
| `{{wip_ref_count}}` | string (u32) | `wip_ref_count` |
| `{{worker_present}}` | string (bool) | `worker_present` |
| `{{worker_log_tail}}` | string | `worker_log_tail` — last ~8 KB, secrets redacted |
| `{{standing_idle_signal}}` | string (bool) | `standing_idle_signal` (demoted `classify_standing_idle`) |
| `{{stale_claim_secs}}` | string (u64 or `none`) | `stale_claim_secs: Option<u64>` (demoted claim-reaper); rendered `none` when no claim is expected or a live worker is present |
| `{{effect_board_missed}}` | string (bool) | `effect_board_missed` (demoted effect-dispatch ledger board-presence check) |

The three demoted-decider variables are surfaced as **facts for the reasoner to
weigh**, never as pre-made verdicts.

## OPTIONS (the six actions)

The prompt instructs the agent to pick exactly one `choice`:

- `continue` — work is genuinely in flight and healthy; do nothing.
- `spawn` — no live work; start the next concrete piece (research goal: seek a
  new source or design a new experiment — never sit idle). Optional `task_hint`.
- `reorient` — the goal needs a new angle; pick one and start it. **Deliberate
  redirect — this is one of only two actions that clears work-in-progress
  refs.**
- `investigate` — something looks wrong (a worker went quiet); read the logs and
  tools to find out **before** any destructive action.
- `wait` — legitimately blocked on an external event (a PR awaiting CI/merge);
  record why, do not churn.
- `complete` — the goal is done; close it.

## RULES — investigate before anything destructive

The prompt's normative rule (mirroring the operator directive): **any
worker-health concern is routed through `investigate` first.** A stale worktree,
a quiet heartbeat (`stale_claim_secs` large), a `standing_idle_signal`, or an
`effect_board_missed` is **never** grounds for reaping, resetting, or faulting a
goal on its own. The reasoner must first emit `investigate` (which inspects
logs/tools); reclaim/reset happens only as the reasoned follow-up on a later
cycle once the investigation shows it is warranted.

The prompt also carries an explicit **anti-heartbeat-reap** example so a future
prompt edit does not re-introduce threshold reaping:

```text
worker_present=false, standing_idle_signal=true, stale_claim_secs=9000

A bursty standing goal with no live worktree is NORMAL, not death. This is the
next piece of ongoing work, not a fault.
→ simard ooda record-decision --choice spawn \
    --reason "standing research goal is idle between bursts; seek a new source" \
    --task-hint "search for 2026 sources on <topic>" ...
```

## HOW TO RECORD — call the typed decision tool

The agent records its verdict by calling the zero-privilege tool
[`simard ooda record-decision`](ooda-record-decision-cli.md) **exactly once**,
using the injected `-c` context vars:

```bash
"$simard_bin" ooda record-decision \
  --choice <continue|spawn|reorient|investigate|wait|complete> \
  --reason "<short concrete reason>" \
  [--task-hint "<hint, spawn only>"] \
  --record-path "$record_path" \
  --goal-id "$goal_id" \
  --cycle-number "$cycle_number"
```

`--choice` is validated against the closed `PerGoalAction` enum (case-insensitive);
only `spawn` may carry a `task_hint`; `--reason` is mandatory and non-empty. The
tool sanitizes and bounds the free text, then atomically writes a single
`PerGoalDecisionRecord`. The agent prints **no JSON envelope** — its stdout is
ignored.

### Reading & no silent fallback ([#1711](https://github.com/rysweet/Simard/issues/1711))

`RecipeBrain::decide_per_goal_cycle` reads the typed record with
`read_verified(record_path, goal_id, cycle_number)` — it does **not** scrape the
agent's stdout. Every failure mode is **fail-CLOSED** → `Err`:

- Record absent (tool never ran / binary unresolvable / non-zero exit) → `Err`.
- Malformed JSON, wrong `schema`, unknown `choice`, missing/empty `reason` → `Err`.
- `goal_id` / `cycle_number` mismatch (stale or prior-cycle record) → `Err`.

An `Err` **surfaces as a cycle failure** — it is never silently coerced into a
`continue` no-op. This is the contract that prevents a parse/read failure from
masquerading as a "do nothing" decision. The **only** brain that returns
`Continue` without reasoning is the explicit `DeterministicLifecycleBrain`
fallback (no LLM available), which by construction never rolls or reaps. The
full fail-closed matrix is in
[Reference: `simard ooda record-decision`](ooda-record-decision-cli.md#read_verified--the-fail-closed-reader).

## Examples

Each example shows the tool invocation and the typed record it writes.

### `continue`

```bash
simard ooda record-decision --choice continue \
  --reason "PR #4501 open and CI running; engineer committed 3 min ago" ...
```

```json
{"schema": "simard.ooda.per_goal_decision.v1", "goal_id": "...", "cycle_number": 4501, "choice": "continue", "reason": "PR #4501 open and CI running; engineer committed 3 min ago"}
```

### `spawn` (standing research goal, idle between bursts)

```bash
simard ooda record-decision --choice spawn \
  --reason "no live work; standing research goal must seek the next source" \
  --task-hint "survey arXiv 2026 for new results on <topic>" ...
```

### `investigate` (quiet worker — NOT an auto-reap)

```bash
simard ooda record-decision --choice investigate \
  --reason "stale_claim_secs=9000 and log tail truncated mid-tool-call; read logs before any reclaim" ...
```

### `wait` (blocked on external CI)

```bash
simard ooda record-decision --choice wait \
  --reason "PR #4501 awaiting required CI checks; nothing actionable this cycle" ...
```

### `reorient` (deliberate redirect — clears wip refs)

```bash
simard ooda record-decision --choice reorient \
  --reason "current experiment design exhausted; pivot to the benchmark-first angle" ...
```

### `complete`

```bash
simard ooda record-decision --choice complete \
  --reason "goal shipped and merged in PR #4501; success criteria met" ...
```

## Comparison with the engineer-lifecycle recipe

| Aspect | `ooda-engineer-lifecycle` | `ooda-per-goal-cycle` |
|---|---|---|
| Fires when | Act phase is about to **skip** a goal with a live worktree | **Every** active goal, **every** cycle |
| Input | Live-worktree facts (mtime, sentinel pid) | **Durable** goal state + 3 demoted signals as inputs |
| Actions | `continue_skipping` / `reclaim_and_redispatch` / `deprioritize` / `open_tracking_issue` / `mark_goal_blocked` / `consider_self_update` | `continue` / `spawn` / `reorient` / `investigate` / `wait` / `complete` |
| Destructive path | direct (`reclaim_and_redispatch`) | **gated** behind an `investigate` verdict first |
| Parse failure | bounded escalation → `Err` | absent/malformed/mismatched typed record → `Err` (no silent fallback) |

## Versioning & Compatibility

Adding a new action requires a coordinated change:

1. Add the variant to `PerGoalAction` in `src/ooda_brain/mod.rs`.
2. Add its `apply_per_goal_action_to_state` mutation (respecting the A6
   `wip_refs` invariant).
3. Extend the `OPTIONS`/`HOW TO RECORD` guidance in the recipe YAML/prompt so the
   agent knows to pass the new value to `--choice`.
4. Add an example to `EXAMPLES` here and in the recipe.
5. Add serde round-trip, `read_verified` fail-closed, and mutation-table tests.

Cosmetic prompt edits (rationale guidance, examples, ROLE phrasing) ship alone
and take effect on the next cycle **without a rebuild**.

## See Also

- [Reference: `simard ooda record-decision` (typed decision tool)](ooda-record-decision-cli.md) — the tool the recipe calls; record format & fail-closed matrix
- [Reference: OODA Per-Goal-Cycle API](ooda-per-goal-cycle-api.md) — types, driver loop, tests
- [Concept: Agentic Per-Goal, Per-Cycle Decision](../concepts/agentic-per-goal-per-cycle.md)
- [Reference: OODA Engineer-Lifecycle Recipe](ooda-engineer-lifecycle-recipe.md) — the template
- [Reference: recipe context variable sanitization](recipe-context-var-sanitization.md)
- [Reference: OODA Brain Decision Protocol](ooda-brain-decision-protocol.md) — shared decision conventions
