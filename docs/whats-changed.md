---
title: What's Changed — documentation audit
description: Change index for the documentation accuracy-and-discoverability audit that reconciled docs/ with the current source, de-forked the cognitive-memory pages, and made every page reachable from the nav.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
doc_type: explanation
---

# What's Changed

This page is the summary index for the documentation audit that brought `docs/`
back in line with the **current** state of the source tree. It records *what*
changed and *why*, so reviewers can verify each claim against code rather than
against a point-in-time snapshot.

Every statement below was verified against the source on `main` at the time of
the audit. Where a doc still describes a removed component, it is now framed
explicitly as history (with the issue that removed it) rather than as current
behaviour.

## Why this audit happened

The documentation had three classes of drift:

1. **Stale symbols.** Pages still described the deleted in-repo native
   cognitive-memory fork (`NativeCognitiveMemory`) as if it were the live
   backend and API.
2. **Discoverability gap.** 118 of the 208 Markdown pages existed on disk but
   were absent from the navigation manifest (`mkdocs.yml`), so they were
   effectively invisible to readers.
3. **Broken internal links.** A handful of cross-page `#anchor` links pointed
   at anchors that no longer existed.

## What changed

### 1. Cognitive memory de-forked to the library backend (#2307)

The native Rust fork `NativeCognitiveMemory` (written directly over
LadybugDB / `lbug`) was **deleted** by the de-fork, Phase 2b (issue #2307).
The sole cognitive-memory backend is now `LibraryCognitiveMemory`
(`src/cognitive_memory/library_adapter.rs`), an adapter over the external
[`amplihack-memory`](https://github.com/rysweet/amplihack-memory-lib) library
(pinned by git rev, built with the `persistent` feature). That library uses
`lbug =0.17.1` internally; Simard keeps `lbug` as a direct dependency **only**
for the `simard-tui` binary / goal board, not for cognitive memory.

Durability, verified backups and pruning are no longer methods on the deleted
fork. They are free functions in the `memory_backup` module — see
[Backup-Pruning API](reference/backup-pruning-api.md),
[Verified Backups](operations/verified-backups.md) and
[Cognitive-Memory Durability](operations/cognitive-memory-durability.md).

Reconciled pages: [Cognitive Memory](architecture/cognitive-memory.md),
[Cognitive Memory — Library Adapter](architecture/cognitive-memory-library-adapter.md),
the `reference/cognitive-memory-*` recall/idempotency/bootstrap pages, and
[RPC Transport Pattern](architecture/rpc-pattern.md) — whose *Data Loss Prevention*
section no longer attributes memory-write durability to a Python client. Writes
now go directly through the in-process `LibraryCognitiveMemory` adapter
(idempotent by `node_id`), and durability is provided by the `memory_backup`
verified-backup APIs.

The **six-type** cognitive-memory model is unchanged: Sensory, Working,
Episodic, Semantic (Fact), Procedural, and Prospective.

### 2. Brains and the OODA loop

The OODA loop drives three brain traits — `OodaDecideBrain` (decide),
`OodaOrientBrain` (orient) and `OodaBrain` (engineer-lifecycle / act). The
unified [`RecipeBrain`](reference/recipe-brain-api.md) struct implements all
three, with deterministic fallbacks (`DeterministicDecideBrain`,
`DeterministicOrientBrain`, `DeterministicLifecycleBrain`). These trait names
are current source symbols — see [Unified RecipeBrain](concepts/unified-recipe-brain.md)
and [OodaBrain API](reference/ooda-brain-api.md).

The Copilot launch-log preamble + ANSI noise is stripped at a **single shared
`recipe_output` chokepoint** so decide/orient/lifecycle/merge-judge/distill all
parse the agent's real output (issue #2496, PR #2504, generalising the distill
fix). See
[Copilot Launch-Log Preamble Stripping](concepts/copilot-launcher-preamble-stripping.md)
and [Distill Recipe-Output Capture](reference/distill-recipe-output-capture.md).

### 3. Base-type adapters

Shipping substrates are `rusty-clawd` and `copilot`; the Claude Agent SDK and
Microsoft Agent Framework adapters are present as structural stubs pending SDK
availability. See [Base-Type Adapters](reference/base-type-adapters.md).

### 4. Self-deploy and self-relaunch (#2467)

`simard self-deploy [--check]` closes the "merged but not running" gap by
building the merged change from source, deploying it, health-verifying it, and
leaving it running (or rolling back). See
[Reconcile-and-Self-Deploy](concepts/reconcile-and-self-deploy.md),
the [Self-Deploy API](reference/self-deploy-api.md), and
[Verify and Roll Back a Self-Deploy](howto/verify-and-roll-back-a-self-deploy.md).

### 5. Goal-board persistence + cross-process write lock (#2511 / #2514)

The file-backed goal store guards `goal_store.json` with an advisory
`flock` acquired via an RAII guard, so concurrent Simard processes cannot
corrupt the board. See [File-Backed Goal Store](reference/file-backed-goal-store.md)
and [Troubleshoot Goal Store](howto/troubleshoot-goal-store.md).

### 6. Discoverability: every page is now in the nav

All 118 previously-orphaned pages were added to the navigation manifest
`mkdocs.yml` under logical sections, including new **Operations**, **Operator
Dashboard**, **Testing**, and **Ecosystem & Audits** sections. Discoverability
is enforced natively in Rust: `tests/docs_integrity.rs` (run by `cargo test`)
walks `docs/**/*.md`, fails on dead intra-repo links, and fails on any
`mkdocs.yml` nav entry that points at a missing file, while
`tests/supply_chain_hardening.rs` additionally asserts the supply-chain
reference pages stay linked from the nav — so orphaned pages and dead anchors
cannot silently regress. There is no Python `mkdocs build` step; `mkdocs.yml`
is retained only as the inert nav manifest those Rust tests read.

### 7. amplihack freshness gate before each engineer spawn (#439)

Engineers run on an installed `amplihack-rs` (its recipes, `recipe-runner`, and
SDK adapters), refreshed by the operator command `amplihack update`. A **stale**
installed bundle had carried per-step agent timeouts that upstream already
**removed**; those leftover timeouts killed working agent steps mid-run. Per the
operator directive — "Simard must always be using the latest `amplihack-rs`" and
"run `amplihack update` before starting each engineer" — the freshness gate now
runs `amplihack update` immediately before `spawn_subordinate` in
`src/ooda_actions/advance_goal/spawn.rs::dispatch_spawn_engineer`, and once at
startup in `run_ooda_daemon`. The gate is serialized and deduplicated by a
`flock(2)` advisory lock at `<state_root>/amplihack-update.lock` plus a durable
last-success TTL in `<state_root>/amplihack-update-state.json` (default
`SIMARD_AMPLIHACK_UPDATE_TTL_SECS=300`), so a spawn burst performs one update,
not one per engineer. A failed update is **surfaced, never swallowed**: it logs
via `tracing` and records an `amplihack_update_failure` metric, then by default
proceeds on the last-known-good install, or — under
`SIMARD_REQUIRE_FRESH_AMPLIHACK=1` — refuses the spawn with an explicit error.
See [The amplihack freshness gate](concepts/amplihack-freshness-gate.md), the
[freshness-gate reference](reference/amplihack-freshness-gate.md), and
[Configure the amplihack freshness gate](howto/configure-amplihack-freshness-gate.md).

### 8. amplihack pins bumped to upstream main (#2626)

Two behind-`main` git-rev pins were reconciled to their current upstream `main`
HEADs so the fixes those repos already merged run in Simard's own build:
`amplihack-agent-eval` (`rysweet/amplihack-rs`) `59548a9 → 2a93441`, and
`amplihack-memory` (`rysweet/amplihack-memory-lib`) `5d7db77 → f800370`. The bump
touches **only** `Cargo.toml` and `Cargo.lock` — both upstream deltas are
API-compatible, so `gym_runner_client`, `LibraryCognitiveMemory`, and their
mirror/conversion consumers compile with **zero call-site edits**. The
`amplihack-memory` HEAD carries no engine change, so `lbug` stays `0.17.1`
(store format v41) and `cargo tree -i lbug` resolves to exactly one version; the
direct `lbug = "=0.17.1"` pin is unchanged. The bump adds no new git source or
crate, so `cargo deny` / `cargo audit` / `cargo vet` stay green. This is a
worked instance of the proactive reconcile in
[Keep Simard's dependency pins up to date](howto/self-maintain-dependency-pins.md);
the full change record is
[amplihack pin bump to upstream main (#2626)](reference/amplihack-pin-bump-2626.md).

## Relationship to the PRD

The original product requirements document,
[`Specs/ProductArchitecture.md`](https://github.com/rysweet/Simard/blob/main/Specs/ProductArchitecture.md),
remains the byte-for-byte source of truth for product intent and was **not**
modified by this audit. Where the current implementation has diverged from the
PRD (for example, the exact adapter list or backend), the divergence is
documented in `docs/` as current reality and the PRD is left unchanged.

## How to keep docs honest

- Verify every claim against source before writing it; avoid point-in-time
  ("as of N reports") snapshots.
- Keep new pages under `docs/`, add them to the `nav`, and cross-link them.
- Follow Diataxis: tutorial / how-to / reference / explanation.
- Run `cargo test docs_integrity supply_chain` — the native Rust docs-integrity
  gate enforces zero dead intra-repo links and that every `mkdocs.yml` nav entry
  resolves to a real file. No Python `mkdocs build` is required.
