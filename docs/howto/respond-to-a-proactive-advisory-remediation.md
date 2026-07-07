---
title: How to respond to a proactive advisory remediation
description: "Operator guide for Simard's daily supply-chain advisory scan: what the tracking issue and remediation PR mean, how to run the steward on demand, how the pinned PR gate keeps a fresh upstream advisory from blocking your PR, and how the advisory-db pin advances."
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
status: active
related:
  - ../reference/supply-chain-advisory-stewardship.md
  - ../reference/supply-chain-audit.md
  - ../reference/dependency-trust-policy.md
  - ../howto/self-maintain-dependency-pins.md
---

# How to respond to a proactive advisory remediation

> **Status: active.** This describes shipped behaviour: the daily
> `advisory-scan` workflow, the `supply-chain-steward` binary, and the pinned
> PR-time advisory gate introduced in issue #2741. For the full design and API,
> see [Supply-chain advisory stewardship](../reference/supply-chain-advisory-stewardship.md).

Every day (06:00 UTC) — and on demand via **Run workflow** — Simard scans the
default branch against the **latest** RUSTSEC advisory database. When a **new**
advisory affects `Cargo.lock`, she files a tracking issue and, when a fix
exists, opens a remediation PR **before** the advisory could ever block your
open PRs. This guide covers what you'll see and what (if anything) you need to do.

## First: your PR did not break because of someone else's advisory

The whole point of #2741 is that a **freshly-published upstream advisory no
longer fails your PR**. The PR-time advisory gate is the `cargo-audit` job,
which runs **offline against a pinned advisory DB** (`.github/advisory-db.sha`),
so a new advisory published this morning cannot retroactively turn your green PR
red. (`cargo-deny` cannot pin its DB revision, so at PR time it runs only the
DB-independent licenses/bans/sources policy; the pinned `cargo-audit` job is the
authoritative advisory gate.)

If your PR's advisory check is failing, it is failing against the **pinned** DB —
i.e. a real, already-known issue in the graph — not upstream churn. Treat it as a
genuine finding (bump the crate, or follow the
[advisory-resolution order](../reference/dependency-trust-policy.md#advisory-resolution-policy)).

## What the daily scan produces

Depending on the reasoner's decision (see the
[decision table](../reference/supply-chain-advisory-stewardship.md#the-decision-function)),
a new vulnerability yields one of:

| Decision | You'll see | Your action |
| --- | --- | --- |
| **Bump** (a fix exists) | A tracking issue **and** a `chore/advisory-<id>` PR doing `cargo update -p <crate> --precise <patched>`. Self-merges when CI is green. | Usually none — it merges itself. Review if labelled `needs-CI-trigger`. |
| **Escalate** (fix exists but not applicable here — semver-incompatible or behind a git dep) | A tracking issue only, **no** PR. | Follow up: bump the upstream git rev ([pin how-to](./self-maintain-dependency-pins.md)) or plan the incompatible upgrade. |
| **JustifiedIgnore** (no fix exists, not exploitable in Simard's usage) | A tracking issue **and** a PR adding an ignore to `deny.toml` + `.cargo/audit.toml` with the issue link. | Review the justification, then merge. |
| **NoAction** (ignore still valid — no upstream fix — or already patched) | Nothing new. | None. |

Every issue carries a `stewardship-signature:` marker, so re-runs **update**
rather than duplicate. Re-running the scan is always safe.

## Reviewing a self-merging bump PR

Bump PRs self-merge **only** when every required check passes (the existing
green-CI-only [merge-authority rail](../reference/cross-repo-merge-authority.md) —
never `--admin`/`--no-verify`). You normally don't need to touch them.

Check on them with:

```bash
gh pr list --repo rysweet/Simard --state open \
  --search 'in:title "chore(advisory)"' \
  --json number,title,headRefName,labels
```

If a bump PR is labelled **`needs-CI-trigger`**, the `STEWARD_GH_TOKEN` bot
secret was absent, so its CI never triggered and it **did not** self-merge (a
deliberate fail-safe). Re-trigger CI and merge it yourself:

```bash
# Re-run required checks, then merge once green (normal review, no --admin):
gh pr ready <pr-number> --repo rysweet/Simard
gh workflow run verify.yml --repo rysweet/Simard --ref <branch>
# …once green:
gh pr merge <pr-number> --repo rysweet/Simard --squash --delete-branch
```

To make bump PRs self-merge again, set the bot token secret (scopes:
`contents`, `pull-requests`, `issues`):

```bash
gh secret set STEWARD_GH_TOKEN --repo rysweet/Simard
```

## Reviewing a justified-ignore PR

An ignore is a **security-sensitive** change. Before merging, confirm the hard
rail held — the reasoner only proposes an ignore when **no patched version
exists**:

1. **No fix upstream.** The advisory's `Solution`/`patched` field is empty. If a
   fix *does* exist, the PR should have been a **bump**, not an ignore — reject
   it and file the discrepancy. (RUSTSEC-2026-0204's original stopgap ignore was
   exactly this bug: the advisory said *"upgrade to >= 0.9.20"*, so it must be a
   bump.)
2. **Tracking issue linked.** The `deny.toml` `{ id, reason }` entry embeds the
   issue URL, and `.cargo/audit.toml` lists the bare ID with the same
   justification. Both files must list the **same** IDs.
3. **Not exploitable in Simard's usage.** The reason states *why* the advisory's
   vulnerable path is unreachable here (as with the genuinely fix-less `rsa`
   entry, RUSTSEC-2023-0071 — **not** RUSTSEC-2026-0204, whose fix shipped, so it
   belongs in a bump per step 1).

```bash
# Confirm both ignore files agree on the ignored ID set:
grep -oE 'RUSTSEC-[0-9]{4}-[0-9]{4}' deny.toml | sort -u
grep -oE 'RUSTSEC-[0-9]{4}-[0-9]{4}' .cargo/audit.toml | sort -u
# The two lists must match.

# Confirm the policy still passes:
cargo deny --locked check
```

## Running the scan on demand

Trigger the workflow manually (e.g. right after a big dependency change):

```bash
gh workflow run advisory-scan.yml --repo rysweet/Simard
gh run watch --repo rysweet/Simard
```

Or inspect decisions locally without side effects:

```bash
cargo audit --json | cargo run --locked --bin supply-chain-steward -- decide-only
```

This prints the `Decision` (`Bump` / `JustifiedIgnore` / `Escalate` /
`NoAction`) for each advisory the live database reports.

## How the advisory-db pin advances

The PR gate is pinned so upstream churn can't break you — but the pin must not
drift stale. When the daily scan finds DB HEAD **clean** (no advisory the gate
would otherwise miss), it opens a `chore(deps): bump advisory-db pin` PR that
advances `.github/advisory-db.sha` to HEAD:

```bash
# See the current pin:
grep -vE '^\s*(#|$)' .github/advisory-db.sha | head -n1

# See any open pin-bump PR:
gh pr list --repo rysweet/Simard --state open \
  --search 'in:title "bump advisory-db pin"'
```

Merge it like any other `chore(deps)` PR. The pin then reflects the latest
reviewed advisory state, and the PR gate keeps evaluating against a known,
deliberate revision.

## Verify end-to-end

1. **The pin file exists and is a 40-char SHA:**

   ```bash
   grep -vE '^\s*(#|$)' .github/advisory-db.sha | head -n1 | grep -qE '^[0-9a-f]{40}$' && echo OK
   ```

2. **The PR gate is offline against the pin** (a new upstream advisory won't
   fail it): `verify.yml`'s `cargo-audit` job passes `--no-fetch --db` against
   the pinned checkout; the `cargo-deny` job runs only `check licenses bans
   sources` (no advisory fetch at PR time).

3. **The scheduled scan is wired:**

   ```bash
   gh workflow list --repo rysweet/Simard | grep advisory-scan
   ```

4. **The reasoner decides correctly** — a green
   `cargo run --bin supply-chain-steward -- decide-only` over the current
   advisories, and a green `cargo deny --locked check licenses bans sources`.

## Related reading

- [Supply-chain advisory stewardship](../reference/supply-chain-advisory-stewardship.md) —
  the full design, reasoner API, and workflow reference.
- [Dependency trust policy → advisory resolution](../reference/dependency-trust-policy.md#advisory-resolution-policy) —
  the bump-or-ignore decision order the reasoner automates.
- [Keep Simard's dependency pins up to date](./self-maintain-dependency-pins.md) —
  bumping the upstream git rev when a fix lives behind a first-party dependency.
