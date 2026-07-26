---
title: Overseer gap-filing dedup reference
description: >
  The stable, content-addressed gap signature on the Overseer stewardship
  gap-notification path — the bounded GapCategory taxonomy
  (GoalUncovered / IssueUncovered / AnomalyUnaddressed), the slug-validated
  signature grammar that replaces the old per-run hash, the in-process
  WhisperGate dedup + operator notification the gap path actually performs, the
  injection-defense slug validation enforced at the filing seam, and the durable
  cross-process open-issue check this signature is the foundation for (scoped as
  follow-on work, not yet wired on the gap path).
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./overseer-workstream-gap-scan.md
  - ./overseer-backoff-gate-api.md
  - ./stewardship-api.md
  - ../concepts/gap-scan-backoff-dedup.md
  - ../howto/configure-gap-durable-dedup.md
  - ../howto/file-stewardship-issues-from-orchestrator-runs.md
  - ../howto/review-overseer-workstream-gaps.md
  - ../design/overseer.md
---

# Overseer gap-filing dedup reference

The acting **Overseer** watches for uncovered backlog work — uncovered goals,
high-signal open issues, and unaddressed telemetry anomalies — and flags it on
the recurring gap-scan. Before this rail the gap signature was derived **per
run** (`originating-run: overseer-<hash>`), so every daemon start or re-run
minted a **fresh** signature. The only dedup guard on the gap path is the
in-process [`WhisperGate` / `BackoffGate`](./overseer-backoff-gate-api.md), which
keys on that signature; a churning per-run key meant the same gap re-surfaced as
noise, and the near-duplicate `[stewardship] workstream_gap:*` flood was observed
downstream (e.g. #4671, #4680, #4685).

This rail fixes the **root cause**: it makes the signature a **stable,
content-addressed** slug (derived from the gap's trusted identifiers, not the run
id). With a stable key, the in-process gate now collapses a recurring gap to a
**single operator notification within a running daemon**, and the slug is a
valid, restart-safe join key that a future durable check can search on.

> **Scope — read this first.** What ships here is the **stable signature**, the
> **bounded `GapCategory` taxonomy**, and **injection-defense slug validation**.
> The gap-notification path (`act_flag_workstream_gaps`,
> `src/overseer/mod.rs`) applies the in-process `WhisperGate` and **notifies the
> operator** (email + Signal); it does **not** call `gh.search_issues`,
> `find_existing`, or `create_issue`, and it is **not restart-safe on its own**
> (the gate is in memory and is wiped on restart). The durable, GitHub-sourced
> open-issue check described under [Future work](#future-work-the-durable-cross-process-check)
> is **not yet wired** on the gap path; the stable signature is the prerequisite
> that makes it viable. The proven durable pattern currently lives on the sibling
> stewardship filing seam (`stewardship::process_orchestrator_run`).

> **Modules:** taxonomy + signature grammar `src/overseer/signal.rs`
> (`GapCategory`, `GapItem::signature`, `has_valid_dedup_signature`); batch key
> and filing seam `src/overseer/mod.rs` (`workstream_gap_key`,
> `act_flag_workstream_gaps`); detectors `src/overseer/sensor.rs`
> (`detect_workstream_gaps`). The durable pattern this signature is designed to
> feed lives in `src/stewardship/gh_client.rs` and `src/stewardship/mod.rs`
> (`find_existing`, `process_orchestrator_run`). Hermetic tests
> `src/overseer/signal.rs` (`gap_dedup_key_tests`),
> `src/stewardship/tests_extra.rs`.

## At a glance

| You want to… | Use |
|---|---|
| Understand why a recurring gap stopped re-notifying every tick | This page — the stable signature + in-process gate |
| See the stable key that dedupes a gap | `GapItem.signature` → `stewardship-signature: workstream-gap:<sig>` |
| Know which gap kinds are deduped | The bounded [`GapCategory` taxonomy](#the-bounded-gapcategory-taxonomy) |
| Confirm a recurring gap was deduped, not re-notified | `overseer::gap_scan` `flagged=…`/`suppressed=…` info log + `ActOutcome::WorkstreamGapsFlagged` |
| Operate / verify it | [How to configure and verify gap dedup](../howto/configure-gap-durable-dedup.md) |

## The bounded `GapCategory` taxonomy

Free-form gap titles let the same underlying condition render slightly different
strings on each tick, so no two dedup keys matched. The taxonomy replaces
free-form detection with a **bounded enum**; every variant carries a short,
stable `.label()` slug that anchors the signature.

| `GapCategory` variant | `.label()` | Signature prefix | Filed for |
|---|---|---|---|
| `GoalUncovered` | `goal` | `goal:<goal_id>` | A p1/p2 active goal with no engineer and no PR |
| `IssueUncovered` | `issue` | `issue:<repo>#<n>` | A high-signal open issue with no PR / workstream |
| `AnomalyUnaddressed` | `anomaly` | `anomaly:<slug>` | A live telemetry anomaly with no fix in flight |

The taxonomy is a **bounded, closed enum** (`src/overseer/signal.rs`), so a gap
can only ever resolve to one of these three stable `.label()` slugs — the same
underlying condition renders an **identical** signature on every tick. Every
exhaustive `match` on `GapCategory` already covers these three arms; **none was
renamed**.

## The dedup key (signature) grammar

Each `GapItem` carries a `signature`: a **stable, constrained slug** built from
**trusted identifiers only** (a goal id, `repo#number`, or an anomaly slug) —
never from hostile free text. Identical inputs yield an identical signature, so a
recurring gap dedupes to at most one live notification per daemon.

The act path wraps the per-gap signature in a filing key:

```text
stewardship-signature: workstream-gap:<GapItem.signature>
```

`workstream_gap_key` composes the batch key deterministically (sorted, deduped)
so a multi-gap tick is stable regardless of detection order:

```rust
// src/overseer/mod.rs
fn workstream_gap_key(gaps: &[GapItem]) -> String {
    let mut sigs: Vec<&str> = gaps.iter().map(|g| g.signature.as_str()).collect();
    sigs.sort_unstable();
    sigs.dedup();
    format!("workstream-gap:{}", sigs.join("|"))
}
```

### Signature slug validation (injection defense)

Signatures are validated **at construction** in `signal.rs` and re-checked at the
filing seam (`has_valid_dedup_signature` in `act_flag_workstream_gaps`), so a
malformed or hostile identifier can never reach a notification body — or a
future `gh search` argument. A valid signature matches:

```text
^[a-z0-9][a-z0-9:_#.\-/]{0,200}$
```

Any identifier that would violate this (control characters, spaces, shell
metacharacters, over-length) is rejected at the boundary and the gap is dropped
(counted as suppressed so the observability contract stays exact). This makes any
downstream search query **inert**: a goal id such as `g-1; rm -rf ~` cannot
survive slug validation, so it can never inject into a `gh issue list --search`
term when the durable check is wired.

## What the gap path does today

The gap-notification act path (`act_flag_workstream_gaps`) runs, per tick:

```text
for each detected gap:
  1. has_valid_dedup_signature(gap)   # IV-1: drop a malformed/hostile signature
        └─ invalid → count suppressed, continue (never notified)
  2. WhisperGate.peek(sig)            # in-process dedup (no gh call)
        └─ SuppressDuplicate/Cap → count suppressed, continue
        └─ Deliver               → collect as a FRESH gap
if any fresh gaps:
  3. notifier.notify(workstream_gap)  # ONE consolidated operator notification
                                      #   (email + Signal), never create_issue
  4. WhisperGate.commit(sig)          # record the fresh notification in-process
```

Key properties **as implemented**:

- **Notification-only.** Routine gap observations notify the operator; they do
  **not** create GitHub issues or stewardship backlog items on this path.
- **Deduped within a process.** The stable signature makes the in-process gate
  collapse a recurring gap to one notification per dedup window (default 900 s).
- **Not restart-safe on its own.** The `WhisperGate` lives in memory, so a daemon
  restart resets it. Cross-restart dedup depends on the future durable check
  below.
- **Fail-closed at the seam.** A malformed signature is dropped **before** it can
  reach a notification body — never rendered into operator-facing text.

## Future work: the durable cross-process check

The stable signature exists so a durable, restart-safe check can be layered in
**without** re-deriving a key. The intended follow-on mirrors the proven
[`stewardship::process_orchestrator_run`](./stewardship-api.md) flow:

```text
for each detected gap (after the in-process gate):
  gh.search_issues(repo, sig)   # DURABLE open-issue equivalence check
        └─ Err(e)               → FAIL LOUD: propagate, file nothing
  find_existing(&issues, sig)   # match `stewardship-signature: <sig>`
        └─ Some(issue)          → reuse/comment/skip (no new issue)
        └─ None                 → gh.create_issue(repo, title, body)
```

When wired, this would upgrade the guarantee to *"at most one open issue per
distinct gap signature across restarts and daemons"* and must **fail loud** — a
`gh` search error stops filing rather than falling back to a blind
`create_issue`, which would reintroduce exactly the flood this work prevents.
**This flow is not implemented on the gap-notification path today; it is
tracked in [#4717](https://github.com/rysweet/Simard/issues/4717).**

## Structured observability

The gap path emits structured `tracing` + OTel only — **no**
`print!`/`println!`. On `target: "overseer::gap_scan"`:

| Field | Meaning |
|---|---|
| `flagged` | Fresh gaps notified this tick |
| `suppressed` | Gaps dropped by the in-process `WhisperGate` or by a malformed signature |
| `dispatched` / `all_sent` | Operator-notification delivery status |

The `ActOutcome::WorkstreamGapsFlagged { flagged, suppressed }` outcome and the
`OverseerTickReport.workstream_gaps_detected` / `…_suppressed` counters are
unchanged and additive.

## Guarantees

- **At most one live notification per distinct gap signature within a running
  daemon** (in-process, `WhisperGate`-sourced). Cross-restart / cross-daemon
  dedup is **not** yet guaranteed — see [Future work](#future-work-the-durable-cross-process-check).
- **Never permanently silent.** The in-process gate window is capped and resets
  after silence; a genuinely recurring gap always re-surfaces.
- **Injection-safe.** A gap whose signature is not a valid restricted slug is
  dropped at the seam, so no hostile identifier reaches a notification body or a
  future search term.
- **Additive / non-breaking.** Only the signature's *value* changes (unstable
  per-run hash → stable per-gap slug); the `stewardship-signature:` marker field,
  the notification structure, and every existing caller are untouched. Pre-fix
  issues carry the old per-run hash and do not retro-dedupe.

## Security notes

| Risk | Mitigation |
|---|---|
| Search-query / notification injection via an unconstrained signature | Slug-validate the signature at construction **and** at the seam (`^[a-z0-9][a-z0-9:_#.\-/]{0,200}$`); drop on failure |
| Fail-open regression re-introducing the flood (when the durable check is wired) | Propagate `search_issues` errors; never default to `create_issue` |
| Self-DoS from a fast detector loop | Mandatory `WhisperGate` pre-filter before any external call |

No credential handling lives on this path: the future durable check would let
`gh` supply its own token, which is never read, logged, or embedded.

## Related

- [Overseer workstream gap-scan reference](./overseer-workstream-gap-scan.md) —
  the `GapItem`/`GapCategory`/`Signal::WorkstreamGap` data model this extends.
- [Gap-scan dedup & exponential backoff](../concepts/gap-scan-backoff-dedup.md)
  — the in-process `BackoffGate` this signature stabilizes.
- [Overseer backoff-gate API reference](./overseer-backoff-gate-api.md) — the
  in-process gate the gap path uses.
- [Goal Stewardship — Orchestrator Failure API reference](./stewardship-api.md)
  — the `search_issues → find_existing → create_issue` flow the future durable
  check would mirror.
- [How to configure and verify gap dedup](../howto/configure-gap-durable-dedup.md).
- PRD: `Specs/ProductArchitecture.md` § *Stewardship Mode* / § *Goal
  Stewardship Mode*.
