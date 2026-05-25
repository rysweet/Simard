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

Used by: `ooda_brain::decide`, `ooda_brain::orient`, `ooda_brain::rustyclawd`

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

### 1a. Decide phase (`decide.rs`)

**Enum:** `DecideJudgment`

**Variant tokens** (case-insensitive):

| Token | Maps to | Notes |
|-------|---------|-------|
| `advance_goal` | `DecideJudgment::AdvanceGoal` | Default for real goal slugs |
| `consolidate_memory` | `DecideJudgment::ConsolidateMemory` | For `__memory__` |
| `run_improvement` | `DecideJudgment::RunImprovement` | For `__improvement__` |
| `poll_developer_activity` | `DecideJudgment::PollDeveloperActivity` | For `__poll_activity__` |
| `extract_ideas` | `DecideJudgment::ExtractIdeas` | For `__extract_ideas__` |
| `research_query` | `DecideJudgment::ResearchQuery` | Reserved |
| `safe_update` | `DecideJudgment::SafeUpdate` | For `__safe_update__` |
| `skip` | `DecideJudgment::Skip` | Explicit skip |

**Labeled fields:** `RATIONALE:`

**Example valid responses:**

Minimal:
```
DECISION: advance_goal
```

With rationale:
```
DECISION: advance_goal
RATIONALE: PR #2023 is open; engineer needed to drive it to completion
```

Prose before decision (ignored):
```
Looking at the priority entry, this is a real goal slug with an open PR.
The engineer should advance it.

DECISION: advance_goal
RATIONALE: ordinary goal id with open PR, default routing
```

**Fallback:** If no `DECISION:` line is found, the deterministic prefix
mapping fires: `__memory__` → `consolidate_memory`, `__improvement__` →
`run_improvement`, etc. Real goal slugs → `advance_goal`.

**Prompt update:** The `ooda_decide.md` prompt's `OUTPUT_FORMAT` section now
instructs models to emit `DECISION: <variant>` instead of a JSON object.
The `EXAMPLES` section shows the text format. The `OPTIONS` section is
unchanged.

---

### 1b. Orient phase (`orient.rs`)

**Struct:** `OrientJudgment`

**Format:** Single-line JSON object (parsed via `serde_json::from_str` after
extracting the `{…}` substring).

| JSON field | Type | Required | Default | Validation |
|------------|------|----------|---------|------------|
| `adjusted_urgency` | `f64` | **Yes** | _(parse fails without it)_ | Must be in `[0.0, base_urgency]`; must be finite |
| `demotion_applied` | `f64` | No | `0.0` | Convenience; daemon recomputes as `base_urgency − adjusted_urgency` |
| `rationale` | `String` | **Yes** | _(parse fails without it)_ | None |
| `confidence` | `f64` | No | `1.0` | Must be in `[0.0, 1.0]` |

Extra fields are silently ignored (forward compatible).

**Example valid responses:**

Full payload (single line):
```json
{"adjusted_urgency": 0.60, "demotion_applied": 0.20, "rationale": "1 failure: standard floor demotion", "confidence": 0.9}
```

With surrounding prose (tolerated — parser extracts first `{` to last `}`):
```
Given a single recent failure, standard demotion.
{"adjusted_urgency": 0.60, "rationale": "1 failure: standard floor demotion", "confidence": 0.9}
```

Minimal valid:
```json
{"adjusted_urgency": 0.6, "rationale": "ok"}
```

**Validation:** `OrientJudgment::validate()` enforces:
- `adjusted_urgency` is finite
- `adjusted_urgency` in `[0.0, 1.0]`
- `adjusted_urgency <= base_urgency` (no escalation; `1e-9` FP slack)

If validation fails, the deterministic floor applies:
`urgency - 0.2 × failure_count`, clamped to `[0.0, 1.0]`.

**Note:** The orient brain does **not** use the `DECISION:` marker protocol
or labeled-line format. Labeled-line responses (e.g. `ADJUSTED_URGENCY: 0.6`)
will fail parsing and trigger the deterministic fallback.

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

Used by: `goal_curation::recipe_progress_checker`, `stewardship::recipe_merge_judge`

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
| `decide.rs` | 8+ | All variant tokens, missing DECISION line, rationale extraction, fallback |
| `orient.rs` | 11+ | Full JSON, defaults, extra fields, prose-wrapped JSON, markdown-fenced JSON, empty, no-JSON, invalid JSON, labeled-line rejection |
| `rustyclawd.rs` | 15+ (T1–T15) | Full behavior matrix per decision protocol reference |
| `recipe_progress_checker.rs` | 4+ | Accept, reject, no keyword (default), mixed case |
| `recipe_merge_judge.rs` | 5+ | Ready, not_ready, unclear, no keyword (default), substring safety |
| `disk_health.rs` | 3+ | Full output, no-cleanup output, malformed lines |

---

## Migration notes for prompt editors

If you maintain OODA brain prompts (`prompt_assets/simard/ooda_*.md`):

1. **OUTPUT_FORMAT sections specify the phase-appropriate format.** The
   `DECISION:` marker protocol is used by the decide and engineer-lifecycle
   brains. The orient brain uses single-line JSON format. Each prompt's
   `OUTPUT_FORMAT` section is the source of truth for its wire format.

2. **EXAMPLES sections use text format.** Update any custom examples you've
   added to follow the text format. JSON examples will not cause parse
   failures (the parser ignores lines it doesn't recognize) but the model
   will learn the wrong output format.

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
