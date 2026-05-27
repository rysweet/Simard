---
title: Unified RecipeBrain
description: One struct, three OODA phases — how RecipeBrain replaced RecipeDecideBrain, RecipeOrientBrain, and RecipeEngineerLifecycleBrain with a single parameterized type.
last_updated: 2026-05-27
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ./text-based-brain-protocol.md
  - ./prompt-driven-ooda-brain.md
  - ../reference/ooda-brain-api.md
  - ../reference/ooda-brain-decision-protocol.md
  - ../howto/edit-the-ooda-brain-prompt.md
---

# Unified RecipeBrain

Simard's three OODA phases — **decide**, **orient**, and **act**
(engineer-lifecycle) — each delegate their LLM call to `recipe-runner-rs`
executing a phase-specific recipe YAML. Before this consolidation, each phase
had its own struct (`RecipeDecideBrain`, `RecipeOrientBrain`,
`RecipeEngineerLifecycleBrain`). They were copy-pasted from each other: same
constructor, same `resolve_recipe_path`, same `truncate()` helper, same
`ascii_contains_ignore_case()` helper. The only differences were the recipe
filename, the adapter tag for error messages, and the trait impl body.

`RecipeBrain` is a single struct that takes the recipe filename and adapter tag
as constructor parameters. It implements all three brain traits.

## Principle

> One agent, one identity, one brain — different recipes for different
> circumstances.

The struct is the brain. The recipe YAML is the circumstance. Duplicating the
struct for each recipe violates this principle the same way duplicating a
function for each argument value would.

## Structure

```rust
pub struct RecipeBrain {
    recipe_path: PathBuf,
    agent_binary: &'static str,
    adapter_tag: &'static str,
}
```

Construction:

```rust
RecipeBrain::new(repo_root, "ooda-decide.yaml", "recipe-decide-brain")
RecipeBrain::new(repo_root, "ooda-orient.yaml", "recipe-orient-brain")
RecipeBrain::new(repo_root, "ooda-engineer-lifecycle.yaml", "recipe-engineer-lifecycle-brain")
```

All three calls return `Option<RecipeBrain>`. The constructor:

1. Resolves the recipe YAML path (hot-reload `~/.simard/...` first, in-tree
   fallback second) using `resolve_recipe_path(repo_root, recipe_filename)`.
2. Resolves the agent binary via `LlmProvider::resolve_agent_binary()`.
3. Probes `recipe-runner-rs --version` to confirm the binary is on `$PATH`.
4. Returns `None` if any step fails — the daemon falls back to the
   deterministic or LLM-backed brain for that phase.

## Trait implementations

`RecipeBrain` implements three traits on one type:

| Trait | Method | Recipe YAML | Output parser |
|-------|--------|-------------|---------------|
| `OodaDecideBrain` | `judge_decision()` | `ooda-decide.yaml` | `parse_action_from_text()` — keyword scan for 10 action keywords |
| `OodaOrientBrain` | `judge_orientation()` | `ooda-orient.yaml` | `parse_orient_from_text()` — 3-tier cascade (JSON → bare float → deterministic floor) |
| `OodaBrain` | `decide_engineer_lifecycle()` | `ooda-engineer-lifecycle.yaml` | `parse_lifecycle_from_text()` — DECISION marker → keyword scan → default |

Each trait impl invokes `recipe-runner-rs` with the stored `recipe_path` and
phase-specific `-c` context vars, then delegates to the corresponding parse
function. The parse functions remain standalone public functions in their
original per-phase files (`recipe_decide.rs`, `recipe_orient.rs`,
`recipe_engineer_lifecycle.rs`) — they are pure `&str -> Judgment` transforms
with no struct dependency.

## Shared helpers

These functions exist once in `recipe_brain.rs`:

- **`resolve_recipe_path(repo_root, recipe_filename)`** — parameterized path
  resolution. Checks `~/.simard/prompt_assets/simard/recipes/<filename>` first
  (hot-reload), then `<repo_root>/prompt_assets/simard/recipes/<filename>`
  (in-tree).
- **`truncate(s, max)`** — char-aware truncation with `…` suffix. Used in
  error messages and rationale fields to cap unbounded LLM output.
- **`ascii_contains_ignore_case(haystack, needle)`** — byte-level
  case-insensitive substring search. Used by keyword-scanning parse functions
  (decide, lifecycle). Not used by orient (which does JSON/float extraction).

## Wiring in the daemon

`brains.rs` constructs three instances of the same type:

```rust
// build_act_brain
RecipeBrain::new(repo_root, "ooda-engineer-lifecycle.yaml", "recipe-engineer-lifecycle-brain")

// build_decide_brain
RecipeBrain::new(repo_root, "ooda-decide.yaml", "recipe-decide-brain")

// build_orient_brain
RecipeBrain::new(repo_root, "ooda-orient.yaml", "recipe-orient-brain")
```

Each is wrapped in `Arc<dyn Trait>` and passed to the OODA cycle. The fallback
chain varies per phase (logged at ERROR severity per issues #1711, #1748):

- **Act (lifecycle):** recipe → LLM-backed → deterministic (always returns `Arc`, never `None`)
- **Decide:** recipe → `None` (no LLM fallback; returns `Option`)
- **Orient:** recipe → LLM-backed → `None` (returns `Option`)

## What was deleted

- `src/ooda_brain/recipe_decide.rs` — struct, constructor, and duplicated
  helpers removed. Parse function (`parse_action_from_text`) and its tests
  remain in this file.
- `src/ooda_brain/recipe_orient.rs` — struct, constructor, and duplicated
  helpers removed. Parse function (`parse_orient_from_text`) and its tests
  remain in this file.
- `src/ooda_brain/recipe_engineer_lifecycle.rs` — struct, constructor, and
  duplicated helpers removed. Parse function (`parse_lifecycle_from_text`),
  `LIFECYCLE_KEYWORDS`, and tests remain in this file.

## Security invariants preserved

All security properties from the original three structs carry forward:

- `sanitize_context_var()` is called on every `-c` context var before passing
  to the subprocess. Prevents YAML injection.
- `truncate(&stderr, 500)` caps error message size on all failure paths.
  Prevents unbounded memory from malformed output.
- `adapter_tag` is `&'static str` — compile-time only, no runtime injection.
- `Command::new("recipe-runner-rs")` is a hardcoded literal — not interpolated.
- `resolve_recipe_path` only checks two fixed filesystem locations — no
  user-controlled path segments.
