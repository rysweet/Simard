---
title: Add a new recipe brain phase
description: How to add a new OODA phase backed by recipe-runner-rs using the existing RecipeBrain struct.
last_updated: 2026-05-27
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/unified-recipe-brain.md
  - ../reference/recipe-brain-api.md
  - ../howto/edit-the-ooda-brain-prompt.md
---

# Add a new recipe brain phase

`RecipeBrain` is a single struct that handles all recipe-runner-backed OODA
phases. To add a new phase, you write a recipe YAML, a parse function, and a
trait impl — you do **not** create a new struct.

## Steps

### 1. Write the recipe YAML

Create `prompt_assets/simard/recipes/ooda-<phase>.yaml`. The recipe receives
context as `-c key=value` args and writes its decision to stdout. The
**first word** of stdout must be the decision variant name (see
[text-parsing wire formats](../reference/text-parsing-wire-formats.md)).

### 2. Define the trait (if new)

If the phase needs a new trait, add it to `src/ooda_brain/mod.rs`:

```rust
pub trait OodaNewPhaseBrain: Send + Sync {
    fn judge_new_phase(&self, ctx: &NewPhaseContext) -> SimardResult<NewPhaseJudgment>;
}
```

### 3. Write a parse function

Create `src/ooda_brain/recipe_<phase>.rs` with a public
`parse_<phase>_from_text(text: &str) -> NewPhaseJudgment` function. This
follows the existing pattern — each phase keeps its parse function and tests
in its own file. Use the first-word extraction pattern:

```rust
let first_word = text.split_whitespace().next()
    .unwrap_or("").to_ascii_lowercase();
match first_word.as_str() {
    "variant_a" => ...,
    "variant_b" => ...,
    _ => /* safe default */,
}
```

Use `truncate()` for rationale capping (imported from `recipe_brain.rs`).

### 4. Implement the trait on RecipeBrain

Add an `impl OodaNewPhaseBrain for RecipeBrain` block in `recipe_brain.rs`.
The body:

1. Builds the `Command` with `self.recipe_path`, `self.agent_binary`, and
   phase-specific `-c` context vars (all sanitized via `sanitize_context_var`).
2. Runs `recipe-runner-rs` and checks exit status.
3. Calls `parse_new_phase_from_text()` on stdout.

### 5. Wire it in brains.rs

Add a `build_new_phase_brain()` function in
`src/operator_commands_ooda/daemon/brains.rs`:

```rust
pub(super) fn build_new_phase_brain(
    state_root: &Path,
    repo_root: &Path,
) -> Option<Arc<dyn OodaNewPhaseBrain>> {
    match RecipeBrain::new(repo_root, "ooda-new-phase.yaml", "recipe-new-phase-brain") {
        Some(b) => {
            daemon_log(state_root, "[simard] OODA daemon: new_phase_brain = RecipeBrain");
            Some(Arc::new(b))
        }
        None => {
            record_fallback(state_root, "new-phase", "recipe unavailable");
            None
        }
    }
}
```

### 6. Add tests

Add tests for `parse_new_phase_from_text()` in the `#[cfg(test)] mod tests`
block in `recipe_<phase>.rs` (the same file as the parse function). Test:

- Each variant is recognized as first word (case-insensitive).
- Unrecognized first word returns the safe default.
- Rationale is truncated for long output.
- Constructor returns `None` for missing recipe file.

## What you do NOT do

- Do **not** create a new struct. `RecipeBrain` handles all phases.
- Do **not** duplicate `resolve_recipe_path` or `truncate`. They are shared.
- Do **not** use `ascii_contains_ignore_case` or keyword scanning — those
  patterns have been removed (#2144). Use first-word extraction only.
- Do **not** add the recipe filename as a module-level `const`. Pass it to
  `RecipeBrain::new()` from `brains.rs`.
