---
title: Enable autonomous self-merge (canary one repo)
description: >
  Operator runbook to turn on Simard's autonomous self-merge safely, one repo at
  a time, via the SIMARD_AUTOMERGE_REPOS allowlist. Ships OFF by default; this
  page shows how to canary-enable a single repo, verify Simard sees and merges
  only her own green + MERGEABLE PRs through the authoritative gate, and how to
  roll back instantly.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/autonomous-self-merge-sensor.md
  - ../reference/ready-prs-sensor-api.md
  - ../reference/cross-repo-merge-authority.md
  - ./triage-stale-pull-requests.md
  - ./watch-overseer-activity.md
---

# Enable autonomous self-merge (canary one repo)

> **Goal.** Turn Simard's autonomous self-merge from OFF (the shipped default)
> to ON for **exactly one canary repo**, confirm she sees and merges only her
> own green + `MERGEABLE` PRs through the authoritative gate, then decide whether
> to widen the allowlist. Roll back is a one-line change.

Autonomous self-merge ships **OFF**: with `SIMARD_AUTOMERGE_REPOS` unset, the
`ready_prs` sensor returns an empty candidate list, so `PrReadyToMerge` is never
emitted and Simard merges nothing on her own. Deploying the feature does **not**
change behavior until you complete this runbook. Background:
[autonomous self-merge sensor](../concepts/autonomous-self-merge-sensor.md) and
its [API reference](../reference/ready-prs-sensor-api.md).

## Prerequisites

- The daemon binary includes the `ready_prs` sensor (this feature).
- **`gh`** authenticated so the daemon can list and merge PRs (`gh auth status`).
- You know the login Simard authors her PRs under (`gh api user --jq .login` on
  the daemon's identity, or the engineer login she commits as) — you set this
  explicitly as `SIMARD_AUTOMERGE_AUTHOR`. There is **no** implicit fallback to
  the ambient `gh` identity: leaving it unset keeps the sensor fail-closed.
- You know the canary repo in `owner/name` form (start with `rysweet/Simard`).
- `NODE_OPTIONS=--max-old-space-size=32768` exported for any Node-backed tooling
  (saved operator preference; change in `~/.amplihack/config`).

## What you are turning on

| Variable | Effect |
|---|---|
| `SIMARD_AUTOMERGE_REPOS` | Comma-separated `owner/name` allowlist of repos eligible for autonomous self-merge. **Unset/empty ⇒ OFF.** |
| `SIMARD_AUTOMERGE_AUTHOR` | **Required.** The `gh` login whose own PRs Simard may merge. **Unset/empty ⇒ OFF** (fail-closed — there is no ambient `gh api user` fallback). |

Setting the allowlist only lets the sensor **list candidates**. Every candidate
still passes the authoritative merge gate in
[`merge_authority`](../reference/cross-repo-merge-authority.md)
(`MERGEABLE` + CI-green + merge-judge over the six merge-ready evidence sections) before any
merge, and that gate never uses `--admin`/`--no-verify`.

## Procedure

### 1. Pick and confirm the canary repo

```bash
gh auth status                          # confirm the OODA/engineer login
gh api user --jq .login                 # this is the author Simard will match
gh pr list --repo rysweet/Simard --state open \
  --json number,author,mergeable,statusCheckRollup \
  --jq '.[] | select(.author.login=="<that-login>")'
```

You want at least one of Simard's own PRs to be green + `MERGEABLE` so the canary
has something to act on. If none are ready, the canary is still safe — it will
simply merge nothing.

### 2. Enable exactly one repo and set the author

Add **both** gate variables to the daemon's systemd unit (user service) — a
single-repo allowlist plus the explicit own-PR author. Leaving either unset
keeps the sensor OFF (fail-closed):

```ini
# ~/.config/systemd/user/simard-ooda.service  (drop-in or [Service] block)
Environment=SIMARD_AUTOMERGE_REPOS=rysweet/Simard
Environment=SIMARD_AUTOMERGE_AUTHOR=<the login from step 1>
```

Then reload and restart so the daemon reads the new values at boot:

```bash
systemctl --user daemon-reload
systemctl --user restart simard-ooda.service
```

> Both gates are read from the daemon's **process environment**, which is
> fixed for the life of the process. Changing them mid-run has no effect —
> restart to apply.

### 3. Verify the wire is live

Watch the Overseer cycle for the newly-emitted signal and intervention (see
[watch Overseer activity](./watch-overseer-activity.md)):

```bash
journalctl --user -u simard-ooda.service -f | \
  grep -E 'PrReadyToMerge|DeliveryReady|VerifyAndMergePr|survey_ready_prs'
```

Expected on a cycle where a candidate exists:

- a `survey_ready_prs` enrichment line naming the candidate PR(s),
- `Signal::PrReadyToMerge { repo: "rysweet/Simard", pr: <N> }`,
- `Intervention::VerifyAndMergePr` for that PR,
- then the authoritative gate's verdict (merged, or refused with a reason).

If there are no ready candidates, the survey simply contributes nothing this
cycle and no `PrReadyToMerge` is emitted — that is the correct
OFF-for-this-cycle behavior, not an error. (Depending on log level you may see
no survey line at all when the candidate set is empty and no repo errored.)

### 4. Confirm the safety boundaries held

- **Human PRs untouched.** Open a human-authored PR (or point at an existing
  one) and confirm it never appears as a candidate — only Simard's own login
  matches.
- **Red / conflicting excluded.** A PR with a failing check or `CONFLICTING`
  never becomes a candidate.
- **Gate still authoritative.** A candidate whose body lacks the six substantive
  merge-ready evidence sections (QA-team, Documentation, Quality-audit, CI, Scope,
  Verdict) is **refused** by the merge-judge, not merged. Check the refusal reason
  in the log.
- **No branch-protection bypass.** The merge command is
  `gh pr merge <N> --squash --delete-branch` — no `--admin`, no `--no-verify`.

## Widen (only after a clean canary)

Once the canary repo has merged (or correctly refused) across several cycles
with no surprises, add the next repo:

```ini
Environment=SIMARD_AUTOMERGE_REPOS=rysweet/Simard,rysweet/azlin
```

`daemon-reload` + `restart` again. Widen one repo at a time.

## Roll back (instant)

Remove the variable (or set it empty) and restart:

```bash
# delete the Environment=SIMARD_AUTOMERGE_REPOS line, then:
systemctl --user daemon-reload
systemctl --user restart simard-ooda.service
```

With the allowlist unset, the sensor returns an empty candidate list on every
cycle and Simard is back to OFF. No code change or redeploy is required to
disable.

## Troubleshooting

| Symptom | Likely cause | Action |
|---|---|---|
| Survey never lists a known-green own PR | `SIMARD_AUTOMERGE_AUTHOR` unset or mismatched | Confirm the var is set (unset ⇒ fail-closed OFF). Casing is tolerated (the match is case-insensitive), but confirm it is the same whole login as the PR's `author.login`. |
| No candidates although author and repo are set | author login typo (wrong whole login) | Compare the configured `SIMARD_AUTOMERGE_AUTHOR` with the PR's `author.login`; the match is a whole-login, case-insensitive equality — a different login (not just different casing) yields nothing. |
| Candidate listed but never merged | authoritative gate refused it | Read the refusal reason in the log — usually a missing or thin merge-ready evidence section (QA-team, Quality-audit, CI link). Fix the PR body; the sensor was correct to list it. |
| No `survey_ready_prs` line at all | allowlist unset or typo | Confirm `SIMARD_AUTOMERGE_REPOS` is exactly `owner/name`; unknown values contribute nothing. Restart after any change. |
| A repo in the allowlist is skipped every cycle | `gh pr list` errored for that repo | Look for the `warn!` line naming the repo (auth/permissions); other repos are unaffected. |
