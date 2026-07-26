---
title: Reclaim disk with the simard disk tool
description: Usage guide for the agent-facing `simard disk reclaim` / `simard disk report` tool — the safety-enforcing command the disk-health recipe calls to act, so no JSON is emitted for Rust to parse. Covers dry-run, per-candidate reclamation, @file/@- input, exit-code semantics, and the guard reasons that keep live worktrees and fresh caches from being deleted.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ./configure-disk-health-check.md
  - ./configure-disk-reclamation.md
  - ../reference/simard-disk-tool.md
  - ../concepts/agentic-disk-reclamation.md
  - ./inspect-and-clean-engineer-worktrees.md
---

# Reclaim disk with the simard disk tool

`simard disk` is the **agent-facing** disk tool. The disk-health recipe calls it
to *act* — survey with `simard disk report`, reclaim a candidate with
`simard disk reclaim --path <P>` — exactly the way `distill-episodes.yaml` calls
`simard memory remember` to commit facts. The recipe **prints no JSON**: Simard
interprets the disk-health run by its **exit status alone**, so there is no
recipe-stdout envelope for Rust to scrape and no parse-fail surface.

The disk-safety heuristic is enforced **inside the tool**, not in the recipe or
the daemon. A worktree or target dir is only reclaimed when its PR is
**merged or closed** *and* it has **no active recipe/engineer session**. Every
deletion re-runs the guard immediately before removing anything.

> **This tool does NOT use `git merge-base --is-ancestor <branch> origin/main`
> as the staleness test.** A fresh worktree sitting at `origin/main` with no
> commits yet *is* an ancestor of `origin/main`, so that heuristic wrongly flags
> live worktrees whose build caches are in active use. The tool instead requires
> a positively-confirmed merged/closed PR; anything it cannot confirm is
> **kept**, not guessed.

## When to use this

Use this guide when:

- you are editing `disk-health-check.yaml` and need to know how the recipe acts,
- you want to reclaim a specific worktree or cache path by hand,
- you want to survey what is reclaimable without deleting anything,
- a path was skipped and you need to understand which safety rail refused it.

For the daemon-driven, threshold-based reclamation loop (largest-first, down to a
target `%-used`), see
[Configure and run disk reclamation](./configure-disk-reclamation.md). This tool
is the per-candidate primitive that recipe agents call; that guide covers the
whole-run orchestrator.

## The two subcommands

| Command | What it does | Deletes? |
| ------- | ------------ | -------- |
| `simard disk report --path <P>…` | Vets each path and prints a per-candidate verdict (size, allow/reject reason). Never deletes. | No |
| `simard disk reclaim --path <P>…` | Vets, then reclaims each cleared path. **Apply by default.** | Yes (guarded) |
| `simard disk reclaim --path <P>… --dry-run` | Same as `report`, but framed as a reclaim preview. | No |

Both accept one or more `--path` flags, or a batched list via `--paths @file`
/ `--paths @-` (see [Large candidate lists](#large-candidate-lists)).

**Candidate discovery is the caller's job, not the tool's.** `simard disk` vets
and acts on the paths you hand it; it does not walk the filesystem to find large
directories. The recipe agent enumerates candidates itself (e.g. `du -x -d1` under
the worktree roots), then passes each one to `report`/`reclaim`. The tool's
contract is narrow on purpose: *given a path, is it safe to reclaim, and if so,
reclaim it.*

## Survey first: `simard disk report`

`report` runs every path through the guard in dry-run and prints what a live
reclaim *would* do:

```bash
simard disk report \
  --path ~/.simard/engineer-worktrees/goal-1841-merge-flow \
  --path /home/azureuser/src/Simard/worktrees/feat-x/target \
  --path /home/azureuser/src/Simard/worktrees/main
```

```text
disk report — 3 candidates
RECLAIMABLE  tracked_worktree   ~/.simard/engineer-worktrees/goal-1841-...  3.9G  pr #1841 merged, idle
RECLAIMABLE  stale_build_cache  .../worktrees/feat-x/target                 6.1G  stale target/
SKIP         tracked_worktree   .../worktrees/main                          —     protected_path
projected: 10.0G reclaimable, 1 candidate skipped
```

`report` never deletes and always exits `0` unless it hits an operational error.

## Reclaim a candidate: `simard disk reclaim`

Unlike the hyphenated `simard disk-reclaim` (which is dry-run by default and
requires `--apply`), **`simard disk reclaim` applies by default** because it is
the tool the recipe agent invokes to actually free space. Pass `--dry-run` to
preview.

```bash
# Apply by default: reclaim this path if — and only if — every rail clears.
simard disk reclaim --path ~/.simard/engineer-worktrees/goal-1841-merge-flow
```

```text
disk reclaim — 1 candidate
REMOVED  tracked_worktree  ~/.simard/engineer-worktrees/goal-1841-...  3.9G  (git worktree remove --force)
reclaimed 3.9G
```

Preview without deleting:

```bash
simard disk reclaim --path ~/.simard/engineer-worktrees/goal-1841-merge-flow --dry-run
```

```text
disk reclaim (dry-run) — 1 candidate
WOULD REMOVE  tracked_worktree  ~/.simard/engineer-worktrees/goal-1841-...  3.9G  pr #1841 merged, idle
```

### A skipped candidate is not a failure

If a rail refuses a path, the tool routes it to skip/human-review and **still
exits `0`**, so the recipe agent can call `reclaim` per path and keep going:

```bash
simard disk reclaim --path /home/azureuser/src/Simard/worktrees/main
```

```text
disk reclaim — 1 candidate
SKIP  tracked_worktree  .../worktrees/main  —  protected_path
nothing reclaimed (1 candidate skipped for human review)
```

```bash
echo $?   # 0 — a safe skip is a success
```

## Exit-code contract

The recipe reads this tool by exit status; scripts should too:

| Exit | Meaning |
| ---- | ------- |
| `0` | Handled — the path was reclaimed **or** safely skipped (rejected by a rail). |
| `1` | Operational failure — e.g. a path was unreadable, `du`/`git` failed, or a delete errored mid-run. |
| `2` | Refused — `reclaim` in apply mode invoked as **root** (`geteuid() == 0`). Running as root would nullify the path-ownership safety model. |

Because a rail rejection maps to `0`, a recipe can call `reclaim` once per
candidate and continue on skips, while a genuine breakage (`1`) or misuse (`2`)
stops it.

## Large candidate lists

Never pass a large path list as one long argv string. For batches, use `--paths`
with a file (`@file`) or stdin (`@-`). `--paths` reads a **newline-delimited path
list** — one path per line — which is distinct from
`simard disk-reclaim exec --candidates`, whose loader expects a JSON candidate
array. Keeping the two loaders separate means a path list is never misparsed as
JSON:

```bash
# From a file — one candidate path per line (blank and #-comment lines ignored).
simard disk reclaim --paths @candidates.txt

# From stdin.
survey-script | simard disk reclaim --paths @-
```

`--paths` composes with `--dry-run`. Each path is re-vetted independently; a path
the guard refuses is skipped and reported (exit `0`) while the run continues with
the rest.

## How the recipe uses it (retcon: the act-via-tool pattern)

`disk-health-check.yaml` no longer emits `DISK_USED_PCT=` / `FREED_BYTES=` /
`ACTION:` markers for Rust to parse. Its agent step now:

1. measures disk pressure with `df`,
2. if over threshold, surveys candidates with `simard disk report`,
3. reclaims each one it wants freed with `simard disk reclaim --path <P>`,
4. **prints nothing for the daemon to parse.**

```yaml
# excerpt — disk-health-check.yaml agent prompt (illustrative)
#   Survey, then act via the tool. Print NO JSON envelope and NO key=value
#   markers. Simard interprets this recipe by EXIT STATUS alone.
#
#   simard disk report --path <candidate>            # inspect verdicts
#   simard disk reclaim --path <candidate>            # free a cleared path
#
# A path the tool SKIPs (protected, active, unknown PR state, unpushed work)
# is left for a human — do not try to force it.
```

This mirrors `prompt_assets/simard/recipes/distill-episodes.yaml`, whose writes
via `simard memory remember` *are* its output — there is no return document to
deserialize, so a stray log line can never discard the run.

## What `disk_health.rs` does now

`src/disk_health.rs` is a **thin trigger**. `run_disk_health_check`:

- decides *whether* to run using a small deterministic Rust `df` gate (this gate
  is the only Rust "decision" and it never parses recipe output),
- spawns the recipe via `recipe-runner-rs` (preserving `AMPLIHACK_AGENT_BINARY`),
- records **success or failure by the child's exit status only** — no
  `--output-format json`, no `RecipeOutput`/`StepResult` scraping, no
  `parse_disk_health_text`.

The deterministic `emergency_cleanup` hard-stop (Tier 1) is unchanged.

## Safety notes

- **The PR gate cannot be bypassed by a mislabelled `kind`.** Any path with a
  `.git` marker is vetted as a tracked worktree — the merged/closed-PR +
  no-active-session rails run even if the caller labels it `orphan_dir` or
  `stale_build_cache`.
- **Inconclusive means keep.** If the PR state cannot be positively confirmed
  merged/closed, the path is skipped as `unknown_pr_state` — never deleted.
- **No merge-base ancestry test.** Fresh `origin/main` worktrees and their live
  caches are protected precisely because ancestry is *not* used as staleness.
- **Refuses apply as root** (exit `2`); **never** passes `--admin` / `--no-verify`
  to git; **no timeouts** on the agentic recipe step.

## Related

- [The simard disk tool (reference)](../reference/simard-disk-tool.md) — CLI grammar, exit codes, guard reasons, module wiring
- [Configure and run disk reclamation](./configure-disk-reclamation.md) — the daemon threshold loop and `simard disk-reclaim`
- [Configure the disk health check](./configure-disk-health-check.md) — the per-cycle trigger and its thresholds
- [Agentic disk reclamation (concept)](../concepts/agentic-disk-reclamation.md) — "agent proposes, Rust disposes" rails
- [Inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md) — manual worktree operations
