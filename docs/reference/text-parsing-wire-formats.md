---
title: Text-parsing wire formats
description: Normative reference for every text-based wire format Simard's Rust code parses from LLM and recipe output. Replaces the former JSON-based contracts.
last_updated: 2026-05-24
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/text-based-brain-protocol.md
  - ./ooda-brain-api.md
  - ./ooda-brain-decision-protocol.md
  - ./disk-health-api.md
  - ./progress-evidence-api.md
---

# Reference: Text-parsing wire formats

Crate: `simard`

This page is the normative definition of every text-based wire format that
Simard's Rust code parses from LLM or recipe output. There are three
protocol families, each used at specific decision sites.

For the design rationale, see
[Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md).

---

## Protocol 1: DECISION marker (OODA brains)

Used by: `ooda_brain::rustyclawd`

> **Note:** `ooda_brain::decide` has moved to the keyword verdict protocol
> (Protocol 2, § 2c) as of #2111. `ooda_brain::orient` uses JSON format —
> see [§ 1b. Orient phase](#1b-orient-phase-orientrs).

### Grammar

```
response     = *blank-line decision-line *body-line
decision-line = "DECISION" *SP ":" *SP variant-token *SP LF
body-line    = labeled-line / rationale-line
labeled-line = label-token *SP ":" *SP value LF
rationale-line = <any line not matching labeled-line> LF
blank-line   = *SP LF
```

- `variant-token` is matched case-insensitively against the known enum variants
  for the specific brain (see per-brain sections below).
- `label-token` is matched case-insensitively against the known field labels
  for the matched variant.
- `value` extends to end-of-line, trimmed.
- Lines before the first `DECISION:` line are ignored.
- Rationale lines (non-labeled lines after `DECISION:`) are concatenated with
  newlines to form the `rationale` field, unless a `RATIONALE:` labeled line
  is present (which takes precedence).

### Common parser implementation

All three OODA brain parsers share the same core logic:

```rust
fn parse_decision_text(raw: &str, known_variants: &[&str]) -> ParseResult {
    // 1. Find first non-blank line starting with "DECISION:" (case-insensitive)
    // 2. Extract variant token, match against known_variants
    // 3. Scan remaining lines for labeled fields
    // 4. Collect unlabeled lines as rationale (fallback)
    // 5. Return structured decision with text fields
}
```

No `serde_json`. No regex. Only `str` methods: `trim()`, `starts_with()`,
`split_once()`, `to_lowercase()`, `parse::<f64>()`.

---

### 1a. Decide phase — MOVED to keyword verdict protocol

> **Moved in [#2111](https://github.com/rysweet/Simard/issues/2111).**
> The decide brain no longer uses the DECISION marker protocol. It now uses
> the **keyword verdict protocol** (§ 2c below), the same pattern as the
> progress checker and merge judge. The decide prompt is now a recipe YAML
> at `prompt_assets/simard/recipes/ooda-decide.yaml`, invoked via
> `recipe-runner-rs`. See
> [§ 2c. Decide brain](#2c-decide-brain-recipe_deciders) for the current
> wire format.
>
> The `DECISION:` marker parser (`parse_judgment_from_response`) and
> `RustyClawdDecideBrain` have been deleted. The `DECIDE_PROMPT_NAME`
> constant is retained for audit-trail versioning only.

---

### 1b. Orient phase (`orient.rs`)

**Struct:** `OrientJudgment`

**JSON fields:**

| Field | Type | Default | Validation |
|-------|------|---------|------------|
| `adjusted_urgency` | `f64` | Required (parse fails without it) | Must be in `[0.0, base_urgency]` |
| `demotion_applied` | `f64` | `0.0` (daemon recomputes) | Must be ≥ 0 |
| `rationale` | `String` | Required (parse fails without it) | None |
| `confidence` | `f64` | `1.0` | Must be in `[0.0, 1.0]` |

**Example valid responses:**

Full JSON object:
```json
{"adjusted_urgency": 0.60, "demotion_applied": 0.20, "rationale": "1 failure: standard floor demotion", "confidence": 0.9}
```

With surrounding prose (tolerated, not encouraged):
```
Given a single recent failure, I'll apply the standard floor demotion.
{"adjusted_urgency": 0.60, "rationale": "1 failure: standard floor demotion", "confidence": 0.9}
```

Minimal valid (confidence defaults to 1.0, demotion_applied defaults to 0.0):
```json
{"adjusted_urgency": 0.6, "rationale": "standard demotion"}
```

**Validation:** `OrientJudgment::validate()` enforces:
- `adjusted_urgency` in `[0.0, 1.0]`
- `adjusted_urgency <= base_urgency` (no escalation)
- `confidence` in `[0.0, 1.0]`

If validation fails, the deterministic floor applies:
`urgency - 0.2 × failure_count`, clamped to `[0.0, 1.0]`.

**Parser:** The `parse_judgment_from_response` function extracts the first
`{…}` substring from the response and deserializes it via `serde_json`.
The old labeled-line format is no longer accepted.

---

### 1c. Engineer lifecycle (`rustyclawd.rs`)

**Enum:** `EngineerLifecycleDecision`

This is the original DECISION marker protocol. The JSON fallback path that
previously existed (lines 306-318 in the old code) has been removed.
`parse_decision_from_response` now uses the DECISION marker as its **sole**
parser.

**Variant tokens** (case-insensitive):

| Token | Required labeled fields |
|-------|----------------------|
| `continue_skipping` | _(none)_ |
| `reclaim_and_redispatch` | `REDISPATCH_CONTEXT:` |
| `deprioritize` | _(none)_ |
| `open_tracking_issue` | `TITLE:`, `BODY:` |
| `mark_goal_blocked` | `REASON:` |
| `consider_self_update` | _(none)_ |

**All variants** accept an optional `RATIONALE:` label. If absent, non-labeled
lines after the DECISION line are concatenated as the rationale. If no rationale
text is found at all, the default `"<no rationale provided>"` is used.

**Example valid responses:**

Simple variant:
```
DECISION: continue_skipping
RATIONALE: engineer is making progress, worktree modified 30s ago
```

Structured variant:
```
DECISION: open_tracking_issue
TITLE: Engineer stuck in compile-error loop for improve-test-coverage
BODY: The engineer has been failing for 6 consecutive cycles with E0277 type errors. The worktree has not been modified in 25200 seconds.
RATIONALE: Persistent failure pattern needs human attention.
```

Reclaim with context:
```
DECISION: reclaim_and_redispatch
REDISPATCH_CONTEXT: Previous engineer was stuck on type errors in src/auth. Try a different approach using the existing AuthProvider trait.
RATIONALE: 12 consecutive skips with no worktree modification.
```

**Retained `serde_json::from_value`:** After the text parser extracts all
fields from labeled lines, it constructs a `serde_json::Value::Object` from
the parsed strings and deserializes it into `EngineerLifecycleDecision`. This
is **not** parsing LLM output — it is deserializing a controlled construction
from text-parsed fields. A `// SAFETY:` comment marks this call.

---

## Protocol 2: Keyword verdict (recipe shims)

Used by: `goal_curation::recipe_progress_checker`, `stewardship::recipe_merge_judge`, `ooda_brain::recipe_decide`

### Grammar

```
response = *line verdict-keyword *line
verdict-keyword = <case-insensitive match of a known keyword>
```

The parser scans the entire stdout for a verdict keyword. Everything else
(all lines that are not the keyword) is collected as the rationale.

### Scanning rules

1. Convert stdout to lowercase for matching.
2. Check for the **negative** keyword first to prevent substring false positives.
3. Check for the **positive** keyword.
4. If no keyword found, apply the safe default.
5. Extract surrounding text as rationale.

---

### 2a. Progress checker (`recipe_progress_checker.rs`)

**Keywords:**

| Keyword | Maps to | Priority |
|---------|---------|----------|
| `reject` | `EvidenceDecision::Reject` | Checked first |
| `accept` | `EvidenceDecision::Accept` | Checked second |

**Default (no keyword):** `EvidenceDecision::Accept` — fail-open. The gate's
purpose is to catch hallucinated jumps, not to block goals on keyword-detection
availability.

**Example recipe stdout:**

```
After reviewing the plan and progress claims:

The goal "improve-amplihack-test-coverage" claims progress from 35% to 43%.
The plan describes adding integration tests for the recipe runner, and the
WIP summary references new test files in tests/integration/.

The 8-point increase is proportional to the described work.

accept
```

Parser result: `EvidenceDecision::Accept { reason: "After reviewing the plan and progress claims: ..." }`

**Changes from prior implementation:**

- `parse_reviewer_response` (which parsed JSON `ReviewerResponse`) is removed.
- `RecipeProgressChecker::check()` now calls `parse_verdict_from_text()`
  directly and returns `EvidenceDecision` without the intermediate
  `ReviewerResponse` type.
- The `progress_reviewer.rs` module (containing `LlmReviewerProgressChecker`)
  is deleted. It was dead code — the daemon wiring already used
  `RecipeProgressChecker` as the primary tier.
- The daemon fallback chain is now: `RecipeProgressChecker` →
  `NoopProgressEvidenceChecker` (was: `RecipeProgressChecker` →
  `LlmReviewerProgressChecker` → `NoopProgressEvidenceChecker`).

---

### 2b. Merge judge (`recipe_merge_judge.rs`)

**Keywords:**

| Keyword | Maps to | Priority |
|---------|---------|----------|
| `not_ready` | `Verdict::NotReady` | Checked first (prevents `ready` substring match) |
| `unclear` | `Verdict::NotReady` | Checked second (conservative — unclear is not ready) |
| `ready` | `Verdict::Ready` | Checked third |

**Default (no keyword):** `Verdict::NotReady` — fail-closed. A PR that
cannot be judged is not merged.

**Example recipe stdout:**

```
Reviewing PR #2023 against merge criteria:

- CI status: all checks passing
- Code review: approved by 1 reviewer
- Test coverage: new tests added for all changed modules
- No breaking API changes

The PR meets all merge-readiness criteria.

ready
```

Parser result: `JudgeOutcome { verdict: Verdict::Ready, rationale: "Reviewing PR #2023 against merge criteria: ...", blockers: [] }`

**Note on blockers:** The keyword verdict parser does not extract structured
`Blocker` entries. `JudgeOutcome.blockers` is always empty when parsed from
text. The recipe agent's prose rationale contains the equivalent information
in human-readable form. If structured blockers are needed in the future, the
recipe prompt can be updated to emit `BLOCKER:` labeled lines.

**Changes from prior implementation:**

- `parse_judge_response` (which parsed JSON) is removed from `merge_judge.rs`.
- `LlmMergeJudge` is removed from `merge_judge.rs`.
- Helper functions `extract_fenced_blocks`, `extract_balanced_objects`, and
  `truncate_for_log` are removed from `merge_judge.rs`.
- `build_merge_judge()` chain is now: `RecipeMergeJudge` →
  `RefusingMergeJudge` (was: `RecipeMergeJudge` → `LlmMergeJudge` →
  `RefusingMergeJudge`).

---

### 2c. Decide brain (`recipe_decide.rs`)

> **New in [#2111](https://github.com/rysweet/Simard/issues/2111).**
> The decide brain moved from the DECISION marker protocol (§ 1a, now
> deleted) to the keyword verdict protocol. This follows the same pattern
> as the progress checker and merge judge.

**Keywords:**

| Keyword | Maps to | Notes |
|---------|---------|-------|
| `advance_goal` | `DecideJudgment::AdvanceGoal` | Default for real goal slugs |
| `consolidate_memory` | `DecideJudgment::ConsolidateMemory` | For `__memory__` |
| `run_improvement` | `DecideJudgment::RunImprovement` | For `__improvement__` |
| `poll_developer_activity` | `DecideJudgment::PollDeveloperActivity` | For `__poll_activity__` |
| `extract_ideas` | `DecideJudgment::ExtractIdeas` | For `__extract_ideas__` |
| `safe_update` | `DecideJudgment::SafeUpdate` | For `__safe_update__` |
| `research_query` | `DecideJudgment::ResearchQuery` | Reserved |
| `run_gym_eval` | `DecideJudgment::RunGymEval` | Reserved |
| `build_skill` | `DecideJudgment::BuildSkill` | Reserved |
| `launch_session` | `DecideJudgment::LaunchSession` | Reserved |

**Default (no keyword):** `DecideJudgment::AdvanceGoal` — fail-safe. An
unrecognized response routes to the most common action kind. This matches
the existing `DeterministicFallbackDecideBrain` behavior for real goal slugs.

**Keyword safety:** No keyword is a substring of another. This was verified
by exhaustive pairwise comparison of the 10 keywords. Unlike the merge
judge (where `not_ready` contains `ready`), no ordering-dependent
disambiguation is needed.

**Example recipe stdout:**

```
Looking at goal "__memory__", this is a reserved synthetic ID for memory
consolidation. The urgency is high at 0.85 because memory hasn't been
consolidated in 12 hours.

The appropriate action is consolidate_memory.
```

Parser result: `DecideJudgment::ConsolidateMemory`

```
This is goal "ship-v1", an ordinary goal slug with an open PR and
moderate urgency. The engineer should advance_goal to drive the PR
to completion.
```

Parser result: `DecideJudgment::AdvanceGoal`

**Changes from prior implementation:**

- `parse_judgment_from_response` (which parsed `DECISION:` markers from
  LLM output) is removed from `decide.rs`.
- `RustyClawdDecideBrain` (which compiled in the prompt via `include_str!`
  and submitted to an `LlmSubmitter`) is removed from `decide.rs`.
- `build_rustyclawd_decide_brain` factory function is removed.
- `RecipeDecideBrain` in `recipe_decide.rs` replaces the above — it
  invokes `recipe-runner-rs` as a subprocess and scans stdout for keywords.
- The prompt is now a recipe YAML at
  `prompt_assets/simard/recipes/ooda-decide.yaml`, not a compiled-in
  markdown file. The prompt's `OUTPUT_FORMAT` section and line-1
  `CRITICAL:` guard have been removed — the agent is no longer required
  to emit a specific format.
- The daemon wiring in `build_decide_brain()` now constructs
  `RecipeDecideBrain::new(repo_root)` instead of
  `build_rustyclawd_decide_brain(state_root)`.

---

## Protocol 3: Key=value (disk health)

Used by: `disk_health`

### Grammar

```
response    = *output-line
output-line = kv-line / action-line / ignored-line
kv-line     = key "=" value LF
action-line = "ACTION:" *SP description LF
ignored-line = <any line not matching kv-line or action-line> LF
key         = "DISK_USED_PCT" / "FREED_BYTES"
value       = 1*DIGIT
description = <text to end of line>
```

### Known keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `DISK_USED_PCT` | `u64` | `0` | Disk usage percentage after cleanup (0–100) |
| `FREED_BYTES` | `u64` | `0` | Bytes freed during this check |
| `ACTION` | `Vec<String>` | `[]` | Collected from all `ACTION:` lines |

### Example recipe stdout

```
DISK_USED_PCT=72
FREED_BYTES=53687091200
ACTION: Removed 48 stale worktrees (50.1G)
ACTION: Removed cargo target dirs from 3 worktrees (1.2G)
ACTION: Pruned 19 LadybugDB backups (512M)
ACTION: Cleaned cargo-target/ (12.0G) and shared-target/ (2.8G)
```

When disk usage is below threshold (no cleanup):

```
DISK_USED_PCT=65
FREED_BYTES=0
```

### Bash production

The recipe YAML bash step produces this format naturally:

```bash
USED_PCT=$(df /home --output=pcent | tail -1 | tr -d ' %')
echo "DISK_USED_PCT=${USED_PCT}"
echo "FREED_BYTES=${TOTAL_FREED}"
for action in "${ACTIONS[@]}"; do
  echo "ACTION: ${action}"
done
```

No JSON `printf`, no quoting concerns, no escaping. Filenames with special
characters in action descriptions are safe — they're just text after `ACTION:`.

### Changes from prior implementation

- `DiskHealthReport` retains `Serialize` (for logging/state) but `Deserialize`
  is removed.
- `run_disk_health_check` calls `parse_disk_health_text()` instead of
  `serde_json::from_slice()`.
- The recipe YAML (`disk-health-check.yaml`) outputs key=value lines instead
  of a JSON object.

---

## Error handling

All text parsers return `SimardError::BrainResponseUnparseable` (or the
site-specific error variant) when parsing fails. The error carries:

- `raw: String` — the **complete, untruncated** text that was received.
- `source: BrainParseSource::Marker(MarkerParseError)` — the specific
  parse failure (missing DECISION line, unknown variant, missing required
  field, validation failure).

Parse failures are logged at `ERROR` level with the full raw response
(truncated to 8 KiB at log-format time). The `ParseFailureRecord` channels
(structured log, metric, cycle report, GitHub issue escalation) continue to
function as documented in
[diagnose-decide-orient-parse-failures](../howto/diagnose-decide-orient-parse-failures.md).

The deterministic fallback brains (`DeterministicFallbackDecideBrain`,
`DeterministicFallbackOrientBrain`, `DeterministicFallbackBrain`) continue
to serve as the no-LLM bootstrap path. They are **not** silent error
handlers — the parse failure is surfaced through all four visibility channels
before the fallback is applied.

---

## Test inventory

Each parser has inline `#[cfg(test)]` tests in its source file:

| Module | Test count | Coverage |
|--------|-----------|----------|
| `decide.rs` | 4+ | JSON round-trip, DeterministicFallback tests |
| `recipe_decide.rs` | 10+ | All 10 action keywords, no keyword (default), mixed case, multiple keywords |
| `orient.rs` | 8+ | JSON parsing, surrounding prose, missing fields, validation, extra fields, markdown fences, empty/invalid responses, labeled-line rejection |
| `rustyclawd.rs` | 15+ (T1–T15) | Full behavior matrix per decision protocol reference |
| `recipe_progress_checker.rs` | 4+ | Accept, reject, no keyword (default), mixed case |
| `recipe_merge_judge.rs` | 5+ | Ready, not_ready, unclear, no keyword (default), substring safety |
| `disk_health.rs` | 3+ | Full output, no-cleanup output, malformed lines |

---

## Migration notes for prompt editors

If you maintain OODA brain prompts (`prompt_assets/simard/ooda_*.md` or
`prompt_assets/simard/recipes/ooda-decide.yaml`):

1. **Each brain has its own wire format.** The engineer-lifecycle brain
   (`rustyclawd.rs`) uses `DECISION:` marker format. The orient brain uses
   **JSON object format**. The decide brain uses **keyword verdict** format
   (action keyword anywhere in prose — no markers, no structured output).
   Do not mix these formats across brains.

2. **EXAMPLES sections use the brain's wire format.** For the engineer-lifecycle
   brain, use text `DECISION:` examples. For orient, use JSON object examples.
   For decide, use natural-prose examples containing the action keyword.
   Mismatched formats cause the model to learn the wrong output pattern.

3. **The parser is more tolerant than JSON.** Models can emit prose before
   and after the structured content. This is by design — the parser finds
   the keywords/labels it needs and ignores everything else.

4. **Forward compatibility is preserved.** Unknown labels are ignored.
   New labeled fields can be added to prompts without code changes — they
   just won't be parsed until the Rust parser is updated.

See [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
for the full editing guide.

## See Also

- [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
- [Reference: OODA Brain API](./ooda-brain-api.md) — trait and type definitions
- [Reference: OODA Brain Decision Protocol](./ooda-brain-decision-protocol.md) — engineer lifecycle specifics
- [Reference: Disk Health API](./disk-health-api.md) — disk health module
- [Reference: Progress-evidence API](./progress-evidence-api.md) — progress checking module
