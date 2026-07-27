---
title: Enable and operate the GitHub merge queue
description: >
  Operator guide to Simard's native GitHub merge queue: why it exists (the
  strict "up-to-date-before-merge" live-lock), how the required CI reruns under
  the `merge_group` trigger, how to enable the queue idempotently with
  scripts/enable-merge-queue.sh, how branch protection is managed externally,
  and how to verify the queue is active and enforcing. Resolves issue #1050.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ./triage-stale-pull-requests.md
  - ./configure-agentic-merge-queue-reasoning.md
  - ./fix-ci-linker-oom.md
  - ../reference/cross-repo-merge-authority.md
---

# Enable and operate the GitHub merge queue

> **Goal.** Enable GitHub's **native merge queue** on `main` so a backlog of
> ready pull requests merges serially without every PR having to re-pass the
> full ~35-minute CI matrix against a branch tip that keeps moving. This
> resolves the merge **live-lock** described in issue
> [#1050](https://github.com/rysweet/Simard/issues/1050).

This is a GitHub-platform feature (`merge_group` workflow trigger + a
`required_merge_queue` ruleset). It is **not** the same thing as Simard's
[agentic merge-queue reasoning](./configure-agentic-merge-queue-reasoning.md),
which is the daemon's observe/orient stage deciding *which* PRs are merge-ready.
The native merge queue documented here is the GitHub mechanism that actually
serializes and lands those PRs; the agentic gate still decides *whether* to
merge. The two compose — they do not overlap.

## The problem this solves

`main` used strict *up-to-date-before-merge* protection
(`required_status_checks.strict = true`) across its required status-check
contexts (a subset of the eight `verify` jobs enumerated below). The
required CI matrix takes roughly **35 minutes** while `main` advances about
every **30 minutes**. A PR that finally goes green is, by the time its checks
finish, already behind the tip — so it can never satisfy "up to date before
merge" before the next commit lands. With a 6-PR backlog this starves the queue
into a **live-lock**: nothing ever merges.

The native merge queue breaks the live-lock by inverting the order:

1. You mark a PR "ready to merge" (or the agentic gate does).
2. GitHub builds a temporary `merge_group` ref: `main` **plus** the queued PR(s).
3. The **same** required checks run once against that merged-result ref.
4. If they pass, GitHub fast-forwards `main` to exactly the ref that was tested.

Freshness is now **guaranteed by construction** — every commit that lands on
`main` was tested against the real merged result — so the strict
"re-update-and-re-run per PR" requirement is no longer needed and is relaxed to
merge-queue semantics.

## What changed in the repo

| Area | Change | File |
|------|--------|------|
| CI trigger | Added a `merge_group:` trigger so every existing required job also runs in the queue, under **identical check names** | `.github/workflows/verify.yml` |
| Coverage | Added a `merge_group:` trigger so coverage runs consistently in queue context (additive) | `.github/workflows/coverage.yml` |
| Enablement | New idempotent script that turns the queue on and relaxes strict | `scripts/enable-merge-queue.sh` |
| Docs | This guide | `docs/howto/merge-queue.md` |

All changes are **additive and non-breaking**: no required check was removed or
weakened, and the same 8 `verify` jobs (`pre-commit`, `cargo-audit`,
`cargo-deny`, `cargo-vet`, `npm-audit`, `scripts-tests`, `install-real`,
`e2e-dashboard`) run under `merge_group` exactly as they do under
`pull_request`.

## 1. How the `merge_group` trigger works

`verify.yml` now fires on three events:

```yaml
on:
  push:
  pull_request:
  merge_group:
```

The jobs are **reused, not duplicated** — the identical job list runs whether the
event is `pull_request` (validating your branch in isolation) or `merge_group`
(validating your branch merged on top of `main`). Because the check **names**
are unchanged across both events, the branch-protection required contexts keep
enforcing: GitHub matches the required context by name, and finds it satisfied
by the `merge_group` run.

> **Enforcement invariant.** Any job step that keys off
> `github.event_name == 'pull_request'` or reads `github.event.pull_request.*`
> must still behave correctly under `merge_group`, where those fields are
> absent. Gating steps run unconditionally; only PR-only *side effects* (for
> example, posting a coverage comment back to the PR) are gated on the
> `pull_request` event and are simply skipped in the queue. No **gate** is ever
> skipped by the event switch — only non-gating conveniences.

You do not run these workflows by hand. GitHub schedules the `merge_group` run
automatically when a PR enters the queue.

## 2. Enable the queue: `scripts/enable-merge-queue.sh`

Branch protection for `main` is managed **externally** through the GitHub API
(there is no settings-as-code file in the repo), so enabling the queue is an
explicit, admin-run apply step rather than something CI can self-provision. The
script is the audited, idempotent way to perform that apply.

```bash
# Preview exactly what would change (no writes):
scripts/enable-merge-queue.sh --dry-run

# Apply to the default target (rysweet/Simard, branch main):
scripts/enable-merge-queue.sh

# Apply to an explicit repo/branch:
scripts/enable-merge-queue.sh --repo rysweet/Simard --branch main
```

The script:

- Creates a **`required_merge_queue`** ruleset on the target branch if one is
  absent (via `gh api .../rulesets`, `X-GitHub-Api-Version: 2022-11-28`). An
  existing ruleset is treated as already-satisfied and left unchanged.
- **Relaxes** strict up-to-date-before-merge (`strict: false`) so the queue's
  merged-result testing — not per-PR re-runs — provides freshness.
- **Preserves** every existing required status check context; it never removes
  or weakens a required check.
- Is **idempotent**: re-running it converges to the same state and is safe to
  run repeatedly (already-enabled → no-op success).

### Flags

| Flag | Meaning |
|------|---------|
| `--repo <owner/name>` | Target repository. Default: `rysweet/Simard`. Validated against `^[A-Za-z0-9._/-]+$`. |
| `--branch <name>` | Target branch. Default: `main`. Validated against `^[A-Za-z0-9._/-]+$`. |
| `--dry-run` | Print the API method + path that *would* be called and exit `0` without writing. |
| `-h`, `--help` | Print usage and exit `0`. |

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success (queue enabled/converged, or `--dry-run`/`--help`). |
| `1` | Generic failure (unexpected API error, malformed response). |
| `2` | Invalid arguments / input validation failure. |
| `3` | Insufficient permission — the token lacks **repo admin** (HTTP 403). |

### Authentication and permissions

- The script uses **`gh`-managed auth** or a `GITHUB_TOKEN` supplied in the
  environment. It **never** accepts a token as a flag and **never** echoes or
  logs a token — `--dry-run` output is limited to the HTTP method and path.
- Enabling a ruleset requires **repository admin**. A non-admin token returns
  HTTP 403, which the script maps to exit `3` with a clear message telling the
  operator which scope is missing.

### Idempotency and API-status handling

| HTTP status | Script behavior |
|-------------|-----------------|
| 2xx | Ruleset created (or `strict` relaxed) → exit `0`. |
| 403 | Missing admin → exit `3`. |
| 404 / 422 | Treated as already-satisfied / benign convergence → idempotent, exit `0`. |
| 429 / 5xx / transport error | **Transient** — retried with bounded exponential backoff (up to 3 attempts, 2s→4s) before being surfaced. |
| other | Unexpected → exit `1`. |

Transient GitHub API failures (rate limits, `5xx`, or transport-level
connection/timeout errors) are retried automatically; each retry is logged to
stderr as `transient GitHub API error … retrying in Ns`. Terminal outcomes
(`403` no-admin, `404`/`422` convergence) are **never** retried — they resolve
immediately. If every attempt is exhausted the underlying error is reported and
the script exits `1`.

## 3. Verify the queue is active and enforcing

```bash
# 1. Confirm the merge_group trigger is wired into the required workflow:
grep -n 'merge_group' .github/workflows/verify.yml

# 2. Preview the enablement state without changing anything:
scripts/enable-merge-queue.sh --dry-run

# 3. Inspect the live ruleset (requires read access):
gh api repos/rysweet/Simard/rulesets --jq '.[].name'
# expect a 'required_merge_queue' rule on main

# 4. On a queued PR, confirm a merge_group check run appears with the SAME
#    required context names as the pull_request run (that name match is what
#    keeps the required contexts enforcing).
gh pr checks <PR-NUMBER>
```

A healthy queue shows:

- `verify` (and its 8 jobs) running under a `merge_group` ref for queued PRs.
- The **same** required-context names green as on the PR itself.
- `main` fast-forwarding only to a ref that has a green `merge_group` run.

## 4. Roll back

The change is additive, so rollback is simply: delete the `required_merge_queue`
ruleset and (if desired) restore `strict: true` from the GitHub branch-protection
UI or API. The `merge_group:` trigger in the workflows is harmless without an
active queue — GitHub only emits `merge_group` events when a queue exists — so
the workflow triggers can be left in place.

## Security considerations

- **The queue must not weaken gating.** The `merge_group` jobs reuse the exact
  required jobs under identical names, and every gating step runs regardless of
  `github.event_name`. Only PR-only *side effects* (e.g., the coverage comment)
  are event-gated, never a gate itself.
- **Least privilege.** Enabling the queue needs repo admin; the script fails
  loudly (`exit 3`) instead of silently no-op'ing when the token can't write.
- **No token leakage.** Tokens come only from `gh`/`GITHUB_TOKEN`; they are
  never passed as arguments, echoed, or logged. `--dry-run` prints only method
  and path.
- **Input validation.** `--repo` / `--branch` are regex-validated and every
  expansion is quoted. The script never `eval`/`source`s API output: response
  bodies are discarded and outcomes are classified only by matching `gh`'s HTTP
  status text (the `--jq` read-back in §3 is illustrative, not part of the
  apply path).
- **Pinned API version.** All calls send `X-GitHub-Api-Version: 2022-11-28` so a
  server-side default change can't silently alter behavior.
- **Unchanged CI permissions.** Adding `merge_group` does not broaden workflow
  permissions and introduces no `pull_request_target` or fork-write pattern.

## Notes and caveats

- `main` is **not** API-protected in every environment (a bare
  `branches/main/protection` GET can 404). Where protection is managed outside
  the repo, running `scripts/enable-merge-queue.sh` (with an admin token) is the
  accepted apply step; CI cannot self-prove queue enablement.
- The upstream issue is tracked as `#1050`
  ([rysweet/Simard#1050](https://github.com/rysweet/Simard/issues/1050); the
  ecosystem handoff refers to the same tree as `amplihack-rs`).
- Relaxing strict *only* provides safety while the queue is actually active and
  enforcing — verify §3 after enabling.
- **Classic protection only:** the script's strict-relax step patches *classic*
  branch protection
  (`repos/{repo}/branches/{branch}/protection/required_status_checks`). If
  up-to-date-before-merge is instead enforced by a repository **ruleset**'s
  `required_status_checks` with `strict: true`, this PATCH does **not** touch it
  — it lands as a `404`/`422` idempotent no-op. In that case relax the `strict`
  flag on the offending ruleset directly (via the GitHub UI or
  `gh api repos/rysweet/Simard/rulesets/{id}`) after enabling the queue.

## See also

- [Triage stale pull requests](./triage-stale-pull-requests.md) — the manual
  path the queue automates for a backlog.
- [Configure agentic merge-queue reasoning](./configure-agentic-merge-queue-reasoning.md)
  — the daemon deciding *which* PRs are merge-ready (distinct from this native
  queue).
- [Fix CI linker OOM](./fix-ci-linker-oom.md) — CI runner tuning that keeps the
  required matrix green.
- [Cross-repo merge authority](../reference/cross-repo-merge-authority.md) — how
  Simard is authorized to merge across governed repos.
