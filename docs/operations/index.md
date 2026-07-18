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
| [agent-kgpacks-rs Parity Goal — Operator Update](agent-kgpacks-rs-parity-goal-signal-2026-07-18.md) | Plain-English finish line for the Rust knowledge-lookup rewrite goal (issue #4321 + done-gate check) |

Related how-to guides:

| Guide | Topic |
|---|---|
| [Diagnose handoff accumulation](../howto/diagnose-handoff-accumulation.md) | Detect, resolve, prevent handoff file buildup (#2268) |

For contributor workflow (branching, merge policy, PR evidence
requirements), see [`CONTRIBUTING.md`](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md) at the
repo root.
