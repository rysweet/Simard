---
title: "Durable-Documentation Policy (G4)"
description: >
  The durable-documentation policy — Simard's repo carries ACCURATE, DURABLE,
  easily-updated documentation of how the system actually works, and NEVER
  point-in-time investigation/testing/diagnosis report docs. Point-in-time
  findings belong in a GitHub issue and/or memory, not in a committed repo doc.
  The policy applies identically to human contributors and to Simard's own OODA
  reasoners and engineer sessions, and is enforced by two rails: an agentic
  prevention rail in the prompts and a deterministic backstop scan at the
  Overseer merge gate.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: concept
status: reference
related:
  - ../reference/no-point-in-time-docs-scan.md
  - ../howto/record-an-investigation-finding.md
  - ../design/overseer.md
  - ../reference/pr-finalization-pipeline.md
  - ../reference/cross-repo-merge-authority.md
  - ../concepts/stewardship-mode.md
---

# Durable-Documentation Policy (G4)

Simard's repository documentation must be **accurate**, **durable**, and easy to
**update** as new PRs change features — documentation that describes how the
system *actually works today* and stays current. The fourth durable engineering
guideline, **G4 — `no-point-in-time-docs`**, protects that property by keeping
one class of writing *out* of the repo entirely: **point-in-time report docs**.

G4 sits alongside [G1/G2/G3](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#engineering-guidelines-g1g2g3g4)
as a durable engineering principle. Like the others it applies to **human
contributors *and*** to Simard's own OODA reasoners and engineer sessions, is
encoded declaratively in the hot-reloaded prompt assets under
`prompt_assets/simard/`, is surfaced as a soft review flag, and is pinned by the
presence test at `tests/engineering_guidelines_prompts.rs`. Unlike G1/G2/G3, G4
also has a **hard deterministic backstop**: the Overseer's pr-verify scan
`scan_no_point_in_time_report_docs` blocks a merge that would commit a new report
doc.

> **The canonical, human-facing source of truth for G4 is
> [`CONTRIBUTING.md` § Engineering Guidelines](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#engineering-guidelines-g1g2g3g4).**
> This concept doc explains the *why* and the architecture; the
> [scan reference](../reference/no-point-in-time-docs-scan.md) documents the
> deterministic behavior; the
> [how-to](../howto/record-an-investigation-finding.md) shows what to do instead.

## The problem G4 solves

Simard's autonomous OODA/engineer loop is prolific. Left unconstrained it does a
useful thing badly: every time it investigates a recurring blockage it wants to
**write the investigation down**, and the path of least resistance is a new
markdown file committed into the repo. Recent examples in `rysweet/Simard` were a
run of `docs(investigation)` / `docs(overseer)` PRs — #2879, #2843, #2819, #2814,
#2801 — each a *kgpacks-rs blockage investigation/diagnosis report* captured as a
committed doc.

That is the wrong home for that content, for three reasons:

1. **It goes stale.** An investigation report is true at the moment it is written
   and progressively wrong thereafter. A doc that says "distillation parse-failure
   rate is ≈62%" or "the kgpacks-rs build is blocked on X" describes a *moment*,
   not durable behavior. Nothing updates it, so it rots in place.
2. **It poisons agent context.** Simard reads her own repo docs as grounding.
   Stale point-in-time reports feed her false "current state" and send later
   reasoning down dead ends — the exact failure the report was trying to prevent.
3. **It buries the durable docs.** A `docs/` tree full of one-shot reports makes
   the accurate, maintained architecture/feature docs harder to find and trust.

The findings themselves are *valuable* — they just belong somewhere **built for
point-in-time knowledge that gets superseded**: a GitHub issue (trackable,
closable, dedup-able) and/or Simard's memory. G4 routes them there.

## The distinction: doc **type**, not topic

G4 is about **what kind of document** you are writing, **not** what it is about.
The same subsystem can be the topic of both a good durable doc and a banned
point-in-time report:

| Same topic — kgpacks-rs parity | Verdict | Why |
|---|---|---|
| `docs/design/kgpacks-parity.md` — how the parity design/architecture works, kept current as it changes | ✅ **durable — keep, maintain** | Describes durable behavior of a subsystem; updated by future PRs |
| `docs/architecture/…` update explaining a module's current behavior | ✅ **durable — keep, maintain** | Reference/architecture that stays accurate |
| A how-to for operating/using a feature | ✅ **durable — keep, maintain** | Task guidance that stays valid |
| `docs/investigation/kgpacks-blockage-2026-07.md` — "what I found while diagnosing the blockage" | ❌ **point-in-time — issue/memory** | A snapshot of a moment; goes stale, poisons context |
| A "testing report" / "what testing I did" write-up | ❌ **point-in-time — issue/memory** | Records an activity at a moment, not durable behavior |
| A diagnosis / blockage-recurrence report | ❌ **point-in-time — issue/memory** | Captures a moment's state of a bug |
| A measured-rate / benchmark **snapshot findings** doc | ❌ **point-in-time — issue/memory** | A single measurement, not a maintained reference |

The heuristic: **if a later PR that changes the feature would be expected to
update the doc, it is durable. If the doc is only ever true "as of" the day it was
written, it is point-in-time.** Durable feature/architecture documentation updates
remain **explicitly encouraged** — G4 never discourages keeping the real docs
current.

## Where findings go instead

When Simard (or any contributor) produces an investigation, testing, or diagnosis
**finding**, it is recorded as:

- **A GitHub issue** — the authoritative sink. Trackable, assignable, closable,
  and deduplicated. Recurring failures are consolidated into a single tracking
  issue rather than a new doc per occurrence. This dovetails with
  [Goal Stewardship Mode](../concepts/stewardship-mode.md), which already turns
  recurring failures into deduplicated issues.
- **Memory (optional)** — Simard may also record the finding in her cognitive
  memory so later reasoning can recall it. Any memory-engine work stays upstream
  in `amplihack-memory-lib` per [G2](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#engineering-guidelines-g1g2g3g4);
  G4 does not add memory code to Simard's repo.

See [Record an investigation finding](../howto/record-an-investigation-finding.md)
for the step-by-step.

## Two-rail enforcement

G4 is enforced by two complementary rails. The agentic rail **prevents** the loop
from authoring a report doc in the first place; the deterministic rail is a
**hard backstop** that blocks any report doc that still reaches the merge gate.

| Rail | Mechanism | Failure mode it closes | Where |
|---|---|---|---|
| **Agentic (prevention)** | G4 guidance in the engineer/OODA prompts + soft review flag | The loop *authors* a report doc at all | `engineer_system.md`, `engineer_planning.md`, `ooda_orient.md`, `ooda_decide.md`, `ooda_brain.md`, `overseer/pr_verify.md`, `merge_readiness_judge.md`, `review_pipeline.md` (+ recipe mirrors) |
| **Deterministic (backstop)** | Pure diff-scan `scan_no_point_in_time_report_docs`, auto-run by `run_diff_scans` | A report-doc PR reaches the **merge gate** | `src/overseer/pr_verify.rs` (pr-verify check #8) |

The deterministic scan is the same pattern as the existing pr-verify scans
(no-`Bridge`-naming, no stray `print!`, additive-only, PRD-preserved): a **pure
function over a unified diff** that returns findings and blocks the merge when it
finds a violation. It never uses `--admin` or `--no-verify`; a flagged PR simply
does not pass the gate until the report doc is removed and its content moved to an
issue/memory. Full behavior is in the
[scan reference](../reference/no-point-in-time-docs-scan.md).

### What the deterministic scan does and does not flag

The scan is deliberately **narrow and forward-only** so it can never touch the
durable docs that already exist:

- **Added-only.** It flags **newly added** markdown files only. Edits to
  pre-existing docs — including updates to durable architecture/reference docs
  that happen to add report-like words — are never flagged. This is what lets G4
  land without disturbing any doc currently in the repo, and what keeps the
  enforcement PR (which only *edits* `CONTRIBUTING.md` and prompt `.md` files)
  from flagging itself.
- **Report-typed, not topic-typed.** A newly added `.md` is flagged when it lives
  under a **reserved report directory** (`docs/investigation/`, `docs/reports/`,
  `docs/runs/`) *or* when an added **title / front-matter** line marks it a report
  (e.g. an H1 or `type:` containing `investigation report`, `testing report`,
  `diagnosis`, `recurrence report`, `benchmark snapshot`, `measured-rate`,
  `postmortem`). Report vocabulary appearing in the **body prose** of an otherwise
  durable doc does **not** trip the scan.
- **Durable docs are safe — when they carry a durable title.** Added durable docs
  under `docs/architecture/`, `docs/design/`, `docs/reference/`, `docs/howto/`,
  `docs/concepts/`, and the `docs/` durable set pass, as do
  `README`/`CONTRIBUTING`/`CHANGELOG` and prompt assets — **provided their title is
  a feature/architecture title, not a report title.** The title rail applies
  everywhere *outside* the reserved report directories, so a newly added
  `docs/design/x.md` whose added H1 is `# Investigation Report` (or whose
  front-matter `type:` marks it a report) **is** flagged even though it sits under a
  durable directory. Give a durable doc a durable title and it passes; a
  report-typed title trips the title rail regardless of directory. (Report
  vocabulary in ordinary body prose never trips the scan — only the title /
  front-matter line does.)

## Applies to everyone

G4 binds **both** Simard and any contributor or agent. A human opening a PR that
adds an investigation report gets the same merge-gate finding, with the same
guidance to move the content to an issue/memory, as Simard's own OODA loop does.
The policy is a property of the repository, not a per-author rule.

## Relationship to the other guidelines

- **G3 (prefer prompts/recipes over brittle code)** — G4 is applied *G3-style*:
  the primary rail is prompt guidance; the deterministic scan is a **thin**
  backstop, not a large parser. The scan reuses the existing pr-verify diff
  primitives rather than inventing new machinery.
- **G2 (memory work upstream)** — the "and/or memory" sink for findings uses the
  upstream `amplihack-memory-lib`; G4 adds no memory-engine code to Simard.
- **G1/G2/G3 encoding** — G4 is registered in the same layers table and pinned by
  the same presence test, so a future prompt edit cannot silently drop it.

## See also

- [No point-in-time report docs — pr-verify scan reference](../reference/no-point-in-time-docs-scan.md)
- [How to record an investigation finding (issue/memory, not a repo doc)](../howto/record-an-investigation-finding.md)
- [Overseer — operator/observer co-process (design)](../design/overseer.md) — the merge gate this scan runs inside
- [Goal Stewardship Mode](../concepts/stewardship-mode.md) — the issue sink for recurring failures
- [`CONTRIBUTING.md` § Engineering Guidelines](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#engineering-guidelines-g1g2g3g4) — the durable source of truth for G1–G4
