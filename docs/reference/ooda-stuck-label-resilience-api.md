---
title: OODA stuck-goal label resilience API reference
description: Reference for the ooda_stuck_label module — the pure, subprocess-free helpers (is_missing_label_error, label_already_exists, issue_create_argv) and the best-effort ensure_ooda_stuck_label() that make every OODA stuck-goal issue filer robust to a missing 'ooda-stuck' GitHub label, so a genuinely-stuck goal always gets a linked tracking issue.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../reference/no-progress-breaker-api.md
  - ../reference/no-progress-root-cause-resolution-api.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../howto/label-and-filter-goals.md
  - ../../src/ooda_stuck_label.rs
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/ooda_actions/advance_goal/spawn.rs
---

# OODA stuck-goal label resilience API reference

> **Status: implemented.** The pure helpers and the best-effort label
> ensurer live in
> [`src/ooda_stuck_label.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_stuck_label.rs)
> and are registered as `pub mod ooda_stuck_label;` in
> [`src/lib.rs`](https://github.com/rysweet/Simard/blob/main/src/lib.rs).
> They are consumed by all three stuck-goal issue-filing sites:
> [`GhIssueFiler::file_issue`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs),
> the deterministic brain-failure safeguard, and the
> `OpenTrackingIssue` engineer-lifecycle path in
> [`src/ooda_actions/advance_goal/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs).

## Why this exists

Every OODA breaker that parks a genuinely-stuck goal files a GitHub
tracking issue and **links it back to the goal** so the done-gate gains a
derivable signal and an operator can navigate goal → issue. All three
filers shelled out to:

```bash
gh issue create --title "…" --body "…" --label ooda-stuck
```

The `ooda-stuck` label did **not** exist in `rysweet/Simard`. `gh issue
create --label` fails hard when the label is absent, so *no issue was ever
filed*. The observed production failure (simard-ooda journal,
`2026-07-21T12:40:45Z`, `cycle=2337`) was:

```text
ERROR simard::ooda: no-progress breaker: gh issue create failed (goal still Blocked)
  stderr=could not add label: 'ooda-stuck' not found
```

The goal stayed `Blocked` with **no linked artifact** — the operator
safety net was silently non-functional. This module makes the filers
robust to the missing label without changing behaviour when the label
exists.

## Contents

- [Behaviour contract](#behaviour-contract)
- [`is_missing_label_error`](#is_missing_label_error)
- [`label_already_exists`](#label_already_exists)
- [`issue_create_argv`](#issue_create_argv)
- [`ensure_ooda_stuck_label`](#ensure_ooda_stuck_label)
- [The filing flow](#the-filing-flow)
- [Call sites](#call-sites)
- [Tracing targets and observability](#tracing-targets-and-observability)
- [Configuration](#configuration)
- [What is unchanged](#what-is-unchanged)
- [Testing](#testing)
- [See also](#see-also)

## Behaviour contract

- **Label absent:** the filer first runs a best-effort, idempotent
  `gh label create ooda-stuck`, then files the issue. If the create still
  fails specifically because the label is missing, it logs the original
  stderr at `warn` and retries the create **exactly once without
  `--label`**. A valid tracking issue is returned and goal linkage
  succeeds.
- **Label present:** observable behaviour is **byte-for-byte unchanged** —
  the happy-path argv is identical to the pre-fix command, no `--repo` is
  added, and the ensure step is a no-op success.
- **No-swallow:** the first failure's stderr is always emitted via
  `tracing` before any retry. The real error stays observable; a
  non-missing-label failure (auth, rate-limit, network, repo) is **not**
  retried and is surfaced exactly as before.
- The failure signature `could not add label: 'ooda-stuck' not found` can
  no longer result in an unfiled issue.

## `is_missing_label_error`

The narrow classifier that decides whether a failed `gh issue create` is a
missing-label failure eligible for the label-less retry. Kept deliberately
narrow so authentication, rate-limit, network, or repository errors return
`false` and are never masked.

```rust
/// True when `stderr` from a failed `gh issue create` indicates the
/// `--label` value does not exist (so a label-less retry is warranted).
/// Case-insensitive substring match. Returns `false` for any other
/// failure (auth, rate-limit, network, repo) so real errors are never
/// masked by the fallback path.
pub fn is_missing_label_error(stderr: &str) -> bool;
```

Recognised patterns (case-insensitive substrings):

| Pattern | Example stderr |
| --- | --- |
| `not found` combined with `label` | `could not add label: 'ooda-stuck' not found` |
| `could not add label` | `could not add label: 'ooda-stuck' not found` |

`is_missing_label_error` returns `false` for, e.g.,
`gh: Not Found (HTTP 404)` on a repo lookup, `HTTP 401: Bad credentials`,
`API rate limit exceeded`, and any empty stderr.

## `label_already_exists`

Classifies the stderr of a failed `gh label create` so a concurrent OODA
run that already created the label is treated as success (idempotence).

```rust
/// True when `gh label create` failed only because the label already
/// exists — treated as success by `ensure_ooda_stuck_label`.
/// Case-insensitive substring match on `already exists`.
pub fn label_already_exists(stderr: &str) -> bool;
```

## `issue_create_argv`

The single source of truth for the `gh issue create` argument vector,
shared by every site so argv discipline is uniform and unit-testable
without a subprocess. `title` and `body` are always discrete argv elements
(never interpolated into a shell string), which is the command-injection
guard.

```rust
/// Build the argv for `gh issue create`. When `with_label` is true the
/// vector ends with `["--label", "ooda-stuck"]`; when false those two
/// elements are omitted (the label-less fallback). Title and body are
/// always discrete elements — no shell, no `sh -c`.
pub fn issue_create_argv<'a>(
    title: &'a str,
    body: &'a str,
    with_label: bool,
) -> Vec<&'a str>;
```

Examples:

```rust
assert_eq!(
    issue_create_argv("T", "B", true),
    vec!["issue", "create", "--title", "T", "--body", "B", "--label", "ooda-stuck"],
);
assert_eq!(
    issue_create_argv("T", "B", false),
    vec!["issue", "create", "--title", "T", "--body", "B"],
);
```

The `with_label: true` form is byte-for-byte identical to the pre-fix
command, which is what guarantees "unchanged when the label exists".

## `ensure_ooda_stuck_label`

Best-effort, idempotent label creation. Runs before the first issue-create
attempt. It never aborts the cycle: a failure to ensure the label simply
means the label-less retry may run later.

```rust
/// Best-effort, idempotent `gh label create ooda-stuck`. Returns `Ok(())`
/// when the label exists afterwards (created now, or already present per
/// `label_already_exists`). A spawn error or a non-"already exists"
/// failure is logged at `debug`/`warn` on the given `target` and returns
/// `Err`, but callers treat this as non-fatal and proceed to the create
/// attempt regardless. Never panics.
pub fn ensure_ooda_stuck_label(target: &'static str) -> SimardResult<()>;
```

Notes:

- Idempotent: concurrent OODA runs racing to create the label are safe —
  the second sees `already exists` stderr and treats it as success.
- Non-fatal: callers ignore the return value for control flow (they always
  proceed to `gh issue create`); it exists for observability and testing.
- Uses `Command::args(["label", "create", "ooda-stuck", ...])` — no shell.

## The filing flow

Every site follows the same three-step flow, implemented once via the
helpers above:

1. **Ensure** — `ensure_ooda_stuck_label(target)` (best-effort,
   idempotent). Keeps future issues correctly tagged.
2. **Create** — `gh issue create` with `issue_create_argv(title, body,
   true)` (the label-included happy path).
3. **Fallback** — if step 2 fails and
   `is_missing_label_error(stderr)` is `true`: log the original stderr at
   `warn` (no-swallow), then retry **once** with
   `issue_create_argv(title, body, false)`. Any other failure is surfaced
   unchanged and **not** retried.

The retry is bounded to exactly one attempt — no recursion, no loop, no
resource exhaustion. `String::from_utf8_lossy` is used on all captured
stderr/stdout, so non-UTF-8 or oversized output never panics.

## Call sites

All three sites capture stderr via `.output()` (the `OpenTrackingIssue`
site was switched from `.status()` to `.output()` so its stderr is
available for missing-label detection).

| Site | Location | Tracing target |
| --- | --- | --- |
| No-progress breaker filer | `GhIssueFiler::file_issue` in `src/ooda_loop/no_progress.rs` | `simard::ooda` |
| Deterministic brain-failure safeguard | `src/ooda_actions/advance_goal/spawn.rs` | `simard::ooda_brain` |
| Engineer-lifecycle `OpenTrackingIssue` | `src/ooda_actions/advance_goal/spawn.rs` | `simard::ooda_brain` |

> **Note:** the deterministic safeguard's existing failure branches already
> log on `simard::ooda_brain` (spawn.rs L391/L399), and every tracing call in
> `spawn.rs` uses `simard::ooda_brain`. The `eprintln!` success line is
> therefore converted to `tracing` on `simard::ooda_brain` (not `simard::ooda`)
> so the target stays consistent within the block and no per-site target
> regresses. Only the no-progress breaker filer in `no_progress.rs` uses
> `simard::ooda`.

The pre-existing idempotence guard in the no-progress breaker
(`escalate_with_tracking_issue`, which never files a second tracking issue
for a goal already linked to one) is preserved unchanged — the fallback
runs *inside* a single filing attempt, so a re-stall still never spams
duplicate `ooda-stuck` issues.

## Tracing targets and observability

Structured tracing + OpenTelemetry only — no `print!`/`println!`/`eprintln!`.
As part of this change the one remaining `eprintln!` in the deterministic
safeguard success path was converted to structured `tracing` on the
`simard::ooda_brain` target (the target its sibling failure branches already
use). Per-site targets are preserved:

- `simard::ooda` — the no-progress breaker filer (`no_progress.rs`).
- `simard::ooda_brain` — the deterministic safeguard **and** the
  engineer-lifecycle `OpenTrackingIssue` path (both in `spawn.rs`).

Representative events for the label-absent path:

```text
WARN  simard::ooda  stderr="could not add label: 'ooda-stuck' not found"
      no-progress breaker: gh issue create failed with missing label; retrying without --label
WARN  simard::ooda  title="…" issue="4231"
      no-progress breaker: tracking issue filed for stuck goal (label-less fallback)
```

The two `spawn.rs` sites emit the equivalent fallback events on
`simard::ooda_brain`:

```text
WARN  simard::ooda_brain  goal="…" stderr="could not add label: 'ooda-stuck' not found"
      deterministic safeguard: gh issue create failed with missing label; retrying without --label
```

The first line proves the original error is never swallowed; the second
proves the issue was still filed and linked.

## Configuration

No new configuration, environment variables, or dependencies are
introduced. The feature reuses the existing `gh` authentication context;
it never reads, logs, or injects tokens. It is always on and non-breaking:
when the `ooda-stuck` label exists (including after the first ensure
succeeds), the code path is identical to the previous behaviour.

Operators who wish to pre-create the label out of band may do so; the
idempotent ensure step will simply observe `already exists`:

```bash
gh label create ooda-stuck \
  --description "OODA daemon: goal parked as genuinely stuck; needs review" \
  --color B60205
```

## What is unchanged

- The `FiledIssue` return type, `parse_issue_number`, and the
  goal-linkage / `wip_refs` behaviour.
- The no-progress breaker's idempotence guard (no duplicate tracking
  issues on re-stall).
- The happy-path argv when the label exists (byte-for-byte).
- The escalation semantics: a filing failure is still logged, never
  propagated, and never aborts the cycle.

## Testing

The decision logic is factored into pure, subprocess-free functions
(mirroring the existing `parse_issue_number` factoring), so the
missing-label handling is unit-tested with **no subprocess mocking**.
Inline `#[cfg(test)] mod tests` in `src/ooda_stuck_label.rs` covers:

- `is_missing_label_error` returns `true` for the production signature
  (`could not add label: 'ooda-stuck' not found`) and other missing-label
  phrasings, and `false` for auth (`HTTP 401`), rate-limit, `Not Found`
  repo errors, and empty stderr — the error-masking guard.
- `label_already_exists` returns `true` only on `already exists` stderr.
- `issue_create_argv` produces the exact label-included vector (identical
  to the pre-fix command) and the label-less vector, with `title`/`body`
  as discrete elements (the command-injection guard).

All of `cargo fmt --check`, `cargo clippy`, `cargo build`, and
`cargo test` are green.

## See also

- [No-progress breaker API reference](../reference/no-progress-breaker-api.md)
  — the safeguard that files these tracking issues.
- [No-progress root-cause resolution API](../reference/no-progress-root-cause-resolution-api.md)
  — the classifier that decides a goal is `GENUINELY-STUCK`.
- [Unblock OODA goals stuck after a brain-failure lockout](../howto/unblock-stuck-ooda-goals.md)
- [Diagnose a no-progress block and read its WHY](../howto/diagnose-a-no-progress-block.md)
- [Label and filter goals](../howto/label-and-filter-goals.md)
