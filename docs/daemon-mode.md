---
title: Daemon mode (autonomous OODA loop)
description: How Simard runs as a long-lived process, observing signals, ranking priorities, dispatching engineer subprocesses, and coordinating distributed work.
last_updated: 2026-04-24
owner: simard
doc_type: concept
---

# Daemon mode (autonomous OODA loop)

When Simard is launched in daemon mode she becomes a long-lived process that runs the **Observe → Orient → Decide → Act → Review** loop on a timer. Each cycle she inspects the goal register and the world (issues, gym scores, meeting handoffs, memory consolidation pressure), ranks priorities, selects an action, dispatches it through one of her base-type adapters, and records the outcome.

Daemon mode is what makes Simard *autonomous* rather than *interactive*. It is the same code path that operator-driven sessions use; the only difference is that the daemon is the operator.

## Start the daemon

```bash
# Run a fixed number of cycles (good for smoke tests)
simard ooda run --cycles=5

# Run indefinitely
simard ooda run
```

The daemon sleeps `60s` between cycles by default and emits a one-line summary of each cycle to stderr. Dashboard cycle numbers map 1:1 to these iterations.

For a full how-to including systemd-user installation, see [Run the OODA daemon](howto/run-ooda-daemon.md).

## What the daemon observes

Each cycle she pulls signals from:

- **Goal register** — `simard goal-curation` priorities and the proposed backlog.
- **Open issues** — `gh issue list` against the tracked repository.
- **Gym scores** — recent benchmark results and any regressions vs. baseline.
- **Meeting handoffs** — files written by `simard meeting repl` that mark decisions ready for engineering.
- **Memory consolidation pressure** — when working memory crosses thresholds, a consolidation action is preferred over a new engineer dispatch.
- **In-flight work** — already-running engineer subprocesses are skipped to avoid duplicate dispatch.

## Actions she can take

The daemon dispatches one action per cycle. Action kinds include:

| Action | What it does |
|--------|--------------|
| `advance-goal` | Spawn an `engineer` subprocess in a per-engineer worktree and let it pursue a single bounded task. |
| `run-improvement` | Run a self-improvement cycle (eval → analyze → improve → re-eval). |
| `run-gym-eval` | Execute a benchmark scenario and record results. |
| `consolidate-memory` | Promote working / episodic items into semantic / procedural memory. |
| `research` | Issue a focused research query and persist findings. |
| `assess-only` | When a goal cannot be safely dispatched (e.g. ambiguous scope), record the assessment and defer. |

Each engineer dispatch:

1. Allocates a per-engineer git worktree under `~/.simard/engineer-worktrees/<goal-id>-<epoch>-<6hex>/` so concurrent engineers cannot collide.
2. Spawns the engineer subprocess with that worktree as its CWD.
3. Verifies the engineer's output via the `verify` gate (`src/engineer_loop/verification.rs`) before any branch push.
4. Records the cycle in episodic memory and updates the goal status.

## Engineer subprocesses

Engineer dispatches are first-class subprocesses — independent OS processes with their own LLM session, tool budget, and worktree. The daemon does not block on them; it polls completion and applies the verifier on next cycle.

For the worktree contract see [Inspect and clean engineer worktrees](howto/inspect-and-clean-engineer-worktrees.md). For the spawn semantics see [Spawn engineers from the OODA daemon](howto/spawn-engineers-from-ooda-daemon.md).

## Inspect and control

Operators interact with a running daemon through the dashboard ([Dashboard](dashboard.md)) and the CLI:

```bash
simard ooda status            # last cycle summary, current state
simard goal-curation read     # active goals + backlog
simard improvement-curation read  # pending improvements awaiting approval
```

## Distributed operation

A single daemon coordinates many engineer subprocesses on the same host through per-engineer worktrees and a shared goal register. For multi-host operation see [Distributed operations](distributed-operations.md).

## Formal contract

The end-to-end OODA contract — including the durable goal-board format, the verify gate, the priority ranking, and the in-flight dedup — is specified in [Specs/ProductArchitecture.md](https://github.com/rysweet/Simard/blob/main/Specs/ProductArchitecture.md). Code-level entry points:

- `src/operator_commands_ooda/daemon.rs` — the loop itself
- `src/engineer_worktree/mod.rs` — per-engineer worktree allocation and orphan sweep
- `src/engineer_loop/verification.rs` — the verify gate
- `src/self_improve/cycle.rs` — improvement-cycle action

## Related

- [Dashboard](dashboard.md) — observe a running daemon
- [Memory architecture](memory.md) — what the daemon writes between cycles
- [Run the OODA daemon (how-to)](howto/run-ooda-daemon.md)
- [Spawn engineers from the OODA daemon](howto/spawn-engineers-from-ooda-daemon.md)
