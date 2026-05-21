# How to diagnose a rejected progress claim

When the progress-evidence gate rejects a brain progress update, the
rejection is recorded as a cognitive-memory episode and surfaced on the
dashboard. This guide walks through the steps to find a specific
rejection, understand why it happened, and decide what to do next.

> Background: [Progress-evidence gating](../concepts/progress-evidence-gating.md).
> API surface: [Progress-evidence API](../reference/progress-evidence-api.md).

---

## 1. Find the rejection

### From the operator dashboard

Open the dashboard's memory search and query for the stable prefix:

```
brain hallucination detected
```

Each result is one rejection. The episode body contains the goal id, the
attempted percent transition, the cutoff timestamp, and the checker's
reason string. Example:

```
brain hallucination detected: rejected progress 35%→75% on enhance-simard-meeting-experience
  — no git evidence since last update: no commits on engineer/enhance-simard-meeting-experience-*,
    no PRs referencing goal, no merged PRs closing #1951 since 2026-05-19T23:06:48Z
```

### From the command line

The dashboard's `/api/memory/search` endpoint accepts a `POST` with a JSON
body whose `query` field is matched case-insensitively against memory
records:

```bash
curl -s -X POST http://localhost:8080/api/memory/search \
  -H 'Content-Type: application/json' \
  -d '{"query":"brain hallucination detected"}' \
  | jq -r '.results[] | "\(.source)\t\(.data | tostring)"'
```

To scope to one goal, include the goal id in the query string (the
endpoint does a substring match against the serialized record):

```bash
GOAL=improve-simard-dashboard
curl -s -X POST http://localhost:8080/api/memory/search \
  -H 'Content-Type: application/json' \
  -d "{\"query\":\"brain hallucination detected $GOAL\"}" \
  | jq -r '.results[].data'
```

### From the daemon log

The cognitive-memory store mirrors writes to stderr, so `journalctl`
also contains the rejections:

```bash
journalctl --user -u simard-ooda -n 5000 | grep 'brain hallucination detected'
```

---

## 2. Read the rejection reason

The reason string follows a fixed template. Each comma-separated clause
corresponds to one of the three evidence rules:

| Clause | Means | Rule |
|---|---|---|
| `no commits on engineer/<slug>-*` | No local branch matching the engineer pattern has a commit at or after `since`. | (1) Local commit |
| `no PRs referencing goal` | No PR on `rysweet/Simard` (any state, any age within the search window) mentions the goal slug or any `wip_refs` id. | (2) PR cross-reference |
| `no merged PRs closing #<issue-list>` | No PR was merged at or after `since` whose body contains `Closes/Fixes/Resolves #N` for any `N` in the goal's `wip_refs`. | (3) Merged-PR closer |
| `since <iso8601>` | The cutoff used. Anything older than this is ignored. | — |

If only **one** of the three clauses appears, the others were
short-circuited by a different match. If all three appear, every rule
failed.

---

## 3. Decide which case you are in

### Case A: The brain hallucinated, the gate worked correctly

- The engineer subprocess produced no branches.
- No PR exists for the goal.
- No merged PR closes a `wip_refs` issue.
- The dashboard percent stayed at its prior value.

**Action:** None on the gate. This is the intended behavior. If you want
to drive actual progress, file a goal-curation task or use the meeting
REPL to set a concrete next step — see
[Start a meeting](start-a-meeting.md) and
[Carry meeting decisions into engineer sessions](carry-meeting-decisions-into-engineer-sessions.md).

### Case B: Real work happened but on the wrong branch

`git branch --list 'engineer/*'` shows no branch for the goal, but you
know an engineer made commits on a differently-named branch.

**Action:** This is an engineer-spawn bug, not a gate bug. The engineer
should be writing to `engineer/<goal-slug>-<timestamp>` (see
[Spawn engineers from OODA daemon](spawn-engineers-from-ooda-daemon.md)).
File an issue against the spawn path and reference the rejection
episode.

### Case C: A PR exists but the gate did not find it

```bash
gh pr list --repo rysweet/Simard --search '<goal-slug or issue#>' --state all \
  --json number,title,body,state,createdAt,mergedAt | jq .
```

If the PR exists but `gh pr list` returns nothing, one of:

- `gh` is unauthenticated on the daemon host (`gh auth status`).
- The PR's title and body do **not** mention the goal slug or any
  `wip_refs` id. The gate cannot find what is not referenced.
- The PR's `createdAt` is before `since`. This is intended — once a
  goal's last-update timestamp moves forward, older PRs no longer count
  as fresh evidence. Either the goal needs a new PR or its
  `last_progress_update_at` was reset spuriously.

**Action:** If `gh` is broken, fix it and the next cycle will Accept. If
the PR is unreferenced, edit its body to mention the goal id or relevant
issue number — this is also good practice for human reviewers.

### Case D: The `since` timestamp looks wrong

Compute the gate's expected `since` for the goal. The new `simard goal show`
subcommand (added by #1967, see design §2.8) exposes the
`last_progress_update_at` field directly:

```bash
# Inspect the goal record (--json prints the full ActiveGoal as JSON).
simard goal show --id <goal-id> --json | jq .last_progress_update_at
```

If that prints `null`, the gate falls back to a memory scan for the most
recent `"goal progress accepted:"` episode for the goal:

```bash
GOAL=<goal-id>
curl -s -X POST http://localhost:8080/api/memory/search \
  -H 'Content-Type: application/json' \
  -d "{\"query\":\"goal progress accepted $GOAL\"}" \
  | jq -r '.results[0].data'
```

If the memory scan also returns no matches, the fallback is the daemon's
process-start time — observable from the boot-log line emitted at
daemon start (see [boot-log contract](../operations/progress-evidence-kill-switch.md#verifying-which-mode-the-daemon-is-running-in)):

```bash
journalctl --user -u simard-ooda --since 'today' | grep 'progress-evidence:' | head -1
```

If `last_progress_update_at` is set but to a wildly wrong value, file a
bug and include the goal id, the value, and the cycle log. (If your
deployment predates the `goal show` subcommand, fall back to reading the
on-disk board snapshot under `$SIMARD_STATE_ROOT`; `simard goal list`
will print the active board without the new field.)

---

## 4. Sanity-check the gate itself

If you suspect the gate is broken (not just rejecting correctly), use
the kill switch on a **non-production** daemon to A/B test:

```bash
# Terminal 1: gate disabled, observe behavior.
SIMARD_PROGRESS_EVIDENCE=off simard daemon --port 18081

# Terminal 2: gate enabled (default).
simard daemon --port 28081
```

Feed both daemons the same goal and engineer activity. If the disabled
daemon also rejects something obvious, the bug is upstream of the gate.
If only the enabled daemon rejects, capture the cycle log and file a
bug against `src/goal_curation/progress_evidence.rs`. Do **not** leave
the kill switch on in production — see
[`SIMARD_PROGRESS_EVIDENCE`](../operations/progress-evidence-kill-switch.md).

---

## 5. After the underlying issue is fixed

The gate is stateless across daemon runs (modulo the `OnceLock`
process-start fallback). Once the missing branch, PR, or `gh` auth is
in place, the next OODA cycle that proposes the same increase will go
through the checker fresh. On `Accept`:

- The goal's `last_progress_update_at` is set to the cycle wall-clock.
- A `"goal progress accepted:"` episode is written to memory.
- The dashboard percent moves.

You do not need to clear rejection episodes manually — they remain as an
audit trail and may be consolidated into semantic memory if they recur.

---

## 6. Quick reference

```bash
# Find all rejections, newest first
curl -s -X POST http://localhost:8080/api/memory/search \
  -H 'Content-Type: application/json' \
  -d '{"query":"brain hallucination detected"}' \
  | jq -r '.results[] | "\(.source)\t\(.data | tostring | .[0:200])"'

# Find all accepts (audit positives)
curl -s -X POST http://localhost:8080/api/memory/search \
  -H 'Content-Type: application/json' \
  -d '{"query":"goal progress accepted"}' \
  | jq -r '.results[] | "\(.source)\t\(.data | tostring | .[0:200])"'

# Confirm the gate is on (drop --user for system-level installs)
journalctl --user -u simard-ooda -n 500 | grep 'progress-evidence:'

# Confirm gh works for the daemon user
gh auth status
gh pr list --repo rysweet/Simard --limit 3

# List engineer branches the gate would scan for goal G
GOAL=enhance-simard-meeting-experience
git -C ~/src/Simard for-each-ref --format='%(refname:short)' "refs/heads/engineer/${GOAL}-*"

# Inspect the last_progress_update_at field directly
simard goal show --id "$GOAL" --json | jq .last_progress_update_at
```

---

## Related

- [Progress-evidence gating (concept)](../concepts/progress-evidence-gating.md)
- [Progress-evidence API (reference)](../reference/progress-evidence-api.md)
- [`SIMARD_PROGRESS_EVIDENCE` kill switch (operations)](../operations/progress-evidence-kill-switch.md)
- [Unblock stuck OODA goals](unblock-stuck-ooda-goals.md)
- [Spawn engineers from OODA daemon](spawn-engineers-from-ooda-daemon.md)
- [Recover the goal board](recover-goal-board.md)
