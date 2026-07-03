---
title: Simard documentation
description: Start here for the shipped `simard` operator CLI, the shared-state-root bridge from bounded terminal sessions into the repo-grounded engineer loop, the `engineer read` audit companion, compatibility binaries, runtime contracts, and benchmark flow.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
---

# Simard documentation

Simard is a terminal-native engineering identity, written in Rust, that drives and curates agentic coding systems. She composes work over a pluggable set of agent base types — including `local-harness`, `terminal-shell`, `rusty-clawd`, and `copilot-sdk` in v1, with Microsoft Agent Framework, Claude Code SDK, and amplihack / amplihack-rs as candidate substrates — and exposes five operating modes (engineer, meeting, goal-curation, improvement-curation, gym). For the full design contract, see [Specs/ProductArchitecture.md](https://github.com/rysweet/Simard/blob/main/Specs/ProductArchitecture.md).

`simard` is the canonical operator-facing CLI.

The shipped command tree covers `engineer`, `meeting`, `goal-curation`, `improvement-curation`, `gym`, `review`, and `bootstrap` from one binary, including the read-only `engineer read` audit companion and the bounded `engineer terminal*` session surfaces. The legacy `simard_operator_probe` and `simard-gym` binaries remain available as compatibility surfaces while operators migrate, but the primary product surface is now `simard ...`.

Terminal sessions and repo-grounded engineer runs now bridge through one explicit local `state-root`. That bridge is file-backed and operator-visible. It does not imply hidden resume logic, external orchestration, or automatic continuation.

!!! note "Recently reconciled"
    The docs were audited against the current source: the cognitive-memory
    pages were de-forked to the library backend (issue #2307), and every page
    is now reachable from the navigation. See
    [What's Changed](./whats-changed.md) for the full change index.

## Start here

- [Tutorial: Run your first local session](./tutorials/run-your-first-local-session.md) - Exercise the local runtime through the primary CLI.
- [How to move from terminal recipes into engineer runs](./howto/move-from-terminal-recipes-into-engineer-runs.md) - Start with a discoverable terminal recipe, then continue into the repo-grounded engineer loop through the same explicit state root.
- [Tutorial: Run your first benchmark gym suite](./tutorials/run-your-first-benchmark-gym.md) - Run the shipped starter benchmark suite.
- [How to configure bootstrap and inspect reflection](./howto/configure-bootstrap-and-inspect-reflection.md) - Bootstrap an explicit runtime selection and inspect the truthful runtime snapshot.
- [How to reclaim disk space and run low-space Rust builds](./howto/reclaim-disk-space-and-run-low-space-rust-builds.md) - Reclaim stale build artifacts and run Cargo through one shared low-space target dir across worktrees.
- [How to configure and monitor the disk health check](./howto/configure-disk-health-check.md) - Tune the per-cycle automated disk cleanup that prevents ENOSPC crashes (#2020).
- [How to diagnose and prevent handoff accumulation](./howto/diagnose-handoff-accumulation.md) - Detect, resolve, and prevent unbounded meeting handoff file growth (#2268).
- [How to start a meeting with Simard](./howto/start-a-meeting.md) - Have a natural conversation with Simard from CLI or dashboard, with full history and memory.
- [How to carry meeting decisions into engineer sessions](./howto/carry-meeting-decisions-into-engineer-sessions.md) - Persist meeting records under a shared state root and confirm later engineer runs carry them forward.
- [How to inspect meeting records](./howto/inspect-meeting-records.md) - Read back the latest durable meeting record without mutating stored state.
- [How to inspect improvement-curation state](./howto/inspect-improvement-curation-state.md) - Read back the latest approved, deferred, and promoted improvement state without mutation.
- [How to inspect the durable goal register](./howto/inspect-durable-goal-register.md) - Read back the active top-5 goals and backlog without mutation.
- [How to recover a corrupted or missing goal board](./howto/recover-goal-board.md) — cognitive-memory-only recovery commands.
- [How to troubleshoot the file-backed goal store](./howto/troubleshoot-goal-store.md) — operator playbook for goal_store.json issues.
- [How to decompose a large goal into linked sub-goals](./howto/decompose-a-large-goal.md) — break one umbrella goal into 2–6 bounded sub-goals with `simard goal decompose`, verify the parent↔child edges round-trip in the graph, and read parent progress as a roll-up (#2405).
- [How to configure adaptive scaling](./howto/configure-adaptive-scaling.md) — enable and tune AIMD concurrency scaling.
- [How to keep Simard's own dependency pins up to date](./howto/self-maintain-dependency-pins.md) — the reactive done-gate and proactive reconcile that bump Simard's own `Cargo.toml` git-rev pins after she lands a change upstream, so the fix runs in her own build (companion to [Safe Self-Update](./safe-self-update.md)).
- [Concept: reconcile-and-self-deploy](./concepts/reconcile-and-self-deploy.md) — how a merged self-change is built-from-source, deployed, health-verified, and left running (or rolled back), closing the "merged != running" gap. See the [self-deploy API reference](./reference/self-deploy-api.md) and the [verify-and-roll-back runbook](./howto/verify-and-roll-back-a-self-deploy.md).
- [Concept: deploy-aware done-gate](./concepts/deploy-aware-done-gate.md) — why a goal is complete only with a merged PR, a closed issue, and (for self-affecting changes) a verified deploy; the gate that prevents evidence-free done-claims. See the [completion-evidence gate API](./reference/completion-evidence-gate-api.md) and the [rejected-completion runbook](./howto/diagnose-a-rejected-goal-completion.md).

- [How to run the OODA daemon](./howto/run-ooda-daemon.md) - Start the continuous OODA loop for autonomous goal-driven operation and act on meeting decisions.
- [How to diagnose OODA decide/orient brain parse failures](./howto/diagnose-decide-orient-parse-failures.md) - Runbook for the silent-fallback fix (#1890) and the Copilot launch-log preamble deadlock (#2496): find the ERROR log, read the `parse_failure` cycle-report block, confirm launcher noise is stripped, and remediate.
- [Concept: Copilot launch-log preamble stripping](./concepts/copilot-launcher-preamble-stripping.md) - Why the Copilot CLI launch-log preamble + ANSI noise is stripped at the single shared `recipe_output` chokepoint so decide/orient/lifecycle/merge-judge/distill all parse the agent's real output — closing the deadlock where every active goal misparsed to `default_malformed`, the ladder exhausted, and Simard spawned zero engineers (#2496, generalising distill PR #2500). See [recipe-brain verdict/decision parsing](./reference/recipe-brain-verdict-parsing.md) and [text-parsing wire formats](./reference/text-parsing-wire-formats.md).
- [OODA brain parse-failure record reference](./reference/ooda-brain-parse-failure-record.md) - Schema and visibility contract for decide/orient brain JSON-parse failures (#1890, sibling of #1711 / #1748).
- [Simard CLI reference](./reference/simard-cli.md) - Look up the shipped command tree, `engineer read` audit surface, and compatibility mappings.
- [Runtime contracts reference](./reference/runtime-contracts.md) - Look up executable contracts, state-root guarantees, and the shipped engineer audit readback semantics.
- [Base type adapters reference](./reference/base-type-adapters.md) - Look up the pluggable agent execution substrates, their capabilities, and topology support.
- [Meeting backend API reference](./reference/meeting-backend-api.md) - Rust API for the unified MeetingBackend.
- [Meeting close lifecycle reference](./reference/meeting-close-lifecycle.md) - Bounded close, partial-handoff envelope, atomic writes (#1908).
- [Handoff lifecycle API reference](./reference/handoff-lifecycle-api.md) - Write guard, batch processing, and reaping for meeting handoff files (#2268).
- [State-root resolution reference](./reference/state-root-resolution.md) - The shared helper honoring `SIMARD_STATE_ROOT` across every Simard mode (#1906).
- [How to recover from a meeting close timeout](./howto/recover-from-meeting-close-timeout.md) - Playbook when `handoff_partial=true` fires.
- [LightweightChatSession reference](./reference/lightweight-chat-session.md) - Direct-subprocess session used for Copilot-provider meeting turns (no PTY overhead).
- [Terminal session idle detection](./reference/terminal-session-idle-detection.md) - How Simard determines when a PTY session is genuinely idle vs. silently computing.
- [Tokenized fact recall in preparation](./reference/cognitive-memory-fact-recall.md) - How `search_facts` tokenizes a multi-word objective into keywords and ORs one `CONTAINS` per token so semantic facts (and `goal-store:record` goal facts) actually surface into the OODA prepared context — fixes the "facts always zero" defect (issue #2302).
- [Ranked episodic recall & memory reinforcement](./reference/cognitive-memory-ranked-episodic-recall.md) - How OODA preparation recalls past episodes with the library's multi-signal ranked recall (relevance + confidence + importance + recency + usage + graph) instead of a flat newest-first keyword scan, how a UNION backfill keeps compressed consolidation sources recallable, and how a usage/recency reinforcement seam plus `CognitiveFact` observability record accesses at the point a memory is used — fixes Simard's under-application of the amplihack-memory model (issue #2395).
- [Cognitive memory bridge helpers](./reference/cognitive-memory-bridge-helpers.md) - `launch_writer_bridge` / `open_reader_bridge` resolution ladder; design notes for the planned in-process Arc shortcut and strict no-silent-degradation contract (issue #1590 follow-up).
- [Procedural-memory store idempotency](./reference/cognitive-memory-procedural-idempotency.md) - `store_procedure` deduplicates on exact name so repeated OODA consolidation cycles stop re-storing identical procedures, ending the 0% compression / frozen procedural-store defect (issue #2298).
- [Episode ingestion policy & automatic promotion](./architecture/episode-ingestion-policy.md) - A deterministic classifier that drops/down-scopes operational-noise episodes before they are stored, plus a scheduler that automatically distills recurring episodes into provenance-linked facts and procedures every cycle — not only when the brain chooses `ConsolidateMemory` (issue #2327).
- [Episode ingestion classifier API](./reference/episode-ingestion-classifier.md) - `classify` / `sanitize_transcript` decision rules, the `EventKind` / `EpisodeMetadata` / `Decision` types, the `store_episode_classified` IO seam, and the metadata JSON contract (issue #2327).
- [Automatic distillation scheduler API](./reference/automatic-distillation-scheduler.md) - `run_scheduled_distillation` / `distill_trigger` trigger predicate, the `SIMARD_DISTILL_MIN_EPISODES` / `SIMARD_DISTILL_INTERVAL_CYCLES` config, and the `DistilledProcedure` / `run_all` distillation procedures extension (issue #2327).
- [Distill recipe output capture](./reference/distill-recipe-output-capture.md) - How the distillation pass reliably captures the distill agent's `{ "facts": …, "procedures": … }` JSON from `recipe-runner-rs` via `--output-format json`, the `RecipeRunnerEnvelope` types, the three-tier parser, failure semantics, and the `redeploy-local.sh` recipe-asset sync — fixes distillation never producing facts in production (issue #2401).
- [How to configure episode hygiene and promotion](./howto/configure-episode-hygiene-and-promotion.md) - Tune the promotion thresholds, read the per-cycle intake/promotion log lines, and verify provenance-linked facts and procedures (issue #2327).
- [Cognitive-memory goal store adapter](./reference/cognitive-memory-goal-store.md) - Superseded design for the planned `GoalStore` implementation that was replaced by the file-backed store (issue #2182).
- [File-backed goal store reference](./reference/file-backed-goal-store.md) - Production GoalStore with flock locking at goal_store.json (issue #2182).
- [String truncation helpers](./reference/string-truncation-helpers.md) - Design for the planned `truncate_to_char_boundary` UTF-8-safe byte-budget helper (issue #1590 follow-up).
- [Concept: truthful runtime metadata](./concepts/truthful-runtime-metadata.md) - Read the design rationale behind the stricter runtime contract.
- [Concept: improvement context — denser execution evidence for the engineer loop](./concepts/improvement-context-execution-evidence-gap.md) - Captured improvement-curation context preserving the active "Capture denser execution evidence" goal and the observation that the legacy `simard_operator_probe` surface does not yet expose a terminal engineer-loop probe.
- [Concept: automated disk health management](./concepts/automated-disk-health.md) - Design rationale for the per-cycle disk health check that prevents disk exhaustion (#2020).
- [Concept: prompt-driven TDD discipline](./concepts/prompt-driven-tdd-discipline.md) - Why TDD commit ordering is enforced through the engineer system prompt, not CI scripts or git history parsing.
- [Concept: pluggable identity](./concepts/pluggable-identity.md) - Design rationale for TOML-driven agent personas that let different repos define distinct identities (#2242).
- [How to configure pluggable identities](./howto/configure-pluggable-identity.md) - Create an `identity.toml` file for custom agent personas, operating modes, and prompt assets.
- [Pluggable identity API reference](./reference/pluggable-identity-api.md) - Rust API for `FileIdentityLoader`, TOML types, `load_watches_from_file`, and error variants.

## Canonical executable surface

Simard guarantees these operator-visible namespaces on the primary binary:

- `simard engineer ...`
- `simard meeting ...`
- `simard goal-curation ...`
- `simard improvement-curation ...`
- `simard gym ...`
- `simard review ...`
- `simard bootstrap ...`
- `simard ooda ...`
- `simard act-on-decisions`
- `simard spawn ...`
- `simard handover ...`

Bare `simard` prints the unified help text instead of attempting a hidden environment-only bootstrap.

## TUI monitoring dashboard

- [How to monitor Simard with the TUI](./howto/monitor-simard-with-tui.md) - Launch `simard-tui` and read daemon health, goals, and system stats from a single terminal pane.
- [simard-tui reference](./reference/simard-tui.md) - Full specification of tabs, data sources, refresh behaviour, environment variables, and security model.
- [Multi-binary self-update reference](./reference/multi-binary-self-update.md) - How `simard update` now replaces the **full** binary set (`simard` plus `simard-tui`, `simard-gym`, and the rest), the dynamic discovery and `InstallReport` main-fatal/aux-best-effort contract, the SHA-256 checksum gate, and the matching release-packaging producer contract (#2252).

## Compatibility binaries

The compatibility binaries remain shipped, but they are no longer the canonical entrypoint:

- `simard_operator_probe` preserves the legacy multi-mode probe commands
- `simard-gym` preserves the legacy benchmark binary

Use them only when you need compatibility with older scripts or exact legacy output.

## Running from source

From the repository root, the corresponding Cargo commands are:

- `cargo run --quiet -- ...` for `simard`
- `cargo run --quiet --bin simard_operator_probe -- ...` for `simard_operator_probe`
- `cargo run --quiet --bin simard-gym -- ...` for `simard-gym`

If you are tight on disk or working across many Simard worktrees, prefer `scripts/cargo-low-space ...` for local builds and use `scripts/reclaim-build-space` to preview or delete stale build artifact directories.

## Contributor verification

Repository changes are expected to pass the same checks locally and in CI:

- `python3 -m pre_commit install --hook-type pre-commit --hook-type pre-push`
- `python3 -m pre_commit run --all-files --hook-stage pre-commit`
- `python3 -m pre_commit run --all-files --hook-stage pre-push`

Those hooks enforce `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, and `cargo test --all-features --locked`.

## Reading paths

If you are new to Simard, start with the [local session tutorial](./tutorials/run-your-first-local-session.md).

If you need exact commands, use the [Simard CLI reference](./reference/simard-cli.md).

If you need exact field names or lifecycle errors, use the [runtime contracts reference](./reference/runtime-contracts.md).

If you are changing architecture, start with the [architecture overview](./architecture/overview.md), then read the [truthful runtime metadata concept guide](./concepts/truthful-runtime-metadata.md).

## Architecture

- [Architecture overview](./architecture/overview.md) - System diagram, core principles, component descriptions, and module map.
- [Goal board persistence](./concepts/goal-board-persistence.md) — cognitive-memory single source of truth.
- [File-backed goal store simplification](./concepts/file-backed-goal-store-simplification.md) — why GoalStore uses a plain JSON file instead of IPC.
- [Adaptive scaling](./concepts/adaptive-scaling.md) — AIMD concurrency control for the OODA cycle.
- [Goal board API reference](./reference/goal-board-api.md) — `active_goals_as_records` adapter and load/save semantics.
- [Goal decomposition & the goal graph](./reference/goal-decomposition.md) — break a large goal into 2–6 bounded sub-goals and record parent↔child structure as typed, queryable edges in the cognitive-memory graph, with parent-progress roll-up and the `simard goal decompose` verb (#2405).
- [Adaptive scaling API reference](./reference/adaptive-scaling-api.md) — AdaptiveScaler Rust API and integration.
- [Maximum safe parallelism](./reference/maximum-safe-parallelism.md) — how the OODA daemon fills spare capacity with concurrent engineers on distinct work items, bounded by the AIMD safety cap.
- [Concurrent engineer dispatch](./reference/concurrent-engineer-dispatch.md) — how the Act phase dispatches spawn-path AdvanceGoal actions concurrently (per-goal LLM sessions, atomic claim, semaphore cap) so multiple engineers start in a single OODA round.
- [PR-finalization review pipeline](./reference/pr-finalization-pipeline.md) — the bounded, ordered review pipeline every engineer runs before merge: a high-end-model crusty review→fix loop, the pr-guide illustrated walkthrough (graceful-skip), a final lightweight review, then the existing merge-ready gate → merge → close issue.
- [Concept: operational autonomy model](./concepts/operational-autonomy-model.md) — how Simard self-promotes goals and self-validates / self-merges clean, green, merge-ready work autonomously for most operations (no human-approver wait), the named HIGH-RISK boundary that still requires operator sign-off, and the quality / safety gates that stay fully intact. See the [cross-repo merge authority reference](./reference/cross-repo-merge-authority.md).
- [Cross-repo merge authority reference](./reference/cross-repo-merge-authority.md) — Simard's repo-parameterized gated squash-merge (default `rysweet/Simard`) and the `simard merge-pr <PR> --repo <owner/repo>` CLI that lands merge-ready PRs across every repo she governs through the same objective-gates + merge-judge pipeline.

- [Agent composition](./architecture/agent-composition.md) - How Simard composes subordinate agents with goal assignment, supervision, and crash recovery.
- [Cognitive memory](./architecture/cognitive-memory.md) - Six-type memory model, session lifecycle mapping, and hive mind integration.
- [Library-backed cognitive memory](./architecture/cognitive-memory-library-adapter.md) - The `amplihack-memory-lib` backend (`LibraryCognitiveMemory`), the sole on-disk cognitive-memory store after the de-fork (Phase 2b).
- [Implementation plan](./architecture/implementation-plan.md) - Phased roadmap with current status and quality gates.
- [OODA meeting handoff integration](./architecture/ooda-meeting-handoff-integration.md) - Wire meeting handoffs into the OODA daemon and seed default goals (Issues #157, #158).
- [Unified meeting backend](./architecture/unified-meeting-backend.md) - One conversational engine behind CLI REPL and dashboard WebSocket chat (Issue #462).
