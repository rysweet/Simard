---
title: Recover from a meeting close timeout
description: What to do when `simard meeting`'s `/close` writes a partial handoff because an agent or bridge exceeded its budget — how to detect, inspect, complete, and re-ingest the bundle.
last_updated: 2026-05-19
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/meeting-close-lifecycle.md
  - ../reference/state-root-resolution.md
  - ../operations/meeting-handoffs.md
  - ./inspect-meeting-records.md
---

# Recover from a meeting close timeout

`simard meeting`'s `/close` finalizes the session within a bounded
budget (default 60s; see [Meeting close
lifecycle](../reference/meeting-close-lifecycle.md)). When an LLM
stream, summarizer, or cognitive-memory bridge subprocess exceeds
its inner timeout, the close still **writes a deserialize-valid
handoff bundle to disk** and prints the bundle paths, but the bundle
is marked **partial** through tracing. This page is the operator
playbook for that case.

> If you are looking for the design contract instead of the
> playbook, read
> [Meeting close lifecycle](../reference/meeting-close-lifecycle.md).

---

## When this applies

You ran `/close` and:

- The REPL printed bundle paths and exited within ~77 seconds
  (master 60s + agent inner 15s + subprocess grace ~2s; see
  [Meeting close lifecycle](../reference/meeting-close-lifecycle.md#why-two-nested-timeouts))
  rather than hanging indefinitely or for many minutes, **and**
- Either the REPL banner or the journal contains
  `handoff_partial=true`, **and/or**
- The handoff JSON has empty `decisions`/`action_items` despite a
  productive conversation.

If `/close` took longer than 90 seconds, that is a bug — please
file an issue and attach
`journalctl --since "5 min ago" | grep meeting`. The contract
guarantees a return within the 77s ceiling.

---

## 1. Detect the partial close

### From the REPL output

The REPL prints a one-line banner immediately after `/close` returns.
The exact format is part of the
[REPL exit-banner contract](../reference/meeting-close-lifecycle.md#repl-exit-banner):

```
[meeting] handoff written: /home/azureuser/.simard/meeting_handoffs/meeting_handoff.json
[meeting] bundle:          /home/azureuser/.simard/meetings/2026-05-19T17-31-42Z-daily-backup-policy/
[meeting] WARNING: partial close (reason=summary_empty). Review the bundle
          before relying on extracted decisions/action items.
```

The `WARNING:` line is only emitted on a partial close. The literal
prefix `[meeting] WARNING: partial close (reason=` is stable and
parsable. `<reason>` is one of the snake_case `PartialReason` wire
values (`close_timeout`, `agent_close_timeout`, `bridge_timeout`,
`summary_empty`, `persistence_error`).

### From the journal

If the meeting ran under `simard ooda daemon` or any other
journal-attached invocation:

```bash
journalctl --since "5 min ago" \
  | grep -E 'handoff_partial=true|meeting.close.(start|phase|done)'
```

Example output for a summarizer timeout:

```
INFO  meeting.close.start meeting_id=2026-05-19T17-31-42Z-daily-backup-policy budget_secs=60
DEBUG meeting.close.phase phase=agent_close ms=812 outcome=ok
WARN  meeting.close.phase phase=summary ms=15001 outcome=timeout
WARN  handoff_partial=true reason=summary_empty meeting_id=2026-05-19T17-31-42Z-daily-backup-policy
INFO  meeting.close.done  meeting_id=2026-05-19T17-31-42Z-daily-backup-policy partial=true total_ms=15904
```

### From the handoff on disk

The on-disk schema is unchanged; partial bundles are not flagged
inside the JSON itself. The signal is the **emptiness of the
extracted fields**. Resolve the handoff path from the same env-var
ladder the backend uses:

```bash
# Same resolution ladder as MeetingBackend::close():
#   SIMARD_HANDOFF_DIR > SIMARD_STATE_ROOT/meeting_handoffs > ~/.simard/meeting_handoffs
HANDOFF_DIR="${SIMARD_HANDOFF_DIR:-${SIMARD_STATE_ROOT:-$HOME/.simard}/meeting_handoffs}"
HANDOFF="$HANDOFF_DIR/meeting_handoff.json"

jq '{decisions: (.decisions|length), actions: (.action_items|length), questions: (.open_questions|length)}' "$HANDOFF"
```

A reading of `{decisions: 0, actions: 0, questions: 0}` from a
conversation that clearly produced decisions is the disk-only
heuristic.

> The same `SIMARD_HANDOFF_DIR > SIMARD_STATE_ROOT > $HOME/.simard`
> ladder is documented in
> [State-root resolution](../reference/state-root-resolution.md).
> If a `simard debug state-root` subcommand is available in your
> build it prints the resolved paths so you can paste them directly.

---

## 2. Inspect what was preserved

Even on a partial close, the **full live transcript** is preserved
in `transcript.json` because it lives in the REPL's in-memory buffer
(not behind any agent stream). Inspect it before deciding what to
do:

```bash
BUNDLE=/home/azureuser/.simard/meetings/2026-05-19T17-31-42Z-daily-backup-policy

# Quick scan
jq '.[] | "\(.role): \(.content[0:120])"' "$BUNDLE/transcript.json" | head -40

# Full markdown view
less "$BUNDLE/meeting_handoff.md"
```

You almost always have everything the operator and Simard actually
said; only the LLM-derived structured extraction is missing or
incomplete.

---

## 3. Decide your recovery path

Pick the lightest workable path:

| Symptom | Recovery |
|---|---|
| Partial close, but the transcript clearly contains 1–3 decisions/actions you can transcribe by hand | **Path A**: hand-edit the JSON (cheapest) |
| Partial close, you just want to drop the bundle and start over | **Path B**: delete and re-run |
| Partial close, you want to keep the bundle on disk for audit but stop ingesters from picking it up | **Path C**: mark processed in place |

> A future `simard meeting replay` subcommand (re-extract a fresh
> handoff from a captured `transcript.json` without rerunning the
> live LLM round trip) is **deferred**; track it in the follow-up
> issue linked from
> [Meeting close lifecycle](../reference/meeting-close-lifecycle.md).
> For now, prefer Path A for short conversations and Path B for long
> ones you would rather have Simard re-discuss.

### Path A — Hand-edit the JSON

The handoff is plain JSON. The schema is documented in
[Meeting REPL & handoff ingestion](../operations/meeting-handoffs.md#handoff-json-schema).
Add `decisions[]` / `action_items[]` entries by hand:

```bash
HANDOFF_DIR="${SIMARD_HANDOFF_DIR:-${SIMARD_STATE_ROOT:-$HOME/.simard}/meeting_handoffs}"
HANDOFF="$HANDOFF_DIR/meeting_handoff.json"

# Edit in place
${EDITOR:-vi} "$HANDOFF"
```

When you save, the next OODA cycle or engineer-loop iteration will
pick the file up exactly as if it had been written full. Ensure
`processed: false` is still set so the ingester does not skip it.

### Path B — Delete and re-run

If the conversation was short and not worth re-extracting, mark the
partial bundle processed (so any racing OODA cycle skips it),
delete the bundle, and run the meeting again:

```bash
BUNDLE=/home/azureuser/.simard/meetings/2026-05-19T17-31-42Z-daily-backup-policy
HANDOFF_DIR="${SIMARD_HANDOFF_DIR:-${SIMARD_STATE_ROOT:-$HOME/.simard}/meeting_handoffs}"
HANDOFF="$HANDOFF_DIR/meeting_handoff.json"

# Atomically flip processed=true so ingesters skip the partial
tmp="$HANDOFF.tmp.$$"
jq '.processed = true' "$HANDOFF" > "$tmp" && mv "$tmp" "$HANDOFF"

# Remove the bundle directory
rm -rf "$BUNDLE"

# Re-run
simard meeting repl daily backup policy
```

### Path C — Just suppress the partial from ingestion

If you want to keep the partial bundle on disk for audit but prevent
the OODA daemon and engineer-loop from ingesting empty
decisions/actions, flip `processed=true` in place via the same
atomic edit used in Path B:

```bash
HANDOFF_DIR="${SIMARD_HANDOFF_DIR:-${SIMARD_STATE_ROOT:-$HOME/.simard}/meeting_handoffs}"
HANDOFF="$HANDOFF_DIR/meeting_handoff.json"

tmp="$HANDOFF.tmp.$$"
jq '.processed = true' "$HANDOFF" > "$tmp" && mv "$tmp" "$HANDOFF"
```

The file is preserved but silently skipped by all ingesters (they
key off the `processed` flag through `mark_meeting_handoff_processed`).

---

## 4. Verify the recovery

After Path A, confirm the rebuilt handoff is non-empty and was
ingested on the next cycle:

```bash
HANDOFF_DIR="${SIMARD_HANDOFF_DIR:-${SIMARD_STATE_ROOT:-$HOME/.simard}/meeting_handoffs}"
HANDOFF="$HANDOFF_DIR/meeting_handoff.json"

# Non-empty extraction
jq '{decisions: (.decisions|length), actions: (.action_items|length)}' "$HANDOFF"

# Ingestion confirmation
journalctl -u simard-ooda --since "2 min ago" | grep "ingested .* meeting handoff"
```

After Path B, the freshly written handoff from the re-run is the
one that gets ingested. After Path C, no further ingestion happens
for this handoff — that's the point.

---

## 5. Reduce repeat occurrences

Partial closes are not free signal — every recurrence costs operator
time. After recovery, look at the `reason=` field from the journal
line and address the underlying cause:

| `reason=` | Likely cause | Mitigation |
|---|---|---|
| `agent_close_timeout` | A child subprocess (Copilot SDK, local-harness PTY) is slow to shut down | Check `journalctl --since "1h ago" -p warning` for subprocess-shutdown patterns; the default is now 45s (raised from 15s in #1999) — consider bumping `SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS` to 90 if your environment routinely shows >45s shutdowns |
| `summary_empty` | The summarizer LLM call did not return in time, or returned without structured fields | Raise `SIMARD_MEETING_CLOSE_TIMEOUT_SECS` to 120 if the provider is consistently slow; or switch base-type via `--base-type` (see [base-type adapters](../reference/base-type-adapters.md)) |
| `bridge_timeout` | The cognitive-memory bridge subprocess is stalled or dead | Check `journalctl -u simard-ooda --since "1h ago" \| grep bridge`; restart the daemon if the bridge process is hung |
| `close_timeout` | A phase outside the named inner budgets exceeded the master | File a bug — the master should rarely fire if every inner budget is healthy |
| `persistence_error` | IO error writing the bundle (disk full, permission denied) | Resolve the underlying disk/permission issue; rerun the meeting |

To raise the master budget for a single invocation:

```bash
SIMARD_MEETING_CLOSE_TIMEOUT_SECS=120 simard meeting repl daily backup policy
```

To raise it persistently for the daemon, set it in the systemd unit
or your operator profile.

---

## See also

- [Meeting close lifecycle](../reference/meeting-close-lifecycle.md)
  — the contract you're recovering against, including the REPL
  exit-banner format and the 77s wall-clock ceiling.
- [State-root resolution](../reference/state-root-resolution.md) —
  the `SIMARD_HANDOFF_DIR > SIMARD_STATE_ROOT > $HOME/.simard`
  ladder used by every recovery command in this page.
- [Meeting REPL & handoff ingestion](../operations/meeting-handoffs.md)
  — full operator workflow, including the on-disk handoff schema.
- [Inspect meeting records](./inspect-meeting-records.md) —
  read-only inspection commands.
