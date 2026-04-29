# How-To: Edit the OODA Brain Prompt

The OODA daemon's engineer-lifecycle behavior is reasoned about by an LLM
reading [`prompt_assets/simard/ooda_brain.md`](../../prompt_assets/simard/ooda_brain.md).
This guide shows how to iterate on that behavior **without touching Rust**.

## TL;DR

1. Edit `prompt_assets/simard/ooda_brain.md`.
2. Rebuild: `cargo build --release -p simard`.
   (The prompt is compiled in via `include_str!`; a rebuild is required.)
3. Restart the daemon (see
   [run-ooda-daemon](run-ooda-daemon.md#restarting-the-daemon)).
4. Tail `~/.simard/ooda.log` and the latest
   `~/.simard/cycle_reports/cycle_*.json` to confirm the new behavior.

## Prompt Structure

The prompt has five fixed sections. The brain parser only cares about
**OUTPUT_FORMAT** (the JSON schema) and **OPTIONS** (which `choice` strings
are legal). Edit the others freely.

| Section | Purpose | Editable? |
|---|---|---|
| `# ROLE` | Identity & tone for the LLM | Yes |
| `# CONTEXT` | Templated variables Simard substitutes per call | Yes (must keep `{{var}}` placeholders the brain populates — see below) |
| `# OPTIONS` | The five `choice` values the brain may return | **Add/remove only with a Rust enum change**; renaming text guidance is fine |
| `# OUTPUT_FORMAT` | JSON schema for the response | **Do not change shape**; only edit prose |
| `# EXAMPLES` | Few-shot input→output pairs | Yes — most useful knob for steering behavior |

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
 OUTPUT: {"choice": "reclaim_and_redispatch", ...}
```

LLMs imitate examples more reliably than they follow prose rules. Move the
threshold by moving the example.

### Add stricter blocking criteria

Add a new few-shot example to **EXAMPLES** showing a log tail containing the
phrase you want to treat as a block signal:

```markdown
### Example: mark_goal_blocked (compile error loop)
CONTEXT: skips=6, log_tail=...error[E0277]: the trait bound...
OUTPUT: {"choice": "mark_goal_blocked", "rationale": "engineer is stuck in a type-error loop", "reason": "compile-error-loop"}
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

* The LLM **must** return a JSON object that deserializes into
  `EngineerLifecycleDecision`. The parser tolerates a preamble or trailing
  prose (it slices from the first `{` to the last `}`), but malformed JSON
  or unknown `choice` values cause the brain to log
  `BrainResponseUnparseable` and fall back to `continue_skipping` for that
  cycle.
* If you remove all examples for a given variant, the LLM may stop emitting
  it. Keep at least one example per variant you want reachable.
* The prompt file is loaded via `include_str!`; it must be valid UTF-8 and
  small enough to embed in the binary without bloat. Keep it under ~32 KB
  as a soft guideline.

## See Also

* [Concept: prompt-driven OODA brain](../concepts/prompt-driven-ooda-brain.md)
* [Reference: `ooda_brain.md` prompt schema](../reference/ooda-brain-prompt.md)
* [Reference: `OodaBrain` API](../reference/ooda-brain-api.md)
