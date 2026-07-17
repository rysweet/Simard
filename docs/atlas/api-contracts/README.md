# Code Atlas — API Contracts

The **public contract surface** of Simard: the axum HTTP/WebSocket routes served
by the operator dashboard, the line-delimited JSON-RPC wire protocol, and the
serde IPC message enums. This is the boundary other processes and operators
program against, so it is enumerated **in full** (58 REST + 3 WS, not sampled).

Every route, method, and enum variant below traces to Rust source truth via a
`file:line` anchor in [Evidence anchors](#evidence-anchors). Host names, session
IDs, tokens, and other infrastructure identifiers are shown as `{placeholders}`,
never as real values.

## Diagrams

![API contracts — Graphviz](api-contracts-dot.svg)

![API contracts — Mermaid](api-contracts-mermaid.svg)

> Rendering: `api-contracts-dot.svg` is produced by `dot -Tsvg`;
> `api-contracts-mermaid.svg` by `mmdc` (mermaid-cli 10+ takes sandbox flags via
> a puppeteer config file, e.g. `-p pp.json` where `pp.json` is
> `{"args":["--no-sandbox"]}`). If `mmdc` fails in a
> sandboxed environment, the Mermaid SVG falls back to a `dot`-rendered copy and
> the failure is noted here.

## Trust boundary

Every dashboard route is registered inside the `require_auth` middleware layer
(`build_router().layer(middleware::from_fn(require_auth))`,
`src/operator_commands_dashboard/routes.rs:124`). `require_auth`
(`src/operator_commands_dashboard/auth.rs:76`) accepts three credentials:

1. `simard_session=<token>` cookie (`auth.rs:104`)
2. `Authorization: Bearer <token>` header, matched against
   `SIMARD_DASHBOARD_TOKEN` or an issued session (`auth.rs:111-122`)
3. `?token=<token>` query param (legacy) (`auth.rs:127-131`)

**Pre-auth (public) routes** — the only paths the middleware lets through
unauthenticated (`auth.rs:79-82`):

- `POST /api/login` → `login` (`routes.rs:121`)
- `GET /login` → `login_page` (`routes.rs:122`)

If no login code is configured, **all** other requests are denied `401`
(`auth.rs:85-95`) — the boundary never silently opens.

## REST route inventory

Single source of truth: `build_router()` in
`src/operator_commands_dashboard/routes.rs:44-124`. Verbs shown are those wired
on each path; handler names are the imported functions (`routes.rs:7-42`).

### Status / metrics / monitoring

| Method | Path | Handler | Line |
|---|---|---|---|
| GET | `/api/status` | `status` | `routes.rs:46` |
| GET | `/api/issues` | `issues` | `routes.rs:47` |
| GET | `/api/metrics` | `metrics` | `routes.rs:48` |
| GET | `/api/costs` | `costs` | `routes.rs:49` |
| GET / POST | `/api/budget` | `get_budget` / `set_budget` | `routes.rs:50` |
| GET | `/api/status/snapshot` | `status_snapshot` | `routes.rs:108` |
| GET | `/api/memory` | `memory_metrics` | `routes.rs:75` |
| GET | `/api/cognition/recall-precision` | `recall_precision_correlation` | `routes.rs:80-83` |

### Goals

| Method | Path | Handler | Line |
|---|---|---|---|
| GET / POST | `/api/goals` | `goals` / `add_goal` | `routes.rs:51` |
| POST | `/api/goals/seed` | `seed_goals` | `routes.rs:52` |
| POST | `/api/goals/promote/{id}` | `promote_backlog_item` | `routes.rs:53` |
| POST | `/api/goals/demote/{id}` | `demote_goal` | `routes.rs:54` |
| DELETE | `/api/goals/{id}` | `remove_goal` | `routes.rs:55` |
| PUT | `/api/goals/{id}/status` | `update_goal_status` | `routes.rs:56` |

### Distributed / hosts / VMs

| Method | Path | Handler | Line |
|---|---|---|---|
| GET | `/api/distributed` | `distributed` | `routes.rs:57` |
| POST | `/api/vm/vacate` | `vacate_vm` | `routes.rs:58` |
| GET / POST / DELETE | `/api/hosts` | `get_hosts` / `add_host` / `remove_host` | `routes.rs:59-62` |

### Registry / build lock / processes

| Method | Path | Handler | Line |
|---|---|---|---|
| GET | `/api/logs` | `logs` | `routes.rs:63` |
| GET | `/api/processes` | `processes` | `routes.rs:64` |
| GET / POST / DELETE | `/api/registry` | `registry_list` / `registry_register` / `registry_deregister` | `routes.rs:65-70` |
| POST | `/api/registry/reap` | `registry_reap` | `routes.rs:71` |
| GET | `/api/agent-graph` | `agent_graph` | `routes.rs:72` |
| GET | `/api/build-lock` | `build_lock_status` | `routes.rs:73` |
| POST | `/api/build-lock/release` | `build_lock_force_release` | `routes.rs:74` |

### Memory

| Method | Path | Handler | Line |
|---|---|---|---|
| GET | `/api/memory/recent` | `memory_recent` | `routes.rs:76` |
| GET | `/api/memory/history` | `memory_history` | `routes.rs:77` |
| POST | `/api/memory/search` | `memory_search` | `routes.rs:78` |
| GET | `/api/memory/graph` | `memory_graph` | `routes.rs:79` |

### Work / OODA / overseer / PRs

| Method | Path | Handler | Line |
|---|---|---|---|
| GET | `/api/enrichment` | `enrichment` | `routes.rs:84` |
| GET | `/api/merge-judge` | `merge_judge_decisions` | `routes.rs:85` |
| GET | `/api/merge-readiness` | `merge_readiness` | `routes.rs:86` |
| GET | `/api/traces` | `traces` | `routes.rs:87` |
| GET | `/api/activity` | `activity` | `routes.rs:88` |
| GET | `/api/workboard` | `workboard` | `routes.rs:89` |
| GET | `/api/current-work` | `current_work` | `routes.rs:90` |
| GET | `/api/ooda-thinking` | `ooda_thinking` | `routes.rs:91` |
| GET | `/api/ooda-cycles` | `ooda_cycles` | `routes.rs:92` |
| GET | `/api/brain-failures` | `brain_failures` | `routes.rs:93` |
| GET | `/api/overseer` | `overseer` | `routes.rs:94` |
| GET | `/api/prs` | `pr_readiness` | `routes.rs:95` |

### Journal

| Method | Path | Handler | Line |
|---|---|---|---|
| GET | `/api/journal/dates` | `journal_dates` | `routes.rs:96` |
| POST | `/api/journal/search` | `journal_search` | `routes.rs:97` |
| GET | `/api/journal/entry/{date}` | `journal_entry` | `routes.rs:98` |
| GET | `/api/journal/render/{date}` | `journal_render` | `routes.rs:99` |

### Creative ideas

| Method | Path | Handler | Line |
|---|---|---|---|
| GET | `/api/creative-ideas` | `creative_ideas` | `routes.rs:100` |
| POST | `/api/creative-ideas/search` | `creative_ideas_search` | `routes.rs:101` |
| POST | `/api/creative-ideas/run` | `creative_ideas_run` | `routes.rs:102` |
| POST | `/api/creative-ideas/{id}/promote` | `creative_ideas_promote` | `routes.rs:103-106` |
| POST | `/api/creative-ideas/{id}/prune` | `creative_ideas_prune` | `routes.rs:107` |

### Feedback / sessions / chat

| Method | Path | Handler | Line |
|---|---|---|---|
| POST | `/api/feedback` | `feedback_submit` | `routes.rs:109` |
| GET | `/api/feedback/status/{id}` | `feedback_status` | `routes.rs:110` |
| GET | `/api/subagent-sessions` | `subagent_sessions` | `routes.rs:111` |
| GET | `/api/chat/sessions` | `chat_sessions` | `routes.rs:112` |
| GET | `/api/chat/sessions/{id}` | `chat_session_by_id` | `routes.rs:113` |
| GET | `/api/azlin/tmux-sessions` | `azlin_tmux_sessions` | `routes.rs:116` |

### Auth + root (see [trust boundary](#trust-boundary))

| Method | Path | Handler | Auth | Line |
|---|---|---|---|---|
| POST | `/api/login` | `login` | **public** | `routes.rs:121` |
| GET | `/login` | `login_page` | **public** | `routes.rs:122` |
| GET | `/` | `index` | gated | `routes.rs:123` |

## WebSocket route inventory

All WS routes are also behind `require_auth`.

| Path | Handler | Line |
|---|---|---|
| `/ws/chat` | `ws_chat_handler` | `routes.rs:114` |
| `/ws/agent_log/{agent_name}` (`WS_AGENT_LOG_ROUTE`) | `ws_agent_log_handler` | `routes.rs:115`; const `agent_log.rs:19` |
| `/ws/tmux_attach/{host}/{session}` | `ws_tmux_attach_handler` | `routes.rs:117-120` |

> `{host}` and `{session}` in `/ws/tmux_attach/{host}/{session}` are user-supplied
> path parameters; real host/session identifiers are never depicted here.

## JSON-RPC wire contract

Simard talks to RPC servers with **one JSON object per line** on stdin/stdout
(`src/rpc.rs:11`).

**Request** `RpcRequest` (`src/rpc.rs:14`):

```json
{"id":"<uuid>","method":"<name>","params":{}}
```

**Response** `RpcResponse` (`src/rpc.rs:26`) — exactly one of `result` / `error`:

```json
{"id":"<uuid>","result":{}}
{"id":"<uuid>","error":{"code":-32601,"message":"..."}}
```

**Error payload** `RpcErrorPayload { code, message }` (`src/rpc.rs:35`).

### Error-code legend (`src/rpc.rs:41-44`)

| Constant | Code | Meaning |
|---|---|---|
| `RPC_ERROR_METHOD_NOT_FOUND` | `-32601` | Method not found |
| `RPC_ERROR_INTERNAL` | `-32603` | Internal error |
| `RPC_ERROR_TIMEOUT` | `-32000` | Timeout |
| `RPC_ERROR_TRANSPORT` | `-32001` | Transport failure |

### Well-known methods

| Method | Result type | Line |
|---|---|---|
| `bridge.health` | `RpcHealth { server_name, healthy }` | `src/rpc.rs:48`, `:78` |

## IPC message contracts (serde enums)

### `MemoryRequest` / `MemoryResponse` (framed JSON over `memory.sock`)

Full variant list is enumerated in the
[runtime-topology](../runtime-topology/README.md#memory-ipc-unix-domain-socket)
layer. Definitions: `MemoryRequest` (`src/memory_ipc/mod.rs:158`),
`MemoryResponse` (`src/memory_ipc/mod.rs:300`). Frame cap `MAX_FRAME = 8 MiB`
(`src/memory_ipc/mod.rs:352`).

### `IpcMessage` (subprocess coordination)

`src/runtime_ipc/mod.rs:20` — serde `#[serde(tag = "type", rename_all = "snake_case")]`:

| Variant | Wire shape |
|---|---|
| `Ping` | `{"type":"ping"}` |
| `Pong` | `{"type":"pong"}` |
| `TaskAssign` | `{"type":"task_assign","id":"…","objective":"…"}` |
| `TaskResult` | `{"type":"task_result","id":"…","outcome":"…"}` |
| `Shutdown` | `{"type":"shutdown"}` |

Serialized via `IpcMessage::to_bytes` / `from_bytes`
(`src/runtime_ipc/mod.rs:29-35`).

## Contract counts

- **REST paths**: 58 (`routes.rs:46-123`), several multiplexing multiple verbs.
- **WebSocket paths**: 3 (`routes.rs:114-120`).
- **Public (pre-auth) paths**: 2 (`/api/login`, `/login`).
- **JSON-RPC error codes**: 4 (`rpc.rs:41-44`).
- **`IpcMessage` variants**: 5 (`runtime_ipc/mod.rs:20-25`).

## Evidence anchors

- `src/operator_commands_dashboard/routes.rs:44` — `pub fn build_router()`
- `src/operator_commands_dashboard/routes.rs:46-123` — all `.route(...)` registrations
- `src/operator_commands_dashboard/routes.rs:114` — `/ws/chat`
- `src/operator_commands_dashboard/routes.rs:115` — `WS_AGENT_LOG_ROUTE`
- `src/operator_commands_dashboard/routes.rs:117-120` — `/ws/tmux_attach/{host}/{session}`
- `src/operator_commands_dashboard/routes.rs:121-123` — `/api/login`, `/login`, `/`
- `src/operator_commands_dashboard/routes.rs:124` — `.layer(middleware::from_fn(require_auth))`
- `src/operator_commands_dashboard/agent_log.rs:19` — `WS_AGENT_LOG_ROUTE = "/ws/agent_log/{agent_name}"`
- `src/operator_commands_dashboard/auth.rs:76` — `pub async fn require_auth(...)`
- `src/operator_commands_dashboard/auth.rs:79-82` — pre-auth allowlist (`/login`, `/api/login`)
- `src/operator_commands_dashboard/auth.rs:85-95` — deny-all when no login code configured
- `src/operator_commands_dashboard/auth.rs:104` — `simard_session=` cookie check
- `src/operator_commands_dashboard/auth.rs:111-122` — `Bearer` token check
- `src/operator_commands_dashboard/auth.rs:127-131` — legacy `?token=` query check
- `src/rpc.rs:11` — wire format: one JSON object per line
- `src/rpc.rs:14` — `pub struct RpcRequest`
- `src/rpc.rs:26` — `pub struct RpcResponse`
- `src/rpc.rs:35` — `pub struct RpcErrorPayload`
- `src/rpc.rs:41-44` — error-code constants
- `src/rpc.rs:48` — `pub struct RpcHealth`
- `src/rpc.rs:78` — `bridge.health` method name
- `src/memory_ipc/mod.rs:158` — `pub enum MemoryRequest`
- `src/memory_ipc/mod.rs:300` — `pub enum MemoryResponse`
- `src/memory_ipc/mod.rs:352` — `MAX_FRAME = 8 * 1024 * 1024`
- `src/runtime_ipc/mod.rs:20` — `pub enum IpcMessage`
- `src/runtime_ipc/mod.rs:29-35` — `to_bytes` / `from_bytes`

## Regeneration

```bash
# From repo root. Requires graphviz (dot) and mermaid CLI (mmdc); no Python/kuzu.
dot -Tsvg docs/atlas/api-contracts/api-contracts.dot \
    -o docs/atlas/api-contracts/api-contracts-dot.svg
# mermaid-cli 10+ passes sandbox flags via a puppeteer config, not --no-sandbox:
echo '{"args":["--no-sandbox","--disable-setuid-sandbox"]}' > /tmp/mmdc-pp.json
mmdc -p /tmp/mmdc-pp.json \
    -i docs/atlas/api-contracts/api-contracts-mermaid.mmd \
    -o docs/atlas/api-contracts/api-contracts-mermaid.svg
# On mmdc failure, fall back:
# dot -Tsvg docs/atlas/api-contracts/api-contracts.dot \
#     -o docs/atlas/api-contracts/api-contracts-mermaid.svg
```
