---
title: RecipeBrain API reference
description: Public API for the unified RecipeBrain struct and its standalone parse functions.
last_updated: 2026-05-27
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/unified-recipe-brain.md
  - ../concepts/text-based-brain-protocol.md
  - ./ooda-brain-api.md
  - ./ooda-brain-decision-protocol.md
  - ./text-parsing-wire-formats.md
---

# RecipeBrain API reference

Module: `src/ooda_brain/recipe_brain.rs`

## RecipeBrain

```rust
pub struct RecipeBrain {
    recipe_path: PathBuf,
    agent_binary: &'static str,
    adapter_tag: &'static str,
}
```

### `RecipeBrain::new`

```rust
pub fn new(
    repo_root: &Path,
    recipe_filename: &str,
    adapter_tag: &'static str,
) -> Option<Self>
```

Constructs a `RecipeBrain` if all preconditions are met. Returns `None` when:

- The recipe YAML file is not found at either resolution path.
- `LlmProvider::resolve_agent_binary()` returns `None` (config unavailable).
- `recipe-runner-rs --version` fails (binary not on `$PATH`).

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `repo_root` | `&Path` | Repository root. Used as fallback for in-tree recipe resolution. |
| `recipe_filename` | `&str` | YAML filename (e.g. `"ooda-decide.yaml"`). Appended to the recipe directory path. |
| `adapter_tag` | `&'static str` | Human-readable identifier for error messages (e.g. `"recipe-decide-brain"`). |

**Standard instances:**

| Phase | `recipe_filename` | `adapter_tag` |
|-------|-------------------|---------------|
| Decide | `"ooda-decide.yaml"` | `"recipe-decide-brain"` |
| Orient | `"ooda-orient.yaml"` | `"recipe-orient-brain"` |
| Act (lifecycle) | `"ooda-engineer-lifecycle.yaml"` | `"recipe-engineer-lifecycle-brain"` |

### Trait implementations

`RecipeBrain` implements three traits simultaneously:

#### `OodaDecideBrain::judge_decision`

```rust
fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment>
```

Invokes `recipe-runner-rs` with context vars `goal_id`, `urgency`, `reason`.
Parses stdout via `parse_action_from_text()`.

#### `OodaOrientBrain::judge_orientation`

```rust
fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment>
```

Invokes `recipe-runner-rs` with context vars `goal_id`, `base_urgency`,
`base_reason`, `failure_count`. Parses stdout via
`parse_orient_from_text()`.

#### `OodaBrain::decide_engineer_lifecycle`

```rust
fn decide_engineer_lifecycle(
    &self,
    ctx: &EngineerLifecycleCtx,
) -> SimardResult<EngineerLifecycleDecision>
```

Invokes `recipe-runner-rs` with the full lifecycle context as `-c` vars.
Parses stdout via `parse_lifecycle_from_text()`.

---

## Standalone parse functions

These are public, pure functions. They take recipe stdout text and return
typed judgments. No struct dependency — usable in tests and other contexts.

### `parse_action_from_text`

```rust
pub fn parse_action_from_text(text: &str) -> DecideJudgment
```

Scans for any of 10 action keywords (`advance_goal`, `consolidate_memory`,
`run_improvement`, `poll_developer_activity`, `extract_ideas`, `safe_update`,
`research_query`, `run_gym_eval`, `build_skill`, `launch_session`) using
case-insensitive ASCII matching. Returns the first match with the agent's
prose as rationale (truncated to 500 chars). Defaults to `AdvanceGoal` if no
keyword found.

### `parse_orient_from_text`

```rust
pub fn parse_orient_from_text(
    text: &str,
    base_urgency: f64,
    failure_count: u32,
) -> OrientJudgment
```

3-tier parse cascade:

1. JSON extraction — `{"adjusted_urgency": f64, ...}` via serde.
2. Bare float — regex-free decimal scan.
3. Deterministic floor — `base_urgency - 0.2 * failure_count`, clamped to 0.

### `parse_lifecycle_from_text`

```rust
pub fn parse_lifecycle_from_text(text: &str) -> EngineerLifecycleDecision
```

2-tier parse:

1. `DECISION:` marker on first non-blank line → extract variant + labeled fields.
2. Keyword scan for 6 lifecycle variant names (`continue_skipping`,
   `reclaim_and_redispatch`, `deprioritize`, `open_tracking_issue`,
   `mark_goal_blocked`, `consider_self_update`).
3. Default: `ContinueSkipping`.

---

## Shared helpers

### `resolve_recipe_path`

```rust
fn resolve_recipe_path(repo_root: &Path, recipe_filename: &str) -> Option<PathBuf>
```

Resolution order:

1. `~/.simard/prompt_assets/simard/recipes/<recipe_filename>` (hot-reload)
2. `<repo_root>/prompt_assets/simard/recipes/<recipe_filename>` (in-tree)

Returns `None` if neither path contains the file.

### `truncate`

```rust
fn truncate(s: &str, max: usize) -> String
```

Char-aware truncation. Appends `…` when truncated. Safe on multi-byte UTF-8.

### `ascii_contains_ignore_case`

```rust
fn ascii_contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool
```

Byte-level sliding window with `eq_ignore_ascii_case`. No regex, no allocations.
