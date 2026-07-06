---
title: Overseer memory-recall API
description: >
  The Overseer's read/write access to Simard's cognitive memory graph as part of its
  observe/orient loop. Covers the MemoryRecall capability trait and its result types
  (RecallKeys, RecallBudget, MemorySnapshot, ObservationEpisode), the MemoryRecallOps
  adapter over the daemon's shared CognitiveMemoryOps handle (single-source, no second
  store), the additive ObservedState.{recall, recall_error} fields, the
  Signal::RecurringSignature vocabulary, the deliberate de-duplicated episode write-back,
  the SIMARD_OVERSEER_MEMORY_RECALL opt-out flag, the additive OverseerTickReport
  counters (memory_recalls / memory_writes / memory_errors), the no-silent-fallback error
  contract, and the untrusted-input security model for recalled content.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../design/overseer.md
  - ./overseer-activity-feed.md
  - ../howto/configure-overseer-memory-recall.md
  - ../howto/watch-overseer-activity.md
  - ../memory.md
  - ./status-snapshot-api.md
  - ./stewardship-api.md
---

# Overseer memory-recall API

The acting **Overseer** runs its own meta-OODA loop alongside Simard's engineer
loop (see the [Overseer design](../design/overseer.md)). Before this feature the
Overseer only saw a **count** of cognitive-memory nodes
(`ObservedState.memory_nodes`) — it never read the graph's *content*. It could
tell you memory was growing, but not *what Simard had already learned* about a
problem it was looking at.

**Memory recall** gives the Overseer bounded **read** access to Simard's
cognitive memory graph, and one deliberate, de-duplicated **write** back into it,
as a first-class part of Observe/Orient:

- In **Observe**, after it derives the cycle's `Signal`s/`Problem`s, the Overseer
  recalls the **semantic** facts, **episodic** memories, **procedural**
  know-how, and **prospective** triggers/ideas that are relevant to those
  signals — so it can recall prior similar problems and their outcomes.
- In **Orient**, recalled episodes that share a failure signature raise a new
  structural [`Signal::RecurringSignature`](#recurring-signature-detection),
  which promotes the problem's priority and surfaces the prior procedure instead
  of relying only on in-process counters.
- After the cycle, the Overseer **writes its own observation back** as an
  episodic memory (de-duplicated), so its stewardship activity becomes part of
  the graph the rest of Simard can recall.

The whole path is **best-effort and explicit**: recall is count-bounded and runs
on the panic-isolated tick thread, so it can never stall or crash the loop; and
if memory is unreachable the error is **surfaced** (typed error + telemetry
counter + `tracing::warn!`) — **never** silently swallowed into an empty result.

> **Single source of truth.** The Overseer reuses the **same**
> `Arc<dyn CognitiveMemoryOps>` handle the daemon already shares with the OODA
> loop, the memory-IPC server, and consolidation. It never opens a second
> cognitive store.

> **Modules (all touched files):**
>
> - `src/overseer/capabilities.rs` — capability seam + result/input types
>   (`MemoryRecall`, `RecallKeys`, `RecallBudget`, `MemorySnapshot`,
>   `RecalledFact` / `RecalledEpisode` / `RecalledProcedure` / `RecalledProspective`,
>   `ObservationEpisode`, `RecordOutcome`) **and** the `sanitize_recalled` egress
>   helper.
> - `src/overseer/wiring.rs` — the `MemoryRecallOps` adapter + the three additive
>   `OverseerTickReport` counters.
> - `src/overseer/mod.rs` + `src/overseer/signal.rs` — two-phase Observe (the
>   whole-pass recall), the de-duplicated write-back, and `RecurringSignature`
>   orientation; also the **single admission boundary** where recalled procedure
>   text is `sanitize_recalled`-cleaned before being folded into an advisory
>   `Problem.summary`.
> - `src/overseer/whisper_ops.rs` + `src/overseer/notify.rs` — the advisory-text
>   **egress** surfaces (`compose_whisper_note` → `OperatorNotification` →
>   email/Signal). They are **unchanged**: they only ever render a `Problem.summary`
>   that was already sanitized upstream (see
>   [security model](#security-model-recalled-content-is-untrusted-input)).
> - `src/overseer/config.rs` — the opt-out flag.
> - `src/operator_commands_ooda/daemon/mod.rs` — daemon plumbing + counter logging.
> - Incidental additive touches required by the new `Signal`/`ObservedState`
>   surface: `src/overseer/observer.rs` (the exhaustive `signal_kind_label` arm
>   for `RecurringSignature`), `src/overseer/sensor.rs` (the `recall`/`recall_error`
>   fields default to `None` in the read-only snapshot projection), and
>   `src/overseer/activity.rs` (the human-readable feed line surfaces recorded
>   memory notes).
>
> Underlying graph queries are the already-shipped
> [`CognitiveMemoryOps`](../memory.md) methods — **no new memory library API and no
> `amplihack-memory-lib` pin bump** (guideline G2 not triggered).

## Where recall sits in the meta-OODA loop

```
Observe ─┬─ snapshot StatusSnapshot → ObservedState        (unchanged)
         ├─ signals_from(&state)      → pre-recall Signals  (unchanged)
         ├─ RecallKeys::from_signals(&signals, &problems)   (NEW: derive keys)
         ├─ MemoryRecall::recall_* ×4 → one MemorySnapshot  (NEW: whole-pass recall)
         │      ├─ all Ok  → store snapshot on .recall; memory_recalls++
         │      └─ any Err → discard ALL reads; .recall=None;
         │                   .recall_error=Some(..); warn!; memory_errors++
         │                   (fail-closed — never a partial snapshot)
         └─ (a snapshot is stored ONLY on the all-Ok path)  (NEW)

Orient ──── signals_from + snapshot   → adds RecurringSignature
                                        when ≥2 recalled episodes
                                        share a failure signature   (NEW)

Decide ──── RecurringSignature → Priority::High advisory Problem,
            surfacing the recalled procedure; still passes the existing
            dedup / WhisperGate / priority / autonomy gates           (NEW)

Act ─────── (unchanged interventions) …

After tick ─ MemoryRecall::record_observation(&ObservationEpisode)
             de-duplicated via a WhisperGate-pattern gate (900s)      (NEW)
```

Recall is **additive**: it never removes or changes an existing signal, problem,
or intervention. With recall disabled (or unreachable) the Overseer behaves
exactly as it did before — every prior gate and decision path is unchanged.

## Capability trait: `MemoryRecall`

A single additive capability trait on the Overseer's capability seam
(`src/overseer/capabilities.rs`). Like every other Overseer capability, an
implementation is a **thin adapter** over an already-shipped Simard function —
never a reimplementation of memory logic.

```rust
/// Bounded read access to Simard's cognitive memory graph, plus one deliberate,
/// de-duplicated episodic write-back. Every read method is fail-closed: an
/// underlying memory error is returned as `OverseerError::Capability`, never
/// collapsed into an empty result (that would be a silent fallback). Each read
/// takes a single-kind `limit` (the caller passes the matching field of
/// `RecallBudget`) so no method ever sees budget fields it does not use.
pub trait MemoryRecall: Send + Sync {
    /// Recall up to `limit` semantic facts relevant to `keys` (concepts, prior
    /// root-causes).
    fn recall_semantic(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledFact>, OverseerError>;

    /// Recall up to `limit` episodic memories relevant to `keys` (prior
    /// occurrences of a problem and their outcomes). Carries each episode's
    /// failure signature so Orient can detect a recurring signature.
    fn recall_episodic(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledEpisode>, OverseerError>;

    /// Recall up to `limit` procedural runbooks relevant to `keys`. Surfaced by
    /// Decide when a recurring signature is seen.
    fn recall_procedural(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledProcedure>, OverseerError>;

    /// Recall up to `limit` prospective memories / ideas whose triggers match
    /// `keys` (deferred intentions the current situation should re-surface).
    fn recall_prospective(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledProspective>, OverseerError>;

    /// Write the Overseer's own observation back as one episodic memory,
    /// de-duplicated within a fixed window. Returns whether it was stored or
    /// suppressed as a duplicate.
    fn record_observation(
        &self,
        episode: &ObservationEpisode,
    ) -> Result<RecordOutcome, OverseerError>;
}
```

### Concrete adapter: `MemoryRecallOps`

`MemoryRecallOps` is the production implementation, constructed in
`wiring.rs::assemble_capabilities` from the **same** shared
`Arc<dyn CognitiveMemoryOps>` the daemon already holds. It maps each trait method
onto an existing memory query:

| `MemoryRecall` method   | Underlying `CognitiveMemoryOps` call                                  |
| ----------------------- | --------------------------------------------------------------------- |
| `recall_semantic`       | [`recall_facts_ranked`](../memory.md) (falls back to `search_facts`)  |
| `recall_episodic`       | [`recall_episodes_ranked`](../memory.md) (keyword-ranked)             |
| `recall_procedural`     | [`recall_procedure`](../memory.md)                                    |
| `recall_prospective`    | [`check_triggers`](../memory.md) / `list_all_prospective`             |
| `record_observation`    | [`store_episode`](../memory.md) with a fixed `source_label`           |

The adapter unpacks each `RecallBudget` field into the matching method's `limit`
(so a call only ever sees the one cap it needs) and maps every underlying `Err`
to `OverseerError::Capability`. Because the underlying
[`check_triggers`](../memory.md) takes a single `content: &str`,
`recall_prospective` joins `keys` into one deterministic, order-stable probe
string before calling it.

The adapter is where the `OverseerError::Capability` mapping and the hard budgets
live. It never chooses the write-back's provenance from caller data: every
write-back uses a **fixed** `source_label = "overseer"` and typed `json!`
metadata carrying only the observation's signature.

## Result & input types

All types live in `src/overseer/capabilities.rs` and use **owned `String`s** so
signal derivation stays pure and self-contained.

### `RecallKeys`

The keyword sets the Overseer recalls against, derived from the cycle's Signals
and Problems (never a full-graph scan):

```rust
pub struct RecallKeys {
    /// Free-text keywords built from the detected Signals/Problems
    /// (e.g. "distill_fail", "restart_churn", a blocked goal id).
    pub keywords: Vec<String>,
    /// Stable `failure_signature`-style keys, one per Problem, used both to
    /// query episodes and to detect a recurring signature on recall.
    pub signatures: Vec<String>,
}
```

`RecallKeys::from_signals(signals, problems)` builds the keys; it mirrors
`crate::stewardship::failure_signature` semantics so the recall key and the
stewardship dedup key line up.

### `RecallBudget`

Hard, constant per-kind caps enforced by the adapter so recall can never fan out
into an unbounded read:

```rust
pub struct RecallBudget {
    pub semantic: u32,    // default 5
    pub episodic: u32,    // default 5
    pub procedural: u32,  // default 3
    pub prospective: u32, // default 5
}
```

`RecallBudget::default()` is `5 / 5 / 3 / 5`. Budgets are constants, not env
knobs — bounding result **size** (plus panic isolation) is what keeps recall
non-blocking; there is no timeout layer because the calls are in-process against
the shared store. The Observe step passes each field as the matching method's
`limit` argument — the trait methods take a bare `u32`, never the whole struct.

### `MemorySnapshot`

The bundle of recalled results, stored on `ObservedState.recall` and consumed by
`signals_from`/Orient:

```rust
pub struct MemorySnapshot {
    pub facts: Vec<RecalledFact>,
    pub episodes: Vec<RecalledEpisode>,
    pub procedures: Vec<RecalledProcedure>,
    pub prospectives: Vec<RecalledProspective>,
}
```

Each `Recalled*` is a flattened, owned projection of the corresponding
`Cognitive*` node (content/summary, ids, and — for episodes — the parsed
`failure_signature`). An **empty** snapshot means "the graph had nothing
relevant" and is a valid, successful result; it is **distinct** from a recall
**error** (see [error contract](#error-contract-no-silent-fallback)).

### `RecalledFact` / `RecalledEpisode` / `RecalledProcedure` / `RecalledProspective`

The load-bearing projections the adapter returns and `MemorySnapshot` bundles.
Each is a flattened, **owned** view of the matching `Cognitive*` node — only the
fields signal derivation and egress actually need, so recalled content never
drags graph internals into the loop. All free-text fields are **untrusted input**
(see [security model](#security-model-recalled-content-is-untrusted-input)):

```rust
pub struct RecalledFact {
    pub id: String,
    pub content: String,   // concept / prior root-cause — untrusted text
    pub score: f32,        // ranking score from `recall_facts_ranked`
}

pub struct RecalledEpisode {
    pub id: String,
    pub summary: String,   // untrusted text
    /// Parsed `failure_signature` — the LOAD-BEARING key Orient counts to raise a
    /// `RecurringSignature`. `None` when the episode carried no signature.
    pub failure_signature: Option<String>,
    pub score: f32,
}

pub struct RecalledProcedure {
    pub id: String,
    pub content: String,   // stored runbook text — untrusted; advisory-only egress
}

pub struct RecalledProspective {
    pub id: String,
    pub content: String,   // deferred-intention text — untrusted
}
```

Only `RecalledEpisode.failure_signature` drives a decision (the recurring-signature
count); every `content`/`summary` field is treated purely as data and is
`sanitize_recalled`-cleaned before it can reach any egress surface.

### `ObservationEpisode` & `RecordOutcome`

The deliberate write-back payload and its result:

```rust
pub struct ObservationEpisode {
    /// Human-readable one-line summary of what the Overseer observed/decided.
    pub content: String,
    /// The failure/observation signature this episode is keyed on — also the
    /// de-dup key for the write-back gate.
    pub signature: String,
}

pub enum RecordOutcome {
    /// A new episode was written; carries the new node id.
    Stored { node_id: String },
    /// An identical-signature observation was written within the dedup window;
    /// nothing was persisted this tick.
    Deduplicated,
}
```

## `ObservedState` additions

Two additive, serde-defaulted fields carry recall through the existing pipeline
(they sit next to the pre-existing `memory_nodes` count, which is unchanged):

```rust
pub struct ObservedState {
    // … existing fields, including:
    pub memory_nodes: Option<u64>,     // unchanged: node COUNT only

    /// The bounded recall for this Observe pass. `None` when recall is disabled
    /// or has not run; `Some(empty)` when the graph had nothing relevant.
    pub recall: Option<MemorySnapshot>,

    /// Set to the surfaced error string when recall FAILED this pass. Kept
    /// separate from `recall` so an empty graph is never confused with a
    /// swallowed error (no silent fallback).
    pub recall_error: Option<String>,
}
```

## Recurring-signature detection

Recall promotes problem detection from **in-process counters** to **the memory
graph**. When ≥2 recalled episodes share a problem's `failure_signature`, Orient
emits a new structural signal:

```rust
pub enum Signal {
    // … existing variants …

    /// ≥2 recalled episodes share a failure signature: this problem has happened
    /// before. Raises priority and surfaces the prior procedure. Derived from
    /// `ObservedState.recall.episodes`, keyed on `failure_signature`.
    RecurringSignature {
        signature: String,
        occurrences: u32,
    },
}
```

`signals_from` gains a branch that reads `state.recall` and pushes
`RecurringSignature` when the threshold is met. Decide keys on the **structure**
(signature + count) — never on recalled free text as an instruction — and:

- raises the matching `Problem`'s priority to `Priority::High`, and
- surfaces the recalled `RecalledProcedure` (if any) as advisory context, folded
  into the `Problem.summary` **only** after passing through
  `capabilities::sanitize_recalled` (this is the admission boundary described in
  the [security model](#security-model-recalled-content-is-untrusted-input)).

The resulting advisory `Problem` still passes **every** existing gate (dedup /
`WhisperGate` / priority / autonomy). Recall can *inform* a decision; it can
never *command* a merge, deploy, or escalation on its own.

## Deliberate, de-duplicated write-back

After the tick, the Overseer records one episodic memory of what it observed via
`record_observation`. To avoid flooding the graph with per-tick duplicates, the
write-back is gated by a **reused `WhisperGate`-pattern** gate keyed by the
observation `signature` with a 900-second dedup window (the same primitive the
whisperer uses, `WhisperGate::new(900, 5)`):

- First observation of a signature in the window → `store_episode` →
  `RecordOutcome::Stored` → `memory_writes += 1`.
- A repeat of the same signature within the window → `RecordOutcome::Deduplicated`
  → nothing persisted, `memory_writes` unchanged.

Provenance is fixed: `source_label = "overseer"` (never caller-chosen) and typed
`json!` metadata carrying only the signature — no secrets, tokens, or env.

## Error contract (no silent fallback)

Memory recall is **fail-closed and loud**. A recall or write that returns `Err`
from the shared handle is:

1. mapped to `OverseerError::Capability { what: "memory-recall", detail }`,
2. surfaced on `ObservedState.recall_error` (recall) with `ObservedState.recall`
   left `None` — the snapshot is **not** replaced by an empty *or partial*
   `MemorySnapshot`,
3. logged with `tracing::warn!` (bounded, sanitized — never raw recalled content
   or secrets), and
4. counted in `OverseerTickReport.memory_errors`.

**Whole-pass granularity.** `ObservedState.recall_error` is a single
`Option<String>` by design: the four sub-reads run as **one atomic recall pass**.
If any one of them errors, the **entire** pass fails closed — the three
successful reads are **discarded**, `recall` stays `None`, `recall_error` is set,
`memory_errors += 1`, and `memory_recalls` is **not** incremented. The Overseer
never orients on a partially-recalled (silently-truncated) view of memory: for a
given tick it either holds the whole snapshot or none of it. Consequently a
recall **pass** either increments `memory_recalls` or contributes to
`memory_errors` — never both. (A separate write-back failure may still add to
`memory_errors` in the same tick.)

The tick then **continues** to completion — surfacing the error never aborts the
loop, and swallowing it into an empty result is explicitly disallowed. An empty
graph (`Some(empty)` snapshot, no error) and an unreachable graph
(`recall = None`, `recall_error = Some(..)`, `memory_errors += 1`) are always
distinguishable by callers, the tick report, and tests.

## `OverseerTickReport` counters

Three additive `#[serde(default)]` counters make recall fully observable through
the existing [Overseer activity feed](./overseer-activity-feed.md) without any
`print!`/`eprintln!`:

| Field           | Meaning                                                              |
| --------------- | ------------------------------------------------------------------- |
| `memory_recalls`| **1** when this tick's whole recall pass completed (all four bounded sub-reads returned `Ok`); **0** otherwise. At most 1 per tick. |
| `memory_writes` | Episodic observations actually persisted (dedup suppressions excluded). |
| `memory_errors` | Surfaced failures this tick — a failed recall pass and/or a failed write-back (0, 1, or 2), never swallowed. |

They are emitted as `tracing` keys on the per-tick event and daemon-logged next
to the existing `problems=… issues_filed=…` line, so a `memory_errors` spike is
visible in the dashboard/TUI Overseer surfaces and in `simard status`.

## Configuration

One additive, opt-out flag, consistent with `SIMARD_OVERSEER_GOAL_HEALTH` /
`SIMARD_OVERSEER_WHISPER`:

| Env var                         | Default | Effect                                                       |
| ------------------------------- | ------- | ------------------------------------------------------------ |
| `SIMARD_OVERSEER_MEMORY_RECALL` | **on**  | Overseer reads/writes the memory graph in its loop.          |

Semantics (`config::memory_recall_enabled_from`, mirroring
`goal_health_enabled_from`):

- **Default ON** whenever the acting Overseer runs. Recall is enabled unless the
  var is an explicit **falsey** value (`0`, `false`, `no`, `off`,
  case-insensitive; whitespace-trimmed).
- A disabled Overseer (`SIMARD_OVERSEER_ENABLED=0`) forces recall off regardless
  of this flag — recall only makes sense while the Overseer runs.
- Parsing never panics on a malformed value; any non-falsey value leaves recall
  enabled.

`build_overseer` threads the resolved flag through
`.with_memory_recall_enabled(memory_recall_enabled())`, mirroring
`.with_goal_health_enabled(...)`. With the flag off, `ObservedState.recall`
stays `None`, no episodes are written, and the three counters stay `0`.

See the how-to: [Configure the Overseer's memory recall](../howto/configure-overseer-memory-recall.md).

## Security model — recalled content is untrusted input

Simard's cognitive-memory graph is **multi-writer** (the OODA loop, the
memory-IPC server, consolidation, journal, and other agents all write to it).
The Overseer therefore treats every recalled fact/episode/procedure/prospective
as **untrusted input**:

- **Data, never control.** Recalled text is never fed to a shell, a path, or a
  prompt-as-instruction. Decisions key on the **structural** `failure_signature`
  + occurrence count, which blocks memory-poisoning → decision-hijack and prompt
  injection.
- **Egress hardening.** `RecurringSignature`'s advisory procedure text *can*
  reach an operator notification: Decide folds it into a `Problem.summary`, which
  `whisper_ops::compose_whisper_note` and `notify::OperatorNotification` then
  render to email/Signal. To keep that path safe, recalled text is passed through
  the `capabilities::sanitize_recalled` helper (length-cap + CR/LF/control-char
  strip/escape) at the **single admission boundary** in `mod.rs` — *before* it is
  ever written into a `Problem.summary`. Everything downstream
  (`whisper_ops.rs`, `notify.rs`) is therefore unchanged and only ever renders
  already-sanitized text, closing log/notification injection and spoofing.
  Because decisions key on structure, no recalled free-text is ever placed into a
  PR-comment body or into a shell/path/prompt context.
- **Fixed provenance & least privilege.** Write-back is read-only recall plus one
  deliberate, gated `store_episode`; there is no delete/overwrite. `source_label`
  is fixed (`"overseer"`), metadata is typed and validated, and the whole seam is
  an in-process trait over the existing shared handle — no new network, socket,
  or auth surface.
- **Bounded / fail-closed.** Constant recall budgets (`5/5/3/5`) + write-back
  dedup + panic-isolated tick + surfaced-error (never silent-empty) mean a slow,
  huge, or hostile graph degrades to "loop continues, error surfaced", never to a
  stall or a silent wrong decision.
- **Advisory only.** `RecurringSignature` cannot directly command merge/deploy —
  it still passes the existing dedup/`WhisperGate`/priority/autonomy gates.

## See also

- [Overseer — operator/observer co-process (design)](../design/overseer.md)
- [Configure the Overseer's memory recall (how-to)](../howto/configure-overseer-memory-recall.md)
- [Overseer activity feed reference](./overseer-activity-feed.md)
- [Cognitive memory](../memory.md)
- [StatusSnapshot API](./status-snapshot-api.md)
- [Stewardship API](./stewardship-api.md)
