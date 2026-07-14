---
title: Engineer Copilot permissions
description: Typed engineer permission propagation and the resulting Copilot CLI launch contract.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../howto/grant-engineer-write-permissions.md
  - ./ooda-capability-api.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
---

# Engineer Copilot permissions

Simard has two Copilot engineer launch contracts:

| Launch | Permission source | Copilot behavior |
| --- | --- | --- |
| Typed OODA spawn | `SpawnEngineer.requested_permissions` propagated through `SIMARD_ENGINEER_PERMISSIONS` | Scoped tool adapters; no allow-all flags |
| Legacy/operator spawn | No typed permission environment | Least privilege; no tools or paths are implicitly allowed |

Do not describe the legacy contract as the security boundary for typed OODA.

## Admission

A typed `SpawnEngineer` action is accepted only when:

1. the actor has the `record_action.spawn_engineer` grant;
2. the action repository matches the actor's bound repository;
3. the repository is allowed by policy;
4. at least one permission is requested;
5. every requested permission is in the intersection of actor scope, action
   scope, the canonical Copilot engineer base type, and policy;
6. the claim key is `<owner>/<repository>:<goal_id>`;
7. concurrency, disk, and active-claim admission pass.

Unknown, broader, or cross-scope permissions are rejected before admission.

## Scoped Copilot argv

When `SIMARD_ENGINEER_PERMISSIONS` is present, the Copilot launcher always adds:

```text
--subprocess-safe
--disallow-temp-dir
--no-remote
--no-remote-export
--secret-env-vars=GH_TOKEN,GITHUB_TOKEN,AZURE_CLIENT_SECRET,OPENAI_API_KEY,ANTHROPIC_API_KEY,AWS_SECRET_ACCESS_KEY
```

It then maps permissions:

| Permission | Additional argv |
| --- | --- |
| `repo_read` | `--allow-tool=read`, `--allow-tool=search` |
| `repo_write` | `--allow-tool=write` |
| `process_exec` | `simard-process-broker(process_exec)` via scoped MCP config |
| `github_issue_write` | `--allow-tool=github-mcp-server`, `--add-github-mcp-tool=create_issue` |
| `github_pr_write` | `--allow-tool=github-mcp-server`, pull-request create/update tools |

Without either GitHub write permission, built-in MCPs are disabled and the
supervisor removes `GH_TOKEN`, `GITHUB_TOKEN`, `SSH_AUTH_SOCK`,
`GIT_ASKPASS`, and `SSH_ASKPASS`.

The supervisor also removes common provider secrets from typed child processes:
`AZURE_CLIENT_SECRET`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, and
`AWS_SECRET_ACCESS_KEY`.

## Process authority

`process_exec` does not grant Copilot's shell tool. Simard reserves individual
commands in the shared SQLite request registry, enforces the policy's
`process_exec_mutations_per_cycle` cap, and replays only identical request
payloads. Running or indeterminate records are returned without re-execution.

Repository worktree placement and the child process UID remain additional OS
boundaries, but they are not substitutes for a command-level broker.

## Propagation

The typed effect executor stores the requested set in
`SubordinateConfig.requested_permissions`. Both subordinate launch paths carry
the same comma-separated environment value:

```text
SIMARD_ENGINEER_PERMISSIONS=github_pr_write,process_exec,repo_read,repo_write
```

- direct spawn sets it on the child `Command`;
- tmux spawn includes it with `tmux new-session -e`.

`run_engineer_subprocess` reads the value and builds the scoped argv. If the
variable is absent, it intentionally preserves legacy behavior.

## Policy

The relevant installed policy fields are:

```toml
repositories = [
  { owner = "rysweet", name = "Simard" },
]
repository_owners = ["rysweet"]

engineer_permissions = [
  "repo_read",
  "repo_write",
  "process_exec",
  "github_issue_write",
  "github_pr_write",
]

[limits]
max_concurrent_engineers = 8
max_disk_used_percent = 90
process_exec_mutations_per_cycle = 8
```

The canonical engineer base type is `copilot`. The action's requested set,
authenticated actor set, and policy set are intersected before dispatch.

## Errors

| Condition | Result |
| --- | --- |
| Empty permission set | `invalid_argument` |
| Permission outside policy | `permission_denied` |
| Cross-repository action | `permission_denied` |
| Duplicate active claim or concurrency/disk limit | `admission_rejected` |
| Non-Copilot live typed spawn | Permanent downstream effect failure |
| Missing scoped environment in a typed child | Legacy broad contract; dispatch defect |
