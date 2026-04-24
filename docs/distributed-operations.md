---
title: Distributed operations
description: How Simard coordinates many engineer subprocesses concurrently on one host today, and what the architecture leaves open for multi-host extension.
last_updated: 2026-04-24
owner: simard
doc_type: concept
---

# Distributed operations

Simard's autonomous mode is *concurrent*: a single OODA daemon dispatches many engineer subprocesses in parallel, each in its own git worktree and its own LLM session. This page describes what is true today and what is explicitly not.

## What is distributed today

**Multi-engineer concurrency on a single host.** A running daemon can have many engineer subprocesses in flight simultaneously. They are coordinated through:

- **Per-engineer git worktrees** under `~/.simard/engineer-worktrees/<goal-id>-<epoch>-<6hex>/` — each engineer gets an isolated checkout so concurrent edits never collide on the working tree. The orphan-sweep guard in `src/engineer_worktree/mod.rs` reclaims worktrees only when their owning subprocess is no longer alive.
- **Shared goal register** at `~/.simard/goals/` — the daemon's single source of truth for what each engineer is supposed to do. In-flight goals are marked busy so they cannot be double-dispatched.
- **In-process hive event bus** (`src/hive_event_bus.rs`) — a `tokio::sync::broadcast` channel that every in-process subsystem (memory consolidation, meeting facilitator, gym runner, engineer dispatcher) can publish to and subscribe from. This is the substrate for cross-agent knowledge sharing.
- **Shared cognitive memory** (LadybugDB at `~/.simard/memory/lbug/`) — semantic, procedural, and prospective layers are visible to every engineer dispatch on the host.

The dashboard's **Processes** tab makes this concurrency visible: the live process tree, per-engineer worktree paths, and per-process resource usage are all surfaced in real time. The **Active Processes** counter on the Overview tab shows how many child processes the daemon currently owns.

## What is *not* distributed today

**The hive event bus is in-process only.** From `src/hive_event_bus.rs`:

> The bus is **in-process only**. It does not cross process or machine boundaries. A future workstream may add a network adapter that bridges this bus to a remote transport; nothing in this module assumes one.

There is no built-in multi-host transport. Two daemons running on two different hosts do not share an event bus, do not share working/sensory memory, and do not coordinate goal dispatch through any network protocol that ships in the binary today.

## What you can do across hosts (out-of-band)

The on-disk artifacts under `~/.simard/` are designed so operators can manually federate state when they want to:

| Artifact | Path | Federation pattern |
|----------|------|--------------------|
| Goal register | `~/.simard/goals/` | Stage on shared storage (NFS / object store) and have each daemon point its `--state-root` there. Goal-locking is file-based. |
| Persistent memory | `~/.simard/memory/lbug/` | Single-writer; safe to snapshot and rsync between hosts when no daemon is writing. Cross-session recall works after restore. |
| Engineer worktrees | `~/.simard/engineer-worktrees/` | **Do not share across hosts** — worktrees are bound to the host's filesystem and process table. |
| Gym results | `~/.simard/gym/` | Append-only JSONL; safe to merge between hosts. |
| Cost ledger | `~/.simard/costs/` | Append-only JSONL; safe to merge. |

A common pattern is one "primary" daemon host that owns the goal register and persistent memory, and ephemeral "worker" hosts that mount the same `~/.simard/goals/` and `~/.simard/memory/` over a network filesystem and run engineer dispatches against shared state. This is operator-assembled today, not a built-in feature.

## What is on the roadmap

A network transport for the hive event bus, plus a goal-dispatch lock service, would turn the multi-engineer concurrency model into a true multi-host one without changing the agent code paths. Tracking issue: [#949](https://github.com/rysweet/Simard/issues/949).

## Code entry points

- `src/operator_commands_ooda/daemon.rs` — the cycle that dispatches engineers
- `src/engineer_worktree/mod.rs` — per-engineer worktree allocation and orphan sweep
- `src/hive_event_bus.rs` — in-process pub/sub
- `src/memory_hive.rs` — hive-aware memory layer

## Related

- [Daemon mode (autonomous OODA loop)](daemon-mode.md)
- [Memory architecture](memory.md)
- [Inspect and clean engineer worktrees](howto/inspect-and-clean-engineer-worktrees.md)
