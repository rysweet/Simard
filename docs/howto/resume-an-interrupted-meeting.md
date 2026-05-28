# How to resume an interrupted meeting

Simard automatically checkpoints the meeting session after every
`/decision`, `/action`, `/question`, `/owner`, `/goal`, and `/theme`
command. If the REPL exits unexpectedly (crash, terminal close, SSH
disconnect), the checkpoint is preserved on disk and can be resumed.

## Resume a meeting

```bash
simard meeting resume
```

Simard loads the last WIP checkpoint, prints a summary of recovered
decisions/actions/questions, and re-enters the interactive REPL with the
original topic. The REPL continues checkpointing as before.

> **Note:** The WIP file is automatically removed when a meeting closes
> normally via `/close`. You only need `resume` after an abnormal exit.

## Discard a stale checkpoint

If the saved state is outdated or unwanted:

```bash
simard meeting resume --discard
```

This removes the WIP file without starting a REPL. Idempotent — safe to
run even when no checkpoint exists.

## Where checkpoints are stored

The WIP file is `meeting_session_wip.json` inside the handoff directory
(default `~/.simard/meeting_handoffs/`, overridable via
`SIMARD_HANDOFF_DIR` or `SIMARD_STATE_ROOT`).

## Limitations

- Only the structured state (decisions, action items, questions, themes,
  owner, goal) is persisted. Free-form conversation history is **not**
  included in the checkpoint — the LLM context is lost on crash.
- Only one WIP checkpoint exists at a time per handoff directory.
  Starting a new meeting overwrites any existing checkpoint once a slash
  command is issued.
