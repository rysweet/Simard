---
title: Text-based brain protocol
description: Why Simard's OODA brains, recipe shims, and progress checkers use text-based wire formats instead of JSON parsing of LLM output — and how the text parsers work.
last_updated: 2026-05-24
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../reference/text-parsing-wire-formats.md
  - ../reference/ooda-brain-api.md
  - ../reference/ooda-brain-decision-protocol.md
  - ../reference/progress-evidence-api.md
  - ../howto/edit-the-ooda-brain-prompt.md
  - ../howto/diagnose-decide-orient-parse-failures.md
  - ./prompt-driven-ooda-brain.md
  - ./progress-evidence-gating.md
---

# Text-based brain protocol

Simard never parses JSON from LLM or recipe output. Every brain, recipe shim,
and progress checker uses text-based wire formats — keyword markers,
labeled lines, or key=value pairs — that the Rust code parses with `str`
methods. No `serde_json::from_str` on model output. No regex crate. No
extraction heuristics like `find('{')..rfind('}')`.

This document explains the problem that motivated the change, the design
principles behind the text-based protocol, and the specific wire formats
used at each decision site.

## The problem: JSON-parsing LLM output is an anti-pattern

Before issue [#1980](https://github.com/rysweet/Simard/issues/1980), eight
sites in the codebase parsed JSON from LLM or recipe output:

| Site | What it parsed | Failure mode |
|------|---------------|--------------|
| `disk_health.rs` | `DiskHealthReport` from recipe stdout | Recipe bash step had to `printf` valid JSON — fragile quoting |
| `recipe_progress_checker.rs` | `ReviewerResponse` from recipe stdout | Double indirection: recipe runs LLM, LLM emits JSON, Rust parses JSON |
| `recipe_merge_judge.rs` | `JudgeOutcome` from recipe stdout | Same double indirection |
| `decide.rs` | `DecideJudgment` from LLM response | `find('{')..rfind('}')` extraction — boundary attacks, partial objects |
| `orient.rs` | `OrientJudgment` from LLM response | Same extraction pattern |
| `rustyclawd.rs` | `EngineerLifecycleDecision` from LLM response | JSON fallback path after DECISION marker — two parsers, double failure surface |
| `progress_reviewer.rs` | `ReviewerResponse` from LLM response | Dead code — replaced by recipe_progress_checker |
| `merge_judge.rs` | `JudgeOutcome` from LLM response | Dead code — replaced by recipe_merge_judge |

Every one of these was brittle and unnecessary:

1. **LLMs don't reliably produce valid JSON.** The production logs showed
   models emitting `"OK"`, `"continue"`, prose paragraphs, markdown-fenced
   JSON, and partial objects. Each failure triggered a silent fallback that
   masked the actual decision the model made.

2. **The recipe runner handles agentic communication natively.** Recipe steps
   return text, not structured JSON. Forcing a bash step to `printf` a JSON
   object with proper escaping is fragile — a filename with a quote character
   breaks the output.

3. **The `find('{')..rfind('}')` extraction pattern is a security risk.**
   It trusts the LLM to produce a single JSON boundary. A model that emits
   `{"choice":"skip"} ... {"choice":"reclaim_and_redispatch"}` gets the
   *second* object parsed — the wrong one.

4. **Fallback storms.** When the JSON parser fails, the code falls back to
   a deterministic brain. The deterministic brain works fine for synthetic
   priorities but does not correctly route real goals with open PRs to
   `dispatch_spawn_engineer`. The result: goals with open PRs stall
   indefinitely because engineers are never spawned.

## Design principles

### Text-first, not JSON-tolerant

The old approach tried to make JSON parsing more tolerant — strip fences,
extract between braces, handle whitespace. Each tolerance added complexity
and new failure modes. The text-based protocol inverts the approach: the
wire format is text from the start, and the parser looks for keywords.

A model that responds with `"advance_goal"` or `"advance_goal because..."`
parses correctly. The parser takes the first word; the remaining prose is
the rationale.

### `str` methods only, no regex

All text parsers use `str::split_whitespace`, `str::trim`,
`str::to_ascii_lowercase`, `str::eq_ignore_ascii_case`, and
`str::parse::<f64>`. No regex crate dependency. This eliminates ReDoS risk
and keeps the parser auditable as a sequence of string operations.

### First-word extraction, not scanning

> **Simplified in [#2144](https://github.com/rysweet/Simard/issues/2144).**
> All three OODA brain parsers now use first-word extraction. The keyword-
> anywhere scanning, DECISION marker parsing, and JSON extraction have
> been deleted.

Recipe prompts instruct the LLM to output the decision as the very first
word. The parser splits on whitespace, takes the first token, and matches
it case-insensitively against the known variants. One comparison per variant
— not a scan.

This is strictly more constrained than keyword-anywhere matching: the
parser checks exactly one position (the first word) rather than scanning
the entire text. This eliminates false positives from keywords appearing
in rationale prose, goal names, or log excerpts.

### Bare-float for numeric output

The orient brain expects a bare decimal number as the first token. The
parser scans for the first float in the text and uses it as the urgency
adjustment. No JSON object, no labeled fields.

### Key=value for bash output

The disk health recipe outputs structured data as key=value lines:

```
DISK_USED_PCT=72
FREED_BYTES=53687091200
ACTION: Removed 48 stale worktrees (50.1G)
ACTION: Cleaned cargo-target/ (12.0G)
```

This is natural to produce from bash (`echo "DISK_USED_PCT=$(df ...)"`)
and natural to parse from Rust (`line.split_once('=')`). No quoting
concerns, no escaping, no JSON `printf` fragility.

### Retained `serde_json` is never on LLM output

The remaining `serde_json` usage in the codebase deserializes controlled
Rust-constructed values — not LLM output. No `serde_json::from_str` is
called on any LLM or recipe output.

## The three protocol families

### 1. First-word extraction protocol (OODA brains)

Used by: `recipe_brain.rs` (decide, orient, lifecycle)

> **Simplified in [#2144](https://github.com/rysweet/Simard/issues/2144).**
> All three OODA brain parsers now use first-word extraction. The DECISION
> marker protocol, keyword-anywhere scanning, and JSON extraction have been
> deleted entirely.

The model emits the decision as the very first word. The parser takes
that word, lowercases it, and matches against the known variants:

```
advance_goal PR #2023 is open; engineer needed to drive it to completion.
```

For the orient brain, the first float in the text is extracted:

```
0.60 Standard demotion for 1 failure. The goal is healthy.
```

The parser:
1. Splits on whitespace, takes the first token.
2. Matches case-insensitively against known variants (decide, lifecycle)
   or parses as `f64` (orient).
3. If no match, applies the safe default (`AdvanceGoal`, `ContinueSkipping`,
   or the deterministic floor formula).
4. Remaining text becomes the rationale (truncated to 500 chars).

See [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md)
for the full grammar and variant-specific details.

### 2. Keyword verdict protocol (recipe shims)

Used by: `recipe_progress_checker.rs`, `recipe_merge_judge.rs`

> **Note:** The decide brain was moved **out** of Protocol 2 in
> [#2144](https://github.com/rysweet/Simard/issues/2144). It now uses
> first-word extraction (Protocol 1).

The recipe runs an agent step. The agent's stdout is scanned for a verdict
keyword. Everything else is treated as rationale.

```
After reviewing the plan and progress claims, the evidence supports the
claimed delta. The PR is in flight and the 8-point increase matches the
described work.

accept
```

The parser:
1. Converts stdout to lowercase.
2. Checks for `"not_ready"` (merge judge) or `"reject"` (progress checker)
   first — prevents `"not_ready"` matching as `"ready"`.
3. Checks for `"ready"` or `"accept"`.
4. If no keyword found, defaults to the safe option (`Accept` for progress
   checker — fail-open; `NotReady` for merge judge — fail-closed).
5. Extracts the full stdout (minus the keyword line) as rationale.

### 3. Key=value protocol (disk health)

Used by: `disk_health.rs`

The recipe bash step outputs key=value lines for numeric fields and
`ACTION:` prefixed lines for the human-readable action list:

```
DISK_USED_PCT=72
FREED_BYTES=53687091200
ACTION: Removed 48 stale worktrees (50.1G)
ACTION: Removed cargo target dirs from 3 worktrees (1.2G)
ACTION: Pruned 19 LadybugDB backups (512M)
ACTION: Cleaned cargo-target/ (12.0G) and shared-target/ (2.8G)
```

The parser:
1. Iterates `.lines()`.
2. For `KEY=VALUE` lines: `split_once('=')`, `trim()`, `parse::<u64>()`.
3. For `ACTION:` lines: strips the prefix, collects into `Vec<String>`.
4. Defaults: `disk_used_pct = 0`, `freed_bytes = 0`, `actions_taken = []`.

## Dead code removal

Several modules were deleted or cleaned up as part of the parser elimination:

- **`progress_reviewer.rs`** — `LlmReviewerProgressChecker` and
  `parse_reviewer_response`. Replaced by the keyword verdict parser in
  `recipe_progress_checker.rs`. The `RecipeProgressChecker` now builds
  `EvidenceDecision` directly without the intermediate `ReviewerResponse`
  type.

- **`merge_judge.rs` (partial)** — `LlmMergeJudge`, `parse_judge_response`,
  `extract_fenced_blocks`, `extract_balanced_objects`, `truncate_for_log`.
  The merge judge fallback chain is now `RecipeMergeJudge` →
  `RefusingMergeJudge`. There is no LLM tier. Types (`JudgeOutcome`,
  `Verdict`, `Blocker`, `MergeJudgeKind`), traits (`MergeJudge`), and
  `RefusingMergeJudge` / `build_merge_judge` are retained.

- **`decide.rs` (partial, #2111)** — `RustyClawdDecideBrain`,
  `parse_judgment_from_response`, `build_rustyclawd_decide_brain`. The
  DECISION marker parser and LLM submitter-based brain are removed. The
  decide brain now uses `RecipeBrain` in `recipe_brain.rs`, which
  invokes `recipe-runner-rs` and extracts the first word as the action keyword.

- **`recipe_brain.rs` parser helpers (#2144)** — `ascii_contains_ignore_case`,
  `LIFECYCLE_KEYWORDS`, `try_keyword_scan`, `build_keyword_decision`,
  `parse_with_marker` / `extract_decision_marker`, `try_json_extraction`,
  and `try_bare_float` (renamed to `try_first_float`). These formed the
  multi-tier parse cascades that scanned entire text for keywords or extracted
  JSON. Replaced by trivial first-word extraction in each parse function.

The daemon wiring in `operator_commands_ooda/daemon/mod.rs` was updated to
match: the `LlmReviewerProgressChecker` fallback arm was removed. The chain
is now `RecipeProgressChecker` → `NoopProgressEvidenceChecker`.

## Why this matters for engineer spawning

The parse failures in the decide brain were the root cause of goals with
open PRs not getting engineers spawned. With first-word extraction, the
model's response `"advance_goal PR is open..."` is parsed directly — the
first word `advance_goal` is matched. No fallback needed. Goals with open
PRs flow to `dispatch_spawn_engineer` reliably.

> **Completed across #2111 and #2144:** The decide brain now runs as a
> recipe step with first-word extraction via `RecipeBrain`. All parser
> machinery — keyword scanning, DECISION markers, JSON extraction — has
> been eliminated from `recipe_brain.rs`.

## Security improvements

Removing all parser machinery is a net security improvement:

- **No `find('{')..rfind('}')` boundary attacks.** JSON extraction deleted.
- **No full-text keyword scanning (O(n×m)).** Replaced by single first-word
  comparison (O(k) where k = number of variants, typically 6-10).
- **No labeled-line extraction.** No `TITLE:`, `BODY:`, `REASON:` parsing
  of untrusted LLM text.
- **No `serde_json::from_str` or `serde_json::from_value` on LLM output.**
  The JSON deserialization attack surface is eliminated entirely.
- **No HashMap construction from line-scanning.** The `parse_with_marker`
  function that built key-value maps from LLM output is deleted.
- **`.split_whitespace().next()` is O(1) per call.** No unbounded allocation
  from huge LLM responses.
- **`truncate()` caps all rationale fields.** Never store unbounded LLM text.
- **`OrientJudgment::validate()` retained.** Bounds guard for NaN/inf/out-of-range.
- **Default variants are safe no-ops.** `AdvanceGoal` and `ContinueSkipping`
  have no destructive side effects.

## Related

- [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md) — full grammar for each protocol
- [Reference: OODA Brain API](../reference/ooda-brain-api.md) — trait and type definitions
- [Reference: OODA Brain Decision Protocol](../reference/ooda-brain-decision-protocol.md) — DECISION marker for engineer lifecycle
- [Reference: Progress-evidence API](../reference/progress-evidence-api.md) — keyword verdict for progress checking
- [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — prompt editing after the text migration
- [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
- [Concept: prompt-driven OODA brain](./prompt-driven-ooda-brain.md) — original brain design
- [Concept: progress-evidence gating](./progress-evidence-gating.md) — progress gating design
