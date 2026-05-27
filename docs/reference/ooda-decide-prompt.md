# Reference: OODA Decide Recipe and Prompt Schema

Recipe: `prompt_assets/simard/recipes/ooda-decide.yaml`
Prompt source: `prompt_assets/simard/ooda_decide.md` (content embedded in recipe YAML)
Parser: `parse_action_from_text()` in `src/ooda_brain/recipe_brain.rs`

This is the single source of truth for the decide-phase action-kind routing
decision. The decide brain runs as a **recipe step** via `recipe-runner-rs`.
The agent's output must start with the action keyword as the first word —
the parser extracts that first word and matches it case-insensitively against
the 10 known action keywords.

> **History:** Before issue
> [#2111](https://github.com/rysweet/Simard/issues/2111), the decide brain
> was `RustyClawdDecideBrain`, which used `DECISION:` markers. In #2111
> it moved to keyword-anywhere scanning via `ascii_contains_ignore_case`.
> In [#2144](https://github.com/rysweet/Simard/issues/2144), the keyword
> scanner was replaced with first-word extraction — the simplest possible
> parse. The recipe prompt now explicitly instructs the LLM to output the
> action keyword as the very first word.

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

      ## OUTPUT FORMAT
      Output the action keyword as the very first word of your response.
      Follow with a brief rationale.

      ## EXAMPLES
      …(first-word format examples, one per routing case)…

      ## Merge Authority
      …

      ## Self-update awareness
      …
```

The recipe is a single `agent` step. The recipe-runner-rs subprocess handles
prompt rendering, agent invocation, and stdout capture. The Rust parser
(`parse_action_from_text`) extracts the first word.

### What changed from prior versions

- **OUTPUT FORMAT section added** — instructs the LLM to output the action
  keyword as the very first word. Previously there was no OUTPUT FORMAT
  section (keyword-anywhere scanning didn't need one).
- **EXAMPLES updated** — examples now show first-word format:
  `advance_goal PR is open; engineer needed.` instead of prose with the
  keyword embedded later in the text.
- The ROLE, CONTEXT, OPTIONS, Merge Authority, and Self-update awareness
  sections are preserved.

## Placeholders (Context Variables)

| Variable | Type | Source |
|---|---|---|
| `{{goal_id}}` | string | `ctx.goal_id` — goal slug or reserved synthetic ID |
| `{{urgency}}` | string (f64) | `ctx.urgency` — Orient's score in `[0.0, 1.0]` |
| `{{reason}}` | string | `ctx.reason` — Orient's rationale for this priority |

## Action Keywords

The `OPTIONS` section enumerates the valid action keywords. Each maps 1:1
to a `DecideJudgment` enum variant. The parser matches the first word of
the agent's output against these keywords case-insensitively.

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

## First-Word Parser

`parse_action_from_text()` extracts the action kind from the first word
of the agent's stdout:

### How it works

1. Split the output on whitespace, take the first token.
2. Lowercase the token via `.to_ascii_lowercase()`.
3. Match against the 10 action keywords using `eq_ignore_ascii_case()`.
4. Return the matching `DecideJudgment`.
5. If no match, return `DecideJudgment::AdvanceGoal` as the default.
6. Remaining text after the first word is the rationale (truncated to 500 chars).

### Why this works

Production daemon logs showed that the agent **always** returned the correct
action keyword — the keyword just wasn't always the first word. The prompt
now explicitly instructs the LLM to output the keyword first.

### Comparison with other parsers

| Site | Parse method | Default (no match) |
|------|--------------|--------------------|
| Progress checker | Keyword-anywhere scan | `Accept` (fail-open) |
| Merge judge | Keyword-anywhere scan | `NotReady` (fail-closed) |
| **Decide brain** | **First-word extraction** | `AdvanceGoal` (fail-safe) |
| **Orient brain** | **First-float extraction** | Deterministic floor |
| **Lifecycle brain** | **First-word extraction** | `ContinueSkipping` (safe) |

> **Changed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The decide, orient, and lifecycle brains all moved from keyword-anywhere
> scanning / JSON extraction / DECISION markers to first-word/first-float
> extraction. The progress checker and merge judge retain keyword-anywhere
> scanning (they parse recipe-shim output, not OODA brain output).

## Examples

The prompt's `EXAMPLES` section shows first-word format:

| Case | Input pattern | Example agent output |
|---|---|---|
| Reserved synthetic ID | `goal_id: "__memory__"` | `consolidate_memory This is a memory consolidation trigger.` |
| Ordinary goal slug | `goal_id: "ship-v1"` | `advance_goal Standard goal with open PR, drive to completion.` |
| Activity polling | `goal_id: "__poll_activity__"` | `poll_developer_activity Reserved polling ID.` |
| Negative example | Real goal with "memory" in name | `advance_goal Despite the name, this is a real goal.` |

Negative examples remain critical: without them, models pattern-match on
substring similarity between the goal name and the action keyword.

## Error Handling

`RecipeBrain` returns `Err(SimardError::AdapterInvocationFailed)` when:

- The `recipe-runner-rs` binary is not found (construction fails;
  `RecipeBrain::new()` returns `None`).
- The subprocess exits with a non-zero status.
- The subprocess cannot be spawned.

The first-word parser itself **never** returns an error — if no keyword is
found in the first word, it returns `AdvanceGoal`. This is a conscious
design choice: the safe default for any real goal.

## Runtime Loading (not compile-time)

The decide recipe is loaded at runtime by the recipe-runner-rs subprocess.
`RecipeBrain` resolves the recipe path relative to `repo_root`:

```
{repo_root}/prompt_assets/simard/recipes/ooda-decide.yaml
```

Prompt edits take effect on the next daemon cycle **without a rebuild**.

## Versioning & Compatibility

Adding a new action keyword requires a coordinated change:

1. Add the variant to `DecideJudgment` in `src/ooda_brain/decide.rs`.
2. Add the mapping from `DecideJudgment` → `ActionKind`.
3. Add the match arm in `parse_action_from_text()` in
   `src/ooda_brain/recipe_brain.rs`.
4. Add the keyword to the `OPTIONS` section in the recipe prompt.
5. Add an example to the `EXAMPLES` section (first-word format).
6. Add a test to `recipe_brain.rs` covering the new keyword.
7. Update the variant table in
   [text-parsing wire formats](text-parsing-wire-formats.md#1a-decide-phase-recipe_brainrs).

Cosmetic edits (rationale guidance, examples, ROLE phrasing) are safe to
ship alone — and take effect without a rebuild.

## See Also

* [Reference: `ooda_brain.md` prompt schema](ooda-brain-prompt.md) — engineer-lifecycle prompt
* [Reference: `ooda_orient.md` prompt schema](ooda-orient-prompt.md) — orient-phase prompt
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md) — normative grammar
* [Reference: `OodaBrain` API](ooda-brain-api.md) — trait and type definitions
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — editing guide
* [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
