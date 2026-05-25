# Reference: `ooda_decide.md` Prompt Schema

File: `prompt_assets/simard/ooda_decide.md`
Loaded at compile time via `include_str!` from `src/ooda_brain/decide.rs`.

This is the single source of truth for the decide-phase action-kind routing
decision. Edit this file to change how Simard routes priorities to action
kinds; no Rust changes required (rebuild + daemon restart).

## File Layout

The prompt is a markdown document with five top-level sections, in this
order:

```markdown
CRITICAL: Your first non-blank line MUST be `DECISION: <variant>`. Do NOT output JSON.

# OODA Brain — Decide Phase: Action-Kind Routing

## ROLE
…

## CONTEXT
…(uses {goal_id}, {urgency}, {reason} placeholders)…

## OPTIONS
…(variant tags: advance_goal, consolidate_memory, etc.)…

## OUTPUT_FORMAT
…(prose-first DECISION marker protocol)…

## EXAMPLES
…(text-format examples, one per routing case)…
```

The **first line** of the file is an anti-JSON guard:

```
CRITICAL: Your first non-blank line MUST be `DECISION: <variant>`. Do NOT output JSON.
```

This line exists because LLMs trained on JSON-heavy corpora will default to
JSON output when the prompt's `OUTPUT_FORMAT` section is ambiguous. The guard
line fires before the model reads any other instruction, making the constraint
impossible to miss. It was added in PR #2035/#2040 after production logs showed
12 decide-brain parse failures in 30 minutes — all caused by the model
emitting `{"choice": "advance_goal", ...}` JSON instead of `DECISION:` markers.

## Placeholders

`DecideBrain` performs literal `{name}` → value substitution in **CONTEXT**
before submission. Unknown placeholders are left untouched.

| Placeholder | Type | Source |
|---|---|---|
| `{goal_id}` | string | `ctx.goal_id` — goal slug or reserved synthetic ID |
| `{urgency}` | f64 | `ctx.urgency` — Orient's score in `[0.0, 1.0]` |
| `{reason}` | string | `ctx.reason` — Orient's rationale for this priority |

## Variant Tokens

The `OPTIONS` section enumerates the valid `DECISION:` variant tokens. Each
maps 1:1 to a `DecideJudgment` enum variant in `src/ooda_brain/decide.rs`.

| Token | Enum variant | When to use |
|---|---|---|
| `advance_goal` | `DecideJudgment::AdvanceGoal` | Default for any non-reserved `goal_id` |
| `consolidate_memory` | `DecideJudgment::ConsolidateMemory` | Reserved `__memory__` synthetic ID |
| `run_improvement` | `DecideJudgment::RunImprovement` | Reserved `__improvement__` synthetic ID |
| `poll_developer_activity` | `DecideJudgment::PollDeveloperActivity` | Reserved `__poll_activity__` synthetic ID |
| `extract_ideas` | `DecideJudgment::ExtractIdeas` | Reserved `__extract_ideas__` synthetic ID |
| `safe_update` | `DecideJudgment::SafeUpdate` | Reserved `__safe_update__` synthetic ID |
| `research_query` | `DecideJudgment::ResearchQuery` | Reserved for future use |
| `skip` | `DecideJudgment::Skip` | Explicit skip |

Tokens are matched case-insensitively by the parser. The variant whitelist
is the `DecideJudgment` enum itself — there is no parallel hand-maintained
list.

## Output Format

The decide brain uses the **prose-first DECISION marker protocol**, the same
format used by the engineer-lifecycle brain (`ooda_brain.md`). The wire format
is documented normatively in
[text-parsing wire formats § decide phase](text-parsing-wire-formats.md#1a-decide-phase-deciders).

### Anti-JSON hardening

The prompt contains **two** anti-JSON directives:

1. **Line 1** — `CRITICAL: Your first non-blank line MUST be DECISION: <variant>. Do NOT output JSON.`
2. **OUTPUT_FORMAT section** — `Do NOT output JSON. The daemon parser reads the first non-blank line for a DECISION: marker — a JSON object on the first line is an immediate parse failure.`

This redundancy is intentional. Production experience showed that a single
`OUTPUT_FORMAT` instruction was insufficient — models would scan the
examples section, find JSON-like patterns in the input context (which is
a JSON object), and mirror that format in the output. The line-1 guard fires
before any context is consumed.

### Response format

```
DECISION: <variant>
<optional rationale prose>
```

The parser:
1. Finds the first non-blank line matching `DECISION:` (case-insensitive on
   the keyword).
2. Extracts the variant token and matches against the `DecideJudgment` enum.
3. Collects remaining lines as the `rationale` field.

If no `DECISION:` line is found, the deterministic prefix mapping fires:
`__memory__` → `consolidate_memory`, `__improvement__` → `run_improvement`,
etc. Real goal slugs → `advance_goal`.

## Examples

The prompt's `EXAMPLES` section contains text-format examples showing the
correct output for each routing case. All examples use the DECISION marker
format — no JSON examples appear in the prompt.

Example categories:

| Case | Input pattern | Expected output |
|---|---|---|
| Reserved synthetic ID | `goal_id: "__memory__"` | `DECISION: consolidate_memory` |
| Ordinary goal slug | `goal_id: "ship-v1"` | `DECISION: advance_goal` |
| Activity polling | `goal_id: "__poll_activity__"` | `DECISION: poll_developer_activity` |
| Negative example | Real goal with "memory" in name | `DECISION: advance_goal` (not `consolidate_memory`) |

Negative examples are critical: without them, models will pattern-match on
substring similarity between the goal name and the variant token.

## Merge Authority Section

The prompt includes a `## Merge Authority` section documenting Simard's gated
authority to squash-merge pull requests via `stewardship::merge_pr_if_merge_ready`.
This section is **informational context** — it does not add a merge-related
variant to the `DECISION:` whitelist. The brain surfaces merge-readiness
observations in the rationale text and routes to `advance_goal`.

## Self-Update Awareness Section

The prompt includes a `## Self-update awareness` section documenting the
four-part doctrine for the `safe_update` variant. This variant triggers
`simard safe-update` (drain → snapshot → pre-test → swap → exec → validate →
optional rollback). The section gates the variant on:

1. Divergence ≥ N commits behind `origin/main`
2. No critical WIP (no in-flight engineers with PR-blocking goals)
3. Clean previous cycle (no failures, no tracking issues)
4. Cooldown elapsed (≥30 min since last attempt)

## Compile-Time Embedding

Same rules as `ooda_brain.md`: the prompt is embedded with `include_str!`,
must exist at build time and be valid UTF-8, and should stay under ~32 KB.

## Parser Rules

`parse_judgment_from_response` (in `src/ooda_brain/decide.rs`) uses the
DECISION marker as its **sole** parser:

1. Find the first non-blank line matching `DECISION:` (case-insensitive).
2. Extract the variant token; match against `DecideJudgment` variants.
3. Collect remaining text as the `rationale`.
4. If no marker is found, return error; caller falls back to deterministic
   prefix mapping.

The JSON parser has been removed. A model that emits `{"choice":"advance_goal"}`
will trigger `BrainResponseUnparseable` and the deterministic fallback.

## Versioning & Compatibility

Semantic changes (adding a new variant token) require a coordinated Rust
change to `DecideJudgment` and `ActionKind`. Cosmetic edits (rationale
guidance, examples, ROLE phrasing) are safe to ship alone.

When adding a new variant:

1. Add the variant to `DecideJudgment` in `src/ooda_brain/decide.rs`.
2. Add the mapping from `DecideJudgment` → `ActionKind`.
3. Add the variant to the `OPTIONS` section in the prompt.
4. Add an example to the `EXAMPLES` section.
5. Add a test to `src/ooda_brain/decide.rs` covering the new token.
6. Update the variant table in
   [text-parsing wire formats § decide](text-parsing-wire-formats.md#1a-decide-phase-deciders).

## See Also

* [Reference: `ooda_brain.md` prompt schema](ooda-brain-prompt.md) — engineer-lifecycle prompt
* [Reference: `ooda_orient.md` prompt schema](ooda-orient-prompt.md) — orient-phase prompt
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md) — normative grammar
* [Reference: OODA Brain Decision Protocol](ooda-brain-decision-protocol.md) — full behavior matrix
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — editing guide
* [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
