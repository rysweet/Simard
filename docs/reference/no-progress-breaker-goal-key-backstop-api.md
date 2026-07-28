---
title: No-progress breaker goal-key backstop API reference
description: >
  Reference for the additive hardening that makes the OODA no-progress breaker's
  issue-storm suppression survive goal-id churn and goal-board resets. Specifies
  the pure `fold_goal_identity` helper (stable, injection-safe
  `sha256[..16]` folding of a churny `ActiveGoal.id`), the `ooda-goal-key:<folded_id>`
  body marker the breaker embeds, the additive
  `NoProgressIssueFiler::issue_open_with_marker` default method (open-issue
  existence backstop), its production `GhIssueFiler` implementation over
  `gh issue list --search … in:body`, and the two-guard filing gate in
  `escalate_with_tracking_issue` (board-local `WipRef` fast path first, backstop
  only on miss). All items are additive and non-breaking.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./no-progress-breaker-storm-suppression-api.md
  - ./no-progress-breaker-api.md
  - ./no-progress-root-cause-resolution-api.md
  - ./goal-board-api.md
  - ../concepts/no-progress-breaker-goal-key-backstop.md
  - ../concepts/no-progress-breaker-storm-suppression.md
  - ../howto/configure-no-progress-breaker-open-issue-backstop.md
  - ../../src/goal_curation/no_progress_breaker.rs
  - ../../src/ooda_loop/no_progress.rs
---

# No-progress breaker goal-key backstop API reference

> **Status: implemented.** `fold_goal_identity` and the `ooda-goal-key` body
> marker live in
> [`src/goal_curation/no_progress_breaker.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/no_progress_breaker.rs);
> the `NoProgressIssueFiler::issue_open_with_marker` method, its `GhIssueFiler`
> implementation, and the two-guard gate in `escalate_with_tracking_issue` live
> in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs).
> The rustdoc on those items is the canonical API; the signatures below are kept
> in sync with it.

This page specifies the **additive, non-breaking** hardening layered on the
[issue-storm suppression](./no-progress-breaker-storm-suppression-api.md) fix.
It adds a second, GitHub-durable suppression guard that survives the two
conditions the board-local `WipRef` marker cannot cover: **goal-id churn** and
**goal-board reset**. For the rationale, see
[the concept doc](../concepts/no-progress-breaker-goal-key-backstop.md).

## Constants

### `OODA_GOAL_KEY_MARKER_PREFIX`

```rust
const OODA_GOAL_KEY_MARKER_PREFIX: &str = "ooda-goal-key:";
```

The body-marker prefix the breaker embeds and the backstop matches on. A
constant — never re-typed at a call site. The full marker line is
`{OODA_GOAL_KEY_MARKER_PREFIX} {folded_id}` (one ASCII space), placed in the
issue **body**, never the title.

## `fold_goal_identity`

```rust
/// Fold a churny `ActiveGoal.id` into a stable, injection-safe identity token:
/// the first 16 lowercase hex characters of `sha256(goal_id)`.
///
/// Pure and total. Mirrors the `stewardship::failure_signature` folding shape so
/// a volatile id collapses to one deterministic `[0-9a-f]{16}` key. Because the
/// output is a fixed-charset hex literal it is safe to interpolate into a `gh`
/// `--search` query argument — it cannot carry whitespace, quotes, or GitHub
/// search qualifiers (`is:`, `label:`, `in:`) that would corrupt the dedup check.
pub(crate) fn fold_goal_identity(goal_id: &str) -> String;
```

- **Input:** any `ActiveGoal.id` (free-form UTF-8).
- **Output:** exactly 16 lowercase hex characters.
- **Determinism:** `fold_goal_identity(x) == fold_goal_identity(x)` always;
  distinct ids collide only at the `sha256[..8]`-byte level (negligible).
- **Location:** `src/goal_curation/no_progress_breaker.rs`. Keeps the breaker
  **pure** — no I/O; it only produces the token and embeds the marker string
  into the body it constructs.

### Marker embedding

The breaker's escalation body builder appends the marker line to the body it
already constructs. Illustrative shape (the surrounding body text is unchanged):

```text
<existing breaker body — WHY, evidence, next steps …>

ooda-goal-key: 9f8c1a2b3c4d5e6f
```

The marker is the last body line and carries no secrets: it is a pure hash of
the id. All other free-text copied into the body continues to pass through the
`redact_token` / `redact_uuids` helpers (see
[Implementation prerequisites](#implementation-prerequisites) — these helpers
are module-private today and must be promoted to `pub(crate)` before
`goal_curation` can call them).

## `NoProgressIssueFiler::issue_open_with_marker`

```rust
pub(crate) trait NoProgressIssueFiler {
    fn file_issue(&self, title: &str, body: &str) -> Option<FiledIssue>;

    /// Existence backstop: is there an OPEN `ooda-stuck` issue whose body carries
    /// `marker` (an `ooda-goal-key:<folded_id>` line)? Additive with a default of
    /// `false` so every existing impl and test fake is unchanged.
    ///
    /// Called ONLY after the board-local `WipRef` fast path misses, so a
    /// steady-state daemon with an intact board makes zero extra API calls.
    ///
    /// Fail-open: a query error MUST return `false` ("no duplicate found") and
    /// let filing proceed — a rare duplicate is strictly better than a lost
    /// stuck-goal signal, and the check must never abort the OODA cycle.
    fn issue_open_with_marker(&self, _marker: &str) -> bool {
        false
    }
}
```

- **Default `false`** makes the method purely additive: `CountingFiler`, the
  test fakes, and any third impl compile and behave exactly as before, taking
  the "no duplicate" path.
- **`marker`** is the full `ooda-goal-key:<folded_id>` line (or just the folded
  key) the caller derives from the stalled goal via `fold_goal_identity`.
- **State scope:** OPEN issues only. A closed issue does not suppress — a
  re-opened stall should re-file, consistent with the storm-suppression
  `find_existing` semantics.

### Production implementation (`GhIssueFiler`)

```rust
impl NoProgressIssueFiler for GhIssueFiler {
    fn issue_open_with_marker(&self, marker: &str) -> bool {
        // Independent, direct `gh issue list` existence query scoped to the
        // breaker's own `ooda-stuck` label and `ooda-goal-key` body marker.
        // This does NOT call the signature-typed `search_issues` trait method
        // (that lives in `supply_chain_steward` and keys on
        // `stewardship-signature`, not `ooda-goal-key`); it only follows the
        // same argv-vector, `--search … in:body` invocation pattern. Argv only
        // — never a shell string.
        match std::process::Command::new("gh")
            .args([
                "issue", "list",
                "--state", "open",
                "--label", "ooda-stuck",
                "--search", &format!("{marker} in:body"),
                "--json", "number,body",
            ])
            .output()
        {
            Ok(out) if out.status.success() => body_contains_marker(&out.stdout, marker),
            _ => false, // fail-open
        }
    }
}
```

- **Search index vs. strong match.** GitHub's `--search` index is eventually
  consistent, so the implementation confirms the marker against the returned
  JSON `body` (a strongly-consistent `gh issue list` field) rather than trusting
  the search match alone — a freshly filed issue that has not yet indexed still
  dedups correctly on the next cycle.
- **Argv only (`SR2`).** The invocation is `Command::new("gh").args([...])`;
  no `sh -c`, no string-interpolated command line.
- **Least privilege (`SR4`).** Repo-scoped `gh`; the implementation never reads,
  logs, or embeds `GH_TOKEN`. An auth failure fails loud (logged via `tracing`)
  but fails **open** for dedup and never aborts the cycle.

## Implementation prerequisites

Two small source reconciliations are required before this feature compiles as
specified. Both are captured here so the implementation order reflects reality
rather than an idealized reuse.

1. **Promote the redaction helpers to `pub(crate)`.** `redact_token` and
   `redact_uuids` are **module-private** functions in
   [`src/stewardship/dedup.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/dedup.rs)
   (only `normalize`, `failure_signature`, and `find_existing` are `pub`).
   Because the breaker lives in `goal_curation`, reusing them for body
   redaction (SR3/SR6) requires changing their visibility from `fn` to
   `pub(crate) fn`. This is a non-breaking visibility widening — no signature or
   behaviour change. This step must precede wiring redaction into the breaker's
   body builder.

2. **Use a direct `gh` call, not the `search_issues` trait method.** The
   existence backstop for the `ooda-goal-key` half is a direct
   `Command::new("gh").args([…])` invocation scoped to `--label ooda-stuck`
   (shown above). It deliberately does **not** reuse
   `supply_chain_steward`'s `search_issues(&self, signature: &str)` trait
   method: that method is signature-typed and matches
   `stewardship-signature: <sig>` bodies, not `ooda-goal-key:<folded_id>`
   bodies. Only the branch-gated `recurring_goal_reblock` half (keyed on
   `stewardship::failure_signature`) reuses `search_issues` /
   `find_existing`; the OODA breaker half uses its own direct query.

## `escalate_with_tracking_issue` — the two-guard gate

The escalation side effect now evaluates two suppression guards in strict cost
order before filing. The `Blocked` status is still written first (unchanged from
the storm-suppression fix); only the *file* step is gated.

```text
1. board-local WipRef suppression marker present (is_breaker_tracking_ref)?
        └─ yes → already suppressed; return (FAST PATH, zero API calls)
2. filer.issue_open_with_marker("ooda-goal-key:<folded_id>")?
        └─ yes → an open duplicate already exists; write the board-local WipRef
                 marker (re-seed the fast path) and return — do NOT file
3. otherwise → file_issue(title, body-with-marker); write the WipRef marker;
               upgrade to a linked tracking ref on success
```

Guarantees:

- **Zero steady-state API cost (`SR7`).** Guard 2 runs only on a guard-1 miss.
  An intact board answers every already-suppressed goal at guard 1.
- **Churn / reset coverage.** When id churn or a board reset erases the
  `WipRef`, guard 2 still finds the open issue by its durable body marker and
  suppresses — then re-seeds the board-local marker so future cycles hit the
  fast path again.
- **Additive.** Removing guard 2 (or a filer returning the `false` default)
  degrades exactly to the prior storm-suppression behaviour — no regression for
  stable-id, intact-board goals.

## Test contract

Inline `#[cfg(test)]` modules in both files cover:

| # | Case | Asserts |
|---|------|---------|
| 1 | no-progress, first filing | Guard 1 & 2 both miss → `file_issue` called once; body carries the `ooda-goal-key` marker |
| 2 | no-progress, duplicate suppressed across id churn / board reset | Guard 1 misses (no `WipRef`), guard 2 hits (`issue_open_with_marker == true`) → `file_issue` NOT called; `WipRef` re-seeded |
| 3 | fast-path, no API call | Guard 1 hits → `issue_open_with_marker` never invoked (counting fake records zero calls) |
| 4 | injection-charset id | `fold_goal_identity` on an id containing spaces / quotes / `is:` / `label:` yields a pure `[0-9a-f]{16}` token |
| 5 | token redaction in body | secrets / UUIDs in copied free-text are redacted before the marker line |
| 6 | fail-open | `issue_open_with_marker` returns `false` on a `gh` error and filing proceeds |

Reblock half (branch-gated, see below) mirrors cases 1–2 keyed on
`stewardship::failure_signature` via `find_existing`.

## The `recurring_goal_reblock` half (branch-gated)

The overseer `recurring_goal_reblock` filer (`src/overseer/observer.rs`) is not
present on every branch. Where present, it applies the identical pattern using
the existing stewardship primitives:

- **Key:** `stewardship::failure_signature(failure_kind, error_text)` — the same
  16-hex signature used across steward dedup.
- **Backstop:** `stewardship::find_existing(&open_issues, &signature)` over
  `SupplyChainGh::search_issues`, which already matches
  `stewardship-signature: <sig>` in the issue body.

Where the filer is absent on the target branch, this is tracked as a documented
follow-up (see the how-to) rather than implemented against a phantom symbol.

## Security summary

| ID | Control |
|----|---------|
| SR1 | Free-form goal id is hashed to `[0-9a-f]{16}` **before** entering a `--search` query — prevents query-injection-as-reliability-bypass. |
| SR2 | `gh` invoked as an argv vector; no `sh -c` / string-interpolated command lines. |
| SR3 | No secrets/PII in title or body; body free-text passes through `redact_token` / `redact_uuids` (requires promoting both to `pub(crate)` — see [Implementation prerequisites](#implementation-prerequisites)); never log the full body at `info`. |
| SR4 | Least privilege; never read/print/embed `GH_TOKEN`; auth failure fails loud but never aborts the cycle. |
| SR5 | Existence-check errors fail **open** (duplicate < lost signal), by design and documented. |
| SR6 | No new dependencies — reuse `sha2` and `serde_json`, and follow the existing argv `gh` invocation *pattern* (a direct call, not the `search_issues` trait method — see [Implementation prerequisites](#implementation-prerequisites)). |
| SR7 | Backstop API call only after the `WipRef` fast-path miss → zero steady-state API cost; avoids secondary-rate-limit self-DoS. |

## See also

- [Concept: the breaker survives goal-id churn and board resets](../concepts/no-progress-breaker-goal-key-backstop.md)
- [Issue-storm suppression API reference](./no-progress-breaker-storm-suppression-api.md)
- [Configure the open-issue backstop](../howto/configure-no-progress-breaker-open-issue-backstop.md)
