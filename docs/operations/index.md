# Operations

Operational documentation for running, maintaining, and verifying a
Simard deployment.

| Page | Topic |
|---|---|
| [Pre-Commit Setup](pre-commit-setup.md) | Local hooks that mirror CI |
| [Cognitive Memory Durability](cognitive-memory-durability.md) | SIGTERM-safe shutdown + periodic backups |
| [Verified Backups of the Live Cognitive Store](verified-backups.md) | Live-store backups, verify-before-prune, bounded quarantines (#2420) |
| [Cognitive-Memory WAL Recovery Runbook](cognitive-memory-wal-recovery-runbook.md) | Corrupt-WAL recovery, `memory import`, startup auto-restore, asset preservation (#2550) |
| [Meeting REPL & Handoff Ingestion](meeting-handoffs.md) | Routing operator intent into the OODA loop |
| [Progress-Evidence Kill Switch](progress-evidence-kill-switch.md) | `SIMARD_PROGRESS_EVIDENCE=off` and when to use it |

Related reference pages:

| Page | Topic |
|---|---|
| [Typed-OODA ledger concurrency hardening](../reference/typed-ooda-ledger-concurrency.md) | WAL + 30s busy_timeout applied at every ledger open, Immediate write txns, fail-visible lock propagation (#4483) |
| [Deploy-gate canary unit-test stage](../reference/deploy-gate-unit-test-canary.md) | The self-deploy canary unit-test gate and the exit-101 red-canary root-cause fix (#4470/#4471/#4481/#4475) |
| [Gym self-eval status wiring](../reference/gym-self-eval-status.md) | Real scenario count + non-idle self-eval in `simard status` |

Related how-to guides:

| Guide | Topic |
|---|---|
| [Diagnose handoff accumulation](../howto/diagnose-handoff-accumulation.md) | Detect, resolve, prevent handoff file buildup (#2268) |
| [Diagnose a typed-OODA "database is locked" crash-loop](../howto/diagnose-typed-ooda-database-locked.md) | Confirm WAL + busy_timeout, clear the persistence crash-loop (#4483) |

For contributor workflow (branching, merge policy, PR evidence
requirements), see [`CONTRIBUTING.md`](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md) at the
repo root.
