---
title: "Reference: Claim-Reaper Convergence & Idempotent Archival"
description: >
  The terminal Converged verdict and the idempotent per-claim archival guard that
  stop the stale-engineer investigation from looping on verdict=pending for
  standing / perpetual research goals. Covers the freshness-window archival dedup
  (reuse-in-place, minted=false) and the guarantee of one terminal decision +
  bounded archival (no 59x re-archival every tick) (issue #4755).
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
2. the existing **idempotent per-claim archival guard** — a freshness-window
   dedup ([`find_recent_archive_epoch`] + [`ARCHIVE_FRESHNESS_WINDOW`]) that
   REUSES a within-window evidence epoch in place (`minted = false`) instead of
   minting a new `<key>-<ts>/` directory every tick.

Result: a standing-goal stale engineer reaches **one terminal verdict** and its
evidence is archived **at most once per freshness window** (not every tick), so
`reaped-engineers/` stops growing unboundedly.

## The `Converged` verdict

`Converged` is an additive, non-terminal-for-reaping variant of
[`InvestigationVerdict`]. Like every non-`Dead` verdict it is **fail-closed**:
it KEEPS the claim (`should_reap()` stays `false`). It differs from `Pending`
in that it is *stable* — it marks a standing goal as fully investigated for this
run, so the seam does not treat it as an outstanding, must-resolve investigation.

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
| `Converged` | Standing goal fully investigated; stable decision | No (terminal for this run) | No |

## Idempotent per-claim archival guard

The re-archival half of the loop is bounded by the reaper's **existing**
per-claim freshness-window dedup in
[`archive_stale_engineer_evidence`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs)
— no new guard store is introduced.

Before minting a fresh archive directory, the seam calls
[`find_recent_archive_epoch`]: if a `reaped-engineers/<sanitized_key>-<ts>/`
evidence epoch for THIS claim already exists within
[`ARCHIVE_FRESHNESS_WINDOW`] (1 hour), that epoch is **reused in place**
(`ArchiveOutcome { minted: false, .. }`) rather than creating a sibling
timestamped directory. Only a freshly-minted epoch (`minted = true`) writes the
`manifest.json` / `evidence.txt` / `journal.txt` (so the bounded `journalctl`
capture runs at most once per epoch, not every tick).

```rust
// src/overseer/claim_reaper.rs — reuse a within-window epoch in place.
if let Some(existing) = find_recent_archive_epoch(&archive_root, &sanitized, ts) {
    return Ok(ArchiveOutcome { dir: existing, minted: false });
}
```

The `<ts>` is parsed from the directory NAME (not its mtime), so the window is
derived from the archive epoch itself and is robust to later in-place refreshes.

This is:

- **bounded** — at most one minted archive per claim per window, so a standing
  goal re-investigated every ~15-minute tick archives once per hour, not 59×;
- **fail-closed** — an I/O error minting or reading the archive is surfaced as
  `Err`, and the caller keeps the claim (never reaps without preserved evidence),
  never fabricating a `Converged`.

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
        mint archive epoch 70ab8541-<ts> (first time) → verdict=converged
tick 2  investigate claim=engineer:70ab8541
        within-window epoch exists → REUSE in place (minted=false) → verdict=converged
tick N  … same: one terminal verdict, archival bounded to once per window,
        no reaped-engineers/ growth every tick
```

Before this change the same sequence produced:

```text
tick 1..59  re-investigate → verdict=pending (never terminal)
            reaped-engineers/ churned every tick; PRs #4608/#4642 re-persist
```

## Fail-closed guarantees

- `Converged` **never reaps** — it keeps the claim like every non-`Dead` verdict.
- The archival guard **never fabricates** a `Converged`: it only bounds *where*
  evidence lands (reuse-in-place vs mint), never the verdict itself.
- A new archive epoch is minted once the freshness window elapses, so a genuinely
  progressing or newly-dead engineer is still re-investigated and can reach
  `Recoverable` / `Dead`.

## Regression tests

Co-located in
[`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs):

- `standing_goal_converges_to_single_verdict` — a standing-goal stale engineer on
  a `Converged` outcome is never reaped across 59 sweeps (mirrors the observed
  59× non-convergence loop); the claim is preserved and its worktree never cleaned.
- `converged_never_reaps` — `Converged.should_reap()` is `false` and its label is
  the stable token `"converged"`.
- `converged_label_does_not_shift_existing_verdict_labels` — adding `Converged`
  leaves every existing name-tagged label unchanged (no persisted-verdict
  migration required).
- `converged_is_kept_like_pending` — both `Pending` and `Converged` keep the
  claim (fail-closed).

## Related

- [Stale-Engineer-Claim Reaper API](./claim-reaper-api.md)
- [Investigate-Before-Reap API](./investigate-stale-engineer-api.md)
- How-to: [Diagnose perpetual completion re-curation](../howto/diagnose-perpetual-completion-recuration.md)
- Runbook: [Claim-reaper kill switch](../operations/claim-reaper-kill-switch.md)
