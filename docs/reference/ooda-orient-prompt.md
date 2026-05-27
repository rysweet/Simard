# Reference: `ooda_orient.md` Prompt Schema

File: `prompt_assets/simard/ooda_orient.md`
Loaded at compile time via `include_str!` from `src/ooda_brain/orient.rs`.

This is the single source of truth for the orient-phase failure-penalty
demotion judgment. Edit this file to change how Simard demotes chronically
failing goals; rebuild + daemon restart are still required.

## File Layout

The prompt is a markdown document with six top-level sections:

```markdown
# OODA Brain — Orient Phase: Failure-Penalty Demotion

## ROLE
…

## CONTEXT
…(uses {goal_id}, {base_urgency}, {base_reason}, {failure_count} placeholders)…

## DECISION
…(demotion guidelines and reference scale)…

## OUTPUT_FORMAT
…(first word must be a decimal number)…

## EXAMPLES
…(bare-float-first-token examples)…
```

> **Changed in #2144:** The orient brain no longer accepts JSON. The model now
> emits a **bare decimal as the first token**, followed by free-form rationale.

## Placeholders

`OrientBrain` performs literal `{name}` → value substitution in **CONTEXT**
before submission.

| Placeholder | Type | Source |
|---|---|---|
| `{goal_id}` | string | `ctx.goal_id` — goal slug from the active board |
| `{base_urgency}` | f64 | `ctx.base_urgency` — urgency before failure penalty, in `[0.0, 1.0]` |
| `{base_reason}` | string | `ctx.base_reason` — rationale Orient has accumulated so far |
| `{failure_count}` | u32 | `ctx.failure_count` — consecutive failures recorded (always ≥ 1) |

Reserved synthetic IDs (`__memory__`, `__improvement__`, etc.) never reach
this brain — they are not subject to failure-penalty demotion.

## Output Format

The orient brain uses the **first-float protocol**. The wire format is
documented normatively in
[text-parsing wire formats § orient phase](text-parsing-wire-formats.md#1b-orient-phase-recipe_brainrs).

### Response format

```
<decimal> <optional rationale text>
```

Example:

```
0.6 Standard floor demotion applied
```

### Parsed fields

| Field | Source | Value |
|---|---|---|
| `adjusted_urgency` | first token | Parsed as `f64` |
| `rationale` | full response text | Entire model response |
| `confidence` | parser default | Always `1.0` |
| `demotion_applied` | daemon/runtime logic | Recomputed outside the parser |

### Validation

`OrientJudgment::validate()` is retained and still enforces:

- `adjusted_urgency` in `[0.0, 1.0]`
- `adjusted_urgency ≤ base_urgency` (no escalation)
- `confidence` in `[0.0, 1.0]`

If validation fails, the deterministic floor still applies:
`urgency - 0.2 × failure_count`, clamped to `[0.0, 1.0]`.

## DECISION Section

The prompt still includes a `## DECISION` section (prompt prose, not a wire
marker). It still provides the demotion reference scale:

| `failure_count` | Expected demotion | Guidance |
|---|---|---|
| 1 | ~0.2 below base | Light penalty |
| 2 | ~0.4 below base | Moderate |
| 3 | ~0.6 below base | Heavy |
| ≥ 5 | Effectively zero | Goal falls below all unfailed work |

The brain may deviate from this scale:
- **More lenient** when `base_reason` indicates transient failures.
- **More aggressive** when the goal ID or rationale suggests the goal is malformed.

## Examples

The prompt's `EXAMPLES` section should use the new bare-float-first-token form.

| Scenario | Example output |
|---|---|
| Standard floor demotion | `0.6 Standard floor demotion applied` |
| Chronic failures | `0.0 Five consecutive failures; drop to zero urgency` |
| Transient cause (leniency) | `0.7 Recent worktree activity suggests a softer demotion` |
| Negative: escalation | `1.2 Escalate urgency` → rejected by validation |

> **Removed in #2144:** JSON object examples and JSON field tables.

## Compile-Time Embedding

Same rules as `ooda_brain.md`: the prompt is embedded with `include_str!`,
must exist at build time and be valid UTF-8, and should stay under ~32 KB.

## Parser Rules

`parse_judgment_from_response` in `src/ooda_brain/orient.rs` now uses the
renamed helper `try_first_float()`:

1. Split the response on whitespace.
2. Take the first token.
3. Parse that token as `f64`.
4. If parsing succeeds, build `OrientJudgment` with `confidence = 1.0` and
   `rationale = full response text`.
5. Run `OrientJudgment::validate()`.
6. If parsing or validation fails, fall back to the deterministic floor.

The helper rename is purely descriptive: `try_bare_float()` →
`try_first_float()`.

## Deterministic Fallback

`DeterministicFallbackOrientBrain` preserves the pre-#1469 formula bit-for-bit:

```
adjusted_urgency = max(0.0, base_urgency - 0.2 * failure_count)
```

This fallback fires when:
- No LLM is configured.
- The LLM response's **first token** cannot be parsed as a float.
- The parsed judgment fails validation (`adjusted_urgency > base_urgency`).

The fallback is **not** a silent error handler — the parse failure is surfaced
through the usual visibility channels before the fallback is applied.

## Versioning & Compatibility

The orient brain still produces a single `OrientJudgment` struct. Semantic
changes to the prompt are safe when they preserve the first-token decimal rule.

If you change the output examples or wording, keep this invariant:

- the **first word must be a decimal number**

A response that starts with prose, markdown fencing, or JSON punctuation will
miss the parse fast path and drop to the deterministic floor.

## See Also

* [Reference: `ooda_brain.md` prompt schema](ooda-brain-prompt.md) — engineer-lifecycle prompt
* [Reference: `ooda_decide.md` prompt schema](ooda-decide-prompt.md) — decide-phase prompt
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md) — normative grammar
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — editing guide
* [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
