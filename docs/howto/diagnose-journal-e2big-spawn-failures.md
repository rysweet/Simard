---
title: Diagnose journal E2BIG spawn failures
description: >
  Operator runbook for the live "journal full of raw error dumps" symptom: confirm
  the recipe-runner-rs spawn E2BIG (os error 7) on the journal draft/de-jargon
  passes, verify the file-channel fix is deployed (day_context_path / draft_path,
  not inline day_context), read the structured `overseer.diagnosis` telemetry for
  the recorded ArgListTooLong cause, and confirm the journal renders as the
  intended jargon-free narrative with no historical error text (#2692).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: how-to
status: implemented
related:
  - ../reference/recipe-context-file-transport.md
  - ../concepts/journal-recipe-spawn-e2big.md
  - ../reference/terminal-failure-diagnosis-api.md
  - ../howto/diagnose-and-recover-ooda-step-failures.md
  - ../howto/browse-the-simard-journal.md
  - ../reference/journal-api.md
  - ../reference/distill-recipe-output-capture.md
---

# Diagnose journal E2BIG spawn failures

Use this runbook when Simard's daily journal reads like a dump of raw error text
and engineering jargon instead of the intended plain-language narrative — the
symptom of the [journal E2BIG recipe-spawn incident](../concepts/journal-recipe-spawn-e2big.md)
(#2692). For the wire-level contract, see the
[recipe context-file transport reference](../reference/recipe-context-file-transport.md).

## Symptom

- The journal (dashboard **Journal** tab, TUI **Journal** pane, or
  `simard journal`) shows verbatim `content` from episodic memory — often
  including *old* `Argument list too long` / `exit status 126` error lines — with
  raw code identifiers and acronyms left in.
- The daemon log emits this **every hour**:

  ```
  WARN simard::journal: journal draft recipe failed; using the deterministic
  report drafter error=… recipe-runner-rs spawn failed: Argument list too long
  (os error 7)
  ```

  (`os error 7` = `E2BIG`.) A matching `journal de-jargon recipe failed; using
  the glossary reviewer` line may accompany it.

## 1. Confirm the spawn E2BIG

The daemon logs to **stderr**. Filter for the journal warning (adjust the capture
for your deployment — journald, a redirect file, etc.):

```bash
# journald
journalctl -u simard --since "2 hours ago" | grep -E "simard::journal|os error 7"

# or a redirected log file
grep -E "journal (draft|de-jargon) recipe failed|os error 7" /path/to/simard.log
```

`os error 7` on the journal draft/de-jargon pass confirms the argv overflow at the
`recipe-runner-rs` spawn. This is **distinct** from the copilot exit-126 E2BIG
covered by [#2640](../howto/diagnose-and-recover-ooda-step-failures.md): it is a
pre-exec `io::Error`, not an exit status.

## 2. Verify the fix is deployed

The fix routes the large context through a file, so `argv` carries only a
`*_path`. Confirm the running daemon has it:

- **Recipe assets read the path.** The hot-reload recipes must reference the
  `*_path` variables:

  ```bash
  grep -n "day_context_path" ~/.simard/prompt_assets/simard/recipes/journal-narrative.yaml
  grep -n "draft_path"       ~/.simard/prompt_assets/simard/recipes/journal-plain-language.yaml
  ```

  If these instead show a raw `{{day_context}}` / `{{draft}}` interpolation, the
  hot-reload assets are **stale** — re-run `scripts/redeploy-local.sh` to sync
  them (see
  [distill recipe output capture — recipe asset sync](../reference/distill-recipe-output-capture.md#recipe-asset-sync))
  and confirm the synced count.

- **The binary is current.** Confirm the deployed daemon is a build that includes
  `src/recipe_context_file.rs` (the `ContextFile` helper). A journal tick after
  the fix writes a per-invocation `simard-journal-*` tempdir; the payload is no
  longer visible on the process command line:

  ```bash
  # After the fix, the journal recipe-runner-rs argv shows a *_path, NOT a giant -c day_context=
  ps -eo pid,args | grep recipe-runner-rs | grep -o "day_context[^ ]*"
  # expect: day_context_path=/tmp/simard-journal-ctx-XXXX/day_context.ctx
  ```

## 3. Read the structured diagnosis telemetry

With the fix, a genuine spawn failure is **classified and recorded**, not
swallowed. Look for the structured `overseer.diagnosis` event carrying the
`ArgListTooLong` cause (JSON logs under `SIMARD_LOG_JSON=1`):

```bash
journalctl -u simard --since "2 hours ago" -o cat \
  | jq -c 'select(.target=="overseer.diagnosis") | .fields'
# {"cause":"arg-list-too-long","exit_code":null,"evidence":"…(os error 7)"}
```

Notes:

- `cause` is `arg-list-too-long` (the stable kebab-case `FailureCause::as_str`
  label); `exit_code` is `null` (there was no exit — it is a pre-exec failure).
  See the
  [terminal failure diagnosis API](../reference/terminal-failure-diagnosis-api.md#failurecause).
- On the next Observe pass this becomes a `ProblemKind::ProcessHealth` problem and
  a corrective `LaunchRecipe`, visible in the Overseer activity feed
  (`GET /api/overseer`, the `simard status` **OVERSEER** section, the TUI Overseer
  pane). A **healthy** post-fix daemon should record **no** new `arg_list_too_long`
  diagnoses for the journal.

## 4. Confirm a healthy journal

After the fix is deployed and the next journal tick runs:

- The draft + de-jargon recipes **succeed** — no `os error 7`, no hourly `journal
  … recipe failed` warning.
- The journal renders as the intended jargon-free narrative report
  ([#2654](../reference/journal-api.md)) — `## Overview`, `## Engineering work`,
  `## Research and findings`, `## Key observations`, `## Remembered moments` — with
  acronyms expanded and **no** raw historical E2BIG error text.
- Browse it to confirm: see
  [Browse the Simard journal](../howto/browse-the-simard-journal.md).

## If it still fails

- **Still `os error 7` with the assets updated:** confirm the deployed binary
  actually includes `ContextFile` (step 2) — a stale binary with fresh assets will
  still inline `day_context` on `argv`.
- **A different errno** (`os error 28` = `DiskFull`, `os error 12` = `OutOfMemory`)
  now appears in the diagnosis: that is a real resource problem the classifier
  surfaces correctly — free disk / memory rather than re-deploying. See
  [reclaim disk space](../howto/reclaim-disk-space-and-run-low-space-rust-builds.md).
- **The fallback drafter still runs** (loudly, now): read the recorded diagnosis
  to see *why* the agentic pass failed, then remediate that cause — the fallback is
  a last resort, not the intended steady state.
