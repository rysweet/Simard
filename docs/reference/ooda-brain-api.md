---
title: OODA Reasoners API
description: Reference for the orient/decide/act reasoner traits of the Brain's OODA thread — their contexts and decision types, the RecipeReasoner and RustyClawd*Reasoner implementations, and the deterministic fallbacks.
last_updated: 2026-07-03
owner: simard
doc_type: reference
related:
  - ../architecture/brain-model.md
  - ../concepts/unified-recipe-brain.md
  - ./recipe-brain-api.md
  - ./ooda-brain-decision-protocol.md
  - ./brain-terminology-migration.md
---

# Reference: OODA Reasoners API

Crate: `simard` · Module: `simard::ooda_reasoners`

The Brain's **OODA thread** performs active cognition. Its three per-phase LLM
components are **reasoners**, not "brains" — the whole cognition is the
[Brain](../architecture/brain-model.md); a single phase reasons. There are three
sibling reasoner traits:

- **`OrientReasoner`** (orient phase),
- **`DecideReasoner`** (decide phase),
- **`ActReasoner`** (act / engineer-lifecycle phase).

Each has a deterministic fallback (`DeterministicFallbackOrientReasoner`,
`DeterministicFallbackDecideReasoner`, `DeterministicFallbackActReasoner`) and a
production implementation (`RecipeReasoner` and/or `RustyClawd*Reasoner`). The
three reasoners are **kept separate** — they are named individually, never
merged.

!!! note "Behavior-preserving"
    The engineer-lifecycle decision protocol and every wire/serde value are
    unchanged; only the Rust identifiers differ. For the historical old→new map
    see the [terminology migration](./brain-terminology-migration.md).

## The `ActReasoner` trait (act / engineer-lifecycle)

The `ActReasoner` trait is the seam between the deterministic OODA loop and
prompt-driven decision-making for the **engineer-lifecycle** phase. It is
consulted by `dispatch_spawn_engineer`
(`simard::ooda_actions::advance_goal::spawn`), which calls
`decide_engineer_lifecycle` before spawning or skipping an engineer subprocess.

```rust
pub trait ActReasoner: Send + Sync {
    fn decide_engineer_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision>;
}
```

* **Synchronous** by design. `RustyClawdActReasoner` blocks on its async
  submitter internally so callers in non-async OODA code never deal with
  futures. The exact bridging mechanism (current-thread runtime owned by the
  reasoner, or a borrowed handle) is an implementation detail of `LlmSubmitter`.
* `Send + Sync` so a single instance can be borrowed across the action
  dispatcher.

### Context

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

### Decision

```rust
#[derive(serde::Serialize, Debug, Clone)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum EngineerLifecycleDecision {
    ContinueSkipping { rationale: String },
    ReclaimAndRedispatch { rationale: String, redispatch_context: String },
    Deprioritize { rationale: String },
    OpenTrackingIssue { rationale: String, title: String, body: String },
    MarkGoalBlocked { rationale: String, reason: String },
    ConsiderSelfUpdate { rationale: String },
}
```

The enum has **6 variants** and its serde tag/values are frozen.
`ConsiderSelfUpdate` is dispatched by `apply_lifecycle_decision` to the
`simard safe-update` path; it is the only variant that can mutate the running
daemon binary. `Deserialize` is not derived — the enum is constructed from
text-parsed fields via the DECISION marker protocol (see
[text-parsing wire formats § engineer lifecycle](./text-parsing-wire-formats.md#1c-engineer-lifecycle-recipe_reasonerrs)).

## Implementations

### `RustyClawdActReasoner<S: LlmSubmitter>`

Production act reasoner. Loads the prompt via
`include_str!("../../prompt_assets/simard/ooda_reasoner_act.md")`, substitutes
`{{var}}` placeholders from the context, submits to an `LlmSubmitter`, and
parses the text response using the DECISION marker protocol.

```rust
pub struct RustyClawdActReasoner<S: LlmSubmitter> {
    submitter: S,
}

impl<S: LlmSubmitter> RustyClawdActReasoner<S> {
    pub fn new(submitter: S) -> Self;
}
```

The free function `build_act_reasoner()`
constructs the production reasoner backed by `RustyClawdActReasoner`:

```rust
pub fn build_act_reasoner() -> SimardResult<RustyClawdActReasoner<RustyClawdSubmitter>>;
```

Returns `Err` when the adapter cannot be constructed (no provider configured,
no API key, etc.). Callers in `cycle.rs` match on the result and fall back to
`DeterministicFallbackActReasoner` on error.

**Adapter session lifetime.** The underlying `RustyClawdAdapter` session is
opened lazily on the first `submit()` call and dropped when the reasoner is
dropped at cycle end. One reasoner instance therefore corresponds to at most one
adapter session per cycle.

### `DeterministicFallbackActReasoner`

```rust
pub struct DeterministicFallbackActReasoner;
```

Always returns `ContinueSkipping { rationale: "deterministic fallback" }`.
Used when `build_act_reasoner()` fails to construct. Preserves the exact
pre-feature behavior of `dispatch_spawn_engineer`.

### `RecipeReasoner`

`RecipeReasoner` is the recipe-runner-rs-backed
implementation that can serve as the orient, decide, or act reasoner. It spawns
`recipe-runner-rs` as a subprocess with the phase recipe and context variables,
captures stdout, and scans it for the phase's action keywords. See
[RecipeReasoner API](./recipe-brain-api.md) for the shared surface.

## The Decide and Orient reasoners

```rust
pub trait DecideReasoner: Send + Sync {
    fn judge(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment>;
}

pub trait OrientReasoner: Send + Sync {
    fn orient(&self, ctx: &OrientContext) -> SimardResult<Orientation>;
}
```

Both are implemented by `RecipeReasoner` (production) with
`DeterministicFallbackDecideReasoner` / `DeterministicFallbackOrientReasoner`
as the deterministic floors. The keyword-verdict scanner
`parse_action_from_text` is unchanged — it always returns a valid
`DecideJudgment` (defaulting to `AdvanceGoal`).

## Submitter Seam

```rust
pub(crate) trait LlmSubmitter: Send + Sync {
    fn submit(&self, prompt: &str) -> SimardResult<String>;
}
```

Production: `RustyClawdSubmitter` (wraps `RustyClawdAdapter`).
Tests: `StubSubmitter { canned: String }` returns a fixed text response — the
seam used by all hermetic unit tests for the reasoners.

## Errors

```rust
SimardError::ReasonerResponseUnparseable {
    raw: String,
    source: ReasonerParseSource,
}

/// Wraps the underlying cause — marker-grammar failure only.
pub enum ReasonerParseSource {
    Marker(MarkerParseError),
}
```

> **Note.** `ReasonerResponseUnparseable` is constructed at the three lossy
> parser sites (`rustyclawd.rs`, `decide.rs`, `orient.rs`) on a marker-grammar
> failure. It carries the full raw text so diagnostics never lose the model's
> output.

* `raw` is the **complete, untruncated** model response text. Truncation to
  `MAX_RAW_LOG_BYTES = 8192` is applied only at log-format time by the shared
  `crate::util::log::truncate_for_log` helper.
* Every parse-failure log line embeds the full (truncated-for-log) text,
  rendered with the `{:?}` Debug format so control characters and ANSI escapes
  are escaped (defends against CRLF / log-injection in model output).

The caller (`dispatch_spawn_engineer`) logs this and falls back to the
deterministic skip outcome. The cycle never panics, never aborts.

## Construction Pattern

```rust
let act_reasoner: Box<dyn ActReasoner> = match build_act_reasoner() {
    Ok(r) => Box::new(r),
    Err(e) => {
        eprintln!("[ooda_reasoners] init failed: {e}; using deterministic fallback");
        Box::new(DeterministicFallbackActReasoner)
    }
};
dispatch_actions(actions, &mut state, act_reasoner.as_ref());
```

Constructed **once per cycle** in `ooda_loop/cycle.rs`; dropped at cycle end.
The reasoners are carried alongside memory and the peer clients in
[`OodaContext`](./brain-executive-api.md#oodacontext), whose
fields are `orient_reasoner`, `decide_reasoner`, and `act_reasoner`.

### Daemon wire-up and health log

`operator_commands_ooda/daemon/reasoners.rs` builds the
three reasoners with the loud-fallback discipline unchanged. When every reasoner
is LLM-backed the daemon logs:

```
[simard] OODA daemon: brain online — orient/decide/act reasoners LLM-backed (no fallback)
```

Per-reasoner lines name the reasoner and its implementation, e.g.
`decide_reasoner = RustyClawdDecideReasoner (prompt-driven)`. A fallback logs
the degraded line naming the phase and reason.

## Side-Effect Handler

The decision returned by the act reasoner is applied by a handler in a separate
module, not in `ooda_reasoners` itself:

```
src/ooda_actions/advance_goal/lifecycle.rs::apply_lifecycle_decision(
    &mut OodaState,
    &str,                          // goal_id
    EngineerLifecycleDecision,
    &engineer_worktree::LiveEngineer,
) -> ActionOutcome
```

Keeping the handler outside the reasoner preserves the reasoner's purity (input
context → decision) and lets state mutations live alongside the other
`advance_goal` actions.

## Module Layout

```
src/ooda_reasoners/
├── mod.rs            # traits, decision enum, error wiring
├── ctx.rs            # gather_engineer_lifecycle_ctx + redaction
├── rustyclawd.rs     # RustyClawd{Act,Decide,Orient}Reasoner + LlmSubmitter
├── decide.rs         # DecideReasoner trait, DecideJudgment,
│                     #   DeterministicFallbackDecideReasoner
├── recipe_reasoner.rs# RecipeReasoner: recipe-runner-rs shim + keyword scanner
├── fallback.rs       # DeterministicFallbackActReasoner
├── decide_tests.rs   # JSON round-trip + DeterministicFallback tests
└── tests.rs          # parse, ctx, integration tests
```

All files respect the per-module 400-LOC cap (#1266).

## See Also

* [The Brain](../architecture/brain-model.md) — the whole cognition these reasoners serve.
* [Concept: prompt-driven OODA reasoning](../concepts/prompt-driven-ooda-brain.md)
* [Reference: OODA Brain Decision Protocol](ooda-brain-decision-protocol.md)
* [Reference: RecipeReasoner API](recipe-brain-api.md)
* [Reference: text-parsing wire formats](text-parsing-wire-formats.md)
* [Reference: base type adapters](base-type-adapters.md)
* [Terminology migration](brain-terminology-migration.md)
