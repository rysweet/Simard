---
title: "Simard Whisperer API"
description: >
  Public surface of the Simard Whisperer: the Whisper intervention and urgency,
  the WhisperSink trait and its real/fake implementations, the LoopDetected /
  DriftCorrection problem kinds and signals, the WhisperGate dedup/rate-limit,
  the SIMARD_OVERSEER_WHISPER config flag, the extended OverseerTickReport,
  operator notification, and error variants.
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: reference
status: design — not yet implemented
related:
  - ../concepts/simard-whisperer.md
  - ../howto/configure-the-simard-whisperer.md
  - ../design/overseer.md
  - ../reference/meeting-handoff-schema.md
---

# Simard Whisperer API Reference

> **Status: design specification — not yet implemented (issue
> [#2605](https://github.com/rysweet/Simard/issues/2605), open).**
>
> This page specifies the **intended** public surface. None of the types,
> traits, or functions below exist in `src/` yet — `whisper_ops.rs` is not
> created, and no `Whisper` / `LoopDetected` / `DriftCorrection` symbols are
> present. Signatures marked *(proposed)* are the design target for the
> implementation PR, not a description of shipped code. The existing modules
> they extend (`intervention.rs`, `signal.rs`, `guardrails.rs`,
> `capabilities.rs`, `config.rs`, `wiring.rs`, `mod.rs`) are real; the additive
> members described here are not.

Module: `simard::overseer`
Primary source: `src/overseer/whisper_ops.rs` (new) with additive edits to
`intervention.rs`, `signal.rs`, `guardrails.rs`, `capabilities.rs`, `config.rs`,
`wiring.rs`, and `mod.rs`.

For the conceptual overview see
[The Simard Whisperer](../concepts/simard-whisperer.md). For operator configuration
see [Configure the Simard Whisperer](../howto/configure-the-simard-whisperer.md).

The Whisperer is **purely additive**: it introduces new enum variants, one new
module, and new report/notification fields. No existing type, function, or field is
renamed or removed, and every existing Overseer test keeps passing unchanged.

## Module layout

```
src/overseer/whisper_ops.rs      NEW: WhisperUrgency, WhisperRecord, WhisperSink,
                                 MeetingHandoffWhisperSink, FakeWhisperSink,
                                 compose_whisper_note, whisper_signature
src/overseer/intervention.rs     + Intervention::Whisper { note, urgency } + label()
src/overseer/signal.rs           + ProblemKind::{LoopDetected, DriftCorrection}
                                 + Signal::{LoopDetected, DriftCorrection}
                                 + signals_from arms (ignore overseer-authored)
src/overseer/guardrails.rs       + WhisperGate (dedup window + per-hour cap);
                                 + classify() arm; RecursionGuard reuse (fail-closed)
src/overseer/capabilities.rs     + ObservedState.{consecutive_no_action, drift_*}
                                 + ActOutcome::{Whispered, WhisperSuppressed}
src/overseer/config.rs           + SIMARD_OVERSEER_WHISPER + whisper_enabled_from()
src/overseer/wiring.rs           + OverseerTickReport.{whispers, whispers_suppressed}
                                 + assemble MeetingHandoffWhisperSink + notify
src/overseer/notify.rs           + OperatorNotification::whisper(...) (kind "whisper")
```

## `Intervention::Whisper`

```rust
// src/overseer/intervention.rs
pub enum Intervention {
    // ...existing variants...

    /// Inject a short, ADVISORY steering note onto Simard's meeting-handoff inbox,
    /// to be folded into her reasoners' Observe context at the start of her NEXT
    /// cycle. The lightweight default steering action.
    /// Capability: `WhisperSink::deliver`.
    Whisper { note: String, urgency: WhisperUrgency },
}
```

`label()` gains one arm:

```rust
Intervention::Whisper { .. } => "whisper",
```

The label `"whisper"` is the stable key used in gate messages, tracing, dedup, and
the tick report.

### `WhisperUrgency`

```rust
// src/overseer/whisper_ops.rs
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WhisperUrgency {
    Low,
    #[default]
    Normal,
    High,
}
```

`High` feeds the [escalation](#escalation) trigger. `Normal` is the default for
composed whispers.

### Risk classification

```rust
// src/overseer/guardrails.rs — classify()
Intervention::Whisper { .. } => RiskClass::Routine,
```

A whisper is **`Routine`**: it takes no action on Simard's behalf, spends no LLM
budget, and merely adds advisory context. It is admitted by the default
`AutonomyGate` without any HIGH-RISK or merge opt-in. (The gating that *does* apply to
whispers — dedup, rate-limit, identity — lives in [`WhisperGate`](#whispergate) and
`RecursionGuard`, not in the autonomy gate.)

A whisper is **not** cost-bearing: `is_cost_bearing(Intervention::Whisper { .. })`
is `false`, so it is never counted against the per-cycle launch cap or the
`BudgetGate`.

## Problem kinds and signals

### `ProblemKind`

```rust
// src/overseer/signal.rs
pub enum ProblemKind {
    // ...existing variants...
    /// Simard is looping: repeated no-action / no-progress on a goal.
    LoopDetected,
    /// Simard's recent work is drifting from the active goal's stated intent.
    DriftCorrection,
}
```

These are **explicit additive variants**, not overloads of `ProcessHealth` /
`GoalHygiene`, so Decide can route them independently and they are independently
testable.

### `Signal`

```rust
// src/overseer/signal.rs
pub enum Signal {
    // ...existing variants...

    /// Consecutive no-action cycles observed for `goal_id`
    /// (`ObservedState.consecutive_no_action`). Emitted at the whisper loop
    /// threshold (2), strictly below `NO_PROGRESS_BREAKER_THRESHOLD` (3).
    LoopDetected { goal_id: String, consecutive_no_action: u32 },

    /// Recent work diverging from the active goal's stated intent
    /// (`ObservedState.drift_detail`).
    DriftCorrection { goal_id: String, detail: String },
}
```

### `ObservedState` additions

```rust
// src/overseer/capabilities.rs — additive fields on ObservedState
pub struct ObservedState {
    // ...existing fields...

    /// Consecutive no-action cycles for the active goal, mirrored from Simard's
    /// no-progress breaker (`goal_curation::NoProgressBreaker::record_no_action`).
    /// `None` when unavailable.
    pub consecutive_no_action: Option<u32>,

    /// The active goal id the loop/drift readings pertain to. `None` when idle.
    pub active_goal_id: Option<String>,

    /// Non-empty when the Overseer detects recent work drifting from the active
    /// goal's stated intent; carries a short human-readable reason.
    pub drift_detail: Option<String>,
}
```

### `signals_from` behaviour

`signals_from(&ObservedState)` gains two arms (thresholds are illustrative env-tunable
defaults, mirroring the existing sketch constants):

```rust
// WHISPER_LOOP_THRESHOLD = 2  (< NO_PROGRESS_BREAKER_THRESHOLD = 3)
if let (Some(n), Some(goal)) = (state.consecutive_no_action, state.active_goal_id.clone())
    && n >= WHISPER_LOOP_THRESHOLD
{
    out.push(Signal::LoopDetected { goal_id: goal, consecutive_no_action: n });
}
if let (Some(detail), Some(goal)) = (state.drift_detail.clone(), state.active_goal_id.clone()) {
    out.push(Signal::DriftCorrection { goal_id: goal, detail });
}
```

!!! important "No self-whisper"
    The Observe adapter that fills `ObservedState` **ignores handoffs authored by the
    Overseer** (`author == overseer_author_login()` or
    `themes` contains `"overseer-whisper"`). The Overseer therefore never observes its
    own whisper and can never whisper about it.

### Orient / Decide routing

`classify_signal` maps the new signals:

```rust
Signal::LoopDetected { goal_id, consecutive_no_action } => (
    ProblemKind::LoopDetected,
    Priority::High,
    format!("loop:{goal_id}"),
    format!("no action for {consecutive_no_action} cycles on {goal_id}"),
),
Signal::DriftCorrection { goal_id, detail } => (
    ProblemKind::DriftCorrection,
    Priority::Normal,
    format!("drift:{goal_id}"),
    format!("work drifting from goal {goal_id}: {detail}"),
),
```

`decide` routes both problem kinds to a **whisper by default**, and to a **meeting
transfer on escalation**:

```rust
ProblemKind::LoopDetected | ProblemKind::DriftCorrection => {
    if escalate {                       // see “Escalation” below
        Intervention::TransferGoal { goal: /* GoalBrief from problem */ }
    } else {
        Intervention::Whisper {
            note: compose_whisper_note(problem, observed),
            urgency: /* Normal, or High when the condition is acute */,
        }
    }
}
```

## `WhisperSink`

The injectable delivery seam. Production writes a real meeting handoff; tests capture
records in memory.

```rust
// src/overseer/whisper_ops.rs

/// A composed whisper ready for delivery.
pub struct WhisperRecord {
    pub note: String,
    pub urgency: WhisperUrgency,
    pub problem: ProblemKind,
    pub goal_id: Option<String>,
    /// The Overseer's distinct steward login (overseer_author_login()).
    pub author: String,
    /// Stable dedup signature: (problem + goal_id + normalized note).
    pub signature: String,
}

/// Deliver a whisper by writing an ADVISORY meeting handoff onto the inbox that
/// Simard's OODA loop scans at cycle start.
pub trait WhisperSink {
    /// Build a `MeetingHandoff` with the note in a NON-PROMOTING field
    /// (`open_questions` / `themes`), `decisions` EMPTY, `action_items` EMPTY,
    /// `processed: false`, `themes` containing `"overseer-whisper"`, authored under
    /// `rec.author`; persist it via `write_meeting_handoff`. Returns the written path.
    fn deliver(&self, rec: &WhisperRecord) -> Result<PathBuf, OverseerError>;
}
```

### `MeetingHandoffWhisperSink` (production)

```rust
pub struct MeetingHandoffWhisperSink {
    dir: PathBuf,   // meeting_facilitator::default_handoff_dir()
}

impl MeetingHandoffWhisperSink {
    /// Construct against the default `<state_root>/meeting_handoffs/` inbox.
    pub fn from_env() -> Self;
    /// Construct against an explicit directory (used by wiring and tests).
    pub fn new(dir: PathBuf) -> Self;
}
```

`deliver` constructs the handoff and calls
`crate::meeting_facilitator::write_meeting_handoff(&self.dir, &handoff)`, producing a
`handoff-<rfc3339>.json`. It never touches the `.txt` `FileHandoffSink` path.

The written handoff is shaped so Simard's `check_meeting_handoffs` cannot promote it
into a goal or backlog item:

| Field | Value |
|---|---|
| `decisions` | `[]` (empty — cannot become a goal) |
| `action_items` | `[]` (empty — cannot become a backlog item) |
| `open_questions` | one entry carrying the whisper note (advisory context) |
| `themes` | `["overseer-whisper"]` (recognition tag) |
| `next_owner` | `Some("ooda-observe")` |
| `participants` | `[author]` (the Overseer's steward login) |
| `processed` | `false` |
| `topic` | `"overseer whisper"` |

See the [Meeting Handoff Schema](../reference/meeting-handoff-schema.md) for the full
struct.

### Ingestion contract (OODA side)

Delivery is only half of the channel; Simard's cycle-start ingest must **fold the
whisper note into the reasoner-facing Observe context** rather than drop it. This is a
small additive branch in the OODA handoff ingest (`check_meeting_handoffs` /
`observe()` in `src/ooda_loop/`):

- A handoff whose `themes` contains `"overseer-whisper"` is a **whisper**, recognised
  before the generic empty-handoff fast-mark path. (Today, a handoff with empty
  `decisions` **and** empty `action_items` is fast-marked `processed` and otherwise
  ignored — which would silently drop a whisper. The whisper branch runs first.)
- The whisper's note (`open_questions` / `themes`) is appended to the Observe context
  the reasoners read this cycle — as **advisory context only**.
- The handoff is then marked `processed: true`. Because it carries **no** `decisions`
  and **no** `action_items`, it never creates a goal, backlog item, or planned action.

This ordering guarantee is what makes the whisper both *visible to the reasoners* and
*incapable of fabricating work*. It is asserted by the integration test
([Verify](../howto/configure-the-simard-whisperer.md#verify)).

### `FakeWhisperSink` (test-only)

```rust
#[cfg(any(test, feature = "test-utils"))]
pub struct FakeWhisperSink {
    /// Every delivered record, in order.
    pub delivered: RefCell<Vec<WhisperRecord>>,
    /// When set, `deliver` returns this error instead of capturing (isolation test).
    pub fail_with: Option<OverseerError>,
    /// When true, `deliver` panics (panic-isolation test).
    pub panic_on_deliver: bool,
}
```

`FakeWhisperSink` lets tests assert *what* was whispered, *how many times*, and the
window/cap behaviour without any filesystem or network I/O. An
`InMemoryWhisperSink` variant that writes to a `tempfile::tempdir()` is used by the
integration test that proves the note reaches the next OODA cycle through a real
`handoff-*.json`.

### Note composer and signature

```rust
/// Compose a concise (1–2 sentence) corrective/additional instruction from the
/// problem and the observed state. Deterministic; no I/O.
pub fn compose_whisper_note(problem: &Problem, state: &ObservedState) -> String;

/// Stable dedup signature = hash of (problem kind + goal_id + normalized note).
/// `normalize` lowercases, trims, and collapses whitespace so trivially-different
/// renderings of the same whisper collapse to one signature.
pub fn whisper_signature(kind: ProblemKind, goal_id: Option<&str>, note: &str) -> String;
```

## `WhisperGate`

Dedup + rate-limit, in the style of `BudgetGate` / `ConflictSequencer`, with an
injected clock so it is unit-testable with a virtual clock.

```rust
// src/overseer/guardrails.rs
pub struct WhisperGate {
    /// Suppress an identical signature seen within this many seconds. Default 900.
    window_secs: u64,
    /// Max whispers admitted per rolling hour across all signatures. Default 5.
    cap_per_hour: usize,
    // internal: seen-signature ledger with timestamps; rolling per-hour counter
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhisperDecision {
    /// Admit and record the whisper for delivery.
    Deliver,
    /// Suppress: identical signature seen within the window.
    SuppressDuplicate,
    /// Suppress: the per-hour cap is reached.
    SuppressCapReached,
}

impl WhisperGate {
    pub fn new(window_secs: u64, cap_per_hour: usize) -> Self;
    /// Defaults: 900 s window, 5/hour cap.
    pub fn from_env() -> Self;
    /// Decide whether to admit `signature` at `now_secs`. Records the timestamp on
    /// `Deliver`. Pure w.r.t. the injected clock — no wall-clock reads.
    pub fn admit(&mut self, signature: &str, now_secs: u64) -> WhisperDecision;
}
```

Both knobs are env-tunable: `SIMARD_OVERSEER_WHISPER_WINDOW_SECS` and
`SIMARD_OVERSEER_WHISPER_CAP_PER_HOUR` (see [Config](#configuration)).

### Fail-closed identity

The Whisperer reuses the existing `RecursionGuard`. Before any whisper is delivered,
the Overseer requires a configured steward identity:

```rust
// unconfigured identity ⇒ refuse; nothing delivered
if overseer_author_login_is_default_only_and_required_unset() {
    return Err(OverseerError::Recursion {
        subject: "unconfigured-identity: whisper".to_string(),
    });
}
```

In practice the whisper path calls `RecursionGuard::admit` with the whisper's author
subject; an unconfigured guard fails closed exactly as it does for PR/commit/goal
subjects (see `src/overseer/guardrails.rs`). A refused whisper never calls
`WhisperSink::deliver`.

## Escalation

Escalation reuses the pre-existing meeting path — `Intervention::TransferGoal` →
`MeetingHost::transfer_goal` — with **no change** to that capability. The Overseer
escalates instead of whispering when **either**:

- the same-signature whisper has been admitted **≥ N times** within the window
  (default **N = 3**, `SIMARD_OVERSEER_WHISPER_ESCALATE_AFTER`) without the condition
  clearing, **or**
- the composed whisper's `urgency == WhisperUrgency::High`.

Otherwise the Overseer emits the lightweight whisper. The default path is always the
whisper.

## `ActOutcome`

```rust
// src/overseer/mod.rs
pub enum ActOutcome {
    // ...existing variants...
    /// A whisper was delivered onto the inbox; carries the written path.
    Whispered { path: PathBuf },
    /// A whisper was suppressed by the WhisperGate (dedup or cap).
    WhisperSuppressed { reason: &'static str },
}
```

`act` dispatches `Intervention::Whisper` through the `WhisperGate` and, when admitted,
the `WhisperSink`:

```rust
Intervention::Whisper { note, urgency } => {
    let rec = /* WhisperRecord: author, signature, ... */;
    match self.whisper_gate.admit(&rec.signature, now_secs) {
        WhisperDecision::Deliver => {
            let path = self.caps.whisper.deliver(&rec)?;
            Ok(ActOutcome::Whispered { path })
        }
        WhisperDecision::SuppressDuplicate =>
            Ok(ActOutcome::WhisperSuppressed { reason: "duplicate" }),
        WhisperDecision::SuppressCapReached =>
            Ok(ActOutcome::WhisperSuppressed { reason: "cap" }),
    }
}
```

## `OverseerTickReport` additions

```rust
// src/overseer/wiring.rs
pub struct OverseerTickReport {
    // ...existing fields...
    /// Advisory whispers delivered onto Simard's steering inbox this tick.
    pub whispers: usize,
    /// Whispers suppressed by the WhisperGate (dedup/cap) this tick.
    pub whispers_suppressed: usize,
}
```

`tally_outcome` increments `whispers` on `ActOutcome::Whispered` and
`whispers_suppressed` on `ActOutcome::WhisperSuppressed`. The per-tick `tracing::info!`
event gains `whispers` and `whispers_suppressed` keys alongside the existing counters.

## Operator notification

```rust
// src/overseer/notify.rs
impl OperatorNotification {
    /// Build a whisper notification (kind "whisper"). `note` is the steering text,
    /// `trigger` names the condition (loop/drift), `urgency` the composed urgency.
    pub fn whisper(note: &str, trigger: &str, urgency: WhisperUrgency, goal_id: &str) -> Self;
}
```

Each **delivered** whisper is surfaced through the mandatory `DualChannelNotifier`
(`notify(&OperatorNotification)`), so whispers appear on the operator's channels and
the dashboard. Suppressed whispers are traced but not notified (to avoid channel
noise); they still increment `whispers_suppressed`.

## Structured tracing

Every whisper emits one event:

```rust
tracing::info!(
    target: "overseer::whisper",
    trigger,                 // "loop_detected" | "drift_correction"
    note,                    // the composed steering text
    urgency = ?urgency,      // Low | Normal | High
    signature,               // dedup signature
    delivered,               // bool
    suppressed,              // "" | "duplicate" | "cap"
    path,                    // written handoff path, when delivered
    "overseer whisper"
);
```

No `println!` / `eprintln!` is added; operator-facing lines continue to use the
existing `"[simard] ..."` convention only where that convention already lives.

## Configuration

All Whisperer knobs are pure and env-**injectable** (`impl Fn(&str) -> Option<String>`),
mirroring the existing `config.rs` pattern (no `std::env` mutation in tests).

| Env var | Meaning | Default |
|---|---|---|
| `SIMARD_OVERSEER_WHISPER` | Enable/disable the Whisperer (opt-out). | enabled when Overseer enabled |
| `SIMARD_OVERSEER_WHISPER_WINDOW_SECS` | Dedup window for identical whispers. | `900` |
| `SIMARD_OVERSEER_WHISPER_CAP_PER_HOUR` | Max whispers per rolling hour. | `5` |
| `SIMARD_OVERSEER_WHISPER_ESCALATE_AFTER` | Same-signature whispers before escalating to a meeting. | `3` |

```rust
// src/overseer/config.rs
pub const SIMARD_OVERSEER_WHISPER_ENV: &str = "SIMARD_OVERSEER_WHISPER";

/// Opt-out gate: enabled unless an explicit falsey value is set, AND only when the
/// Overseer itself is enabled. Mirrors `overseer_acting_enabled_from`.
pub fn whisper_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    if !overseer_acting_enabled_from(&lookup) {
        return false; // whispering only makes sense when the Overseer runs
    }
    !matches!(
        lookup(SIMARD_OVERSEER_WHISPER_ENV).as_deref().map(str::trim),
        Some(v) if is_falsey(v)
    )
}

/// Production entry point: read the real process environment.
pub fn whisper_enabled() -> bool {
    whisper_enabled_from(|k| std::env::var(k).ok())
}
```

Truthy/falsey recognition reuses the existing `is_truthy` / `is_falsey` helpers, so
`0`/`false`/`no`/`off` disable and everything else (unset, empty, `1`, `true`, `yes`,
`on`, or garbage) leaves it enabled — provided the Overseer is enabled.

## Error variants

The Whisperer reuses the existing `OverseerError` enum
(`src/overseer/capabilities.rs`); no new error type is introduced.

| Variant | When |
|---|---|
| `OverseerError::Recursion { subject }` | Whisper refused: unconfigured steward identity (fail-closed), or the subject is the Overseer's own work. |
| `OverseerError::Capability { what, detail }` | `write_meeting_handoff` failed (e.g. I/O error) while delivering the whisper. |

A whisper never produces `Gated`, `Budget`, or `Conflict` errors: it is `Routine` and
non-cost-bearing. Suppression is **not** an error — it is an `ActOutcome`, counted and
traced.

## Test surface

Test-only re-exports from `overseer::whisper_ops`:

```rust
#[cfg(any(test, feature = "test-utils"))]
pub use whisper_ops::{FakeWhisperSink, InMemoryWhisperSink};
```

The Whisperer is fully testable with injected fakes (observed-state, whisper sink,
clock, identity) and **no network**. See
[Configure the Simard Whisperer › Verify](../howto/configure-the-simard-whisperer.md#verify)
for the end-to-end scenarios.

## See also

- Concept: [The Simard Whisperer](../concepts/simard-whisperer.md)
- How-to: [Configure the Simard Whisperer](../howto/configure-the-simard-whisperer.md)
- Design: [Overseer — operator/observer co-process](../design/overseer.md)
- Reference: [Meeting Handoff Schema](../reference/meeting-handoff-schema.md)
