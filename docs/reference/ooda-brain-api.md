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
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
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
}
```

Forward-compatible:

* New optional fields use `#[serde(default)]` so older payloads that omit them
  still parse.
* Unknown extra fields are ignored (serde's default; `deny_unknown_fields`
  is intentionally **not** set).
* Unknown `choice` values fail parsing and trigger the safe fallback (skip).

## Implementations

### `RustyClawdBrain<S: LlmSubmitter>`

Production brain. Loads the prompt via
`include_str!("../../prompt_assets/simard/ooda_brain.md")`, substitutes
`{{var}}` placeholders from the context, submits to an `LlmSubmitter`, and
parses the JSON response.

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
Tests: `StubSubmitter { canned: String }` returns a fixed JSON string. This is
the seam used by all hermetic unit tests for the brain.

## Errors

Added to `SimardError`:

```rust
SimardError::BrainResponseUnparseable {
    raw: String,
    source: serde_json::Error,
}
```

Caller (`dispatch_spawn_engineer`) logs this and falls back to the deterministic
skip outcome. The cycle never panics, never aborts.

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
├── mod.rs          # trait, decision enum, error wiring         (~110 LOC)
├── ctx.rs          # gather_engineer_lifecycle_ctx + redaction  (~90  LOC)
├── rustyclawd.rs   # RustyClawdBrain + LlmSubmitter             (~140 LOC)
├── fallback.rs     # DeterministicFallbackBrain                 (~35  LOC)
└── tests.rs        # parse, ctx, integration tests              (~200 LOC)
```

All files respect the per-module 400-LOC cap (#1266).

## Test Inventory

`src/ooda_brain/tests.rs` ships eight named tests covering parse, context
assembly, and end-to-end brain behavior:

1. `parse_continue_skipping_minimal`
2. `parse_reclaim_and_redispatch_full`
3. `parse_deprioritize_round_trip`
4. `parse_open_tracking_issue_round_trip`
5. `parse_mark_goal_blocked_round_trip`
6. `parse_unknown_choice_yields_unparseable_error`
7. `gather_ctx_handles_missing_log_and_state`
8. `rustyclawd_brain_with_stub_submitter_returns_decision`

These act as the TDD checklist for the builder.

## See Also

* [Concept: prompt-driven OODA brain](../concepts/prompt-driven-ooda-brain.md)
* [Reference: `ooda_brain.md` prompt schema](ooda-brain-prompt.md)
* [Reference: base type adapters](base-type-adapters.md)
