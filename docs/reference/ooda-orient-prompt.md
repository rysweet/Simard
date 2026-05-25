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
…(JSON object: adjusted_urgency, demotion_applied, rationale, confidence)…

## EXAMPLES
…(JSON-format examples, one per demotion scenario)…
```

Unlike the decide and engineer-lifecycle brains, the orient brain does **not**
use a `DECISION:` marker. It uses **JSON object format** — the model returns
a single JSON object on one line with the required fields.

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

The orient brain uses **JSON object format**. The wire format is documented
normatively in
[text-parsing wire formats § orient phase](text-parsing-wire-formats.md#1b-orient-phase-orientrs).

### Response format

```json
{"adjusted_urgency": <float in [0,1]>, "demotion_applied": <float ≥ 0>, "rationale": "<short reason>", "confidence": <float in [0,1]>}
```

### JSON fields

| Field | Type | Required | Default | Validation |
|---|---|---|---|---|
| `adjusted_urgency` | `f64` | **Yes** | _(parse fails without it)_ | Must be in `[0.0, base_urgency]` |
| `rationale` | `String` | **Yes** | _(parse fails without it)_ | None |
| `confidence` | `f64` | No | `1.0` | Must be in `[0.0, 1.0]` |
| `demotion_applied` | `f64` | No | `0.0` | Convenience; daemon recomputes |

The parser extracts the first `{…}` substring from the response, so the
model may optionally surround the JSON with prose (tolerated, not encouraged).
Extra fields are silently ignored (forward compatible).

### Validation

`OrientJudgment::validate()` enforces:

- `adjusted_urgency` in `[0.0, 1.0]`
- `adjusted_urgency ≤ base_urgency` (no escalation — escalation belongs to
  the engineer-lifecycle brain)
- `confidence` in `[0.0, 1.0]`

If validation fails, the deterministic floor applies:
`urgency - 0.2 × failure_count`, clamped to `[0.0, 1.0]`.

### JSON parsing notes

The parser uses `serde_json::from_str` on the extracted `{…}` substring.
Malformed JSON or a response with no `{` causes a parse failure, and the
deterministic floor applies. The old labeled-line format
(`ADJUSTED_URGENCY:`, `RATIONALE:`, `CONFIDENCE:`) is no longer accepted.

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

The prompt's `EXAMPLES` section contains JSON-format examples matching the
parser's expected wire format.

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

`parse_judgment_from_response` (in `src/ooda_brain/orient.rs`) extracts and
deserializes a JSON object from the brain response:

1. Trim whitespace; reject empty responses.
2. Find the first `{` and last `}` in the response to locate the JSON object.
3. Deserialize the substring via `serde_json::from_str::<OrientJudgment>`.
4. `adjusted_urgency` and `rationale` are required fields; `confidence`
   defaults to `1.0` and `demotion_applied` defaults to `0.0`.
5. If no JSON object is found or deserialization fails, return error; caller
   falls back to the deterministic floor formula.

The old labeled-line parser has been removed. Labeled-line responses
(e.g. `ADJUSTED_URGENCY: 0.6`) will fail parsing because no `{` is found.

## Deterministic Fallback

`DeterministicFallbackOrientBrain` preserves the pre-#1469 formula bit-for-bit:

```
adjusted_urgency = max(0.0, base_urgency - 0.2 * failure_count)
```

This fallback fires when:
- No LLM is configured.
- The LLM response cannot be parsed (no JSON object found, or deserialization fails).
- The parsed judgment fails validation (`adjusted_urgency > base_urgency`).

The fallback is **not** a silent error handler — the parse failure is surfaced
through all four visibility channels (structured log, metric, cycle report,
GitHub issue escalation) before the fallback is applied.

## Versioning & Compatibility

The orient brain has no enum variants to extend — it produces a single
`OrientJudgment` struct. Adding new JSON fields to the prompt is safe:
the `serde` deserializer ignores unknown fields (forward compatible), and
new fields will not be parsed until the Rust struct is updated.

Changes to the demotion reference scale or guidance prose are safe to ship
alone. Changes to the JSON field names require a coordinated Rust change
to the `OrientJudgment` struct and parser in `orient.rs`.

## See Also

* [Reference: `ooda_brain.md` prompt schema](ooda-brain-prompt.md) — engineer-lifecycle prompt
* [Reference: `ooda_decide.md` prompt schema](ooda-decide-prompt.md) — decide-phase prompt
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md) — normative grammar
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — editing guide
* [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
