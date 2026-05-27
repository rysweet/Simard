# Reference: OODA Brain Decision Protocol (first-word extraction)

Crate: `simard` · Module: `simard::ooda_brain::recipe_brain`
Closes the design gap that Issue [#1711](https://github.com/rysweet/Simard/issues/1711) opened.

This page is the normative definition of the **wire format** the OODA brain
accepts from recipe output when emitting an `EngineerLifecycleDecision`. It
replaces the `DECISION:` marker protocol that was introduced in #1711 and
simplified further in #2144.

> **TL;DR** — The recipe prompt instructs the LLM to output the variant name
> as the very first word. The parser takes the first non-whitespace token,
> matches it case-insensitively against the 6 variant names, and uses defaults
> for all extra fields. No markers, no labeled lines, no JSON, no keyword
> scanning. Unrecognized first words default to `ContinueSkipping`.

## Why the simplified protocol

Before #2144, `parse_lifecycle_from_text` had a 2-tier parse cascade:

1. `DECISION:` marker on first non-blank line → extract variant + labeled fields
2. Keyword scan for variant names anywhere in text via `ascii_contains_ignore_case`

Both tiers were over-engineered:

- The marker protocol required a specific `DECISION: <variant>` format that
  models frequently ignored.
- The keyword scan searched the *entire* text for variant names, which could
  match false positives in log excerpts and rationale prose.
- Labeled-line extraction (`TITLE:`, `BODY:`, `REASON:`, `REDISPATCH_CONTEXT:`)
  added complexity for fields that are primarily used for logging/debugging.

The recipe prompts already tell the LLM to output a specific action word.
The first word of the output IS the decision.

## The wire format

A response is **valid** if:

1. **First-word form** — the first non-whitespace token (extracted via
   `split_whitespace().next()`) matches case-insensitively against the
   `EngineerLifecycleDecision` variant whitelist.

If no match is found, the parser returns `ContinueSkipping` with the full
text as rationale. This is a safe default — it means "do nothing this cycle."

### Variant whitelist

| Variant token            | Extra fields (defaults)                                     |
|--------------------------|-------------------------------------------------------------|
| `continue_skipping`      | _(none)_                                                    |
| `reclaim_and_redispatch` | `redispatch_context: ""`                                    |
| `deprioritize`           | _(none)_                                                    |
| `open_tracking_issue`    | `title: "OODA stuck"`, `body: truncate(rest, 500)`         |
| `mark_goal_blocked`      | `reason: truncate(rest, 500)`                               |
| `consider_self_update`   | _(none)_                                                    |

`rationale` is `truncate(remaining_text_after_first_word, 500)` for all
variants. If the response is just the variant name with no following text,
rationale defaults to `"<no rationale provided>"`.

### Grammar

```
<response>      ::= <ws>* <variant-token> (<ws>+ <rationale>)?
<variant-token> ::= "continue_skipping"
                  | "reclaim_and_redispatch"
                  | "deprioritize"
                  | "open_tracking_issue"
                  | "mark_goal_blocked"
                  | "consider_self_update"
<rationale>     ::= <any text to end of input>
```

The variant token is matched case-insensitively via `.eq_ignore_ascii_case()`.

## Behavior matrix

The following table is the canonical specification. Every row is exercised
by a test in `src/ooda_brain/recipe_brain.rs`.

| # | Input shape (illustrative)                                                | Result                                                                                  |
|---|---------------------------------------------------------------------------|-----------------------------------------------------------------------------------------|
| 1 | `continue_skipping`                                                       | `ContinueSkipping { rationale: "<no rationale provided>" }`                             |
| 2 | `continue_skipping engineer made progress 12s ago`                        | `ContinueSkipping { rationale: "engineer made progress 12s ago" }`                      |
| 3 | `Continue_Skipping mixed case`                                            | `ContinueSkipping { ... }` — case-insensitive match                                     |
| 4 | `open_tracking_issue Engineer stuck in compile-error loop`                | `OpenTrackingIssue { title: "OODA stuck", body: "Engineer stuck...", rationale: "..." }` |
| 5 | `mark_goal_blocked compile-error-loop persistent failure`                 | `MarkGoalBlocked { reason: "compile-error-loop persistent failure", rationale: "..." }`  |
| 6 | `reclaim_and_redispatch Try a different approach`                         | `ReclaimAndRedispatch { redispatch_context: "", rationale: "Try a different approach" }` |
| 7 | `bogus_word some rationale`                                               | `ContinueSkipping { rationale: "bogus_word some rationale" }` — default                 |
| 8 | `` (empty)                                                                | `ContinueSkipping { rationale: "<no rationale provided>" }` — default                   |
| 9 | `  deprioritize  with leading whitespace`                                 | `Deprioritize { rationale: "with leading whitespace" }`                                 |
| 10| `consider_self_update`                                                    | `ConsiderSelfUpdate { rationale: "<no rationale provided>" }`                           |

## What changed

> **Removed in [#2144](https://github.com/rysweet/Simard/issues/2144):**
> - `DECISION:` marker parsing (`extract_decision_marker`, `parse_with_marker`)
> - Labeled-line field extraction (`TITLE:`, `BODY:`, `REASON:`, `REDISPATCH_CONTEXT:`, `RATIONALE:`)
> - `LIFECYCLE_KEYWORDS` constant and `try_keyword_scan` / `build_keyword_decision`
> - `ascii_contains_ignore_case` byte-level scanner
> - `serde_json::from_value` construction from parsed fields

> **Removed in [#1980](https://github.com/rysweet/Simard/issues/1980):**
> - JSON extraction path (`find('{')..rfind('}')`)
> - Hybrid form (marker + JSON body)

## What did **not** change

* The `OodaBrain` trait, `EngineerLifecycleCtx`, and
  `EngineerLifecycleDecision` types are byte-identical to their pre-#1711
  shapes. No caller changes are required.
* `DeterministicFallbackBrain` still returns `ContinueSkipping` and is still
  used when `RecipeBrain::new()` fails to construct.
* The fallback to `ContinueSkipping` in `dispatch_spawn_engineer` on a
  parser default is preserved — the parser now returns a default instead of
  an error, so the safety net is exercised less frequently.
* `truncate()` caps all text fields at 500 chars.
* `OrientJudgment::validate()` enforces bounds after orient extraction.

## Examples

### Minimal: `continue_skipping`

Recipe output:

```
continue_skipping engineer touched worktree 8 seconds ago; let it cook
```

Parsed as:

```rust
EngineerLifecycleDecision::ContinueSkipping {
    rationale: "engineer touched worktree 8 seconds ago; let it cook".into(),
}
```

### Structured: `open_tracking_issue`

Recipe output:

```
open_tracking_issue Engineer panics on goal improve-amplihack-test-coverage. Repro: spawn engineer, wait 30s, observe panic. Log shows thread 'main' panicked. Recurred across 3 spawns.
```

Parsed as:

```rust
EngineerLifecycleDecision::OpenTrackingIssue {
    rationale: "Engineer panics on goal improve-amplihack-test-coverage. ...".into(),
    title:     "OODA stuck".into(),
    body:      "Engineer panics on goal improve-amplihack-test-coverage. ...".into(),
}
```

### Structured: `reclaim_and_redispatch`

Recipe output:

```
reclaim_and_redispatch Previous engineer attempted X; please retry with Y. Worktree idle 7h, no log activity.
```

Parsed as:

```rust
EngineerLifecycleDecision::ReclaimAndRedispatch {
    rationale:          "Previous engineer attempted X; please retry with Y. ...".into(),
    redispatch_context: "".into(),
}
```

Note: `redispatch_context` defaults to empty string. The LLM's prose is
captured entirely in `rationale`.

## Security

The simplified protocol retains security hardening:

| ID    | Threat                                                  | Mitigation                                                                                |
|-------|---------------------------------------------------------|-------------------------------------------------------------------------------------------|
| SR-1  | ~~Mid-response `DECISION:` token injection~~            | Eliminated — no marker parsing. Only the first word matters.                              |
| SR-2  | ~~Variant smuggling via diverging marker / JSON~~       | Eliminated — single source of truth (first word).                                         |
| SR-3  | UTF-8 boundary panic on malformed model output          | `split_whitespace()` and `str` methods are UTF-8 safe — no raw byte indexing.             |
| SR-4  | Log-flood DoS from a runaway 1 GB model response        | `truncate()` caps rationale at 500 chars. `truncate_for_log` caps raw text at 8192 bytes. |
| SR-5  | Log injection via CRLF / ANSI escapes in raw response   | All `raw` fields rendered via the `{:?}` Debug format, which escapes control chars.       |
| SR-6  | Variant whitelist drift between parser and enum         | Match arms are inline in the parse function — single source of truth.                     |

## Compatibility

| Caller                                          | Affected?                                                     |
|-------------------------------------------------|---------------------------------------------------------------|
| `RecipeBrain::decide_engineer_lifecycle`        | Yes — uses first-word parser (DECISION marker removed).       |
| `DeterministicFallbackBrain`                    | No — bypasses the parser entirely.                            |
| Any test using DECISION marker format           | **Yes** — must be updated to first-word format.               |
| Any test using JSON format                      | **Yes** — already removed in #1980.                           |
| `ooda-engineer-lifecycle.yaml` prompt           | Updated to instruct first-word output format.                 |

## See Also

* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md)
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md)
* [Reference: `OodaBrain` API](ooda-brain-api.md)
* [Reference: `ooda_brain.md` Prompt Schema](ooda-brain-prompt.md)
* [How-to: diagnose brain decision parse failures](../howto/diagnose-brain-decision-parse-failures.md)
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
* [Issue #1711 — fall back to prose-first decision protocol](https://github.com/rysweet/Simard/issues/1711)
* [Issue #1980 — root out JSON-parsing LLM output anti-pattern](https://github.com/rysweet/Simard/issues/1980)
* [Issue #2144 — eliminate all parsers from recipe_brain.rs](https://github.com/rysweet/Simard/issues/2144)
