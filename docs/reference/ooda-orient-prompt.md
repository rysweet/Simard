# Reference: `ooda_orient.md` Prompt Schema

File: `prompt_assets/simard/ooda_orient.md`
Loaded at compile time via `include_str!` from `src/ooda_brain/orient.rs`.

This is the single source of truth for the orient-phase failure-penalty
demotion judgment. Edit this file to change how Simard demotes chronically
failing goals; no Rust changes required (rebuild + daemon restart).

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
…(labeled-line format: ADJUSTED_URGENCY, RATIONALE, CONFIDENCE)…

## EXAMPLES
…(text-format examples, one per demotion scenario)…
```

Unlike the decide and engineer-lifecycle brains, the orient brain does **not**
use a `DECISION:` marker. It uses **labeled-line format** — each output field
appears on its own line with a case-insensitive prefix label.

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

The orient brain uses **labeled-line format**. The wire format is documented
normatively in
[text-parsing wire formats § orient phase](text-parsing-wire-formats.md#1b-orient-phase-orientrs).

### Response format

```
ADJUSTED_URGENCY: <float in [0,1]>
RATIONALE: <short reason>
CONFIDENCE: <float in [0,1]>
```

### Labeled fields

| Label | Type | Required | Default | Validation |
|---|---|---|---|---|
| `ADJUSTED_URGENCY:` | `f64` | **Yes** | _(parse fails without it)_ | Must be in `[0.0, base_urgency]` |
| `RATIONALE:` | `String` | No | `"<no rationale provided>"` | None |
| `CONFIDENCE:` | `f64` | No | `1.0` | Must be in `[0.0, 1.0]` |
| `DEMOTION_APPLIED:` | `f64` | No | Computed as `base_urgency - adjusted_urgency` | Must be ≥ 0 |

Labels are matched case-insensitively. Unknown labels are silently ignored
(forward compatible). Non-labeled lines before or after the labeled fields
are ignored — the model may include reasoning prose around the labels.

### Validation

`OrientJudgment::validate()` enforces:

- `adjusted_urgency` in `[0.0, 1.0]`
- `adjusted_urgency ≤ base_urgency` (no escalation — escalation belongs to
  the engineer-lifecycle brain)
- `confidence` in `[0.0, 1.0]`

If validation fails, the deterministic floor applies:
`urgency - 0.2 × failure_count`, clamped to `[0.0, 1.0]`.

### Anti-JSON note

The prompt's `OUTPUT_FORMAT` section explicitly states:

> Do NOT output JSON — the daemon parser reads labeled lines, not JSON objects.

The orient parser does not accept JSON. A model that emits
`{"adjusted_urgency": 0.6, ...}` will fail parsing (no `ADJUSTED_URGENCY:`
label found) and the deterministic floor will apply. This instruction was
strengthened in PR #2035/#2040 to prevent models from mimicking the JSON
format of the input context.

## DECISION Section

The prompt includes a `## DECISION` section (note: this is a prompt section
header, not the `DECISION:` wire format marker used by other brains). This
section provides the demotion reference scale:

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

The prompt's `EXAMPLES` section contains labeled-line examples. All examples
use the text format — no JSON examples appear in the prompt.

| Scenario | `failure_count` | Expected `ADJUSTED_URGENCY` | Key signal |
|---|---|---|---|
| Standard floor demotion | 1 | `base - 0.2` | Default case |
| Chronic failures | 5 | `0.0` | Drive to zero |
| Transient cause (leniency) | 2 | Slightly above floor | Dirty tree suggests active work |
| Negative: escalation | 1 | _(rejected)_ | `adjusted > base` triggers fallback |

## Compile-Time Embedding

Same rules as `ooda_brain.md`: the prompt is embedded with `include_str!`,
must exist at build time and be valid UTF-8, and should stay under ~32 KB.

## Parser Rules

`parse_judgment_from_response` (in `src/ooda_brain/orient.rs`) scans lines
for labeled fields:

1. Iterate `response.lines()`.
2. For each line, check `starts_with("ADJUSTED_URGENCY:")` (case-insensitive),
   extract and parse the float value.
3. Similarly for `RATIONALE:`, `CONFIDENCE:`, and `DEMOTION_APPLIED:`.
4. Non-matching lines are ignored.
5. If `ADJUSTED_URGENCY:` is missing, return error; caller falls back to the
   deterministic floor formula.

The JSON parser has been removed. `serde_json::from_str` is never called on
the LLM response.

## Deterministic Fallback

`DeterministicFallbackOrientBrain` preserves the pre-#1469 formula bit-for-bit:

```
adjusted_urgency = max(0.0, base_urgency - 0.2 * failure_count)
```

This fallback fires when:
- No LLM is configured.
- The LLM response cannot be parsed (missing `ADJUSTED_URGENCY:` label).
- The parsed judgment fails validation (`adjusted_urgency > base_urgency`).

The fallback is **not** a silent error handler — the parse failure is surfaced
through all four visibility channels (structured log, metric, cycle report,
GitHub issue escalation) before the fallback is applied.

## Versioning & Compatibility

The orient brain has no enum variants to extend — it produces a single
`OrientJudgment` struct. Adding new labeled fields to the prompt is safe:
the parser ignores unknown labels (forward compatible), and new fields
will not be parsed until the Rust parser is updated.

Changes to the demotion reference scale or guidance prose are safe to ship
alone. Changes to the labeled-line field names require a coordinated Rust
change to the parser in `orient.rs`.

## See Also

* [Reference: `ooda_brain.md` prompt schema](ooda-brain-prompt.md) — engineer-lifecycle prompt
* [Reference: `ooda_decide.md` prompt schema](ooda-decide-prompt.md) — decide-phase prompt
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md) — normative grammar
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — editing guide
* [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
