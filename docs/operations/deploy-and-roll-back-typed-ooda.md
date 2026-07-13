---
title: Deploy and roll back typed OODA
description: Verified installer deployment and rollback for the typed-capability OODA route.
last_updated: 2026-07-13
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

`simard install` stages the candidate binary and complete prompt asset tree,
then creates and verifies a transaction backup before replacing any live
surface. The backup covers:

- the Simard binary;
- prompts, recipes, and capability policies;
- OODA and Signal service units;
- `config.toml`;
- the compatible state tree, including typed outcomes and effect jobs.

The installer refuses replacement when backup creation or digest verification
fails.

## Deploy

```text
simard install \
  --simard-home "$SIMARD_HOME" \
  --systemd-user-dir "$HOME/.config/systemd/user"
```

The command prints the verified manifest path before restarting services. Keep
that path; it is the rollback authority for this deployment.

Confirm that the actor assets were installed:

```text
test -f "$SIMARD_HOME/prompt_assets/simard/recipes/goal-session-actor.yaml"
test -f "$SIMARD_HOME/prompt_assets/simard/policies/goal-session-capabilities.toml"
```

Run the bounded fixture cycles from the
[typed OODA tutorial](../tutorials/complete-a-typed-ooda-cycle.md) against an
isolated state root before authorizing production expansion.

## Roll back

Stop admission of new work and allow running effects to finish. Then use the
candidate binary (not an unverified copied binary) to restore the manifest:

```text
simard install \
  --simard-home "$SIMARD_HOME" \
  --systemd-user-dir "$HOME/.config/systemd/user" \
  --rollback "$SIMARD_HOME/.install-backups/install-<transaction>/manifest.json"
```

Rollback first verifies every backup digest and validates that the manifest's
surface inventory and install root match the requested installation. It then
restores all surfaces and restarts the user services. Missing pre-deployment
surfaces are removed rather than synthesized.

Typed terminal and effect records are durable state. Rollback never reconstructs
or changes them from recipe output or logs.

## Failure behavior

| Failure | Required behavior |
| --- | --- |
| Backup copy or digest verification fails | Installation stops before replacement. |
| Manifest is outside `.install-backups` | Rollback is rejected. |
| Manifest targets another install root | Rollback is rejected. |
| A backup digest changed | Rollback is rejected before restoration. |
| Service activation fails | The installer returns failure; use the verified manifest after correcting systemd access. |

There is no production switch from typed execution to a prose-parser route.
Deployment rollback restores one previously verified release as a coherent
unit.
