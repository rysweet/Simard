# Reference: OODA Brain Decision Protocol (DECISION marker)

Crate: `simard` · Module: `simard::ooda_brain::rustyclawd`
Closes the design gap that Issue [#1711](https://github.com/rysweet/Simard/issues/1711) opened.

This page is the normative definition of the **wire format** the OODA brain
accepts from an LLM when emitting an `EngineerLifecycleDecision`. It replaces
the legacy "must reply with a single JSON object, no prose, no fences" rule
that previously lived in [`ooda-brain-prompt.md`](ooda-brain-prompt.md).

> **TL;DR** — Models now emit a leading `DECISION: <variant>` marker line.
> Variants that need structured fields (`open_tracking_issue`,
> `mark_goal_blocked`, `reclaim_and_redispatch`) follow the marker with a JSON
> object. Pure-JSON responses (the legacy format) still parse — backward
> compatible. Parse failures embed the **full raw response** in the error so
> operators can diagnose 3-byte `"OK"` responses instead of guessing.

## Why a new protocol

Before this protocol, `parse_decision_from_response` required a strict
JSON-extraction pass. In production this manifested as:

```
WARN simard::ooda_brain: brain.decide_engineer_lifecycle failed; falling back
    to continue_skipping
    goal=improve-amplihack-test-coverage
    error=base type "ooda-brain" failed during invocation:
          no JSON object found in LLM response (got 3 bytes)
```

The 3-byte response was almost certainly a parseable prose token (`"OK"`,
`"continue"`, etc.) the model emitted instead of a JSON document. The strict
parser rejected it, the cycle fell back to `continue_skipping`, and the dead
engineer was never reclaimed. Compounded over hours, the goal stalled
indefinitely.

The new protocol makes the wire format **prose-first** — the same direction
the companion `goal_action` path already took (see journal entries tagged
`LLM emitted prose for`).

## The wire format

A response is **valid** if any one of the following is true:

1. **Prose marker form** — the first non-blank line matches the regex
   `^\s*DECISION\s*:\s*<variant_token>\s*$` (case-insensitive on the literal
   word `DECISION`; `<variant_token>` is matched exact-snake-case against the
   `EngineerLifecycleDecision` whitelist).
2. **Hybrid form** — the prose marker (rule 1) is followed by a JSON object
   on subsequent lines that supplies any variant-specific fields.
3. **Legacy JSON form** — the entire response (after trimming) parses as a
   single JSON object whose `choice` field names a known variant. Code-fence
   wrappers (```` ```json ... ``` ````) and surrounding prose are tolerated,
   matching the pre-#1711 behavior.

If none of the three apply, the parser returns
`SimardError::BrainResponseUnparseable { raw, source }` with the **full raw
response text** embedded.

### Variant whitelist

The whitelist is the `EngineerLifecycleDecision` enum itself — there is no
hand-maintained parallel list. The current 6 variants are:

| Variant token            | Required fields (besides `rationale`)               |
|--------------------------|-----------------------------------------------------|
| `continue_skipping`      | _(none)_                                            |
| `reclaim_and_redispatch` | `redispatch_context: String`                        |
| `deprioritize`           | _(none)_                                            |
| `open_tracking_issue`    | `title: String`, `body: String`                     |
| `mark_goal_blocked`      | `reason: String`                                    |
| `consider_self_update`   | _(none)_                                            |

`rationale` is required by every variant but defaults to a placeholder
(`"<no rationale provided>"`) when the marker form is used without a JSON
follow-up. The handler in
`src/ooda_actions/advance_goal/lifecycle.rs::apply_lifecycle_decision` does
not differentiate between a model-supplied rationale and the placeholder.

### Marker grammar

```
<response>      ::= <marker-line> ("\n" <body>)? | <legacy-json>
<marker-line>   ::= <ws>* "DECISION" <ws>* ":" <ws>* <variant-token> <ws>*
<variant-token> ::= "continue_skipping"
                  | "reclaim_and_redispatch"
                  | "deprioritize"
                  | "open_tracking_issue"
                  | "mark_goal_blocked"
                  | "consider_self_update"
<body>          ::= <free-prose>? (<json-object> <free-prose>?)?
```

The marker is matched **only on the first non-blank line of the response**.
A `DECISION:` token that appears mid-response or inside JSON is ignored.
This is a deliberate hardening choice (see [Security](#security) below).

### Marker-wins precedence

If both a prose marker **and** a JSON `choice` field are present and they
disagree (`DECISION: continue_skipping` followed by
`{"choice": "deprioritize", ...}`), the **marker wins**. The JSON object is
then re-parsed with its `choice` field overwritten so that field harvesting
still succeeds.

This rule exists so that a downstream prompt rewrite that drops the JSON
discriminator never silently changes the dispatch path; the marker is the
authoritative signal.

## Behavior matrix

The following table is the canonical specification. Every row is exercised
by a test in `src/ooda_brain/tests.rs`.

| # | Input shape (illustrative)                                                | Result                                                                                  |
|---|---------------------------------------------------------------------------|-----------------------------------------------------------------------------------------|
| 1 | `DECISION: continue_skipping`                                             | `Ok(ContinueSkipping { rationale: "<no rationale provided>" })`                         |
| 2 | `DECISION: continue_skipping\nengineer made progress 12s ago`             | `Ok(ContinueSkipping { rationale: "engineer made progress 12s ago" })`                  |
| 3 | `decision: CONTINUE_SKIPPING` (case variations on `DECISION`)             | `Ok(ContinueSkipping { ... })` — case-insensitive on the keyword                        |
| 4 | `DECISION: CONTINUE_SKIPPING` (uppercase variant)                         | `Err(BrainResponseUnparseable)` — variant matched exact-snake-case only                 |
| 5 | `DECISION: open_tracking_issue\n{"title":"X","body":"Y","rationale":"Z"}` | `Ok(OpenTrackingIssue { title: "X", body: "Y", rationale: "Z" })`                       |
| 6 | `DECISION: open_tracking_issue\nrationale only, no JSON`                  | `Err(BrainResponseUnparseable)` — required `title`/`body` missing                       |
| 7 | ` ```json\n{"choice":"continue_skipping","rationale":"x"}\n``` `          | `Ok(ContinueSkipping { rationale: "x" })` — code fences stripped                        |
| 8 | `Some prose\n{"choice":"reclaim_and_redispatch",...}\nMore prose`         | `Ok(ReclaimAndRedispatch { ... })` — JSON object extraction (legacy path)               |
| 9 | `{"choice":"continue_skipping","rationale":"x"}` (pure JSON)              | `Ok(ContinueSkipping { rationale: "x" })` — legacy path                                 |
| 10| `OK` (3-byte non-JSON, no marker) — **the #1711 bug**                     | `Err(BrainResponseUnparseable { raw: "OK", source })` — full text in error              |
| 11| `` (empty)                                                                | `Err(BrainResponseUnparseable { raw: "", source })` — note "empty response"             |
| 12| `DECISION: bogus_variant`                                                 | `Err(BrainResponseUnparseable)` — error lists all 6 valid tokens                        |
| 13| `random prose ... DECISION: deprioritize ... more prose`                  | `Err(BrainResponseUnparseable)` — marker not on first non-blank line, ignored           |
| 14| `DECISION: continue_skipping` (with `{"choice":"deprioritize",...}` body) | `Ok(ContinueSkipping { ... })` — marker wins; JSON `choice` overwritten                 |
| 15| `DECISION: 🚀continue_skipping` (multibyte garbage prefix on token)       | `Err(BrainResponseUnparseable)` — UTF-8-safe slicing, no panic                          |

## Error format

> **New in [#1711](https://github.com/rysweet/Simard/issues/1711).**
> `BrainResponseUnparseable` is a new `SimardError` variant introduced by
> this PR. Cross-check with
> [Reference: `OodaBrain` API → Errors](ooda-brain-api.md#errors).

```rust
SimardError::BrainResponseUnparseable {
    raw: String,            // full untruncated response (see "raw lifecycle" below)
    source: BrainParseSource,
}

/// Single wrapper so one error variant can carry either failure mode.
pub enum BrainParseSource {
    Json(serde_json::Error),
    Marker(MarkerParseError),
}
```

### `raw` lifecycle

The `raw` field is stored **untruncated** in the struct so that
`{:#?}` debug printing and any downstream tooling has access to the full
text the model returned. Truncation to `MAX_RAW_LOG_BYTES = 8192` is
applied **only at log-format time**, via the shared `truncate_for_log`
helper at `src/util/log.rs::truncate_for_log` (hoisted from
`src/ooda_actions/advance_goal/spawn.rs:318` as part of this PR — see
[`OodaBrain` API → `truncate_for_log` reuse](ooda-brain-api.md#truncate_for_log-reuse)).
The truncated rendition is suffixed with `…(truncated, total {n} bytes)`.

* `raw` is the **complete** model response. Previously the warn-level log
  reported only `got N bytes`; this is fixed at all three lossy parser sites
  (`rustyclawd.rs`, `decide.rs`, `orient.rs`).
* `raw` is rendered using the `{:?}` Debug format wherever it appears in
  log lines, so control characters and ANSI escapes are escaped (defends
  against CRLF / log-injection in the model output).

The companion log line at the call site looks like:

```
WARN simard::ooda_brain: brain.decide_engineer_lifecycle parse failed
    goal=improve-amplihack-test-coverage
    raw="OK"
    error=no DECISION: marker found and response is not valid JSON
```

Compare with the legacy log:

```
WARN simard::ooda_brain: brain.decide_engineer_lifecycle failed; falling
    back to continue_skipping
    goal=improve-amplihack-test-coverage
    error=base type "ooda-brain" failed during invocation:
          no JSON object found in LLM response (got 3 bytes)
```

## What did **not** change

* The `OodaBrain` trait, `EngineerLifecycleCtx`, and
  `EngineerLifecycleDecision` types are byte-identical to their pre-#1711
  shapes. No caller changes are required.
* `DeterministicFallbackBrain` still returns `ContinueSkipping` and is still
  used when `build_rustyclawd_brain()` fails to construct.
* The fallback to `ContinueSkipping` in `dispatch_spawn_engineer` on a
  parser error is preserved — the parser is now strictly more lenient, so
  fewer cycles take that fallback, but the safety net itself is unchanged.
* `decide.rs` and `orient.rs` were **not** migrated to prose-first parsing.
  They received only the one-line raw-response-in-error fix because they
  share the same logging anti-pattern. Migrating their parsers is tracked
  separately and is explicitly out of scope for #1711.

## Examples

### Minimal: `continue_skipping`

Model output:

```
DECISION: continue_skipping
engineer touched worktree 8 seconds ago; let it cook
```

Parsed as:

```rust
EngineerLifecycleDecision::ContinueSkipping {
    rationale: "engineer touched worktree 8 seconds ago; let it cook".into(),
}
```

### Hybrid: `open_tracking_issue`

Model output:

````
DECISION: open_tracking_issue
{
  "rationale": "engineer panic recurred across 3 spawns",
  "title": "Engineer panics on goal improve-amplihack-test-coverage",
  "body": "Repro: spawn engineer, wait 30s, observe panic in tail.\nLog tail:\n```\nthread 'main' panicked at ...\n```"
}
````

Parsed as:

```rust
EngineerLifecycleDecision::OpenTrackingIssue {
    rationale: "engineer panic recurred across 3 spawns".into(),
    title:     "Engineer panics on goal improve-amplihack-test-coverage".into(),
    body:      "Repro: spawn engineer, ...".into(),
}
```

### Legacy JSON (still works)

Model output:

```json
{
  "choice": "reclaim_and_redispatch",
  "rationale": "worktree idle 7h, no log activity",
  "redispatch_context": "Previous engineer attempted X; please retry with Y."
}
```

Parsed as:

```rust
EngineerLifecycleDecision::ReclaimAndRedispatch {
    rationale:          "worktree idle 7h, no log activity".into(),
    redispatch_context: "Previous engineer attempted X; please retry with Y.".into(),
}
```

### Marker-wins on conflict

Model output:

```
DECISION: continue_skipping
{"choice": "deprioritize", "rationale": "ignore me"}
```

Parsed as `ContinueSkipping { rationale: "ignore me" }`. The marker took
precedence; the JSON `rationale` field was harvested; the JSON `choice` was
overwritten by the marker.

## Security

The protocol is hardened against six classes of model misbehavior. All
six are exercised by tests in `src/ooda_brain/tests.rs` and discussed in
detail under Security Considerations in the PR.

| ID    | Threat                                                  | Mitigation                                                                                |
|-------|---------------------------------------------------------|-------------------------------------------------------------------------------------------|
| SR-1  | Mid-response `DECISION:` token injection                | Marker is matched **only on the first non-blank line** (test T13).                        |
| SR-2  | Variant smuggling via diverging marker / JSON `choice`  | Marker wins on conflict; JSON `choice` is overwritten before harvest (test T14).          |
| SR-3  | UTF-8 boundary panic on malformed model output          | All slicing uses `char_indices()` / `str::get()`; no raw byte indexing (test T15).        |
| SR-4  | Log-flood DoS from a runaway 1 GB model response        | `truncate_for_log` caps raw text at `MAX_RAW_LOG_BYTES = 8192` at format time.            |
| SR-5  | Log injection via CRLF / ANSI escapes in raw response   | All `raw` fields rendered via the `{:?}` Debug format, which escapes control chars.       |
| SR-6  | Variant whitelist drift between parser and enum         | Whitelist **is** the `EngineerLifecycleDecision` enum; single source of truth (test T12). |

The parser does **not** sandbox the LLM, validate model identity, or
rate-limit responses — those concerns live one layer up at the
`LlmSubmitter` boundary.

## Compatibility

| Caller                                          | Affected?                                                     |
|-------------------------------------------------|---------------------------------------------------------------|
| `RustyClawdBrain::decide_engineer_lifecycle`    | Yes — uses the new parser.                                    |
| `DeterministicFallbackBrain`                    | No — bypasses the parser entirely.                            |
| Any test using `StubSubmitter` with pure-JSON   | No — legacy path is preserved.                                |
| `decide::parse_judgment_from_response`          | Error format only (raw text now embedded); shape unchanged.   |
| `orient::parse_judgment_from_response`          | Error format only (raw text now embedded); shape unchanged.   |
| `ooda_brain.md` prompt                          | Updated to recommend the marker form; legacy JSON documented. |

A model that is still emitting pure JSON because its prompt has not been
re-deployed will continue to work without change.

## See Also

* [Reference: `OodaBrain` API](ooda-brain-api.md)
* [Reference: `ooda_brain.md` Prompt Schema](ooda-brain-prompt.md)
* [How-to: diagnose brain decision parse failures](../howto/diagnose-brain-decision-parse-failures.md)
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
* [Issue #1711 — fall back to prose-first decision protocol](https://github.com/rysweet/Simard/issues/1711)
