---
title: "How to move from terminal recipes into engineer runs"
description: Start with a discoverable bounded terminal recipe, inspect the truthful terminal audit trail, and then continue into the repo-grounded engineer loop through the same explicit local state root.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ../reference/runtime-contracts.md
  - ../tutorials/run-your-first-local-session.md
---

# How to move from terminal recipes into engineer runs

Use this guide when you want one specific operator workflow:

1. discover a shipped bounded terminal recipe
2. run it locally
3. inspect the truthful terminal audit trail
4. continue into the existing repo-grounded engineer loop with the same explicit `state-root`

This bridge is local and file-backed. It is not hidden orchestration, remote continuation, or an automatic resume system.

## Prerequisites

- [ ] You are in the repository root
- [ ] `cargo run --quiet -- ...` works locally
- [ ] You want a local file-backed bridge, not a network service or remote orchestrator

## 1. Create one explicit durable state root

The bridge only works when the terminal and engineer commands point at the same durable state root.

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-terminal-engineer.XXXXXX)"
```

Keep that shell variable for the rest of this guide.

## 2. Discover a shipped terminal recipe

List the built-in recipes and inspect one before running it.

```bash
cargo run --quiet -- engineer terminal-recipe-list
cargo run --quiet -- engineer terminal-recipe-show foundation-check
```

The important contract here is visibility:

- `terminal-recipe-list` shows what Simard ships today
- `terminal-recipe-show` prints the real bounded recipe contents
- neither command starts engineer mode or claims repo-grounded verification happened
- today that includes a minimal `foundation-check` recipe and a bounded `copilot-status-check` recipe that only probes local `amplihack copilot -- --version`
- the Copilot probe is deliberately fail-closed: missing `amplihack`, a non-zero probe exit, or a missing `GitHub Copilot CLI` version line stops the recipe instead of pretending an interactive Copilot session exists

## 3. Run the terminal recipe through the canonical CLI

```bash
cargo run --quiet -- engineer terminal-recipe single-process foundation-check "$STATE_ROOT"
```

Look for output shaped like this:

```text
Mode boundary: terminal
Terminal recipe source: foundation-check
Terminal steps count: 2
Terminal last output line: terminal-recipe-ok
Engineer next step: simard engineer run <topology> <workspace-root> <objective> <same-state-root>
```

That output matters because it tells the truth:

- this was a bounded local terminal session
- Simard persisted local terminal artifacts under `STATE_ROOT`
- the next step into engineer mode is explicit; nothing resumed automatically

## 4. Read back the persisted terminal audit trail

```bash
cargo run --quiet -- engineer terminal-read single-process "$STATE_ROOT"
```

Look for output shaped like this:

```text
Probe mode: terminal-read
Mode boundary: terminal
Terminal handoff source: latest_terminal_handoff.json
Terminal recipe source: foundation-check
Terminal last output line: terminal-recipe-ok
Terminal transcript preview:
Engineer next step: simard engineer run <topology> <workspace-root> <objective> <same-state-root>
```

This confirms the readback contract:

- `terminal-read` is read-only
- the terminal audit comes from a terminal-scoped handoff artifact
- the printed bridge guidance stays explicit and local

## 5. Continue into the repo-grounded engineer loop

Now run the real engineer loop against the repository with the same `STATE_ROOT`.

```bash
ENGINEER_OBJECTIVE=$'inspect the repository state
run one safe local engineering action
verify the outcome explicitly
persist truthful local evidence and memory'

cargo run --quiet -- engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

Look for output shaped like this:

```text
Mode boundary: engineer
Repo root: /path/to/repo
Terminal continuity available: yes
Terminal continuity source: latest_terminal_handoff.json
Terminal recipe source: foundation-check
Action plan: Inspect the repo ...
Verification status: verified
```

The contract is additive:

- the terminal summary is descriptive continuity only
- the engineer loop still inspects the repository before acting
- the engineer loop still forms its own short plan
- the engineer loop still verifies explicitly

The terminal recipe does **not** choose the engineer action, set `workspace-root`, or replace the engineer objective.

## 6. Read back the persisted engineer audit trail

```bash
cargo run --quiet -- engineer read single-process "$STATE_ROOT"
```

Look for output shaped like this:

```text
Probe mode: engineer-read
Engineer handoff source: latest_engineer_handoff.json
Mode boundary: engineer
Repo root: /path/to/repo
Terminal continuity available: yes
Terminal continuity source: latest_terminal_handoff.json
Selected action: cargo-metadata-scan
Verification status: verified
```

This proves the bridge stayed honest after execution:

- engineer readback prefers the engineer-scoped handoff
- the carried terminal summary remains a separate section
- persisted terminal continuity does not replace repo grounding, planning, or verification

## 7. Configuration rules that matter

For predictable bridge behavior, keep these rules in mind:

- pass the same explicit `state-root` argument to both the terminal command and the later engineer command
- treat `engineer terminal*` and `engineer run/read` as separate operator-visible modes even though they share the `simard engineer ...` namespace
- expect `terminal-read` to prefer `latest_terminal_handoff.json`
- expect `engineer read` to prefer `latest_engineer_handoff.json`
- expect compatibility fallback to `latest_handoff.json` only when the mode-specific file is absent
- expect a malformed mode-specific handoff to fail explicitly instead of silently falling back
- keep `workspace-root` and engineer `objective` explicit on every engineer run

## 8. Troubleshoot the common failure shapes

### `Terminal continuity available: no`

Usually one of these is true:

- the earlier terminal run used a different `STATE_ROOT`
- you ran `engineer run` before any terminal session wrote state into this root
- the terminal-scoped handoff was deleted

### `terminal-read` or `engineer read` fails on malformed handoff data

That is the intended fail-closed behavior. If `latest_terminal_handoff.json` or `latest_engineer_handoff.json` exists but is malformed, Simard does not silently downgrade to `latest_handoff.json`.

### The engineer loop ignored the terminal recipe and still inspected the repo

That is correct. The bridge is descriptive continuity, not authority transfer. Engineer mode must remain repo-grounded, planned, bounded, and explicitly verified.

## Related reading

- For the exact command tree and help text, see [Simard CLI reference](../reference/simard-cli.md).
- For the file and readback contracts, see [Runtime contracts reference](../reference/runtime-contracts.md).
- For a broader end-to-end tour of the local operator flows, see [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md).
