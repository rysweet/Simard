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

The prompt has five fixed sections. The brain parser looks for `DECISION:`
marker lines (not JSON) — the `OUTPUT_FORMAT` section instructs the model
to emit text-based responses.

| Section | Purpose | Editable? |
|---|---|---|
| `# ROLE` | Identity & tone for the LLM | Yes |
| `# CONTEXT` | Templated variables Simard substitutes per call | Yes (must keep `{{var}}` placeholders the brain populates — see below) |
| `# OPTIONS` | The six `choice` values the brain may return | **Add/remove only with a Rust enum change**; renaming text guidance is fine |
| `# OUTPUT_FORMAT` | Text format for the response (`DECISION: <variant>`) | **Do not change the marker format**; only edit prose and examples |
| `# EXAMPLES` | Few-shot input→output pairs | Yes — most useful knob for steering behavior |

### Output format

The brain expects responses in the `DECISION:` marker format:

```
DECISION: continue_skipping
RATIONALE: engineer is making progress, worktree modified 30s ago
```

For structured variants, labeled fields follow the decision line:

```
DECISION: open_tracking_issue
TITLE: Engineer stuck in compile-error loop
BODY: The engineer has failed for 6 consecutive cycles with E0277 errors.
RATIONALE: Persistent failure needs human attention.
```

**Do not** instruct the model to emit JSON. The parser does not accept JSON —
it looks for `DECISION:` markers and labeled lines only. See
[text-parsing wire formats § engineer lifecycle](../reference/text-parsing-wire-formats.md#1c-engineer-lifecycle-rustyclawdrs)
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
 DECISION: reclaim_and_redispatch
 REDISPATCH_CONTEXT: Previous engineer was idle for too long. Try fresh approach.
 RATIONALE: 4 skips with stale worktree suggests engineer is stuck.
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
DECISION: mark_goal_blocked
REASON: compile-error-loop
RATIONALE: engineer is stuck in a type-error loop
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

* The LLM **must** return a `DECISION: <variant>` marker line as the first
  non-blank line. The parser scans for this marker and extracts the variant
  token. Responses without a `DECISION:` line trigger
  `BrainResponseUnparseable` and the brain falls back to
  `continue_skipping` for that cycle. The JSON format is no longer accepted.
* For structured variants (`open_tracking_issue`, `mark_goal_blocked`,
  `reclaim_and_redispatch`), the model must emit labeled lines for the
  required fields (`TITLE:`, `BODY:`, `REASON:`, `REDISPATCH_CONTEXT:`).
  Missing required fields use default values.
* If you remove all examples for a given variant, the LLM may stop emitting
  it. Keep at least one example per variant you want reachable.
* The prompt file is loaded via `include_str!`; it must be valid UTF-8 and
  small enough to embed in the binary without bloat. Keep it under ~32 KB
  as a soft guideline.

## Editing the decide and orient prompts

The decide and orient brains have their own prompt files:

- `prompt_assets/simard/ooda_decide.md` — action-kind routing
- `prompt_assets/simard/ooda_orient.md` — failure-penalty demotion

These use the same text-based output formats:

**Decide** uses `DECISION: <variant>`:
```
DECISION: advance_goal
RATIONALE: ordinary goal id with open PR, default routing
```

**Orient** uses labeled lines:
```
ADJUSTED_URGENCY: 0.60
DEMOTION_APPLIED: 0.20
RATIONALE: 1 failure: standard floor demotion
CONFIDENCE: 0.9
```

Edit the `OUTPUT_FORMAT` and `EXAMPLES` sections of each prompt to steer
behavior. Do not use JSON examples — the parser does not accept JSON.

See [text-parsing wire formats](../reference/text-parsing-wire-formats.md)
for the full grammar of each format.

## See Also

* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md)
* [Concept: prompt-driven OODA brain](../concepts/prompt-driven-ooda-brain.md)
* [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md)
* [Reference: `OodaBrain` API](../reference/ooda-brain-api.md)
* [Reference: `ooda_brain.md` prompt schema](../reference/ooda-brain-prompt.md)
