---
title: "Reference: Claim-Reaper Convergence & Idempotent Archival"
description: >
  The terminal Converged verdict and the idempotent-archival guard that stop the
  stale-engineer investigation from looping on verdict=pending for standing /
  perpetual research goals. Covers the (claim_key, evidence_fingerprint) dedup
  key, SHA-256 canonicalization, the bounded per-claim guard store, and the
  guarantee of one terminal decision + one archival (no 59x re-archival)
  (issue #4755).
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./claim-reaper-api.md
  - ./investigate-stale-engineer-api.md
  - ./tombstoned-goal-engineer-reaper-api.md
  - ../howto/investigate-a-stale-engineer-before-reap.md
  - ../howto/diagnose-perpetual-completion-recuration.md
  - ../operations/claim-reaper-kill-switch.md
---

# Reference: Claim-Reaper Convergence & Idempotent Archival

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary source:
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs).
> Tracked by [issue #4755](https://github.com/rysweet/Simard/issues/4755).

## Overview

The stale-engineer investigation seam
([`StaleEngineerInvestigator`](./investigate-stale-engineer-api.md)) archives an
engineer's diagnostic evidence and returns an [`InvestigationVerdict`] before the
reaper decides whether to reclaim the claim. For a **standing / perpetual
research goal**, the investigation legitimately never reaches `Dead` — but it
also never reached a *terminal* state. Each Overseer tick re-investigated the
same still-alive engineer, re-archived byte-identical evidence, and returned
`verdict=pending`. In production this produced **59× re-archival** of the same
evidence for stale engineer `70ab8541` and unbounded growth of
`reaped-engineers/`, with per-cycle band-aid PRs (#4608, #4642) repeatedly
persisting the same "fail-closed still-alive" verdict.

Two additive mechanisms make the investigation **converge**:

1. a terminal **`Converged`** verdict — a stable, fail-closed decision that a
   standing goal's engineer has been investigated and needs no further
   re-investigation this run; and
2. an **idempotent-archival guard** — a per-claim dedup keyed on
   `(claim_key, evidence_fingerprint)` so byte-identical evidence archives
   exactly once.

Result: a standing-goal stale engineer reaches **one terminal verdict with a
single archival**, and `reaped-engineers/` stops growing unboundedly.

## The `Converged` verdict

`Converged` is an additive, non-terminal-for-reaping variant of
[`InvestigationVerdict`]. Like every non-`Dead` verdict it is **fail-closed**:
it KEEPS the claim (`should_reap()` stays `false`). It differs from `Pending`
in that it is *stable* — once reached for a given evidence fingerprint, the
investigation does not re-run and re-archive on subsequent ticks.

```rust
// src/overseer/claim_reaper.rs
pub enum InvestigationVerdict {
    /// FALSE POSITIVE — the engineer is actually still working. Never reaped.
    #[default]
    StillAlive,
    /// Stuck on a missing precondition but not dead. Never reaped.
    Blocked,
    /// Died from a TRANSIENT condition a relaunch would clear. Not reaped.
    Recoverable,
    /// The agentic investigation is still IN FLIGHT. Not reaped; a later sweep
    /// resolves it.
    Pending,
    /// Investigation reached a STABLE terminal decision for a standing /
    /// perpetual goal: fully investigated, no further re-investigation or
    /// re-archival this run. Never reaped (fail-closed like every non-Dead
    /// verdict).
    Converged,
    /// Genuinely gone AND unrecoverable. The ONLY verdict that reaps.
    Dead { cause: InvestigationCause },
}
```

`should_reap()` remains `matches!(self, Dead { .. })` — `Converged` never reaps.
`label()` returns the stable, log-safe token `"converged"`.

> **Serialization compatibility.** `Converged` is inserted *before* `Dead` in the
> enum for readability, but the wire/persisted form is **name-tagged, not
> positional** — verdicts are serialized by their variant name (e.g. `"dead"`,
> `"pending"`), never by ordinal/index. Adding `Converged` therefore does not
> shift any existing tag, so verdicts already persisted under
> `reaped-engineers/` deserialize unchanged, and an older reader that predates
> `Converged` treats it as an unknown non-`Dead` verdict (fail-closed: keeps the
> claim). No migration of persisted verdicts is required.

### Pending vs. Converged

| Verdict | Meaning | Re-investigates next tick? | Reaps? |
| --- | --- | --- | --- |
| `Pending` | Investigation launched, not yet resolved | Yes (a later sweep resolves it) | No |
| `Converged` | Standing goal fully investigated; stable decision | No (guard holds it) | No |

## Idempotent-archival guard

Before archiving evidence, the seam computes an **evidence fingerprint** and
consults a per-claim guard. If the same `(claim_key, evidence_fingerprint)` has
already been archived, the archival is skipped and the prior terminal verdict is
returned unchanged.

### Fingerprint

The fingerprint is a **SHA-256** over the **canonicalized** evidence: fields are
serialized in a stable, deterministic order (sorted keys, normalized whitespace,
volatile fields such as timestamps and per-tick evidence-dir paths excluded) so
that logically-identical evidence always produces the same digest, and any real
change in the engineer's state produces a different one.

```rust
/// SHA-256 over canonicalized (stable-ordered, volatile-fields-excluded)
/// evidence. Collision-resistant so distinct evidence never aliases to a
/// premature `Converged`; deterministic so identical evidence archives once.
fn evidence_fingerprint(evidence: &StaleEngineerEvidence) -> [u8; 32];
```

> **Security note.** A weak or truncated hash could alias distinct evidence to
> the same key, producing a premature `Converged` and a wrongful skip of a real
> re-investigation. The guard uses full-width SHA-256 specifically to prevent
> this.

### Guard store

The guard is a **bounded per-claim store** keyed on `claim_key`, holding the set
of already-archived fingerprints for that claim. It is:

- **bounded** — capped size per claim; entries are evicted when the claim is
  reaped or released, so the store cannot grow unboundedly;
- **fail-closed** — if the guard cannot be consulted (I/O fault), the seam
  behaves as before (archive + return the fail-closed default), never
  fabricating a `Converged`.

## Configuration

Convergence is on by default and requires no configuration to stop the loop. The
existing reaper kill-switch and interval knobs
(`SIMARD_CLAIM_REAP_*`, see the
[claim-reaper kill switch runbook](../operations/claim-reaper-kill-switch.md))
continue to govern the sweep.

## Examples

### A standing research goal converges once

```text
tick 1  investigate claim=engineer:70ab8541
        archive evidence fp=9f3c… (first time) → verdict=converged
tick 2  investigate claim=engineer:70ab8541
        fp=9f3c… already archived → SKIP archival → verdict=converged (stable)
tick N  … same: single verdict, single archival, no reaped-engineers/ growth
```

Before this change the same sequence produced:

```text
tick 1..59  archive evidence fp=9f3c… (again) → verdict=pending
            reaped-engineers/ grows every tick; PRs #4608/#4642 re-persist
```

## Fail-closed guarantees

- `Converged` **never reaps** — it keeps the claim like every non-`Dead` verdict.
- The guard **never fabricates** a `Converged`: it only holds an *already-decided*
  terminal verdict for *byte-identical* evidence.
- Any real change in engineer state changes the fingerprint, so a genuinely
  progressing or newly-dead engineer is re-investigated and can still reach
  `Recoverable` / `Dead`.

## Regression tests

Co-located in
[`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs):

- `standing_goal_converges_to_single_verdict` — a standing-goal stale engineer
  reaches exactly one terminal `Converged` verdict across many ticks.
- `identical_evidence_archives_once` — byte-identical evidence archives a single
  time; no 59× re-archival.
- `changed_evidence_reinvestigates` — a different fingerprint re-runs the
  investigation (no premature convergence).
- `fingerprint_non_collision` — canonicalized-distinct evidence yields distinct
  digests.
- `converged_never_reaps` — `Converged.should_reap()` is `false`.
- `guard_store_is_bounded_and_evicts_on_reap` — the per-claim store stays bounded.
- `persisted_verdicts_deserialize_after_converged_added` — verdicts serialized
  before `Converged` existed round-trip by name, confirming the additive variant
  needs no migration.

## Related

- [Stale-Engineer-Claim Reaper API](./claim-reaper-api.md)
- [Investigate-Before-Reap API](./investigate-stale-engineer-api.md)
- How-to: [Diagnose perpetual completion re-curation](../howto/diagnose-perpetual-completion-recuration.md)
- Runbook: [Claim-reaper kill switch](../operations/claim-reaper-kill-switch.md)
