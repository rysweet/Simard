# Reference: `OodaBrain` API

Crate: `simard` · Module: `simard::ooda_brain`

The `OodaBrain` trait is the seam between Simard's deterministic OODA loop and
prompt-driven decision-making. As of this PR it is wired into one site —
`dispatch_spawn_engineer`'s skip path — but the trait, context, and decision
types are designed to absorb the other OODA phases incrementally.

## Trait

```rust
pub trait OodaBrain: Send + Sync {
    fn decide_engineer_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision>;
}
```

* **Synchronous** by design. `RustyClawdBrain` blocks on its async submitter
  internally so callers in non-async OODA code never deal with futures. The
  exact bridging mechanism (current-thread runtime owned by the brain, or a
  borrowed handle) is an implementation detail of `LlmSubmitter`.
* `Send + Sync` so a single instance can be borrowed across the action
  dispatcher.

## Context

```rust
pub struct EngineerLifecycleCtx {
    pub goal_id: String,
    pub goal_description: String,
    pub cycle_number: u64,
    pub consecutive_skip_count: u32,
    pub failure_count: u32,
    pub worktree_mtime_secs_ago: u64,
    pub sentinel_pid: Option<u32>,
    pub last_engineer_log_tail: String,
}
```

Built by:

```rust
pub(crate) fn gather_engineer_lifecycle_ctx(
    state: &OodaState,
    goal_id: &str,
    live: &engineer_worktree::LiveEngineer,
) -> EngineerLifecycleCtx;
```

Each field is best-effort: missing log files, unreadable mtimes, and absent
state entries degrade to default values (`0`, `""`, `None`) — they never
propagate errors.

## Decision

```rust
#[derive(serde::Serialize, Debug, Clone)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum EngineerLifecycleDecision {
    ContinueSkipping {
        rationale: String,
    },
    ReclaimAndRedispatch {
        rationale: String,
        redispatch_context: String,
    },
    Deprioritize {
        rationale: String,
    },
    OpenTrackingIssue {
        rationale: String,
        title: String,
        body: String,
    },
    MarkGoalBlocked {
        rationale: String,
        reason: String,
    },
    ConsiderSelfUpdate {
        rationale: String,
    },
}
```

The enum has **6 variants**. `ConsiderSelfUpdate` is dispatched by
`apply_lifecycle_decision` to the `simard safe-update` path; it is the only
variant that can mutate the running daemon binary.

`Deserialize` is no longer derived — the enum is never deserialized from
JSON. It is constructed from text-parsed fields via the DECISION marker
protocol (see [text-parsing wire formats § engineer lifecycle](../reference/text-parsing-wire-formats.md#1c-engineer-lifecycle-rustyclawdrs)).
`Serialize` is retained for logging and state persistence.

## Implementations

### `RustyClawdBrain<S: LlmSubmitter>`

Production brain. Loads the prompt via
`include_str!("../../prompt_assets/simard/ooda_brain.md")`, substitutes
`{{var}}` placeholders from the context, submits to an `LlmSubmitter`, and
parses the text response using the DECISION marker protocol.

The parser finds the first `DECISION: <variant>` line, extracts labeled
fields for structured variants, and collects remaining lines as rationale.
There is no JSON fallback — the DECISION marker is the sole parser. See
[text-parsing wire formats § engineer lifecycle](../reference/text-parsing-wire-formats.md#1c-engineer-lifecycle-rustyclawdrs)
for the full grammar.

```rust
pub struct RustyClawdBrain<S: LlmSubmitter> {
    submitter: S,
}

impl<S: LlmSubmitter> RustyClawdBrain<S> {
    pub fn new(submitter: S) -> Self;
}
```

The free function `build_rustyclawd_brain()` constructs the production
brain backed by `RustyClawdAdapter`:

```rust
pub fn build_rustyclawd_brain() -> SimardResult<RustyClawdBrain<RustyClawdSubmitter>>;
```

Returns `Err` when the adapter cannot be constructed (no provider configured,
no API key, etc.). Callers in `cycle.rs` match on the result and fall back to
`DeterministicFallbackBrain` on error.

**Adapter session lifetime.** The underlying `RustyClawdAdapter` session is
opened lazily on the first `submit()` call and dropped when the brain is
dropped at cycle end (via the adapter's existing `Drop` impl). One brain
instance therefore corresponds to at most one adapter session per cycle.

### `DeterministicFallbackBrain`

```rust
pub struct DeterministicFallbackBrain;
```

Always returns `ContinueSkipping { rationale: "deterministic fallback" }`.
Used when `build_rustyclawd_brain()` fails to construct. Preserves the
exact pre-feature behavior of `dispatch_spawn_engineer`.

## Submitter Seam

```rust
pub(crate) trait LlmSubmitter: Send + Sync {
    fn submit(&self, prompt: &str) -> SimardResult<String>;
}
```

Production: `RustyClawdSubmitter` (wraps `RustyClawdAdapter`).
Tests: `StubSubmitter { canned: String }` returns a fixed text response. This is
the seam used by all hermetic unit tests for the brain.

## Errors

> **New in [#1711](https://github.com/rysweet/Simard/issues/1711).**
> `BrainResponseUnparseable` is a new `SimardError` variant introduced by
> this PR. Pre-#1711 callers surfaced parse failures via the generic
> `BaseTypeInvocation` error path, which is what the legacy `got N bytes`
> log line was rendering.

```rust
SimardError::BrainResponseUnparseable {
    raw: String,
    source: BrainParseSource,
}

/// Wraps the underlying cause — marker-grammar failure only.
/// The legacy `Json(serde_json::Error)` variant was removed in #1980.
pub enum BrainParseSource {
    Marker(MarkerParseError),
}
```

* `raw` is the **complete, untruncated** model response text — stored in
  full so that `Debug` formatting (`{:#?}`) and downstream tooling can see
  exactly what the model returned. Truncation to `MAX_RAW_LOG_BYTES = 8192`
  is applied only at log-format time by the shared `truncate_for_log`
  helper (see [`truncate_for_log` reuse](#truncate_for_log-reuse) below).
* As of [#1711](https://github.com/rysweet/Simard/issues/1711) every
  parse-failure log line embeds the full (truncated-for-log) text — the
  legacy `got N bytes` summary that hid the diagnostic information has
  been removed at all three lossy parser sites: `rustyclawd.rs`,
  `decide.rs`, and `orient.rs`.
* `raw` is rendered with the `{:?}` Debug format wherever it appears in
  log lines, so control characters and ANSI escapes are escaped (defends
  against CRLF / log-injection in model output).

### `truncate_for_log` reuse

A `truncate_for_log` helper already exists at
`src/ooda_actions/advance_goal/spawn.rs:318`. As part of this PR it is
**hoisted** to a shared module (`src/util/log.rs`) and re-exported as
`crate::util::log::truncate_for_log`, so all four log sites
(`spawn.rs`, `rustyclawd.rs`, `decide.rs`, `orient.rs`) call the same
implementation. The previous private function in `spawn.rs` becomes a
re-export shim to keep the diff small.

The caller (`dispatch_spawn_engineer`) logs this and falls back to the
deterministic skip outcome. The cycle never panics, never aborts.

For the wire format that `parse_decision_from_response` accepts, see
[Reference: OODA Brain Decision Protocol](ooda-brain-decision-protocol.md).
For an operator runbook that consumes these logs, see
[How-to: diagnose brain decision parse failures](../howto/diagnose-brain-decision-parse-failures.md).

## Construction Pattern

```rust
let brain: Box<dyn OodaBrain> = match build_rustyclawd_brain() {
    Ok(b) => Box::new(b),
    Err(e) => {
        eprintln!("[ooda_brain] init failed: {e}; using deterministic fallback");
        Box::new(DeterministicFallbackBrain)
    }
};
dispatch_actions(actions, &mut state, brain.as_ref());
```

Constructed **once per cycle** in `ooda_loop/cycle.rs`; dropped at cycle end.

## Side-Effect Handler

The decision returned by the brain is applied by a handler in a separate
module, not in `ooda_brain` itself:

```
src/ooda_actions/advance_goal/lifecycle.rs::apply_lifecycle_decision(
    &mut OodaState,
    &str,                          // goal_id
    EngineerLifecycleDecision,
    &engineer_worktree::LiveEngineer,
) -> ActionOutcome
```

Keeping the handler outside `ooda_brain` preserves the brain's purity (input
context → decision) and lets state mutations live alongside the other
`advance_goal` actions.

## Module Layout

```
src/ooda_brain/
├── mod.rs            # trait, decision enum, error wiring           (~110 LOC)
├── ctx.rs            # gather_engineer_lifecycle_ctx + redaction    (~90  LOC)
├── rustyclawd.rs     # RustyClawdBrain + LlmSubmitter               (~140 LOC)
├── decide.rs         # OodaDecideBrain trait, DecideJudgment,
│                     #   DeterministicFallbackDecideBrain            (~200 LOC)
├── recipe_decide.rs  # RecipeDecideBrain: recipe-runner-rs shim
│                     #   + keyword scanner + inline tests            (~180 LOC)
├── fallback.rs       # DeterministicFallbackBrain                   (~35  LOC)
├── decide_tests.rs   # JSON round-trip + DeterministicFallback tests (~90  LOC)
└── tests.rs          # parse, ctx, integration tests                (~200 LOC)
```

All files respect the per-module 400-LOC cap (#1266).

## Decide Brain: `RecipeDecideBrain`

> **New in [#2111](https://github.com/rysweet/Simard/issues/2111).**
> Replaces `RustyClawdDecideBrain` which parsed `DECISION:` markers.

```rust
pub struct RecipeDecideBrain {
    recipe_path: PathBuf,
}

impl RecipeDecideBrain {
    /// Returns `Some(brain)` if `recipe-runner-rs` is on $PATH and the
    /// recipe YAML exists; `None` otherwise.
    pub fn new(repo_root: &Path) -> Option<Self>;
}

impl OodaDecideBrain for RecipeDecideBrain {
    fn judge(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment>;
}
```

The `judge` method:

1. Spawns `recipe-runner-rs` as a subprocess with the recipe path and
   context variables (`goal_id`, `urgency`, `reason`) passed as arguments.
2. Captures stdout.
3. Scans stdout for action keywords using `parse_action_from_text()`.
4. Returns the matched `DecideJudgment` variant.

### `parse_action_from_text`

```rust
pub(crate) fn parse_action_from_text(text: &str) -> DecideJudgment;
```

Scans the lowercased text for any of the 10 known action keywords using
`contains()`. Returns the first match. If no keyword is found, returns
`DecideJudgment::AdvanceGoal` (the safe default).

This function is the keyword-verdict equivalent of the deleted
`parse_judgment_from_response` — but simpler, because it has no failure
mode. It always returns a valid `DecideJudgment`.

### Construction pattern

```rust
// In daemon/brains.rs:
pub fn build_decide_brain(
    state_root: &Path,
    repo_root: &Path,
) -> Box<dyn OodaDecideBrain> {
    match RecipeDecideBrain::new(repo_root) {
        Some(b) => Box::new(b),
        None => {
            eprintln!("[ooda] recipe-runner-rs not found; \
                       using deterministic decide fallback");
            Box::new(DeterministicFallbackDecideBrain)
        }
    }
}
```

### What was deleted

- `RustyClawdDecideBrain` — compiled in prompt via `include_str!`,
  submitted to `LlmSubmitter`, parsed `DECISION:` markers.
- `parse_judgment_from_response` — the `DECISION:` marker parser in
  `decide.rs`.
- `build_rustyclawd_decide_brain` — factory function.
- `StubSubmitter` and `RustyClawdDecideBrain` tests in `decide_tests.rs`.

## Test Inventory

`src/ooda_brain/tests.rs` is the authoritative inventory for the
engineer-lifecycle brain; the file ships the legacy parse / context /
end-to-end tests **plus** the 15 protocol tests (T1 – T15) enumerated in
the **Behavior matrix** of the
[OODA Brain Decision Protocol reference](ooda-brain-decision-protocol.md#behavior-matrix).

`src/ooda_brain/recipe_decide.rs` contains inline `#[cfg(test)]` tests for
the keyword scanner, covering all 10 action keywords, the no-keyword
default, mixed-case input, and multi-keyword input.

`src/ooda_brain/decide_tests.rs` retains the JSON round-trip tests for
`DecideJudgment` serialization and the `DeterministicFallbackDecideBrain`
tests.

## See Also

* [Concept: prompt-driven OODA brain](../concepts/prompt-driven-ooda-brain.md)
* [Reference: `ooda_brain.md` prompt schema](ooda-brain-prompt.md)
* [Reference: OODA decide recipe and prompt schema](ooda-decide-prompt.md)
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md)
* [Reference: base type adapters](base-type-adapters.md)
