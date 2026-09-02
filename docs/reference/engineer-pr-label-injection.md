---
title: Engineer-PR Label Injection API reference
description: >
  How Simard stamps every autonomous engineer / recipe PR with the
  simard-autonomous marker at creation time by exporting WORKFLOW_PR_LABELS to
  the amplihack publish step. Covers the WORKFLOW_PR_LABELS_ENV single-source
  constant, the five PR-producing spawn/recipe seams that set it (direct-exec
  spawn, tmux compute_tmux_env, Overseer recipe launch, monthly self-quality
  audit, and the engineer-loop recipe bin), the tmux forwarding gotcha, the
  paired amplihack-rs workflow_publish_pr.sh consumer contract, and the seam
  unit tests that pin the wire var name.
last_updated: 2026-07-23
owner: simard
doc_type: reference
status: reference
related:
  - ./ready-prs-sensor-api.md
  - ./cross-repo-merge-authority.md
  - ./self-quality-audit-api.md
  - ../concepts/autonomous-self-merge-sensor.md
  - ../design/agentic-observe-orient-merge-queue.md
---

# Engineer-PR Label Injection API reference

Every PR that a Simard autonomous engineer or recipe opens is stamped with the
`simard-autonomous` label at creation time. This label is the primary
self-identifying marker the [`ready_prs` sensor](./ready-prs-sensor-api.md) and
[merge authority](./cross-repo-merge-authority.md) use to tell Simard's own
merge-ready PRs apart from the operator's review PRs when both are authored by
the same `gh` login (e.g. `rysweet`).

Simard's engineers commit as the operator onto shared `feat/*` / `fix/*`
branches, which are deliberately excluded from the engineer branch-prefix
filter (operators use them too). On a shared-prefix branch the label is the
**only** thing that qualifies a PR as a self-merge candidate. This page
documents how Simard guarantees that label is applied to every autonomous,
PR-producing invocation.

The mechanism is a single environment variable, `WORKFLOW_PR_LABELS`, exported
onto every PR-producing spawn/recipe command. The amplihack publish step reads
it and applies the labels best-effort at `gh pr create` success.

## Why this exists (relationship to the prompt instruction)

This env-var mechanism does **not** replace an existing path — it hardens it.
The engineer system prompt (`prompt_assets/simard/engineer_system.md`) already
instructs the agent to *"Label every PR you open with `simard-autonomous`"* by
passing `--label simard-autonomous` to `gh pr create` (or adding it afterward
with `gh pr edit`). That path is **advisory and LLM-dependent**: a model that
forgets the instruction, uses a different publish path, or has its context
truncated opens an unlabeled PR — which then silently fails the self-merge
gate on shared `feat/*` / `fix/*` branches.

`WORKFLOW_PR_LABELS` closes that reliability gap deterministically:

| Path | Trigger | Reliability | Role |
|---|---|---|---|
| Prompt instruction (`engineer_system.md`) | LLM chooses to run `gh pr create --label` | Best-effort, model-dependent | Advisory / defense-in-depth (retained, not removed) |
| `WORKFLOW_PR_LABELS` env (this doc) | amplihack publish step, mechanical | Deterministic on every publish-tool PR | Primary guarantee |

The two are **complementary and idempotent** — the label matches whole-string
(`is_engineer_pr_label`), so both paths applying `simard-autonomous` is a
harmless no-op. The env-var path is authoritative for any PR opened through the
amplihack publish tool; the prompt instruction remains as defense-in-depth for
any agent path that bypasses that tool.

## How it works

```
Simard spawn/recipe site
  └─ sets env  WORKFLOW_PR_LABELS = "simard-autonomous"
       └─ child runs amplihack / recipe-runner-rs
            └─ workflow_publish_pr.sh (amplihack-rs)
                 └─ on PR-publish success:
                      apply_pr_labels_best_effort() reads WORKFLOW_PR_LABELS
                      → applies each label to the new PR via `gh` (best-effort)
```

Simard owns only the **producer** side (setting the env). The **consumer**
side — parsing `WORKFLOW_PR_LABELS` and calling `gh` — lives in the
amplihack-rs `workflow_publish_pr.sh` publish tool.

## The `WORKFLOW_PR_LABELS_ENV` constant

The wire name of the environment variable is defined exactly once, in
`src/overseer/config.rs`, next to
[`SIMARD_ENGINEER_PR_LABEL`](./ready-prs-sensor-api.md):

```rust
/// The environment variable the amplihack publish step (`workflow_publish_pr.sh`)
/// reads for best-effort PR labels (comma-separated). Every autonomous,
/// PR-producing recipe/engineer invocation sets this to
/// [`SIMARD_ENGINEER_PR_LABEL`] so its published PR is visible to the self-merge
/// queue. Kept here as the single grep-able anchor so the many spawn sites can
/// never drift on the var name.
pub const WORKFLOW_PR_LABELS_ENV: &str = "WORKFLOW_PR_LABELS";
```

Every spawn site references `config::WORKFLOW_PR_LABELS_ENV` and
`config::SIMARD_ENGINEER_PR_LABEL` — never a magic string. There is exactly one
place to look, and one place a rename can happen.

- **Env var name (wire contract):** `WORKFLOW_PR_LABELS`
- **Value:** `simard-autonomous` (the compile-time constant
  `SIMARD_ENGINEER_PR_LABEL`)
- **Format:** comma-separated label list (single label today)

> **Note:** From bin targets, `config` is reached via the crate path
> `simard::overseer::config::…`; from library modules via `crate::overseer::config::…`.

## Configuration

There is nothing to configure. Label injection is unconditional and additive on
every autonomous PR-producing invocation.

| Aspect | Value |
|---|---|
| Env var | `WORKFLOW_PR_LABELS` |
| Value | `simard-autonomous` |
| Enable/disable | Not configurable — always set (fail-safe, additive) |
| Consumer behavior when unset | Old publish script ignores the var (no-op) |

The label **value** is always the compile-time constant
`SIMARD_ENGINEER_PR_LABEL`. It is never derived from PR titles, branch names, or
inbound environment — this keeps a spoofed look-alike label from ever reaching
the merge gate.

## The five PR-producing seams

All five autonomous, PR-producing spawn/recipe sites export the env through the
shared constant. Non-PR-producing sites (self-improve cycle, freshness-gate
`amplihack update`, and the observer/judge recipe-runner-rs calls) are
deliberately excluded.

| # | Site | File | How the env is set |
|---|---|---|---|
| a | Direct-exec spawn (non-tmux fallback) | `src/agent_supervisor/lifecycle/spawn.rs` | `.env(WORKFLOW_PR_LABELS_ENV, SIMARD_ENGINEER_PR_LABEL)` on the `Command` |
| b | tmux-wrapped engineer | `src/agent_supervisor/tmux.rs` → `compute_tmux_env()` | pair pushed onto the `tmux_env` vec (`tmux -e` flags) |
| c | Overseer fix-launch recipe | `src/overseer/launch.rs` → `build_overseer_recipe_command()` | `.env(…)` on the `amplihack recipe run smart-orchestrator` command |
| d | Monthly self-quality audit | `src/self_quality_audit.rs` → `build_audit_command()` | `.env(…)` beside `AMPLIHACK_AGENT_BINARY` on the `recipe-runner-rs` command |
| e | Engineer-loop recipe bin | `src/bin/simard_engineer_loop_recipe.rs` → `build_engineer_loop_command()` | `.env(…)` on the `amplihack recipe run simard-engineer-loop.yaml` command |

### (a) Direct-exec spawn

The non-tmux fallback path in `spawn_subordinate` builds a `Command` for the
`engineer run single-process` child. It carries the label env alongside
`SIMARD_AGENT_NAME` and `CARGO_BUILD_JOBS`:

```rust
cmd.env(config::WORKFLOW_PR_LABELS_ENV, config::SIMARD_ENGINEER_PR_LABEL);
```

### (b) tmux `compute_tmux_env()` — the forwarding gotcha

Production engineers are tmux-wrapped, so this is the seam that matters in
production. `compute_tmux_env()` does **not** inherit the parent process
environment wholesale — it forwards only a fixed seed set plus every `SIMARD_*`
var. Because `WORKFLOW_PR_LABELS` is **not** a `SIMARD_*` var, the `.env()` call
on the direct-exec command in (a) never reaches the tmux child. The pair must be
seeded explicitly here:

```rust
tmux_env.push((
    config::WORKFLOW_PR_LABELS_ENV.to_string(),
    config::SIMARD_ENGINEER_PR_LABEL.to_string(),
));
```

The `compute_tmux_env` rustdoc lists `WORKFLOW_PR_LABELS` among the seeded vars
and notes it is the one non-`SIMARD_*` var that is explicitly forwarded.

### (c) Overseer recipe launch

The Overseer's `RealChildSpawner` runs `amplihack recipe run
smart-orchestrator` to fix a goal — a real PR producer. Command construction is
extracted into a testable seam:

```rust
fn build_overseer_recipe_command(brief: &RecipeBrief) -> Command {
    let mut cmd = Command::new("amplihack");
    cmd.args(smart_orchestrator_args(brief))
        .env(config::WORKFLOW_PR_LABELS_ENV, config::SIMARD_ENGINEER_PR_LABEL);
    cmd
}
```

The helper returns only the argv + env. The secret-safe stdout/stderr piping to
the owner-only (`0600`) temp log (PR #4142) stays at the call site, applied to
the returned `Command` before `spawn()` — the extraction must not absorb it, or
recipe output could leak to a world-readable fd.

### (d) Monthly self-quality audit

The monthly audit opens PRs against `rysweet/Simard` via `recipe-runner-rs`.
The command is extracted into `build_audit_command(...)`, which sets the label
env beside the existing `AMPLIHACK_AGENT_BINARY`:

```rust
fn build_audit_command(
    recipe_path: &Path,
    state_root: &Path,
    repo_root: &Path,
    agent_binary: &str,
) -> Command {
    let mut cmd = Command::new("recipe-runner-rs");
    cmd.arg(recipe_path.as_os_str())
        .arg("--output-format")
        .arg("json")
        .env("AMPLIHACK_AGENT_BINARY", agent_binary)
        .env(
            crate::overseer::config::WORKFLOW_PR_LABELS_ENV,
            crate::overseer::config::SIMARD_ENGINEER_PR_LABEL,
        )
        .arg("-c")
        .arg(format!("state_root={}", state_root.display()))
        .arg("-c")
        .arg(format!("repo_path={}", repo_root.display()));
    cmd
}
```

### (e) Engineer-loop recipe bin

The `simard_engineer_loop_recipe` bin runs `amplihack recipe run
simard-engineer-loop.yaml`. Command construction is extracted into
`build_engineer_loop_command(...)`, which sets the env via the `simard::` crate
path:

```rust
fn build_engineer_loop_command(
    recipe_path: &str,
    workspace: &str,
    objective: &str,
    topology: &str,
    state_root: &str,
) -> Command {
    let mut cmd = Command::new("amplihack");
    cmd.args(["recipe", "run", recipe_path, /* -c … */])
        .env(
            simard::overseer::config::WORKFLOW_PR_LABELS_ENV,
            simard::overseer::config::SIMARD_ENGINEER_PR_LABEL,
        );
    cmd
}
```

## Consumer contract (amplihack-rs `workflow_publish_pr.sh`)

The paired amplihack-rs change adds `apply_pr_labels_best_effort()` to
`amplifier-bundle/tools/workflow_publish_pr.sh`. On PR-publish success it:

- reads the comma-separated `WORKFLOW_PR_LABELS` env var,
- applies each label to the freshly published PR via `gh`,
- is **best-effort**: it no-ops / warns when the var, `gh`, host, or PR number
  is absent, and skips PRs already `MERGED` / `CLOSED`,
- **never** changes the publish step's exit code or stdout.

Because the value is a fixed, charset-restricted constant
(`simard-autonomous` matches `^[a-z-]+$`), no shell metacharacter can reach the
consumer.

### Forward/backward compatibility

- **Before the consumer ships:** the old publish script ignores the unknown
  `WORKFLOW_PR_LABELS` var, so this Simard change is inert and lands safely on
  its own.
- **When unset:** existing behavior is unchanged (no label applied).
- **Additive & infallible:** setting the env is `.env()` — it can never fail
  and PR creation is never gated on the label. A child overriding the value
  causes only cosmetic mislabeling.

## Tests

Each testable seam has a unit test that builds the `Command` (or seeds the
tmux env vec) and asserts the exported pair, comparing via `OsStr::new(...)`:

- **(b)** `compute_tmux_env()` includes `("WORKFLOW_PR_LABELS",
  "simard-autonomous")`.
- **(c)** `build_overseer_recipe_command(...).get_envs()` contains
  `WORKFLOW_PR_LABELS_ENV == SIMARD_ENGINEER_PR_LABEL`.
- **(d)** `build_audit_command(...).get_envs()` contains the pair beside
  `AMPLIHACK_AGENT_BINARY`.
- **(e)** `build_engineer_loop_command(...).get_envs()` contains the pair.

At least one assertion pins the **literal** wire name `"WORKFLOW_PR_LABELS"`, so
a silent rename of the constant value cannot break the shell contract with the
consumer.

Seam (a) relies on the shared constant plus the other seams' coverage; the
direct-exec path is exercised indirectly.

## Security posture

- **Constant-only value.** The label is always `SIMARD_ENGINEER_PR_LABEL`;
  never derived from PR titles, branch names, or passthrough env. This prevents
  a spoofed look-alike label from reaching the merge gate.
- **Injection-safe charset.** `simard-autonomous` contains only `[a-z-]`; no
  shell metacharacters can reach the consumer's `gh` invocation.
- **No secrets.** `WORKFLOW_PR_LABELS` carries public metadata only and is safe
  in `ps auxe`, tmux env dumps, and CI logs. This seam (especially the tmux
  path) must never be reused for secrets.
- **Fail-safe blast radius.** `.env()` is infallible and additive; PR creation
  is never conditional on the label. There is no new panic, error, or DoS path.

## Related

- [`ready_prs` sensor API](./ready-prs-sensor-api.md) — consumes the label via
  `SIMARD_ENGINEER_PR_LABEL` / `is_engineer_pr_label()`.
- [Cross-Repo Merge Authority](./cross-repo-merge-authority.md) — the
  authoritative merge decision.
- [Self-Quality-Audit API](./self-quality-audit-api.md) — seam (d)'s owner.
- [Autonomous self-merge sensor concept](../concepts/autonomous-self-merge-sensor.md)
  — the *why* and safety posture.
- `prompt_assets/simard/engineer_system.md` (§"Label every PR you open") — the
  advisory prompt-instruction path this env-var mechanism hardens (retained as
  defense-in-depth).
