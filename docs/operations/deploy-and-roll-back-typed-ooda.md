---
title: Deploy and roll back typed OODA
description: Operator procedure for installer deployment, verification, and explicit rollback of typed OODA.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../reference/simard-installer.md
  - ../reference/ooda-capability-api.md
  - ../architecture/typed-ooda-loop.md
---

# Deploy and roll back typed OODA

Use `simard install` to deploy a candidate binary and matching prompt assets.
The installer creates a verified backup manifest before replacing live files.

The current backup is a recursive filesystem copy, not an online database
snapshot. Stop the services first when the state tree may be restored later.

## Before deployment

```bash
test -x ./target/release/simard
test -f prompt_assets/simard/recipes/goal-session-actor.yaml
test -f prompt_assets/simard/policies/goal-session-capabilities.toml

systemctl --user stop simard-ooda.service simard-signal.service
```

Stopping services prevents concurrent writes while the installer copies
`$SIMARD_HOME/state`. If your typed ledger uses another state root, back it up
separately with a store-appropriate consistent snapshot.

## Preview

```bash
./target/release/simard install \
  --simard-home "$HOME/.simard" \
  --systemd-user-dir "$HOME/.config/systemd/user" \
  --dry-run
```

Dry run validates paths and required assets. It does not create a backup or
invoke `systemctl`.

## Deploy

```bash
./target/release/simard install \
  --simard-home "$HOME/.simard" \
  --systemd-user-dir "$HOME/.config/systemd/user"
```

Capture the printed path:

```text
$HOME/.simard/.install-backups/install-<transaction>/manifest.json
```

The manifest covers the binary, prompt assets, both units, `config.toml`, and
`state/`. It does not separately snapshot a typed-OODA database or cognitive
store outside that state tree.

## Verify

The installer restarts both units but does not run an application health check.
Verify them explicitly:

```bash
systemctl --user is-active --quiet simard-ooda.service
systemctl --user is-active --quiet simard-signal.service

test -f "$HOME/.simard/prompt_assets/simard/recipes/goal-session-actor.yaml"
test -f "$HOME/.simard/prompt_assets/simard/policies/goal-session-capabilities.toml"

"$HOME/.simard/bin/simard" ooda outcomes list \
  --state-root "$HOME/.simard/state" \
  --limit 1
```

The outcome command proves that the installed binary can open the selected
typed ledger. It does not prove that a provider-backed cycle can complete.

## Roll back

Choose the manifest printed by the deployment:

```bash
manifest="$HOME/.simard/.install-backups/install-<transaction>/manifest.json"
jq '{version, transaction_id, simard_home, entries}' "$manifest"
```

Stop services so state is not modified during restoration:

```bash
systemctl --user stop simard-ooda.service simard-signal.service
```

Restore:

```bash
"$HOME/.simard/bin/simard" install \
  --simard-home "$HOME/.simard" \
  --systemd-user-dir "$HOME/.config/systemd/user" \
  --rollback "$manifest"
```

The command verifies all backup digests before deleting or restoring a live
surface. It then reloads, enables, and restarts both services.

## Failure handling

| Failure | Operator action |
| --- | --- |
| Backup or manifest verification fails | Installation stops before replacement; correct the path or storage failure. |
| File replacement or activation fails | Preserve the printed manifest, correct the failure, then run explicit rollback. |
| Rollback copy fails | Stop; preserve backup and live artifacts. Restoration is sequential and may be partial. |
| Service restart fails after rollback | Inspect `systemctl --user status` and restart after correcting the unit/runtime issue. |

The installer does not automatically roll back a failed deployment, does not
reverse-compensate a partial rollback, and does not restore prior service
enablement or active state. Do not claim a deployment or rollback succeeded
until the explicit verification commands pass.
