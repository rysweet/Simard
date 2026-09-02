---
title: Overseer gap-scan durable open-issue dedup reference
description: >
  The durable, GitHub-side open-issue dedup rail that stops the Overseer's
  workstream gap-scan from re-filing an already-open covering issue across
  process restarts (#4717). Documents the two-tier gate (in-process WhisperGate
  burst pre-filter + strongly-consistent GitHub open-issue check), the
  GapItem::dedup_key signature contract, the with_gap_issue_client wiring seam,
  the extended ActOutcome::WorkstreamGapsFlagged counters, the
  stewardship-signature body marker, the fail-loud error contract, and the
  observability surface.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference
related:
  - ./overseer-workstream-gap-scan.md
  - ../concepts/gap-scan-backoff-dedup.md
  - ./stewardship-api.md
  - ../howto/configure-overseer-gap-scan-durable-dedup.md
  - ../howto/review-overseer-workstream-gaps.md
  - ../design/overseer.md
  - ../concepts/operational-autonomy-model.md
---

# Overseer gap-scan durable open-issue dedup reference

> **Closes [#4717](https://github.com/rysweet/Simard/issues/4717).** The
> Overseer's workstream gap-scan now performs a **durable, GitHub-side**
> open-issue dedup before it files a covering issue for a backlog-coverage gap.
> A cold daemon (fresh in-process state after a restart) that re-detects a gap
> whose covering issue is already **open** on GitHub reuses that issue instead
> of filing a duplicate — closing the seam that produced the repeated
> `[stewardship] workstream_gap:*` bursts
> ([#4726](https://github.com/rysweet/Simard/issues/4726)–[#4730](https://github.com/rysweet/Simard/issues/4730)).

The [workstream gap-scan](./overseer-workstream-gap-scan.md) surveys the whole
work picture each tick and, for each uncovered backlog gap, notifies the
operator and files a covering issue. Before this rail, the "already filed?"
check was **in-process only** (the `WhisperGate` dedup window), so a restart
lost that memory and the next scan re-filed an issue that was still open on
GitHub. This page documents the rail that makes the "one open covering issue
per distinct gap" guarantee survive restarts by treating **GitHub itself as the
source of truth**.

> **Modules:** dedup key `src/overseer/signal.rs` (`GapItem::dedup_key`);
> act path + wiring seam `src/overseer/mod.rs`
> (`act_flag_workstream_gaps`, `Overseer::with_gap_issue_client`,
> `ActOutcome::WorkstreamGapsFlagged`); GitHub client + candidate resolution
> reused from `src/stewardship/gh_client.rs`
> (`GhClient`, `RealGhClient`, `resolve_dedup_candidates`); the line-bounded
> marker matcher is `open_issue_has_marker` in `src/overseer/mod.rs`;
> daemon wiring + counters `src/overseer/wiring.rs`; activity rendering
> `src/overseer/activity.rs`. Hermetic tests `src/overseer/tests_gap_scan.rs`.

## At a glance

| You want to… | Use |
|---|---|
| Turn on durable dedup | Inject a `GhClient` via `Overseer::with_gap_issue_client(..)` (wired automatically in the daemon when the gap-scan is enabled) |
| See how many gaps reused an already-open issue | `ActOutcome::WorkstreamGapsFlagged { reused_existing }` / tick counter `workstream_gaps_reused_existing` / tracing field `reused_existing` on target `overseer::gap_scan` |
| Find the stable dedup key for a gap | `GapItem::dedup_key()` → `workstream-gap:<signature>` |
| Find the marker in a filed issue | own body line `stewardship-signature: workstream-gap:<signature>` (newline-terminated, matched line-bounded) |
| Understand the in-process burst guard | The [`WhisperGate`](../concepts/gap-scan-backoff-dedup.md) (900 s window) still runs first |

## The two-tier gate

`act_flag_workstream_gaps` gates every observed gap through **two** independent
tiers, in order. A gap must clear **both** to result in a newly-filed issue.

```
observed gaps
   │
   ▼
Tier 1 — WhisperGate.peek (in-process, same-cycle burst pre-filter)
   │   suppress duplicates seen within the 900 s window / cap
   ▼  fresh gaps only
Tier 2 — durable GitHub open-issue check (per fresh gap)
   │   gh.search_issues(repo, dedup_key)  → find_existing
   │     MatchedExisting → reuse (skip create), count reused_existing
   │     no match        → create_issue with signature body marker (FiledNew)
   ▼
commit gate + ONE consolidated operator notification for genuinely-new gaps
```

**Tier 1 — in-process burst pre-filter (unchanged).** The existing
`WhisperGate` (`gap_gate`, 900 s window, 200-entry cap) suppresses the same
signature within a single running process. This is a cheap first line that
prevents a same-cycle rapid-fire burst from issuing a storm of `gh` queries.
It is **retained** by this rail, not replaced.

**Tier 2 — durable GitHub open-issue check (new).** For each gap that clears
Tier 1, the Overseer queries GitHub for an already-open covering issue keyed by
the gap's stable signature. If one exists, the gap **reuses** it (no new issue,
no phantom re-notification); if none exists, it files one. Because the query
hits GitHub — not in-memory state — the check is correct after a restart.

> **Ordering matters.** Tier 1 runs before any `gh` call so a burst cannot
> trigger a query storm. Tier 2's durable check is what makes the guarantee
> survive restarts.

## Dedup key: `GapItem::dedup_key`

```rust
impl GapItem {
    /// The stable, durable dedup key for this gap:
    /// `workstream-gap:<signature>`.
    ///
    /// `signature` is already a content-addressed, injection-safe slug
    /// (`goal:<id>` / `issue:<repo>#<n>` / `anomaly:<slug>`), so identical
    /// gaps yield identical keys across process restarts. The key is validated
    /// as a bounded slug (`[A-Za-z0-9:_#/.-]`, length-capped) before use; a
    /// malformed signature fails loud rather than widening the search or
    /// forging a marker.
    pub fn dedup_key(&self) -> String;
}
```

`dedup_key` is the **single source** of the durable key. It replaces the two
former inline `format!("workstream-gap:{}", g.signature)` call sites in
`act_flag_workstream_gaps`, so the key used for the in-process gate, the GitHub
search, and the issue body marker can never drift apart.

| Gap category | Example `signature` | `dedup_key()` |
|---|---|---|
| High-priority goal | `goal:g-1042` | `workstream-gap:goal:g-1042` |
| High-signal issue | `issue:rysweet/Simard#4717` | `workstream-gap:issue:rysweet/Simard#4717` |
| Live anomaly | `anomaly:overseer-restart-loop` | `workstream-gap:anomaly:overseer-restart-loop` |

## Wiring seam: `with_gap_issue_client`

Durable dedup is **opt-in and additive**. The Overseer gains one optional field
and one builder; when the field is unset the gap-scan behaves exactly as before
(notify + in-process dedup only).

```rust
pub struct Overseer {
    // …existing fields…

    /// Optional GitHub client used for the durable open-issue dedup check in
    /// the gap-scan act path. `None` → notify-only, in-process dedup
    /// (unchanged legacy behaviour).
    gap_issue_client: Option<Box<dyn GhClient>>,
}

impl Overseer {
    /// Inject the GitHub client that backs the durable gap-scan dedup check.
    /// Additive builder, mirrors `with_operator_notifier`.
    pub fn with_gap_issue_client(mut self, gh: Box<dyn GhClient>) -> Self;
}
```

The `GhClient` trait and its production implementation `RealGhClient` are
**reused unchanged** from `stewardship::gh_client` — the same argv-safe,
body-via-stdin, injection-hardened client the stewardship failure loop uses.
See [`stewardship-api.md`](./stewardship-api.md) for the trait surface.

### Daemon wiring

`src/overseer/wiring.rs` constructs a `RealGhClient` and passes it via
`with_gap_issue_client` whenever the gap-scan is enabled. The **target repo is
sourced from trusted config only** (never from `GapItem` data), which prevents
any gap-derived value from re-targeting issue creation at another repo.

```rust
let overseer = Overseer::new(/* … */)
    .with_operator_notifier(notifier)
    .with_gap_issue_client(Box::new(RealGhClient::new()));
```

## Outcome & counters: `ActOutcome::WorkstreamGapsFlagged`

The act outcome gains a third counter so durable reuse is observable end to end.

```rust
pub enum ActOutcome {
    // …
    WorkstreamGapsFlagged {
        /// Genuinely-new gaps that filed a fresh covering issue this cycle.
        flagged: usize,
        /// Gaps suppressed by the in-process WhisperGate (Tier 1).
        suppressed: usize,
        /// Gaps that matched an already-open GitHub issue (Tier 2) and reused
        /// it instead of filing a duplicate.
        reused_existing: usize,
    },
    // …
}
```

| Field | Meaning | Source |
|---|---|---|
| `flagged` | New covering issues filed this cycle | Tier 2 `FiledNew` |
| `suppressed` | Same-cycle duplicates dropped in-process | Tier 1 `WhisperGate` |
| `reused_existing` | Open issues reused across a restart / index-lag window | Tier 2 `MatchedExisting` |

`reused_existing` is threaded through the daemon into the tick report
(`OverseerTickReport.workstream_gaps_reused_existing`) and the activity
counters, so a reused issue is visible on the same surfaces as a flagged one.

## The GitHub open-issue check

Tier 2 reuses the stewardship candidate-resolution and marker-matching helpers,
so the gap-scan inherits their resilience to GitHub's eventually-consistent
search index — subject to two composite-key caveats called out below:

- **`GhClient::search_issues(repo, dedup_key)`** →
  `resolve_dedup_candidates` runs a fast full-text
  `stewardship-signature:<key> in:body` search **unioned** with a
  strongly-consistent `RecentOpen(100)` scan of the newest open issues. The
  union defeats search-index lag: two sweeps within the multi-minute indexing
  window cannot both see an empty search and double-file.

  > **Composite-key caveat.** Unlike a stewardship failure signature (a fixed
  > 16-hex-char token), a gap `dedup_key` is a **multi-colon composite slug**
  > (`workstream-gap:goal:g-1042`). GitHub's full-text search tokenizes on
  > colons, so the search leg is **best-effort** for gap keys — a weaker
  > guarantee than in the stewardship case. The **`RecentOpen(100)` scan is
  > therefore the authoritative net**, and it is exhaustive only within the
  > newest-100 open-issue window. See
  > [Risks & design constraints](#risks--design-constraints) for the bounded
  > residual this leaves.
- **`overseer::open_issue_has_marker(candidates, dedup_key)`** returns whether a
  matching open issue exists. Match → reuse (`MatchedExisting`); no match →
  `create_issue`. For gap keys the match is **line-bounded**: the marker is
  matched as a whole (trimmed) line equal to `stewardship-signature: <key>`,
  never a bare substring `contains` — a dedicated matcher rather than the
  stewardship substring `dedup::find_existing`, because a composite gap key is a
  prefix of a longer sibling key (`…g-1042` ⊂ `…g-1042-extra`) and a substring
  match would silently swallow a genuinely-new gap. See the prefix-collision
  constraint below.
- **`GhClient::create_issue(repo, title, body)`** files the covering issue with
  the marker on its **own newline-terminated line** in the **body**:
  `stewardship-signature: workstream-gap:<signature>\n` — matching the
  stewardship convention that `--search … in:body` queries against, while the
  trailing newline is what makes the match line-bounded.

### Fail-loud contract

Any error from `gh` (search or create) **propagates** as an
`OverseerError` and results in **no issue creation and no operator
notification** for the affected cycle. The rail never blind-creates on a
degraded search — a fail-open path would re-introduce the very bursts #4717
fixes. The in-process gate is **not** committed for a gap whose durable
resolution failed, so a later successful cycle can retry cleanly.

## Observability

All gap-scan tracing is emitted on target `overseer::gap_scan`, structured
fields only — **no `print!`/`println!`**, no "Bridge" naming.

| Event | Fields |
|---|---|
| All gaps within dedup window | `flagged=0`, `suppressed`, `reused_existing=0` |
| Gaps filed / reused | `flagged`, `suppressed`, `reused_existing`, `dispatched`, `all_sent` |
| Durable reuse | per-gap `reused_existing=true` with the slug key only (never issue bodies or secrets) |

Only slugs / dedup keys appear in traces; issue bodies, gap prose, and any
secret material are never logged.

## Risks & design constraints

Two properties of the reused marker/search path hold *trivially* only for
stewardship's fixed-length 16-hex signatures. Gap `dedup_key`s are
**variable-length composite slugs**, so the gap-scan path must satisfy them
**explicitly**. Both are enforced by construction and pinned by
`tests_gap_scan`.

### Substring-prefix collision → line-bounded marker match

The shared `find_existing` historically matched with
`body.contains("stewardship-signature: <sig>")`. For fixed 16-hex signatures no
signature is a prefix of another, so a bare substring match is safe. Gap keys
are **not** length-uniform: `workstream-gap:goal:g-1042` is a **substring** of
`workstream-gap:goal:g-1042-extra`. A bare `contains` would let the covering
issue for `g-1042` falsely match the *distinct* gap for `g-1042-extra` → the
genuine `g-1042-extra` gap is **silently skipped** (false-positive reuse).

**Constraint (enforced):** the gap marker is written on its **own line
terminated by a newline**, and the matcher is **line-bounded** — it matches the
full line `stewardship-signature: <key>\n`, never a bare substring. This makes
prefix pairs non-matching regardless of key length. Stated as an invariant: gap
`dedup_key`s must be mutually non-prefixing *or* matched line-bounded; the
implementation guarantees the latter unconditionally, so the property does not
depend on how keys are chosen. `tests_gap_scan` pins a **prefix-collision
case** (`g-1042` vs `g-1042-extra` must not cross-match).

### Composite-key search fidelity → RecentOpen is authoritative

The full-text `… in:body` leg is **best-effort** for multi-colon gap keys
because GitHub tokenizes on colons (see the composite-key caveat under
[The GitHub open-issue check](#the-github-open-issue-check)). The
strongly-consistent `RecentOpen(100)` scan is the authoritative reuse net, but
only within the newest-100 open-issue window. **Bounded residual:** a distinct
gap whose still-open covering issue has aged past the newest 100 open issues
*and* is not surfaced by the tokenized search can be re-filed. This is accepted
for the gap-scan's cadence. If the residual proves material, a follow-up may
attach a stable `stewardship-gap` **label** to the filed issues and search by
label (label filters are not tokenized), making the search leg exact.
`tests_gap_scan` pins a **composite-key match case** so the intended reuse path
stays green.

## Invariants

- **One open covering issue per distinct gap — across restarts.** The durable
  GitHub check holds the guarantee even when in-process state is cold.
- **Additive / non-breaking.** New optional field + builder; unset
  `gap_issue_client` preserves the exact prior notify-only behaviour. The PRD
  and every existing caller are unchanged.
- **Two-tier, burst-safe.** `WhisperGate` pre-filters bursts before any `gh`
  call; the durable check runs only on fresh gaps.
- **Line-bounded marker match.** The gap marker is a whole newline-terminated
  line and is matched line-bounded, so no gap key can be a false-positive
  prefix match of another (see Risks). This is the property that makes
  variable-length composite keys safe with the shared marker convention.
- **RecentOpen is the authoritative net.** For composite gap keys the
  tokenized search leg is best-effort; correctness rests on the
  strongly-consistent `RecentOpen(100)` scan within its window.
- **Fail-loud, never blind-create.** A degraded `gh` search files nothing and
  notifies nothing; it never silently yields "no match".
- **Signature-stable keys.** `GapItem::dedup_key()` is the one source of the
  `workstream-gap:<signature>` key for the gate, the search, and the body
  marker — they cannot drift.
- **Trusted repo targeting.** The dedup/create repo comes from config, never
  from gap data — no cross-repo issue injection.

## Related reading

- [Overseer workstream gap-scan reference](./overseer-workstream-gap-scan.md)
  — the step this rail guards and its data model.
- [Gap-scan dedup & exponential backoff](../concepts/gap-scan-backoff-dedup.md)
  — the in-process gate and why the cross-process check was needed.
- [Goal Stewardship — Orchestrator Failure API Reference](./stewardship-api.md)
  — the reused `GhClient` / dedup contract.
- [Configure Overseer gap-scan durable dedup](../howto/configure-overseer-gap-scan-durable-dedup.md)
  — enable, verify, and troubleshoot end to end.
