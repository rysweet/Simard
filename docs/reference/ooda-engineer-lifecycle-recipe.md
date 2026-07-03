# Reference: OODA Engineer Lifecycle Recipe and Prompt Schema

Recipe: `prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml`
Prompt source: `prompt_assets/simard/ooda_brain.md` (content embedded in recipe YAML)
Shim: `src/ooda_reasoners/recipe_engineer_lifecycle.rs`

This is the single source of truth for the engineer-lifecycle decision at the
Act phase's skip branch. The engineer-lifecycle brain runs as a **recipe step**
via `recipe-runner-rs`, following the same pattern as `ooda-decide.yaml`,
`ooda-orient.yaml`, `progress-assessment.yaml`, and
`merge-readiness-judge.yaml`.

> **History:** Before issue
> [#2115](https://github.com/rysweet/Simard/issues/2115), the engineer
> lifecycle brain was `RustyClawdActReasoner`, which compiled the prompt via
> `include_str!`, submitted it to an `LlmSubmitter`, and parsed the response
> using `DECISION:` markers on the first non-blank line. The recipe-based
> approach moves the prompt to a YAML file that can be edited without a
> rebuild, and uses keyword scanning (the same protocol as the decide brain)
> to extract the lifecycle variant from the agent's prose output.

## Recipe Layout

```yaml
name: ooda-engineer-lifecycle
description: OODA Act brain — engineer lifecycle decisions
context:
  goal_id: ""
  goal_description: ""
  cycle_number: ""
  consecutive_skip_count: ""
  failure_count: ""
  worktree_path: ""
  worktree_mtime_secs_ago: ""
  sentinel_pid: ""
  commits_behind: ""
  in_flight_engineer_count: ""
  minutes_since_last_update_attempt: ""
  last_engineer_log_tail: ""
steps:
  - name: decide-lifecycle
    type: agent
    prompt: |
      # OODA Brain — Engineer Lifecycle Decision

      ## ROLE
      …

      ## CONTEXT
      - goal_id: {{goal_id}}
      - goal_description: {{goal_description}}
      - cycle_number: {{cycle_number}}
      …(12 context fields total)…

      ## OPTIONS
      …(6 variant tags)…

      ## EXAMPLES
      …(text-format examples)…
```

The recipe is a single `agent` step. The recipe-runner-rs subprocess handles
prompt rendering, agent invocation, and stdout capture. The Rust shim
(`RecipeEngineerLifecycleReasoner`) parses the stdout using keyword scanning.

### What changed from `ooda_brain.md`

The recipe prompt preserves all content from the original `ooda_brain.md`
**except**:

- **Placeholders converted** — all 12 `{var}` → `{{var}}` for Handlebars
  templating.
- **OUTPUT_FORMAT section removed** — the `DECISION:` marker protocol
  instructions are removed. The keyword scanner finds the lifecycle variant
  in natural prose, like the decide brain.
- **Special value rendering** — `sentinel_pid` renders as `"<none>"` when
  `None`. `minutes_since_last_update_attempt` renders as `"never"` when
  `u64::MAX`.

The ROLE, CONTEXT, OPTIONS, and EXAMPLES sections are preserved. Examples
are adapted to show prose-form output instead of `DECISION:` marker format.

## Placeholders (Context Variables)

The recipe-runner-rs performs Handlebars `{{name}}` substitution from the
context variables passed by `RecipeEngineerLifecycleReasoner`.

| Variable | Type | Source |
|---|---|---|
| `{{goal_id}}` | string | `ctx.goal_id` |
| `{{goal_description}}` | string | `ctx.goal_description` |
| `{{cycle_number}}` | string (u32) | `ctx.cycle_number` |
| `{{consecutive_skip_count}}` | string (u32) | `ctx.consecutive_skip_count` |
| `{{failure_count}}` | string (u32) | `ctx.failure_count` |
| `{{worktree_path}}` | string (PathBuf) | `ctx.worktree_path` |
| `{{worktree_mtime_secs_ago}}` | string (u64) | `ctx.worktree_mtime_secs_ago` |
| `{{sentinel_pid}}` | string | `ctx.sentinel_pid` — renders as `"<none>"` when `None` |
| `{{commits_behind}}` | string (u32) | `ctx.commits_behind` |
| `{{in_flight_engineer_count}}` | string (u32) | `ctx.in_flight_engineer_count` |
| `{{minutes_since_last_update_attempt}}` | string | `ctx.minutes_since_last_update_attempt` — renders as `"never"` when `u64::MAX` |
| `{{last_engineer_log_tail}}` | string | `ctx.last_engineer_log_tail` — last ~8 KB, secrets redacted |

## Keyword Scanning

`RecipeEngineerLifecycleReasoner` uses keyword scanning (the same **keyword
verdict protocol** as `RecipeDecideReasoner`) to extract the lifecycle variant
from the agent's stdout. The agent's prose is scanned case-insensitively
for one of the 6 lifecycle variant keywords.

### Keywords

| Keyword | Maps to | Required extra fields |
|---------|---------|----------------------|
| `continue_skipping` | `EngineerLifecycleDecision::ContinueSkipping` | _(none)_ |
| `reclaim_and_redispatch` | `EngineerLifecycleDecision::ReclaimAndRedispatch` | `redispatch_context` |
| `deprioritize` | `EngineerLifecycleDecision::Deprioritize` | _(none)_ |
| `open_tracking_issue` | `EngineerLifecycleDecision::OpenTrackingIssue` | `title`, `body` |
| `mark_goal_blocked` | `EngineerLifecycleDecision::MarkGoalBlocked` | `reason` |
| `consider_self_update` | `EngineerLifecycleDecision::ConsiderSelfUpdate` | _(none)_ |

### Extra field extraction

For variants that carry extra fields (`reclaim_and_redispatch`,
`open_tracking_issue`, `mark_goal_blocked`), the parser attempts to
extract values from labeled lines in the agent's output:

```
TITLE: Engineer stuck in compile-error loop
BODY: The engineer has failed for 6 consecutive cycles.
REASON: ANTHROPIC_API_KEY not set in daemon environment
REDISPATCH_CONTEXT: Previous engineer was stuck on type errors.
```

Labels are matched case-insensitively. If a required labeled line is
missing, defaults are applied:

| Field | Default |
|-------|---------|
| `redispatch_context` | `""` (empty string) |
| `title` | `"OODA stuck"` |
| `body` | First 500 chars of agent output |
| `reason` | First 500 chars of agent output |

### Default on no keyword

If no lifecycle keyword is found in the output, the parser returns
`ContinueSkipping` — the safe default. This matches the
`DeterministicFallbackActReasoner` behavior.

### Keyword safety

Unlike the decide brain (where no keyword is a substring of another),
the lifecycle keywords require no substring disambiguation — none of the
6 keywords overlap. The scan order does not matter for correctness.

## Error Handling

`RecipeEngineerLifecycleReasoner` returns
`Err(SimardError::AdapterInvocationFailed)` when:

- The `recipe-runner-rs` binary is not found (construction fails;
  `RecipeEngineerLifecycleReasoner::new()` returns `None`).
- The subprocess exits with a non-zero status.
- The subprocess cannot be spawned.

On subprocess success, the keyword scanner **never fails** — it always
produces a valid `EngineerLifecycleDecision`. The `ContinueSkipping`
default is the unconditional safety net.

On `AdapterInvocationFailed`, the caller in `dispatch_spawn_engineer`
falls back to `DeterministicFallbackActReasoner` and logs the error.

## Runtime Loading (not compile-time)

Unlike the old `ooda_brain.md` (which was embedded via `include_str!`),
the engineer lifecycle recipe is loaded at runtime by recipe-runner-rs.
`RecipeEngineerLifecycleReasoner` resolves the recipe path in this order:

1. `~/.simard/prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml`
   (hot-reload)
2. `{repo_root}/prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml`
   (in-tree)

Prompt edits take effect on the next daemon cycle **without a rebuild**.

## Construction Pattern

```rust
let brain: Arc<dyn ActReasoner> = match RecipeEngineerLifecycleReasoner::new(repo_root) {
    Some(b) => {
        daemon_log(state_root, "[simard] OODA daemon: brain = RecipeEngineerLifecycleReasoner");
        Arc::new(b)
    }
    None => {
        record_fallback(state_root, "act", "recipe-runner-rs or recipe YAML not available");
        Arc::new(DeterministicFallbackActReasoner)
    }
};
```

`RecipeEngineerLifecycleReasoner::new(repo_root)` returns `None` when:
- The `recipe-runner-rs` binary is not on `$PATH`.
- The recipe YAML file does not exist at either resolution path.

The daemon wiring in `operator_commands_ooda/daemon/brains.rs` calls
`build_act_reasoner(state_root, repo_root)`, which tries
`RecipeEngineerLifecycleReasoner` first and falls back to
`DeterministicFallbackActReasoner`.

## Examples

### Agent output → `continue_skipping`

```
The engineer's worktree was modified 8 seconds ago and the log tail shows
active commit activity. This engineer is making healthy progress.

The appropriate action is continue_skipping.
```

Parser result:
```rust
EngineerLifecycleDecision::ContinueSkipping {
    rationale: "The engineer's worktree was modified 8 seconds ago…",
}
```

### Agent output → `reclaim_and_redispatch`

```
The worktree has been idle for 7 hours (worktree_mtime_secs_ago=25200)
and the log tail trails off mid-tool-call. The engineer is wedged.

The decision is reclaim_and_redispatch.

REDISPATCH_CONTEXT: Previous engineer hung during file edit. Start by
re-reading the goal and pick a fresh approach.
```

Parser result:
```rust
EngineerLifecycleDecision::ReclaimAndRedispatch {
    rationale: "The worktree has been idle for 7 hours…",
    redispatch_context: "Previous engineer hung during file edit. Start by re-reading the goal and pick a fresh approach.",
}
```

### Agent output → `open_tracking_issue`

```
The log tail shows a recurring panic: `thread 'main' panicked at 'unwrap
on None'`. This has persisted across 3 engineer spawns.

My recommendation is open_tracking_issue.

TITLE: Engineer panics on goal improve-test-coverage
BODY: The engineer has panicked 3 times with the same unwrap error. See
agent_logs/engineer-improve-test-coverage-*.log for stack traces.
```

Parser result:
```rust
EngineerLifecycleDecision::OpenTrackingIssue {
    rationale: "The log tail shows a recurring panic…",
    title: "Engineer panics on goal improve-test-coverage",
    body: "The engineer has panicked 3 times…",
}
```

### Agent output → `mark_goal_blocked`

```
The log shows repeated 401 errors and "ANTHROPIC_API_KEY not set" messages.
The engineer cannot make API calls without credentials.

I recommend mark_goal_blocked.

REASON: ANTHROPIC_API_KEY not set in daemon environment
```

Parser result:
```rust
EngineerLifecycleDecision::MarkGoalBlocked {
    rationale: "The log shows repeated 401 errors…",
    reason: "ANTHROPIC_API_KEY not set in daemon environment",
}
```

## Comparison with the old `DECISION:` marker protocol

| Aspect | Old (`RustyClawdActReasoner`) | New (`RecipeEngineerLifecycleReasoner`) |
|--------|------------------------|-------------------------------------|
| Prompt location | `include_str!` (compiled in) | Recipe YAML (runtime) |
| Prompt edit cycle | Edit → `cargo build` → `safe-update` | Edit YAML → next cycle |
| Output format | `DECISION:` on first non-blank line | Keyword anywhere in prose |
| Extra fields | `TITLE:`, `BODY:` labeled lines | Same labeled lines (extracted after keyword match) |
| Parse failure mode | `ReasonerResponseUnparseable` error | Always succeeds (defaults to `ContinueSkipping`) |
| Structured variants | Marker-wins precedence | Keyword + labeled lines |

## Test Inventory

`src/ooda_reasoners/recipe_engineer_lifecycle.rs` contains inline
`#[cfg(test)]` tests covering all variants and parsing edge cases:

| Test | Coverage |
|------|----------|
| All 6 keywords recognized | Each keyword in prose → correct variant |
| Case insensitive | `CONTINUE_SKIPPING`, `Continue_Skipping` → `ContinueSkipping` |
| No keyword → default | Prose without any keyword → `ContinueSkipping` |
| Empty output → default | Empty string → `ContinueSkipping` |
| Labeled field extraction | `TITLE:`, `BODY:`, `REASON:`, `REDISPATCH_CONTEXT:` parsed |
| Missing labeled fields → defaults | `open_tracking_issue` without `TITLE:` → `"OODA stuck"` |
| Multiple keywords → first match | Scan order determines winner |
| Keyword embedded in prose | `…should reclaim_and_redispatch…` detected |
| Rationale truncation | Long output truncated to 500 chars |

## Versioning & Compatibility

Adding a new lifecycle variant (a new `EngineerLifecycleDecision` enum
value) requires a coordinated change:

1. Add the variant to `EngineerLifecycleDecision` in `src/ooda_reasoners/mod.rs`.
2. Add the keyword to the scanner in
   `src/ooda_reasoners/recipe_engineer_lifecycle.rs`.
3. Add the variant to the `OPTIONS` section in the recipe YAML.
4. Add an example to the `EXAMPLES` section.
5. Add `apply_decision_to_state` handling in `src/ooda_reasoners/mod.rs`.
6. Add a test covering the new keyword.
7. Update the variant table in
   [text-parsing wire formats](text-parsing-wire-formats.md).

Cosmetic edits (rationale guidance, examples, ROLE phrasing) are safe to
ship alone — and take effect without a rebuild.

## See Also

* [Reference: `ooda_brain.md` prompt schema](ooda-brain-prompt.md) — historical prompt schema (superseded by recipe)
* [Reference: OODA Brain Decision Protocol](ooda-brain-decision-protocol.md) — historical wire format (superseded by keyword scanning)
* [Reference: OODA decide recipe and prompt schema](ooda-decide-prompt.md) — decide-phase recipe
* [Reference: OODA orient recipe and prompt schema](ooda-orient-recipe.md) — orient-phase recipe
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md) — normative grammar
* [Reference: `ActReasoner` API](ooda-brain-api.md) — trait and type definitions
* [Concept: prompt-driven OODA brain](../concepts/prompt-driven-ooda-brain.md) — concept overview
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — editing guide
* [Reference: recipe context variable sanitization](recipe-context-var-sanitization.md) — `sanitize_context_var` helper that strips newlines from context values before passing to recipe-runner-rs
* [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
