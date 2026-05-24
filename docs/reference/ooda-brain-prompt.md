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
…(text format: DECISION marker and labeled lines)…

# EXAMPLES
### Example: continue_skipping
CONTEXT: …
OUTPUT:
DECISION: continue_skipping
RATIONALE: engineer touched worktree 8s ago; let it cook

### Example: reclaim_and_redispatch
…

### Example: deprioritize
…

### Example: open_tracking_issue
…

### Example: mark_goal_blocked
…

### Example: consider_self_update
…
```

Section headers are part of the contract: `RustyClawdBrain` does not parse
the markdown structurally, but the shipped prompt and tests assume these
headers exist.

> **About `consider_self_update`.** This variant is the only one whose
> side-effect handler can mutate the running daemon binary (it dispatches
> `simard safe-update`). Its example deliberately models a *cautious*
> trigger condition (e.g. "current binary is N hours behind upstream and
> in-flight cycles are quiescent") rather than a generic
> "always update" heuristic — see
> [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md).

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

> **Updated as of [#1980](https://github.com/rysweet/Simard/issues/1980).**
> The wire format is **text-only with a `DECISION:` marker** and labeled
> lines for structured fields. JSON is no longer accepted — the legacy
> JSON path and hybrid prose-plus-JSON form from #1711 have been removed.
> The full specification lives in
> [Reference: text-parsing wire formats](text-parsing-wire-formats.md);
> this section is a quick summary.

**Text marker form:**

```
DECISION: continue_skipping
RATIONALE: engineer touched worktree 8 seconds ago; let it cook
```

**Structured variant (labeled lines):**

```
DECISION: open_tracking_issue
TITLE: Engineer panics on goal X
BODY: Repro: spawn engineer, wait 30s, observe panic in tail.
RATIONALE: engineer panic recurred across 3 spawns
```

All variants accept an optional `RATIONALE:` label. If absent, non-labeled
lines after the DECISION line are concatenated as the rationale.

### `continue_skipping`

```
DECISION: continue_skipping
RATIONALE: engineer made progress 12s ago
```

### `reclaim_and_redispatch`

```
DECISION: reclaim_and_redispatch
REDISPATCH_CONTEXT: Previous engineer attempted X; log tail showed Y; please retry with Z.
RATIONALE: worktree idle 7h, no log activity
```

`redispatch_context` is appended to the engineer task description on respawn.

### `deprioritize`

```
DECISION: deprioritize
RATIONALE: 20 skips, 8 failures — reduce priority -10
```

Side-effect: `state.goal_priorities[goal_id] -= 10` (saturating).

### `open_tracking_issue`

```
DECISION: open_tracking_issue
TITLE: Engineer panics on goal X
BODY: Repro: spawn engineer, wait 30s, observe panic in tail.
RATIONALE: log shows panic recurring across 3 spawns
```

Side-effect: appends a record to `<state_root>/pending_issues.jsonl`, a new
on-disk queue introduced by this feature. A follow-up OODA action will drain
the queue and run `gh issue create --label ooda-stuck`; until that lands the
file is a write-only audit trail.

### `mark_goal_blocked`

```
DECISION: mark_goal_blocked
REASON: awaiting-human-decision
RATIONALE: engineer log states 'requires human decision'
```

Side-effect: `state.blocked_goals.insert(goal_id, reason)`.

### `consider_self_update`

```
DECISION: consider_self_update
RATIONALE: current daemon is 6h behind upstream main; in-flight cycles quiescent for 4 minutes; safe-update window open
```

Side-effect: `apply_lifecycle_decision` invokes the `simard safe-update`
path (see `src/operator_cli/safe_update.rs`), which rebuilds, drains
in-flight cycles, hot-swaps the binary, and verifies the new daemon is
responsive. This is the **only** lifecycle variant that can mutate the
running daemon binary; the prompt's `# OPTIONS` section should describe
it as a "last-resort, quiescent-only" choice and gate it on the
`{{cycle_number}}` / `{{worktree_mtime_secs_ago}}` placeholders rather
than firing on every cycle.

## Parser Rules

`parse_decision_from_response` (in `src/ooda_brain/rustyclawd.rs`) uses the
DECISION marker as its **sole** parser:

1. **Find the DECISION marker.** The first non-blank line matching
   `DECISION:` (case-insensitive on the keyword `DECISION`) is found.
   The variant token after the colon is matched exact-snake-case against
   the `EngineerLifecycleDecision` whitelist.
2. **Extract labeled fields.** Remaining lines are scanned for labeled
   fields (`TITLE:`, `BODY:`, `REASON:`, `REDISPATCH_CONTEXT:`,
   `RATIONALE:`). Labels are matched case-insensitively.
3. **Collect rationale.** If no `RATIONALE:` label is found, non-labeled
   lines after the DECISION line are concatenated as the rationale.
4. **Construct the decision.** The text-parsed fields are assembled into
   a `serde_json::Value::Object` and deserialized into
   `EngineerLifecycleDecision` via `serde_json::from_value`. This is
   **not** parsing LLM output — it is deserializing a controlled
   construction from text-parsed fields. A `// SAFETY:` comment marks
   this call.

If no `DECISION:` marker is found, the parser returns
`SimardError::BrainResponseUnparseable { raw, source }`. The JSON
fallback path (legacy `find('{')..rfind('}')` extraction) and the
hybrid prose-plus-JSON form have been removed as of #1980.

Failures produce `SimardError::BrainResponseUnparseable { raw, source }`,
logged at warn level with the **full raw response text** embedded
(truncated to `MAX_RAW_LOG_BYTES = 8192` and rendered with `{:?}` so
control characters are escaped). The caller falls back to the
deterministic skip outcome for that cycle.

The complete behavior matrix — including UTF-8 hardening, marker-injection
defenses, and per-variant field requirements — is documented in
[Reference: OODA Brain Decision Protocol](ooda-brain-decision-protocol.md).

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
   on any new fields. The variant token (snake_case) is automatically part
   of the `DECISION:` marker whitelist via serde's tag derivation — the
   parser has no parallel hand-maintained list (SR-6 in the
   [protocol reference](ooda-brain-decision-protocol.md#security)).
2. Add a side-effect handler arm in
   `src/ooda_actions/advance_goal/lifecycle.rs::apply_lifecycle_decision`.
3. Add an example to `# EXAMPLES` (and a corresponding section to
   [Reference: OODA Brain Decision Protocol → Examples](ooda-brain-decision-protocol.md#examples)
   if it has non-trivial structured fields).
4. Add a row to the
   [Behavior matrix](ooda-brain-decision-protocol.md#behavior-matrix) in
   the protocol reference covering the new variant's marker-only,
   marker-with-labeled-lines, and missing-required-fields cases.
5. Add the matching `#[test]` function(s) to `src/ooda_brain/tests.rs`,
   numbered as the next available `Tn`. Behavior-matrix rows and tests
   must stay 1:1.
6. Update the variant count and table in
   [Reference: `OodaBrain` API → Decision](ooda-brain-api.md#decision).

## See Also

* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md)
* [Concept: prompt-driven OODA brain](../concepts/prompt-driven-ooda-brain.md)
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md)
* [Reference: `OodaBrain` API](ooda-brain-api.md)
* [Reference: OODA Brain Decision Protocol](ooda-brain-decision-protocol.md)
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
* [How-to: diagnose brain decision parse failures](../howto/diagnose-brain-decision-parse-failures.md)
