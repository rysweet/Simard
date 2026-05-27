# Reference: `ooda_orient.md` Prompt Schema

File: `prompt_assets/simard/recipes/ooda-orient.yaml`
Parser: `parse_orient_from_text()` in `src/ooda_brain/recipe_brain.rs`

This is the single source of truth for the orient-phase failure-penalty
demotion judgment. The orient brain runs as a recipe step via
`recipe-runner-rs`. The agent outputs a bare decimal number as the first
token — the parser extracts the first float from the text.

> **Changed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The orient brain previously used JSON object format (`{"adjusted_urgency":
> 0.6, "rationale": "..."}`) parsed via `serde_json`. This has been replaced
> with bare-float extraction. The prompt now instructs the LLM to output the
> adjusted urgency as a bare decimal as its first token.

## File Layout

The prompt is embedded in a recipe YAML with these top-level sections:

```markdown
# OODA Brain — Orient Phase: Failure-Penalty Demotion

## ROLE
…

## CONTEXT
…(uses {{goal_id}}, {{base_urgency}}, {{base_reason}}, {{failure_count}} placeholders)…

## DECISION
…(demotion guidelines and reference scale)…

## OUTPUT FORMAT
Output the adjusted urgency as a bare decimal number (e.g. `0.42`) as
the first token of your response. Follow with your rationale.

## EXAMPLES
…(bare-float format examples, one per demotion scenario)…
```

## Placeholders

| Placeholder | Type | Source |
|---|---|---|
| `{{goal_id}}` | string | `ctx.goal_id` — goal slug from the active board |
| `{{base_urgency}}` | f64 | `ctx.base_urgency` — urgency before failure penalty, in `[0.0, 1.0]` |
| `{{base_reason}}` | string | `ctx.base_reason` — rationale Orient has accumulated so far |
| `{{failure_count}}` | u32 | `ctx.failure_count` — consecutive failures recorded (always ≥ 1) |

Reserved synthetic IDs (`__memory__`, `__improvement__`, etc.) never reach
this brain — they are not subject to failure-penalty demotion.

## Output Format

The orient brain uses **bare-float format**. The first number in the
output text becomes `adjusted_urgency`. The full text becomes `rationale`.

### Response format

```
<float> <rationale text>
```

Example:
```
0.60 Standard demotion for 1 failure. The goal is healthy but needs a penalty.
```

### Fields produced

| Field | Source | Default |
|---|---|---|
| `adjusted_urgency` | First float in text (`try_first_float`) | Deterministic floor |
| `rationale` | Full text of response | `"deterministic floor"` |
| `confidence` | Always `1.0` | `1.0` |
| `demotion_applied` | `0.0` (daemon recomputes) | `0.0` |

### Validation

`OrientJudgment::validate()` enforces:

- `adjusted_urgency` in `[0.0, 1.0]`
- `adjusted_urgency ≤ base_urgency` (no escalation — escalation belongs to
  the engineer-lifecycle brain)
- `confidence` in `[0.0, 1.0]`

If validation fails, the deterministic floor applies:
`urgency - 0.2 × failure_count`, clamped to `[0.0, 1.0]`.

## DECISION Section

The prompt includes a `## DECISION` section (note: this is a prompt section
header, not a wire format marker). This section provides the demotion
reference scale:

| `failure_count` | Expected demotion | Guidance |
|---|---|---|
| 1 | ~0.2 below base | Light penalty |
| 2 | ~0.4 below base | Moderate |
| 3 | ~0.6 below base | Heavy |
| ≥ 5 | Effectively zero | Goal falls below all unfailed work |

The brain may deviate from this scale:
- **More lenient** when `base_reason` indicates transient failures (CI flake,
  recent spawn).
- **More aggressive** when the goal_id pattern or reason suggests the goal is
  malformed.

## Examples

The prompt's `EXAMPLES` section uses bare-float format:

| Scenario | `failure_count` | Example output |
|---|---|---|
| Standard floor demotion | 1 | `0.60 Standard demotion. 1 failure, penalty of 0.2 applied.` |
| Chronic failures | 5 | `0.0 Driven to zero. 5 consecutive failures, goal should yield to all unfailed work.` |
| Transient cause (leniency) | 2 | `0.55 Slightly above floor. Dirty tree suggests active work despite failures.` |

## Parser Rules

`parse_orient_from_text()` in `src/ooda_brain/recipe_brain.rs` extracts
the urgency from the brain response:

1. Call `try_first_float(text)` — scans for the first decimal substring.
2. If found, use it as `adjusted_urgency`; full text as `rationale`;
   `confidence = 1.0`.
3. If no float found, apply deterministic floor:
   `max(0.0, base_urgency - 0.2 * failure_count)`.
4. Call `OrientJudgment::validate()` — if bounds violated, fall back to
   deterministic floor.

> **Removed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The JSON extraction tier (`try_json_extraction` using `serde_json::from_str`
> on `{…}` substrings) has been deleted. The `try_bare_float` function has
> been renamed to `try_first_float` — logic unchanged.

## Deterministic Fallback

`DeterministicFallbackOrientBrain` preserves the pre-#1469 formula bit-for-bit:

```
adjusted_urgency = max(0.0, base_urgency - 0.2 * failure_count)
```

This fallback fires when:
- No LLM is configured.
- The recipe subprocess fails.
- No float is found in the output text.
- The parsed judgment fails validation (`adjusted_urgency > base_urgency`).

The fallback is **not** a silent error handler — the parse failure is surfaced
through all four visibility channels (structured log, metric, cycle report,
GitHub issue escalation) before the fallback is applied.

## Runtime Loading

The orient recipe is loaded at runtime by the recipe-runner-rs subprocess.
`RecipeBrain` resolves the recipe path relative to `repo_root`:

```
{repo_root}/prompt_assets/simard/recipes/ooda-orient.yaml
```

Prompt edits take effect on the next daemon cycle **without a rebuild**.

## Versioning & Compatibility

The orient brain has no enum variants to extend — it produces a single
`OrientJudgment` struct. Changes to the demotion reference scale or guidance
prose are safe to ship alone — and take effect without a rebuild.

## See Also

* [Reference: `ooda_brain.md` prompt schema](ooda-brain-prompt.md) — engineer-lifecycle prompt
* [Reference: `ooda_decide.md` prompt schema](ooda-decide-prompt.md) — decide-phase prompt
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md) — normative grammar
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — editing guide
* [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
