---
title: Enable the objective merge-judge fallback (converge delivery-ready PRs)
description: >
  How to turn on the opt-in objective merge-judge tier so Simard actually merges
  green, mergeable, non-in-flight, trusted-author PRs instead of re-escalating
  them every Overseer tick — set SIMARD_MERGE_OBJECTIVE_FALLBACK and
  SIMARD_MERGE_TRUSTED_AUTHORS, canary one repo, verify prs_merged advances,
  confirm the fail-closed default, and roll back.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/objective-merge-judge-fallback.md
  - ../reference/objective-merge-judge-api.md
  - ../reference/autonomous-merge-review-gate.md
  - ../reference/ready-prs-sensor-api.md
  - ./enable-autonomous-self-merge-canary.md
  - ./triage-stale-pull-requests.md
  - ./diagnose-merge-pr-verdict-parse-failures.md
---

# Enable the objective merge-judge fallback

> **Goal.** Turn on the opt-in **objective merge-judge** so delivery-ready PRs —
> green (`mergeStateStatus=CLEAN`), `MERGEABLE`, non-draft, authored by a
> **trusted** author, and owned by no in-flight engineer — actually **merge**
> instead of being re-escalated every Overseer tick. The default is fail-closed
> (`RefusingMergeJudge`); this is a deliberate, reversible opt-in.

For the *why* and the safety model, read
[the objective merge-judge fallback concept](../concepts/objective-merge-judge-fallback.md);
for the typed surface and edge-case matrix, see
[the API reference](../reference/objective-merge-judge-api.md).

## Before you start

- Confirm the symptom this addresses: the delivery step **selects** or
  **escalates** the same green PRs every tick without merging. Check the
  Overseer activity feed / `merge_judge_kind` telemetry — if it reads
  `refusing`, no review authority is wired and every green PR is refused.
- Ensure the objective gates you rely on are genuinely green (CI, `MERGEABLE`,
  base/repo allow-lists). The fallback replaces only the **judgment** half; it
  never bypasses those gates.
- `gh` authenticated as the daemon identity; the daemon's own bot login is
  **never** trusted (no self-merge).

## Steps

### 1. Choose the trusted authors

Set the allowlist to the human/owner logins whose green PRs you want landed.
Matching is against the **authenticated `author.login`** (exact, lowercased) —
not any PR text. The default is `rysweet`.

```bash
export SIMARD_MERGE_TRUSTED_AUTHORS=rysweet
```

Multiple authors are comma-separated (`rysweet,other-login`). Entries with
whitespace or `/` are rejected and logged.

### 2. Enable the fallback

```bash
export SIMARD_MERGE_OBJECTIVE_FALLBACK=1
```

Accepted truthy values (case-insensitive): `1`, `true`, `yes`, `on`. Anything
else — or unset — keeps `RefusingMergeJudge`.

### 3. Canary one repo first

Scope the autonomous-merge repo allowlist to a single repo before a fleet-wide
rollout (see
[Enable autonomous self-merge (canary one repo)](./enable-autonomous-self-merge-canary.md)),
then start (or restart) the daemon so it re-reads the environment.

For systemd deployments, add the two variables to the unit's environment and
reload:

```ini
# /etc/systemd/system/simard-overseer.service (drop-in)
[Service]
Environment=SIMARD_MERGE_OBJECTIVE_FALLBACK=1
Environment=SIMARD_MERGE_TRUSTED_AUTHORS=rysweet
```

```bash
sudo systemctl daemon-reload
sudo systemctl restart simard-overseer
```

### 4. Verify convergence

Confirm the tier switched and PRs actually merge:

- **Telemetry:** `merge_judge_kind` now reports `objective` (was `refusing`).
- **Outcome:** `prs_merged` advances across ticks; the previously re-escalated
  green PRs disappear from the delivery/escalation set.
- **Selection:** trusted-author non-engineer PRs now appear in `ready_prs`
  (gate #3 admission) and known-non-draft PRs are no longer dropped (gate #5
  `is_draft` hydration).

```bash
# The green, mergeable, trusted-author PR that used to loop should now be MERGED.
gh pr view <N> -R rysweet/Simard --json state,mergeStateStatus,author,isDraft
```

## Verify the fail-closed default still holds

Unsetting the switch must return the daemon to refusing every PR:

```bash
unset SIMARD_MERGE_OBJECTIVE_FALLBACK   # or set to 0/false
# restart daemon → merge_judge_kind == refusing, no autonomous merges
```

## Roll back

Remove both variables (or set `SIMARD_MERGE_OBJECTIVE_FALLBACK=0`) from the unit
environment and restart. No data migration is involved — the tier selection is
recomputed at boot.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Still `refusing` after enabling | Trusted set empty/all-invalid, or a recipe-/LLM-backed judge is wired and takes precedence | Check `SIMARD_MERGE_TRUSTED_AUTHORS`; review `tracing` warns for rejected entries |
| Trusted PR still `NotReady` | An objective gate is red (CI, conflict, draft) — the pre-filter blocks upstream | Fix the gate; the judge only runs past objective gates |
| Bot's own PR not merging | Bot identity is excluded by design (no self-merge) | Expected — use a human trusted author |
| Green PR never enters `ready_prs` | Gate #3/#5 — author not trusted or `isDraft` absent from JSON | Add the author to the allowlist; confirm the listing hydrates `isDraft` |
