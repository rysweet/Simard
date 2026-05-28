# How-To: Edit the OODA Brain Prompt

The OODA daemon's engineer-lifecycle behavior is reasoned about by an LLM
reading `prompt_assets/simard/ooda_brain.md`.
This guide shows how to iterate on that behavior **without touching Rust**.

## TL;DR

1. Edit `prompt_assets/simard/ooda_brain.md`.
2. Rebuild: `cargo build --release -p simard`.
   (The prompt is compiled in via `include_str!`; a rebuild is required.)
3. Restart the daemon (see
   [run-ooda-daemon](run-ooda-daemon.md)).
4. Tail `~/.simard/ooda.log` and the latest
   `~/.simard/cycle_reports/cycle_*.json` to confirm the new behavior.

## Prompt Structure

All three OODA brain prompts use the same output contract: the model's
**first word** must be the decision (variant name for decide/lifecycle, bare
decimal for orient). Everything after the first word is rationale text.

> **Changed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The `DECISION:` marker format, labeled-line fields, JSON object format,
> and keyword-scanning protocol have all been removed. All three brains now
> use first-word/first-float extraction.

| Section | Purpose | Editable? |
|---|---|---|
| `# ROLE` | Identity & tone for the LLM | Yes |
| `# CONTEXT` | Templated variables Simard substitutes per call | Yes (must keep `{{var}}` placeholders — see below) |
| `# OPTIONS` | The variant values the brain may return | **Add/remove only with a Rust enum change**; renaming text guidance is fine |
| `# OUTPUT_FORMAT` | Instructs model to output variant as first word | **Keep the first-word instruction**; edit only the prose and examples |
| `# EXAMPLES` | Few-shot input→output pairs | Yes — most useful knob for steering behavior |

### Output format

All three brains expect the decision as the **first word** of the response:

```
continue_skipping engineer is making progress, worktree modified 30s ago
```

For the orient brain, the first token must be a bare decimal:

```
0.6 Standard floor demotion applied
```

**Do not** instruct the model to emit JSON, `DECISION:` markers, or labeled
lines. The parsers look for a single first word only. See
[text-parsing wire formats](../reference/text-parsing-wire-formats.md)
for the full grammar.

### Available context variables

These are substituted by `gather_engineer_lifecycle_ctx` before the prompt is
sent to the LLM. Unknown placeholders are left as literal text.

| Placeholder | Source |
|---|---|
| `{{goal_id}}` | The goal being dispatched |
| `{{goal_description}}` | Human description from the goal register |
| `{{cycle_number}}` | Current OODA cycle counter |
| `{{consecutive_skip_count}}` | Skips of this goal in the most recent 50 cycle reports |
| `{{failure_count}}` | `state.goal_failure_counts.get(goal_id)` |
| `{{worktree_mtime_secs_ago}}` | Seconds since the worktree was last modified |
| `{{sentinel_pid}}` | Live engineer's PID |
| `{{last_engineer_log_tail}}` | Last ~8 KB of the newest `~/.simard/agent_logs/engineer-{goal_id}-*.log`, with secrets redacted |

## Common Iterations

### Make the brain reclaim sooner

Lower the idle threshold in your few-shot examples:

```diff
-### Example: reclaim_and_redispatch
-CONTEXT: skips=12, failures=0, worktree_idle=25200s, ...
+### Example: reclaim_and_redispatch
+CONTEXT: skips=4, failures=0, worktree_idle=3600s, ...
 OUTPUT:
 reclaim_and_redispatch Previous engineer was idle for too long. Try fresh approach.
```

LLMs imitate examples more reliably than they follow prose rules. Move the
threshold by moving the example.

### Add stricter blocking criteria

Add a new few-shot example to **EXAMPLES** showing a log tail containing the
phrase you want to treat as a block signal:

```markdown
### Example: mark_goal_blocked (compile error loop)
CONTEXT: skips=6, log_tail=...error[E0277]: the trait bound...
OUTPUT:
mark_goal_blocked engineer is stuck in a type-error loop
```

### Tune the rationale style

Change the **ROLE** section's tone. Example: change "be terse" to "explain in
one sentence why you chose this option". The `rationale` field will follow.

## Validating Your Edits

The fastest validation loop is the brain's unit tests, which exercise the
parser against the shipped prompt and a stub LLM submitter:

```bash
cargo test -p simard ooda_brain
```

For end-to-end behavior, restart the daemon (TL;DR above) and watch
`~/.simard/cycle_reports/cycle_*.json` for the new `ActionOutcome.detail`
prefixes (`engineer alive — continue (brain): …`, `reclaimed pid …`, etc.).

## Constraints

* The LLM **must** output the lifecycle variant name as the **first word** of
  its response (for the **engineer-lifecycle** brain). The parser extracts this
  first word and matches it case-insensitively. Responses where the first word
  is not a valid variant default to `continue_skipping`.
* The **decide brain** also uses first-word extraction. The first word must be
  one of the 10 action keywords. If no keyword matches, `advance_goal` is used.
* The **orient brain** uses first-float extraction. The first decimal number in
  the response becomes `adjusted_urgency`. If no float is found, the
  deterministic floor applies.
* Structured extra fields (`title`, `body`, `reason`, `redispatch_context`) are
  no longer extracted from the output. They use defaults. Do not instruct the
  model to emit labeled lines — they will be treated as rationale text.
* If you remove all examples for a given variant, the LLM may stop emitting
  it. Keep at least one example per variant you want reachable.
* The engineer-lifecycle prompt file is loaded via `include_str!`; it must
  be valid UTF-8 and small enough to embed in the binary without bloat.
  Keep it under ~32 KB as a soft guideline.
* The decide and orient prompts are loaded at runtime from recipe YAMLs and
  have no compile-time embedding constraint.

## Editing the decide and orient prompts

The decide and orient brains have their own prompt files:

- `prompt_assets/simard/recipes/ooda-decide.yaml` — action-kind routing (recipe step)
- `prompt_assets/simard/ooda_orient.md` — failure-penalty demotion (compiled in)

### Decide prompt (recipe-based — no rebuild required)

The decide prompt lives inside the recipe YAML file. Edit the `prompt:`
field in `prompt_assets/simard/recipes/ooda-decide.yaml`:

```yaml
steps:
  - name: decide-action
    type: agent
    prompt: |
      # OODA Brain — Decide Phase: Action-Kind Routing
      ## ROLE
      …
      ## OPTIONS
      …
```

**No rebuild or `safe-update` required.** Because the recipe YAML is loaded
at runtime by `recipe-runner-rs`, edits take effect on the next OODA cycle
automatically.

The decide brain uses **first-word extraction** — the first word of the
agent's output is matched case-insensitively against the 10 action keywords.
There is no keyword-scanning of the full response. The agent must output the
action keyword as its first word.

To steer the agent toward different routing:

1. **Edit the `OPTIONS` section** — add or remove action keywords.
   Adding a keyword requires a coordinated Rust change to `DecideJudgment`
   and `parse_action_from_text()`.
2. **Edit the `EXAMPLES` section** — the most effective knob. LLMs imitate
   examples more reliably than they follow prose rules. Ensure every example
   starts with the keyword as the first word.
3. **Add negative examples** — critical for preventing substring
   pattern-matching (e.g., a goal named "memory-allocation" should route to
   `advance_goal`, not `consolidate_memory`).

### Orient prompt (recipe-based — rebuild required)

**Orient** uses first-float extraction — the first decimal number in the
response becomes `adjusted_urgency`:
```
0.6 Standard floor demotion applied
```

Edit `prompt_assets/simard/recipes/ooda-orient.yaml`, then rebuild:

```bash
cargo build --release -p simard
simard safe-update
```

See [text-parsing wire formats](../reference/text-parsing-wire-formats.md)
for the full grammar of each format.

## See Also

* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md)
* [Concept: prompt-driven OODA brain](../concepts/prompt-driven-ooda-brain.md)
* [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md)
* [Reference: `OodaBrain` API](../reference/ooda-brain-api.md)
* [Reference: `ooda_brain.md` prompt schema](../reference/ooda-brain-prompt.md)
* [Reference: `ooda_decide.md` prompt schema](../reference/ooda-decide-prompt.md) — decide recipe and first-word parser
* [Reference: `ooda_orient.md` prompt schema](../reference/ooda-orient-prompt.md)
