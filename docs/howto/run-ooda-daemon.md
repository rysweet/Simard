---
title: How to run the OODA daemon
description: Start the continuous OODA loop so Simard autonomously observes goals, prioritizes work, and dispatches bounded actions.
last_updated: 2026-04-03
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/simard-cli.md
  - ../architecture/overview.md
  - ../howto/carry-meeting-decisions-into-engineer-sessions.md
---

# How to run the OODA daemon

The OODA daemon lets Simard operate autonomously: she observes her goal board, ranks priorities, selects actions, dispatches them, and repeats on a timer.

## Prerequisites

- Simard binary built (`cargo build --quiet`)
- Python ecosystem bridges available (cognitive memory, knowledge packs, gym) — the daemon launches these automatically
- `ANTHROPIC_API_KEY` set if RustyClawd-backed actions are enabled
- Goal board populated (via `simard goal-curation run` or meeting sessions)

## Start the daemon

Run a fixed number of cycles:

```bash
simard ooda run --cycles=5 "$PWD/target/simard-ooda"
```

Run indefinitely (omit `--cycles` or pass `--cycles=0`):

```bash
simard ooda run
```

The daemon sleeps 60 seconds between cycles and logs one-line summaries to stderr.

## Override defaults

| Variable | Default | Purpose |
| --- | --- | --- |
| `SIMARD_STATE_ROOT` | `/tmp/simard-ooda` | State root directory (overridden by the positional `[state-root]` argument) |
| `SIMARD_AGENT_NAME` | `simard-ooda` | Agent name for bridge registration |
| `ANTHROPIC_API_KEY` | (none) | Required when RustyClawd-backed actions are enabled |

```bash
SIMARD_STATE_ROOT="$PWD/target/simard-state" \
SIMARD_AGENT_NAME="simard-prod" \
simard ooda run --cycles=0
```

## What happens each cycle

1. **Observe** — load goal statuses, gym health scores, memory statistics. If a bridge is unavailable, the observation records `None` for that source (honest degradation, Pillar 11).
2. **Orient** — rank goals by urgency. Blocked goals sort first, then not-started, then in-progress.
3. **Decide** — select up to `max_concurrent_actions` (default 3) actions from the ranked priority list.
4. **Act** — dispatch each action independently. A single failed action does not abort the cycle.

Action kinds: `AdvanceGoal`, `RunImprovement`, `ConsolidateMemory`, `ResearchQuery`, `RunGymEval`, `BuildSkill`.

## Verify it works

After running one cycle, check stderr output:

```text
[simard] OODA cycle 1: observed 3 goals, 2 priorities, dispatched 2 actions, 2 succeeded, 0 failed
```

Inspect state under the state root:

```bash
ls target/simard-ooda/
# cognitive_memory/  (memory bridge database)
```

## Meeting handoff → goals (Issues #157, #158)

When a meeting closes, `meeting_repl.rs` writes a handoff artifact to
`target/meeting_handoffs/meeting_handoff.json`. The OODA daemon automatically
picks up unprocessed handoffs at the start of each cycle and converts them
to goals on the board:

- **Decisions** become active goals (up to the board cap of 5).
- **Action items** with priority >= 2 become scored backlog items.
- The handoff is marked as processed so it is not re-ingested.

This means meeting outcomes flow into autonomous work without manual
intervention. See the [architecture doc](../architecture/ooda-meeting-handoff-integration.md)
for the full design.

## Goal seeding

If the goal store is empty on daemon startup (first run or reset), Simard
seeds 5 default goals that reflect her core purpose: keeping goals honest,
improving gym scores, consolidating memory, advancing open issues, and
curating the backlog. Seeding is idempotent — it only triggers on an empty
store.

## Act on meeting decisions (CLI)

To manually convert meeting decisions to GitHub issues instead of OODA goals:

```bash
simard act-on-decisions
```

This reads `target/meeting_handoffs/meeting_handoff.json`, creates GitHub issues via `gh issue create` for each decision and action item, prints open questions to stdout, and marks the handoff as processed. Individual `gh` failures are warnings — the command continues. Requires the `gh` CLI to be installed and authenticated.

See the [CLI reference](../reference/simard-cli.md) for full details.
