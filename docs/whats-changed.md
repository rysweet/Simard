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
   were absent from the MkDocs navigation, so they were effectively invisible
   to readers.
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
[Adapter Pattern](architecture/adapter-pattern.md) — whose *Data Loss Prevention*
section no longer attributes memory-write durability to a Python subprocess server. Writes
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

All 118 previously-orphaned pages were added to `mkdocs.yml` under logical
sections, including new **Operations**, **Operator Dashboard**, **Testing**,
and **Ecosystem & Audits** sections. A native MkDocs
[`validation`](https://www.mkdocs.org/user-guide/configuration/#validation)
block now makes `mkdocs build --strict` fail on future orphaned pages and dead
anchors, so discoverability cannot silently regress.

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
- Run `mkdocs build --strict` — it now also enforces zero orphans and no dead
  anchors.
