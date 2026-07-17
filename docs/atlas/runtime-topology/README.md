# Runtime Topology Atlas Layer

This layer maps the running Simard deployment as a single native Rust daemon (`simard ooda run`) supervised by the user-level `simard-ooda.service`. The daemon owns the OODA loop, dashboard, embedded Signal channel, memory IPC server, cognitive threads, and overseer. External work is coordinated through subprocesses (`recipe-runner-rs`, `amplihack`, `gh`, `git`, `systemctl`, `tmux`, `journalctl`, `curl`, `cargo`) and local sockets/ports.

![Runtime topology DOT](runtime-topology-dot.svg)

![Runtime topology Mermaid](runtime-topology-mermaid.svg)

## Source anchors

- CLI entrypoint: `src/main.rs:1` dispatches all commands through `dispatch_operator_cli`.
- OODA daemon command: `src/operator_cli/ooda.rs:31` and `src/operator_cli/ooda.rs:35` route `simard ooda run` to `run_ooda_daemon`.
- Daemon wiring: `src/operator_commands_ooda/daemon/mod.rs:173`, `src/operator_commands_ooda/daemon/mod.rs:200`, `src/operator_commands_ooda/daemon/mod.rs:260`, `src/operator_commands_ooda/daemon/mod.rs:270`, `src/operator_commands_ooda/daemon/mod.rs:625`, `src/operator_commands_ooda/daemon/mod.rs:646`.
- Dashboard server: `src/operator_commands_dashboard/mod.rs:253` and `src/operator_commands_dashboard/mod.rs:296` bind Axum listeners.
- Signal TCP transport: `src/signal_conversation/transport.rs:177` and `src/signal_conversation/transport.rs:214`.
- Memory IPC socket: `src/memory_ipc/mod.rs:70`, `src/memory_ipc/mod.rs:90`, `src/memory_ipc/server.rs:53`, `src/memory_ipc/server.rs:137`.
- Systemd unit renderer: `src/install/systemd.rs:22`, `src/install/systemd.rs:102`.

## Runtime inventory

| Component | Kind | Endpoint | Protocol | Source file |
|---|---|---|---|---|
| `simard-ooda.service` | unit | user systemd unit | `ExecStart={binary} ooda run`; `Restart=always` | `src/install/systemd.rs:22`, `src/install/systemd.rs:102` |
| `simard` daemon | process | `simard ooda run [--cycles=N] [--dashboard-port=PORT] [state-root]` | native Rust CLI dispatch | `src/operator_cli/ooda.rs:31`, `src/operator_cli/ooda.rs:35` |
| OODA loop / scheduler | process-internal subsystem | daemon thread/main loop | in-process Rust calls over `OodaClients` | `src/operator_commands_ooda/daemon/mod.rs:173`, `src/operator_commands_ooda/daemon/mod.rs:294` |
| Embedded dashboard | process-internal task | `0.0.0.0:{port}`, default `8080` | HTTP + WebSocket via Axum | `src/operator_commands_ooda/daemon/mod.rs:625`, `src/operator_commands_dashboard/mod.rs:253` |
| Standalone dashboard | process mode | `simard dashboard serve --port=PORT`, default `8080` | HTTP + WebSocket via Axum | `src/operator_cli/dashboard.rs:15`, `src/operator_commands_dashboard/mod.rs:263` |
| Dashboard auth key | storage | `~/.simard/.dashkey` | local file; session cookie / bearer token guard | `src/operator_commands_dashboard/auth.rs:13`, `src/operator_commands_dashboard/auth.rs:68` |
| Embedded Signal channel | process-internal thread | enabled unless `SIMARD_SIGNAL_ENABLED=0/false/no/off` | background Tokio runtime; reconnect loop | `src/operator_commands_ooda/daemon/signal_embed.rs:38`, `src/operator_commands_ooda/daemon/signal_embed.rs:86` |
| signal-cli daemon | external service | default `127.0.0.1:7583` | newline-delimited JSON-RPC 2.0 over TCP | `src/signal_conversation/config.rs:17`, `src/signal_conversation/transport.rs:214` |
| Overseer Signal notifier | network client | default `127.0.0.1:7583` via `SIMARD_SIGNAL_RPC_ADDR` | JSON-RPC over TCP | `src/overseer/notify.rs:967`, `src/overseer/notify.rs:987` |
| Loopback probe listener | port | `127.0.0.1:0` | local TCP listener for JSON-RPC tests/probes | `src/overseer/notify.rs:1524`, `src/overseer/notify.rs:1693` |
| Memory IPC server | process-internal thread | `<state_root>/memory.sock` or `SIMARD_MEMORY_SOCKET` | Unix-domain socket, 4-byte length-prefixed JSON frames | `src/memory_ipc/mod.rs:90`, `src/memory_ipc/server.rs:53`, `src/memory_ipc/mod.rs:352` |
| lbug memory store | storage | `<state_root>/cognitive` (via memory library) | embedded graph store through `CognitiveMemoryOps` | `src/memory_ipc/launcher.rs:252`, `src/memory_ipc/launcher.rs:285` |
| `recipe-runner-rs` | spawned process | resolved binary / PATH | subprocess stdout/stderr; JSON/text recipe envelopes | `src/ooda_brain/recipe_brain.rs:888`, `src/journal/recipe.rs:186`, `src/typed_ooda/route.rs:274` |
| `amplihack` | spawned process | `amplihack recipe run smart-orchestrator` | subprocess stdout/stderr | `src/overseer/launch.rs:225`, `src/overseer/launch.rs:347` |
| LLM provider agent | spawned process | provider binary from config | subprocess stdin/stdout; recipe-runner child | `src/typed_ooda/route.rs:246`, `src/meeting_backend/agent_proxy.rs:336` |
| `gh` | spawned process | GitHub CLI | JSON stdout for issues, PRs, CI, merge evidence | `src/operator_commands_dashboard/routes.rs:249`, `src/stewardship/gh_client.rs:269` |
| `git` | spawned process | Git CLI | subprocess stdout/stderr for repo status, worktrees, diffs | `src/ooda_loop/observe.rs:19`, `src/overseer/conflict.rs:42` |
| `systemctl` | spawned process | user systemd manager | subprocess stdout/stderr | `src/install/systemd.rs:35`, `src/self_deploy/restart.rs:167` |
| `tmux` | spawned process | local tmux server | subprocess stdout/stderr; dashboard WebSocket attach uses session names | `src/subagent_sessions/mod.rs:74`, `src/operator_commands_dashboard/tmux.rs:237` |
| `journalctl` | spawned process | user journal | subprocess stdout for logs/activity | `src/operator_commands_dashboard/logs.rs:177`, `src/bin/simard_tui/app.rs:830` |
| `curl` | spawned process | release/download endpoints | subprocess stdout/stderr | `src/cmd_self_update/download.rs:178`, `src/cmd_self_update/release.rs:20` |
| `cargo` | spawned process | build/test/canary gates | subprocess stdout/stderr | `src/self_relaunch/canary.rs:30`, `src/self_relaunch/gates.rs:64` |

## Protocol notes

- Dashboard traffic is protected by `require_auth`; `/login` and `/api/login` are explicit bypasses, while API and WebSocket paths return `401` when unauthenticated (`src/operator_commands_dashboard/auth.rs:68`).
- Memory IPC uses serde-tagged `MemoryRequest` / `MemoryResponse` JSON frames and rejects frames above `MAX_FRAME = 8 MiB` before allocation (`src/memory_ipc/mod.rs:155`, `src/memory_ipc/mod.rs:297`, `src/memory_ipc/mod.rs:352`).
- Signal high-risk remote commands (`deploy`, `merge #NNNN`) are not executed directly from text; they become pending sign-off and require `approve` (`src/signal_conversation/gating.rs:95`, `src/signal_conversation/gating.rs:108`).
