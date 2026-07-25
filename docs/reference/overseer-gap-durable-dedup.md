---
title: Overseer durable gap-filing dedup reference
description: >
  The restart-safe, GitHub-side dedup guard on the Overseer stewardship
  gap-filing path — the durable open-issue equivalence check that stops the
  Overseer from re-filing a near-duplicate stewardship issue every tick and
  across daemon restarts and re-runs. Covers the bounded GapCategory taxonomy
  (GoalUncovered / IssueUncovered / AnomalyUnaddressed), the stable
  slug-validated signature grammar, the WhisperGate pre-filter →
  gh.search_issues → find_existing → reuse/skip/create flow, the
  `stewardship-signature:` body marker, the fail-loud contract, and the input
  validation / secret-redaction hardening applied before any public issue write.
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

# Overseer durable gap-filing dedup reference

The acting **Overseer** files stewardship issues for uncovered backlog work —
uncovered goals, high-signal open issues, and unaddressed telemetry anomalies.
The filed issue already carries a `stewardship-signature:` body
marker and the act path already searches GitHub for it — but before this rail
the signature was derived **per run** (`originating-run: overseer-<hash>`), so
every daemon start or re-run minted a **fresh** signature that matched no
existing issue. The in-process [`WhisperGate` / `BackoffGate`](./overseer-backoff-gate-api.md)
was the only other guard, and it lives in memory and is wiped on every daemon
restart. A restart (or a second daemon) therefore re-filed a gap that was
**already open** on GitHub — the near-duplicate `[stewardship] workstream_gap:*`
flood (observed on e.g. #4671, #4680, #4685).

This rail closes that seam. It makes the `stewardship-signature:` a **stable,
content-addressed** slug (derived from the gap's trusted identifiers, not the
run id) so the **already-existing** durable open-issue search actually matches
across runs: before filing, the Overseer searches the target repo for an
already-open issue carrying the gap's stable marker and, if one exists,
**reuses / comments / skips** instead of filing a new one. Because the source of
truth is the GitHub issue tracker (not process memory) *and* the key is now
stable across runs, the guarantee survives restarts and multiple daemons.

> **This is additive.** It layers a durable, content-addressed check **in front
> of** the existing in-process gates; it does not replace them and does not
> change how any other intervention decides or acts. The in-process
> `WhisperGate` remains the fast pre-filter (it avoids a `gh` round-trip on the
> common repeat-within-window case); the durable check is the restart-safe
> backstop. The issue-body **structure** and the `stewardship-signature:` marker
> *field* are reused unchanged; what changes is the marker's *value* — from an
> unstable per-run hash to a stable per-gap slug. Consequently, issues filed
> **before** this rail carry the old per-run hash signatures and will **not**
> dedupe against the new slug signatures; those pre-existing duplicates
> (#4671–#4689 range) are out of scope to retroactively reconcile.

> **Modules:** taxonomy + signature grammar `src/overseer/signal.rs`
> (`GapCategory`, `GapItem::signature`); batch key `src/overseer/mod.rs`
> (`workstream_gap_key`); detectors `src/overseer/sensor.rs`
> (`detect_workstream_gaps`) and `src/overseer/observer.rs`; rendering
> `src/overseer/notify.rs`; reused GitHub seam + redaction
> `src/stewardship/gh_client.rs`, `src/stewardship/dedup.rs`,
> `src/stewardship/mod.rs` (`sanitize_issue_text`, `find_existing`,
> `process_orchestrator_run` — the sibling filing loop this mirrors). Hermetic
> tests `src/overseer/tests_gap_scan.rs`, `src/stewardship/tests_extra.rs`.
>
> **Note on the filing seam:** the GitHub-issue filing that produced the flood
> is the stewardship seam (`filed-by: simard-stewardship`,
> `failed-step: workstream-gap-scan`, `source-module: overseer`), **not**
> `act_flag_workstream_gaps` (`src/overseer/mod.rs:1954`), which today only
> *notifies* the operator (email + Signal) and never calls `create_issue`. The
> durable, content-addressed guard must wrap the actual stewardship filing seam;
> `act_flag_workstream_gaps` keeps the in-process `WhisperGate` (900 s)
> pre-filter.

## At a glance

| You want to… | Use |
|---|---|
| Understand why duplicates stopped after a restart | This page — the durable open-issue check |
| See the stable key that dedupes a gap | `GapItem.signature` → `stewardship-signature: <sig>` in the issue body |
| Know which gap kinds are deduped | The bounded [`GapCategory` taxonomy](#the-bounded-gapcategory-taxonomy) |
| Confirm a duplicate was reused, not re-filed | `overseer::gap_scan` `reused_existing=…` info log + `ActOutcome::WorkstreamGapsFlagged` |
| Operate / verify it | [How to configure and verify durable gap dedup](../howto/configure-gap-durable-dedup.md) |

## The bounded `GapCategory` taxonomy

Free-form gap titles were the root of the flood: the same underlying condition
produced slightly different strings on each tick, so no two matched an existing
open issue. The taxonomy replaces free-form detection with a **bounded enum**;
every variant carries a short, stable `.label()` slug that anchors the dedup
signature.

| `GapCategory` variant | `.label()` | Signature prefix | Filed for |
|---|---|---|---|
| `GoalUncovered` | `goal` | `goal:<goal_id>` | A p1/p2 active goal with no engineer and no PR |
| `IssueUncovered` | `issue` | `issue:<repo>#<n>` | A high-signal open issue with no PR / workstream |
| `AnomalyUnaddressed` | `anomaly` | `anomaly:<slug>` | A live telemetry anomaly with no fix in flight |

The taxonomy is a **bounded, closed enum** (`src/overseer/signal.rs`), so a gap
can only ever resolve to one of these three stable `.label()` slugs — the same
underlying condition renders an **identical** title and signature on every tick.
The durable change is not new variants but a **stable signature**: the filed
`stewardship-signature:` moved from an unstable per-run *hash* to the
content-addressed per-gap slug below, so the already-existing open-issue search
now matches the same gap across runs and restarts. Every exhaustive `match` on
`GapCategory` (in `notify.rs`, `sensor.rs`, `observer.rs`) already covers these
three arms; **none was renamed**. (Pre-fix filed issues carried a per-run hash
signature, not these slugs, so they do not retroactively dedupe — see the format
note above.)

## The dedup key (signature) grammar

Each `GapItem` carries a `signature`: a **stable, constrained slug** built from
**trusted identifiers only** (a goal id, `repo#number`, or an anomaly slug) —
never from hostile free text. Identical inputs yield an identical signature, so
a recurring gap dedupes to at most one open issue.

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

Signatures are validated **at construction** in `signal.rs`, not at the call
sites, so a malformed or hostile identifier can never reach a `gh search`
argument or an issue body. A valid signature matches:

```text
^[a-z0-9][a-z0-9:_#.\-/]{0,200}$
```

Any identifier that would violate this (control characters, spaces, shell
metacharacters, over-length) is slugified/rejected at the boundary. This makes
the search query **inert**: a goal id such as `g-1; rm -rf ~` cannot inject into
the `gh issue list --search` term because it never survives slug validation.

## The durable filing flow

The gap-filing act path (`act_flag_workstream_gaps`) mirrors the proven
[`stewardship::process_orchestrator_run`](./stewardship-api.md) dedup flow:

```text
for each detected gap:
  1. WhisperGate.peek(sig)         # in-process fast pre-filter (no gh call)
        └─ SuppressDuplicate/Cap → count as suppressed, continue
  2. gh.search_issues(repo, sig)   # DURABLE open-issue equivalence check
        └─ Err(e)                  → FAIL LOUD: propagate, file nothing
  3. find_existing(&issues, sig)   # match `stewardship-signature: <sig>`
        └─ Some(issue)             → reuse/comment/skip (no new issue)
        └─ None                    → gh.create_issue(repo, title, body)
                                       body embeds `stewardship-signature: <sig>`
  4. WhisperGate.commit(sig)       # record the fresh filing in-process
```

Key properties:

- **Restart-safe.** Step 2 queries GitHub, the durable source of truth, so a
  cold in-process gate after a restart still finds the already-open issue and
  reuses it. The guarantee is now *"at most one open issue per distinct gap
  signature"* **across restarts and daemons**, strengthening the previous
  *within one process* guarantee.
- **Reuses the frozen `GhClient` seam.** No new methods, scopes, or endpoints —
  the durable check composes the existing `search_issues` / `create_issue`
  trait. `create_issue` still pipes the body via `--body-file -` on stdin, so
  argv length and shell quoting are non-issues.
- **The `stewardship-signature:` marker is the join key.** `find_existing`
  matches an open issue whose body contains `stewardship-signature: <sig>` —
  the same marker and matcher the orchestrator-failure loop uses.

## Fail-loud contract

If the durable open-issue search errors (`gh` unauthenticated, rate-limited,
network down), the act path **propagates the error and files nothing**. It never
falls back to a blind `create_issue` — a fail-*open* fallback would reintroduce
exactly the flood this rail prevents.

- No `unwrap`/`expect` on the `gh` paths; every fallible step returns
  `Result<_, OverseerError>` / `SimardResult<_>`.
- A search error surfaces as a first-class `OverseerError` and is
  `tracing::error!`-logged on `target: "overseer::gap_scan"`; it is **not**
  swallowed.
- Contrast with the in-process `BackoffGate`, which fails **toward surfacing**
  (a clock regression resolves to *admit*). Both directions err on the side of
  *not* silently dropping a genuine gap — the durable check refuses to file on
  uncertainty, and the in-process gate refuses to hide on uncertainty.

## Input validation & secret hygiene (before any issue write)

Every value that reaches a `gh` argument or a public issue body is treated as
untrusted and hardened, reusing the stewardship helpers:

- **Repo identifier** is validated against `^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$`
  and every `gh` invocation uses argv (no shell) with a `--` argument
  terminator, so a repo or title beginning with `-` cannot be read as a flag.
- **Titles** are truncated to ≤120 characters and stripped of control
  characters (`\x00`–`\x1F` except `\n`/`\t`) to prevent log/notification
  injection.
- **Body detail** derived from logs is passed through
  `stewardship::sanitize_issue_text` (see `src/stewardship/mod.rs`), which
  redacts `ghp_`/`gho_` GitHub tokens, AWS keys, `Bearer` tokens, and PEM
  blocks to `[redacted secret]` **before** the text can enter a public issue
  body.

## Structured observability

The rail emits structured `tracing` + OTel only — **no** `print!`/`println!`.
On `target: "overseer::gap_scan"`:

| Field | Meaning |
|---|---|
| `flagged` | Fresh gaps filed this tick (new issues created) |
| `reused_existing` | Gaps that matched an already-open issue and were reused |
| `suppressed` | Gaps dropped by the in-process `WhisperGate` pre-filter |
| `key` | The batch `workstream_gap_key` for the tick |

The `ActOutcome::WorkstreamGapsFlagged { flagged, suppressed }` outcome and the
`OverseerTickReport.workstream_gaps_detected` / `…_suppressed` counters are
unchanged and additive; reused-existing hits count as suppressed for the tick
totals (no new issue was created).

## Guarantees

- **At most one open issue per distinct gap signature**, across daemon
  restarts and concurrent daemons (durable, GitHub-sourced).
- **Never permanently silent.** If the covering issue is closed and the gap
  recurs, the `--state open` search finds nothing and a fresh issue is filed —
  identical to the stewardship loop's semantics.
- **Additive / non-breaking.** A durable, content-addressed check in front of
  the existing gates; every current caller and the issue-body *structure* are
  untouched. The `stewardship-signature:` marker field is reused; only its *value* changes (unstable per-run hash →
  stable per-gap slug), so pre-fix duplicate issues do not retro-dedupe. The PRD
  (`Specs/ProductArchitecture.md` § *Stewardship Mode* / § *Goal Stewardship
  Mode*) is preserved.
- **Fail-loud, never fail-open.** A `gh` search error stops filing; it never
  degrades to a blind create.

## Security notes

| Risk | Mitigation |
|---|---|
| Search-query injection via an unconstrained signature | Slug-validate the signature at construction (`^[a-z0-9][a-z0-9:_#.\-/]{0,200}$`); the search term is inert |
| Fail-open regression re-introducing the flood | Propagate `search_issues` errors; never default to `create_issue` |
| Secret disclosure in a public issue body | `sanitize_issue_text` redacts token/key/PEM patterns before the write |
| `gh` flag-injection via a repo/title beginning with `-` | Repo regex validation + `--` argument terminator; argv (no shell) |
| Self-DoS from a fast detector loop | Mandatory `WhisperGate` pre-filter before any `gh` call |
| Log/notification injection via control chars | Strip `\x00`–`\x1F` (except `\n`/`\t`); truncate titles ≤120 chars |

No credential handling lives on this path: `gh` supplies its own token, which is
never read, logged, or embedded. The rail needs only `issues:write` — no new
scopes or endpoints.

## Related

- [Overseer workstream gap-scan reference](./overseer-workstream-gap-scan.md) —
  the `GapItem`/`GapCategory`/`Signal::WorkstreamGap` data model this extends.
- [Gap-scan dedup & exponential backoff](../concepts/gap-scan-backoff-dedup.md)
  — the in-process `BackoffGate` this rail's durable check backstops.
- [Overseer backoff-gate API reference](./overseer-backoff-gate-api.md) — the
  in-process pre-filter.
- [Goal Stewardship — Orchestrator Failure API reference](./stewardship-api.md)
  — the `search_issues → find_existing → create_issue` flow this mirrors.
- [How to configure and verify durable gap dedup](../howto/configure-gap-durable-dedup.md).
- PRD: `Specs/ProductArchitecture.md` § *Stewardship Mode* / § *Goal
  Stewardship Mode*.
