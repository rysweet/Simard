---
title: How to clean a fixture leak from the live goal board
description: Runbook for removing synthetic test-fixture goals that leaked into ~/.simard/cognitive_memory.ladybug, and restoring lost production goals.
last_updated: 2026-05-19
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/simard-cli.md
  - ../reference/goal-board-api.md
  - ../reference/cognitive-memory-bridge-helpers.md
  - ../testing/hermetic-tests.md
  - ./unblock-stuck-ooda-goals.md
  - ./recover-goal-board.md
---

# How to clean a fixture leak from the live goal board

## Symptom

`simard goal list` shows one or more goals whose descriptions look like
placeholders — `Goal alpha`, `Goal stuck-a`, `Goal stuck-b`,
`Goal operator-blocked`, `Goal stuck-goal`, `Goal working` — instead of
the real production descriptions. The OODA daemon dispatches engineers
against these synthetic goals and the actual production work
(`enhance-simard-meeting-experience`, `improve-cognitive-memory-persistence`,
`fix-broken-features`, `drive-amplihack-rs-feature-parity`,
`improve-simard-dashboard`) is missing or partially missing from the
board.

This is the fixture-leak class of corruption tracked by
[#1923](https://github.com/rysweet/Simard/issues/1923) and
[#1925](https://github.com/rysweet/Simard/issues/1925). It is the
**inverse** of [#1915](https://github.com/rysweet/Simard/issues/1915):
that bug was about goals *vanishing* under concurrent writes; this one
is about test fixtures *appearing* in the live cognitive memory after
a `cargo test` run. The root cause is a daemon-versus-test socket-path
collision plus tests that did not set `SIMARD_STATE_ROOT` to a hermetic
`TempDir` — both fixed in the PR that closes #1923 and #1925.

If you are seeing this symptom on a Simard build that includes that PR,
the leak is from older test runs that wrote into the live DB before the
fix landed. The board does not heal automatically because the OODA
daemon never *removes* goals from the board — only the operator does,
via the commands below.

## Diagnosis

```bash
simard goal list
```

A fixture-leak board looks like this:

```text
active goals: 5 / 5
ID                PRIORITY  STATUS                                ASSIGNED                     DESCRIPTION
alpha             p1        not-started                           -                            Goal alpha
operator-blocked  p1        blocked: waiting on human review      -                            Goal operator-blocked
stuck-a           p1        in-progress(5%)                       engineer-stuck-a-1779154672  Goal stuck-a
stuck-b           p1        in-progress(5%)                       engineer-stuck-b-1779154698  Goal stuck-b
stuck-goal        p1        in-progress(5%)                       engineer-stuck-goal-…        Goal stuck-goal
backlog: 0 item(s)
```

Confirm the leak class by inspecting any one DESCRIPTION column: the
production code paths never emit `Goal <id>` literals
(`rg "format!\(\"Goal \{" src/` returns only `#[cfg(test)]` matches),
so any active goal whose description matches `^Goal <id>$` for that
goal's `id` is a fixture.

## Remediation

The remediation is two phases — sweep the synthetic goals, then restore
the production goals — both routed through the new operator surface so
the OODA daemon stays running throughout.

### Phase 1: sweep the fixture goals

For an unknown id vector, prefer the placeholder-pattern sweep:

```bash
simard goal cleanup --placeholders
```

This routes through the daemon's IPC writer
([tier 1 of `launch_writer_bridge`](../reference/cognitive-memory-bridge-helpers.md#launch_writer_bridge)),
computes the id list from the freshly-read board, and persists via
[`save_goal_board_with_removals`](../reference/goal-board-api.md#save_goal_board_with_removals).
That filter runs *after* the merge-on-write step, so the removed ids
cannot be resurrected from the persisted snapshot — the precise failure
mode that defeated PR #1926's earlier `goal delete` attempt.

For a known id vector — for example when triaging a specific leaked
fixture you've already named — use the precise removal form, variadic:

```bash
simard goal remove stuck-a stuck-b operator-blocked stuck-goal alpha
```

Both commands are idempotent. Rerunning either with the leak already
cleared exits zero with `removed=0` in the stderr summary.

Verify with `simard goal list`:

```text
active goals: 0 / 5
  (none)
backlog: 0 item(s)
```

### Phase 2: restore the production goals

The 5 production goals listed in the symptom section can be restored
through whichever curation surface is shipped on your build — the
meeting REPL's `goal-curation` flow, the dashboard goals API, or, for
the deliverable that closes #1923/#1925, the one-shot restoration that
the PR ships as a Rust integration test fixture in
`src/operator_cli/tests_restore_production_goals.rs`. The fixed
production goal vector for #1923/#1925 is:

| id                                       | priority | description                                                                                                |
|------------------------------------------|----------|------------------------------------------------------------------------------------------------------------|
| `enhance-simard-meeting-experience`      | p1       | Enhance Simard meeting experience — richer handoffs, durable state, no silent loss                          |
| `improve-cognitive-memory-persistence`   | p1       | Improve cognitive memory persistence — recovery ladder, schema versioning, hermetic tests                   |
| `fix-broken-features`                    | p1       | Fix broken features identified in fix-broken-features audit (#1907, #1909, #1910)                           |
| `drive-amplihack-rs-feature-parity`      | p1       | Drive amplihack-rs feature parity per the parity inventory (#1897-#1901)                                    |
| `improve-simard-dashboard`               | p2       | Improve Simard dashboard — surface merge-judge and per-PR readiness (#1880, #1893, #1894)                   |

Whichever surface you use to restore them, all writers funnel through
`save_goal_board` (or its `_with_removals` sibling), so the
merge-on-write guarantees of [#1915](https://github.com/rysweet/Simard/issues/1915)
still apply: concurrent operator and daemon writes to disjoint goal-id
subsets will not lose either side's work.

Final verification on a healthy board:

```text
active goals: 5 / 5
ID                                       PRIORITY  STATUS       ASSIGNED  DESCRIPTION
enhance-simard-meeting-experience        p1        not-started  -         Enhance Simard meeting experience …
improve-cognitive-memory-persistence     p1        not-started  -         Improve cognitive memory persistence …
fix-broken-features                      p1        not-started  -         Fix broken features identified in fix-broken-features audit …
drive-amplihack-rs-feature-parity        p1        not-started  -         Drive amplihack-rs feature parity per the parity inventory …
improve-simard-dashboard                 p2        not-started  -         Improve Simard dashboard — surface merge-judge and per-PR readiness …
backlog: 0 item(s)
```

The next OODA cycle picks up the restored board automatically — no
daemon restart required. The merge-on-write step on the daemon's
end-of-cycle save will preserve the operator-written board because the
in-flight active set is empty for the relevant ids (the daemon's
in-memory copy still holds the cleaned snapshot from before the
restoration), so persisted-only ids are kept verbatim.

## What to do if the leak comes back

Do not just rerun `goal cleanup --placeholders` in a loop — that masks
a regression. The fix that closes #1923/#1925 has two regression
guards:

- The socket-path resolver now lives next to the state root (see
  [Shared socket-path contract](../reference/simard-cli.md#shared-socket-path-contract)),
  so a hermetic test with `SIMARD_STATE_ROOT=$TMPDIR/…` connects to
  its own daemon, not yours.
- Every test that exercises `save_goal_board` asserts that the
  resolved state root is under `env::temp_dir()` and is not the
  operator's home — see
  [Hermetic-test guard](../testing/hermetic-tests.md).

If a placeholder goal re-appears after cleanup on a fixed build, you
have found a regression — file a new issue with the timing data
(when the placeholder first appeared in `simard goal list`, the
sequence of `cargo test` / dev-loop commands that ran just before,
and any non-default values of `SIMARD_STATE_ROOT` /
`SIMARD_MEMORY_SOCKET` in the shell). Tag the issue with
`fixture-leak` and link both #1923 and #1925.

## Related runbooks

- [Unblock OODA goals stuck after a brain-failure lockout](./unblock-stuck-ooda-goals.md)
  — for `Blocked` goals where the brain-failure marker is the cause
  (a different remediation — `simard goal unblock-all`).
- [How to recover a corrupted or missing goal board](./recover-goal-board.md)
  — for the broader corruption surface (missing `goal-board:snapshot`
  fact, deserialization failure, mid-write crashes).
- [Goal board API reference](../reference/goal-board-api.md) — the
  persistence semantics that make this remediation safe.
- [Hermetic-test guard](../testing/hermetic-tests.md) — what test
  authors must do to prevent this regression class.
