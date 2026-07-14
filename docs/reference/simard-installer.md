---
title: Simard installer reference
description: Canonical binary, asset, user-systemd, verified-backup, and rollback contract for simard install.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../operations/deploy-and-roll-back-typed-ooda.md
  - ../howto/run-ooda-daemon.md
  - ./ooda-capability-api.md
---

# Simard installer reference

`simard install` deploys the currently executing binary, the complete prompt
asset tree, and two user-level systemd units. Before replacing live files it
creates a digest-verified backup manifest for the surfaces it owns or may need
to restore.

## Command

```text
simard install \
  [--simard-home PATH] \
  [--systemd-user-dir PATH] \
  [--systemctl PATH] \
  [--dry-run]

simard install \
  --rollback MANIFEST \
  [--simard-home PATH] \
  [--systemd-user-dir PATH] \
  [--systemctl PATH]
```

| Option | Purpose |
| --- | --- |
| `--simard-home PATH` | Absolute install root. Overrides `SIMARD_HOME`. |
| `--systemd-user-dir PATH` | User unit directory. Defaults to `$XDG_CONFIG_HOME/systemd/user` or `$HOME/.config/systemd/user`. |
| `--systemctl PATH` | `systemctl` executable override. |
| `--dry-run` | Validate inputs and print the install and activation plan without mutation. |
| `--rollback MANIFEST` | Restore one verified backup. Cannot be combined with `--dry-run`. |

There is no `--health-timeout` option and the installer does not run a
post-activation health command.

## Environment

| Variable | Default | Purpose |
| --- | --- | --- |
| `SIMARD_HOME` | `$HOME/.simard` | Install root. |
| `XDG_CONFIG_HOME` | `$HOME/.config` | Default user-systemd base. |
| `SIMARD_INSTALL_PROMPT_ASSETS_ROOT` | Auto-discovered | Preferred source for the prompt asset tree. |
| `SIMARD_PROMPT_ASSET_ROOT` | Auto-discovered | Compatibility source. |
| `SIMARD_PROMPT_ASSETS_DIR` | Auto-discovered | Compatibility source pointing to the root or `simard` directory. |

The source must contain the typed goal-session recipe and policy:

```text
simard/recipes/goal-session-actor.yaml
simard/policies/goal-session-capabilities.toml
```

## Installed and protected surfaces

```text
$SIMARD_HOME/
|- bin/simard
|- prompt_assets/
|- config.toml
|- state/
|- .install.lock
|- .install-staging/
`- .install-backups/

$XDG_CONFIG_HOME/systemd/user/
|- simard-ooda.service
`- simard-signal.service
```

The installer writes the binary, prompt assets, and unit files. It does not
rewrite `config.toml` or `state/`, but includes both in the pre-install backup
and explicit rollback inventory.

The canonical typed-OODA fixture/read API stores its ledger at
`<state-root>/typed-ooda/outcomes.sqlite3`. That database is protected by the
installer only when the selected state root is `$SIMARD_HOME/state`.

## Backup manifest

Before live replacement, a non-dry-run install writes:

```text
$SIMARD_HOME/.install-backups/install-<transaction>/manifest.json
```

The version-1 manifest contains:

- `transaction_id`;
- `simard_home`;
- one entry for `binary`, `prompt_assets`, `ooda_unit`, `signal_unit`,
  `config`, and `state`;
- each destination and backup path;
- whether the destination existed;
- a SHA-256 digest for each existing file or directory tree.

Rollback accepts only a manifest inside the selected
`$SIMARD_HOME/.install-backups` tree, with the exact expected surface inventory
and matching install root. Every backup digest is verified before restoration.

### Snapshot limitation

The current backup implementation recursively copies live files and
directories. It does not use SQLite's online backup API or a LadybugDB export,
and it does not stop services before copying.

Digest verification proves that the copied artifact did not change after the
copy. It does not prove an application-consistent snapshot of a database that
was being written concurrently. Stop the OODA and Signal services before
install when state rollback consistency matters.

## Install sequence

1. Resolve and validate install and unit paths.
2. Resolve the executing binary and prompt asset source.
3. Validate required assets and render unit files.
4. For a dry run, print the plan and stop.
5. Resolve `systemctl` and acquire the per-home install lock.
6. Stage the binary and prompt assets.
7. Copy and digest the six backup surfaces; write and verify the manifest.
8. Replace the binary, prompt assets, and unit files.
9. Remove staging.
10. Print the manifest path and rollback command.
11. Run `systemctl --user daemon-reload`, enable both units, and restart both
    services.

The binary and prompt asset candidates are staged before replacement. Unit
files are written through temporary files and renamed into place.

## Rollback behavior

Explicit rollback:

1. verifies the manifest location, version, install root, inventory, and backup
   digests;
2. removes each current destination;
3. copies each previously existing backup into place;
4. verifies the restored digest;
5. leaves destinations that were absent before the install absent;
6. reloads, enables, and restarts both services.

Rollback is not an atomic multi-surface transaction. It restores surfaces in
sequence and does not preserve the prior enabled/active service state. If a
copy or later activation step fails, the command returns an error but does not
automatically compensate earlier restored surfaces.

## Dry run

```bash
simard install \
  --simard-home "$HOME/.simard" \
  --systemd-user-dir "$HOME/.config/systemd/user" \
  --dry-run
```

Dry run validates paths and assets and prints planned file and systemd
operations. It does not acquire the install lock, stage files, create a backup,
replace files, or invoke `systemctl`.

## Failure contract

The installer returns nonzero for invalid paths, missing assets, lock
contention, staging or backup failure, manifest verification failure, file
replacement/restoration failure, or a failed `systemctl` command.

Automatic rollback after install or activation failure is not implemented. Use
the printed verified manifest after correcting the underlying problem.
