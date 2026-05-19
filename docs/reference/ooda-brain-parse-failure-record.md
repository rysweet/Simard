---
title: OODA Brain Parse-Failure Record
description: Normative schema and visibility contract for decide/orient brain JSON-parse failures (#1890). Companion to the engineer-lifecycle protocol (#1711) and the broader fail-open audit (#1245).
last_updated: 2026-05-19
review_schedule: as-needed
owner: simard
---

# Reference: OODA Brain Parse-Failure Record

Crate: `simard` · Module: `simard::ooda_brain::parse_failure`
Closes the visibility gap that Issue [#1890](https://github.com/rysweet/Simard/issues/1890) opened. Sibling of [#1711](https://github.com/rysweet/Simard/issues/1711) (engineer-lifecycle decision protocol) and [#1748](https://github.com/rysweet/Simard/issues/1748) (silent deterministic fallback audit).

This page is the normative definition of how `decide_with_brain` and `orient_with_brain` in `simard::ooda_loop` surface a brain-invocation failure (the dominant case is a JSON-parse failure; other adapter `Err` variants are covered identically — see [Scope of the record](#scope-of-the-record)). Before this contract, a brain failure produced a single `WARN` line of the form `no JSON object found in LLM response (got N bytes)` and was then silently substituted by `DeterministicFallbackDecideBrain` / `DeterministicFallbackOrientBrain`. The cycle still ran, the cycle report still claimed a decision, and the operator had no way to tell a healthy cycle from a degraded one.

> **TL;DR** — Decide and orient brain failures fire four visibility channels in a fixed sequence: (1) a single `ERROR`-level structured `tracing` event, (2) a `brain_parse_failure` metric increment, (3) a new `parse_failure` block on the corresponding `BrainJudgmentRecord` (which lands in `cycle_reports/cycle_*.json`), and (4) a throttled `gh issue create` escalation at ≥3 consecutive failures per `(phase, goal_id)`. The cycle does **not** abort — it continues with a deterministic substitution whose record now carries a `parse_failure` block, so the cycle report makes the degraded state machine-readable.

## Scope of the record

The `_with_brain` call site for decide currently has the shape:

```rust
match brain.judge_decision(&ctx) {
    Ok(j)  => …,
    Err(_) => fallback.judge_decision(&ctx)?,
}
```

and orient has:

```rust
match brain.judge_orientation(&ctx) {
    Ok(j) if j.validate(ctx.base_urgency).is_ok() => (j, false),
    _ => (DeterministicFallbackOrientBrain::compute(&ctx), true),
}
```

This PR fires the four-channel record for **every `Err(_)` return from the LLM brain** at those call sites. That includes:

* JSON-parse failures from `RustyClawd*Brain::run` (the dominant cause).
* Adapter-level errors (5xx, rate-limit, timeout, missing adapter) — they reach the same `Err` arm.

This PR deliberately does **not** fire the record for orient's `Ok(j) if j.validate(...).is_err()` case — that is a *structurally parsed* judgment that violated post-parse invariants. It is a different failure mode (the model cooperated; the daemon rejected the content) and is tracked separately in a follow-up issue noted in [Known limits](#known-limits). When that case is addressed, it should use the same `ParseFailureRecord` shape but a distinct `error_message` prefix (`"orient validation failed: …"`) so operators reading `cycle_*.json` can distinguish the two failure modes by string-matching on `error_message` alone.

The `consecutive_count` therefore tracks consecutive *brain-invocation failures*, not strictly parse failures. The metric name (`brain_parse_failure`) is retained for back-compat with planned dashboards; a future PR may rename it once the validation-failure case is folded in.

## Why a new record

Before this PR, the only on-disk evidence of a parse-failed cycle was an indistinguishable `cycle_N.json` whose `brain_judgments[].fallback` was `true` and whose `rationale` was the literal `"deterministic fallback"`. That sentinel is also legitimately produced by the no-LLM bootstrap path — there was no way to tell "operator chose deterministic" from "LLM responded with `OK`". Goals would stall for hundreds of cycles before anyone noticed; the parent issue documents one case that ran 89 cycles at 0.00% completion across 13 days before the fail-open audit (#1245) caught it.

The new record makes the difference **machine-detectable** and **operator-visible** in one read of one cycle file: the deterministic-fallback shape is unchanged (preserving back-compat for every existing dashboard query that filters on `fallback == true`), and the discriminator is the presence of the new `parse_failure` object. Healthy cycles and bootstrap cycles continue to serialize without the key.

## The `ParseFailureRecord` schema

`ParseFailureRecord` is a serde-serializable struct exported from `simard::ooda_brain::parse_failure`. It is embedded on `BrainJudgmentRecord` via a new optional field (see below).

```rust
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParseFailureRecord {
    /// `"decide"` or `"orient"`. Other phases (act, engineer-lifecycle)
    /// have their own records; this struct is only emitted by the two
    /// `_with_brain` call sites in `simard::ooda_loop`.
    pub phase: String,
    /// The goal under which the brain was invoked. Internal id; never a
    /// user-controlled string.
    pub goal_id: String,
    /// `err.to_string()` of the `SimardError` the brain returned (the
    /// dominant variant is `AdapterInvocationFailed`; other adapter
    /// errors that reach the `Err(_)` arm of the `_with_brain` call site
    /// are recorded identically — see [Scope of the record]). Uses
    /// `Display`, never `Debug`, to avoid leaking unrelated fields from
    /// future error variants.
    pub error_message: String,
    /// The complete model response (or, for non-parse `Err` variants, an
    /// empty string when no body was returned), truncated via
    /// `simard::util::string_truncate::truncate_to_char_boundary` at 8 KiB
    /// on a UTF-8 boundary. Longer responses get a trailing
    /// `…(truncated, total N bytes)` marker (see the helper's contract in
    /// [Reference: string truncation helpers](./string-truncation-helpers.md))
    /// so the operator can correlate against the adapter log if the full
    /// body was captured upstream.
    pub raw_response_truncated: String,
    /// The name of the prompt asset the brain loaded (e.g.
    /// `"ooda_decide.md"`, `"ooda_orient.md"`). Sourced from the
    /// `DECIDE_PROMPT_NAME` / `ORIENT_PROMPT_NAME` `&'static str`
    /// constants the call site already passes to
    /// `prompt_store::current_version`. No new prompt-store API is
    /// introduced for this field.
    pub prompt_name: String,
    /// 12-char sha256 prefix of the prompt-asset content that was loaded
    /// for this brain invocation, sourced from the existing
    /// `prompt_store::current_version(name)` helper. Empty string when
    /// the prompt-asset directory is unreachable (`prompt_store` returns
    /// embedded fallback content; an empty version distinguishes that
    /// case). Together with `prompt_name`, this identifies the exact
    /// prompt bytes the model saw without re-loading the asset.
    pub prompt_version: String,
    /// How many *consecutive* failures (per `(phase, goal_id)`) have
    /// occurred up to and including this one. Resets to 0 on the next
    /// successful parse for the same key. The `gh issue create`
    /// escalation fires when this reaches `ISSUE_ESCALATION_THRESHOLD`
    /// (currently 3).
    pub consecutive_count: u32,
    /// Reserved for future retry-with-feedback. Always `false` in this
    /// release; the call site has at most one parse attempt per cycle.
    /// Kept on the schema so the JSON shape is stable when retry lands.
    pub retry_attempted: bool,
    /// RFC 3339 UTC timestamp of the failure. Set by the
    /// `record_parse_failure` helper, not by the call site, so all four
    /// visibility channels agree on the moment.
    pub timestamp: String,
}
```

### Field-level guarantees

| Field | Source | Stability |
|---|---|---|
| `phase` | `BrainPhase::Decide` / `Orient` lowercased | Enum-equivalent; treated as opaque bytes when used in `gh` `Command::args`. |
| `goal_id` | Internal goal id from `OodaState` | Originates from meeting decisions and `goal-curation` commands (so is user-influenced at origin); treated as opaque bytes — only used as a `HashMap` key and as an argument to `Command::args` (never shell-interpolated, never path-joined). |
| `error_message` | `SimardError::Display` (`err.to_string()`) | Stable across the existing `AdapterInvocationFailed { base_type, reason }` shape. `Display`, never `Debug`, so future variants that grow private fields cannot leak. |
| `raw_response_truncated` | `let mut s = raw.to_string(); truncate_to_char_boundary(&mut s, RAW_RESPONSE_TRUNCATE_BYTES);` | UTF-8-boundary safe; in-place mutation on a `String` (the existing helper signature). |
| `prompt_name` | `&'static str` constant the call site already passes to `prompt_store::current_version` (e.g. `DECIDE_PROMPT_NAME`) | No new prompt-store API needed. |
| `prompt_version` | `prompt_store::current_version(prompt_name)` (12-char sha256 prefix) | Same helper the `BrainJudgmentRecord.prompt_version` field already uses; empty string means embedded fallback was served. |
| `consecutive_count` | `Mutex<HashMap<(BrainPhase, String), u32>>` in `parse_failure` module (a `OnceLock` global) | Process-local; resets on success per `(phase, goal_id)`; cross-restart loss is acceptable and documented. |
| `retry_attempted` | Always `false` in this release | Reserved; schema-stable for future retry-with-feedback. |
| `timestamp` | `chrono::Utc::now().to_rfc3339()` at moment of failure | One source of truth across all four channels. |

`ParseFailureRecord` deliberately holds **no `Option` fields** — every field is always populated. If `prompt_version` is unknown (embedded-fallback path), it is the empty string, not a missing key.

### Why these fields and not others

* **No `cycle_id`** — `cycle_N.json` already carries it on the enclosing record; embedding here would duplicate state.
* **No `model_name` / `adapter` / `provider`** — the brain log line for the same cycle already names them; this record describes the *failure*, not the brain.
* **No `secrets_scrubbed: bool`** — truncation is the only sanitization (see [Known limits](#known-limits)). A boolean would imply guarantees the implementation does not make.
* **No `prompt_summary`** — earlier drafts proposed a one-line prose summary, but the existing `prompt_name` + `prompt_version` pair (both already sourced from `prompt_store`) is already sufficient to identify the exact prompt bytes, and avoids introducing a new prompt-store API (`prompt_summary(name) -> String`) just for this record. Operators who need the full prompt can `cat $SIMARD_PROMPT_ASSETS_DIR/$prompt_name` (or read the embedded fallback in `src/ooda_brain/prompt_store.rs`).
* **No back-references to `BrainJudgmentRecord.rationale`** — the rationale field continues to hold the existing fallback rationale (`"deterministic fallback"`) so `BrainJudgmentRecord` remains self-describing for callers that don't load `parse_failure`. The `parse_failure.error_message` carries the human-readable failure detail.

## `BrainJudgmentRecord` extension

The existing record gains exactly one additive field:

```rust
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BrainJudgmentRecord {
    pub phase: BrainPhase,
    pub context_summary: String,
    pub decision: String,
    pub rationale: String,
    pub confidence: f32,
    pub fallback: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt_version: String,

    /// Present only when this record corresponds to a brain JSON-parse
    /// failure (decide / orient call sites). `None` on every healthy
    /// cycle and on every deterministic-bootstrap cycle, so consumers
    /// that don't care about parse failures see no schema change at
    /// all. See [`ParseFailureRecord`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_failure: Option<ParseFailureRecord>,
}
```

### Back-compat properties

* `#[serde(default, skip_serializing_if = "Option::is_none")]` — older `cycle_*.json` files (before this PR) round-trip identically; newer files without a parse failure emit no extra key.
* `decision` and `rationale` keep their pre-existing semantics — the deterministic substitution still populates them — so dashboards that index on `decision` continue to work.
* `fallback` remains `true` on a parse-failure cycle for the same reason. Readers that previously branched on `fallback` get the same behavior; readers that need to distinguish operator-chosen deterministic from forced-by-failure deterministic now check `parse_failure.is_some()`.

### One-line discriminator

A `cycle_N.json` reader (dashboard, ad-hoc `jq`) can classify a fallback cycle in one expression:

```bash
jq '.brain_judgments[]
    | select(.fallback == true)
    | { phase, forced_by_parse_failure: (.parse_failure != null) }' \
   ~/.simard/cycle_reports/cycle_42.json
```

## The four visibility channels

Every call to `record_parse_failure(phase, goal_id, &err, &raw, prompt_name)` fires the four channels in a **fixed sequence**: tracing → metric → counter-increment → in-memory record returned for embedding into `BrainJudgmentRecord` → conditional `gh issue create`. The sequence is not atomic (channel 4 spawns a subprocess; channel 3 only lands on disk when `persist_cycle_report` runs at end-of-cycle), but an observer that sees channel N is guaranteed to have a record of channels `1..N−1` as well: the helper runs synchronously and only returns once channels 1, 2, and 4 (the externally observable ones) have been invoked.

**Sequencing relative to the fallback brain.** The four channels fire *before* the deterministic fallback brain is invoked, so even if the fallback itself errors (the `?` propagation on `fallback.judge_decision(&ctx)?` in `decide.rs`) the parse failure has already been recorded on channels 1, 2, and 4. Only channel 3 (the on-disk `BrainJudgmentRecord`) is lost in that pathological case — the cycle never gets to call `push_brain_judgment` — but the tracing event, the metric increment, and the auto-filed issue all survive. This sequencing is asserted by a regression test that injects a failing fallback brain and verifies channels 1, 2, and 4 still fire.

### 1. Structured `tracing::error!`

Target: `simard::ooda_brain`. Level: `ERROR`. Shape:

```
ERROR simard::ooda_brain: brain.{phase} parse failed
    phase="decide"
    goal_id="improve-amplihack-test-coverage"
    error="base type \"ooda-brain\" failed during invocation: …"
    raw_response_truncated="OK"
    prompt_name="ooda_decide.md"
    prompt_version="a1b2c3d4e5f6"
    consecutive_count=1
    retry_attempted=false
```

The fields are emitted via `tracing`'s structured-field syntax, not interpolated into the message, so log subscribers (jsonl writer, dashboard tail) get a real key/value map. The message string itself is constant (`brain.decide parse failed` / `brain.orient parse failed`) so subscribers can `MATCH` on it without regex. Per the `target=simard::ooda_brain` namespace, operators who need to tame a pathological per-cycle failure loop on the console (without losing on-disk evidence) can subscriber-side filter on a different `MAX_LEVEL` for that target rather than touching the helper.

### 2. `brain_parse_failure` metric

```rust
let ctx_json = serde_json::to_string(&serde_json::json!({
    "phase": "decide",
    "goal_id": &goal_id,
    "retry_attempted": false,
    "consecutive_count": 1u32,
}))
.unwrap_or_default();
let _ = record_metric("brain_parse_failure", 1.0, &ctx_json);
```

The existing `record_metric(name: &str, value: f64, context: &str)` API (in `simard::self_metrics`) takes `context` as an opaque string; the helper above renders the structured dimensions as a JSON object inside that string, matching the convention the dashboard already uses to parse `context` back out. The return value is ignored — a metric-write failure must not abort a cycle, and channels 1 and 3 still record the event. The metric lands in `~/.simard/metrics/metrics.jsonl` (single append-only file; see `metrics_file_path()`). One counter name with dimensions (per A7 in the spec); aggregation by phase is `select(.metric_name == "brain_parse_failure") | (.context | fromjson).phase`.

### 3. `BrainJudgmentRecord.parse_failure` on disk

The record produced by `record_parse_failure` is wrapped into the existing `push_brain_judgment(BrainJudgmentRecord { …, parse_failure: Some(record), … })` call. `persist_cycle_report` in `operator_commands_ooda::persistence` serializes it via `serde_json::to_value` with no manual schema work — the field is on the struct, so it lands in the JSON.

### 4. Throttled `gh issue create`

When `consecutive_count >= ISSUE_ESCALATION_THRESHOLD` (currently `3`), the helper spawns:

```rust
std::process::Command::new("gh")
    .args([
        "issue", "create",
        "--repo", ESCALATION_REPO_SLUG,     // compile-time &'static str
        "--title", &title,                  // pre-formatted, no shell
        "--body-file", body_path,           // NamedTempFile, never --body
        "--label", "ooda-brain-parse-failure",
        "--label", "auto-filed",
    ])
    .status()
```

The title pattern is `OODA decide brain parse failure: goal=<id> (N consecutive)`; the body is a `tempfile::NamedTempFile`-backed markdown file containing the full `ParseFailureRecord` serialized as a fenced JSON block plus a short hand-off paragraph. **No shell interpolation, ever** — see [Security](#security).

`ESCALATION_REPO_SLUG` is a compile-time `&'static str` constant (`"rysweet/Simard"` in this repo) defined alongside `ISSUE_ESCALATION_THRESHOLD`. This matches the existing `--repo` pattern used by `stewardship/merge_authority.rs` and `worktree_gc/runner.rs` — the slug is passed positionally to `Command::args`, never interpolated into a shell string. Forks rebuilding this binary must edit the constant; this is intentional, so escalations cannot be silently redirected by an env var or runtime flag.

## Operator-visible decision tree

```
                    LLM brain returns…
                          │
            ┌─────────────┴──────────────┐
        Ok(judgment)              Err(AdapterInvocationFailed{…})
            │                             │
            ▼                             ▼
   continue cycle             record_parse_failure(…)
   parse_failure = None              │
                                     ├─► tracing::error!  (ch.1)
                                     ├─► record_metric   (ch.2)
                                     ├─► BrainJudgmentRecord.parse_failure = Some(_)  (ch.3)
                                     ├─► counter[ (phase, goal_id) ] += 1
                                     │       │
                                     │       └─► if ≥3:  gh issue create  (ch.4)
                                     │
                                     ▼
                          DeterministicFallback*Brain
                          (cycle continues; BrainJudgmentRecord keeps its
                           existing fallback shape — fallback: true,
                           rationale: "deterministic fallback",
                           prompt_version: "" — but the new
                           parse_failure: Some(_) field is the
                           machine-readable discriminator.)
```

On the **next** successful parse for the same `(phase, goal_id)`, the counter is reset to `0`. The threshold therefore means *three in a row*, not *three in this cycle's lifetime*.

## Configuration

None. No new env var, CLI flag, or `~/.simard/config` key is introduced. The visibility channels are non-opt-out by design — silencing them was the bug that #1890 closes. Three compile-time constants live in `parse_failure.rs` for reviewers:

| Constant | Value | Meaning |
|---|---|---|
| `RAW_RESPONSE_TRUNCATE_BYTES` | `8192` | Cap fed to `truncate_to_char_boundary`. Matches the existing helper's default for `~/.simard/logs` protection. |
| `ISSUE_ESCALATION_THRESHOLD` | `3` | Mirror of `spawn_engineer`/`#1711` throttle (see A6). |
| `ESCALATION_REPO_SLUG` | `"rysweet/Simard"` | `--repo` target for `gh issue create`. Forks must edit at compile time. |

Operators who want to disable the `gh issue create` channel (e.g., on an air-gapped daemon) should ensure the `gh` binary is not on `PATH`; the spawn fails fast, the failure is itself logged at `WARN`, and the other three channels continue unaffected. There is intentionally no "silence the issue filer" boolean — see [#1245](https://github.com/rysweet/Simard/issues/1245).

## Cycle-report shape

A `cycle_N.json` produced by a cycle that hit a decide-brain parse failure now contains a `brain_judgments` entry shaped as:

```json
{
  "phase": "decide",
  "context_summary": "goal=improve-amplihack-test-coverage; urgency=…; reason=…",
  "decision": "advance_goal",
  "rationale": "deterministic fallback",
  "confidence": 0.5,
  "fallback": true,
  "prompt_version": "",
  "parse_failure": {
    "phase": "decide",
    "goal_id": "improve-amplihack-test-coverage",
    "error_message": "base type \"ooda-brain\" failed during invocation: no JSON object found in LLM response (got 3 bytes)",
    "raw_response_truncated": "OK",
    "prompt_name": "ooda_decide.md",
    "prompt_version": "a1b2c3d4e5f6",
    "consecutive_count": 1,
    "retry_attempted": false,
    "timestamp": "2026-05-19T04:44:26.848Z"
  }
}
```

The `decision`, `rationale`, `confidence`, `fallback`, and `prompt_version` fields keep the values that `BrainJudgmentRecord::from_decide(..., fallback=true, prompt_version="")` (the existing fallback path) already produces — this PR does **not** introduce a synthetic `"deterministic-fallback (forced)"` label or rewrite the rationale. The `parse_failure.is_some()` discriminator is the single signal operators and dashboards use to distinguish "operator deliberately ran without an LLM brain" (no `parse_failure` key) from "LLM brain failed and the deterministic floor caught it" (`parse_failure` present). Note that `prompt_version` on the outer record is empty here because the fallback brain doesn't read a prompt asset; the `parse_failure.prompt_version` field carries the version of the LLM brain's prompt that produced the unparseable response.

Healthy cycles serialize **byte-for-byte identically** to the pre-PR shape: the `parse_failure` key is omitted via `skip_serializing_if`. The TR4 regression test fixture pins this property.

## Security

* **`gh` invocation discipline.** All `gh issue create` calls use `Command::args([...])` with positional arguments. Bodies are passed via `--body-file <NamedTempFile>` (RAII-cleaned), not `--body` with `format!()`. The codebase contains an explicit grep-based test (`gh_issue_invocation_uses_arg_array_not_shell`) that fails CI if any caller composes a `gh` invocation from an interpolated string.
* **Tempfile mode.** `tempfile::NamedTempFile::new()` creates the body file with mode `0600` on Unix (the crate's documented default; the open uses `O_CREAT | O_EXCL` and chmod-on-create). A regression test (`gh_body_tempfile_is_0600`) opens the file via `metadata().permissions().mode()` and asserts the low 9 bits are `0o600`, so a future tempfile-crate change or platform port can't silently relax this.
* **Display, not Debug.** `error_message` is `err.to_string()`. Future `SimardError` variants that grow sensitive fields cannot leak into the record via `{:?}`.
* **Truncation as sanitization.** `raw_response_truncated` is `truncate_to_char_boundary(&mut s, 8192)`. No regex secret-scrubbing — see [Known limits](#known-limits).
* **No new auth surface.** The `gh` channel reuses the operator's pre-existing `GH_TOKEN` / `gh auth` configuration. No new credential file, no new env var.
* **Process-local counter.** The `(phase, goal_id) → u32` map lives behind `Mutex<HashMap<…>>` (in a `OnceLock`) to prevent a race when decide and orient fail for the same goal in the same cycle (SR7). Cross-restart counter loss is documented and accepted — the worst case is one extra `gh issue` per daemon restart loop, also documented in the PR body.
* **JSON-injection resistance.** `ParseFailureRecord` is built as a typed struct and serialized via `serde_json` — no manual string concatenation into JSON. The `parse_failure_record_serializes_safely_with_quotes_and_braces` regression test feeds a `raw_response_truncated` containing `"}` and asserts the resulting JSON round-trips.

## Known limits

* **No secret scrubbing of LLM output.** Per A9 in the requirements, `raw_response_truncated` contains whatever the model returned, truncated only. The model is reading our prompt; secret exposure in its *response* is low-probability in practice. If a leak class emerges, it will be addressed separately — there is no module-level toggle to add scrubbing.
* **No retry-with-feedback in this release.** The `retry_attempted` field is reserved and always `false`. A future PR can wire a single-attempt retry through the brain trait without changing this record's JSON shape.
* **Counter is process-local.** Daemon restart resets all `(phase, goal_id)` counters to zero. A pathological restart loop could file one issue per (3 × restart). Operators who see this should investigate the restart loop, not the threshold.
* **Counter map is unbounded.** The `(BrainPhase, String) → u32` map is keyed by `goal_id` and pruned only by successful-parse reset, not by absolute time or size. For daemons with unbounded goal cardinality the map grows monotonically (bounded only by distinct `goal_id` count per process lifetime). In practice goal cardinality is small (top-5 active + backlog) and daemon process lifetimes are bounded by `safe-update`, so no eviction logic is gated. If a future deployment shape changes this, an LRU cap is the obvious next step and does not change the JSON shape.
* **`gh` is best-effort.** If `gh` is missing, unauthenticated, or rate-limited, the spawn failure is logged at `WARN` and the other three channels still fire. The cycle does not block on the network.
* **Tracing-flood self-limiting.** A pathological per-cycle failure produces one `ERROR` line per cycle (one per phase). At default cycle cadence this is bounded by the cycle pacing, not by an in-helper rate limit; operators who need a louder cap can subscriber-side filter `target=simard::ooda_brain` to a lower level. The `gh` channel's per-`(phase, goal_id)` throttle (A6) is the protection against issue-tracker flooding; the tracing channel intentionally remains uncapped so the on-disk evidence is complete.
* **Validation-failure case not covered.** The orient call site collapses `Ok(j) if j.validate(...).is_err()` into the same `_ =>` deterministic-substitution arm. This PR fires the record for `Err(_)` only, not for `Ok-but-invalid`. The latter is a structurally separate failure mode tracked in a follow-up issue; when addressed it should reuse `ParseFailureRecord` with an `error_message` prefix of `"orient validation failed: …"` to keep operator tooling unchanged.
* **Deterministic-fallback brains are unchanged.** This PR does **not** remove `DeterministicFallbackDecideBrain` / `DeterministicFallbackOrientBrain`. They are still the legitimate substitute for the no-LLM bootstrap path (operator deliberately ran the daemon without an LLM brain configured). Removing them is the scope of [#1748](https://github.com/rysweet/Simard/issues/1748), explicitly out of scope here.

## Operator replay surface

To let operators reproduce a parse failure without re-running the daemon, this PR exposes two `pub(crate)` helpers (re-exported `pub` under `#[cfg(any(test, feature = "ooda-brain-replay"))]` so they are available to integration tests and ad-hoc replay binaries without widening the always-shipped public API):

```rust
pub fn try_parse_decide_response(raw: &str)
    -> Result<DecideJudgment, SimardError>;
pub fn try_parse_orient_response(raw: &str)
    -> Result<OrientJudgment, SimardError>;
```

Both helpers wrap the same parse path that `RustyClawdDecideBrain::run` / `RustyClawdOrientBrain::run` use internally. They take a `&str` matching the `raw_response_truncated` field shape and return either a typed judgment or the same `SimardError::AdapterInvocationFailed` variant the call site sees in production. Operator-replay use is documented in the [how-to guide](../howto/diagnose-decide-orient-parse-failures.md#step-3-replay-the-prompt-locally). These helpers are the only new public surface this PR introduces beyond `ParseFailureRecord` and the `parse_failure` field on `BrainJudgmentRecord`.

## See also

* [How-to: diagnose decide/orient brain parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook for the new logs, metric, and `cycle_*.json` shape.
* [Reference: OODA Brain Decision Protocol](./ooda-brain-decision-protocol.md) — sibling visibility contract for the engineer-lifecycle brain (#1711).
* [How-to: diagnose OODA brain decision parse failures](../howto/diagnose-brain-decision-parse-failures.md) — engineer-lifecycle equivalent of this PR's runbook.
* [Fail-Open Audit (P5 / #1245)](../fail-open-audit.md) — broader audit that this PR partially discharges.
* [Reference: `OodaBrain` API](./ooda-brain-api.md) — public surface of the brain trait, unchanged by this PR.
* [Reference: string truncation helpers](./string-truncation-helpers.md) — `truncate_to_char_boundary` UTF-8 contract.
