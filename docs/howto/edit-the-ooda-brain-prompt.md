# How-To: Edit the OODA Brain Prompt

The OODA daemon's three brain phases (decide, orient, lifecycle) each have
a recipe YAML prompt that controls their behavior. All three use
**first-word/first-float extraction** — the parser takes the first token
from the LLM output and matches it against known variants. This guide shows
how to iterate on behavior **without touching Rust**.

## TL;DR

1. Edit the recipe YAML in `prompt_assets/simard/recipes/`.
2. **No rebuild required** — recipe YAMLs are loaded at runtime.
3. Tail `~/.simard/ooda.log` and the latest
   `~/.simard/cycle_reports/cycle_*.json` to confirm the new behavior.

## Prompt Structure

All three brains use the same prompt pattern. The parser expects the LLM
to output the decision as the **very first word** (or first number for
orient).

| Section | Purpose | Editable? |
|---|---|---|
| `# ROLE` | Identity & tone for the LLM | Yes |
| `# CONTEXT` | Templated variables substituted per call | Yes (must keep `{{var}}` placeholders) |
| `# OPTIONS` | The variant names the brain may return | **Add/remove only with a Rust enum change** |
| `# OUTPUT FORMAT` | Instructs "output the variant name as the first word" | **Do not remove** — the parser depends on first-word format |
| `# EXAMPLES` | Few-shot input→output pairs in first-word format | Yes — most useful knob for steering behavior |

### Output format

All three brains expect the decision as the first token:

**Decide brain:**
```
advance_goal PR is open; engineer needed to drive it to completion.
```

**Orient brain:**
```
0.60 Standard demotion for 1 failure. The goal is healthy.
```

**Lifecycle brain:**
```
continue_skipping engineer is making progress, worktree modified 30s ago
```

**Do not** instruct the model to emit JSON, `DECISION:` markers, or place
the keyword later in the text. The parser checks only the first word.

### Available context variables (lifecycle brain)

| Placeholder | Source |
|---|---|
| `{{goal_id}}` | The goal being dispatched |
| `{{goal_description}}` | Human description from the goal register |
| `{{cycle_number}}` | Current OODA cycle counter |
| `{{consecutive_skip_count}}` | Skips of this goal in the most recent 50 cycle reports |
| `{{failure_count}}` | `state.goal_failure_counts.get(goal_id)` |
| `{{worktree_mtime_secs_ago}}` | Seconds since the worktree was last modified |
| `{{sentinel_pid}}` | Live engineer's PID |
| `{{last_engineer_log_tail}}` | Last ~8 KB of the newest engineer log, with secrets redacted |

## Common Iterations

### Make the brain reclaim sooner

Lower the idle threshold in your few-shot examples:

```diff
-### Example: reclaim_and_redispatch
-CONTEXT: skips=12, failures=0, worktree_idle=25200s, ...
-OUTPUT: reclaim_and_redispatch Previous engineer was idle for 7 hours. Try fresh approach.
+### Example: reclaim_and_redispatch
+CONTEXT: skips=4, failures=0, worktree_idle=3600s, ...
+OUTPUT: reclaim_and_redispatch Previous engineer was idle for 1 hour. Try fresh approach.
```

LLMs imitate examples more reliably than they follow prose rules. Move the
threshold by moving the example.

### Add stricter blocking criteria

Add a new few-shot example to **EXAMPLES** showing when to block:

```markdown
### Example: mark_goal_blocked (compile error loop)
CONTEXT: skips=6, log_tail=...error[E0277]: the trait bound...
OUTPUT: mark_goal_blocked compile-error-loop Engineer stuck in type-error loop for 6 cycles.
```

### Tune the rationale style

Change the **ROLE** section's tone. The text after the first word becomes
the rationale field.

## Editing the three prompts

### Decide prompt (recipe-based — no rebuild required)

Edit `prompt_assets/simard/recipes/ooda-decide.yaml`:

```yaml
steps:
  - name: decide-action
    type: agent
    prompt: |
      # OODA Brain — Decide Phase: Action-Kind Routing
      ## ROLE
      …
      ## OUTPUT FORMAT
      Output the action keyword as the very first word of your response.
      Follow with a brief rationale.
      ## OPTIONS
      …
      ## EXAMPLES
      advance_goal Standard goal with open PR, drive to completion.
      consolidate_memory This is a memory consolidation trigger.
```

The decide brain matches the first word against 10 action keywords. If no
match, defaults to `advance_goal`.

### Orient prompt (recipe-based — no rebuild required)

Edit `prompt_assets/simard/recipes/ooda-orient.yaml`:

```yaml
steps:
  - name: orient-urgency
    type: agent
    prompt: |
      # OODA Brain — Orient Phase: Failure-Penalty Demotion
      ## OUTPUT FORMAT
      Output the adjusted urgency as a bare decimal number (e.g. `0.42`)
      as the first token of your response. Follow with your rationale.
      ## EXAMPLES
      0.60 Standard demotion for 1 failure.
      0.0 Driven to zero after 5 consecutive failures.
```

The orient brain extracts the first float from the text. If no float found,
falls back to deterministic floor formula.

### Lifecycle prompt (recipe-based — no rebuild required)

Edit `prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml`:

```yaml
steps:
  - name: lifecycle-decision
    type: agent
    prompt: |
      # OODA Brain — Engineer Lifecycle Decision
      ## OUTPUT FORMAT
      Output the variant name as the very first word of your response.
      Follow with your rationale.
      ## OPTIONS
      continue_skipping, reclaim_and_redispatch, deprioritize,
      open_tracking_issue, mark_goal_blocked, consider_self_update
      ## EXAMPLES
      continue_skipping engineer is healthy, worktree modified recently.
      reclaim_and_redispatch engineer idle for 7 hours, try fresh approach.
```

The lifecycle brain matches the first word against 6 variant names. If no
match, defaults to `continue_skipping`.

## Validating Your Edits

The fastest validation loop is the brain's unit tests:

```bash
cargo test -p simard recipe_brain
```

For end-to-end behavior, wait for the next OODA cycle (recipe YAMLs are
loaded at runtime — no rebuild needed) and watch the logs.

## Constraints

* **First word must be a recognized variant** (decide, lifecycle) or a
  **decimal number** (orient). If the LLM outputs prose before the keyword,
  the parser will default. Ensure the OUTPUT FORMAT section says "as the
  very first word."
* **Case-insensitive matching** on the first word — `Advance_Goal` and
  `ADVANCE_GOAL` both match. This is a single `eq_ignore_ascii_case()`
  call, not a scan.
* **Extra fields on lifecycle variants use defaults.** There is no
  labeled-line extraction for `TITLE:`, `BODY:`, `REASON:`,
  `REDISPATCH_CONTEXT:`. The LLM's prose after the first word becomes
  the rationale.
* If you remove all examples for a given variant, the LLM may stop emitting
  it. Keep at least one example per variant you want reachable.

## See Also

* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md)
* [Concept: prompt-driven OODA brain](../concepts/prompt-driven-ooda-brain.md)
* [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md)
* [Reference: `OodaBrain` API](../reference/ooda-brain-api.md)
* [Reference: `ooda_brain.md` prompt schema](../reference/ooda-brain-prompt.md)
* [Reference: `ooda_decide.md` prompt schema](../reference/ooda-decide-prompt.md)
* [Reference: `ooda_orient.md` prompt schema](../reference/ooda-orient-prompt.md)
