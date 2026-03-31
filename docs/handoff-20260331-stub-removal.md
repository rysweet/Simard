# Handoff: Simard Stub Removal and End-to-End Integration

**Date**: 2026-03-31
**Session**: Long session that implemented Phases 0-9 + quality remediation
**State**: All 10 phases merged, zero open issues, but fundamental stubs remain in legacy code

## The Core Problem

Phases 0-9 added new modules (bridge infrastructure, cognitive memory types, knowledge packs, gym scoring, agent composition, self-improvement, remote orchestration, meeting/goals, OODA loop). These modules are tested against in-memory test doubles. But the **legacy runtime** (base_types.rs, agent_program.rs, operator_commands.rs, bootstrap.rs) still uses placeholder implementations that don't do real work.

The result: `simard engineer terminal-recipe` works end-to-end through real PTY. But `simard engineer run`, `simard meeting run`, and all non-terminal paths go through `ObjectiveRelayProgram` which just echoes the objective back as a formatted string.

## Exact Stubs That Must Be Removed

### 1. ObjectiveRelayProgram (src/agent_program.rs:61)

**What it does**: `plan_turn()` takes the objective text and returns it as the `BaseTypeTurnInput` with no processing. No LLM call, no planning, no reasoning.

**What it should do**: Use a real LLM (via `amplihack copilot` or Claude API through the CopilotSdkAdapter) to analyze the objective, retrieve relevant memory, and produce a structured plan.

**Fix**: Replace `ObjectiveRelayProgram` with a program that:
- Calls `prepare_turn_context()` from `base_type_turn.rs` (already built in Phase 3)
- Formats the context with `format_turn_input()` (already built)
- Sends it to the base type adapter for real LLM processing
- Parses the response with `parse_turn_output()` (already built)

The Phase 3 code (`base_type_turn.rs`, `base_type_copilot.rs`) was designed for this but was never wired into the `AgentProgram` trait.

### 2. LocalProcessHarnessAdapter (src/base_types.rs:397-434)

**What it does**: `run_turn()` generates a formatted string like `"Local single-process harness session executed {objective_metadata} via selected base type '{id}'"`. This is fake evidence — no command was executed.

**What it should do**: Execute the turn input through a real command (the `RealLocalHarnessAdapter` from Phase 3 does this via PTY) or be removed entirely.

**Fix**: Either:
- Replace `LocalProcessHarnessAdapter::run_turn()` with a delegation to `RealLocalHarnessAdapter` (Phase 3)
- Or remove `LocalProcessHarnessAdapter` and use `RealLocalHarnessAdapter` everywhere

The `RealLocalHarnessAdapter` at `src/base_type_harness.rs` already implements `BaseTypeFactory` and `BaseTypeSession` with real PTY execution. The old adapter should be deleted.

### 3. spawn_subordinate() (src/agent_supervisor.rs:163-182)

**What it does**: Creates a `SubordinateHandle` with `pid: 0` and a comment "not yet physically spawned". It tracks the handle in memory but never forks a process.

**What it should do**: Actually call `std::process::Command::new("cargo").args(["run", "--quiet", "--", ...])` to spawn a real Simard child process with the subordinate's goal as the objective.

**Fix**:
```rust
let child = Command::new(std::env::current_exe()?)
    .args(["engineer", "run", &config.topology, &config.workspace, &config.goal])
    .env("SIMARD_AGENT_NAME", &config.agent_name)
    .env("SIMARD_SUBORDINATE_DEPTH", (config.current_depth + 1).to_string())
    .current_dir(&config.worktree_path)
    .spawn()?;
```

### 4. handover() (src/self_relaunch.rs)

**What it does**: Validates that the canary PID is non-zero and the current PID is non-zero, then returns Ok. It does not actually replace the running process.

**What it should do**: Call `std::os::unix::process::CommandExt::exec()` to replace the current process with the canary binary, or use a semaphore file + exit pattern.

### 5. Bootstrap Memory Fallback (src/bootstrap.rs:379-395)

**What it does**: Tries to launch the cognitive memory bridge. If it fails, silently falls back to `FileBackedMemoryStore` (JSON files).

**What it should do**: If the user configured Kuzu, failing to connect is an error, not a fallback. The fallback masks a broken configuration.

**Fix**: Remove the fallback. If `build_memory_store()` fails, return the error. Let the operator see that the bridge is broken and fix it, rather than silently running with degraded storage.

### 6. Meeting Mode Has No Interactive Loop

**What it does**: `simard meeting run` creates one session, runs one `run_turn()` call (which goes through `ObjectiveRelayProgram` and produces a formatted string), then exits.

**What it should do**: Enter an interactive loop where:
1. Simard presents the agenda (from meeting prep doc)
2. User types input
3. Simard processes via LLM and responds
4. Decisions are captured to `MeetingDecision` structs
5. Loop continues until user says "end meeting"
6. Meeting record persisted to Kuzu

**Fix**: Add a REPL to the meeting command path in `operator_commands_meeting.rs`. This requires reading stdin in a loop and sending each input as a new turn to the base type adapter.

### 7. OODA Loop Has No Daemon Mode

**What it does**: `run_ooda_cycle()` runs one Observe→Orient→Decide→Act cycle and returns.

**What it should do**: Run continuously in a loop, sleeping between cycles, checking for new goals, dispatching work, and monitoring subordinates.

**Fix**: Add a `simard ooda run` CLI command that:
```rust
loop {
    let report = run_ooda_cycle(&mut state, &bridges, &config)?;
    eprintln!("[simard-ooda] cycle {} complete: {} actions dispatched", report.cycle, report.actions.len());
    std::thread::sleep(config.cycle_interval);
}
```

### 8. RustyClawdAdapter (src/base_types.rs)

**What it does**: Same as `LocalProcessHarnessAdapter` — generates formatted strings without executing anything.

**What it should do**: Either integrate with the real RustyClawd binary or be removed if RustyClawd isn't available.

## Files That Need Changes

| File | LOC | What | Priority |
|------|-----|------|----------|
| `src/agent_program.rs` | 830 | Replace ObjectiveRelayProgram with real LLM integration | HIGH |
| `src/base_types.rs` | 715 | Remove LocalProcessHarnessAdapter/RustyClawdAdapter stubs, use Phase 3 adapters | HIGH |
| `src/bootstrap.rs` | 833 | Remove silent fallback, wire Phase 3 adapters into registry | HIGH |
| `src/agent_supervisor.rs` | 389 | Make spawn_subordinate() fork a real process | HIGH |
| `src/self_relaunch.rs` | 334 | Make handover() actually exec the new binary | MEDIUM |
| `src/operator_commands_meeting.rs` | 336 | Add interactive REPL for meeting mode | MEDIUM |
| `src/operator_commands.rs` | 1180 | Wire `simard ooda run` command | MEDIUM |
| `src/ooda_loop.rs` | 354 | Add daemon loop mode | MEDIUM |

## What Already Works and Must Not Break

- `simard engineer terminal-recipe single-process foundation-check /tmp/state` — real PTY execution
- `simard engineer copilot-submit single-process /tmp/state` — real Copilot launch
- `simard gym list` — real scenario listing
- `simard engineer terminal-read single-process /tmp/state` — real state readback
- Python memory bridge → Kuzu storage (4 live tests)
- Python knowledge bridge → pack listing
- Python gym bridge → scenario listing
- 575 unit/integration tests
- 7 qa-team outside-in scenarios

## How to Execute This Work

1. **Read this document and Specs/ProductArchitecture.md**
2. **Follow the DEFAULT_WORKFLOW** — all 22 steps for every change
3. **Use the quality-audit skill** after each major change
4. **Use merge-ready** before merging any PR
5. **Test end-to-end** — run every `simard` command and verify it does real work
6. **Zero stubs when done** — every function either does real work or is deleted

## Architecture Decision: How to Wire the LLM

The Phase 3 code already built the execution path:
- `base_type_copilot.rs`: `CopilotSdkAdapter` launches `amplihack copilot` via PTY
- `base_type_turn.rs`: `prepare_turn_context()` enriches with memory + knowledge
- `base_type_turn.rs`: `format_turn_input()` produces the prompt
- `base_type_turn.rs`: `parse_turn_output()` extracts actions from LLM response

The missing wire: `AgentProgram::plan_turn()` needs to call `prepare_turn_context()` + `format_turn_input()`, and the base type session's `run_turn()` needs to use `CopilotSdkAdapter` (or `RealLocalHarnessAdapter`) instead of the stub adapters.

The registration in `bootstrap.rs` currently registers `LocalProcessHarnessAdapter` for "local-harness". Change it to register `RealLocalHarnessAdapter` or `CopilotSdkAdapter`.

## Environment Requirements

- `amplihack copilot` v1.0.14 available on PATH
- `amplihack claude` v2.1.88 available on PATH
- Python 3.12 with `kuzu` package (for memory bridge)
- amplihack-memory-lib at `/home/azureuser/src/amplirusty/amplihack-memory-lib/src`
- agent-kgpacks at `/home/azureuser/src/agent-kgpacks`
- amplihack at `/home/azureuser/src/amplihack/src`
- Symlink: `/home/azureuser/.amplihack/src` → `/home/azureuser/src/amplihack/src`
- amplihack proxy_manager fix applied locally (amplihack#3953)

## Recipe Runner Notes

The recipe runner works after two local fixes:
1. `proxy_manager = None` added to `ClaudeLauncher.__init__` in amplihack/launcher/core.py
2. Symlink created: `~/.amplihack/src` → amplihack source tree

To run the default-workflow recipe:
```bash
cd /home/azureuser/src/Simard/worktrees/simard-live
env -u CLAUDECODE \
  AMPLIHACK_HOME=/home/azureuser/.amplihack \
  PYTHONPATH="/home/azureuser/src/amplihack/src" \
  AMPLIHACK_MAX_DEPTH=0 \
  AMPLIHACK_NONINTERACTIVE=1 \
  python3 -c "
from amplihack.recipes import run_recipe_by_name
result = run_recipe_by_name('default-workflow', user_context={
    'task_description': 'YOUR TASK HERE',
    'repo_path': '/home/azureuser/src/Simard/worktrees/simard-live',
    'expected_gh_account': 'rysweet',
}, progress=True)
"
```

## Session Stats

This session produced:
- 22 PRs merged
- 46+ new Rust modules
- ~38,000 LOC total codebase
- 575 tests passing
- 7 qa-team outside-in scenarios
- 1 quality-audit-cycle run (SUCCESS)
- Zero open GitHub issues
- amplihack#3953 filed for proxy_manager bug
