---
title: RecipeBrain API reference
description: Public API for the unified RecipeBrain struct and its standalone parse functions.
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/unified-recipe-brain.md
  - ../concepts/text-based-brain-protocol.md
  - ./ooda-brain-api.md
  - ./ooda-brain-decision-protocol.md
  - ./text-parsing-wire-formats.md
  - ./recipe-brain-verdict-parsing.md
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

Invokes `recipe-runner-rs` with `--output-format json` and context vars
`goal_id`, `urgency`, `reason` (plus the ladder rung's `escalation_note`),
extracts the agent decision text from the JSON envelope via
`extract_recipe_decision_output`, and parses it via `parse_action_outcome` (the
outcome-classifying variant; `parse_action_from_text` is the thin decision-only
wrapper).

> **Fixed in [#2421](https://github.com/rysweet/Simard/issues/2421).** This call
> site reads the agent decision from the `--output-format json` envelope
> (`step_results[].output`), never the default text-mode summary banner. On a
> parse-miss it runs `run_brain_ladder` (the shared escalation ladder) and only
> after the ladder is exhausted falls back **loudly** to `AdvanceGoal` (via
> `default_advance_goal`), classified `DefaultEmpty` / `DefaultMalformed` —
> distinct from a genuine LLM `advance_goal`. Each invocation emits a
> `brain_verdict_parsed_total` metric (`phase=decide`). See
> [Recipe-brain verdict/decision parsing](./recipe-brain-verdict-parsing.md).

#### `OodaOrientBrain::judge_orientation`

```rust
fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment>
```

Invokes `recipe-runner-rs` with `--output-format json` and context vars
`goal_id`, `base_urgency`, `base_reason`, `failure_count` (plus the ladder
rung's `escalation_note`), extracts the agent output from the JSON envelope via
`extract_recipe_decision_output`, and parses it via `parse_orient_outcome` (the
outcome-classifying variant; `parse_orient_from_text` is the thin wrapper).

> **Fixed in [#2421](https://github.com/rysweet/Simard/issues/2421).** Because
> urgency is parsed from the JSON envelope's agent output rather than the
> text-mode banner, the banner's `(0.0s)` timing string can no longer be scraped
> as `adjusted_urgency` — urgency `0.0` from a banner can no longer happen. On a
> parse-miss it runs `run_brain_ladder` and then falls back to the deterministic
> urgency **floor** (`base_urgency − 0.2 × failure_count`, clamped), the only
> fallback. Each invocation emits a `brain_verdict_parsed_total` metric
> (`phase=orient`). See
> [Recipe-brain verdict/decision parsing](./recipe-brain-verdict-parsing.md).

#### `OodaBrain::decide_engineer_lifecycle`

```rust
fn decide_engineer_lifecycle(
    &self,
    ctx: &EngineerLifecycleCtx,
) -> SimardResult<EngineerLifecycleDecision>
```

Invokes `recipe-runner-rs` with the full lifecycle context as `-c` vars and
**`--output-format json`**, then extracts the agent's decision text from the
final `step_results[].output` of the JSON envelope before running first-word
extraction via `parse_lifecycle_outcome()`.

> **Fixed in [#2419](https://github.com/rysweet/Simard/issues/2419):** This
> call site previously read recipe-runner-rs's **default `text` output**,
> which prints only a human summary banner (`Recipe: … SUCCESS …`) to stdout —
> the agent's decision text is not on stdout in text mode. First-word
> extraction therefore always saw `Recipe:`, matched no variant, and silently
> defaulted to `ContinueSkipping` on ~99.6% of invocations, so non-default
> decisions (`reclaim_and_redispatch`, `deprioritize`, …) never fired. The fix
> switches to `--output-format json` + envelope extraction, mirroring the
> already-correct `disk_health.rs` path.

Every invocation emits one `brain_lifecycle_decision` metric event (see
[Lifecycle decision metric](#lifecycle-decision-metric)) so the parse-failure
rate is measurable from `metrics.jsonl`.

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

`parse_action_from_text` is the decision-only thin wrapper over
`parse_action_outcome(text) -> (DecideJudgment, LifecycleParseOutcome)`, which
additionally returns a `LifecycleParseOutcome` classifying *how* the decision
was produced (`Parsed` vs `DefaultEmpty` / `DefaultMalformed`). `judge_decision`
calls `parse_action_outcome` so the parse-failure rate is measurable and the
escalation ladder fires only on a real miss.

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

`parse_orient_from_text` is the thin wrapper over `parse_orient_outcome(text,
base_urgency, failure_count) -> (OrientJudgment, LifecycleParseOutcome)`, which
additionally returns a `LifecycleParseOutcome`. `judge_orientation` calls
`parse_orient_outcome` over the JSON-envelope-extracted agent output, so the
deterministic floor is the only fallback and the ladder fires on a real miss.

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

### `parse_lifecycle_outcome`

```rust
pub fn parse_lifecycle_outcome(
    text: &str,
) -> (EngineerLifecycleDecision, LifecycleParseOutcome)
```

Canonical lifecycle parser (added in
[#2419](https://github.com/rysweet/Simard/issues/2419)). Identical first-word
extraction to `parse_lifecycle_from_text`, but additionally returns a
`LifecycleParseOutcome` classifying *how* the decision was produced:

| Variant | Meaning |
|---------|---------|
| `Parsed` | First word matched a known variant — a real decision. |
| `DefaultEmpty` | Output was empty/whitespace → defaulted to `ContinueSkipping`. |
| `DefaultMalformed` | Output non-empty but first word matched no variant → defaulted. |
| `Error` | recipe-runner invocation/envelope decode failed (set on the error path, not by the pure parser). |

`LifecycleParseOutcome::is_parse_failure()` is `true` for everything except
`Parsed`. This split is what makes the parse-failure rate measurable —
previously a genuine `continue_skipping` decision and a silent fallback were
indistinguishable. `parse_lifecycle_from_text` is the decision-only wrapper
over this function.

### Lifecycle decision metric

`decide_engineer_lifecycle` records one `brain_lifecycle_decision` metric
(value `1.0`) per invocation via `self_metrics::record_metric`. The context
JSON carries:

| Field | Description |
|-------|-------------|
| `goal_id` | Goal under inspection. |
| `outcome` | `parsed` \| `default_empty` \| `default_malformed` \| `error`. |
| `is_parse_failure` | `true` for any `outcome != "parsed"` (the numerator). |
| `first_word` | First whitespace-delimited token of the decision text (bounded). |
| `consecutive_skip_count` | Skip streak for this goal at decision time. |
| `decision` | Resulting `EngineerLifecycleDecision` choice tag. |
| `cause` | `ok` on the happy path; `spawn_failed` / `nonzero_exit` / `envelope_decode_failed` on the error path. |

Parse-failure rate over a window:
`count(is_parse_failure == true) / count(*)`.

---

## Shared helpers

### `resolve_recipe_path`

```rust
pub fn resolve_recipe_path(
    repo_root: &Path,
    recipe_filename: &str,
    home_override: Option<&Path>,
) -> Option<PathBuf>
```

Resolution order:

1. `<home>/.simard/prompt_assets/simard/recipes/<recipe_filename>` (hot-reload)
2. `<repo_root>/prompt_assets/simard/recipes/<recipe_filename>` (in-tree)

`home_override` selects the hot-reload base directory: `Some(path)` uses `path`
(a test seam to stay hermetic against the ambient `~/.simard`), `None` falls
back to [`dirs::home_dir`] — production always passes `None` (via
`RecipeBrain::new`). Mirrors the `home_override` convention already used by
`disk_health::resolve_recipe_path` and `brain_introspection::resolve_recipe_path`.

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
