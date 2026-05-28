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
and progress checker uses text-based wire formats — first-word matching,
first-float extraction, keyword scanning, or key=value pairs — that the Rust
code parses with `str` methods. No `serde_json::from_str` on model output.
No regex crate. No extraction heuristics like `find('{')..rfind('}')`.

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

A model that responds with `"advance_goal"` or `"advance_goal drive the PR"`
all parse correctly — the first word is the decision; everything after is
rationale.

### `str` methods only, no regex

All text parsers use `str::split_whitespace`, `eq_ignore_ascii_case`,
`str::trim`, and `str::parse::<f64>`. No regex crate dependency. This
eliminates ReDoS risk and keeps the parser auditable as a sequence of
string operations.

### First-word matching for decisions

> **Changed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> All three OODA brain parsers (decide, orient, lifecycle) now use
> first-word/first-float extraction instead of keyword scanning or markers.

The OODA brain parsers extract the **first whitespace-delimited token** from
the model response, lowercase it, and match against known variants. This is
simpler than keyword scanning (which checked every token in the response) and
simpler than the `DECISION:` marker protocol (which required a specific line
format with labeled fields).

Recipe shim parsers (progress checker, merge judge) still use keyword scanning
of the full response — their prompts are not under our control in the same
way, and the keyword-anywhere pattern remains appropriate for them.

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

There are no remaining `serde_json` calls on LLM output. The `OrientJudgment`
struct retains `Deserialize` for internal use, but the parser never calls
`serde_json::from_str` on model text.

> **Removed in #2144:** The `serde_json::from_value` call in the lifecycle
> parser that deserialized text-parsed fields into `EngineerLifecycleDecision`.

## The three protocol families

### 1. First-word match protocol (OODA brains)

Used by: `recipe_brain.rs` (all three phases)

> **Changed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> All three OODA brain parsers now use the same first-word extraction pattern.

The model emits the decision word as its **first token**. For decide and
lifecycle, this is a variant name. For orient, this is a bare decimal number.
Everything after the first word is treated as rationale.

```
reclaim_and_redispatch Engineer stuck on type errors for 12 cycles.
```

The parser:
1. Calls `split_whitespace().next()` to extract the first token.
2. Lowercases it via `to_ascii_lowercase()`.
3. Matches against the known variant whitelist.
4. On match: constructs the variant with default extra fields and remaining
   text as rationale.
5. On no match: returns the safe default (e.g., `AdvanceGoal`, `ContinueSkipping`).

See [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md)
for the full grammar and variant-specific field requirements.

### 2. Keyword verdict protocol (recipe shims)

Used by: `recipe_progress_checker.rs`, `recipe_merge_judge.rs`

> **Changed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The decide brain has moved from this protocol to the first-word match
> protocol (§ 1). The keyword verdict protocol is now used only by the
> progress checker and merge judge.

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

Several modules were deleted or cleaned up as part of the text-migration changes:

- **`progress_reviewer.rs`** — `LlmReviewerProgressChecker` and
  `parse_reviewer_response`. Replaced by the keyword verdict parser in
  `recipe_progress_checker.rs`.

- **`merge_judge.rs` (partial)** — `LlmMergeJudge`, `parse_judge_response`,
  `extract_fenced_blocks`, `extract_balanced_objects`, `truncate_for_log`.

- **`decide.rs` (partial, #2111)** — `RustyClawdDecideBrain`,
  `parse_judgment_from_response`, `build_rustyclawd_decide_brain`.

- **`recipe_brain.rs` (partial, #2144)** — `ascii_contains_ignore_case()`,
  `try_json_extraction()`, `try_bare_float()`, `parse_with_marker()`,
  `extract_decision_marker()`, `try_keyword_scan()`, `build_keyword_decision()`,
  `LIFECYCLE_KEYWORDS`. All three parse functions (`parse_action_from_text`,
  `parse_orient_from_text`, `parse_lifecycle_from_text`) rewritten as trivial
  first-word/first-float extractors.

The daemon wiring in `operator_commands_ooda/daemon/mod.rs` was updated to
match: the `LlmReviewerProgressChecker` fallback arm was removed. The chain
is now `RecipeProgressChecker` → `NoopProgressEvidenceChecker`.

## Why this matters for engineer spawning

The JSON parse failures in `decide.rs` were the root cause of goals with
open PRs not getting engineers spawned. The flow:

1. Orient phase produces priority for `improve-amplihack-test-coverage`
   with `urgency: 0.8`.
2. Decide phase asks the LLM brain for an action kind.
3. LLM responds with `advance_goal` (correct!) but the response is prose,
   not JSON.
4. `serde_json::from_str::<DecideJudgment>` fails.
5. Fallback to deterministic mapping: check if `goal_id` starts with `__`.
   It doesn't, so return `advance_goal`.
6. This works — but only because the deterministic mapping happens to
   produce the right answer for this case. For edge cases where the LLM
   would have routed differently (e.g., `research_query`), the fallback
   silently overrides the model's judgment.

With first-word extraction (#2144), step 3 succeeds directly. The model's
first word `"advance_goal"` is matched immediately. No keyword scanning,
no fallback needed. Goals with open PRs flow to `dispatch_spawn_engineer`
reliably.

> **Completed in [#2111](https://github.com/rysweet/Simard/issues/2111) and
> [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The decide brain now uses first-word extraction. The `DECISION:` marker
> parser, keyword scanner, JSON extractor, and `RustyClawdDecideBrain` have
> all been deleted. Parse failures from the OODA brains are eliminated.

## Security improvements

Removing the JSON extraction patterns is a net security improvement:

- **No `find('{')..rfind('}')` boundary attacks.** A model that emits
  multiple JSON objects no longer gets the wrong one parsed.
- **No quoting/escaping concerns in bash recipes.** Key=value lines don't
  need JSON-safe quoting of filenames or paths.
- **`.lines()` iteration is lazy with early break.** No unbounded
  allocation from huge LLM responses.
- **`str` methods are constant-time per character.** No backtracking
  regex that could be exploited with crafted input.

## Related

- [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md) — full grammar for each protocol
- [Reference: OODA Brain API](../reference/ooda-brain-api.md) — trait and type definitions
- [Reference: OODA Brain Decision Protocol](../reference/ooda-brain-decision-protocol.md) — first-word match for engineer lifecycle
- [Reference: Progress-evidence API](../reference/progress-evidence-api.md) — keyword verdict for progress checking
- [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md) — prompt editing after the text migration
- [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
- [Concept: prompt-driven OODA brain](./prompt-driven-ooda-brain.md) — original brain design
- [Concept: progress-evidence gating](./progress-evidence-gating.md) — progress gating design
