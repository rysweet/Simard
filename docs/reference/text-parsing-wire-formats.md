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

## Protocol 1: First-word extraction (OODA brains)

Used by: all three OODA brain parsers in `recipe_brain.rs`

> **Simplified in [#2144](https://github.com/rysweet/Simard/issues/2144).**
> All three OODA brain parsers (decide, orient, lifecycle) now use the same
> **first-word extraction** pattern. The DECISION marker protocol, keyword
> scanning, and JSON extraction have been deleted. The recipe prompts
> instruct the LLM to output the decision as the first token.

### Grammar

```
response      = first-token *SP rationale
first-token   = 1*<non-whitespace character>
rationale     = <remaining text after first token, trimmed>
```

- `first-token` is extracted by splitting on whitespace and taking the first
  element.
- The token is lowercased via `.to_ascii_lowercase()` for matching.
- Matching uses `.eq_ignore_ascii_case()` — a single comparison per variant,
  not a scan.
- All remaining text after the first word becomes the rationale (truncated
  to 500 chars).

### Common parser pattern

All three OODA brain parsers share the same core logic:

```rust
fn parse_phase_from_text(raw: &str) -> PhaseJudgment {
    let first_word = raw.split_whitespace().next().unwrap_or("");
    let rest = raw[first_word.len()..].trim();
    match first_word.to_ascii_lowercase().as_str() {
        "variant_a" => PhaseJudgment::VariantA { rationale: truncate(rest, 500) },
        "variant_b" => PhaseJudgment::VariantB { rationale: truncate(rest, 500) },
        _ => PhaseJudgment::Default { rationale: truncate(raw, 500) },
    }
}
```

No `serde_json`. No regex. No keyword scanning. Only `str` methods:
`split_whitespace()`, `trim()`, `to_ascii_lowercase()`,
`eq_ignore_ascii_case()`.

---

### 1a. Decide phase (`recipe_brain.rs`)

**Enum:** `DecideJudgment`

The first word of the recipe output is matched case-insensitively against
the 10 action keywords. Default: `AdvanceGoal`.

**Variant tokens** (case-insensitive first word):

| Token | Maps to |
|-------|---------|
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

**Example valid responses:**

```
advance_goal PR #2023 is open; engineer needed to drive it to completion.
```

```
consolidate_memory This is a reserved synthetic ID for memory consolidation.
```

> **Changed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> Previously, the keyword could appear anywhere in the prose (keyword-anywhere
> scanning via `ascii_contains_ignore_case`). Now the keyword must be the
> first word. The recipe YAML prompt instructs the LLM accordingly.

---

### 1b. Orient phase (`recipe_brain.rs`)

**Struct:** `OrientJudgment`

The parser extracts the first float from the output text. This is a 2-tier
parse:

1. **First float** (`try_first_float`) — scans for the first decimal number
   in the text. This becomes `adjusted_urgency`.
2. **Deterministic floor** — `base_urgency - 0.2 × failure_count`, clamped
   to `[0.0, 1.0]`.

**Fields produced:**

| Field | Source | Default |
|-------|--------|---------|
| `adjusted_urgency` | First float in text | Deterministic floor |
| `rationale` | Full text of response | `"deterministic floor"` |
| `confidence` | Always `1.0` | `1.0` |
| `demotion_applied` | `0.0` (daemon recomputes) | `0.0` |

**Example valid responses:**

Bare float as first token (preferred):
```
0.60 Standard demotion for 1 failure. The goal is healthy but needs a penalty.
```

Float embedded in prose (still works — first float is extracted):
```
After analysis, the adjusted urgency should be 0.45 given 2 consecutive failures.
```

**Validation:** `OrientJudgment::validate()` enforces:
- `adjusted_urgency` in `[0.0, 1.0]`
- `adjusted_urgency <= base_urgency` (no escalation)
- `confidence` in `[0.0, 1.0]`

If validation fails, the deterministic floor applies.

> **Removed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The JSON extraction tier (`try_json_extraction` using `serde_json`) has
> been deleted. The orient prompt now instructs the LLM to output a bare
> decimal as its first token. The `try_bare_float` function has been renamed
> to `try_first_float` — logic unchanged.

---

### 1c. Engineer lifecycle (`recipe_brain.rs`)

**Enum:** `EngineerLifecycleDecision`

The first word of the recipe output is matched case-insensitively against
the 6 lifecycle variant names. Default: `ContinueSkipping`.

**Variant tokens** (case-insensitive first word):

| Token | Extra fields (defaults) |
|-------|------------------------|
| `continue_skipping` | _(none)_ |
| `reclaim_and_redispatch` | `redispatch_context: ""` |
| `deprioritize` | _(none)_ |
| `open_tracking_issue` | `title: "OODA stuck"`, `body: truncate(rest, 500)` |
| `mark_goal_blocked` | `reason: truncate(rest, 500)` |
| `consider_self_update` | _(none)_ |

All variants: `rationale` is `truncate(remaining_text, 500)`.

**Example valid responses:**

Simple variant:
```
continue_skipping engineer is making progress, worktree modified 30s ago
```

Structured variant (extra fields use defaults):
```
open_tracking_issue Engineer stuck in compile-error loop for improve-test-coverage. The engineer has been failing for 6 consecutive cycles with E0277 type errors.
```

Reclaim:
```
reclaim_and_redispatch Previous engineer was stuck on type errors. Try a different approach using AuthProvider trait. Worktree idle 7h.
```

> **Removed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> The `DECISION:` marker protocol, labeled-line field extraction
> (`TITLE:`, `BODY:`, `REASON:`, `REDISPATCH_CONTEXT:`, `RATIONALE:`),
> `LIFECYCLE_KEYWORDS` constant, `try_keyword_scan`, `build_keyword_decision`,
> `parse_with_marker`, and `ascii_contains_ignore_case` have all been deleted.
> Structured fields on lifecycle variants now use defaults — the LLM's prose
> after the first word becomes the rationale. The `serde_json::from_value`
> call that constructed `EngineerLifecycleDecision` from parsed fields is
> also removed.

---

## Protocol 2: Keyword verdict (recipe shims)

Used by: `goal_curation::recipe_progress_checker`, `stewardship::recipe_merge_judge`

> **Note:** The decide brain was moved **out** of Protocol 2 in
> [#2144](https://github.com/rysweet/Simard/issues/2144). It now uses
> first-word extraction (Protocol 1, § 1a above), the same pattern as the
> orient and lifecycle brains.

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
| `decide.rs` | 4+ | DeterministicFallback tests |
| `recipe_brain.rs` | 50+ | First-word extraction for all 10 action keywords, first-float extraction, lifecycle first-word for all 6 variants, case-insensitive matching, no-match defaults, empty input, multi-word rationale capture |
| `orient.rs` | 8+ | Legacy JSON round-trip, DeterministicFallback, validation |
| `recipe_progress_checker.rs` | 4+ | Accept, reject, no keyword (default), mixed case |
| `recipe_merge_judge.rs` | 5+ | Ready, not_ready, unclear, no keyword (default), substring safety |
| `disk_health.rs` | 3+ | Full output, no-cleanup output, malformed lines |

---

## Migration notes for prompt editors

If you maintain OODA brain prompts (`prompt_assets/simard/recipes/*.yaml`):

1. **All three brains use first-word/first-float extraction.** The decide
   and lifecycle brains expect the variant name as the first word. The orient
   brain expects a bare decimal as the first token. Do not instruct the model
   to emit JSON, `DECISION:` markers, or keyword-anywhere prose. The first
   token IS the decision.

2. **EXAMPLES sections use first-word format.** For the decide brain:
   `advance_goal PR is open, engineer needed.` For the lifecycle brain:
   `continue_skipping engineer is healthy.` For orient:
   `0.60 Standard demotion for 1 failure.` Mismatched formats cause the
   model to learn the wrong output pattern.

3. **The parser is NOT tolerant of keywords later in the text.** Unlike
   the previous keyword-anywhere scanning, only the first word is checked.
   If the LLM buries the keyword in prose, it will not be matched and the
   default variant will be used. Ensure your prompt examples show the
   keyword as the very first word.

4. **Structured lifecycle fields use defaults.** The labeled-line extraction
   for `TITLE:`, `BODY:`, `REASON:`, `REDISPATCH_CONTEXT:` no longer exists.
   The LLM's prose after the first word becomes the rationale. If you need
   structured fields in the future, update the Rust parser.

See [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
for the full editing guide.

## See Also

- [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale
- [Reference: OODA Brain API](./ooda-brain-api.md) — trait and type definitions
- [Reference: OODA Brain Decision Protocol](./ooda-brain-decision-protocol.md) — engineer lifecycle specifics
- [Reference: Progress-evidence API](./progress-evidence-api.md) — progress checking module
