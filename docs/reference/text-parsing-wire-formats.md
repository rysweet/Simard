---
title: Text-parsing wire formats
description: Normative reference for every text-based wire format Simard's Rust code parses from LLM and recipe output. Replaces the former JSON-based contracts.
last_updated: 2026-06-29
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/text-based-brain-protocol.md
  - ../concepts/copilot-launcher-preamble-stripping.md
  - ./ooda-brain-api.md
  - ./ooda-brain-decision-protocol.md
  - ./recipe-brain-verdict-parsing.md
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

## Protocol 0: Shared noise pre-stripping (`recipe_output`)

Used by: **every** recipe-backed parser below, before it runs.

> **Added in [#2484](https://github.com/rysweet/Simard/issues/2484):**
> `recipe-runner-rs` stdout (and the `step_results[].output` string inside its
> `--output-format json` envelope) is routinely contaminated with **four** kinds
> of non-payload noise that broke the formerly bespoke per-phase extractors:
>
> 1. **ANSI SGR/CSI/OSC colour codes** from `tracing`/`env_logger` (e.g. a
>    leading `\x1b[2m` "dim" whose raw `ESC`/`0x1b` byte is invalid inside a
>    JSON document, so `serde_json` rejects the span).
> 2. **Timestamped tracing-log lines** interleaved with the agent answer.
> 3. The runner's **text-mode summary banner** (`Recipe: … SUCCESS`, `Steps: …`,
>    `[completed] …`).
> 4. **Copilot CLI launch-log preamble** (added in
>    [#2496](https://github.com/rysweet/Simard/issues/2496), generalised here
>    from the distill path that PR [#2500](https://github.com/rysweet/Simard/pull/2500)
>    pinned). The Copilot agent binary prepends launcher lines that carry **no**
>    ISO-8601 timestamp — so category 2 above did not catch them: the
>    `ℹ NODE_OPTIONS=… (saved preference)` info marker, `Run 'copilot update'…`
>    nags, `… launching copilot binary=… version="GitHub Copilot CLI …"`
>    lines, and leading `INFO`/`WARN` launcher lines. Left in place, the first
>    token of the cleaned text was `ℹ`/`Run`/the version string `1.0.66-2`
>    instead of an action keyword or urgency decimal, so **every** decide/orient
>    parse missed and the goal deadlocked (see
>    [Concept: Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md)).
>
> The single hardened `src/recipe_output/` module is now the **only**
> ANSI/log/banner/launcher-stripping path. The two former duplicate strippers
> (`meeting_backend::sanitize::strip_ansi_escapes`, `stewardship::dedup::normalize`)
> and the distill-private ANSI/launcher stripper now delegate to it. Extending
> the one `is_noise_line` predicate re-hardens **every** consumer — decide,
> orient, engineer-lifecycle, merge-judge, progress checker, distill — at once.

### Functions

| Function | Behaviour |
|---|---|
| `strip_ansi(&str) -> Cow` | Single ANSI (CSI/OSC/two-char) stripper. `Cow::Borrowed` on the clean path (no `ESC` byte). |
| `strip_recipe_noise(&str) -> Cow` | `strip_ansi` + drop ISO-8601 tracing lines, runner-banner lines, **and Copilot launch-log preamble lines** (via `is_noise_line`). `Cow::Borrowed` on the clean path. |
| `is_noise_line(&str) -> bool` *(private)* | Per-line predicate: `true` for an ISO-timestamp tracing line, a runner summary-banner line, **or** a Copilot launcher line (`is_copilot_launcher_line`). A JSON payload line beginning (after `trim_start`) with a structural token (`{`, `"`, `[`), an action keyword, a bare decimal, or a verdict keyword never matches, so dropping such a line never discards the answer. |
| `is_copilot_launcher_line(&str) -> bool` *(private)* | The launcher-only arm (#2496). Anchored `starts_with`/`contains` matches on the four launcher shapes below; matches **no** payload line. ANSI is stripped before it runs. |
| `balanced_objects` / `last_balanced_object` / `extract_json_payload` | String-literal-aware balanced `{…}` scan. JSON extraction is **dual-pass** (line-dropped **and** ANSI-only) so the payload survives both an interleaved log line inside a pretty body and a same-line log prefix. |
| `extract_verdict(raw, keywords)` | Precedence keyword scan over cleaned text. |
| `record_parse_outcome(phase, success)` | Emits `recipe_parse_{success,failure}_total{phase}` to `metrics.jsonl`. |

### Copilot launcher-line shapes (`is_copilot_launcher_line`)

The predicate drops a line (after `trim_start`) when it matches one of these
anchored launcher shapes and **only** these — it is deliberately conservative so
no decision token, JSON payload, decimal, or verdict keyword is ever eaten:

| Shape (anchored) | Example line |
|---|---|
| `ℹ`/info-marker line containing `NODE_OPTIONS=` and `(saved preference)` | `ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: …` |
| starts with `Run 'copilot update'` | `Run 'copilot update' to check for updates.` |
| contains `launching copilot binary=` / `version="GitHub Copilot CLI` | `… INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot version="GitHub Copilot CLI 1.0.66-2."` |
| leading `INFO`/`WARN` launcher line with **no** ISO-8601 timestamp | `INFO using cached login`, `WARN extension not pinned` |

A line that begins (after `trim_start`) with a JSON **structural token** — `{`,
`"`, or `[` — a known action keyword, a bare decimal, or a verdict keyword is
**never** classified as a launcher line. The structural-token guard is explicit
([#2570](https://github.com/rysweet/Simard/issues/2570)) so the `contains`-based
`launching copilot binary=` / `version="GitHub Copilot CLI` arms above cannot
drop a pretty-printed fact `"content"` line that quotes one of those substrings.
ANSI escapes are stripped first, so an ANSI-coloured `INFO`/`WARN` launcher line
still matches and a coloured payload line still survives. See
[Concept: Copilot launch-log preamble stripping § correctness as safety](../concepts/copilot-launcher-preamble-stripping.md#correctness-as-safety-never-eat-the-payload).

#### Example: launcher preamble + ANSI around a decide decision

Raw `step_results[].output` (ANSI shown as `\x1b[…m`, leading launcher preamble):

```
\x1b[2mℹ\x1b[0m NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: /home/azureuser/.amplihack/config
\x1b[34mINFO\x1b[0m launching copilot binary=/home/azureuser/.npm-global/bin/copilot version="GitHub Copilot CLI 1.0.66-2."
Run 'copilot update' to check for updates.
advance_goal The next PR is ready to open; proceeding with the implementation.
```

After `strip_recipe_noise`, the cleaned text is just the agent answer, so the
first-word parser reads `advance_goal` (not `ℹ`):

```
advance_goal The next PR is ready to open; proceeding with the implementation.
```

The same cleaning makes the orient parser read the model's real urgency decimal
rather than scraping `1.0.66-2` → `1.0` from the version string.

### Clean-path guarantee

`strip_ansi` and `strip_recipe_noise` return `Cow::Borrowed` — byte-identical
text with zero allocation — when stdout carries no `ESC` byte and no droppable
log/banner line. Adopting the shared helper therefore does **not** change any
phase's behaviour on clean output; only previously-defaulted noisy cases now
recover.

### Observability counters

On every recipe-backed phase invocation, `record_parse_outcome(phase, success)`
is emitted at the subprocess call site (never inside a pure parse function, so
unit tests write no metrics):

```
recipe_parse_success_total{phase}
recipe_parse_failure_total{phase}   # incremented when the permissive default fires
```

`phase ∈ {distill, merge_judge, engineer_lifecycle, decide, orient,
progress_checker}`. These are **complementary** to the brain phases' existing
`brain_verdict_parsed_total{phase,outcome}` counter (issue #2429): the latter
owns the brain-phase success-rate dashboard; these add the memory/distill and
progress-checker phases to the same counter family and give both numerator and
denominator per phase.

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

> **Noise pre-stripping (#2484, extended for the Copilot launcher preamble in
> [#2496](https://github.com/rysweet/Simard/issues/2496)):** each parser first
> routes its input through
> [`recipe_output::strip_recipe_noise`](#protocol-0-shared-noise-pre-stripping-recipe_output)
> so an ANSI-coloured log prefix, a runner banner, **or a Copilot launch-log
> preamble line** cannot shadow the first-word / first-float token. Without the
> launcher arm the first token was `ℹ`/`Run`/`1.0.66-2` and every decide/orient
> parse missed → ladder exhaustion → deterministic default → a stalled goal.
> Clean output is passed through unchanged (`Cow::Borrowed`).

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

> **Transport (fixed in [#2421](https://github.com/rysweet/Simard/issues/2421)):**
> the `text` passed to this parser is the agent output extracted from the
> `recipe-runner-rs --output-format json` envelope (`step_results[].output`),
> **not** the text-mode summary banner. `judge_decision` parses it via
> `parse_action_outcome` (the outcome-classifying variant of this function) and
> runs the escalation ladder on a parse-miss before the loud `AdvanceGoal`
> default. See
> [Recipe-brain verdict/decision parsing](./recipe-brain-verdict-parsing.md).

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

> **Transport (fixed in [#2421](https://github.com/rysweet/Simard/issues/2421)):**
> the `text` is the agent output extracted from the `--output-format json`
> envelope, **not** the text-mode banner. The envelope carries no banner, so the
> first float is the model's real decimal and the deterministic floor below is
> the only fallback — the banner's `(0.0s)` timing string can no longer be
> scraped as `adjusted_urgency`, so urgency `0.0` from a banner can no longer
> happen. `judge_orientation` parses via `parse_orient_outcome` (the
> outcome-classifying variant) and runs the escalation ladder on a parse-miss.
> See [Recipe-brain verdict/decision parsing](./recipe-brain-verdict-parsing.md).

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
(thin wrapper over `parse_lifecycle_outcome`, which also returns a
`LifecycleParseOutcome` for metrics).

Extracts the first whitespace-delimited word, lowercases it, and matches
against the 6 lifecycle variant names. Defaults to `ContinueSkipping`.

> **Fixed in [#2419](https://github.com/rysweet/Simard/issues/2419):** The
> `text` passed to this parser is the agent decision text extracted from the
> `recipe-runner-rs --output-format json` envelope
> (`step_results[].output`), **not** raw stdout. recipe-runner-rs's default
> `text` mode prints only a summary banner to stdout, so reading raw stdout
> made first-word extraction always see `Recipe:` and silently default
> (~99.6% of calls). Each call now also emits a `brain_lifecycle_decision`
> metric whose `outcome` label measures the parse-failure rate.

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

0. **Pre-strip noise (#2484):** route stdout through
   [`recipe_output::strip_recipe_noise`](#protocol-0-shared-noise-pre-stripping-recipe_output)
   first, so a dropped tracing-log line's keyword substring (e.g. `already`
   containing `ready`) cannot fabricate a verdict and a real verdict behind an
   ANSI/log prefix is not silently missed. Clean output is unchanged.
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

> **Transport (fixed in [#2428](https://github.com/rysweet/Simard/issues/2428) /
> [#2430](https://github.com/rysweet/Simard/issues/2430) /
> [#2435](https://github.com/rysweet/Simard/issues/2435) /
> [#2462](https://github.com/rysweet/Simard/issues/2462) /
> [#2463](https://github.com/rysweet/Simard/issues/2463)):**
> `RecipeMergeJudge::judge` invokes `recipe-runner-rs` with `--output-format
> json` and parses the agent verdict text extracted from the envelope
> (`step_results[].output`), **not** the text-mode summary banner. It parses via
> `parse_merge_outcome`, runs the escalation ladder on a parse-miss, and **fails
> closed to `Unclear`** when no verdict parses. See
> [Recipe-brain verdict/decision parsing](./recipe-brain-verdict-parsing.md#merge-judge-phase-2462)
> and the [operator runbook](../howto/diagnose-merge-pr-verdict-parse-failures.md).

**Parser:** `parse_merge_outcome(text) -> (JudgeOutcome, LifecycleParseOutcome)`,
which composes two parsers over the envelope-extracted agent output:

1. `merge_judge::parse_judge_response` — structured `{"verdict":…}` JSON
   (fenced ` ```json ` block → first brace-balanced `{…}` that parses →
   outermost `{…}`), which **does** populate structured `Blocker` entries.
   Tried **first**.
2. `parse_merge_verdict_from_text` — the case-insensitive substring keyword scan
   below, used as a **prose fallback** when no structured JSON is present.

**Keyword scan (`parse_merge_verdict_from_text`)** — the prose fallback:

| Keyword (substring) | Maps to | Priority |
|---------------------|---------|----------|
| `not_ready` / `not ready` | `Verdict::NotReady` | Checked first (prevents `ready` substring match) |
| `unclear` | `Verdict::Unclear` | Checked second (treated as not-ready at the call site) |
| `ready` | `Verdict::Ready` | Checked third |

**On no verdict from either parser, `parse_merge_outcome` fails CLOSED to
`Verdict::Unclear`** (classified as a parse-miss in the returned
`LifecycleParseOutcome`). The merge authority treats `Unclear` as a refusal, so
an unparseable verdict can never become `Ready`. Genuine infrastructure failures
— spawn failure, nonzero exit, or an undecodable JSON envelope — still propagate
from `RecipeMergeJudge::judge` as `SimardError::AdapterInvocationFailed`; they
are distinct from a successful run whose verdict merely fails to parse (which
drives the ladder, then the fail-closed `Unclear`).

**Note on blockers:** the structured `parse_judge_response` path populates
`JudgeOutcome.blockers`; the keyword-scan prose fallback does not (`blockers`
empty, rationale = truncated raw text).

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
| `recipe_output/extract.rs` | 25+ | `strip_ansi` (CSI/OSC/two-char, clean-path borrow, JSON-escaped literal), `strip_recipe_noise` (drops tracing/banner lines, keeps prose), balanced-object scan (string-literal aware), `extract_json_payload` dual-pass recovery (ANSI+log, same-line prefix, interleaved log line), `extract_verdict` precedence + dropped-log-line substring safety |
| `decide.rs` | 4+ | DeterministicFallback tests |
| `recipe_brain.rs` | 30+ | All 10 action keywords (first-word), first-float orient, 6 lifecycle variants (first-word), case-insensitive match, unrecognized defaults, #2484 banner→DefaultMalformed / pure-noise→DefaultEmpty |
| `orient.rs` | 8+ | Float parsing, validation, extra fields, empty/invalid responses |
| `rustyclawd.rs` | 15+ (T1–T15) | Full behavior matrix per decision protocol reference |
| `recipe_progress_checker.rs` | 6+ | Accept, reject, no keyword (default), mixed case, #2484 noise-obscured reject recovery, `parse_verdict_outcome` match flag |
| `recipe_merge_judge.rs` | 8+ | `parse_merge_outcome`: structured `parse_judge_response` JSON (populates blockers), prose keyword scan (ready / not_ready / unclear; substring safety), JSON-envelope extraction, fail-closed `Verdict::Unclear` on an unparseable verdict, and #2484 noise-stripping (dropped-log-line `ready` substring safety, verdict recovery past an ANSI log prefix). |
| `memory_consolidation/distillation.rs` | 17+ | Plain object, prose-embedded object, last-non-empty preference, unmatched-quote tolerance, ANSI-log recovery, #2484 runner-banner recovery |
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
