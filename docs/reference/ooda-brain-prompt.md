# Reference: `ooda_brain.md` Prompt Schema

File: `prompt_assets/simard/ooda_brain.md`
Loaded at compile time via `include_str!` from `src/ooda_brain/rustyclawd.rs`.

This is the single source of truth for the engineer-lifecycle decision. Edit
this file to change Simard's behavior; no Rust changes required (rebuild +
daemon restart).

## File Layout

The prompt is a markdown document with five top-level sections, in this
order:

```markdown
# ROLE
…

# CONTEXT
…(uses {{placeholders}})…

# OPTIONS
…

# OUTPUT_FORMAT
…(JSON schema)…

# EXAMPLES
### Example: continue_skipping
CONTEXT: …
OUTPUT: {…}

### Example: reclaim_and_redispatch
…

### Example: deprioritize
…

### Example: open_tracking_issue
…

### Example: mark_goal_blocked
…
```

Section headers are part of the contract: `RustyClawdBrain` does not parse
the markdown structurally, but the shipped prompt and tests assume these
headers exist.

## Placeholders

`RustyClawdBrain` performs literal `{{name}}` → value substitution in
**CONTEXT** before submission. Unknown placeholders are left untouched.

| Placeholder | Type | Source |
|---|---|---|
| `{{goal_id}}` | string | `ctx.goal_id` |
| `{{goal_description}}` | string | `ctx.goal_description` |
| `{{cycle_number}}` | u64 | `ctx.cycle_number` |
| `{{consecutive_skip_count}}` | u32 | `ctx.consecutive_skip_count` |
| `{{failure_count}}` | u32 | `ctx.failure_count` |
| `{{worktree_mtime_secs_ago}}` | u64 | `ctx.worktree_mtime_secs_ago` |
| `{{sentinel_pid}}` | string (`"None"` if absent) | `ctx.sentinel_pid` |
| `{{last_engineer_log_tail}}` | string (≤8 KB, redacted) | `ctx.last_engineer_log_tail` |

## Output Schema

The LLM **must** reply with a single JSON object that deserializes into
`EngineerLifecycleDecision`. The discriminator is `choice`. No prose, no
fences.

### `continue_skipping`

```json
{ "choice": "continue_skipping", "rationale": "engineer made progress 12s ago" }
```

### `reclaim_and_redispatch`

```json
{
  "choice": "reclaim_and_redispatch",
  "rationale": "worktree idle 7h, no log activity",
  "redispatch_context": "Previous engineer attempted X; log tail showed Y; please retry with Z."
}
```

`redispatch_context` is appended to the engineer task description on respawn.

### `deprioritize`

```json
{ "choice": "deprioritize", "rationale": "20 skips, 8 failures — reduce priority -10" }
```

Side-effect: `state.goal_priorities[goal_id] -= 10` (saturating).

### `open_tracking_issue`

```json
{
  "choice": "open_tracking_issue",
  "rationale": "log shows panic recurring across 3 spawns",
  "title": "Engineer panics on goal X",
  "body": "Repro: …\nLog tail: …"
}
```

Side-effect: appends a record to `<state_root>/pending_issues.jsonl`, a new
on-disk queue introduced by this feature. A follow-up OODA action will drain
the queue and run `gh issue create --label ooda-stuck`; until that lands the
file is a write-only audit trail.

### `mark_goal_blocked`

```json
{
  "choice": "mark_goal_blocked",
  "rationale": "engineer log states 'requires human decision'",
  "reason": "awaiting-human-decision"
}
```

Side-effect: `state.blocked_goals.insert(goal_id, reason)`.

## Parser Rules

`RustyClawdBrain::parse_response()` performs:

1. Trim whitespace.
2. Slice from the first `{` to the last `}` (tolerates a preamble or
   trailing prose, though the prompt instructs against it).
3. `serde_json::from_str::<EngineerLifecycleDecision>(slice)`.

Failures produce `SimardError::BrainResponseUnparseable { raw, source }`,
logged at warn level. The caller falls back to the deterministic skip outcome
for that cycle.

## Compile-Time Embedding

The prompt is embedded with `include_str!`, so:

* It must exist at build time and be valid UTF-8 (otherwise the build fails
  with the standard `include_str!` error).
* Its size becomes part of the binary. Keep it under ~32 KB as a soft
  guideline to avoid bloat.

The five top-level headers (`# ROLE`, `# CONTEXT`, `# OPTIONS`,
`# OUTPUT_FORMAT`, `# EXAMPLES`) are conventions, not enforced by the
compiler. The shipped prompt and the unit tests in `src/ooda_brain/tests.rs`
assume they are present; removing them will break the prompt's effectiveness
even though it will still compile.

## Secret Redaction

Before substitution into `{{last_engineer_log_tail}}`, `redact_secrets()`
replaces values matching the case-insensitive regex
`(token|key|secret|password|bearer)\s*[:=]\s*\S+` with
`<placeholder>: ***`. Redaction is best-effort; do not rely on it for
adversarial scenarios.

## Versioning & Compatibility

The prompt file is **not** versioned in its body. Semantic changes (adding a
new `choice` value, changing a field name) require a coordinated Rust change
to `EngineerLifecycleDecision`. Cosmetic edits (rationale tone, examples,
ROLE phrasing) are safe to ship alone.

When adding a new variant:

1. Add the variant to `EngineerLifecycleDecision` with `#[serde(default)]`
   on any new fields.
2. Add a side-effect handler arm in
   `src/ooda_actions/advance_goal/lifecycle.rs::apply_lifecycle_decision`.
3. Add an example to `# EXAMPLES`.
4. Add a parse round-trip test to `src/ooda_brain/tests.rs`.

## See Also

* [Concept: prompt-driven OODA brain](../concepts/prompt-driven-ooda-brain.md)
* [Reference: `OodaBrain` API](ooda-brain-api.md)
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
