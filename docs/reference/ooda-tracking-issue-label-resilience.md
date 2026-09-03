---
title: "Reference: resilient tracking-issue label ensure (`ooda-stuck`)"
description: >
  The label-ensure prelude that makes every OODA tracking-issue creation robust
  to a missing `ooda-stuck` label. Documents the pure `ensure_gh_label` helper
  and its `LabelEnsure` classification, the idempotent `gh label create` +
  graceful-degradation contract shared by the three `gh issue create` sites (the
  deterministic brain-failure safeguard, the `OpenTrackingIssue` lifecycle path,
  and the no-progress breaker's `GhIssueFiler`), and the structured tracing each
  site emits. Fixes the recurring `could not add label: 'ooda-stuck' not found`
  escalation failure (#4472).
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./no-progress-breaker-api.md
  - ./no-progress-root-cause-resolution-api.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../concepts/steerable-ooda-daemon.md
  - ../../src/ooda_actions/gh_label.rs
  - ../../src/ooda_actions/advance_goal/spawn.rs
  - ../../src/ooda_loop/no_progress.rs
---

# Reference: resilient tracking-issue label ensure (`ooda-stuck`)

> **Status: implemented (issue #4472).** The pure helper lives at
> [`src/ooda_actions/gh_label.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/gh_label.rs).
> It is called from the three sites that create the operator-facing tracking
> issue for a stuck goal:
> [`src/ooda_actions/advance_goal/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs)
> (the deterministic brain-failure safeguard **and** the
> `EngineerLifecycleDecision::OpenTrackingIssue` path) and
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs)
> (the no-progress breaker's `GhIssueFiler`).

## The defect this fixes

Before #4472, all three tracking-issue sites invoked
`gh issue create … --label ooda-stuck` with the label name **hardcoded and
unconditional**. The `ooda-stuck` label does not exist in `rysweet/Simard`, so
`gh` exited non-zero on **every** escalation with:

```
could not add label: 'ooda-stuck' not found
```

The no-progress breaker recorded `escalated=1` internally while the GitHub issue
was **never created** — a silent, broken escalation path. The recurring journal
signature was:

```
no-progress breaker: gh issue create failed (goal still Blocked) stderr=could not add label: 'ooda-stuck' not found
```

observed 4× in a 6-hour window (cycles 2430 / 2433 / 2436 / 2439), hiding stuck
goals such as `audit-simard-s-test-coverage` (`4d27c91a`) from the operator.

The fix makes label attachment **best-effort**: the label is idempotently
ensured before filing, and if it cannot be ensured the issue is filed **without**
the label rather than not filed at all. Escalation is now visible to the operator
in every case.

## Design invariant

> **The tracking issue MUST be filed. The label is a nicety, never a
> precondition.** A missing, un-creatable, or auth-restricted label degrades the
> issue to *unlabeled-but-filed*; it never turns escalation into a silent no-op.

This is the same fail-**visible** (not fail-silent) posture as the rest of the
no-progress breaker — see the
[no-progress breaker API](./no-progress-breaker-api.md) and
[steerable-daemon concept](../concepts/steerable-ooda-daemon.md).

## `ensure_gh_label` — the pure helper

```rust
// src/ooda_actions/gh_label.rs

/// The compile-time label name attached to every OODA tracking issue.
/// Never a runtime-derived string — see "Security" below.
pub(crate) const OODA_STUCK_LABEL: &str = "ooda-stuck";

/// Outcome of an idempotent `gh label create ooda-stuck`.
#[derive(Debug)]
pub(crate) enum LabelEnsure {
    /// The label did not exist and was just created. Callers emit an
    /// `info!`/`debug!` on this arm (a genuine, one-time repo mutation).
    Created,
    /// The label already existed. No mutation; no tracing warranted.
    AlreadyExists,
    /// The label could not be ensured (auth error, network error, `gh`
    /// missing, unclassifiable stderr). Carries a human-readable reason for
    /// the caller's `warn!`. Callers MUST fall back to filing without a label.
    Unavailable(String),
}

/// Idempotently ensure the `ooda-stuck` label exists in the CWD's repo.
///
/// Runs `gh label create ooda-stuck` (argv-only, never `sh -c`). Treats an
/// "already exists" stderr as success (`AlreadyExists`). Any other non-zero
/// exit, or a spawn error, is classified `Unavailable(reason)`.
///
/// This function emits **no** tracing itself — it returns a classification so
/// each call site can log with its own static `target:`. It never panics and
/// never calls `unwrap`/`expect`.
pub(crate) fn ensure_gh_label(label: &'static str) -> LabelEnsure { /* … */ }
```

### Classification rules

`ensure_gh_label` shells out with argv only:

```
gh label create <label>
```

and classifies the result:

| `gh` result | stderr contains | `LabelEnsure` |
| --- | --- | --- |
| exit 0 | — | `Created` |
| exit ≠ 0 | `already exists` (case-insensitive substring) | `AlreadyExists` |
| exit ≠ 0 | anything else (auth, not-found repo, rate-limit, …) | `Unavailable(<trimmed stderr>)` |
| spawn error (`gh` not on `PATH`, etc.) | — | `Unavailable(<io error>)` |

The "already exists" substring match is intentionally the **only** wording the
helper treats as idempotent success. If a future `gh` release changes that
phrasing, the helper degrades to `Unavailable` and the caller files the issue
**unlabeled** — a fail-safe outcome, never a fail-silent one.

The substring classifier is extracted into a tiny pure function so it can be
unit-tested without spawning `gh` (see [Testing](#testing)).

## Call-site contract (all three sites)

Every site follows the same three-step prelude before `gh issue create`:

1. **Ensure the label.** Call `ensure_gh_label(OODA_STUCK_LABEL)`.
   - `AlreadyExists` → attach `--label ooda-stuck`. No tracing.
   - `Created` → attach `--label ooda-stuck`. Emit a structured
     `info!`/`debug!` on the site's existing `target:` noting the auto-create.
   - `Unavailable(reason)` → **do not abort**. Emit a `tracing::warn!` with
     `reason` on the site's existing `target:`, then drop `--label` for this
     filing.
2. **Build argv conditionally.** The `--label ooda-stuck` pair is appended to
   the `gh issue create` argument vector **only** when the label was ensured;
   otherwise the issue is created without it. (Implemented as a `Vec<&str>` so
   the two trailing args are simply not pushed on the degraded path.)
3. **File the issue**, preserving each site's existing result handling and every
   pre-existing failure-branch trace verbatim.

> The behaviour is **additive and non-breaking**: no function signature changes,
> no new panics, and no new `print!`/`println!`/`eprintln!`. New code emits
> structured tracing / OTel only. The pre-existing success `eprintln!` in the
> deterministic safeguard is intentionally left untouched.

### Site 1 — deterministic brain-failure safeguard

`src/ooda_actions/advance_goal/spawn.rs` (`.output()` + `match`, target
`simard::ooda_brain`). The success `eprintln!` (`DETERMINISTIC SAFEGUARD: … +
tracking issue filed`) and both existing `error!` branches
(`deterministic safeguard: gh issue create FAILED …`,
`deterministic safeguard: gh process spawn FAILED …`) are preserved. The
label-ensure prelude runs immediately before the `gh issue create` invocation.

### Site 2 — `OpenTrackingIssue` engineer-lifecycle path

`src/ooda_actions/advance_goal/spawn.rs`
(`EngineerLifecycleDecision::OpenTrackingIssue`, `.status()` + `if let Err`,
target `simard::ooda_brain`). The existing
`open_tracking_issue: gh issue create failed` warn is preserved; the prelude and
conditional `--label` are inserted ahead of it.

### Site 3 — no-progress breaker `GhIssueFiler`

`src/ooda_loop/no_progress.rs` (`.output()` + `match`, target `simard::ooda`).
This is the emitter of the recurring journal signature at the historical
`no-progress breaker: gh issue create failed (goal still Blocked)` line. The
success arm (which parses the created issue URL / number into `FiledIssue`) and
both failure arms are preserved; the prelude and conditional `--label` are
inserted ahead of the `gh issue create`.

Because the prelude lives inside `GhIssueFiler::file_issue`, it inherits the
caller's idempotence guard: `escalate_with_tracking_issue` only invokes
`file_issue` when the goal is **not** already linked to a breaker tracking issue
(`already_tracked == false`). A re-stall of an already-tracked goal therefore
skips both the `gh issue create` **and** the `ensure_gh_label` subprocess. The
extra `gh label create` runs only while the goal is **not yet
tracked** — normally exactly once, because the first *successful* filing links
the issue (`link_tracking_issue`) and suppresses every later attempt. If filing
keeps failing (so no issue is ever linked), the prelude re-runs each escalate
cycle until one succeeds — which is the intended fail-visible behaviour, not a
cost regression. Sites 1 and 2 have no such pre-check, so their prelude runs on
every safeguard/lifecycle invocation (still negligible — those paths are rare).

## Tracing surface

The label-ensure feature adds exactly two new structured events per site and
changes none of the existing ones:

| Event | Level | Target | When |
| --- | --- | --- | --- |
| label auto-created | `info!` (or `debug!`) | site's existing target | `ensure_gh_label` returned `Created` |
| label unavailable, filing unlabeled | `warn!` | site's existing target | `ensure_gh_label` returned `Unavailable`; includes `reason` |

There is **no** new event for `AlreadyExists` (the steady state) — it is silent
by design to avoid per-cycle log noise.

Example (degraded path, breaker site, `target: simard::ooda`):

```
WARN simard::ooda label=ooda-stuck reason="HTTP 403: Resource not accessible by integration" no-progress breaker: could not ensure label, filing tracking issue without it
```

Example (auto-create, breaker site):

```
INFO simard::ooda label=ooda-stuck no-progress breaker: created missing 'ooda-stuck' label
```

## Security

- **Argv-only invocation.** Both `gh label create` and `gh issue create` are
  built with `std::process::Command` and separate arguments — never `sh -c` or
  string interpolation — so no goal title, body, or branch can inject a shell
  command or an extra argument.
- **Label is a compile-time constant.** `OODA_STUCK_LABEL` is a `&'static str`;
  no runtime-derived string ever flows into the `--label` argument.
- **Ambient auth reused, never handled.** The helper relies on `gh`'s existing
  auth (`GH_TOKEN` / keyring). It never reads, logs, or passes tokens. The
  degraded path uses identical credentials, repo, and scope — no elevation, no
  retry loop.
- **No silent swallow.** `Unavailable` is always surfaced via `warn!` with its
  cause, so auth or label failures stay operator-visible. Raw `stderr` is
  confined to `warn!`/`debug!`, never echoed at `info!`.
- **Idempotency race is safe.** Concurrent `gh label create` calls converge
  because "already exists" is treated as success — the operation is
  non-destructive.

## Testing

- **Unit (hermetic).** An inline `#[cfg(test)]` test in `gh_label.rs` exercises
  the pure stderr classifier: `already exists` → `AlreadyExists`; an auth
  string → `Unavailable`; empty/other → `Unavailable`. No live `gh` is spawned.
- **Regression (must stay green).**
  - `src/ooda_loop/tests_no_progress_investigation.rs`
  - `tests/gadugi/no-progress-tracking-issue-link.sh`
  - `tests/gadugi/no-progress-tracking-issue-link.yaml`
- **Gate.** `cargo build`, `cargo clippy -- -D warnings`, and the targeted tests
  above are all clean; CI-green and merge-ready.

## Out of scope

Unchanged by this fix: escalation counting, the `Blocked` transition logic,
manually creating the label in repo settings, issue title/body content, and any
broader `eprintln!` → tracing convention rework.

## See also

- [No-progress breaker API reference](./no-progress-breaker-api.md) — the
  safeguard whose escalation this fix unblocks.
- [No-progress root-cause resolution API reference](./no-progress-root-cause-resolution-api.md) —
  the ladder that escalates to a human tracking issue as its last rung.
- [Diagnose a no-progress block and read its WHY](../howto/diagnose-a-no-progress-block.md).
- [Unblock stuck OODA goals](../howto/unblock-stuck-ooda-goals.md).
