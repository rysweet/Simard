---
title: OODA escalation issue-filing API reference
description: Reference for the shared OODA escalation issue-filing helper — `spawn_tracking_issue(repo, title, body) -> TrackingIssueOutcome` in src/ooda_actions/advance_goal/spawn.rs, its mandatory validated `--repo`, the consistent `ooda-stuck` label, the value-bound `--title`/`--body` argument-injection hardening, and the full tracing+OTel surfacing of every outcome arm (success / non-zero exit / spawn error). Fixes the two `gh issue create` sites (the deterministic safeguard site and the OpenTrackingIssue site) that failed or silently dropped a non-zero exit, so a stuck goal's operator escalation is never lost (#4472).
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./no-progress-breaker-api.md
  - ./no-progress-root-cause-resolution-api.md
  - ./tombstoned-goal-engineer-reaper-api.md
  - ./spawn-agent-for-goal.md
  - ./engineer-loop-argv-sanitization.md
  - ./goal-target-repo-routing.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../../src/ooda_actions/advance_goal/spawn.rs
---

# OODA escalation issue-filing API reference

> **Status: implemented.** The shared helper `spawn_tracking_issue` and the
> `TrackingIssueOutcome` enum live in
> [`src/ooda_actions/advance_goal/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs).
> Both `gh issue create` call sites in that file — the deterministic
> brain-failure **safeguard site** and the engineer-lifecycle
> **`OpenTrackingIssue`** site — now route through it. The change is
> **additive and non-breaking**; it surfaces every failure via `tracing` + OTel
> and files against an explicit, validated `--repo`. References **#4472**.

## Why this exists

When the OODA loop escalates a stuck goal, it files an operator-facing tracking
issue with `gh issue create`. Two call sites did this independently in
`spawn.rs`, and both were unreliable:

- **Site A — the deterministic brain-failure safeguard** (~L370). It ran `gh
  issue create` with `--label ooda-stuck` but **no `--repo`**, so the target
  repo was whatever `gh`'s ambient context resolved to — which, from a daemon
  process or a worktree, is frequently wrong or unresolved, causing the create
  to fail. On success it emitted a stray `eprintln!` (~L384) instead of
  structured tracing.

- **Site B — the engineer-lifecycle `OpenTrackingIssue` decision** (~L926). It
  used `.status()` and only logged on `Err(e)` (the process failed to *spawn*).
  A `gh` that spawned fine but **exited non-zero** — auth failure, bad repo,
  rate limit, malformed label — returned `Ok(status)` with `status.success() ==
  false`, and that arm was **silently dropped**. The escalation issue was never
  created and nothing was logged: a stuck goal missed its operator escalation
  with **zero** signal.

The result (**#4472**): the OODA no-progress breaker's safeguard could not file
its tracking issue, so stuck goals silently missed escalation.

This feature centralizes issue filing in one hardened helper that:

1. requires and **validates** an explicit `--repo owner/name`;
2. applies the consistent `ooda-stuck` label;
3. hardens `title` / `body` against argument injection by binding each to its own
   `--title` / `--body` flag value;
4. **surfaces every outcome** — success, non-zero exit, and spawn error — via
   `tracing` + OTel, so no failure is ever silently dropped.

## Data model

### `TrackingIssueOutcome`

```rust
/// Result of an escalation `gh issue create`. Every variant is surfaced by the
/// helper before it returns — no arm is silently dropped (#4472).
#[derive(Debug)]
pub enum TrackingIssueOutcome {
    /// `gh` exited 0. Carries the created issue URL/number when parseable.
    Filed { reference: Option<String> },
    /// `gh` spawned but exited non-zero (auth, bad repo, rate limit, bad
    /// label…). Carries the exit code and captured stderr (bounded).
    Rejected { code: Option<i32>, stderr: String },
    /// Input rejected BEFORE spawning `gh` — an invalid `repo` (fails the
    /// `owner/name` shape). Fail-closed: `gh` is never invoked.
    InvalidRepo { repo: String },
    /// `gh` failed to spawn at all (binary missing, `E2BIG`, …).
    SpawnError { error: String },
}

impl TrackingIssueOutcome {
    /// `true` only for `Filed`. The callers do NOT block their primary action
    /// on this — a goal is still marked `Blocked` even if filing fails — but
    /// they use it to choose the log level and OTel attributes.
    pub fn succeeded(&self) -> bool;
}
```

## `spawn_tracking_issue`

```rust
/// File an operator-facing OODA escalation issue against `repo`.
///
/// * `repo`  — REQUIRED, validated `owner/name`. Never inferred from ambient
///             `gh` context (SR-V3 authz boundary). An invalid value returns
///             `InvalidRepo` WITHOUT spawning `gh`.
/// * `title` / `body` — sanitized and each bound to its own `--title` / `--body`
///             flag value, so a leading `-`/`--` can never be parsed by `gh` as
///             a flag (SR-V1).
///
/// Always applies `--label ooda-stuck`. Surfaces EVERY outcome via `tracing`
/// + OTel before returning — `Filed` at INFO, all failure arms at ERROR — so an
/// escalation is never silently lost. Never panics; never `process::exit`s.
pub fn spawn_tracking_issue(repo: &str, title: &str, body: &str)
    -> TrackingIssueOutcome;
```

### Argument construction

`gh issue create` takes `title` and `body` as **values of `--title` / `--body`
flags**, not as positional arguments. The helper therefore hardens against
argument injection by **binding each untrusted value to its own flag with a
dedicated `.arg()` call** (never string-concatenated into the arg list and
never passed positionally). Because a value bound to `--title` is consumed as
that flag's argument, a leading `-`/`--` inside `title` or `body` can never be
re-parsed by `gh` as a separate flag:

```rust
// Validate the repo BEFORE spawning gh (fail-closed authz boundary).
if !is_valid_repo(repo) {
    // ^ matches ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$
    tracing::error!(
        target: "simard::ooda_brain",
        repo = %repo,
        "spawn_tracking_issue: invalid --repo, refusing to file (fail-closed)",
    );
    return TrackingIssueOutcome::InvalidRepo { repo: repo.to_string() };
}

let title = sanitize_issue_field(title, TITLE_MAX); // strip C0 controls, clamp ≤256
let body  = sanitize_issue_field(body,  BODY_MAX);  // strip C0 controls, clamp

let out = std::process::Command::new("gh")
    .args(["issue", "create", "--repo", repo, "--label", "ooda-stuck"])
    .arg("--title").arg(&title)  // value-bound: title is never a flag
    .arg("--body").arg(&body)    // value-bound: body is never a flag
    .output();
```

> **Note.** Because `gh issue create` has no untrusted *positional* arguments,
> value-binding alone closes SR-V1 and a trailing `--` end-of-options sentinel
> is inapplicable here. (The design spec calls for a `--` sentinel; that
> pattern applies to subcommands that take untrusted positionals — it is a
> no-op for this all-flag invocation. The equivalent guarantee is provided by
> the value-bound `.arg()` construction plus the fixed flag ordering above.)

### Outcome surfacing

Every arm is logged **before** the helper returns:

| `gh` result | `TrackingIssueOutcome` | Log level / target | OTel attributes |
| --- | --- | --- | --- |
| exit 0 | `Filed { reference }` | `INFO simard::ooda_brain` | `repo`, `reference` |
| exit ≠ 0 | `Rejected { code, stderr }` | `ERROR simard::ooda_brain` | `repo`, `exit_code`, `stderr` (bounded) |
| invalid repo | `InvalidRepo { repo }` | `ERROR simard::ooda_brain` | `repo` |
| spawn failed | `SpawnError { error }` | `ERROR simard::ooda_brain` | `repo`, `error` |

Because Simard's `tracing` layer is the OTel bridge, these structured
key=value fields **are** the OTel attributes — no separate exporter call site.
The `Rejected` arm is the specific bug #4472 fixed: a non-zero exit is now an
`ERROR` event, not a silent drop.

## Call-site changes

Both sites keep their existing control flow — the goal is still marked `Blocked`
regardless of filing outcome — and simply delegate the `gh` invocation:

### Site A — deterministic safeguard (~L370)

```rust
// Was: Command::new("gh").args([...no --repo...]).output() + eprintln! on Ok.
let outcome = spawn_tracking_issue(&escalation_repo, &title, &body);
// (goal already marked Blocked above; outcome is already fully logged.)
```

The stray `eprintln!` at ~L384 is **removed** — success is now the helper's
structured `INFO` event.

### Site B — engineer-lifecycle `OpenTrackingIssue` (~L926)

```rust
if let EngineerLifecycleDecision::OpenTrackingIssue { title, body, .. } = &decision {
    // Was: .status() with only the Err(e) arm logged — a non-zero exit was
    // silently dropped. Now every arm is surfaced by the helper.
    let _ = spawn_tracking_issue(&escalation_repo, title, body);
}
```

### `--repo` resolution

`escalation_repo` is resolved from the goal's target-repo routing
([goal target-repo routing](./goal-target-repo-routing.md)), falling back to the
Simard self-repo (`rysweet/Simard`) when a goal carries no explicit target. The
resolved value is always validated by `spawn_tracking_issue` before `gh` runs; an
unresolved/invalid repo yields `InvalidRepo` (logged, no spawn) rather than a
wrong-repo file.

## Security & fail-closed properties

- **SR-V1 argument injection.** `title` / `body` originate from goal/brain
  output. Each is bound as the value of its own `--title` / `--body` flag via a
  dedicated `.arg()` call (never concatenated, never positional), so a leading
  `-`/`--` in either field is a literal, never a `gh` flag.
- **SR-V2 field hygiene.** `sanitize_issue_field` strips C0 control characters
  and clamps `title` to ≤256 bytes and `body` to a bounded length before `gh`
  sees them.
- **SR-V3 authz boundary.** `--repo` is mandatory and validated against
  `^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$`; the target is never inferred from ambient
  `gh` context.
- **SR-A1 credential non-disclosure.** No token or environment value is ever
  written into the issue body or the logs. Only `gh`'s own stderr (bounded) is
  surfaced on `Rejected`.
- **Fail-closed & non-blocking.** Filing failure never blocks the primary
  escalation: the goal is marked `Blocked` regardless. The helper never panics
  and never `process::exit`s.

## Compatibility

- **Additive.** New helper and enum; both call sites keep their surrounding
  control flow. No public signature is removed.
- **No new inputs.** No CLI flag, config key, or RPC. The `ooda-stuck` label is
  unchanged, so existing issue filters/queries keep working.
- **No `print`-family macros / no `bridge` naming.** The stray `eprintln!` is
  replaced; all emission is structured `tracing` at INFO/ERROR.

## Testing

Hermetic, string-logic tests (no live `gh`, no network):

| Test | Asserts |
| --- | --- |
| `valid_repo_regex_accepts_owner_name` | `is_valid_repo("rysweet/Simard")` true; `"Simard"`, `"a/b/c"`, `"a b/c"` false. |
| `invalid_repo_returns_invalidrepo_without_spawn` | An invalid repo yields `InvalidRepo` and does **not** invoke `gh`. |
| `title_body_injection_sanitized` | A `title`/`body` beginning with `--force` is passed as a literal value, and C0 controls are stripped. |
| `nonzero_exit_surfaces_rejected` | A stubbed non-zero `gh` maps to `Rejected` and emits an `ERROR` event (the #4472 regression guard). |
| `spawn_error_surfaces_spawnerror` | A missing `gh` binary maps to `SpawnError` and logs at `ERROR`. |
| `success_maps_to_filed` | A 0-exit maps to `Filed`; the parsed issue reference is captured. |

Both call sites are covered by a test that asserts the goal is still marked
`Blocked` even when filing returns a failure arm.

## See also

- [No-progress breaker API](./no-progress-breaker-api.md) — the `Escalate` resolution whose `issue_title` / `issue_body` this helper files.
- [No-progress root-cause resolution API](./no-progress-root-cause-resolution-api.md) — the WHY-bearing escalation body.
- [Goal target-repo routing](./goal-target-repo-routing.md) — how `escalation_repo` is resolved.
- [Engineer-loop argv sanitization](./engineer-loop-argv-sanitization.md) — the sibling argument-injection hardening pattern.
- [How-to: diagnose a no-progress block](../howto/diagnose-a-no-progress-block.md) and [unblock stuck OODA goals](../howto/unblock-stuck-ooda-goals.md).
