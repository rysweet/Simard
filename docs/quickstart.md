# Quickstart

Five minutes from install to a productive session.

## 1. Install

See [installation.md](installation.md). For impatient readers:

```bash
npx github:rysweet/Simard install
```

## 2. Run your first engineer session

```bash
simard engineer run single-process /path/to/repo "improve test coverage in module X"
```

Simard will inspect the repo, select a subtask, execute it, verify the result, and record the outcome. The transcript is printed to stdout and persisted to the session directory.

See [tutorials/run-your-first-local-session.md](tutorials/run-your-first-local-session.md) for a walkthrough.

## 3. Hold a meeting

```bash
simard meeting repl "weekly sync"
```

You get an interactive REPL that persists decisions, action items, and handoff files you can feed into later engineer runs.

See [howto/carry-meeting-decisions-into-engineer-sessions.md](howto/carry-meeting-decisions-into-engineer-sessions.md).

## 4. Benchmark Simard (gym)

```bash
simard gym list
simard gym run repo-exploration-local
```

**Note:** Today gym still calls through `python/simard_gym_bridge.py` into `amplihack.eval.*`. Until the native Rust gym parity issue ships, you need `amplihack` installed. See [amplihack-comparison.md](amplihack-comparison.md#evaluation).

## 5. Turn on the dashboard

```bash
simard dashboard serve
# Open http://localhost:8080
```

Live view of issues, metrics, costs, processes, and logs.

## What next?

- [Philosophy](philosophy.md) — how Simard reasons about simplicity, evidence, and honesty.
- [Workflows](workflows.md) — OODA daemon, engineer loop, self-improve cycle.
- [CLI reference](reference/simard-cli.md) — every command, every flag.
- [Comparison with amplihack](amplihack-comparison.md) — what replaces what.
