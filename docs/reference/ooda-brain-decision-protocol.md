# Reference: OODA Brain Decision Protocol (first-word match)

Crate: `simard` · Module: `simard::ooda_brain::rustyclawd`
Closes the design gap that Issue [#1711](https://github.com/rysweet/Simard/issues/1711) opened.

This page is the normative definition of the **wire format** the OODA brain
accepts from an LLM when emitting an `EngineerLifecycleDecision`.

> **Changed in #2144:** The lifecycle brain no longer parses a `DECISION:`
> marker, labeled lines, or JSON-shaped fallback bodies. It now lowercases the
> **first whitespace-delimited token** and matches that token directly against
> the `EngineerLifecycleDecision` whitelist. The rest of the response is kept
> as rationale text; structured fields now use defaults.

## Why this protocol changed

The old lifecycle parser had multiple text-parsing layers:

- `DECISION:` marker detection on the first line
- labeled-field extraction (`TITLE:`, `BODY:`, `REASON:`,
  `REDISPATCH_CONTEXT:`)
- a HashMap builder and `serde_json::from_value` conversion step

That stack was more complex than the actual routing requirement. The brain
only needs a lifecycle variant plus human-readable rationale. Issue #2144
reduced the contract to one rule: **put the lifecycle variant first**.

## The wire format

A response is **valid** if its first whitespace-delimited token matches one of
these lifecycle variants after `to_ascii_lowercase()`:

- `continue_skipping`
- `reclaim_and_redispatch`
- `deprioritize`
- `open_tracking_issue`
- `mark_goal_blocked`
- `consider_self_update`

The parser shape is:

```rust
let first_word = text.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
match first_word.as_str() {
    "continue_skipping" => ...,
    "reclaim_and_redispatch" => ...,
    "deprioritize" => ...,
    "open_tracking_issue" => ...,
    "mark_goal_blocked" => ...,
    "consider_self_update" => ...,
    _ => Err(SimardError::BrainResponseUnparseable { .. }),
}
```

### Result construction

Once the first word matches a variant:

- `rationale` = full response text when present, otherwise `"<no rationale provided>"`
- `title` = `""`
- `body` = `""`
- `reason` = `""`
- `redispatch_context` = `""`
- `truncate()` still caps stored text fields

> **Removed in #2144:** labeled-line field extraction and the
> `serde_json::from_value` conversion step.

### Grammar

```
<response>      ::= <ws>* <variant-token> (<ws> <free-text>)?
<variant-token> ::= "continue_skipping"
                  | "reclaim_and_redispatch"
                  | "deprioritize"
                  | "open_tracking_issue"
                  | "mark_goal_blocked"
                  | "consider_self_update"
<free-text>     ::= <any remaining text>
```

The match is case-insensitive because the parser lowercases the first token
before the `match`.

## Behavior matrix

The following table is the canonical specification.

| # | Input shape (illustrative) | Result |
|---|---|---|
| T1 | `continue_skipping` | `Ok(ContinueSkipping { rationale: "continue_skipping" })` |
| T2 | `continue_skipping rest of text` | `Ok(ContinueSkipping { rationale: "continue_skipping rest of text" })` |
| T3 | `CONTINUE_SKIPPING` | `Ok(ContinueSkipping { ... })` — case-insensitive first-word match |
| T5 | `open_tracking_issue rest` | `Ok(OpenTrackingIssue { title: "", body: "", rationale: "open_tracking_issue rest" })` |
| T10 | `OK` | `Err(BrainResponseUnparseable)` |
| T11 | `` (empty) | `Err(BrainResponseUnparseable)` |
| T12 | `bogus_variant` | `Err(BrainResponseUnparseable)` |
| T14 | `reclaim_and_redispatch rest` | `Ok(ReclaimAndRedispatch { redispatch_context: "", rationale: "reclaim_and_redispatch rest" })` |
| T15 | `🚀continue_skipping` | `Err(BrainResponseUnparseable)` — first word does not match a whitelisted variant |

> **Removed in #2144:**
> - T4 (`DECISION: CONTINUE_SKIPPING` exact-snake-case marker parsing)
> - T6–T9 (JSON and marker-body examples)
> - T13 (mid-response `DECISION:` injection)

## Error format

`SimardError::BrainResponseUnparseable` still carries the full raw response so
operators can diagnose bad model output.

```rust
SimardError::BrainResponseUnparseable {
    raw: String,
    source: BrainParseSource,
}
```

Under the first-word protocol, parse failures now come from:

- empty responses
- an unrecognized first token
- malformed leading bytes that change the first token

> **Removed in #2144:** marker-not-found errors, labeled-field errors, and
> marker/JSON conflict paths.

### `raw` lifecycle

The `raw` field remains **untruncated** in the error struct. Truncation to
`MAX_RAW_LOG_BYTES = 8192` still happens only at log-format time.

* `raw` is the complete model response.
* `raw` is rendered with `{:?}` in logs, so control characters and ANSI escapes
  are escaped.

A representative parse-failure log now looks like:

```
WARN simard::ooda_brain: brain.decide_engineer_lifecycle parse failed
    goal=improve-amplihack-test-coverage
    raw="OK"
    error=unrecognized lifecycle variant in first token
```

## What did **not** change

* The `OodaBrain` trait, `EngineerLifecycleCtx`, and
  `EngineerLifecycleDecision` types are still the public contract.
* `DeterministicFallbackBrain` still returns `ContinueSkipping` when the brain
  cannot be constructed.
* The fallback path on parse error is still `ContinueSkipping`.
* `truncate()` still protects rationale and other text fields from unbounded
  output.

## Examples

### Minimal: `continue_skipping`

Model output:

```
continue_skipping engineer touched worktree 8 seconds ago; let it cook
```

Parsed as:

```rust
EngineerLifecycleDecision::ContinueSkipping {
    rationale: "continue_skipping engineer touched worktree 8 seconds ago; let it cook".into(),
}
```

### `open_tracking_issue`

Model output:

```
open_tracking_issue engineer panic recurred across 3 spawns
```

Parsed as:

```rust
EngineerLifecycleDecision::OpenTrackingIssue {
    title: "".into(),
    body: "".into(),
    rationale: "open_tracking_issue engineer panic recurred across 3 spawns".into(),
}
```

### `reclaim_and_redispatch`

Model output:

```
reclaim_and_redispatch worktree idle 7h, retry with a fresh engineer
```

Parsed as:

```rust
EngineerLifecycleDecision::ReclaimAndRedispatch {
    redispatch_context: "".into(),
    rationale: "reclaim_and_redispatch worktree idle 7h, retry with a fresh engineer".into(),
}
```

## Security

The first-word protocol is hardened against the classes of misbehavior that now
matter.

| ID    | Threat | Mitigation |
|-------|--------|------------|
| SR-3  | UTF-8 boundary panic on malformed model output | Matching is done on Rust `&str`; unrecognized first tokens return an error instead of panicking. |
| SR-4  | Log-flood DoS from a runaway model response | `truncate_for_log` still caps raw text at `MAX_RAW_LOG_BYTES = 8192` at format time. |
| SR-5  | Log injection via CRLF / ANSI escapes in raw response | `raw` is still rendered via `{:?}`. |
| SR-6  | Variant whitelist drift between parser and enum | The whitelist is still the `EngineerLifecycleDecision` enum vocabulary. |

> **Removed in #2144:**
> - SR-1 (mid-response `DECISION:` injection)
> - SR-2 (marker/JSON conflict)

The parser does **not** sandbox the LLM, validate model identity, or
rate-limit responses — those concerns still live one layer up.

## Compatibility

| Caller | Affected? |
|--------|-----------|
| `RustyClawdBrain::decide_engineer_lifecycle` | Yes — now uses first-word matching instead of marker parsing. |
| `DeterministicFallbackBrain` | No — bypasses the parser entirely. |
| Prompt examples in `ooda_brain.md` | Yes — must start with the lifecycle variant name. |

A model that still emits `DECISION:` lines or labeled fields may fail parsing
if the first token is not itself a valid lifecycle variant.

## See Also

* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md)
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md)
* [Reference: `OodaBrain` API](ooda-brain-api.md)
* [Reference: `ooda_brain.md` Prompt Schema](ooda-brain-prompt.md)
* [How-to: diagnose brain decision parse failures](../howto/diagnose-brain-decision-parse-failures.md)
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
* [Issue #1711 — fall back to prose-first decision protocol](https://github.com/rysweet/Simard/issues/1711)
* [Issue #1980 — root out JSON-parsing LLM output anti-pattern](https://github.com/rysweet/Simard/issues/1980)
