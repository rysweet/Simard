---
title: How OODA spawns engineer agents
description: Intended operation for the recipe-backed OODA goal-session path that turns prompt-owned decisions into engineer subprocesses, no-action outcomes, and progress updates.
last_updated: 2026-07-13
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ./run-ooda-daemon.md
  - ../concepts/steerable-ooda-daemon.md
  - ../reference/simard-cli.md
  - ../reference/ooda-coverage-parallelism-ceiling.md
  - ../../prompt_assets/simard/goal_session_objective.md
---

# [PLANNED - Implementation Pending] How OODA spawns engineer agents

Use this guide to understand or verify the intended strict-contract behavior for
how an OODA cycle turns one active goal into either an engineer subprocess, a
no-action outcome, or a progress update.

The goal-session brain owns the judgment. Rust owns the rails:

- build the prompt input from known daemon state;
- invoke the recipe-backed brain through structured subprocess arguments;
- validate the response contract;
- spawn the engineer or record no action;
- surface invalid output as a visible failure.

For the design rationale, see
[prompt-owned OODA semantics and thin Rust rails](../concepts/steerable-ooda-daemon.md).

## Prerequisites

- Simard installed through the canonical installer path:

  ```bash
  npx github:rysweet/Simard install
  ```

  or, for a local release candidate:

  ```bash
  cargo build --release
  ./target/release/simard install
  ```

- A configured LLM provider in `$SIMARD_HOME/config.toml` or through
  `SIMARD_LLM_PROVIDER`.
- Recipe assets installed under `$SIMARD_HOME/prompt_assets` by `simard install`.
- An engineer agent available through `SIMARD_ENGINEER_AGENT`. The default is
  `copilot`; `rustyclawd` is also accepted.

## 1. Confirm the recipe-backed brain is active

Check the installed daemon logs:

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
journalctl --user -u simard-ooda.service -n 100 --no-pager
grep "RecipeBrain" "$SIMARD_HOME/ooda.log"
```

The log should name recipe-backed brain phases:

```text
[simard] OODA daemon: brain = RecipeBrain (recipe-runner-rs backed, engineer-lifecycle)
[simard] OODA daemon: decide_brain = RecipeBrain (recipe-runner-rs backed, decide)
[simard] OODA daemon: orient_brain = RecipeBrain (recipe-runner-rs backed, orient)
```

If the log contains `DEGRADED`, fix provider, recipe-runner, or prompt-asset
configuration before judging OODA behavior. A degraded fallback is visible by
design and is not the healthy architecture.

## 2. Configure the engineer runner

Set the engineer agent in the user systemd environment when the daemon runs as a
service:

```bash
systemctl --user import-environment SIMARD_ENGINEER_AGENT
systemctl --user restart simard-ooda.service
```

Examples:

```bash
export SIMARD_ENGINEER_AGENT=copilot
systemctl --user import-environment SIMARD_ENGINEER_AGENT
systemctl --user restart simard-ooda.service
```

```bash
export SIMARD_ENGINEER_AGENT=rustyclawd
systemctl --user import-environment SIMARD_ENGINEER_AGENT
systemctl --user restart simard-ooda.service
```

For per-cycle parallelism, prefer `SIMARD_OODA_MAX_CONCURRENT`:

```bash
export SIMARD_OODA_MAX_CONCURRENT=12
systemctl --user import-environment SIMARD_OODA_MAX_CONCURRENT
systemctl --user restart simard-ooda.service
```

`SIMARD_MAX_CONCURRENT_ACTIONS` remains a legacy fallback only when
`SIMARD_OODA_MAX_CONCURRENT` is unset. Raising the ceiling only permits more
independent goal coverage; resource admission and overlap gates still bound
actual spawns.

## 3. Add a bounded goal

Create a goal with a concrete done condition:

```bash
simard goal add 2 "update OODA documentation to describe prompt-owned semantics and thin Rust rails"
```

For work in another governed repository, pass the repo slug:

```bash
simard goal add 2 "fix amplihack-rs issue #808; done when the fix is merged" --repo amplihack-rs
```

The OODA daemon reads the goal board on its next cycle. For a foreground smoke
test, run one bounded cycle:

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
"$SIMARD_HOME/bin/simard" ooda run --cycles=1 "$SIMARD_HOME"
```

## 4. Understand the goal-session response contract

The goal-session prompt at
`prompt_assets/simard/goal_session_objective.md` must instruct the brain to emit
one explicit response shape before this planned contract is considered
implemented.

### Spawn an engineer

```text
ACTION: SPAWN_ENGINEER
TASK:
Check out PR #4042, fix confirmed quality-audit findings only, update the PR
body with merge-readiness evidence, wait for green checks, then merge through
`simard merge-pr`.
PROGRESS: 70
```

What happens:

1. Rust validates that `ACTION: SPAWN_ENGINEER` is the only action marker.
2. Rust requires `TASK:` and reads the prose after it as the engineer objective.
3. Rust applies `PROGRESS: 70` to the goal if present and valid.
4. Rust starts the configured engineer agent with the task prose.

The engineer objective is intentionally prose. The brain can cite PR numbers,
issue numbers, files, commands, or acceptance criteria without forcing Rust to
understand the semantics.

### Take no action

```text
NO ACTION
REASON: engineer simard-4042-finalizer is already repairing the PR branch.
PROGRESS: 80
```

What happens:

1. Rust validates that `NO ACTION` is on its own line and no spawn marker is
   present.
2. Rust requires `REASON:` and records that reason as the cycle outcome.
3. Rust applies `PROGRESS: 80` if present and valid.
4. No engineer subprocess is spawned.

Use no-action when another engineer is already working, a real external blocker
exists, or the cycle only needs to record a progress assessment.

## 5. Recognize invalid output

Invalid goal-session output fails the cycle visibly. It is not converted into a
spawn, a no-op, or a fake progress update.

| Output | Why it fails |
| --- | --- |
| Empty or whitespace-only response | There is no action contract to execute. |
| Prose with no `ACTION: SPAWN_ENGINEER` or `NO ACTION` marker | Rust would have to guess intent. |
| Both `ACTION: SPAWN_ENGINEER` and `NO ACTION` | Conflicting actions. |
| `ACTION: SPAWN_ENGINEER` without `TASK:` | There is no engineer objective. |
| `NO ACTION` without `REASON:` | There is no auditable no-action reason. |
| `PROGRESS: 125` | Progress must be `0..=100`. |
| Two different `PROGRESS:` values | Ambiguous state mutation. |
| Unknown action marker | The prompt and rails are out of sync. |

When this happens, inspect the daemon log:

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
journalctl --user -u simard-ooda.service -n 100 --no-pager
tail -n 100 "$SIMARD_HOME/ooda.log"
```

Fix the prompt or recipe contract. Do not add Rust keyword parsing to guess what
the brain "probably meant". Until the strict parser lands, the existing
compatibility path may accept non-empty free-form prose; that is not the target
behavior described by this guide.

## 6. Drive a PR to merge and deployment

When an OODA-spawned engineer opens or updates a Simard PR, the goal-session
brain should keep the next cycle focused on landing that PR instead of opening a
duplicate. A merge task should instruct the engineer to:

```text
ACTION: SPAWN_ENGINEER
TASK:
Drive PR #4042 to merge-readiness. Confirm checks are green, resolve blocking
review comments, update the PR body with evidence, merge through
`simard merge-pr 4042`, then run the standard installer deployment path from
the merged main branch and verify the installed binary is healthy.
PROGRESS: 90
```

The merge must use Simard's gated merge authority:

```bash
simard merge-pr 4042
```

After a Simard PR is merged, deploy the installed binary and assets through the
standard installer rail:

```bash
git checkout main
git pull --ff-only
cargo build --release
./target/release/simard install
systemctl --user status simard-ooda.service --no-pager
"$HOME/.simard/bin/simard" status --json
```

The install step copies the new binary and prompt assets to `SIMARD_HOME`,
reloads user systemd, and restarts the OODA and Signal units. Do not leave the
daemon running an old worktree binary after a merge.

## Configuration reference

| Setting | Default | Purpose |
| --- | --- | --- |
| `SIMARD_HOME` | `$HOME/.simard` | Install root, state root, service working directory, prompt-asset location. |
| `SIMARD_LLM_PROVIDER` | config file value | Selects the provider used by prompt-driven brains. |
| `SIMARD_ENGINEER_AGENT` | `copilot` | Selects the subordinate engineer agent; valid values are `copilot` and `rustyclawd`. |
| `SIMARD_OODA_MAX_CONCURRENT` | `24` | Preferred OODA per-cycle goal coverage ceiling, range `1..=64`. |
| `SIMARD_MAX_CONCURRENT_ACTIONS` | `24` | Legacy fallback only when `SIMARD_OODA_MAX_CONCURRENT` is unset. |

## Troubleshooting

### No engineer spawned

Check whether the brain emitted `NO ACTION`, whether an engineer is already in
flight for the goal, and whether the response failed validation:

```bash
SIMARD_HOME="${SIMARD_HOME:-$HOME/.simard}"
tail -n 100 "$SIMARD_HOME/ooda.log"
simard goal list
```

If the outcome is no-action with a real reason, no subprocess should exist. If
the outcome is an invalid contract, fix the prompt or recipe output.

### Engineer agent selection looks wrong

Confirm the environment visible to the user systemd manager:

```bash
systemctl --user show-environment | grep '^SIMARD_ENGINEER_AGENT='
```

Then re-import and restart:

```bash
export SIMARD_ENGINEER_AGENT=copilot
systemctl --user import-environment SIMARD_ENGINEER_AGENT
systemctl --user restart simard-ooda.service
```

### OODA behavior changed after editing prompts

Reinstall after changing packaged prompt or recipe assets:

```bash
cargo build --release
./target/release/simard install
```

The live service reads installed assets under `SIMARD_HOME`, not arbitrary files
from a worktree.

## See also

- [Concept: prompt-owned OODA semantics and thin Rust rails](../concepts/steerable-ooda-daemon.md)
- [How to run the OODA daemon](./run-ooda-daemon.md)
- [Simard CLI reference](../reference/simard-cli.md)
- [OODA coverage parallelism ceiling](../reference/ooda-coverage-parallelism-ceiling.md)
