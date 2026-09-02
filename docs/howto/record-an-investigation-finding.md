---
title: "Record an investigation finding (issue/memory, not a repo doc)"
description: >
  How-to for the durable-documentation policy (G4). Capture an investigation,
  testing, or diagnosis finding as a GitHub issue and/or memory instead of a
  committed repo doc; respond when the Overseer pr-verify scan flags a PR for
  adding a point-in-time report doc; and split a durable doc out of a mixed PR.
  Applies to human contributors and to Simard's own OODA/engineer loop.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: howto
status: reference
related:
  - ../concepts/durable-documentation-policy.md
  - ../reference/no-point-in-time-docs-scan.md
  - ../concepts/stewardship-mode.md
  - ../howto/file-stewardship-issues-from-orchestrator-runs.md
---

# Record an investigation finding (issue/memory, not a repo doc)

This guide shows what to do when you — or Simard's OODA/engineer loop — produce an
**investigation, testing, or diagnosis finding**: record it as a **GitHub issue
and/or memory**, never as a committed repo doc. This is the operational side of
the [durable-documentation policy (G4)](../concepts/durable-documentation-policy.md).
It applies identically to human contributors and to Simard's autonomous loop.

For the deterministic gate that enforces this, see the
[pr-verify scan reference](../reference/no-point-in-time-docs-scan.md). The
canonical rule lives in
[`CONTRIBUTING.md` § Engineering Guidelines](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#engineering-guidelines-g1g2g3g4).

## First: is your finding point-in-time or durable?

Decide **doc type, not topic** (see the
[policy concept](../concepts/durable-documentation-policy.md#the-distinction-doc-type-not-topic)):

- **Point-in-time → issue/memory (this guide).** "What I found while diagnosing
  X", a testing/"what testing I did" write-up, a blockage/recurrence report, a
  measured-rate or benchmark **snapshot**. True as-of a moment; goes stale.
- **Durable → keep it in the repo, and keep it current.** How a feature/subsystem
  actually works (architecture/design/reference/how-to). A later PR that changes
  the feature is expected to update it. G4 **encourages** these; put them under
  `docs/architecture/`, `docs/design/`, `docs/reference/`, `docs/howto/`, or
  `docs/concepts/`.

If it is durable, write/update the durable doc as usual — you are done. If it is
point-in-time, continue below.

## Record a finding as a GitHub issue (authoritative sink)

A GitHub issue is the authoritative home for a point-in-time finding: trackable,
assignable, closable, and deduplicated.

```bash
gh issue create \
  --repo rysweet/Simard \
  --title "kgpacks-rs blockage — investigation findings" \
  --body-file - <<'EOF'
## Summary
<one-paragraph statement of the finding as of today>

## Evidence
- <observation, log excerpt, measured rate, repro command>
- <link to the run / PR / CI job this came from>

## Current hypothesis / next step
<what you believe and what would confirm or fix it>
EOF
```

Guidelines:

- **Consolidate recurrences into one tracking issue.** If the same blockage keeps
  recurring, add a comment to the existing tracking issue instead of opening a new
  issue (or a new doc) per occurrence. One issue accumulates the history; the doc
  tree stays clean.
- **Keep the moment in the moment.** Dates, measured rates, and "as of" state
  belong in the issue body/comments, where they are expected to be superseded —
  not in a repo doc, where they rot.
- Simard's OODA loop already has a deduplicated failure→issue path via
  [Goal Stewardship Mode](../concepts/stewardship-mode.md); see
  [File stewardship issues from orchestrator runs](../howto/file-stewardship-issues-from-orchestrator-runs.md).

## Optionally also record it in memory

Simard may additionally record the finding in her cognitive memory so later
reasoning can recall it. Any memory-engine work stays **upstream** in
`amplihack-memory-lib` per [G2](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#engineering-guidelines-g1g2g3g4)
— **do not** add memory code to Simard's repo to satisfy G4. The issue remains the
authoritative, human-visible sink; memory is a convenience for the loop.

## Respond when the pr-verify scan flags your PR

If a PR **adds a new point-in-time report doc**, the Overseer pr-verify scan
`scan_no_point_in_time_report_docs` (check #8) flags it and the merge gate blocks
with a finding like:

```
docs/investigation/kgpacks-blockage.md — point-in-time report doc
(no-point-in-time-docs / G4). Record findings in a GitHub issue and/or memory;
do not commit investigation/testing/diagnosis reports to the repo.
```

There is **no `--admin` / `--no-verify` bypass** — resolve it, don't override it:

1. **Move the content to an issue** (or the consolidated tracking issue) using the
   steps above. Preserve the substantive findings; nothing is thrown away.
2. **Remove the report doc from the PR:**
   ```bash
   git rm docs/investigation/kgpacks-blockage.md
   git commit -m "Move kgpacks-rs blockage findings to issue #<n> (G4)"
   ```
3. **Re-run the checks** (pre-commit/pre-push locally, then CI). The scan is
   added-only, so once the added report doc is gone the finding clears.

If the flagged file is genuinely a **durable** feature/architecture doc that only
*looks* report-shaped, the fix is to make it durable, not to bypass the gate:
place it under a durable directory (`docs/architecture/`, `docs/design/`,
`docs/reference/`, `docs/howto/`, `docs/concepts/`) and give it a
feature/architecture title rather than a report title (`# Investigation Report`,
`type: diagnosis`, …). The scan only reserves the report directories and
report-typed titles; durable docs pass.

## Split a durable doc out of a mixed PR

If a PR contains **both** a point-in-time report **and** a keep-worthy durable
doc, split them:

1. Create a separate branch/PR containing only the durable doc, placed under the
   right durable directory with a durable title. Merge it normally.
2. In the original PR, remove the report doc and move its findings to an issue
   (steps above). The original PR then either lands its remaining durable changes
   or is closed.

This is exactly how the historical violating `docs(investigation)` PRs were
reconciled: findings preserved in a consolidated tracking issue, the doc-only PRs
closed with a pointer to that issue, and any durable content split out to its own
keep PR.

## Common pitfalls

- **"But I need to write the investigation down."** You do — in the **issue**. G4
  bans the *repo doc*, not the record. The issue is the durable-for-this-purpose
  home.
- **Renaming to dodge the scan.** Moving a report to `docs/notes/` with a
  `# Diagnosis Report` H1 still trips the **title rail**. The right move is
  issue/memory, not a relocation.
- **Editing an existing report doc.** Allowed by the scan (added-only), but if
  you are adding a *fresh* snapshot, prefer an issue — pre-existing point-in-time
  docs are legacy, not a pattern to extend.
- **Fearing your durable-doc PR will be flagged.** It won't: durable docs under
  the durable directories, and edits to any existing doc, are never flagged. Only
  **newly added** report-typed markdown is.
- **Trying to override the gate.** `--admin`/`--no-verify` are never used. Move
  the content and re-run; that is the only path.

## Related

- [Durable-Documentation Policy (G4)](../concepts/durable-documentation-policy.md)
- [No point-in-time report docs — pr-verify scan reference](../reference/no-point-in-time-docs-scan.md)
- [Goal Stewardship Mode](../concepts/stewardship-mode.md) — the deduplicated failure→issue loop
- [File stewardship issues from orchestrator runs](../howto/file-stewardship-issues-from-orchestrator-runs.md)
- [`CONTRIBUTING.md` § Engineering Guidelines](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#engineering-guidelines-g1g2g3g4)
