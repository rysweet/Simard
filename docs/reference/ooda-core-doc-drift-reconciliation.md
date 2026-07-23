---
title: "OODA-core documentation-drift reconciliation (#4285/#4286/#4288/#4290)"
description: >
  Record of the mechanical, one-at-a-time documentation-drift sweep that
  re-aligned the shared OODA-core reference/concept pages with current code and
  restored cross-links, gated by scripts/verify-docs.sh under mkdocs --strict.
last_updated: 2026-07-18
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/durable-documentation-policy.md
  - ../concepts/no-progress-terminal-investigation.md
  - ./goal-board-authoritative-cycle-persistence.md
  - ./e2big-errno-signal-alignment.md
---

# OODA-core documentation-drift reconciliation

> **Status: implemented.** The four `code-atlas-bughunt` doc-drift findings
> ([#4285](https://github.com/rysweet/Simard/issues/4285),
> [#4286](https://github.com/rysweet/Simard/issues/4286),
> [#4288](https://github.com/rysweet/Simard/issues/4288),
> [#4290](https://github.com/rysweet/Simard/issues/4290)) are reconciled: the
> drifted OODA-core docs now match current code and are cross-linked. Each issue
> is linked and closed.

## What drifted

The four findings flagged OODA-core documentation that no longer matched code or
behavior — stale references, moved/renamed surfaces, and missing cross-links
across the shared `docs/reference/`, `docs/concepts/`, and `docs/atlas/` pages
that describe the OODA loop, goal board, and diagnosis paths.

## How it was corrected (mechanical, sequenced)

Because these findings touch **shared** OODA-core files, the sweep runs strictly
**one issue at a time** as a single sequenced workstream (`sequence_group:
ooda-core`) to avoid concurrent edits colliding on the same files. It is a
mechanical documentation sweep — no code behavior changes:

1. For each issue, correct the drifted prose/tables/links to match the current
   code surface.
2. Restore or add the cross-links between the affected pages (and to the related
   [#4287 persistence](./goal-board-authoritative-cycle-persistence.md) and
   [#4289 E2BIG](./e2big-errno-signal-alignment.md) docs where relevant).
3. Run the docs gate and close the issue before starting the next.

The sweep is additive/non-breaking, preserves the PRD, adds no "Bridge" naming,
and introduces no stray `print!`/`println!` (docs only).

## Verification gate

Every corrected page passes the durable-documentation gate before its issue is
closed:

```bash
scripts/verify-docs.sh      # link-integrity + orphan check (mkdocs --strict, nav completeness)
```

The gate enforces the [durable-documentation policy](../concepts/durable-documentation-policy.md):
zero orphaned pages (every `docs/*.md` appears in `mkdocs.yml` nav) and zero dead
links/anchors under `--strict`.

## Issue disposition

| Issue | Scope | Status |
|-------|-------|--------|
| #4285 | OODA-core doc drift | corrected, cross-linked, closed |
| #4286 | OODA-core doc drift | corrected, cross-linked, closed |
| #4288 | OODA-core doc drift | corrected, cross-linked, closed |
| #4290 | OODA-core doc drift | corrected, cross-linked, closed |
