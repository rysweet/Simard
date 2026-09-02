---
title: Configure and operate Overseer gap-scan durable dedup
description: >
  Operator + developer guide for the Overseer's durable, GitHub-side gap-scan
  open-issue dedup (#4717): how to enable the durable check by injecting a
  GhClient, confirm a restart no longer re-files an already-open covering issue,
  read the reused_existing counter, understand the fail-loud contract, and
  verify the behaviour with the hermetic gap-scan tests.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: howto
status: reference
related:
  - ../reference/overseer-gap-scan-durable-dedup.md
  - ../reference/overseer-workstream-gap-scan.md
  - ../concepts/gap-scan-backoff-dedup.md
  - ../reference/stewardship-api.md
  - ./configure-overseer-gap-scan-backoff.md
  - ./review-overseer-workstream-gaps.md
  - ./watch-overseer-activity.md
  - ../design/overseer.md
---

# Configure and operate Overseer gap-scan durable dedup

> **Status: implemented (#4717).** The durable open-issue dedup check, the
> `GapItem::dedup_key()` contract, the `Overseer::with_gap_issue_client(..)`
> seam, and the `reused_existing` counter ship in the Overseer. For the typed
> surface see the
> [durable dedup reference](../reference/overseer-gap-scan-durable-dedup.md); for
> the in-process burst gate see
> [gap-scan dedup & backoff](../concepts/gap-scan-backoff-dedup.md).

The gap-scan durable dedup rail stops the Overseer from re-filing a covering
issue for a backlog gap **when an equivalent issue is already open on GitHub**,
even after a daemon restart wipes the in-process dedup memory. It treats GitHub
as the source of truth, so the "one open covering issue per distinct gap"
guarantee survives restarts. This page covers enabling it, verifying it, and
troubleshooting.

## Enable durable dedup

Durable dedup is opt-in via one additive builder. In the daemon it is wired
automatically when the gap-scan is enabled; you only wire it by hand in custom
embeddings or tests.

```rust
use simard::overseer::Overseer;
use simard::stewardship::RealGhClient;

let overseer = Overseer::new(/* … */)
    .with_operator_notifier(notifier)
    // Inject the GitHub client that backs the durable open-issue check.
    .with_gap_issue_client(Box::new(RealGhClient::new()));
```

- **Omit `with_gap_issue_client`** → the gap-scan keeps its prior behaviour
  (operator notification + in-process `WhisperGate` dedup only). Nothing breaks.
- **Provide it** → each fresh gap is checked against GitHub's open issues before
  a covering issue is filed.

The gap-scan cadence knobs are unchanged and still gate how often the scan
runs:

| Env var | Effect |
|---|---|
| `SIMARD_OVERSEER_GAP_SCAN` | Enable/disable the gap-scan step |
| `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` | Run the scan every *Nth* tick |

> The target repo the durable check queries and files against comes from
> **trusted Overseer config**, never from gap data. There is no per-gap repo
> override.

## Verify it works

### 1. A restart does not re-file an open issue

1. Let the Overseer detect a genuine backlog gap and file a covering issue. Note
   the issue and its body marker:

   ```text
   stewardship-signature: workstream-gap:<signature>
   ```

2. Restart the daemon (this clears the in-process `WhisperGate`).
3. On the next gap-scan tick, confirm the same gap **reuses** the open issue
   instead of filing a new one. In the tracing on target `overseer::gap_scan`
   you should see the gap counted under `reused_existing`, and **no** new issue
   appears in the repo.

### 2. Read the counters

The act outcome and tick report expose three counts:

```rust
ActOutcome::WorkstreamGapsFlagged {
    flagged,          // new covering issues filed this cycle
    suppressed,       // same-cycle duplicates dropped in-process (Tier 1)
    reused_existing,  // open issues reused across restart / index lag (Tier 2)
}
```

Watch them live on the activity surfaces:

- Dashboard **Overseer** tab / TUI **Overseer** pane / `simard status` →
  **OVERSEER** — the per-tick line reports flagged, suppressed, and reused-existing gaps.
- `GET /api/overseer` → `data.recent[].report.workstream_gaps_reused_existing`.

### 3. Confirm the fail-loud contract

If `gh` is unavailable or the search errors, the affected cycle files **nothing**
and notifies **nothing** — the error propagates as an `OverseerError` and the
in-process gate is not committed, so a later healthy cycle retries. Verify by
running with an unauthenticated / broken `gh` and confirming no phantom issues
are created and the failure is surfaced (not swallowed).

## How the check resolves a match

Tier 2 reuses the stewardship candidate-resolution and marker-matching helpers,
so it inherits their resilience to GitHub's eventually-consistent search index:

1. `search_issues(repo, "workstream-gap:<signature>")` runs a fast
   `stewardship-signature:<key> in:body` search **unioned** with a
   strongly-consistent scan of the newest 100 open issues.
2. `stewardship::dedup::find_existing` matches the open issue carrying that body
   marker. For gap keys the match is **line-bounded** — it matches the whole
   newline-terminated marker line, not a bare substring.
3. Match → reuse (`MatchedExisting`, counted in `reused_existing`); no match →
   `create_issue` with the marker on its own newline-terminated line in the body
   (`FiledNew`, counted in `flagged`).

The union step is what stops two sweeps inside the search-index window from both
seeing an empty search and double-filing.

> **Two composite-key caveats** (gap keys are multi-colon slugs like
> `workstream-gap:goal:g-1042`, unlike stewardship's fixed 16-hex signatures):
>
> - **Search is best-effort.** GitHub tokenizes on colons, so the `in:body`
>   search leg is weaker for gap keys than for stewardship signatures. The
>   **newest-100 `RecentOpen` scan is the authoritative net** — a covering issue
>   that has aged past the newest 100 *and* is not surfaced by the tokenized
>   search can escape reuse. This bounded residual is documented in the
>   [reference](../reference/overseer-gap-scan-durable-dedup.md#risks--design-constraints).
> - **Match is line-bounded.** Because `workstream-gap:goal:g-1042` is a
>   substring of `workstream-gap:goal:g-1042-extra`, the marker is written as its
>   own newline-terminated line and matched line-bounded so prefixed keys never
>   cross-match.

## Verify with tests

The hermetic gap-scan tests use a fake `GhClient` so no network is touched:

```bash
cargo test -p simard overseer::tests_gap_scan
```

The suite covers:

- **open-dup → no new file:** an existing open issue with the matching marker →
  the gap reuses it, `reused_existing` increments, `flagged` does not.
- **no-dup → files once:** a genuinely new gap files exactly one issue.
- **restart (fresh gate) → still dedups:** a cold in-process gate with the issue
  still open on GitHub reuses it.
- **gh search error → files nothing:** a failing search propagates and creates
  no issue (fail-loud).
- **prefix-collision safety:** a covering issue for `g-1042` must **not** match
  a distinct gap keyed `g-1042-extra` (line-bounded marker match).
- **composite-key match:** a multi-colon gap key
  (`workstream-gap:issue:rysweet/Simard#4717`) still matches its open covering
  issue via the authoritative `RecentOpen` scan.
- **burst suppression:** Tier 1 pre-filters a same-cycle burst before any `gh`
  call.
- **dedup-key stability / injection safety:** `GapItem::dedup_key()` is stable
  across runs and rejects malformed or shell-metacharacter-bearing signatures.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Duplicate covering issues still appear after a restart | `with_gap_issue_client` not wired | Confirm the daemon injects a `RealGhClient` when the gap-scan is enabled |
| A gap never reuses an open issue | Marker missing/mismatched in the existing issue body | Ensure the open issue carries `stewardship-signature: workstream-gap:<signature>`; a differently-keyed filer will not match |
| Whole cycle files nothing and logs an error | Fail-loud on a `gh` error (by design) | Fix `gh` auth/connectivity; the next healthy cycle retries |
| A brand-new gap is suppressed | Tier 1 `WhisperGate` window still open for that signature | Expected within the 900 s window; see [gap-scan backoff](./configure-overseer-gap-scan-backoff.md) |

## Related reading

- [Overseer gap-scan durable open-issue dedup reference](../reference/overseer-gap-scan-durable-dedup.md)
  — the typed API, the two-tier gate, and invariants.
- [Overseer workstream gap-scan reference](../reference/overseer-workstream-gap-scan.md)
  — the step this rail guards.
- [Configure and operate Overseer gap-scan backoff](./configure-overseer-gap-scan-backoff.md)
  — the in-process burst / backoff tier.
- [Goal Stewardship — Orchestrator Failure API Reference](../reference/stewardship-api.md)
  — the reused `GhClient` / dedup contract.
