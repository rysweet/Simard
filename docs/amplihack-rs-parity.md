# amplihack-rs ↔ amplihack (Python) Parity Matrix

**Status**: initial inventory — 2026-05-16.
**Scope**: tracks which subsystems of the original Python `amplihack` framework
have a working Rust counterpart in `amplihack-rs`, and which are still missing,
partial, or only available in the Python implementation. Compiled by walking
both source trees and the `amplihack-rs` workspace `Cargo.toml`.

This document exists in the Simard repo because Simard composes amplihack as
one of its agent substrates and needs a queryable, version-pinned record of
when it can safely move its hot path off the Python amplihack reference and
onto the Rust port. It is **not** prescriptive of amplihack-rs itself — gaps
identified here are filed as `parity` issues in `rysweet/Simard` and
triaged into upstream amplihack-rs work as engineering capacity allows.

## References

- **amplihack (Python reference)**: `/home/azureuser/src/amplihack`
  (`src/amplihack/…`, `amplifier-bundle/…`) — 311 Python modules.
- **amplihack-rs (Rust port)**: `/home/azureuser/src/amplihack-rs`
  (26 member crates under `crates/`, 3 binaries under `bins/`) — `v0.9.3`.
- **Simard adapters that depend on amplihack** live under
  `src/operator_commands_engineer/`, `src/ooda_actions/`,
  `src/stewardship/`, `src/worktree_gc/`, and the `simard-engineer-*`
  binaries declared in the root `Cargo.toml`.

## Legend

- **Both** — first-class implementation exists in both `amplihack-rs` and the
  Python reference. Behavioral drift, if any, is called out in the notes.
- **Rust-only** — implemented in `amplihack-rs`, no longer present (or never
  present) in Python.
- **Python-only** — exists in Python `amplihack`, no `amplihack-rs` crate
  covers it. Simard cannot rely on the Rust port for this surface.
- **Simard-only** — concept lives entirely in the Simard runtime (e.g.
  stewardship, worktree GC, OODA brain). Included here because the task
  description groups them under "amplihack-rs parity"; in practice these are
  consumer-side concerns and never had a Python amplihack analog.

## Parity matrix

| Subsystem | Status | amplihack-rs location | Python amplihack location | Behavioral drift / notes |
|---|---|---|---|---|
| Recipe runner (`amplihack recipe run`) | Both | `crates/amplihack-cli/src/commands/recipe/run/` (`execute.rs`, `binary.rs`, `format.rs`) | `src/amplihack/recipe_cli/`, `src/amplihack/recipes/`, `amplifier-bundle/recipes/` | Both can execute `smart-orchestrator.yaml`; Rust path is the documented default in the dev-orchestrator skill. Rust hardened `NONINTERACTIVE` env injection and `working_directory` propagation (amplihack-rs CHANGELOG #277, #251). |
| Recipe parse / resolve / validate (`amplihack recipe show / validate`) | Both | `crates/amplihack-cli/src/commands/recipe/{parse,resolve,show_validate}.rs` | `src/amplihack/recipe_cli/`, `src/amplihack/recipes/` | Equivalent. Rust path is faster and avoids the Python interpreter on cold start. |
| Agentic / OODA loop (in-agent reasoning) | Both | `crates/amplihack-agent-core/src/agentic_loop/` (`loop_core.rs`, `reasoning.rs`, `reasoning_eval.rs`) | `src/amplihack/agent/` (minimal — most reasoning is recipe-driven via prompts) | Not the same shape as Simard's OODA loop. amplihack-rs models a generic agentic loop; the Python `amplihack/agent/` package is a thin stub. |
| Simard OODA loop / brain (`simard ooda-step`, brain construction) | Simard-only | Not applicable | Not applicable | Lives in `Simard/src/ooda_actions/` and `simard-ooda-step` bin. Listed in the task description but **never** had an amplihack analog. Tracked separately in Simard issues (e.g. #1711, #1748, #1754, #1876, #1884). |
| Hooks: `PostToolUse` workflow enforcement | Both | `crates/amplihack-hooks/src/post_tool_use/{workflow,validation,launcher}.rs` + `bins/amplihack-hooks` | `src/amplihack/hooks/strategies/`, `src/amplihack/hooks/manager.py` | Rust port is the active hook surface (used by Copilot CLI). Workflow-state directory is `/tmp/amplihack-workflow-state/` per the dev-orchestrator skill. Behavioral drift: Rust adds `workflow_classification.rs` for stricter intent gating; Python relied on free-form regexes. |
| Hooks: `PreCompact` export-on-compact | Both | `crates/amplihack-hooks/src/pre_compact/{mod,export,request}.rs`, `crates/amplihack-builders/src/export_on_compact.rs` | `src/amplihack/hooks/`, builder transcripts in `amplifier-bundle/tools/` | Both produce a session export on compaction; Rust integrates with `amplihack-builders` for Claude/Codex transcript formats. |
| Agent binary adapters — Claude | Both | `crates/amplihack-launcher/src/claude_binary_manager.rs`, `crates/amplihack-builders/src/claude/` | `src/amplihack/launcher/core.py`, `src/amplihack/launcher/copilot.py` (shared bits) | Equivalent. Rust passes through `--dangerously-skip-permissions` matching Python launcher parity (commented in `commands/mod.rs`). |
| Agent binary adapters — GitHub Copilot CLI | Both | `crates/amplihack-launcher/src/{copilot_launcher,copilot_mcp,copilot_staging,copilot_auto_install}.rs`, `crates/amplihack-cli/src/copilot_setup/` | `src/amplihack/launcher/copilot.py`, `src/amplihack/copilot_auto_install.py`, `src/amplihack/hooks/copilot_stop_handler.py` | Both adapters cover install + launch + MCP wiring. Rust defaults to Copilot `--remote` injection (Unreleased CHANGELOG entry). |
| Agent binary adapters — Codex | Both | `crates/amplihack-launcher/src/codex.rs`, `crates/amplihack-builders/src/codex/` | (none — Codex support was added during the Rust port) | **Rust-leaning**. Python `amplihack` has no first-class Codex adapter; the Rust crate is the canonical implementation. |
| Agent binary adapters — Amplifier / Microsoft Agent Framework | Both | `crates/amplihack-launcher/src/amplifier.rs` | `src/amplihack/launcher/amplifier.py` | Equivalent surface, both wrap `amplifier-bundle/`. |
| Subprocess prompt delivery (temp-file vs shell-escape, refs #1871/#1879) | Python-only | Not implemented in amplihack-rs adapters (`amplihack-orchestration/src/claude_process_builder.rs` and `amplihack-launcher/src/claude_binary_manager.rs` pass prompts on the command line; no `tempfile` / `NamedTempFile` / `stdin` route was found) | Python adapters use shell-escaping; same vulnerability as Simard pre-#1871 | Direct gap that affects Simard adapters too. amplihack-rs needs a temp-file or stdin prompt-delivery mode before Simard can swap its Copilot/Claude probes to call amplihack-rs subprocess paths instead of building its own. See follow-up issue. |
| Stewardship / merge-judge (PR adjudication) | Simard-only | Not applicable | Not applicable | Lives in `Simard/src/stewardship/{merge_judge,routing,merge_authority,gh_client}.rs`. Listed in the task because it is grouped under "amplihack-rs parity" by the requester, but it was never an amplihack feature; Simard owns the entire pipeline (e.g. PR #1895 fix). |
| Worktree GC (background pruning of stale per-agent worktrees) | Simard-only | Not applicable | `src/amplihack/worktree/git_utils.py` (helper only; no GC daemon) | Simard's `worktree_gc::runner` plus `operator_cli/worktree_gc.rs` is the actual GC. Python amplihack has worktree helpers but no GC. Recent fix: proof-of-life check before pruning (#1886/#1889). |
| Engineer worktree lifecycle (`simard-engineer-step`, `simard-engineer-loop-recipe`) | Simard-only | Not applicable | Not applicable | Lives in `Simard/src/engineer_worktree/`. `amplihack engineer` is **not** a documented subcommand in either amplihack-rs or Python amplihack. |
| CLI surface — `install` / `uninstall` / `update` / `doctor` / `launch` | Both | `crates/amplihack-cli/src/commands/install/`, `commands/doctor.rs`, `commands/launch.rs`, `cli.rs::update` | `src/amplihack/cli.py`, `src/amplihack/install.py`, `src/amplihack/uninstall.py`, `src/amplihack/auto_update.py`, `src/amplihack/health_check.py` | Equivalent. Rust adds `self_heal` and asset-version reconciliation that Python's auto_update did not handle. |
| CLI surface — `memory` subtree (`get`/`store`/`list`/`delete`/`tree`/`index-scip`/`transfer`/`clean`) | Both | `crates/amplihack-cli/src/commands/memory/` (10+ submodules) | `src/amplihack/memory/`, `src/amplihack/knowledge_builder/` | Rust port is more featureful (native SCIP indexing, indexing-job state, staleness detector). |
| CLI surface — `fleet` (Azure VM remote fleet) | Both | `crates/amplihack-fleet/`, `crates/amplihack-remote/`, `crates/amplihack-cli/src/commands/fleet/` | `src/amplihack/fleet/`, `src/amplihack/remote/` | Both ship; Rust adds `fleet_local` for the local-session TUI (separate from Azure orchestration). |
| CLI surface — `plugin` install / verify / uninstall | Both | `crates/amplihack-cli/src/commands/plugin/`, `crates/amplihack-cli/src/claude_plugin.rs` | `src/amplihack/plugin_cli.py`, `src/amplihack/plugin_manager.py` | Equivalent. |
| CLI surface — `reflect`, `multitask`, `pr`, `orch`, `session-tree`, `mode`, `builder`, `new-agent`, `append`, `lock`, `mcp-eval`, `cs-validate`, `query-code`, `hive-haymaker`, `uvx-help`, `completions`, `rustyclawd` | Rust-only (mostly) | `crates/amplihack-cli/src/commands/*` | Partial: `mode_detector.py`, `meta_delegation/`, `hive.py`, `hive_haymaker.py`, `uvx/` exist in Python but are not exposed as top-level subcommands. | Rust port has standardized these as `amplihack <verb>`. Python required separate entry points or in-skill invocation. |
| CLI surface — `engineer` subcommand | Neither | Not implemented | Not implemented | Listed in the task description but never shipped in either tree. The Simard `simard-engineer-*` bins are the only "engineer" surface today; an `amplihack engineer` would need to be a new shared abstraction. |
| Knowledge builder / code-graph indexing | Both | `crates/amplihack-blarify/`, `crates/amplihack-multilspy/`, `crates/amplihack-memory/` (code-graph types), `crates/amplihack-cli/src/commands/memory/scip_indexing/` | `src/amplihack/knowledge_builder/`, `src/amplihack/lsp_detector/` | Rust port replaces external `blarify` and `multilspy` Python wrappers with native crates. |
| XPIA security / prompt-injection armor | Both | `crates/amplihack-security/` | `src/amplihack/security/` | Equivalent — both implement the XPIA policy gate. |
| Safety: auto-mode git conflict guard | Both | `crates/amplihack-safety/` | `src/amplihack/safety/`, `src/amplihack/staging_safety.py`, `src/amplihack/staging_cleanup.py` | Equivalent. |
| Reflection / session learning | Both | `crates/amplihack-reflection/` | `src/amplihack/agent/` (reflection helpers scattered) | Rust consolidates state machine + semantic dedup + sanitization into one crate; Python has the same logic but split across helpers. |
| Recovery pipeline (4-stage test/build recovery) | Both | `crates/amplihack-recovery/` | `src/amplihack/recovery/` | Equivalent. |
| Domain agents (teaching / security / synthesis / learning) | Both | `crates/amplihack-domain-agents/` | `src/amplihack/agents/`, `amplifier-bundle/agents/` | Equivalent. |
| Hive mind orchestration (4-layer) | Both | `crates/amplihack-hive/` | `src/amplihack/cli/hive.py`, `src/amplihack/cli/hive_haymaker.py` | Python is thinner; the Rust crate is the production hive runtime. |
| Agent generator (goal → agent pipeline) | Both | `crates/amplihack-agent-generator/` | `src/amplihack/goal_agent_generator/`, `src/amplihack/bundle_generator/` | Equivalent surface; Rust uses a stronger type schema for skill synthesis. |
| Progressive eval framework (L1–L12) | Both | `crates/amplihack-agent-eval/` | `src/amplihack/eval/` | Equivalent. |
| Observability / structured tracing | Both (Rust-leaning) | `tracing`/`tracing-subscriber` baked into every crate (e.g. `amplihack-cli/src/main.rs`) | `src/amplihack/observability.py`, `src/amplihack/tracing/` | Rust gets free per-span structured logs via `tracing`; Python had to roll its own. |
| Bundle / runtime asset resolution | Both | `bins/amplihack-asset-resolver/`, `crates/amplihack-cli/src/{resolve_bundle_asset,runtime_assets}.rs` | `src/amplihack/resolve_bundle_asset.py`, `src/amplihack/runtime_assets.py`, `src/amplihack/path_resolver/` | Equivalent. Rust path resolves the same `amplifier-bundle/` layout. |
| Auto-update / self-heal | Both | `crates/amplihack-cli/src/{auto_update,self_heal,update}.rs` | `src/amplihack/auto_update.py`, `src/amplihack/copilot_auto_install.py` | Rust adds binary-asset version pinning via `AMPLIHACK_RELEASE_VERSION` (closes self-update prompt loop). |
| Proxy / LLM brokering | Python-only | (no crate) | `src/amplihack/proxy/`, `src/amplihack/llm/` | The Python proxy / LLM-broker surface (used for local model routing during eval) has **no Rust counterpart**. amplihack-rs assumes the host agent binary owns LLM dispatch. Filed as a follow-up issue. |
| Power-steering harness (token + step budgets) | Both | `crates/amplihack-utils/` (steering helpers), references in `amplihack-cli` | `src/amplihack/power_steering/` | Both present; Rust split power-steering helpers across `utils` and `orchestration` rather than a dedicated crate. Worth re-evaluating: a dedicated `amplihack-power-steering` crate would mirror Python more cleanly. |
| Mode detector (auto / interactive / non-interactive routing) | Both | `crates/amplihack-context/`, `crates/amplihack-cli/src/commands/mode/` | `src/amplihack/mode_detector/`, `src/amplihack/context/` | Equivalent. |
| Workflows compiler (`gh aw` → recipes) | Both | `crates/amplihack-workflows/src/gh_aw_compiler.rs` | `src/amplihack/workflows/gh_aw_compiler.py` | Equivalent. |
| Test harness / scenarios (qa-team Gherkin) | Python-only | (none — qa-team runs through Python and recipes) | `amplifier-bundle/agents/qa-team/`, `src/amplihack/testing/` | Listed for completeness; this is invoked via recipes from both runtimes but the harness lives in `amplifier-bundle/` (asset bundle, not Rust). |

## Top gaps (filed as `amplihack-rs` + `parity` follow-up issues)

The following missing-or-partial subsystems are the highest-leverage gaps for
Simard's consumption of `amplihack-rs`. Each has a dedicated GitHub issue
filed against `rysweet/Simard` with reproduction context, the Python reference
path, the missing Rust equivalent, and acceptance criteria framed as
"Rust implementation passes the same behavioral test as the Python reference".

- [ ] **Subprocess prompt delivery: temp-file / stdin mode** — `amplihack-rs`
      `claude_process_builder` and `launcher::*_launcher` paths pass prompts
      on the command line and inherit the same shell-escaping fragility that
      Simard issues #1871 and #1879 patched on the Simard side.
      *Filed below.*
- [ ] **Proxy / LLM brokering crate** — Python `src/amplihack/proxy/` and
      `src/amplihack/llm/` have no Rust counterpart; Simard cannot run
      eval-mode workloads through `amplihack-rs` without re-wiring its own
      LLM router.
      *Filed below.*
- [ ] **`amplihack engineer` subcommand (shared engineer-loop CLI surface)**
      — Neither amplihack-rs nor Python amplihack exposes an `engineer`
      verb. Simard's `simard-engineer-*` bins are the only implementation
      and embed Simard-specific OODA assumptions. Promoting a generic
      engineer loop into amplihack-rs would let Simard drop its bespoke
      bins.
      *Filed below.*
- [ ] **Dedicated `amplihack-power-steering` crate** — Power-steering
      helpers are scattered across `amplihack-utils` and `amplihack-cli`
      rather than mirroring Python's `power_steering/` module. Consolidate
      so consumers (Simard, the recipe runner, eval) can depend on a single
      crate.
      *Filed below.*
- [ ] **Behavioral parity tests for `PostToolUse` workflow enforcement
      across Python vs Rust hook strategies** — Both strategies exist but
      we lack an executable contract test asserting the Rust and Python
      strategies emit equivalent decisions for the same `tool_call`
      payload. Without it, drift can creep in unnoticed.
      *Filed below.*

Each checkbox above will be linked to its GitHub issue once filed (see
the PR description and the issue cross-references appended below).

## Out of scope for this PR

- No implementation work. This PR is inventory + scaffolded issue tracking
  only, as required by the parity-driving task.
- Subsystems listed as **Simard-only** are noted for completeness but do
  not generate follow-up `amplihack-rs` issues — they belong to Simard's
  own roadmap.
- Behavioral drift between Python and Rust hook strategies is captured at
  the matrix-row level; the executable contract test is filed as one of
  the top-5 gaps above rather than enumerated here.
