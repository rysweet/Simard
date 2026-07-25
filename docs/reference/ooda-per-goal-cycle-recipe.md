# Reference: OODA Per-Goal-Cycle Recipe & Prompt Schema

Recipe: `prompt_assets/simard/recipes/ooda-per-goal-cycle.yaml`
Prompt source: `prompt_assets/simard/ooda_per_goal_cycle.md`
Shim: `RecipeBrain::decide_per_goal_cycle` (`src/ooda_brain/recipe_brain.rs`)

This is the single source of truth for the **per-goal, per-cycle decision** — the
reasoning step run once for **every** goal on `board.active`, every cycle
([#4453](https://github.com/rysweet/Simard/issues/4453)). It is the sibling of
[`ooda-engineer-lifecycle.yaml`](ooda-engineer-lifecycle-recipe.md) and follows
the identical recipe pattern: a single `agent` step whose stdout is a JSON
decision envelope parsed by the Rust shim.

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
      ## OUTPUT FORMAT ...(JSON envelope)...
      ## EXAMPLES ...
    output: "per_goal_cycle_result"
```

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

```
worker_present=false, standing_idle_signal=true, stale_claim_secs=9000

A bursty standing goal with no live worktree is NORMAL, not death. This is the
next piece of ongoing work, not a fault.
{"choice": "spawn", "reason": "standing research goal is idle between bursts; seek a new source", "task_hint": "search for 2026 sources on <topic>"}
```

## OUTPUT FORMAT — JSON envelope

The agent responds with a single JSON object (a fenced ```json block is fine;
the shim strips surrounding banner/prose before parsing):

```json
{"choice": "<action>", "reason": "<why>"}
```

where `<action>` is exactly one of `continue`, `spawn`, `reorient`,
`investigate`, `wait`, `complete`. Only `spawn` may add a `task_hint` string.
`reason` is **required** for every variant.

### Parsing & no silent fallback ([#1711](https://github.com/rysweet/Simard/issues/1711))

`RecipeBrain::decide_per_goal_cycle` parses the envelope strictly:

- Unknown `choice` → `Err`.
- Missing/empty `reason` → `Err`.
- Non-JSON / no envelope found → `Err`.
- Subprocess spawn failure / non-zero exit → `Err`.

An `Err` **surfaces as a cycle failure** — it is never silently coerced into a
`continue` no-op. This is the contract that prevents a parse failure from
masquerading as a "do nothing" decision. The **only** brain that returns
`Continue` without reasoning is the explicit `DeterministicLifecycleBrain`
fallback (no LLM available), which by construction never rolls or reaps.

## Examples

### `continue`

```json
{"choice": "continue", "reason": "PR #4501 open and CI running; engineer committed 3 min ago"}
```

### `spawn` (standing research goal, idle between bursts)

```json
{"choice": "spawn", "reason": "no live work; standing research goal must seek the next source", "task_hint": "survey arXiv 2026 for new results on <topic>"}
```

### `investigate` (quiet worker — NOT an auto-reap)

```json
{"choice": "investigate", "reason": "stale_claim_secs=9000 and log tail truncated mid-tool-call; read logs before any reclaim"}
```

### `wait` (blocked on external CI)

```json
{"choice": "wait", "reason": "PR #4501 awaiting required CI checks; nothing actionable this cycle"}
```

### `reorient` (deliberate redirect — clears wip refs)

```json
{"choice": "reorient", "reason": "current experiment design exhausted; pivot to the benchmark-first angle"}
```

### `complete`

```json
{"choice": "complete", "reason": "goal shipped and merged in PR #4501; success criteria met"}
```

## Comparison with the engineer-lifecycle recipe

| Aspect | `ooda-engineer-lifecycle` | `ooda-per-goal-cycle` |
|---|---|---|
| Fires when | Act phase is about to **skip** a goal with a live worktree | **Every** active goal, **every** cycle |
| Input | Live-worktree facts (mtime, sentinel pid) | **Durable** goal state + 3 demoted signals as inputs |
| Actions | `continue_skipping` / `reclaim_and_redispatch` / `deprioritize` / `open_tracking_issue` / `mark_goal_blocked` / `consider_self_update` | `continue` / `spawn` / `reorient` / `investigate` / `wait` / `complete` |
| Destructive path | direct (`reclaim_and_redispatch`) | **gated** behind an `investigate` verdict first |
| Parse failure | bounded escalation → `Err` | bounded escalation → `Err` (no silent fallback) |

## Versioning & Compatibility

Adding a new action requires a coordinated change:

1. Add the variant to `PerGoalAction` in `src/ooda_brain/mod.rs`.
2. Add its `apply_per_goal_action_to_state` mutation (respecting the A6
   `wip_refs` invariant).
3. Add the `choice` tag + guidance to the `OPTIONS`/`RULES` sections of the
   recipe YAML.
4. Add an example to `EXAMPLES` here and in the recipe.
5. Add serde round-trip + mutation-table tests.

Cosmetic prompt edits (rationale guidance, examples, ROLE phrasing) ship alone
and take effect on the next cycle **without a rebuild**.

## See Also

- [Reference: OODA Per-Goal-Cycle API](ooda-per-goal-cycle-api.md) — types, driver loop, tests
- [Concept: Agentic Per-Goal, Per-Cycle Decision](../concepts/agentic-per-goal-per-cycle.md)
- [Reference: OODA Engineer-Lifecycle Recipe](ooda-engineer-lifecycle-recipe.md) — the template
- [Reference: recipe context variable sanitization](recipe-context-var-sanitization.md)
- [Reference: OODA Brain Decision Protocol](ooda-brain-decision-protocol.md) — shared envelope conventions
