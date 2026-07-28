---
title: The no-progress breaker survives goal-id churn and board resets
description: >
  Why the OODA no-progress breaker no longer re-files identical `ooda-stuck`
  tracking issues (observed: five identical `UNCLEAR-CRITERIA` issues #4944,
  #4946, #4952, #4954, #4958 for one goal in a single day) even when the goal's
  transient id churns between cycles or the goal board is reset. Explains the
  second-order defect the original storm-suppression fix could not cover — the
  durable `WipRef` suppression marker is keyed to the volatile goal id, so id
  churn and board-reset both silently defeat it — and the two additive
  hardening layers: a stable, injection-safe *folded goal identity*
  (`fold_goal_identity`) embedded as an `ooda-goal-key:<folded_id>` marker in
  the issue body, and a strongly-consistent *open-issue existence backstop*
  (`NoProgressIssueFiler::issue_open_with_marker`) queried only after the
  board-local marker misses. Both are additive; a goal with a stable id and an
  intact board behaves identically to before.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./no-progress-breaker-storm-suppression.md
  - ./no-progress-root-cause-resolution.md
  - ./no-progress-terminal-investigation.md
  - ../reference/no-progress-breaker-goal-key-backstop-api.md
  - ../reference/no-progress-breaker-storm-suppression-api.md
  - ../reference/no-progress-breaker-api.md
  - ../howto/configure-no-progress-breaker-open-issue-backstop.md
  - ../howto/diagnose-a-no-progress-breaker-issue-storm.md
---

# The no-progress breaker survives goal-id churn and board resets

> **Status: implemented.** The pure `fold_goal_identity` helper and the
> `ooda-goal-key:<folded_id>` body marker live in
> [`src/goal_curation/no_progress_breaker.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/no_progress_breaker.rs);
> the additive `NoProgressIssueFiler::issue_open_with_marker` backstop and its
> wiring into `escalate_with_tracking_issue` live in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs).
> For the exact types and functions see the
> [goal-key backstop API reference](../reference/no-progress-breaker-goal-key-backstop-api.md).

## The residual defect

The [issue-storm suppression fix](./no-progress-breaker-storm-suppression.md)
closed the first-order loop: it writes a durable `WipRef` suppression marker to
the goal board **before and independent of** the best-effort `gh` link, so a
failed `gh` call can no longer leave a goal "untracked" and re-filing forever.

That marker, however, is **keyed to the goal's transient id**. Two real
conditions defeat it:

1. **Goal-id churn.** A goal's `ActiveGoal.id` is not guaranteed stable across
   OODA cycles — re-articulation, curation, and re-selection can mint a fresh
   id for what is, semantically, the same stuck goal. The suppression marker
   travels with the old id; the re-minted goal carries no marker and escalates
   again. Each churn produces one more identical issue.
2. **Goal-board reset.** The `WipRef` marker lives on the goal board. A board
   reset (a wipe, a schema migration, a restore from an older snapshot, or a
   fresh daemon started against an empty board) discards every marker. The next
   time the same population stalls, every goal escalates as if for the first
   time.

The observed evidence is a single-goal storm: five identical
`OODA no-progress breaker … (UNCLEAR-CRITERIA)` issues — **#4944, #4946,
#4952, #4954, #4958** — all citing one goal (`a8f57a50`) filed within a single
day. That is not five stuck goals; it is one stuck goal whose board-local
marker was repeatedly bypassed.

> The first-order fix was necessary but not sufficient. A board-local marker can
> only suppress re-filing while the board (and the id it is keyed to) survives.
> The durable record of "I already filed for this goal" must instead live where
> it cannot churn or be reset: **on the GitHub issue itself.**

## The two hardening layers

### Layer 1 — a stable folded goal identity

`fold_goal_identity` collapses a churny `ActiveGoal.id` into a stable,
injection-safe token: the first 16 hex characters of
`sha256(<goal id>)`. It mirrors the stewardship
[`failure_signature`](../reference/no-progress-breaker-storm-suppression-api.md)
folding contract — same hashing shape, same `[0-9a-f]`-only output — so a
volatile id maps to one deterministic key.

The breaker embeds that key into the **body** of every issue it constructs, as a
single marker line:

```
ooda-goal-key: 9f8c1a2b3c4d5e6f
```

The marker is placed in the body, never the title, so the human-facing title
stays clean and stable regardless of the underlying id. Because the token is a
bare hex string it is safe to interpolate into a `gh` `--search` query — see
[Why the key is hashed](#why-the-key-is-hashed) below.

### Layer 2 — an open-issue existence backstop

`NoProgressIssueFiler` gains one additive method:

```rust
fn issue_open_with_marker(&self, marker: &str) -> bool { false }
```

The default is `false`, so every existing filer and every test fake compiles
unchanged and behaves exactly as before. The production `GhIssueFiler`
implements it by querying **open** `ooda-stuck` issues for the embedded
`ooda-goal-key:<folded_id>` marker via a direct, argv-vector
`gh issue list --search … in:body` call. It follows the same query *pattern*
the supply-chain steward uses for its own dedup, but is an independent call
scoped to the breaker's own `ooda-stuck` label — it does not reuse the
steward's signature-typed `search_issues` method (that keys on
`stewardship-signature`, not `ooda-goal-key`).

`escalate_with_tracking_issue` now checks two guards, in cost order:

```text
goal stalls past threshold → escalate
        │
        ▼
1. board-local WipRef marker present?   ── yes ──► suppress (fast path, zero API)
        │ no
        ▼
2. open ooda-stuck issue carries this
   ooda-goal-key marker?                ── yes ──► suppress (backstop, one API call)
        │ no
        ▼
file the issue (embedding the marker) + write the WipRef marker
```

The backstop runs **only** on a board-local miss, so a steady-state daemon with
an intact board makes **zero** extra API calls — the fast path answers every
already-suppressed goal without touching GitHub. The one search happens only
when the cheap marker is absent, which is exactly the churn / board-reset case
the backstop exists to cover.

## Why the key is hashed

`fold_goal_identity` is not only a *stability* choice; it is a *security*
control. The folded key is interpolated into a `gh issue list --search` query.
A raw `ActiveGoal.id` is free-form text; if it contained whitespace, quotes, or
GitHub search qualifiers (`is:`, `label:`, `in:`) it could corrupt the search
and turn the dedup check into a reliability bypass — the query could silently
match nothing and re-enable the storm. Folding to a fixed `[0-9a-f]{16}` token
makes the search argument a constant-charset literal that cannot carry
qualifiers. The `gh` invocation itself stays an **argv vector**
(`Command::new("gh").args([...])`) — never a shell string — so there is no
command-injection surface either.

Any free-text the breaker copies into the issue body (goal descriptions, error
excerpts) continues to pass through the
[`redact_token` / `redact_uuids`](../reference/no-progress-breaker-storm-suppression-api.md)
helpers, so no secrets or volatile UUIDs leak into a durable public issue.
(These helpers are module-private in `stewardship::dedup` today and must be
widened to `pub(crate)` so `goal_curation` can call them — see the
[API reference implementation prerequisites](../reference/no-progress-breaker-goal-key-backstop-api.md#implementation-prerequisites).)

## Fail-open by design

If the backstop's `gh issue list` call fails — a network blip, an auth hiccup,
a rate-limit — `issue_open_with_marker` returns `false` (treat as "no duplicate
found") and filing proceeds. This is the deliberate, documented direction: a
**rare duplicate issue is strictly better than a lost stuck-goal signal.** An
existence-check outage must never abort the OODA cycle or silently swallow a
real stall. The board-local `WipRef` fast path remains the primary,
API-free guarantee; the backstop is the durability layer for the churn/reset
edge, not a replacement for it.

## What did not change

- **Thresholds and the resolution ladder.** `NO_PROGRESS_BREAKER_THRESHOLD` and
  the MarkDone / Drop / Escalate ladder are untouched. This change gates only
  the *file* step, not *when or whether* the breaker fires.
- **The breaker's purity.** `no_progress_breaker.rs` stays hermetic: it only
  gained the pure `fold_goal_identity` helper and now embeds the marker string
  into the body it constructs. It performs no GitHub I/O.
- **The `WipRef` schema.** No new marker kind; the folded key rides in the issue
  body, and the existing board-local suppression marker is unchanged.
- **Clear-criteria goals.** A goal with a stable id and an intact board hits the
  fast path on every re-stall exactly as before — no new API cost, no behaviour
  change.

## The `recurring_goal_reblock` half

The originating brief also cited a stewardship `recurring_goal_reblock` storm
(issues #4945, #4951, #4956, one shared signature `cfa5358a3b59894c`). That
filer's dedup is signature-based via
[`stewardship::failure_signature` / `find_existing`](../reference/no-progress-breaker-storm-suppression-api.md)
and its filing path (`src/overseer/observer.rs`) is **not present on every
branch**. The reblock half therefore follows the identical pattern —
signature-folded key + `find_existing` over open issues — but is gated on
branch reconciliation: where the overseer reblock filer exists it applies the
same backstop; where it does not, the work is tracked as a documented follow-up
rather than fabricated. See
[the how-to](../howto/configure-no-progress-breaker-open-issue-backstop.md#the-recurring_goal_reblock-half)
for reconciliation steps.

## See also

- [No-progress breaker issue-storm suppression](./no-progress-breaker-storm-suppression.md)
  — the first-order fix this hardens.
- [Goal-key backstop API reference](../reference/no-progress-breaker-goal-key-backstop-api.md)
  — exact signatures and contracts.
- [Configure the no-progress breaker open-issue backstop](../howto/configure-no-progress-breaker-open-issue-backstop.md)
  — operator configuration, labels, and verification.
- [Diagnose a no-progress breaker issue storm](../howto/diagnose-a-no-progress-breaker-issue-storm.md)
  — triage when duplicates are observed.
