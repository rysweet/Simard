# Reference: OODA Orient Recipe and Prompt Schema

Recipe: `prompt_assets/simard/recipes/ooda-orient.yaml`
Prompt source: `prompt_assets/simard/ooda_orient.md` (content embedded in recipe YAML)
Shim: `src/ooda_brain/recipe_orient.rs`

This is the single source of truth for the orient-phase failure-penalty
demotion judgment. The orient brain runs as a **recipe step** via
`recipe-runner-rs`, following the same pattern as `ooda-decide.yaml`,
`progress-assessment.yaml`, and `merge-readiness-judge.yaml`.

> **Superseded on the OODA path by the typed-record model
> ([#4719](https://github.com/rysweet/Simard/issues/4719), Group A).**
> The orient recipe no longer prints an urgency for Rust to scrape. It now calls
> the gated `simard ooda record-orient` tool, which validates the judgment
> through the shared `OrientFields::from_fields` chokepoint and atomically writes
> a typed `OrientDecisionRecord` (`schema: simard.ooda.orient.v1`). `RecipeBrain`
> reads that record **fail-CLOSED** with `read_verified_orient` and ignores
> stdout entirely; an absent/malformed/mismatched record keeps the base urgency
> (safe no-op), never the deterministic floor. The recipe YAML header documents
> `Output: NONE scraped from stdout`. The 3-tier stdout parser described below is
> **legacy** and retained only for historical reference. See
> [Reference: `simard ooda record-orient` / `record-decide`](./ooda-record-orient-decide-cli.md).

> **History:** Before issue
> [#2115](https://github.com/rysweet/Simard/issues/2115), the orient brain
> was `RustyClawdOrientBrain`, which compiled the prompt via `include_str!`,
> submitted it to an `LlmSubmitter`, and parsed the response as JSON. The
> recipe-based approach moves the prompt to a YAML file that can be edited
> without a rebuild, and adds a 3-tier parsing strategy (JSON → bare
> float → deterministic floor) that eliminates parse failures from
> legitimate model output.

## Recipe Layout

```yaml
name: ooda-orient
description: OODA Orient brain — failure-penalty demotion judgment
context:
  goal_id: ""
  base_urgency: ""
  base_reason: ""
  failure_count: ""
steps:
  - name: judge-demotion
    type: agent
    prompt: |
      # OODA Brain — Orient Phase: Failure-Penalty Demotion

      ## ROLE
      …

      ## CONTEXT
      Goal: {{goal_id}}
      Base urgency: {{base_urgency}}
      Reason: {{base_reason}}
      Failure count: {{failure_count}}

      ## DECISION
      …(demotion guidelines and reference scale)…

      ## EXAMPLES
      …(JSON-format examples, one per demotion scenario)…
```

The recipe is a single `agent` step. The recipe-runner-rs subprocess handles
prompt rendering, agent invocation, and stdout capture. The Rust shim
(`RecipeOrientBrain`) parses the stdout using a 3-tier strategy.

### What changed from `ooda_orient.md`

The recipe prompt preserves all content from the original `ooda_orient.md`
**except**:

- **Placeholders converted** — `{goal_id}` → `{{goal_id}}`,
  `{base_urgency}` → `{{base_urgency}}`, `{base_reason}` →
  `{{base_reason}}`, `{failure_count}` → `{{failure_count}}` to match
  recipe-runner-rs Handlebars templating.
- **OUTPUT_FORMAT section removed** — the strict "single JSON object on a
  single line, no prose before or after, no markdown fences" instruction is
  removed. The 3-tier parser handles JSON, bare floats, and any surrounding
  prose. The examples still show JSON format to guide the model.

The ROLE, CONTEXT, DECISION, and EXAMPLES sections are preserved verbatim.

## Placeholders (Context Variables)

The recipe-runner-rs performs Handlebars `{{name}}` substitution from the
context variables passed by `RecipeOrientBrain`.

| Variable | Type | Source |
|---|---|---|
| `{{goal_id}}` | string | `ctx.goal_id` — goal slug from the active board |
| `{{base_urgency}}` | string (f64) | `ctx.base_urgency` — urgency before failure penalty, in `[0.0, 1.0]` |
| `{{base_reason}}` | string | `ctx.base_reason` — rationale Orient has accumulated so far |
| `{{failure_count}}` | string (u32) | `ctx.failure_count` — consecutive failures recorded (always ≥ 1) |

Reserved synthetic IDs (`__memory__`, `__improvement__`, etc.) never reach
this brain — they are not subject to failure-penalty demotion.

## 3-Tier Parsing Strategy

`RecipeOrientBrain` uses a 3-tier parsing chain to extract the urgency
from the agent's stdout. Each tier is tried in order; the first success
wins. All tiers validate the result through `OrientJudgment::validate()`.

### Tier 1: JSON extraction

Scan stdout for the first `{…}` substring and deserialize it as an
`OrientJudgment` via `serde_json`. This matches the format the prompt's
examples show:

```json
{"adjusted_urgency": 0.60, "demotion_applied": 0.20, "rationale": "1 failure: standard floor demotion", "confidence": 0.9}
```

The model may optionally surround the JSON with prose — the parser
extracts the first `{…}` substring regardless. `adjusted_urgency` and
`rationale` are required; `confidence` defaults to 1.0, `demotion_applied`
defaults to 0.0.

### Tier 2: Bare float extraction

If no JSON object is found (or JSON parsing fails), scan for a bare
floating-point number matching the regex pattern `[0-9]+\.[0-9]+`. The
first match is used as `adjusted_urgency`. The rationale is set to the
full stdout text (truncated to 500 chars).

This handles cases where the agent emits prose like:

```
The adjusted urgency should be 0.60 based on the single failure.
```

The regex excludes `NaN`, `Inf`, and negative values by design.

### Tier 3: Deterministic floor

If neither JSON nor a bare float is found, apply the deterministic
formula:

```
adjusted_urgency = max(0.0, base_urgency - 0.2 × failure_count)
```

This is the same formula used by `DeterministicFallbackOrientBrain` and
matches the pre-prompt-driven behavior exactly. The rationale is set to
`"recipe output unparseable; deterministic floor applied"`.

### Validation on every tier

All three tiers pass the result through `OrientJudgment::validate()`,
which enforces:

- `adjusted_urgency` in `[0.0, 1.0]`
- `adjusted_urgency ≤ base_urgency` (no escalation)
- `confidence` in `[0.0, 1.0]`

If validation fails on Tier 1 or Tier 2, the parser falls through to the
deterministic floor (Tier 3). A misbehaving LLM **cannot** inflate
priorities — this is the primary security invariant.

## Error Handling

`RecipeOrientBrain` returns `Err(SimardError::AdapterInvocationFailed)` when:

- The `recipe-runner-rs` binary is not found (construction fails;
  `RecipeOrientBrain::new()` returns `None`).
- The subprocess exits with a non-zero status.
- The subprocess cannot be spawned.

On subprocess success, the 3-tier parser **never fails** — it always
produces a valid `OrientJudgment`. Tier 3 (deterministic floor) is the
unconditional safety net.

On `AdapterInvocationFailed`, the caller falls back per-priority to
`DeterministicFallbackOrientBrain` and logs the error with truncated
stderr (500 chars).

## Runtime Loading (not compile-time)

Unlike the old `ooda_orient.md` (which was embedded via `include_str!`),
the orient recipe is loaded at runtime by the recipe-runner-rs subprocess.
`RecipeOrientBrain` resolves the recipe path in this order:

1. `~/.simard/prompt_assets/simard/recipes/ooda-orient.yaml` (hot-reload)
2. `{repo_root}/prompt_assets/simard/recipes/ooda-orient.yaml` (in-tree)

Prompt edits take effect on the next daemon cycle **without a rebuild**.

## Construction Pattern

```rust
let brain: Option<Arc<dyn OodaOrientBrain>> = RecipeOrientBrain::new(repo_root)
    .map(|b| Arc::new(b) as Arc<dyn OodaOrientBrain>);
```

`RecipeOrientBrain::new(repo_root)` returns `None` when:
- The `recipe-runner-rs` binary is not on `$PATH`.
- The recipe YAML file does not exist at either resolution path.

The daemon wiring in `operator_commands_ooda/daemon/brains.rs` calls
`build_orient_brain(state_root, repo_root)`, which tries
`RecipeOrientBrain` first and falls back to
`DeterministicFallbackOrientBrain`.

## Test Inventory

`src/ooda_brain/recipe_orient.rs` contains inline `#[cfg(test)]` tests
covering all three parse tiers and edge cases:

| Test | Tier | Coverage |
|------|------|----------|
| Full JSON response | 1 | JSON with all fields |
| JSON with surrounding prose | 1 | `{…}` extraction from mixed output |
| JSON missing optional fields | 1 | `confidence` default, `demotion_applied` default |
| Bare float in prose | 2 | `0.60` extracted from natural language |
| Multiple floats | 2 | First match used |
| No parseable output | 3 | Deterministic floor applied |
| Empty output | 3 | Deterministic floor applied |
| Escalation rejected → floor | 3 | `adjusted > base` triggers validation failure |
| Negative float rejected | 3 | Not matched by `[0-9]+\.[0-9]+` |
| NaN/Inf rejected | 1 | JSON parses but validate() rejects |

## Versioning & Compatibility

Changes to the demotion reference scale or guidance prose are safe to
ship alone — and take effect without a rebuild.

Changes to the `OrientJudgment` struct fields require a coordinated Rust
change to the struct in `orient.rs` and the Tier 1 parser in
`recipe_orient.rs`. The Tier 2 and Tier 3 parsers only produce
`adjusted_urgency` and are unaffected.

## See Also

* [Reference: `ooda_orient.md` prompt schema](ooda-orient-prompt.md) — historical prompt schema (superseded by recipe)
* [Reference: OODA decide recipe and prompt schema](ooda-decide-prompt.md) — decide-phase recipe
* [Reference: OODA engineer lifecycle recipe](ooda-engineer-lifecycle-recipe.md) — engineer lifecycle recipe
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md) — normative grammar
* [Reference: `OodaBrain` API](ooda-brain-api.md) — trait and type definitions
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — editing guide
* [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
