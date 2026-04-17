# Simard vs amplihack — Feature-by-Feature Comparison

Simard is the **Rust-native successor** to [amplihack](https://github.com/rysweet/amplihack). The trajectory is replacement. This page is the honest ledger of what has reached parity, what has not, and what each remaining gap costs.

**Last updated:** April 2026.

## Summary

| Area | amplihack | Simard today | Parity? |
|---|---|---|---|
| [Memory](#memory) | `amplihack-memory-lib` (Python + Rust) | `cognitive_memory` (Rust, via `amplihack-memory` crate) | ✅ Rust surface — same crate; Python surface not ported |
| [Workflow execution](#workflow-execution) | `amplihack-recipe-runner` + `smart-orchestrator` YAML recipes | OODA daemon + engineer loop (Rust) | ⚠ Functional coverage, no YAML DSL |
| [Agent catalog](#agents) | Dozens of markdown agents under `amplifier-bundle/agents/` | Base types + identity manifests (Rust) | ❌ Gap — no markdown catalog |
| [Skills](#skills) | ~150+ bundled skills under `amplifier-bundle/skills/` | None native | ❌ Gap — full port needed |
| [Evaluation](#evaluation) | `amplihack-agent-eval` (Python) | Python bridge into `amplihack.eval.*` | ❌ Gap — bridge is the dependency |
| [`copilot-sdk` adapter](#copilot-sdk) | amplihack copilot CLI | PTY shell-out to `amplihack copilot` | ❌ Gap — runtime dep |
| [XPIA defense](#xpia) | `amplihack-xpia-defender` | Not shipped in Simard | ❌ Gap — not yet on roadmap |
| [Claude hooks / slash commands](#claude-hooks) | Full Claude Code integration | Out of scope | — Non-goal |

## Memory

**amplihack:** `amplihack-memory-lib` exposes a 6-type cognitive memory (sensory, working, episodic, semantic, procedural, prospective) in both Python and Rust.

**Simard:** the `cognitive_memory` module uses the **`amplihack-memory` Rust crate** directly as a git dependency in `Cargo.toml`. This is the same crate that backs amplihack's Rust-side memory. The name is legacy — despite the `amplihack-` prefix, no Python runtime is required for the core memory operations.

**Gap:** only the Rust surface of `amplihack-memory-lib` is covered. The Python bindings and Python-side adapters in `amplihack-memory-lib` are not mirrored and are not used by Simard. For Simard's own workloads this is complete; for operators migrating a pure-Python pipeline, the Python surface is still unported.

**Migration path:** already migrated. See [architecture/cognitive-memory.md](architecture/cognitive-memory.md).

## Workflow execution

**amplihack:** `amplihack-recipe-runner` executes YAML recipes from `amplifier-bundle/recipes/`. `smart-orchestrator` is the top-level recipe that classifies tasks and routes to `default-workflow`, `investigation-workflow`, or `consensus-workflow`. Recipes can invoke nested recipes, base-type adapters, and bash steps.

**Simard:** the **OODA daemon** plus the **engineer loop** cover the same functional ground in Rust. There is no YAML recipe DSL — orchestration is encoded in Rust code and in the OODA loop's observation → decision → action pipeline.

**Gap:** no declarative YAML recipe DSL. For operators who want to define new workflows declaratively without writing Rust, this is a real limitation.

**Migration path:** tracked as **parity issue: YAML recipe DSL for Simard**. Design sketch in [recipes.md](recipes.md).

## Agents

**amplihack:** dozens of markdown agents under `amplifier-bundle/agents/` organized into `core/`, `specialized/`, `workflows/` (verify with `find amplifier-bundle/agents -name '*.md' | wc -l`). Each agent is a markdown file with system-prompt-shaped content and metadata.

**Simard:** agent behavior is expressed through **base types** (the runtime adapter layer — `claude-agent-sdk`, `rusty-clawd`, `copilot-sdk`, `ms-agent-framework`) plus **identity manifests** (Rust structures + markdown prompts under `prompt_assets/simard/`). No markdown-catalog agent system.

**Gap:** a user cannot drop a markdown file into `agents/` and have a new agent appear. Agent authoring requires editing Rust.

**Migration path:** tracked as **parity issue: markdown agent catalog**. A Rust loader that reads markdown agent definitions and registers them into the identity manifest system. Expected relatively low complexity (~hundreds of lines).

## Skills

**amplihack:** a large bundled skill catalog under `amplifier-bundle/skills/` (150+ skills as of April 2026; verify with `ls amplifier-bundle/skills | wc -l`). A skill is a named capability with activation conditions, a prompt fragment, and optional wiring to recipes / MCP servers.

**Simard:** zero bundled skills.

**Gap:** the largest parity gap. Skills are how amplihack composes agent capabilities, and Simard has no native story for them yet.

**Migration path:** tracked as **parity issue: skill catalog migration + incremental port plan**. See [skills.md](skills.md) for priority ordering.

## Evaluation

**amplihack:** `amplihack-agent-eval` provides progressive test suites, long-horizon memory benchmarks, and scoring utilities. All Python.

**Simard:** `python/simard_gym_bridge.py` imports `amplihack.eval.progressive_test_suite` and `amplihack.eval.long_horizon_memory` and invokes them from Rust via subprocess. The `simard gym` CLI is native Rust; the evaluators it calls are not.

**Gap:** Simard cannot run gym without amplihack installed. The Python bridge is a hard runtime dependency.

**Migration path:** tracked as **parity issue: native Rust gym/eval**. Port the evaluator types and progressive test suites to Rust. The scenario-discovery surface is already Rust; the evaluator bodies are not.

## `copilot-sdk`

**amplihack:** `amplihack copilot` is amplihack's Copilot CLI integration — it launches a Copilot session with amplihack's context, hooks, and skills.

**Simard:** the `copilot-sdk` base type in `src/base_type_copilot.rs`, `src/copilot_status_probe.rs`, `src/copilot_task_submit/`, and `src/terminal_session/workflow_guard.rs` spawns `amplihack copilot` as a PTY subprocess and drives the terminal.

**Gap:** every `copilot-sdk` session is an amplihack invocation. Without amplihack installed, this base type does not work.

**Migration path:** tracked as **parity issue: native copilot-sdk**. The replacement should talk directly to the Copilot API or CLI without going through amplihack. Once this ships, `cmd_ensure_deps.rs` can stop auto-installing amplihack.

## XPIA

**amplihack:** `amplihack-xpia-defender` provides cross-prompt-injection-attack detection and filtering for agent pipelines.

**Simard:** not shipped. There is no XPIA layer in Simard's Rust core yet.

**Gap:** security hardening missing.

**Migration path:** not currently on the near-term roadmap. File an issue if needed.

## Claude hooks

**amplihack:** `.claude/` directory with `hooks/`, `commands/`, `agents/`, `context/`, `workflows/`, `docs/`. Claude Code–specific extension surface (slash commands, PreToolUse / PostToolUse hooks, Task tool wiring).

**Simard:** out of scope. Simard targets terminal-first, binary-first distribution and does not attempt to replace Claude Code's extension model.

**Gap:** not a parity target. Operators who need Claude Code slash commands should continue to use amplihack for that layer.

## The auto-install shim (`cmd_ensure_deps.rs`)

Simard's `cmd_ensure_deps.rs` command auto-installs amplihack (via `cargo install amplihack`) and clones the amplihack repository to populate `~/.amplihack/src`. This exists because the `copilot-sdk` base type and the gym bridge need amplihack on disk.

**This is a contradiction with Simard's successor framing**, and it is acknowledged rather than hidden. The shim stays until the two runtime dependencies above (`copilot-sdk`, gym eval) have native replacements. That removal is a tracked parity issue.

## End-state summary

**Replaced (native Rust):**

- Cognitive memory.
- OODA loop orchestration (functional parity with smart-orchestrator, no YAML DSL).
- Meeting facilitator (new capability, no amplihack analog).
- Goal / improvement curation.
- Review pipeline.
- Dashboard.
- Base-type adapter layer (for three of four external runtimes).

**Still depending on amplihack at runtime:**

- `copilot-sdk` base type.
- Gym evaluation.
- `cmd_ensure_deps.rs` auto-install (consequence of the above).

**Not attempted:**

- XPIA defense.
- Claude Code hooks / slash commands / Task tool wiring.

**Recommended migration command for operators:**

```bash
npx github:rysweet/Simard install
```

Keep amplihack installed alongside until your workflow no longer hits any of the runtime-dependency items above.

## Tracked parity issues

All gaps above **will be** filed as GitHub issues labeled `parity` as part of the PR that lands these docs. The `parity` label and the six tracking issues (YAML recipe DSL, markdown agent catalog, skill catalog, native gym eval, native `copilot-sdk`, retire `cmd_ensure_deps`) are not yet filed at the time of writing — this note will be removed once they exist. See the label on [rysweet/Simard](https://github.com/rysweet/Simard/issues?q=label%3Aparity).
