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

## Protocol 1: First-word match (OODA brains)

Used by: `ooda_brain::recipe_brain` (all three phases)

> **Changed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> All three OODA brain parsers now use the same first-word extraction pattern.
> The `DECISION:` marker protocol, JSON extraction, and keyword-scanning
> fallback chains have been removed.

### Grammar

```
response      = *SP variant-token (*SP free-text)?
variant-token = <known enum variant, matched case-insensitively>
free-text     = <any remaining text — kept as rationale>
```

- `variant-token` is the **first whitespace-delimited word** of the response.
- It is lowercased via `to_ascii_lowercase()` before matching.
- If no known variant matches, a safe default is returned (not a parse error).
- Everything after the first word is the rationale (truncated to 500 chars).

### Common parser shape

All three parsers follow the same pattern:

```rust
let first_word = text.split_whitespace().next()
    .unwrap_or("").to_ascii_lowercase();
match first_word.as_str() {
    "variant_a" => ...,
    "variant_b" => ...,
    _ => /* safe default */,
}
```

No `serde_json`. No regex. No keyword scanning. Only `str::split_whitespace()`,
`eq_ignore_ascii_case()`, and `match`.

---

### 1a. Decide phase (`recipe_brain.rs`)

**Enum:** `DecideJudgment`

**Parser:** `parse_action_from_text(text) -> DecideJudgment`

Extracts the first whitespace-delimited word, lowercases it, and matches
against the 10 action keywords. Defaults to `AdvanceGoal`.

**Keywords:**

| First word | Maps to |
|------------|---------|
| `advance_goal` | `DecideJudgment::AdvanceGoal` |
| `consolidate_memory` | `DecideJudgment::ConsolidateMemory` |
| `run_improvement` | `DecideJudgment::RunImprovement` |
| `poll_developer_activity` | `DecideJudgment::PollDeveloperActivity` |
| `extract_ideas` | `DecideJudgment::ExtractIdeas` |
| `safe_update` | `DecideJudgment::SafeUpdate` |
| `research_query` | `DecideJudgment::ResearchQuery` |
| `run_gym_eval` | `DecideJudgment::RunGymEval` |
| `build_skill` | `DecideJudgment::BuildSkill` |
| `launch_session` | `DecideJudgment::LaunchSession` |

**Example recipe stdout:**

```
consolidate_memory Memory hasn't been consolidated in 12 hours.
```

> **Removed in #2144:** `ascii_contains_ignore_case()` keyword scanning. The
> old parser scanned the entire response for keywords anywhere in the text.
> The new parser only checks the first word.

---

### 1b. Orient phase (`recipe_brain.rs`)

**Struct:** `OrientJudgment`

**Parser:** `parse_orient_from_text(text, base_urgency, failure_count) -> OrientJudgment`

2-tier parse:

1. **First float** — `try_first_float(text)` scans for the first substring
   matching `[0-9]+.[0-9]+` and parses it as `f64`. This becomes
   `adjusted_urgency`.
2. **Deterministic floor** — `base_urgency - 0.2 × failure_count`, clamped
   to `[0.0, 1.0]`.

**Parsed fields:**

| Field | Source | Value |
|-------|--------|-------|
| `adjusted_urgency` | first float token | Parsed as `f64` |
| `rationale` | full response text | Entire model response (truncated) |
| `confidence` | parser default | Always `1.0` |
| `demotion_applied` | computed | `base_urgency - adjusted_urgency` |

**Example recipe stdout:**

```
0.6 Standard floor demotion applied
```

**Validation:** `OrientJudgment::validate()` enforces:
- `adjusted_urgency` in `[0.0, 1.0]`
- `adjusted_urgency <= base_urgency` (no escalation)
- `confidence` in `[0.0, 1.0]`

If validation fails, the deterministic floor applies.

> **Removed in #2144:** `try_json_extraction()` (tier 1 JSON `{…}` extraction
> via `serde_json::from_str`). The orient prompt now instructs the LLM to
> output a bare decimal as its first token.

---

### 1c. Engineer lifecycle (`recipe_brain.rs`)

**Enum:** `EngineerLifecycleDecision`

**Parser:** `parse_lifecycle_from_text(text) -> EngineerLifecycleDecision`

Extracts the first whitespace-delimited word, lowercases it, and matches
against the 6 lifecycle variant names. Defaults to `ContinueSkipping`.

**Keywords:**

| First word | Maps to |
|------------|---------|
| `continue_skipping` | `EngineerLifecycleDecision::ContinueSkipping` |
| `reclaim_and_redispatch` | `EngineerLifecycleDecision::ReclaimAndRedispatch` |
| `deprioritize` | `EngineerLifecycleDecision::Deprioritize` |
| `open_tracking_issue` | `EngineerLifecycleDecision::OpenTrackingIssue` |
| `mark_goal_blocked` | `EngineerLifecycleDecision::MarkGoalBlocked` |
| `consider_self_update` | `EngineerLifecycleDecision::ConsiderSelfUpdate` |

Extra fields use defaults:
- `open_tracking_issue` → `title: "OODA stuck"`, `body: truncate(remaining_text, 500)`
- `mark_goal_blocked` → `reason: truncate(remaining_text, 500)`
- `reclaim_and_redispatch` → `redispatch_context: ""`
- All variants: `rationale: truncate(text_after_first_word, 500)`

**Example recipe stdout:**

```
reclaim_and_redispatch Engineer stuck on type errors for 12 cycles.
```

> **Removed in #2144:** `DECISION:` marker parsing, labeled-line field
> extraction (`TITLE:`, `BODY:`, `REASON:`, `REDISPATCH_CONTEXT:`),
> `serde_json::from_value` conversion, `LIFECYCLE_KEYWORDS` constant,
> `try_keyword_scan()`, `build_keyword_decision()`, `parse_with_marker()`,
> and `extract_decision_marker()`.

---

## Protocol 2: Keyword verdict (recipe shims)

Used by: `goal_curation::recipe_progress_checker`, `stewardship::recipe_merge_judge`

> **Changed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The decide brain has moved from this protocol to the first-word match
> protocol (§ 1a above). The keyword verdict protocol is now used only by
> the progress checker and merge judge.

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

### 2c. Decide brain — MOVED to first-word match protocol

> **Moved in [#2144](https://github.com/rysweet/Simard/issues/2144).**
> The decide brain now uses the first-word match protocol (§ 1a above).
> It no longer scans the entire response for keywords — it only checks the
> first word. See § 1a for the current wire format.

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
- `source: BrainParseSource` — the specific parse failure context.

For the first-word match parsers (decide, lifecycle), an unrecognized first
word returns a safe default rather than an error. Only truly unparseable
input (empty response, no whitespace tokens) triggers the error path.

For the orient parser, a missing float triggers the deterministic floor
fallback (not an error).

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
| `decide.rs` | 4+ | DeterministicFallback tests |
| `recipe_brain.rs` | 30+ | All 10 action keywords (first-word), first-float orient, 6 lifecycle variants (first-word), case-insensitive match, unrecognized defaults |
| `orient.rs` | 8+ | Float parsing, validation, extra fields, empty/invalid responses |
| `rustyclawd.rs` | 15+ (T1–T15) | Full behavior matrix per decision protocol reference |
| `recipe_progress_checker.rs` | 4+ | Accept, reject, no keyword (default), mixed case |
| `recipe_merge_judge.rs` | 5+ | Ready, not_ready, unclear, no keyword (default), substring safety |
| `disk_health.rs` | 3+ | Full output, no-cleanup output, malformed lines |

---

## Migration notes for prompt editors

If you maintain OODA brain prompts (`prompt_assets/simard/recipes/ooda-*.yaml`):

1. **All three brains use first-word/first-float extraction.** The decide and
   lifecycle brains extract the first word and match case-insensitively against
   known variants. The orient brain extracts the first decimal number.
   Do not use `DECISION:` markers, JSON objects, or keyword-anywhere patterns
   in brain prompts — they are no longer parsed.

2. **EXAMPLES sections must put the decision first.** The first word of every
   example response must be the variant name (for decide/lifecycle) or a bare
   decimal (for orient). Free-form rationale follows on the same line.

3. **The parser is strict about position, tolerant about content.** Only the
   first token matters. Everything after it is rationale text. The model can
   emit as much prose as it wants after the first word.

4. **Extra structured fields are not parsed from output.** Variants with extra
   fields (`open_tracking_issue`, `mark_goal_blocked`, `reclaim_and_redispatch`)
   use defaults. Do not instruct the LLM to emit `TITLE:` or `REASON:` labels
   — they will be ignored.

> **Removed in #2144:** JSON object format, `DECISION:` marker format,
> labeled-line extraction, keyword-anywhere scanning.

See [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
for the full editing guide.

## See Also

- [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
- [Reference: OODA Brain API](./ooda-brain-api.md) — trait and type definitions
- [Reference: OODA Brain Decision Protocol](./ooda-brain-decision-protocol.md) — engineer lifecycle specifics
- [Reference: Progress-evidence API](./progress-evidence-api.md) — progress checking module
