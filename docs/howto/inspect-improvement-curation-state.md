---
title: How to inspect improvement-curation state
description: Use `simard improvement-curation read` to inspect the latest review-driven priority decisions without mutating stored state.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ../reference/runtime-contracts.md
  - ../tutorials/run-your-first-local-session.md
  - ./inspect-durable-goal-register.md
---

# How to inspect improvement-curation state

This guide shows how to use `simard improvement-curation read <base-type> <topology> [state-root]` as the read-only audit companion to `improvement-curation run`.

## Prerequisites

- [ ] You are in the repository root
- [ ] `cargo run --quiet -- ...` works locally
- [ ] You are willing to use one explicit state root for `review run`, `improvement-curation run`, and `improvement-curation read`

## 1. Pick one explicit state root

The read workflow only sees durable state stored under the selected root.

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-improvement-curation.XXXXXX)"
```

Keep that shell variable for the rest of this guide.

## 2. Persist a real review artifact first

`improvement-curation read` is intentionally not a synthetic report. It reads real persisted review state plus the later improvement decision state built on top of it.

```bash
cargo run --quiet -- \
  review run local-harness single-process \
  "inspect the current Simard review surface and preserve concrete proposals" \
  "$STATE_ROOT"
```

Look for output like this:

```text
Probe mode: review-run
Review proposals: 2
Review artifact: ...
```

## 3. Approve one proposal and defer one proposal

Curate the review findings through the public CLI before you try to read them back.

```bash
cargo run --quiet -- \
  improvement-curation run local-harness single-process \
  "$(cat <<'EOF'
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now
defer: Promote this pattern into a repeatable benchmark | rationale=hold this until the next benchmark planning pass
EOF
)" \
  "$STATE_ROOT"
```

Look for:

- `Identity: simard-improvement-curator`
- `Approved proposals: 1`
- `Deferred proposals: 1`
- `Active goals count: 1`
- `Active goal 1: p1 [active] Capture denser execution evidence`

That mutation path is still the only place where approval and deferral decisions are made. The dedicated read workflow below is audit-only.

## 4. Read the durable improvement-curation state

```bash
cargo run --quiet -- \
  improvement-curation read local-harness single-process "$STATE_ROOT"
```

Expected output shape:

```text
Probe mode: improvement-curation-read
Identity: simard-improvement-curator
Selected base type: local-harness
Topology: single-process
State root: /tmp/simard-improvement-curation.XXXXXX
Latest review artifact: /tmp/simard-improvement-curation.XXXXXX/review_artifacts/review-....json
Review id: review-...
Review target: operator-review
Review proposals: 2
Approved proposals: 1
Approved proposal 1: p1 [active] Capture denser execution evidence
Deferred proposals: 1
Deferred proposal 1: Promote this pattern into a repeatable benchmark (hold this until the next benchmark planning pass)
Active goals count: 1
Active goal 1: p1 [active] Capture denser execution evidence
Proposed goals count: 0
Proposed goals: <none>
Latest improvement record: review=review-... target=operator-review approvals=[p1 [active] Capture denser execution evidence] deferred=[Promote this pattern into a repeatable benchmark (hold this until the next benchmark planning pass)]
```

The important contract is structural:

- the command is read-only
- it reads the latest persisted review artifact from the same validated `state-root`, where "latest" means the artifact with the highest `reviewed_at_unix_ms`
- it reads the latest persisted improvement decision record from that same root, where "latest" means the last decision memory record whose key ends with `improvement-curation-record`
- sections always appear in this order: latest review metadata, approved proposals, deferred proposals, active goals, proposed goals, latest improvement record
- empty approved, deferred, active-goal, or proposed-goal sections render explicit `0` and `<none>` lines instead of disappearing
- persisted proposal titles, rationales, goal text, and decision records are sanitized before printing so stored terminal control sequences are not replayed
- when `[state-root]` is omitted, the command reuses the same canonical durable root that `review run` and `improvement-curation run` use for the validated runtime pairing

## 5. Configuration rules that matter

For predictable future improvement-state inspection, keep these rules in mind:

- pass the exact same explicit `state-root` you used for earlier `review run` and `improvement-curation run` activity when you want to inspect that stored decision state
- if you omit `[state-root]`, keep the same shipped `base-type` and `topology` pairing you used for `review run` or `improvement-curation run` so Simard resolves the same canonical durable root
- the canonical default root for the shared review/improvement state is `target/operator-probe-state/review-run/simard-engineer/<base-type>/<topology>`
- use `improvement-curation run` when you want to approve or defer proposals
- use `improvement-curation read` when you want a read-only operator summary of the latest decisions and promoted goals
- expect invalid `state-root` values, missing review artifacts, missing improvement records, unreadable storage, and malformed decision data to fail explicitly rather than silently rendering a partial report

## 6. Troubleshoot the common failure shapes

### The command fails because no persisted review artifact exists

That usually means one of these is true:

- you passed a different `STATE_ROOT` than the one used for `review run`
- the selected state root is valid but no review artifact was persisted there yet
- you expected the command to synthesize review context from goal state alone

Fix the durable state first by running `review run` against the same root.

### The command fails because no persisted improvement record exists

That means no improvement-curation decision has been written under the selected root yet, or you are looking at the wrong durable state.

Run `improvement-curation run` first, then use the read command against the same root.

### The command prints no proposed goals

That is fine when the approved proposals were promoted as `active` priorities instead of `proposed` priorities. `improvement-curation read` should still print:

- `Proposed goals count: 0`
- `Proposed goals: <none>`

### The command fails instead of printing a summary

That is the intended contract for invalid or unreadable operator state. Fix the selected `state-root` or the underlying storage rather than assuming Simard should guess.

## Related reading

- For the exact command tree and example syntax, see [Simard CLI reference](../reference/simard-cli.md).
- For the executable contract behind the read and run paths, see [Runtime contracts reference](../reference/runtime-contracts.md).
- For the wider tutorial flow that also exercises engineer, meeting, goal-curation, and bootstrap modes, see [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md).
