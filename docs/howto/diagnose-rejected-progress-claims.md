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

The reason string follows a fixed template. The LLM reviewer returns a
short rationale explaining why the progress claim was rejected:

```
brain hallucination detected: rejected progress 35%→75% on enhance-simard-meeting-experience
  — no git evidence since last update: progress-assessment-reviewer: reject — 75% claim with no plan and no WIP; likely hallucinated
```

The `progress-assessment-reviewer:` prefix identifies that the rejection
came from the LLM reviewer. The rationale after the dash is the reviewer's
one-sentence explanation.

Common rejection patterns:

| Rationale pattern | Means |
|---|---|
| Large delta with no plan/WIP | The brain claimed a big jump but the goal has no current activity or WIP references. |
| 100% claim with no shipped artifact | The brain marked the goal complete but the plan doesn't mention a shipped PR or merged change. |
| Claim contradicts plan | The plan says "blocked" or "investigating" but the brain claimed high progress. |
| No plan, big jump | The goal has no `current_activity` set and the percent jumped significantly. |

---

## 3. Decide which case you are in

### Case A: The brain hallucinated, the gate worked correctly

- The goal has no current plan or WIP references.
- The brain claimed a large delta despite no described work.
- The dashboard percent stayed at its prior value.

**Action:** None on the gate. This is the intended behavior. If you want
to drive actual progress, file a goal-curation task or use the meeting
REPL to set a concrete next step — see
[Start a meeting](start-a-meeting.md) and
[Carry meeting decisions into engineer sessions](carry-meeting-decisions-into-engineer-sessions.md).

### Case B: The plan is set but the reviewer misjudged

The goal has a meaningful `current_activity` and/or WIP references, but
the reviewer still rejected the claim. This could mean:

- The plan text is too vague for the reviewer to correlate with the
  claimed delta (e.g. "working on stuff" → 80%).
- The WIP references are stale or don't match what the brain described.

**Action:** Update the goal's `current_activity` to accurately describe
the work being done. The reviewer compares the claimed delta against the
plan — a clearer plan leads to better accept/reject decisions. If the
reviewer is consistently wrong despite good plans, file a bug against the
prompt template at `prompt_assets/simard/progress_assessment_reviewer.md`.

### Case C: The reviewer accepted on infrastructure failure

The LLM reviewer fails open — if the LLM endpoint is down or returns
unparseable output, the gate accepts with a diagnostic rationale containing
`"LLM submit failed"` or `"parse error"`. This is by design (the gate
should not block goals on LLM availability), but if it happens frequently:

```bash
curl -s -X POST http://localhost:8080/api/memory/search \
  -H 'Content-Type: application/json' \
  -d '{"query":"LLM submit failed"}' \
  | jq -r '.results[].data'
```

**Action:** Investigate the LLM endpoint availability. Frequent fail-open
accepts mean the gate is effectively disabled.

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

# Find LLM infrastructure failures (fail-open accepts)
curl -s -X POST http://localhost:8080/api/memory/search \
  -H 'Content-Type: application/json' \
  -d '{"query":"LLM submit failed"}' \
  | jq -r '.results[] | "\(.source)\t\(.data | tostring | .[0:200])"'

# Confirm the gate is on (drop --user for system-level installs)
journalctl --user -u simard-ooda -n 500 | grep 'progress-evidence:'

# Inspect the last_progress_update_at field directly
GOAL=enhance-simard-meeting-experience
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
