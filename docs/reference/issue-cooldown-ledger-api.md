---
title: Issue-cooldown ledger API
description: >
  The typed surface of the durable `IssueCooldownLedger` — the single dedup
  primitive intended to de-duplicate the OODA-core auto-issue filers
  (`ooda-stuck`, `recurring_goal_reblock`, `workstream_gap:issue`) across daemon
  exec-reload, restart, and cross-client goal-board merges. Documents the
  `FindingKind`/`CooldownKey`/`CooldownDecision` types, the
  `allow_emit`/`record_emit`/`note_still_observed`/`prune` methods, the durable
  cognitive-memory backing (namespace `overseer:issue-cooldown`), the reused
  `WhisperGate::with_backoff` window formula, configuration, and the contract
  tests.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/durable-issue-cooldown-ledger.md
  - ./whisper-gate-backoff-api.md
  - ./overseer-gap-scan-durable-dedup.md
  - ./cognitive-memory-fact-recall.md
---

# Issue-cooldown ledger API

> **Status.** The durable primitive — `IssueCooldownLedger`, `FindingKind`,
> `CooldownKey`, and `CooldownDecision` — is **implemented and unit-tested** in
> [`src/overseer/issue_cooldown.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/issue_cooldown.rs),
> with its config knobs in
> [`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs)
> and the reused window math in
> [`src/overseer/guardrails.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/guardrails.rs).
> Routing the three OODA-core filers through it (and the defensive
> `merge_boards` `wip_refs` union) is the **planned integration** described in
> [Wiring](#wiring-planned-integration); it is additive and does not change the
> primitive's contract below. Conceptual overview:
> [The durable issue-cooldown ledger stops the OODA-core auto-issue storm](../concepts/durable-issue-cooldown-ledger.md).

## Purpose

`IssueCooldownLedger` is the single durable dedup layer for the OODA-core
auto-issue filers. It guarantees that a given `(finding_kind, subject)` opens
**at most one** tracking issue per cooldown window — where the window floor
spans **≥ 1 full OODA cycle** — and that the guarantee **survives daemon
exec-reload, process restart, and cross-client goal-board merges** (the three
ways the previous per-path dedup leaked).

It is additive: absent the ledger, each filer keeps its prior behavior.

## Types

```rust
/// Stable identifier for the class of finding being filed. Serialized as a
/// fixed, charset-restricted slug so keys are stable across cycles and safe to
/// embed in a `gh` search signature.
pub enum FindingKind {
    OodaStuck,            // slug "ooda-stuck"
    RecurringGoalReblock, // slug "recurring-goal-reblock"
    WorkstreamGapIssue,   // slug "workstream-gap-issue"
}

impl FindingKind {
    pub fn slug(self) -> &'static str;
}

/// Durable, canonicalized dedup key: `(finding_kind, subject)`.
pub struct CooldownKey {
    pub kind: FindingKind,
    pub subject: String,
}

impl CooldownKey {
    /// Build a key from a finding kind and a raw subject (goal id / gap
    /// signature). Canonicalizes `subject`. Total and pure.
    pub fn new(kind: FindingKind, raw_subject: &str) -> Self;

    /// The stable cognitive-memory fact concept:
    /// `overseer:issue-cooldown:<slug>:<canonical-subject>`.
    pub fn fact_concept(&self) -> String;
}

/// Result of consulting the ledger before filing.
pub enum CooldownDecision { Emit, Throttle }
```

### Subject canonicalization

`CooldownKey::new` reduces the untrusted `subject` to `[a-z0-9_]` — lower-case,
keep alphanumerics, map every other byte to `_`, collapse runs of `_`, and trim.
This is deliberately **stricter** than the `[a-z0-9:_-]` concept charset: dropping
`:` and `-` from the *subject* means untrusted goal/gap text can never contribute
a `gh --search` qualifier such as `is:issue` or `label:...` (SR-V3). The `:`/`-`
that appear in a `fact_concept` come only from the fixed namespace and slug that
the module controls. Two raw subjects that canonicalize to the same value (e.g.
`"Goal FOO"` and `"goal___foo"`) collapse to one stable key.

## Ledger

```rust
pub struct IssueCooldownLedger { /* memory + WhisperGate window */ }

impl IssueCooldownLedger {
    /// Construct with the durable memory backing and the exponential window.
    pub fn new(memory: Arc<dyn CognitiveMemoryOps>, window: WhisperGate) -> Self;

    /// Decide whether a filer may open a new issue now. Reads the durable
    /// last-emit + strikes for `key` and applies the backoff window. Fail-OPEN:
    /// a memory-read error returns `Emit`. Does not mutate durable state.
    pub fn allow_emit(&self, key: &CooldownKey, now_secs: i64) -> CooldownDecision;

    /// Record that an issue was filed. Upsert-idempotent: advances last-emit and
    /// grows the strike count **in place** via `store_fact_with_caller_key`
    /// (no duplicate fact).
    pub fn record_emit(&self, key: &CooldownKey, issue: &GhIssue, now_secs: i64)
        -> SimardResult<()>;

    /// Comment-and-throttle: annotate the ONE existing tracking issue for `key`
    /// instead of filing a new one. Fail-OPEN on `gh` errors. Never files.
    pub fn note_still_observed(&self, key: &CooldownKey, gh: &dyn GhClient,
        repo: &str, now_secs: i64) -> SimardResult<()>;

    /// Count cooldown facts not touched for `> cap_secs` (bounded-memory
    /// hygiene). Actual node reclamation is delegated to the memory backend's
    /// retention pass.
    pub fn prune(&self, now_secs: i64) -> SimardResult<usize>;
}
```

### Emit / throttle protocol

A filer consults the ledger **before** any `create_issue`:

```rust
let key = CooldownKey::new(FindingKind::OodaStuck, &format!("goal:{goal_id}"));
match ledger.allow_emit(&key, now) {
    CooldownDecision::Emit => {
        let issue = gh.create_issue(repo, &title, &body)?;
        ledger.record_emit(&key, &issue, now)?;
    }
    CooldownDecision::Throttle => {
        // Do NOT file. Keep the signal alive on the ONE canonical issue.
        ledger.note_still_observed(&key, gh, repo, now)?;
    }
}
```

`Emit` is only returned when there is no prior durable emit **or** the backoff
window has elapsed. Every `record_emit` advances the durable timestamp and the
strike count, so the next in-window cycle returns `Throttle`.

## Window formula

The ledger reuses [`WhisperGate::with_backoff`](./whisper-gate-backoff-api.md)
verbatim via the exposed `WhisperGate::window_for_strikes(strikes)`. For a key
with `strikes` prior in-window emits:

```
window(strikes) = min(base_secs * 2^(strikes - 1), cap_secs)   // strikes >= 1
window(0)       = base_secs
```

- **`base_secs`** floors at **one full OODA cycle** — the observer cadence
  `overseer_interval_secs()` (default `900`, floor `60`) — so the same
  `(goal, finding)` can never re-file every cycle. Default `21_600` (6 h).
- **`cap_secs`** hard-caps at **`86_400` (24 h)** so a still-open finding
  re-surfaces at least daily and is never permanently silenced.

With the defaults the effective sequence is `6h → 12h → 24h → 24h → …`.

## Durable backing (cognitive memory)

The ledger stores one fact per key via
[`CognitiveMemoryOps::store_fact_with_caller_key`](./cognitive-memory-fact-recall.md)
— the caller-key variant, **not** plain `store_fact`. Plain `store_fact` appends
a new fact on every call; `store_fact_with_caller_key` supersedes the prior fact
bearing the same caller key, giving the in-place, idempotent upsert:

- **caller_key / concept / source_id** — all `key.fact_concept()` =
  `overseer:issue-cooldown:<slug>:<canonical-subject>` (isolated namespace).
- **content** — a small JSON blob: `{ last_emit_secs, strikes, issue_number }`.
  **No** issue body, no secrets, no tokens (see Security).
- **tags** — `["overseer", "issue-cooldown", "<slug>"]` for recall/prune scans.

Because the fact namespace is **separate from the goal-board snapshot**, it is
immune to both `merge_boards` last-writer-wins and per-cycle goal re-creation,
and it is re-read on exec-reload/restart — the two failure modes that let the
in-memory gate leak.

## Configuration

All knobs are additive `SIMARD_OVERSEER_*` env vars, injectable for hermetic
tests (`impl Fn(&str) -> Option<String>`), following the existing
`src/overseer/config.rs` convention.

| Env var | Default | Effect |
|---|---|---|
| `SIMARD_OVERSEER_ISSUE_COOLDOWN` | on | Opt-out flag (`0`/`false`/`no`/`off` disables the ledger; each filer falls back to its per-path dedup). |
| `SIMARD_OVERSEER_ISSUE_COOLDOWN_BASE_SECS` | `21600` | Backoff window floor. Clamped to `>= overseer_interval_secs()` (one OODA cycle) so it can never drop below one cycle. |
| `SIMARD_OVERSEER_ISSUE_COOLDOWN_MAX_SECS` | `86400` | Backoff window cap (24 h). Clamped to `>= base`. |
| `SIMARD_OVERSEER_ISSUE_COOLDOWN_CAP_PER_HOUR` | `20` | Rolling-hour emit budget shared with the `WhisperGate` semantics. |

## Observability

- **tracing target `overseer::issue_cooldown`** at `debug`: one line per
  `allow_emit` decision (`emit` / `throttle`), the canonical key, the current
  window, and strike count. Never logs issue bodies, tokens, or secrets.
- Cooldown facts are readable via `search_facts` with query
  `overseer:issue-cooldown`.

## Security & invariants

- **SR-V3 — no search-qualifier injection.** `subject` is reduced to `[a-z0-9_]`
  before it is embedded in any fact concept or `gh` signature.
- **SR-D1/D2 — no sensitive data in memory or logs.** The stored fact holds only
  `{ last_emit_secs, strikes, issue_number }`; no issue body, no token. Tracing
  is `debug`-level metadata only. No `print!`/`println!` — `tracing` only.
- **SR-D3 — fail-open read, isolated namespace.** `allow_emit` returns `Emit` on
  a memory-read error; the cooldown namespace is isolated from other facts.
- **Comment-and-throttle fails open on `gh`.** `note_still_observed` swallowing a
  `gh` error loses an annotation, not the "file once" guarantee — `record_emit`
  already prevented the duplicate.
- **Additive / non-breaking.** No PRD change; no "Bridge" naming; the existing
  per-path dedup, markers, and their tests remain as regression guards.

## Wiring (planned integration)

The primitive is designed to be constructed once and shared by all three filers:

```rust
let cooldown = Arc::new(IssueCooldownLedger::new(
    memory.clone(),
    WhisperGate::with_backoff(
        issue_cooldown_base_secs(),    // default 21_600, clamped >= overseer_interval_secs()
        issue_cooldown_cap_secs(),     // default 86_400 (24h)
        issue_cooldown_cap_per_hour(), // default 20
    ),
));
```

- **Provide it** (daemon default) → all three filers consult the durable ledger.
- **Omit it** (custom embeddings / legacy tests) → each filer keeps its prior
  per-path dedup only. Nothing breaks.

A defensive `merge_boards` field-level `wip_refs` **union**
(`src/goal_curation/operations.rs`) is the intended secondary guard so a durable
suppression marker written by one client survives a cross-client merge even if
the ledger is bypassed.

> The routing of the filers and the `wip_refs` union are additive follow-ups
> tracked with issue #4930; they consume the contract above unchanged.

## Contract tests

Run hermetically (no network; fake `GhClient` + in-memory `CognitiveMemoryOps`):

```bash
cargo test -p simard --lib overseer::tests_issue_cooldown
```

| Test | Asserts |
|---|---|
| `cooldown_emits_once_then_throttles` | First `allow_emit` → `Emit`; subsequent in-window → `Throttle`. |
| `cooldown_window_floor_is_ooda_cycle` | `base_secs` clamped up to `>= overseer_interval_secs()`. |
| `cooldown_window_doubles_and_caps_at_24h` | Window follows `6h → 12h → 24h`, never exceeds the cap. |
| `cooldown_refires_after_window` | A still-open finding re-emits once the window elapses. |
| `cooldown_keys_are_per_subject_isolated` | Distinct `(kind, subject)` keys never share a window. |
| `cooldown_upsert_is_idempotent` | Re-recording a key upserts (no duplicate fact). |
| `cooldown_read_fails_open` | A memory-read error yields `Emit`, not silent suppression. |
| `cooldown_survives_reload` | A fresh ledger over the same durable memory still throttles. |
| `cooldown_subject_rejects_search_qualifiers` | Canonicalization strips `gh` search metacharacters. |
| `cooldown_prunes_stale_keys` | Keys unseen for `> cap_secs` are counted for eviction. |
| `cooldown_fact_stores_no_sensitive_body` | The durable fact keeps only non-sensitive metadata. |
| `cooldown_cap_defaults_to_24h_and_clamps_to_base` / `cooldown_cap_per_hour_defaults_to_20` / `cooldown_enabled_by_default_and_opt_out` | Config defaults + opt-out. |

## See also

- [The durable issue-cooldown ledger stops the OODA-core auto-issue storm](../concepts/durable-issue-cooldown-ledger.md) — the concept and the storm it fixes.
- [WhisperGate Exponential-Backoff API](./whisper-gate-backoff-api.md) — the window math reused here.
- [Overseer gap-scan durable open-issue dedup reference](./overseer-gap-scan-durable-dedup.md) — the sibling GitHub-side durable check.
- [Cognitive-memory fact recall reference](./cognitive-memory-fact-recall.md) — the durable backing store.
