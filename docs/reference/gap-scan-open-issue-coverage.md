---
title: Gap-scan open-issue coverage reference
description: >
  The typed surface of the Overseer gap-scan's cross-process dedup: how the
  workstream_gaps wiring seeds detect_workstream_gaps' `coverage` slice from
  open, signature-stamped GitHub issues (`workstream-gap:<sig>`) via the existing
  `stewardship::dedup` helpers in `src/stewardship/dedup.rs`, how the covering
  issue is stamped, the fail-safe degrade to the in-memory coverage BackoffGate,
  and the signature grammar it validates.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/cross-process-gap-scan-dedup.md
  - ../concepts/gap-scan-backoff-dedup.md
  - ./overseer-workstream-gap-scan.md
  - ./overseer-backoff-gate-api.md
  - ./stewardship-api.md
  - ../howto/diagnose-duplicate-gap-scan-issues.md
  - ../design/overseer.md
---

# Gap-scan open-issue coverage reference

> **Status: implemented.** The gap-scan wiring in
> [`src/overseer/wiring.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/wiring.rs)
> now seeds `detect_workstream_gaps`'
> ([`src/overseer/sensor.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/sensor.rs))
> `coverage` slice from open, signature-stamped GitHub issues via
> `BoardGoalCurator::open_gap_coverage_signatures`, reusing the
> `stewardship-signature` contract in
> [`src/stewardship/dedup.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/dedup.rs).
> This extends the in-memory coverage
> [BackoffGate](./overseer-backoff-gate-api.md) across daemon restarts — the
> cross-process follow-on the [backoff rail](../concepts/gap-scan-backoff-dedup.md)
> named as future work. For the rationale see
> [cross-process gap-scan dedup](../concepts/cross-process-gap-scan-dedup.md).

## The detector contract (unchanged shape)

`detect_workstream_gaps` already accepts a `coverage` slice — a set of signatures
already covered by in-flight work — and dedups any candidate whose signature is in
it:

```rust
pub fn detect_workstream_gaps(
    board: &GoalBoard,
    issues: &[SurveyedIssue],
    anomalies: &[String],
    coverage: &[String],   // signatures already covered ⇒ never re-flagged
) -> Vec<GapItem>;
```

Each `GapItem` carries a **stable signature** derived by category:

| Gap category         | Signature form        | Example                                   |
| -------------------- | --------------------- | ----------------------------------------- |
| `GoalUncovered`      | `goal:<id>`           | `goal:harden-amplihack-rs-recipes-tool`   |
| Issue (uncovered)    | `issue:<repo>#<n>`    | `issue:rysweet/Simard#4316`               |
| Anomaly (uncovered)  | `anomaly:<slug>`      | `anomaly:coverage-comment-timeout`        |

`is_covered(sig)` inside the detector is a simple membership test against
`coverage`, so populating `coverage` with the right signatures is all that is
needed to suppress a duplicate.

## The wiring change: populate `coverage`

Previously the wiring passed an **empty** coverage slice:

```rust
// before
Ok(detect_workstream_gaps(&board, &[], anomalies, &[]))
```

The `workstream_gaps` capability now builds the coverage set from currently-open,
signature-stamped issues before detecting:

```rust
// after — seed cross-process coverage from open, signed issues
let coverage = self.open_gap_coverage_signatures();   // best-effort; see below
Ok(detect_workstream_gaps(&board, &[], anomalies, &coverage))
```

`open_gap_coverage_signatures`:

1. Lists open issues (bounded / paginated) via the shared `gh` issue client.
2. Extracts every embedded `stewardship-signature: workstream-gap:<sig>` stamp.
3. Validates each `<sig>` against the grammar (`goal:<id>` / `issue:<repo>#<n>` /
   `anomaly:<slug>`) and discards malformed/forged stamps.
4. Returns the validated signature set.

> **`gh`-boundary decision.** `detect_workstream_gaps` is a pure detector: it
> reads only Simard's in-memory board and *"never calls `gh`, never parses
> issue/PR JSON"*. Seeding coverage from open issues crosses that boundary, so the
> open-issue fetch runs **outside** the detector — in the wiring/capability layer
> or the Observe pass, which already hold a `gh` client — and only the extracted
> signatures are passed in via `coverage`. `detect_workstream_gaps` stays
> `gh`-free. Update the stale *"never calls `gh`"* comment on the wiring capability
> to reflect that the *wiring* now fetches issues while the *detector* still does
> not.

## Stamping the covering issue

The gap-scan's closing edge is a `WORKSTREAM_COVERAGE_GROUP` **recipe launch**, not
a direct issue write: `act` launches a coverage workstream (see
`ProblemKind::WorkstreamCoverage` in
[`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)),
and that launched workstream opens the *"Cover uncovered backlog workstream(s)"*
issue. For the next pass to recognise that coverage, the covering issue must embed
the gap signature in its body:

```text
stewardship-signature: workstream-gap:<sig>
```

This is the **same** embed shape `stewardship::dedup::find_existing` already scans
for:

```rust
/// Find the first issue whose body embeds `stewardship-signature: <sig>`.
pub fn find_existing<'a>(issues: &'a [GhIssue], signature: &str) -> Option<&'a GhIssue> {
    let needle = format!("stewardship-signature: {signature}");
    issues.iter().find(|i| i.body.contains(&needle))
}
```

The recipe brief already carries each gap as `category: signature` (built in
`ProblemKind::WorkstreamCoverage`), so the launched workstream has the exact
`<sig>` to stamp. On the next pass, `open_gap_coverage_signatures` reads that stamp
back into `coverage`, `detect_workstream_gaps` treats the gap as covered, and the
coverage launch is suppressed **at detection** — so no duplicate workstream is
launched and no duplicate issue is opened. No parallel dedup key is introduced:
`workstream-gap:<sig>` is a `stewardship-signature` value like any other, matched
by the same `find_existing` / `search_issues` path.

## Fail-safe degrade

The open-issue query is **best-effort**:

- **`gh` query fails** (error, rate limit, malformed JSON) → the coverage set
  degrades to empty and the scan relies on the in-memory coverage
  [`BackoffGate`](./overseer-backoff-gate-api.md). A gap is **never silently
  dropped**; ambiguity resolves toward surfacing it once.
- **Malformed/forged signature** → ignored (advisory dedup only), so a crafted
  issue body cannot suppress a legitimate gap.
- **Issue flood** → the ingested open-issue count is bounded/paginated, so the
  coverage set cannot grow without limit.

All failures are surfaced through structured `tracing` on the
`overseer::gap_scan` target (never `print!`/`println!`).

## Testing

`src/overseer/tests_gap_scan.rs` pins the cross-process contract:

- **Two-pass single-launch** — running the gap-scan twice across a **simulated
  restart** (fresh in-memory state, but the first pass's stamped issue present in
  the open-issue set) detects the gap **only once**: the second pass sees the
  `workstream-gap:<sig>` stamp in `coverage`, emits no `WorkstreamGap` signal, and
  launches no second coverage workstream — so exactly one covering issue exists.
- **Degrade-on-error** — a failing open-issue query falls back to the in-memory
  coverage gate and still surfaces an uncovered gap (never drops it).
- **Malformed-signature rejection** — an issue with a bad `workstream-gap:` stamp
  does not suppress a real gap.

## Invariants

- **Coverage seeded from open, signed issues.** The wiring passes a populated
  `coverage` slice, not `&[]`.
- **One open covering issue per distinct gap, across restarts.** A stamped open
  issue covers its signature on subsequent passes and processes.
- **Reuse the stewardship-signature contract.** `workstream-gap:<sig>` +
  `stewardship::dedup::find_existing` — no new key or grammar.
- **Fail toward surfacing.** A query failure degrades to the in-memory coverage
  gate; gaps are never silently dropped.
- **Validated, bounded.** Signatures are grammar-checked; ingested issue count is
  bounded.

## Related reading

- [Cross-process gap-scan dedup](../concepts/cross-process-gap-scan-dedup.md) —
  the *why* and the duplicate-issue clusters it fixes.
- [Overseer BackoffGate & gap-scan dedup reference](./overseer-backoff-gate-api.md)
  — the in-process rail this coverage set extends across restarts.
- [Overseer workstream gap-scan reference](./overseer-workstream-gap-scan.md) —
  the `Signal::WorkstreamGap` data model and detector.
- [Stewardship API reference](./stewardship-api.md) — the `stewardship::dedup`
  helpers reused here.
