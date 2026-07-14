---
title: Grant engineer permissions
description: Configure and verify the permission ceiling used by typed OODA engineer spawns.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/engineer-copilot-permissions.md
  - ../reference/ooda-capability-api.md
  - ./spawn-engineers-from-ooda-daemon.md
---

# Grant engineer permissions

Typed OODA engineers receive the permissions named by the admitted
`SpawnEngineer` action. The installed capability policy defines the maximum set
that an action may request.

This scope applies only when `SIMARD_ENGINEER_PERMISSIONS` is present. Legacy
operator-created engineer sessions that do not carry a typed permission set
retain their existing broad Copilot launch contract.

## 1. Edit the capability policy

Edit:

```text
$SIMARD_HOME/prompt_assets/simard/policies/goal-session-capabilities.toml
```

Set the top-level permission ceiling:

```toml
engineer_permissions = [
  "repo_read",
  "repo_write",
  "process_exec",
  "github_pr_write",
]
```

The recognized permission names are:

| Permission | Copilot adapter |
| --- | --- |
| `repo_read` | `--allow-tool=read` and `--allow-tool=search` |
| `repo_write` | `--allow-tool=write` |
| `process_exec` | Simard process-broker MCP tool |
| `github_issue_write` | GitHub MCP with `create_issue` |
| `github_pr_write` | GitHub MCP with pull-request create/update tools |

`process_exec` never enables Copilot's built-in shell tool. Each command uses a
typed request ID and consumes one transactionally reserved per-cycle slot.

## 2. Restrict repository scope

Use exact repositories where possible:

```toml
repositories = [
  { owner = "rysweet", name = "Simard" },
]
repository_owners = []
```

An action that names a different repository than its authenticated actor
binding is rejected even when the repository owner is allowed by policy.

## 3. Request only the required subset

The actor supplies the requested subset in the typed action:

```json
{
  "kind": "spawn_engineer",
  "task": {
    "encoding": "base64",
    "data": "Rml4IHRoZSB0eXBlZCBPT0RBIHJlcGxheSByZWdyZXNzaW9uLgo="
  },
  "repository": {"owner": "rysweet", "name": "Simard"},
  "base_type": "copilot",
  "requested_permissions": [
    "repo_read",
    "repo_write",
    "process_exec",
    "github_pr_write"
  ],
  "claim_key": "rysweet/Simard:goal-4052"
}
```

The capability handler rejects an empty set or any permission outside
`engineer_permissions`. It does not silently narrow the request.

The live typed spawn path currently supports only `base_type = "copilot"`.
Although the wire enum also contains `rusty_clawd`, the production effect
executor rejects it.

## 4. Verify the recorded scope

Run a cycle, then inspect the durable action:

```bash
simard ooda outcomes list --state-root "$SIMARD_STATE_ROOT" --limit 10 |
  jq '
    .outcomes[]
    | select(.payload.action.kind == "spawn_engineer")
    | {
        base_type: .payload.action.base_type,
        permissions: .payload.action.requested_permissions,
        repository: .payload.action.repository
      }
  '
```

For a running typed engineer, Simard sets
`SIMARD_ENGINEER_PERMISSIONS=<comma-separated-set>`. The Copilot launcher then:

- omits `--allow-all-tools` and `--allow-all-paths`;
- removes `COPILOT_ALLOW_ALL`;
- disables temporary-directory and remote/export access;
- exposes only the adapters listed in the table above;
- removes common provider secrets;
- removes GitHub and SSH credentials unless issue or PR write access was
  requested.

## Troubleshooting

### The action returns `permission_denied`

Confirm the requested set is a subset of `engineer_permissions`, the action
repository matches the actor's bound repository, and the repository is governed
by the policy.

### The effect reports an unsupported base type

Use `copilot` for the live typed path. Adding another enum value or policy entry
does not add a production launcher.

### A typed engineer receives broad Copilot flags

Check that `SIMARD_ENGINEER_PERMISSIONS` reached the engineer process. The direct
spawn path and tmux path both propagate it. A missing variable selects the
legacy broad-permission contract and must be treated as a dispatch defect for a
typed OODA engineer.

### A command is denied

Add `process_exec` only when shell execution is required. There is currently no
per-command allowlist or mutation cap, so granting it is broader than granting a
single executable.
