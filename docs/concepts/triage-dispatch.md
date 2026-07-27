# Ecosystem-Observe Triage Dispatch

The **triage dispatch** stage converts an `observed_problems.ctx` handoff into a
single, validated JSON array of dispatch items. Each item is either a
**brief** (an actionable `smart-orchestrator` fix request targeting a specific
repo) or an **escalation** (a problem that cannot be fixed by an engineer in its
current state and requires manual operator intervention).

This document describes the dispatch contract, its schema, ordering rules,
configuration, and worked examples.

---

## Overview

- **Input:** an `observed_problems.ctx` file emitted by the ecosystem observer
  (a de-duplicated list of observed problems with severity, category, target
  repo, and corroborating evidence).
- **Output:** one JSON array. Every element is a dispatch item. The array is
  ordered **most-important first** (see [Ordering](#ordering)).
- **Side effects:** none. Dispatch is emit-only — it authors no code and mutates
  no repository. Briefs are picked up by downstream `smart-orchestrator` runs;
  escalations are routed to a human operator.

The dispatch stage is deterministic given its input: the same
`observed_problems.ctx` always yields the same ordered array.

---

## Dispatch item schema

The output is a JSON array. Each element is one of two shapes.

### Brief item

An actionable fix request. Consumed by a `smart-orchestrator` run.

The **canonical required set** is exactly four fields — `recipe`,
`task_description`, `target_repo`, and `success_criteria` — matching the design.
`is_mechanical_sweep` and `sequence_group` are **optional producer hints**; a
consumer must not require them and must treat an omitted hint as its default
(`is_mechanical_sweep` → `false`, `sequence_group` → `null`).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `recipe` | string | yes | Recipe to run. For actionable fixes this is `"smart-orchestrator"`. |
| `task_description` | string | yes | Self-contained problem statement: observed symptom, suspected cause, smallest responsible surface, additive/non-breaking constraints, and merge-ready expectations. Must be understandable without the original `.ctx`. |
| `target_repo` | string | yes | `owner/name` of the repo the fix targets. |
| `success_criteria` | string[] | yes | Concrete, verifiable acceptance checks. Each item is one testable statement. |
| `is_mechanical_sweep` | boolean | no (hint) | `true` for repetitive mechanical edits across many files; `false` for a targeted fix. Defaults to `false` when omitted. |
| `sequence_group` | string \| null | no (hint) | Ordering group when a brief must run before/after siblings; `null` (or omitted) when independent. |

### Escalation item

A problem that is **not** engineer-fixable in its current state (for example, a
bootstrap deadlock where the fix itself requires resources that are currently
unavailable). Routed to a human operator instead of a recipe.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `recipe` | null | yes | Always `null`. The presence of `null` recipe marks the item as an escalation. |
| `escalate` | string | yes | Human-actionable explanation: why it is not engineer-fixable, the exact manual remediation, and any follow-up bugs to file **after** the manual step unblocks the system. |

> **Discriminator:** an item is an escalation **iff** `recipe` is `null`. A brief
> always has a non-null `recipe` and never carries an `escalate` field.

---

## Ordering

Items are ordered by **blast radius, most-critical first**:

1. **Ecosystem-wide blockers** (e.g., a bootstrap deadlock that prevents *any*
   engineer from spawning) come first, regardless of whether they are briefs or
   escalations.
2. **Cross-cutting regressions** (affect many PRs / the whole default branch).
3. **Isolated regressions** (single subsystem, e.g., docs-pages-only).

Escalations are ordered inline with briefs by the same blast-radius rule — an
escalation is not automatically first or last; it sits where its impact places
it.

---

## Configuration

The `NODE_OPTIONS` memory preference used by the observe/dispatch tooling is a
saved preference. Set it to whatever heap ceiling your host requires, for
example:

```
NODE_OPTIONS=--max-old-space-size=<MB>
```

To change it, edit the amplihack config in your home directory:

```
~/.amplihack/config
```

The dispatch stage reads its input path from the observe run's temp directory,
e.g.:

```
/tmp/simard-ecosystem-observe-ctx-*/observed_problems.ctx
```

No other configuration is required. Dispatch inherits repo/target settings from
each observed problem record.

---

## Exclusions

- Items already flagged `dropped_as_in_flight` in the input are **excluded** from
  the dispatch array — they are being handled by an existing workstream.
- Dispatch never auto-remediates escalations (e.g., it will not run a disk
  cleanup). Escalations are advisory to a human operator only.

---

## Constraints applied to every brief

All briefs are authored to be **additive / non-breaking by default**:

- Do not remove or weaken required status checks or branch protections.
- Do not modify already-green CI (core CI, Auto Release).
- Preserve the PRD.
- No `Bridge` naming.
- No stray `print!` / `println!` in new code — structured `tracing` + OpenTelemetry only.
- **Least privilege for CI/workflow changes.** Any workflow permissions a brief
  introduces or edits stay at the minimum required and scoped to the job — e.g.
  a GitHub Pages deploy uses `permissions: { pages: write, id-token: write }`
  at job scope, never broadened. Branch-protection/merge-queue changes are
  additive-only and never weaken existing required checks.

---

## Examples

### Example 1 — Escalation (bootstrap deadlock)

A critical resource-pressure problem where the fix requires an engineer, but no
engineer can be admitted until disk is manually freed. Emitted as an escalation,
with two follow-up bugs to file **after** the manual step:

```json
{
  "recipe": null,
  "escalate": "Problem 1 (critical, resource_pressure, rysweet/Simard, dedup process:disk_full:root_fs) is NOT engineer-fixable in its current state: root fs / is 100% full (28G/28G, 0 avail), so the disk-admission ceiling (max_disk_used_percent=90) rejects every engineer spawn. This is a bootstrap deadlock: the fix requires an engineer, but no engineer can be admitted until disk is freed. Requires MANUAL operator intervention first: reclaim the ~2.78 GiB of stale ~/.simard/cognitive.corrupt-*/bad-*/predeploy-*/pr*-canary-* snapshots to drop usage below 90% and unblock #4803 / cycle-2630. AFTER disk is freed, file two follow-up bugs: (a) the periodic reclaim job frees 0 bytes because it perpetually defers stale snapshots 'for review' instead of collecting them; (b) `simard status` mis-reports '/home 25 GiB free' via a statvfs f_bavail illusion while df shows root full."
}
```

### Example 2 — Brief (cross-cutting merge live-lock)

```json
{
  "recipe": "smart-orchestrator",
  "task_description": "In target_repo rysweet/amplihack-rs, fix the CI/merge strict up-to-date branch-protection live-lock tracked in issue #1050. Required 'branch must be up to date before merging' protection forces every PR to rebase/re-run CI whenever any other PR merges, causing a merge live-lock (evidence: open PR #1063 sitting in mergeStateStatus=BEHIND while mergeable=MERGEABLE). Smallest surface: the repo's branch-protection / merge-queue config and CI workflows under .github/workflows that gate merges — enable a GitHub merge queue (merge_group trigger + 'Require merge queue') so PRs are batched and tested serially. Wire required status checks to run on the merge_group event. Additive / non-breaking: do not remove existing required checks or weaken protection; preserve the PRD; no Bridge naming; no stray print!/println! (tracing + OTel only). Link the resulting PR to issue #1050.",
  "target_repo": "rysweet/amplihack-rs",
  "is_mechanical_sweep": false,
  "sequence_group": null,
  "success_criteria": [
    "CI green on all required checks (including on the merge_group event)",
    "GitHub merge queue enabled and required status checks wired to merge_group; PR live-lock relieved",
    "additive / non-breaking; existing required checks and protections preserved; PRD preserved",
    "no Bridge naming; no stray print!/println! in new code (tracing + OTel only)",
    "docs/CONTRIBUTING updated with the merge-queue flow and links; PR linked to issue #1050"
  ]
}
```

### Example 3 — Brief (isolated docs-Pages regression)

```json
{
  "recipe": "smart-orchestrator",
  "task_description": "In target_repo rysweet/amplihack-recipe-runner, fix the red default branch: the 'Deploy mdBook to GitHub Pages' workflow build check failed on the head commit (failing run 30187028559 / job 89753457708). Fresh regression, not chronic — prior 4 runs of that workflow succeeded; core CI and Auto Release were green on the same push, so blast radius is docs-pages-only. Smallest surface: the .github/workflows/ mdBook/GitHub-Pages deploy workflow and the book source it builds (book.toml, docs/ or book/ SUMMARY.md and referenced markdown) — inspect the failing job log for the actual build error (missing/renamed page in SUMMARY.md, broken intra-book link, mdBook version pin drift, or preprocessor change) and correct the root cause. Additive / non-breaking: restore the docs deploy without altering unrelated CI; preserve the PRD; no Bridge naming; no stray print!/println! (tracing + OTel only).",
  "target_repo": "rysweet/amplihack-recipe-runner",
  "is_mechanical_sweep": false,
  "sequence_group": null,
  "success_criteria": [
    "CI green on all required checks",
    "'Deploy mdBook to GitHub Pages' workflow build check passes on main head",
    "additive / non-breaking; only docs-pages deploy restored, unrelated CI untouched; PRD preserved",
    "no Bridge naming; no stray print!/println! in new code (tracing + OTel only)",
    "docs/link updates included; quality-audit cycles pass"
  ]
}
```

---

## Full dispatch array (worked end-to-end)

The three examples above compose into one ordered array (P1 escalation →
P3 cross-cutting brief → P2 isolated brief):

```json
[
  { "recipe": null, "escalate": "…Problem 1 bootstrap deadlock…" },
  { "recipe": "smart-orchestrator", "target_repo": "rysweet/amplihack-rs", "task_description": "…merge queue fix…", "is_mechanical_sweep": false, "sequence_group": null, "success_criteria": ["…"] },
  { "recipe": "smart-orchestrator", "target_repo": "rysweet/amplihack-recipe-runner", "task_description": "…mdBook Pages fix…", "is_mechanical_sweep": false, "sequence_group": null, "success_criteria": ["…"] }
]
```

---

## Validation rules

A dispatch array is valid iff **all** of the following hold:

1. Top-level value is a **non-empty** JSON array.
2. Every element is either a **brief** (non-null `recipe`, all four
   canonical required fields present — `recipe`, `task_description`,
   `target_repo`, `success_criteria` — and no `escalate` field) or an
   **escalation** (`recipe` is `null`, `escalate` is a non-empty string).
   Optional producer hints (`is_mechanical_sweep`, `sequence_group`) may be
   present or absent and are not required for validity.
3. Every brief's `target_repo` is a well-formed `owner/name`.
4. Every brief's `success_criteria` is a non-empty array of non-empty strings.
5. Elements are ordered by blast radius, most-critical first.
6. No element corresponds to a `dropped_as_in_flight` input problem.
7. The array contains no credential-shaped secrets — a heuristic check for
   token shapes (e.g. `ghp_`, `AKIA`, PEM blocks, inline bearer tokens). PII is
   **producer-trust**: because dispatch JSON is plaintext and human-readable,
   producers must not emit personal data, but this is not (and cannot be)
   machine-enforced by the contract.

Consumers should reject any array that fails these checks before acting on it.

Rules 1–4 are **self-contained** — checkable from the array alone. Rule 7's
credential-shape heuristic is likewise self-contained; its PII clause is
producer-trust rather than machine-checked. Rules 5 (blast-radius ordering) and
6 (`dropped_as_in_flight` exclusion) are **source-relative**: they can only be
fully verified against the originating `observed_problems.ctx`, since
severity/blast-radius and in-flight status are properties of the input records,
not the emitted items. A consumer without the source `.ctx` can enforce 1–4 and
the rule 7 credential-shape heuristic but must trust the producer for 5–6 and
for PII.
