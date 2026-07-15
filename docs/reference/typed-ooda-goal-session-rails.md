---
title: Typed OODA goal-session deterministic rails
description: Reference for the two thin deterministic-rail fixes in the typed OODA goal-session path — the recipe-runner AMPLIHACK_AGENT_BINARY propagation rail in route.rs (so nested agents run the operator's authenticated binary instead of defaulting to an unauthenticated "claude") and the goal-repo admission dedup rail in typed_goal_session.rs (removing a redundant, buggy "Simard"-only inline check so a goal storing a bare repo name like "agent-kgpacks-rs-audit" admits an owner-qualified "rysweet/…" spawn through the canonical goal_repository normalizer) — plus the additive Act-loop failure-detail log line. Both are zero-parser, no-silent-fallback rails.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./ooda-capability-api.md
  - ../architecture/typed-ooda-loop.md
  - ../daemon-mode.md
  - ../howto/run-ooda-daemon.md
  - ./argv-free-copilot-invocation.md
  - ../../src/typed_ooda/route.rs
  - ../../src/ooda_actions/advance_goal/typed_goal_session.rs
  - ../../src/ooda_loop/cycle.rs
  - ../../src/session_builder.rs
---

# Typed OODA goal-session deterministic rails

> **Status: implemented (issue #4076).** Two surgical deterministic-rail fixes in
> the typed OODA goal-session path, both of which were silently failing every
> live OODA goal on the installed daemon:
>
> 1. **Agent-binary propagation rail** —
>    [`src/typed_ooda/route.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/route.rs)
>    (`TypedGoalSessionRoute::execute`, `build_goal_session_command`) now sets
>    `AMPLIHACK_AGENT_BINARY` on the `recipe-runner-rs` subprocess, resolved from
>    the canonical [`LlmProvider::resolve_agent_binary`](https://github.com/rysweet/Simard/blob/main/src/session_builder.rs).
> 2. **Goal-repo admission dedup rail** —
>    [`src/ooda_actions/advance_goal/typed_goal_session.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/typed_goal_session.rs)
>    (`spawn_engineer`) **deletes** a redundant, buggy inline `"Simard"`-only repo
>    check and generalizes normalization to the single
>    [`RepositoryRef::from_goal_slug`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/types.rs)
>    (to which `goal_repository` delegates), so **any** bare goal repo name binds
>    to `rysweet/<name>` before the sole `require_goal_repository` admission
>    check — one normalizer, no duplication, and unit-testable despite the
>    handler being `cfg(not(test))`.
>
> A third, purely additive change logs the failing `outcome.detail` in the OODA
> Act outcomes loop
> ([`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs))
> so a goal failure is *explained* in the daemon log, not merely counted.
>
> These are **thin deterministic rails only**: no parsing of agent
> prose/JSON/markers, no recipe/prompt/policy asset changes, and no loosening of
> the capability / admission / idempotency / least-privilege rails. The
> zero-parser, thin-rail, and no-silent-fallback invariants are preserved.

## Contents

- [Background: the live outage](#background-the-live-outage)
- [Rail 1 — AMPLIHACK_AGENT_BINARY on the goal-session runner](#rail-1--amplihack_agent_binary-on-the-goal-session-runner)
  - [Public API](#public-api-rail-1)
  - [Resolution and failure semantics](#resolution-and-failure-semantics)
  - [Configuration](#configuration)
- [Rail 2 — goal-repo bare-name normalization](#rail-2--goal-repo-bare-name-normalization)
  - [The fix: delete the redundant check](#the-fix-delete-the-redundant-check)
  - [Normalization table](#normalization-table)
  - [Regression test](#regression-test)
- [Observability — Act-loop failure detail](#observability--act-loop-failure-detail)
- [Safety invariants](#safety-invariants)
- [Examples](#examples)
- [Migration notes](#migration-notes)
- [Related](#related)

## Background: the live outage

On the installed systemd daemon, `terminal_outcomes` stayed empty and every OODA
goal logged "consecutive failures". Two independent thin-rail defects in the
typed goal-session path were each sufficient to fail a goal:

- **The nested agent ran the wrong binary.** `route.rs` spawned
  `recipe-runner-rs` on the goal-session-actor recipe **without** setting
  `AMPLIHACK_AGENT_BINARY`. `recipe-runner-rs` resolves its agent binary from the
  `--agent-binary` flag, else the `AMPLIHACK_AGENT_BINARY` env var, else the
  hardcoded string `"claude"`. The daemon environment does not export
  `AMPLIHACK_AGENT_BINARY`, so the runner fell back to `claude` — which is
  unauthenticated on this Copilot host (`Not logged in`, exit 1). The agent step
  failed, no durable terminal was recorded, and `execute()` returned
  `RecipeFailed`/`MissingTerminal`. This was the primary, multi-day outage cause.
- **Bare goal repo names were rejected at spawn.** The typed `spawn_engineer`
  effect handler compared the actor's owner-qualified `requested_repo` (e.g.
  `"rysweet/agent-kgpacks-rs-audit"`) against the goal's stored repo, which is
  frequently a **bare** name (e.g. `"agent-kgpacks-rs-audit"`, `"skwaq"`), in a
  **redundant second check**. The handler already runs `require_goal_repository`
  first, which normalizes bare names correctly via `goal_repository`; but a stale
  inline block then re-derived the expected repo and special-cased only the single
  value `"Simard"`, so every other bare name mismatched and the spawn was rejected
  as `typed spawn repository … does not match goal repository …` **despite the
  first check admitting it**. On the live daemon this failed ~11/20 goals.

Every *other* `recipe-runner-rs` spawn site in the repo already sets
`AMPLIHACK_AGENT_BINARY` (`src/journal/recipe.rs`,
`src/stewardship/recipe_merge_judge.rs`, `src/disk_health.rs`,
`src/disk_reclaim/recipe.rs`). Rail 1 brings the goal-session runner into line
with that established pattern.

## Rail 1 — AMPLIHACK_AGENT_BINARY on the goal-session runner

`TypedGoalSessionRoute::execute()` builds the `recipe-runner-rs` `Command` for
the goal-session-actor recipe. Construction is factored into a small pure helper,
`build_goal_session_command`, so the env wiring is unit-testable; `execute()`
resolves the agent binary, applies stdio, and runs the command.

The agent binary is resolved from the **canonical** source of truth,
`crate::session_builder::LlmProvider::resolve_agent_binary()`, which reads
`SIMARD_LLM_PROVIDER` (env) then `~/.simard/config.toml` `llm_provider` and maps
the provider to its binary name (`Copilot => "copilot"`, `RustyClawd =>
"rustyclawd"`). It returns `None` only when configuration is unavailable.

### Public API {#public-api-rail-1}

```rust
// src/typed_ooda/route.rs

/// Build the `recipe-runner-rs` Command for the goal-session-actor recipe.
///
/// Pure constructor: sets the recipe path, `--no-auto-stage`, the `-C` repo
/// root, every `-c key=value` context pair, and — the Rail 1 fix —
/// `.env("AMPLIHACK_AGENT_BINARY", agent_binary)` so the nested agent runs the
/// operator's authenticated binary instead of `recipe-runner-rs`'s hardcoded
/// "claude" default. Stdio and `.status()` stay in `execute()`.
///
/// `agent_binary` is the already-resolved provider binary (see
/// `LlmProvider::resolve_agent_binary`); this helper never resolves config
/// itself, so it is deterministic and testable.
fn build_goal_session_command(
    runner: &Path,
    recipe_path: &Path,
    repo_root: &Path,
    context: &[String],
    agent_binary: &str,
) -> Command;
```

`execute()` resolves the binary and fails visibly *before* spawning:

```rust
// src/typed_ooda/route.rs — TypedGoalSessionRoute::execute()
let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()
    .ok_or_else(|| {
        CycleError::new(
            CycleErrorCode::RecipeFailed,
            "goal-session agent binary unresolved (LLM provider config unavailable)",
        )
    })?;

let mut command =
    build_goal_session_command(&runner, &self.recipe_path, repo_root, &context, agent_binary);
command.stdout(Stdio::null()).stderr(diagnostic_file);
let status = command.status().map_err(/* … RecipeFailed … */)?;
```

The built `Command` carries the env pair, verifiable via `command.get_envs()`:

```rust
// regression test (src/typed_ooda/route.rs #[cfg(test)])
let command = build_goal_session_command(runner, recipe, repo_root, &ctx, "copilot");
let has_binary = command.get_envs().any(|(k, v)| {
    k == OsStr::new("AMPLIHACK_AGENT_BINARY") && v == Some(OsStr::new("copilot"))
});
assert!(has_binary, "goal-session runner must carry AMPLIHACK_AGENT_BINARY");
```

### Resolution and failure semantics

| `resolve_agent_binary()` | `AMPLIHACK_AGENT_BINARY` on runner | `execute()` result |
| --- | --- | --- |
| `Some("copilot")` (Copilot provider) | `copilot` | recipe runs under the authenticated binary |
| `Some("rustyclawd")` (RustyClawd provider) | `rustyclawd` | recipe runs under that binary |
| `None` (config unavailable) | *(never spawned)* | `CycleError { code: RecipeFailed }` — **no spawn**, no silent `claude` |

The binary is **never** hardcoded. `"copilot"` is not a silent default — it is
returned by the resolver for the Copilot provider. When the resolver cannot
determine a provider, `execute()` fails visibly rather than letting
`recipe-runner-rs` fall through to its `"claude"` default. This is the
no-silent-fallback invariant.

### Configuration

| Source | Read by | Purpose |
| --- | --- | --- |
| `SIMARD_LLM_PROVIDER` (env) | `LlmProvider::resolve` | Highest-priority provider selection. |
| `~/.simard/config.toml` `llm_provider` | `LlmProvider::resolve` | Persistent provider selection when the env var is unset. |
| `AMPLIHACK_AGENT_BINARY` (env, **set by Simard on the child**) | `recipe-runner-rs` | The value Rail 1 now propagates; the child no longer needs it in the daemon's own environment. |

No new configuration is introduced. Operators do **not** need to add
`AMPLIHACK_AGENT_BINARY` to the systemd unit — Simard resolves it per spawn and
sets it on the child process.

## Rail 2 — goal-repo bare-name normalization

The typed `spawn_engineer` effect handler admits a spawn only when the requested
repository matches the goal's repository. The actor always produces an
owner-qualified `requested_repo` (`{owner}/{name}`), but a goal frequently stores
a **bare** repo name. The rail's job is to canonicalize the goal's stored value
to an owner-qualified form before the equality check so bare names like
`"agent-kgpacks-rs-audit"` admit a `rysweet/agent-kgpacks-rs-audit` spawn.

**The canonicalization lives in one place** — `RepositoryRef::from_goal_slug`
in [`src/typed_ooda/types.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/types.rs),
the single source of truth for goal-repo normalization. The `goal_repository`
helper in the (test-excluded) effect handler **delegates** to it:

```rust
// src/typed_ooda/types.rs — the single normalizer (unit-testable here)
impl RepositoryRef {
    pub fn from_goal_slug(slug: Option<&str>) -> Self {
        match slug {
            None => Self::new("rysweet", "Simard"),          // default goal repo
            Some(value) => match value.split_once('/') {
                Some((owner, name)) => Self::new(owner, name), // qualified: preserved
                None => Self::new("rysweet", value),           // bare: rysweet/<name>
            },
        }
    }
}

// src/ooda_actions/advance_goal/typed_goal_session.rs — delegates, no duplication
fn goal_repository(goal: &ActiveGoal) -> Result<RepositoryRef, String> {
    Ok(RepositoryRef::from_goal_slug(goal.repo.as_deref()))
}
```

`spawn_engineer` already invokes this via `require_goal_repository` at the **top**
of the handler, *before* any spawn work:

```rust
self.require_goal_repository(goal_id, &spawn.repository)?;
// require_goal_repository normalizes with goal_repository(goal) and compares the
// resulting RepositoryRef against spawn.repository.
```

For a bare goal repo `"agent-kgpacks-rs-audit"` and a spawn of
`rysweet/agent-kgpacks-rs-audit`, this first check already **passes**.

### The fix: delete the redundant check {#the-fix-delete-the-redundant-check}

The defect is a **second, redundant** repo check that runs *after*
`require_goal_repository` and re-derives the expected repo with a buggy
`"Simard"`-only branch:

```rust
// src/ooda_actions/advance_goal/typed_goal_session.rs — the buggy inline block (REMOVED)
let expected_repo = goal_repo.as_deref().unwrap_or("rysweet/Simard");
let expected_repo = if expected_repo == "Simard" { "rysweet/Simard" } else { expected_repo };
let requested_repo = format!("{}/{}", spawn.repository.owner, spawn.repository.name);
if requested_repo != expected_repo {
    return Err(EffectExecutionError::permanent(format!(
        "typed spawn repository {requested_repo:?} does not match goal repository {expected_repo:?}"
    )));
}
```

Only `"Simard"` gets the `rysweet/` prefix here; every *other* bare name (e.g.
`"agent-kgpacks-rs-audit"`, `"skwaq"`) stays bare and mismatches the
owner-qualified `requested_repo`, so the spawn is rejected **even though
`require_goal_repository` already admitted it**.

**The fix has two halves, and neither loosens the rail.** First, the redundant
inline block is **deleted** — `require_goal_repository` (which routes through
`goal_repository` → `from_goal_slug`) is the sole admission gate. Second, the
normalization rule is generalized: instead of an inline `"Simard"`-only branch,
`goal_repository` delegates to `RepositoryRef::from_goal_slug`, which prefixes
**any** bare name with `rysweet/`. That keeps normalization in exactly **one**
place — `from_goal_slug` — with `goal_repository` a one-line delegate, so there
is no second normalizer that can drift.

Why does the normalizer live on `RepositoryRef` rather than inline in the
handler? Because the effect handler module
(`src/ooda_actions/advance_goal/typed_goal_session.rs`) is compiled only under
`cfg(not(test))` and is **excluded from test builds** — its functions cannot be
unit-tested directly. Hoisting the rule to `RepositoryRef::from_goal_slug` in the
always-compiled `typed_ooda::types` module makes the single source of truth
directly testable without duplicating it.

The former `(goal_repo, already_assigned)` binding is retained: `already_assigned`
guards the double-assignment case and `goal_repo` still feeds
`repo_resolver::resolve_goal_repo` for worktree placement. Only the redundant
equality *check* is removed.

### Normalization table

All rows are produced by `RepositoryRef::from_goal_slug` (reached via
`goal_repository` → `require_goal_repository`), the sole normalizer after this fix:

| Goal repo (stored) | `from_goal_slug` | Requested spawn | Admission |
| --- | --- | --- | --- |
| `Some("agent-kgpacks-rs-audit")` | `rysweet/agent-kgpacks-rs-audit` | `rysweet/agent-kgpacks-rs-audit` | **allowed** (was rejected before) |
| `Some("skwaq")` | `rysweet/skwaq` | `rysweet/skwaq` | **allowed** (was rejected before) |
| `Some("Simard")` | `rysweet/Simard` | `rysweet/Simard` | **allowed** (now a subset of the bare-name rule) |
| `None` | `rysweet/Simard` | `rysweet/Simard` | **allowed** (default) |
| `Some("otherowner/thing")` | `otherowner/thing` | `rysweet/thing` | **rejected** (owner mismatch preserved) |

The last row is the security-relevant case: an already-qualified value with a
*different* owner is compared as-is (via `RepositoryRef` equality) and still
mismatches, so bare-name normalization can never smuggle a cross-owner spawn past
admission.

### Regression test

The single normalizer `RepositoryRef::from_goal_slug` is pinned by four cases in
`src/typed_ooda/types.rs` (the effect handler is `cfg(not(test))`, so the contract
is asserted at its always-compiled normalization seam). Each case pairs the
normalized goal slug with the actor's owner-qualified request:

```rust
// src/typed_ooda/types.rs #[cfg(test)]
// (a) bare name admits the rysweet-scoped request
assert_eq!(RepositoryRef::from_goal_slug(Some("agent-kgpacks-rs-audit")),
           RepositoryRef::new("rysweet", "agent-kgpacks-rs-audit"));
// (b) bare "Simard" is a subset of the general rule
assert_eq!(RepositoryRef::from_goal_slug(Some("Simard")),
           RepositoryRef::new("rysweet", "Simard"));
// (c) None defaults to rysweet/Simard
assert_eq!(RepositoryRef::from_goal_slug(None),
           RepositoryRef::new("rysweet", "Simard"));
// (d) explicit different owner is preserved and still mismatches rysweet/thing
assert_eq!(RepositoryRef::from_goal_slug(Some("otherowner/thing")),
           RepositoryRef::new("otherowner", "thing"));
assert_ne!(RepositoryRef::from_goal_slug(Some("otherowner/thing")),
           RepositoryRef::new("rysweet", "thing"));
```

## Observability — Act-loop failure detail

In the OODA Act outcomes loop (`src/ooda_loop/cycle.rs`), where
`scaler.report_reason(...)` runs and the consecutive-failures counter is
incremented, a single additive line logs the failing `outcome.detail`:

The goal id lives on `outcome.action.goal_id` (an `Option<String>`), **not** a
direct `outcome.goal_id` field, so the line must sit inside the existing
`if let Some(goal_id) = &outcome.action.goal_id` block (cycle.rs ~line 378),
next to the consecutive-failures counter:

```rust
// src/ooda_loop/cycle.rs — inside `if let Some(goal_id) = &outcome.action.goal_id`,
// in the failure (`else`) branch next to the consecutive-failures counter.
// Additive; no control-flow change.
eprintln!(
    "[simard] OODA cycle: goal '{goal_id}' failure detail: {}",
    truncate_detail(&outcome.detail, 240)
);
```

This is a pure additive log line — no control-flow change, no new fields beyond
the already-present `outcome.action.goal_id` / `outcome.detail`, no payload
dumps, and no secrets.

> **Accuracy caveat.** `outcome.detail` is **not** entirely unlogged today: on
> failure the loop already folds it into `goal.current_activity` as
> `"{kind} (failed): {detail}"` (cycle.rs ~line 407) and there is a `tracing`
> record carrying `detail = %outcome.detail` (~line 677). The value of this line
> is a *stderr* explanation co-located with the consecutive-failures counter, so
> the daemon's stderr log explains a failure at the same site it counts it. The
> claim should be "surfaced next to the failure counter", not "otherwise
> unexplained".

## Safety invariants

- **Zero parser.** Neither rail parses agent prose, JSON, or markers. Rail 1
  wires an environment variable; Rail 2 removes a redundant string check and
  keeps the surviving deterministic `from_goal_slug` normalization + equality
  compare.
- **No silent fallback (Rail 1).** An unresolvable provider yields a visible
  `CycleError { RecipeFailed }` *before* spawning. Simard never lets
  `recipe-runner-rs` default to `claude` and never hardcodes a binary.
- **Admission not loosened (Rail 2).** The fix *removes* a redundant,
  stricter-but-buggy inline check; the surviving `require_goal_repository` →
  `goal_repository` → `from_goal_slug` path prefixes only bare names (no `'/'`)
  with `rysweet/` and compares any already-qualified value verbatim, so an
  explicit different owner still mismatches. The capability, admission,
  idempotency, and least-privilege rails are unchanged.
- **Argv/env-only, no shell.** All subprocess inputs are passed via
  `Command::arg` / `Command::env` (execve argv/envp) — there is no `sh -c`, so a
  malicious `goal.repo` becomes an inert rail string, never a shell argument. The
  `AMPLIHACK_AGENT_BINARY` value is a `&'static str` provider enum mapping, not
  user input, so there is no env-injection surface.
- **No asset changes.** No recipe, prompt, or policy asset is touched.
- **Additive observability only.** The Act-loop log line adds no control flow and
  no sensitive data.

## Examples

### Confirm the goal-session runner carries the right binary

```bash
# On the daemon host, a live OODA cycle now records durable terminals instead of
# "consecutive failures". The nested recipe runs under the resolved provider
# binary (copilot), not claude.
grep 'terminal' ~/.simard/ooda.log | tail
```

### Confirm a bare-repo goal spawns

```bash
# A goal whose stored repo is a bare name now admits an owner-qualified spawn.
simard goal show agent-kgpacks-rs-audit    # repo: "agent-kgpacks-rs-audit"
# The next Act cycle spawns an engineer for rysweet/agent-kgpacks-rs-audit
# instead of rejecting it with "does not match goal repository".
```

### Read the new failure explanation

```bash
# When a goal does fail, the daemon log now explains WHY, not just that it failed.
grep 'OODA cycle: goal .* failure detail:' ~/.simard/ooda.log | tail
```

## Migration notes

- **No config change required.** Existing deployments gain both rails
  automatically. Operators do **not** need to export `AMPLIHACK_AGENT_BINARY` in
  the systemd unit — Simard resolves and sets it per spawn.
- **Goals with bare repo names now work.** Previously-rejected goals
  (`agent-kgpacks-rs-audit`, `skwaq`, and any other bare name) admit spawns after
  this change with no goal-board edits.
- **Cross-owner goals are unaffected.** A goal that explicitly stores
  `owner/repo` for a non-`rysweet` owner keeps its exact admission behavior.

## Related

- [OODA capability API](./ooda-capability-api.md) — terminal schemas,
  authorization, and the effect leases the goal-session route records into.
- [Typed-capability OODA architecture](../architecture/typed-ooda-loop.md) — the
  actor-session authority and durable-terminal model these rails sit inside.
- [Daemon mode](../daemon-mode.md) / [Run the OODA daemon](../howto/run-ooda-daemon.md)
  — the loop that spawns the goal-session runner each cycle.
- [Argv-free Copilot invocation](./argv-free-copilot-invocation.md) — the
  companion no-silent-fallback binary-resolution contract for agent spawns.
