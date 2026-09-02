---
title: "No point-in-time report docs — pr-verify scan"
description: >
  Reference for scan_no_point_in_time_report_docs, the Overseer pr-verify diff
  scan that enforces the durable-documentation policy (G4). It flags a PR that
  ADDS a new point-in-time investigation/testing/diagnosis report doc, while
  never flagging durable feature/architecture docs or edits to pre-existing
  docs. A pure function over `gh pr diff` output, registered as pr-verify check
  #8 in run_diff_scans, so it auto-enforces at the merge gate with no
  --admin/--no-verify path.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/durable-documentation-policy.md
  - ../howto/record-an-investigation-finding.md
  - ../design/overseer.md
  - ./cross-repo-merge-authority.md
  - ./pr-finalization-pipeline.md
---

# No point-in-time report docs — pr-verify scan

`scan_no_point_in_time_report_docs` is the **deterministic backstop** for the
[durable-documentation policy (G4)](../concepts/durable-documentation-policy.md).
It is one of the Overseer's pr-verify safety diff-scans in
[`src/overseer/pr_verify.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/pr_verify.rs),
added as **check #8** alongside the existing no-`Bridge`-naming, no-stray-`print!`,
additive-only, and PRD-preserved scans.

Like every scan in that module it is a **pure function over a unified diff**
(`gh pr diff` output), so the whole merge-safety surface stays testable on fixture
diffs with zero network. It flags a PR that **adds a new point-in-time report doc**
(investigation / testing / diagnosis / recurrence / benchmark-snapshot report) and
returns a finding pointing the author at the right home for that content — a
GitHub issue and/or memory. It **never** flags durable feature/architecture docs,
and it **never** flags edits to pre-existing docs.

## Signature

```rust
use crate::overseer::pr_verify::{scan_no_point_in_time_report_docs, DiffFinding};

/// Check #8: no newly-added point-in-time report docs (policy G4). Durable
/// feature/architecture docs are fine; run-specific findings belong in a
/// GitHub issue and/or memory. Pure over a unified diff; no network.
pub fn scan_no_point_in_time_report_docs(diff: &str) -> Vec<DiffFinding>;
```

Its input is a unified diff string (the same `gh pr diff` text the other scans
read). Its output is a `Vec<DiffFinding>` — one entry per offending added report
doc. An **empty vector means the PR is clean** for this check.

### `DiffFinding`

The scan reuses the module's shared finding type — no new result type is
introduced:

```rust
pub struct DiffFinding {
    /// Path (new-side) the finding is in.
    pub file: String,
    /// Best-effort new-file line number (`None` for a whole-file / path finding).
    pub line: Option<usize>,
    /// The offending source text (trimmed) or a short policy note.
    pub text: String,
}
```

## What it flags

A file is flagged only when **all** of these hold:

1. It is a **newly added** file (the diff shows it added — old side is
   `/dev/null` / `new file mode`), **and**
2. Its path ends in `.md`, **and**
3. It matches a **report signal** — either the **path rail** or the **title
   rail** below.

### Path rail — reserved report directories

A newly added `.md` under a **reserved report directory** is flagged on the path
alone; the directory is reserved by policy for point-in-time reports:

| Reserved prefix | Meaning |
|---|---|
| `docs/investigation/` | investigation / diagnosis reports |
| `docs/reports/` | testing / status / findings reports |
| `docs/runs/` | per-run snapshot write-ups |

The finding for a path-rail hit has `line: None` and a policy note naming the
file.

### Title rail — report-typed docs anywhere else

A newly added `.md` **outside** the reserved directories is flagged only when an
**added title or front-matter line** marks it a report — an H1 (`# …`), or a
front-matter `title:` / `type:` — whose text (lowercased) contains a **report
marker**:

```
investigation report      testing report / test report
diagnosis / diagnosis report / diagnostic report
recurrence report         blockage report
benchmark snapshot        measured-rate / measured rate / snapshot findings
findings report           run summary / session report
point-in-time             postmortem
```

The match is a substring test on the lowercased title/front-matter line only —
the **same keyword-level substring approach** the sibling `scan_no_bridge_naming`
uses. Report vocabulary in ordinary **body prose** of an otherwise durable doc is
**not** a title-rail hit, so a durable reference that merely discusses an
"investigation" is never flagged. A title-rail finding carries the offending
line's new-side line number.

## What it does NOT flag

The scan is deliberately narrow so it can never disturb existing or durable docs:

- **Edits to pre-existing docs.** Only newly added files qualify (criterion 1).
  An update to an existing `docs/architecture/x.md` that adds report-like words is
  **not** flagged. This is what lets G4 land without touching any current doc, and
  what stops the enforcement PR — which only *edits* `CONTRIBUTING.md` and prompt
  `.md` files — from flagging itself.
- **Durable feature/architecture docs.** Newly added docs under
  `docs/architecture/`, `docs/design/`, `docs/reference/`, `docs/howto/`,
  `docs/concepts/`, plus `README` / `CONTRIBUTING` / `CHANGELOG` and
  `prompt_assets/**`, are never flagged (they are not report paths and do not
  carry a report-typed title).
- **Non-markdown files.** Added `.rs`, `.yaml`, fixtures, tests, etc. are ignored
  — the path test is `path.ends_with(".md")`, so only `.md` files are in scope
  (a `.markdown` extension is not matched).
- **Deletions and removed lines.** Removing a doc is never a G4 violation.

## Newly-added-file detection

The module's shared `for_each_diff_line` walker intentionally discards the
old-side (`--- `) header, so it cannot by itself tell an *added* file from an
*edit*. The scan therefore uses a dedicated pre-pass that reads the raw diff for
the git extended `new file mode` header and/or the `--- /dev/null` old-side, and
collects the new-side paths a diff **adds**:

```rust
/// New-side paths this diff ADDS (old side is /dev/null or `new file mode`).
/// A separate pass from `for_each_diff_line`, so the existing scans are
/// byte-for-byte unchanged (sibling isolation).
fn newly_added_files(diff: &str) -> BTreeSet<String>;
```

Both the `new file mode` header **and** the `--- /dev/null` old-side are accepted:
`gh pr diff` emits the git extended header, while some tools/fixtures emit only
the `/dev/null` old side. Either alone proves the file is an add.

## Registration and enforcement

The scan is registered in `run_diff_scans` next to the existing checks:

```rust
// in run_diff_scans, alongside checks 3–6:
finding_check(
    "no point-in-time report docs (G4)",
    scan_no_point_in_time_report_docs(diff),
),
```

Because `run_diff_scans` is already wired into the Overseer merge gate
(`merge_ops.rs` → the merge-safety scan set), registering the scan **auto-enforces**
it — no change to `merge_ops.rs` is required. A finding produces a normal failing
`CheckItem` (`ready: false`), exactly like the sibling scans, which **blocks the
merge**. There is **no `--admin` / `--no-verify` bypass**: a flagged PR does not
merge until the report doc is removed and its content moved to an issue/memory.

This is the same layering the [pr-verify checklist](../design/overseer.md) and
[cross-repo merge authority](./cross-repo-merge-authority.md) describe: the scan
**adds** a gate on top of the objective gates (CI-green, mergeable, base
allowlist) and the merge-judge — it never replaces them.

## Finding text and author guidance

Each finding names the offending file and states where the content should go
instead, so the author (human or Simard's loop) gets an actionable next step
rather than a bare rejection. For example:

```
docs/investigation/kgpacks-blockage.md — point-in-time report doc
(no-point-in-time-docs / G4). Record findings in a GitHub issue and/or memory;
do not commit investigation/testing/diagnosis reports to the repo. Durable
feature/architecture docs are welcome — see docs/concepts/durable-documentation-policy.md.
```

## Invariants

- **Pure and network-free.** `scan_* : &str -> Vec<DiffFinding>`; fixture-testable
  with no `gh` call, identical to the sibling scans.
- **Added-only.** Only files the diff **adds** are eligible; edits to
  pre-existing docs are never flagged.
- **Report-typed, not topic-typed.** A durable doc about the same topic as a
  report is never flagged; only reserved report paths or report-typed titles trip
  the scan.
- **Sibling isolation.** `newly_added_files` is a separate pass;
  `for_each_diff_line` and the four pre-existing scans are unchanged, so their
  findings on any diff are exactly as before.
- **Forward-only.** `docs/investigation/` etc. hold nothing today, so the scan has
  **zero retroactive false positives** against the current repo.
- **Fails the gate, never bypasses it.** A finding sets `ready: false` and blocks
  the merge; the policy is enforced without `--admin`/`--no-verify`.
- **Applies to everyone.** The gate is diff-shaped, so it treats a human PR and
  Simard's own OODA-loop PR identically.

## Testing

The scan ships with unit tests over fixture diffs in the module's `#[cfg(test)]`
block:

```bash
cargo test -p simard overseer::pr_verify
```

The fixtures assert, at minimum:

1. Added `docs/investigation/run-42.md` → **1 finding** (path rail).
2. Added `docs/design/kgpacks-parity.md` (durable) containing "investigation" in
   its body → **0 findings** (durable path, body-only vocabulary).
3. Added `docs/notes/x.md` with an H1 `# Investigation Report` → **1 finding**
   (title rail).
4. An **edit** to a pre-existing `docs/investigation/old.md` (not newly added) →
   **0 findings** (added-only).
5. `newly_added_files` distinguishes a `new file mode` add, a plain edit, and a
   `/dev/null` deletion.
6. **Sibling regression:** a diff that also adds `Bridge` naming or a `print!`
   still yields exactly the pre-existing sibling findings (isolation holds).

The G4 prompt/keyword invariants (and prompt↔recipe parity) are pinned separately
in `tests/engineering_guidelines_prompts.rs`, next to the G1/G2/G3 invariants:

```bash
cargo test --test engineering_guidelines_prompts
```

## See also

- [Durable-Documentation Policy (G4)](../concepts/durable-documentation-policy.md) — the *why* and the two-rail architecture.
- [How to record an investigation finding](../howto/record-an-investigation-finding.md) — what to do instead of committing a report doc, and how to respond to a flag.
- [Overseer — operator/observer co-process (design)](../design/overseer.md) — the pr-verify checklist this scan extends (adds row #8).
- [Cross-Repo Merge Authority](./cross-repo-merge-authority.md) — the gated merge pipeline the scan runs inside.
- [`CONTRIBUTING.md` § Engineering Guidelines](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#engineering-guidelines-g1g2g3g4) — the durable source of truth for G1–G4.
