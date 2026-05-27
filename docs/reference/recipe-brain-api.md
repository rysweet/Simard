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

All three parsers use the same **first-word extraction** pattern: split the
output on whitespace, take the first token, match it case-insensitively
against known variants, and default to a safe variant if unrecognized. No
keyword scanning, no JSON extraction, no marker protocols. The recipe YAML
prompts instruct the LLM to output the decision word as the first token.

### `parse_action_from_text`

```rust
pub fn parse_action_from_text(text: &str) -> DecideJudgment
```

Extracts the first non-whitespace word from `text`, lowercases it, and
matches against the 10 action keywords (`advance_goal`,
`consolidate_memory`, `run_improvement`, `poll_developer_activity`,
`extract_ideas`, `safe_update`, `research_query`, `run_gym_eval`,
`build_skill`, `launch_session`). Returns the matching `DecideJudgment`
variant. Defaults to `AdvanceGoal` if no match.

The remaining text after the first word is captured as the rationale
(truncated to 500 chars).

### `parse_orient_from_text`

```rust
pub fn parse_orient_from_text(
    text: &str,
    base_urgency: f64,
    failure_count: u32,
) -> OrientJudgment
```

2-tier parse:

1. **First float** — regex-free decimal scan (`try_first_float`). Finds the
   first substring matching `[0-9]+\.[0-9]+` or `[0-9]+` and parses it as
   `f64`. This becomes `adjusted_urgency`.
2. **Deterministic floor** — `base_urgency - 0.2 * failure_count`, clamped
   to `[0.0, 1.0]`.

The full text is used as `rationale`. `confidence` is always `1.0`.
`OrientJudgment::validate()` enforces bounds after extraction.

> **Removed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The JSON extraction tier (`try_json_extraction`) has been deleted. The
> orient prompt now instructs the LLM to output a bare decimal as its first
> token. No `serde_json::from_str` on LLM output.

### `parse_lifecycle_from_text`

```rust
pub fn parse_lifecycle_from_text(text: &str) -> EngineerLifecycleDecision
```

Extracts the first non-whitespace word from `text`, lowercases it, and
matches against the 6 lifecycle variant names (`continue_skipping`,
`reclaim_and_redispatch`, `deprioritize`, `open_tracking_issue`,
`mark_goal_blocked`, `consider_self_update`). Returns the matching
`EngineerLifecycleDecision` with default extra fields. Defaults to
`ContinueSkipping` if no match.

Extra fields use defaults:
- `open_tracking_issue` → `title: "OODA stuck"`, `body: truncate(remaining_text, 500)`
- `mark_goal_blocked` → `reason: truncate(remaining_text, 500)`
- `reclaim_and_redispatch` → `redispatch_context: ""`
- All variants: `rationale: truncate(remaining_text_after_first_word, 500)`

> **Removed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The `DECISION:` marker parser, keyword scan fallback, labeled-line field
> extraction, and `LIFECYCLE_KEYWORDS` constant have been deleted. The
> lifecycle prompt now instructs the LLM to output the variant name as its
> first word.

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

### `try_first_float`

```rust
fn try_first_float(text: &str) -> Option<f64>
```

Scans `text` for the first substring that looks like a decimal number
(`[0-9]+\.[0-9]+` or bare `[0-9]+`). Returns the parsed `f64` or `None`.
Used by `parse_orient_from_text()` to extract the urgency adjustment.

> **Removed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> `ascii_contains_ignore_case` (byte-level sliding window keyword scanner),
> `try_json_extraction` (JSON `{…}` extraction + serde),
> `parse_with_marker` / `extract_decision_marker` (DECISION: marker line parser),
> `try_keyword_scan` (multi-keyword full-text scan),
> `build_keyword_decision` (decision builder from keyword scan results),
> and `LIFECYCLE_KEYWORDS` (static keyword array) have all been deleted.
> All three parse functions now use trivial first-word/first-float extraction.
