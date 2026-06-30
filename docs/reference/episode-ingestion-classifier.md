---
title: Episode ingestion classifier API
description: Rust API reference for the deterministic episode-ingestion classifier that guards every store_episode intake site — the classify pure function, the sanitize_transcript helper, the EventKind / EpisodeMetadata / IntakeContext / IntakeDecision types, the store_episode_classified IO seam, and the IntakeCounters metric. Documents the strict-priority decision rules, the metadata JSON contract, and the per-site wiring.
last_updated: 2026-06-20
owner: simard
doc_type: reference
related:
  - ../architecture/episode-ingestion-policy.md
  - ./automatic-distillation-scheduler.md
  - ../architecture/episode-distillation.md
  - ../memory.md
  - ../howto/configure-episode-hygiene-and-promotion.md
---

# Episode ingestion classifier API

> Shipped in issue [#2327](https://github.com/rysweet/Simard/issues/2327).
> Module: `src/memory_consolidation/classifier.rs`
> (`mod classifier;` in `src/memory_consolidation/mod.rs`).

The classifier is the deterministic policy that runs before every
`store_episode` intake site. It is a **pure decision function**
(`classify`) plus a thin IO seam (`store_episode_classified`) that
performs the store and bumps counters. This page is the executable
contract; for the rationale see
[Episode ingestion policy & automatic promotion](../architecture/episode-ingestion-policy.md).

---

## Types

### `EventKind`

Coarse taxonomy of *why* an episode is being stored. Serializes as
`snake_case`, so the string lands in episode metadata exactly as the
taxonomy enumerates it.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    ActionFailure,   // "action_failure"
    ActionCompleted, // "action_completed"
    Handoff,         // "handoff"
    GoalArchival,    // "goal_archival"
    GoalPromotion,   // "goal_promotion"
    UserDecision,    // "user_decision"
    RecipeFailure,   // "recipe_failure"
    Operational,     // "operational"  (down-scope bucket)
}
```

### `EpisodeMetadata`

Structured metadata attached to every **stored** and **down-scoped**
episode. Serialized to JSON and passed as the `metadata` argument of
`store_episode`.

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeMetadata {
    pub importance: f64,          // 0.0..=1.0
    pub event_kind: EventKind,
    pub goal_id: Option<String>,
    pub cycle: Option<u32>,
    pub is_operational: bool,
}
```

### `IntakeContext`

Caller-supplied context that the `content` / `source_label` strings alone
cannot convey. Both fields are optional; `Default` yields an all-`None`
context.

```rust
#[derive(Clone, Debug, Default)]
pub struct IntakeContext {
    pub goal_id: Option<String>,
    pub cycle: Option<u32>,
}
```

`goal_id` and `cycle` are threaded straight into the stored metadata so a
distilled fact/procedure can be traced back to the goal and cycle that
produced its source episode.

### `IntakeDecision`

The result of classification.

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum IntakeDecision {
    Drop,                       // do not store; increment `dropped`
    DownScope(EpisodeMetadata), // store, but operational/low-importance
    Store(EpisodeMetadata),     // store with durable metadata
}
```

Helper predicates make call sites and tests read cleanly:

```rust
impl IntakeDecision {
    pub fn is_dropped(&self) -> bool;
    pub fn is_store(&self) -> bool;
    pub fn is_downscoped(&self) -> bool;
    pub fn metadata(&self) -> Option<&EpisodeMetadata>; // None for Drop
}
```

---

## Functions

### `classify`

```rust
pub fn classify(content: &str, source_label: &str, ctx: &IntakeContext) -> IntakeDecision;
```

Pure, no IO — the fully unit-testable core. Classification is **content- and
source-driven** (call sites do not have to pre-tag every event). Evaluates
four rules in **strict priority order** and returns on the first match:

1. **Failure override (highest).** If `content` contains any of `error`,
   `failed`, `failure`, `panic`, `exception` (case-insensitive):
   `Store(EpisodeMetadata { importance: 0.9, event_kind, is_operational:
   false, goal_id, cycle })`, where `event_kind` is `RecipeFailure` when the
   content/source mentions a recipe and `ActionFailure` otherwise. Overrides
   every rule below — a noisy line that records a failure is
   still kept.
2. **Known-noise markers → `Drop`.** Case-insensitive substring match on
   any of:
   `started with objective`, `completed and persisted`,
   `flushing working memory`, `continue_skipping`, `no decision keyword`.
3. **Meaningful content → `Store`.** Content/source matching a durable
   episodic event, stored at the importance from the table below with
   `is_operational: false`:
   - user decisions (`user decided` / `user decision`, or the
     `user-decision` source) → `UserDecision`;
   - goal-board promotions (`promoted goal` / `from backlog to active`) →
     `GoalPromotion`, archival (`archived goal` / `goal archival`) →
     `GoalArchival`;
   - handoffs (`handoff` in content or source) → `Handoff`;
   - durable completions (`opened pr` / `pull request` / `merged`) →
     `ActionCompleted`;
   - any `goal-curator`-sourced board summary → `GoalArchival`.
4. **Default → `DownScope`.** Anything unmatched — including cross-session
   hydration bookkeeping (`Hydrated N prior-session facts …` from the
   `consolidation-intake` site) — is **stored down-scoped**:
   `DownScope(EpisodeMetadata { importance: 0.1, event_kind: Operational,
   is_operational: true, goal_id, cycle })`. Unrecognised events are never
   dropped — only de-prioritised.

**Importance table:**

| `event_kind` | importance | `is_operational` |
|---|---:|:---:|
| `action_failure` / `recipe_failure` | 0.90 | false |
| `user_decision` | 0.85 | false |
| `handoff` / `goal_promotion` / `goal_archival` | 0.80 | false |
| `action_completed` | 0.70 | false |
| `operational` (down-scope) | 0.10 | true |
| *(dropped)* | — not stored — | — |

### `sanitize_transcript`

```rust
pub fn sanitize_transcript(transcript: &str) -> Option<String>;
```

Strips noise lines from a concatenated reflection transcript so the
transcript episode can still be stored (its id is needed for fact
provenance) without carrying `continue_skipping` chatter.

- Splits on `\n`; drops lines containing `continue_skipping` or
  `no decision keyword`.
- If the **original** transcript contains a failure signal (the rule-1
  token set), returns the original text **unchanged** — a transcript that
  records a failure is never stripped.
- Otherwise returns the joined survivors.
- Returns `None` when the survivor set is empty or whitespace-only (pure
  noise, no failure) — the caller then drops the episode unless it still
  needs the id for provenance.

### `store_episode_classified`

```rust
pub fn store_episode_classified(
    bridge: &dyn CognitiveMemoryOps,
    content: &str,
    source_label: &str,
    ctx: &IntakeContext,
) -> SimardResult<Option<String>>;
```

The IO seam every intake site calls instead of `bridge.store_episode`.

1. `classify(content, source_label, ctx)`.
2. Records the decision into the process-global [`IntakeCounters`]
   (`global_intake_counters()`).
3. On `Drop`: returns `Ok(None)` — never touches the bridge.
4. On `Store(meta)` / `DownScope(meta)`: serializes `meta` via
   `EpisodeMetadata::to_json`, calls
   `bridge.store_episode(content, source_label, Some(&value))`, and returns
   `Ok(Some(episode_id))`.

The returned id is the same id used downstream for
`store_fact_with_provenance` / `store_procedure_with_provenance`.

A companion seam, `store_episode_for_provenance(bridge, content, source_label,
ctx) -> SimardResult<String>`, ALWAYS stores and returns an id (a `Drop`
decision is promoted to a down-scoped store) — used by the reflection site so
the transcript episode id is available as the provenance anchor for the facts
it derives, even when the transcript would otherwise be dropped as noise.

> **Boundary observability.** `CognitiveEpisode` carries **no** metadata
> field — metadata passed to `store_episode` is write-only (folded into
> the library node, never round-tripped through
> `list_undistilled_episodes`). Classifier behaviour is therefore
> asserted at the **`store_episode` call boundary** (the test mock records
> the `(content, label, metadata)` triple), not by reading episodes back.
> "Down-scope" is observable only as `is_operational = true,
> importance = 0.1` on that recorded call.

### `IntakeCounters`

```rust
#[derive(Default)]
pub struct IntakeCounters { /* AtomicU32: dropped, stored, downscoped */ }

impl IntakeCounters {
    pub fn record(&self, decision: &IntakeDecision);
    pub fn snapshot(&self) -> (u32, u32, u32); // (dropped, stored, downscoped)
    pub fn log_summary(&self); // eprintln! + tracing, per cycle; then resets
}

/// Process-global counters shared by every intake chokepoint within a cycle.
pub fn global_intake_counters() -> &'static IntakeCounters;
```

The OODA cycle calls `global_intake_counters().log_summary()` once at the end
of each cycle, which emits one aggregated line and resets the counters:

```
[simard] episode-intake dropped=7 stored=3 downscoped=2
```

---

## Metadata JSON contract

`store_episode_classified` writes this exact shape as the `metadata`
argument:

```json
{
  "importance": 0.9,
  "event_kind": "action_failure",
  "goal_id": "improve-foo",
  "cycle": 42,
  "is_operational": false
}
```

- `importance` — `f64` in `0.0..=1.0`, per the importance table.
- `event_kind` — the `snake_case` enum string.
- `goal_id`, `cycle` — threaded from `IntakeContext`; serialize to JSON
  `null` when `None`.
- `is_operational` — `true` only for down-scoped episodes.

`Drop` writes nothing.

---

## Intake site wiring

Each `store_episode` chokepoint routes through `store_episode_classified`:

| Site (source label) | Content | Expected decision |
|---|---|---|
| `session-intake` | `Session … started with objective …` | Drop (unless failure override) |
| `session-reflection` | cycle transcript | `sanitize_transcript` → Store/DownScope/Drop, id preserved when facts derived |
| `session-persistence` | `Session … completed and persisted` | Drop |
| `consolidation-intake` | `Hydrated N prior-session facts …` | DownScope (operational bookkeeping) |
| working-memory slot (`slot.slot_type`) | `slot.content` | classify per content |
| `consolidation-persistence` | `Session … flushing working memory …` | Drop |
| `goal-curator` | active-goal board summary (metadata `{active_count, backlog_count, force_removed}`) | Store; `event_kind = goal_archival` when `force_removed > 0` |

---

## Examples

### A noise episode is dropped

```rust
let ctx = IntakeContext::default();
let d = classify("Session s1 started with objective: ship X", "session-intake", &ctx);
assert_eq!(d, IntakeDecision::Drop);
// store_episode_classified(...) returns Ok(None); store_episode never called.
```

### A meaningful episode is stored with metadata

```rust
let ctx = IntakeContext {
    goal_id: Some("improve-foo".into()),
    cycle: Some(42),
};
match classify("act: opened PR #7 and merged it", "act-outcome", &ctx) {
    IntakeDecision::Store(m) => {
        assert_eq!(m.event_kind, EventKind::ActionCompleted);
        assert_eq!(m.importance, 0.70);
        assert_eq!(m.is_operational, false);
        assert_eq!(m.goal_id.as_deref(), Some("improve-foo"));
        assert_eq!(m.cycle, Some(42));
    }
    other => panic!("expected Store, got {other:?}"),
}
```

### A noisy line that records a failure is kept

```rust
let ctx = IntakeContext::default();
match classify("Session s1 completed and persisted — error: disk full", "session-persistence", &ctx) {
    IntakeDecision::Store(m) => {
        assert_eq!(m.importance, 0.9);          // failure override beats the drop marker
        assert_eq!(m.is_operational, false);
    }
    other => panic!("expected Store, got {other:?}"),
}
```

### Unrecognised content is down-scoped, never dropped

```rust
let ctx = IntakeContext::default();
match classify("some novel observation we did not anticipate", "obs", &ctx) {
    IntakeDecision::DownScope(m) => {
        assert_eq!(m.importance, 0.1);
        assert!(m.is_operational);
        assert_eq!(m.event_kind, EventKind::Operational);
    }
    other => panic!("expected DownScope, got {other:?}"),
}
```

### Transcript sanitization

```rust
let t = "ran tests: 12 passed\n\
         brain: continue_skipping (no decision keyword in transcript)\n\
         handoff recorded for goal improve-foo";
let cleaned = sanitize_transcript(t).unwrap();
assert!(!cleaned.contains("continue_skipping"));
assert!(cleaned.contains("ran tests"));
assert!(cleaned.contains("handoff recorded"));

assert_eq!(sanitize_transcript("brain: continue_skipping (no decision keyword)"), None);

// failure transcripts are kept whole
let f = "step 1 ok\npanic: index out of bounds\ncontinue_skipping (no decision keyword)";
assert_eq!(sanitize_transcript(f).as_deref(), Some(f)); // unchanged
```

---

## Related

- [Episode ingestion policy & automatic promotion](../architecture/episode-ingestion-policy.md) —
  design rationale and the two-brick split
- [Automatic distillation scheduler API](./automatic-distillation-scheduler.md) —
  the promotion half
- [Episode distillation](../architecture/episode-distillation.md) — the
  fact-extraction pipeline
- [Configure episode hygiene and promotion](../howto/configure-episode-hygiene-and-promotion.md) —
  operator tuning and observability
