---
title: How to verify and repair PATH-entrypoint parity
description: Operator runbook for the installer's PATH-entrypoint ownership guarantee — confirm the `simard` on your PATH is the freshly deployed binary, read the `entrypoint_parity` self-health probe, understand the `redeploy-local.sh` parity gate, and repair a stale entrypoint or a foreign shadow.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../reference/simard-installer.md
  - ../reference/self-deploy-api.md
  - ../reference/simard-cli.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../howto/run-ooda-daemon.md
---

# How to verify and repair PATH-entrypoint parity

`simard install` owns the `simard` entrypoint on your PATH: after any deploy,
`~/.local/bin/simard` is a symlink to the versioned live binary
`~/.simard/bin/simard`, no stale installer-owned `simard` shadows it, and
`simard --version` on PATH equals the deployed daemon's version. This runbook
shows how to confirm that guarantee and how to repair the two ways it can be
broken: a **stale entrypoint** and a **foreign shadow**.

For the full contract, see the
[installer reference](../reference/simard-installer.md#path-entrypoint-ownership-guarantee)
and the [`EntrypointParityProbe`](../reference/self-deploy-api.md#entrypointparityprobe).

## Why this matters

The systemd unit renders `PATH` with `~/.local/bin` **first**
(`{home}/.local/bin:{home}/.cargo/bin:{simard_home}/bin:...`). Before this
guarantee existed, a deploy would rebuild and reinstall `~/.simard/bin/simard`
(the version the daemon runs) but leave an older `~/.local/bin/simard` — first on
PATH — shadowing it. The operator's `simard status` / `goal list` then silently
ran a version-skewed CLI, and its writes to the cognitive-memory goal board
could be lost or incompatible. The owned entrypoint and the parity gate make
that impossible to leave behind.

## 1. Verify parity

Run these checks. All three versions should match:

```bash
which simard                          # expect: ~/.local/bin/simard
readlink -f "$(command -v simard)"    # expect: /home/you/.simard/bin/simard
simard --version                      # PATH-resolved
"$HOME/.simard/bin/simard" --version  # installed daemon binary
```

Then read the daemon's own probe:

```bash
simard self-health --json | jq .probes.entrypoint_parity
```

Healthy output:

```json
{
  "healthy": true,
  "installed_version": "simard 0.35.0",
  "path_version": "simard 0.35.0",
  "resolved_path": "/home/you/.local/bin/simard",
  "canonical_path": "/home/you/.simard/bin/simard",
  "path_mismatch": false,
  "foreign_shadow": false
}
```

`entrypoint_parity` is healthy only when **both** hold: `canonical_path` (the
`readlink -f` of the PATH `simard`) equals `~/.simard/bin/simard` — *path
identity* — and the two version strings match. Path identity is the check that
catches a stale *file* at the same version, which a version-string comparison
alone would silently pass.

`simard self-health` exits non-zero if any probe — including
`entrypoint_parity` — is unhealthy.

## 2. Understand the `redeploy-local.sh` parity gate

`scripts/redeploy-local.sh` runs the same check automatically after every
non-dry-run deploy. It asserts **path identity first**, then version equality. On
success:

```text
[redeploy] verifying PATH-entrypoint parity ...
[redeploy] PATH-resolved:  /home/you/.local/bin/simard -> /home/you/.simard/bin/simard
[redeploy] installed:      simard 0.35.0
[redeploy] PATH-version:   simard 0.35.0
[redeploy] parity OK (path identity + version match)
```

On a stale-file path mismatch (even when the version happens to match) it exits
non-zero:

```text
[simard] FATAL: PATH-entrypoint parity check failed after install
[simard]   PATH-resolved: /home/you/.local/bin/simard -> /home/you/.local/bin/simard
[simard]   expected:      -> /home/you/.simard/bin/simard
[simard]   the 'simard' on PATH is not the installed entrypoint (stale file or foreign shadow)
```

On a version skew it prints a loud `[simard]` diagnostic:

```text
[simard] FATAL: version parity check failed after install
[simard]   installed:     simard 0.35.0  (/home/you/.simard/bin/simard)
[simard]   PATH-resolved: simard 0.31.0  (/home/you/.local/bin/simard)
[simard]   a stale 'simard' is still shadowing the freshly installed binary on PATH
```

The gate re-runs `hash -r` before re-resolving `simard`, so a shell PATH cache
cannot hide a real skew. It is skipped only when `DRY_RUN=1`.

## 3. Repair a stale entrypoint

If `entrypoint_parity` is unhealthy with `foreign_shadow: false` (typically
`path_mismatch: true`, or a version skew), an installer-owned but stale `simard`
is on PATH (for example, a leftover `~/.local/bin/simard` copy from an old
release, or a `~/.cargo/bin/simard` from `cargo install`) — or a same-version
stale *file* that only the path-identity check catches. The fix is simply to
**redeploy** — reconciliation runs unconditionally on every install and repairs
the entrypoint and prunes verified-ours orphans:

```bash
./scripts/redeploy-local.sh --branch main
```

or, if you already have the binary you want:

```bash
"$HOME/.simard/bin/simard" install
```

Then re-run the [verify](#1-verify-parity) checks. Reconciliation is idempotent:
rerunning `install` converges on exactly one owned entrypoint symlink.

## 4. Resolve a foreign shadow

If `simard self-health` shows `foreign_shadow: true`, or the installer printed:

```text
[simard] WARNING: a foreign 'simard' occupies the PATH entrypoint and was left untouched:
[simard]   /home/you/.local/bin/simard
```

then a file at the entrypoint path is **not** installer-owned — it is not a
symlink into `~/.simard/bin/` and its `--version` did not start with `simard `.
The installer deliberately never deletes it. Resolve it manually:

1. Inspect what it is before touching it:

   ```bash
   ls -l ~/.local/bin/simard
   file ~/.local/bin/simard
   ~/.local/bin/simard --version || true
   ```

2. If it is genuinely unrelated to Simard, move it out of the way (rename, do
   not blindly delete) so it stops shadowing:

   ```bash
   mv ~/.local/bin/simard ~/.local/bin/simard.foreign.bak
   ```

3. Redeploy so the installer can take ownership of the now-free entrypoint:

   ```bash
   "$HOME/.simard/bin/simard" install
   ```

4. Verify with the [step 1 checks](#1-verify-parity). `foreign_shadow` should now
   be `false` and all versions should match.

> **Caution — an unrelated tool named `simard`.** The installer identifies a
> non-symlink ours-copy by its `--version` banner. If you keep an *unrelated*
> third-party binary that is also named `simard` **and** prints a `--version`
> line starting with `simard `, the installer will classify it as its own and
> replace or remove it at a reconciled path (`~/.local/bin`, `~/.cargo/bin`).
> Keep any such unrelated tool out of those directories, or under a different
> name. See
> [the `OursMarker` residual](../reference/simard-installer.md#clearly-ours-classification-two-tier-fail-closed).

## Related

- [Simard installer reference](../reference/simard-installer.md) — the ownership
  guarantee, orphan reconciliation rules, and install outcome fields.
- [Self-deploy API reference](../reference/self-deploy-api.md#entrypointparityprobe)
  — the `EntrypointParityProbe` fields and fail-closed semantics.
- [How to verify and roll back a self-deploy](./verify-and-roll-back-a-self-deploy.md)
  — the broader post-deploy health and rollback runbook.
