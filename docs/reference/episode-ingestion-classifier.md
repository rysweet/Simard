---
title: Episode ingestion classifier API
description: Rust API reference for the deterministic episode-ingestion classifier that guards every store_episode intake site — the classify pure function, the sanitize_transcript helper, the EventKind / EpisodeMetadata / IntakeContext / Decision types, the store_episode_classified IO seam, and the IntakeCounters metric. Documents the strict-priority decision rules, the metadata JSON contract, and the per-site wiring.
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
cannot convey. Every field is optional; `Default` yields an all-`None`
context.

```rust
#[derive(Clone, Debug, Default)]
pub struct IntakeContext {
    pub goal_id: Option<String>,
    pub cycle: Option<u32>,
    pub event_kind_hint: Option<EventKind>,
}
```

`event_kind_hint` lets a call site assert a known kind (e.g. the
goal-curator site hints `GoalArchival` when goals were force-removed). The
hint participates in the failure override and the meaningful-hint rule.

### `Decision`

The result of classification.

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Drop,                    // do not store; increment `dropped`
    Store(EpisodeMetadata),  // store with durable metadata
    DownScope(EpisodeMetadata), // store, but operational/low-importance
}
```

---

## Functions

### `classify`

```rust
pub fn classify(content: &str, source_label: &str, ctx: &IntakeContext) -> Decision;
```

Pure, no IO — the fully unit-testable core. Evaluates four rules in
**strict priority order** and returns on the first match:

1. **Failure override (highest).** If `content` contains any of `error`,
   `failed`, `failure`, `panic`, `exception` (case-insensitive) **or**
   `ctx.event_kind_hint ∈ {ActionFailure, RecipeFailure}`:
   `Store(EpisodeMetadata { importance: 0.9, event_kind: <hint or
   ActionFailure>, is_operational: false, goal_id, cycle })`.
   Overrides every rule below — a noisy line that records a failure is
   still kept.
2. **Known-noise markers → `Drop`.** Case-insensitive substring match on
   any of:
   `started with objective`, `completed and persisted`,
   `flushing working memory`, `continue_skipping`, `no decision keyword`,
   or both `hydrated ` and `prior-session facts`.
3. **Meaningful hint → `Store`.** If `ctx.event_kind_hint ∈ {Handoff,
   GoalArchival, GoalPromotion, UserDecision, ActionCompleted}`: `Store`
   with the importance from the table below and `is_operational: false`.
4. **Default → `DownScope`.** Anything unmatched is **stored
   down-scoped**: `DownScope(EpisodeMetadata { importance: 0.1,
   event_kind: Operational, is_operational: true, goal_id, cycle })`.
   Unrecognised events are never dropped — only de-prioritised.

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
    counters: &IntakeCounters,
) -> SimardResult<Option<String>>;
```

The IO seam every intake site calls instead of `bridge.store_episode`.

1. `classify(content, source_label, ctx)`.
2. On `Drop`: increment `counters.dropped`, return `Ok(None)` — never
   touches the bridge.
3. On `Store(meta)` / `DownScope(meta)`: serialize `meta` via
   `serde_json::to_value`, call
   `bridge.store_episode(content, source_label, Some(&value))`, increment
   `stored` or `downscoped`, and return `Ok(Some(episode_id))`.

The returned id is the same id used downstream for
`store_fact_with_provenance` / `store_procedure_with_provenance`.

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
    pub fn log_summary(&self); // eprintln! + tracing, per cycle
}
```

`log_summary` emits one aggregated line per cycle:

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
| `consolidation-intake` | `Hydrated N prior-session facts …` | Drop |
| working-memory slot (`slot.slot_type`) | `slot.content` | classify per content |
| `consolidation-persistence` | `Session … flushing working memory …` | Drop |
| `goal-curator` | active-goal board summary (metadata `{active_count, backlog_count, force_removed}`) | Store; `event_kind = goal_archival` when `force_removed > 0` |

---

## Examples

### A noise episode is dropped

```rust
let ctx = IntakeContext::default();
let d = classify("Session s1 started with objective: ship X", "session-intake", &ctx);
assert_eq!(d, Decision::Drop);
// store_episode_classified(...) returns Ok(None); store_episode never called.
```

### A meaningful episode is stored with metadata

```rust
let ctx = IntakeContext {
    goal_id: Some("improve-foo".into()),
    cycle: Some(42),
    event_kind_hint: Some(EventKind::ActionCompleted),
};
match classify("Refactored bridge; tests green", "act", &ctx) {
    Decision::Store(m) => {
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
    Decision::Store(m) => {
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
    Decision::DownScope(m) => {
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
