# Reference: OODA Decide Recipe and Prompt Schema

Recipe: `prompt_assets/simard/recipes/ooda-decide.yaml`
Prompt source: `prompt_assets/simard/ooda_decide.md` (content embedded in recipe YAML)
Shim: `src/ooda_brain/recipe_decide.rs`

This is the single source of truth for the decide-phase action-kind routing
decision. The decide brain runs as a **recipe step** via `recipe-runner-rs`,
following the same pattern as `progress-assessment.yaml` and
`merge-readiness-judge.yaml`. The agent's prose output is scanned for action
keywords — no `DECISION:` marker or structured output format is required.

> **History:** Before issue
> [#2111](https://github.com/rysweet/Simard/issues/2111), the decide brain
> was `RustyClawdDecideBrain`, which compiled the prompt via `include_str!`,
> submitted it to an `LlmSubmitter`, and parsed the response using a
> `DECISION:` marker on the first line. This was fragile — the agent
> consistently returned the correct action keyword in its prose, but the
> parser demanded a specific format the model frequently ignored. The
> recipe-based approach removes the format requirement entirely and scans
> for keywords instead.

## Recipe Layout

```yaml
name: ooda-decide
description: Route an OODA priority to the correct action kind
context:
  goal_id: ""
  urgency: ""
  reason: ""
steps:
  - name: decide-action
    type: agent
    prompt: |
      # OODA Brain — Decide Phase: Action-Kind Routing

      ## ROLE
      …

      ## CONTEXT
      Goal ID: {{goal_id}}
      Urgency: {{urgency}}
      Reason: {{reason}}

      ## OPTIONS
      …(variant tags: advance_goal, consolidate_memory, etc.)…

      ## EXAMPLES
      …(text-format examples, one per routing case)…

      ## Merge Authority
      …

      ## Self-update awareness
      …
```

The recipe is a single `agent` step. The recipe-runner-rs subprocess handles
prompt rendering, agent invocation, and stdout capture. The Rust shim
(`RecipeDecideBrain`) parses the stdout.

### What changed from `ooda_decide.md`

The recipe prompt preserves all content from the original `ooda_decide.md`
**except**:

- **Line 1 deleted** — the `CRITICAL: Your first non-blank line MUST be
  DECISION: <variant>` guard is removed. The agent is no longer required
  to emit a specific marker format.
- **OUTPUT_FORMAT section deleted** — the entire section instructing the
  model to emit `DECISION:` markers is removed. The keyword scanner finds
  the action kind in natural prose.
- **Placeholders converted** — `{goal_id}` → `{{goal_id}}`, `{urgency}` →
  `{{urgency}}`, `{reason}` → `{{reason}}` to match recipe-runner-rs
  Handlebars templating.

The ROLE, CONTEXT, OPTIONS, EXAMPLES, Merge Authority, and Self-update
awareness sections are preserved verbatim.

## Placeholders (Context Variables)

The recipe-runner-rs performs Handlebars `{{name}}` substitution from the
context variables passed by `RecipeDecideBrain`.

| Variable | Type | Source |
|---|---|---|
| `{{goal_id}}` | string | `ctx.goal_id` — goal slug or reserved synthetic ID |
| `{{urgency}}` | string (f64) | `ctx.urgency` — Orient's score in `[0.0, 1.0]` |
| `{{reason}}` | string | `ctx.reason` — Orient's rationale for this priority |

## Action Keywords

The `OPTIONS` section enumerates the valid action keywords. Each maps 1:1
to a `DecideJudgment` enum variant in `src/ooda_brain/decide.rs`. The keyword
scanner in `recipe_decide.rs` finds these keywords in the agent's prose
output.

| Keyword | Enum variant | When to use |
|---|---|---|
| `advance_goal` | `DecideJudgment::AdvanceGoal` | Default for any non-reserved `goal_id` |
| `consolidate_memory` | `DecideJudgment::ConsolidateMemory` | Reserved `__memory__` synthetic ID |
| `run_improvement` | `DecideJudgment::RunImprovement` | Reserved `__improvement__` synthetic ID |
| `poll_developer_activity` | `DecideJudgment::PollDeveloperActivity` | Reserved `__poll_activity__` synthetic ID |
| `extract_ideas` | `DecideJudgment::ExtractIdeas` | Reserved `__extract_ideas__` synthetic ID |
| `safe_update` | `DecideJudgment::SafeUpdate` | Reserved `__safe_update__` synthetic ID |
| `research_query` | `DecideJudgment::ResearchQuery` | Reserved for future use |
| `run_gym_eval` | `DecideJudgment::RunGymEval` | Reserved for future use |
| `build_skill` | `DecideJudgment::BuildSkill` | Reserved for future use |
| `launch_session` | `DecideJudgment::LaunchSession` | Reserved for future use |

No keyword is a substring of another (verified at compile time). The scanner
checks all 10 keywords on the lowercased output using `contains()`. If
multiple keywords appear (rare — the agent is asked for a single routing
decision), the first match in scan order is used.

## Keyword Scanner (replaces DECISION marker parser)

`RecipeDecideBrain` uses `parse_action_from_text()` in
`src/ooda_brain/recipe_decide.rs` to extract the action kind from the
agent's stdout. This follows the same **keyword verdict protocol** used by
`recipe_progress_checker.rs` and `recipe_merge_judge.rs`.

### How it works

1. Convert the full stdout to lowercase.
2. Scan for each of the 10 action keywords using `contains()`.
3. Return the first matching keyword as the `DecideJudgment`.
4. If no keyword is found, return `DecideJudgment::AdvanceGoal` as the
   default (same as the existing deterministic fallback for real goal slugs).

### Why this works

Production daemon logs showed that the agent **always** returned the correct
action keyword in its prose. Typical responses:

```
Looking at goal "__memory__", this is a reserved synthetic ID for memory
consolidation. The appropriate action is consolidate_memory.
```

```
This is an ordinary goal with an open PR. The engineer should advance_goal
to drive the PR to completion.
```

The old `DECISION:` marker parser rejected both of these because the keyword
wasn't on the first line in `DECISION: <variant>` format. The keyword scanner
finds `consolidate_memory` and `advance_goal` directly.

### Comparison with other keyword scanners

| Site | Keywords | Default (no match) | Fail mode |
|------|----------|--------------------|-----------|
| Progress checker | `accept`, `reject` | `Accept` (fail-open) | Goal unblocked |
| Merge judge | `ready`, `not_ready`, `unclear` | `NotReady` (fail-closed) | PR not merged |
| **Decide brain** | 10 action keywords | `AdvanceGoal` (fail-safe) | Goal gets default routing |

The `AdvanceGoal` default is safe because the deterministic fallback brain
already maps real goal slugs to `advance_goal`. The keyword scanner simply
makes the LLM's judgment reachable for edge cases where the deterministic
mapping would be wrong (e.g., `research_query`).

## Error Handling

`RecipeDecideBrain` returns `Err(SimardError::AdapterInvocationFailed)` when:

- The `recipe-runner-rs` binary is not found (construction fails;
  `RecipeDecideBrain::new()` returns `None`).
- The subprocess exits with a non-zero status.
- The subprocess cannot be spawned (permission error, missing binary at
  runtime, etc.).

On `AdapterInvocationFailed`, the caller in `ooda_loop/decide.rs` records a
parse failure, falls back per-priority to the deterministic mapping, and
logs the error with the full stderr (truncated to 500 chars).

The keyword scanner itself **never** returns an error — if no keyword is
found, it returns `AdvanceGoal`. This is a conscious design choice: the
agent is always given the option list, and `advance_goal` is the safe
default for any real goal.

## Examples

The prompt's `EXAMPLES` section contains routing examples. Unlike the old
DECISION marker format, examples now show natural prose responses:

| Case | Input pattern | Example agent output |
|---|---|---|
| Reserved synthetic ID | `goal_id: "__memory__"` | `This is a memory consolidation trigger. consolidate_memory.` |
| Ordinary goal slug | `goal_id: "ship-v1"` | `Standard goal with open PR. advance_goal to drive completion.` |
| Activity polling | `goal_id: "__poll_activity__"` | `Reserved polling ID. poll_developer_activity.` |
| Negative example | Real goal with "memory" in name | `Despite the name, this is a real goal. advance_goal.` |

Negative examples remain critical: without them, models pattern-match on
substring similarity between the goal name and the action keyword.

## Merge Authority Section

The prompt includes a `## Merge Authority` section documenting Simard's gated
authority to squash-merge pull requests via `stewardship::merge_pr_if_merge_ready`.
This section is **informational context** — it does not add a merge-related
action keyword. The brain surfaces merge-readiness observations in the
rationale text and routes to `advance_goal`.

## Self-Update Awareness Section

The prompt includes a `## Self-update awareness` section documenting the
four-part doctrine for the `safe_update` action. This action triggers
`simard safe-update` (drain → snapshot → pre-test → swap → exec → validate →
optional rollback). The section gates the action on:

1. Divergence ≥ N commits behind `origin/main`
2. No critical WIP (no in-flight engineers with PR-blocking goals)
3. Clean previous cycle (no failures, no tracking issues)
4. Cooldown elapsed (≥30 min since last attempt)

## Runtime Loading (not compile-time)

Unlike `ooda_brain.md` (which is embedded via `include_str!`), the decide
recipe is loaded at runtime by the recipe-runner-rs subprocess.
`RecipeDecideBrain` resolves the recipe path relative to `repo_root`:

```
{repo_root}/prompt_assets/simard/recipes/ooda-decide.yaml
```

This means prompt edits take effect on the next daemon cycle **without a
rebuild** — just edit the YAML and the next cycle picks it up. This is a
significant improvement over the old `include_str!` approach, which required
`cargo build` + `simard safe-update` for every prompt change.

> **Note:** The `DECIDE_PROMPT_NAME` constant is retained in `decide.rs`
> for audit-trail versioning via `prompt_store::current_version()`. It
> identifies the prompt content for parse-failure diagnostics, not for
> compile-time loading.

## Versioning & Compatibility

Semantic changes (adding a new action keyword) require a coordinated change:

1. Add the variant to `DecideJudgment` in `src/ooda_brain/decide.rs`.
2. Add the mapping from `DecideJudgment` → `ActionKind`.
3. Add the keyword to `parse_action_from_text()` in
   `src/ooda_brain/recipe_decide.rs`.
4. Add the keyword to the `OPTIONS` section in the recipe prompt.
5. Add an example to the `EXAMPLES` section.
6. Add a test to `recipe_decide.rs` covering the new keyword.
7. Update the variant table in
   [text-parsing wire formats § decide](text-parsing-wire-formats.md#2c-decide-brain-recipe_deciders).

Cosmetic edits (rationale guidance, examples, ROLE phrasing) are safe to
ship alone — and take effect without a rebuild.

## Construction Pattern

```rust
let brain: Box<dyn OodaDecideBrain> = match RecipeDecideBrain::new(repo_root) {
    Some(b) => Box::new(b),
    None => {
        eprintln!("[ooda] recipe-runner-rs not found; using deterministic fallback");
        Box::new(DeterministicFallbackDecideBrain)
    }
};
```

`RecipeDecideBrain::new(repo_root)` returns `None` when:
- The `recipe-runner-rs` binary is not on `$PATH`.
- The recipe YAML file does not exist at the expected path.

The daemon wiring in `operator_commands_ooda/daemon/brains.rs` calls
`build_decide_brain(state_root, repo_root)`, which performs this
construction.

## See Also

* [Reference: `ooda_brain.md` prompt schema](ooda-brain-prompt.md) — engineer-lifecycle prompt
* [Reference: `ooda_orient.md` prompt schema](ooda-orient-prompt.md) — orient-phase prompt
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md) — normative grammar
* [Reference: `OodaBrain` API](ooda-brain-api.md) — trait and type definitions
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — editing guide
* [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
