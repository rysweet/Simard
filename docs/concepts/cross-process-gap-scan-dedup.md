---
title: Cross-process gap-scan dedup
description: >
  Why the Overseer's gap-scan seeds its coverage set from currently-open,
  signature-stamped GitHub issues before it decides — so a daemon restart no
  longer relaunches a duplicate coverage workstream (and the "Cover uncovered
  backlog workstream(s)" issue that workstream opens) for a gap that is already
  tracked (#4340/#4341, #4337/#4338, and the #4297/#4301/#4304/#4306/#4316
  cluster). Explains why the in-memory coverage BackoffGate alone could not
  survive a restart, how reusing the existing `stewardship-signature` dedup
  contract closes the cross-process seam, and why the change is fail-safe toward
  surfacing.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: concept
status: reference
related:
  - ./gap-scan-backoff-dedup.md
  - ../reference/gap-scan-open-issue-coverage.md
  - ../reference/overseer-workstream-gap-scan.md
  - ../reference/overseer-backoff-gate-api.md
  - ../reference/stewardship-api.md
  - ../howto/diagnose-duplicate-gap-scan-issues.md
  - ../howto/review-overseer-workstream-gaps.md
  - ./operational-autonomy-model.md
  - ../design/overseer.md
---

# Cross-process gap-scan dedup

> **Operator symptom (Overseer gap-scan duplicates):** the gap-scan emitted
> near-identical GitHub issues on `rysweet/Simard` —
> [#4340](https://github.com/rysweet/Simard/issues/4340) /
> [#4341](https://github.com/rysweet/Simard/issues/4341) both *"Bake a FUNDAMENTAL
> DESIGN PRINCIPLE…"*, and
> [#4337](https://github.com/rysweet/Simard/issues/4337) /
> [#4338](https://github.com/rysweet/Simard/issues/4338) both *"Cover uncovered
> backlog… goal:harden-amplihack-rs-recipes-tool"* — part of a recurring cluster
> ([#4297](https://github.com/rysweet/Simard/issues/4297),
> [#4301](https://github.com/rysweet/Simard/issues/4301),
> [#4304](https://github.com/rysweet/Simard/issues/4304),
> [#4306](https://github.com/rysweet/Simard/issues/4306),
> [#4316](https://github.com/rysweet/Simard/issues/4316)).

The [gap-scan backoff rail](./gap-scan-backoff-dedup.md) already suppresses
duplicate gap-cover work **within one running daemon**: an in-memory coverage
`BackoffGate` (`coverage_backoff`) keyed by each gap's stable signature holds the
duplicate coverage launch across ticks. That doc explicitly named the remaining
seam as **future work**:

> The BackoffGate is in-memory, so a daemon restart forgets its state and a cold
> gate could re-file a duplicate that is already open on GitHub.

This is that seam, and this is its fix. **Cross-process gap-scan dedup** makes the
gap-scan check GitHub itself — not just process memory — before filing.

## Why the in-memory gate could not close this

The `detect_workstream_gaps` detector is deliberately internal: it surveys
Simard's **own** in-memory goal board and anomaly list and never calls `gh`. Its
dedup input is a `coverage: &[String]` slice of already-covered signatures — and
the gap-scan wiring passed an **empty slice**:

```rust
// wiring.rs — before: no cross-process coverage
Ok(detect_workstream_gaps(&board, &[], anomalies, &[]))
//                                              ^^^ empty coverage
```

With an empty coverage set, the only thing standing between a re-detected gap and
a fresh duplicate was the in-memory coverage `BackoffGate` (`coverage_backoff`).
Across a **restart** (or between two independently-launched observe passes) that
memory is gone, the gate is cold, and the same still-open gap relaunches a
coverage workstream that opens a second, third, … *"Cover uncovered backlog
workstream(s)"* issue — `#4340`/`#4341`, `#4337`/`#4338`, and the rest of the
cluster.

## The fix: seed coverage from open, signed issues

The gap-scan's closing edge for an uncovered gap is a **`WORKSTREAM_COVERAGE_GROUP`
recipe launch** (`overseer/mod.rs`, `ProblemKind::WorkstreamCoverage`): the
Overseer launches a workstream that *covers* the gap, and that launched workstream
opens the *"Cover uncovered backlog workstream(s)"* GitHub issue. The cross-process
fix seeds a **coverage set** so that even a *cold* `coverage_backoff` — after a
restart — declines to relaunch a gap that already has an open covering issue. It
reuses the *existing* `stewardship-signature` dedup contract rather than inventing
a parallel key:

1. **Stamp on cover.** The launched coverage workstream embeds
   `stewardship-signature: workstream-gap:<sig>` in the covering issue it opens,
   where `<sig>` is the gap's stable signature carried in the recipe brief —
   `goal:<id>`, `issue:<repo>#<n>`, or `anomaly:<slug>`.
2. **Read before you decide.** Before detecting gaps, the Observe/wiring layer
   lists open issues, extracts their `workstream-gap:<sig>` signatures, and passes
   that set as the `coverage` argument to `detect_workstream_gaps`. A gap whose
   signature is already covered by an open issue is suppressed **at detection** —
   no `WorkstreamGap` signal, no `WorkstreamCoverage` launch, and therefore no
   second covering issue.
3. **Reuse, don't reinvent.** The lookup uses the shipped
   [`stewardship::dedup`](../reference/stewardship-api.md) helpers (`find_existing`
   / the signature `search_issues` path) — the same contract the failure-issue
   dedup already relies on. There is no second dedup key and no new grammar.

Because the signature is **stable and process-independent**, this closes the gap
the in-memory gate could not: two identical gap-scan passes across a restart now
produce **exactly one** covering workstream and **exactly one** covering issue.

## Hardening the write and read halves (#4353)

The design above pinned **one** implementation contract on a component the
Overseer does not control in code: step 1 asked the *launched* coverage workstream
to stamp the issue it opens. In practice that write half was carried only by the
model brief, so a run that opened the *"Cover uncovered backlog workstream(s)"*
issue **without** copying the `stewardship-signature: workstream-gap:<sig>` line
produced an unstamped issue the next scan could not read back — and the gap
re-emitted (the #4297/#4301/#4304/#4306/#4316/#4337/#4338 cluster). Two
deterministic guarantees close that residual seam:

- **F2 — deterministic write half.** When the Overseer launches a coverage
  workstream for a `WorkstreamGap`, it *also* files a small, code-owned **coverage
  anchor** issue itself — title
  `[Overseer] gap-scan coverage anchor (cross-process dedup)` — whose body is built
  in code by `build_gap_coverage_issue_body` and carries one
  `stewardship-signature: workstream-gap:<sig>` line per covered gap. The write is
  produced by the *same* formatter the reader parses (`gap_coverage_stamp_line` ↔
  `extract_gap_coverage_signatures`), so the write↔read grammar cannot drift, and
  it is **idempotent**: `ensure_coverage_stamp` first reads the open coverage set
  and only files anchors for gaps that are not already stamped, so a repeat tick
  files nothing. The stamp no longer depends on a launched agent remembering to
  copy a line.
- **F4 — trusted-author scope.** The read half now accepts a coverage stamp only
  when the carrying issue also bears a trusted provenance marker —
  `filed-by: simard-overseer`, matched line-exact by `body_has_filed_by`. A crafted
  issue that carries a valid-looking `workstream-gap:<sig>` stamp but is **not**
  filed by the stewardship bot cannot seed coverage and therefore cannot suppress a
  real gap (dedup poisoning is neutralised). The Overseer's own anchors carry the
  marker deterministically via the F2 formatter, so legitimate coverage still
  suppresses as before.

Together, F2 makes the dedup stamp appear **without** relying on a model, and F4
ensures only the Overseer's own stamps are trusted to suppress a gap — so a repeat
tick over an already-covered gap re-emits **nothing**.

> **Where the boundary sits.** `detect_workstream_gaps` is a pure detector that
> reads only Simard's in-memory board and **never calls `gh`**. The open-issue
> query therefore runs in the Observe/wiring layer, which already holds a `gh`
> client; only the extracted signatures cross into the detector, so the detector
> stays `gh`-free (its "never calls `gh`" invariant is preserved). Because seeding
> suppresses the gap *upstream* of the closing edge, whichever edge would follow —
> the coverage recipe launch, and the issue that workstream would open — never
> fires. The one implementation contract this design pins down is step 1: the
> covering workstream **must** stamp the issue with
> `stewardship-signature: workstream-gap:<sig>` (using the signature already in
> its recipe brief) so the next pass can read it back.

## Fail-safe toward surfacing

The cross-process check is **best-effort** and degrades safely:

- If the open-issue query fails (a `gh` error, rate limit, or malformed JSON), the
  scan **falls back to the in-memory coverage `BackoffGate`** rather than either
  relaunching duplicate coverage or silently dropping a real gap. Ambiguity
  resolves toward surfacing the gap once, never toward muting it.
- Signatures are **advisory dedup only**: the extractor validates the signature
  grammar (`goal:<id>` / `issue:<repo>#<n>` / `anomaly:<slug>`) and ignores
  malformed or forged stamps, so a crafted issue body cannot suppress a legitimate
  gap.
- The ingested open-issue count is **bounded/paginated** so an issue flood cannot
  grow the coverage set without limit.

This mirrors the backoff rail's posture: the dedup layer can only ever *remove* a
duplicate coverage launch (and the issue that launch would open) — it can never
authorise a launch, a merge, or an issue the objective gates would otherwise
block.

## Relationship to the backoff rail

The two layers compose, they do not replace each other:

| Layer                                     | Scope                       | Survives restart? |
| ----------------------------------------- | --------------------------- | ----------------- |
| In-memory coverage `BackoffGate` (#4186)  | within one daemon process   | no                |
| Open-issue coverage set (this)            | across processes / restarts | yes               |

The coverage `BackoffGate` still rate-limits re-launch *within* a process; the
open-issue coverage set stops a *cold* gate (after a restart) from relaunching
coverage for a gap that is already tracked by an open issue. Together they give the
invariant the backoff doc could previously only promise per-process: **at most one
open covering issue per distinct gap, across restarts.**

## Invariants

- **One open covering issue per distinct gap, across restarts.** Coverage is
  seeded from open signature-stamped issues, so a cold gate cannot relaunch
  coverage for a tracked gap.
- **Deterministic, idempotent write half (#4353 F2).** The coverage stamp is
  filed by the Overseer in code (`ensure_coverage_stamp` /
  `build_gap_coverage_issue_body`) using the reader's own formatter — never left
  to a launched agent — and a repeat tick over an already-stamped gap files
  nothing.
- **Trusted-author scope (#4353 F4).** A coverage stamp suppresses a gap only when
  its issue also carries the `filed-by: simard-overseer` provenance marker, so a
  forged stamp from an untrusted author cannot poison the dedup set.
- **Reuse the stewardship-signature contract.** Dedup uses the existing
  `workstream-gap:<sig>` stamp and `stewardship::dedup` helpers — no parallel key.
- **Fail toward surfacing.** A `gh` query failure degrades to the in-memory
  coverage `BackoffGate`; a gap is never silently dropped.
- **Advisory, validated signatures.** Malformed/forged stamps are ignored; the
  coverage set is bounded.
- **Additive / non-breaking.** The coverage argument was already part of
  `detect_workstream_gaps`; the change populates it instead of passing `&[]`.

## Related reading

- [Gap-scan open-issue coverage reference](../reference/gap-scan-open-issue-coverage.md)
  — the wiring, the `workstream-gap:<sig>` stamp, and the `stewardship::dedup`
  reuse.
- [Gap-scan dedup & exponential backoff](./gap-scan-backoff-dedup.md) — the
  in-process rail this layer extends across restarts.
- [Overseer workstream gap-scan reference](../reference/overseer-workstream-gap-scan.md)
  — the `Signal::WorkstreamGap` data model and the detector.
- [Diagnose duplicate gap-scan issues](../howto/diagnose-duplicate-gap-scan-issues.md)
  — the operator playbook for the symptom.
