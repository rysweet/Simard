# Event Bus Stats in Cluster Topology

The Cluster Topology panel of the operator dashboard surfaces live statistics
for the in-process `HiveEventBus` alongside the existing distributed-node
view. This document describes the user-facing behavior, the JSON contract
exposed through `/api/distributed`, the DOM contract used by automated tests,
and the runtime semantics of the underlying stats collector.

## Overview

`HiveEventBus` is the process-wide Tokio `broadcast` channel that fans
hive-mind events (fact promotions, node membership, memory sync requests,
etc.) out to in-process subscribers. Until now its activity was invisible to
operators. This feature adds a lightweight, always-on stats collector and
exposes a snapshot via the dashboard so operators can answer:

- "Is anyone listening to the bus right now?"
- "How busy has the bus been over the last five minutes?"
- "When did we last see an event of kind *X*?"

The feature is **purely additive**: publish/subscribe semantics, the event
envelope, existing JSON keys on `/api/distributed`, and authorization
behavior are unchanged. Older API consumers that do not know about
`event_bus` continue to work without modification.

## Where the data appears

### Dashboard

Open the operator dashboard and select the **Cluster Topology** tab. Below
the existing distributed-node view, an **Event Bus** block is rendered:

```
Event Bus
  Subscribers: 3
  Events/min: 1.6
  Last event: 2026-04-19T15:50:00Z

  fact_promoted:         3 subs, 1.2/min, last 2026-04-19T15:50:00Z
  fact_imported:         3 subs, 0.0/min, last —
  node_joined:           3 subs, 0.4/min, last 2026-04-19T15:48:00Z
  node_left:             3 subs, 0.0/min, last —
  memory_sync_requested: 3 subs, 0.0/min, last —
```

If the server is older than this feature (or if the API ever omits the
`event_bus` key), the block is silently not rendered — the rest of the
panel continues to function normally.

### API

`GET /api/distributed` returns its existing payload with one additional
top-level key, `event_bus`:

```json
{
  "...existing keys unchanged...": "...",
  "event_bus": {
    "topics": {
      "fact_promoted": {
        "subscribers": 3,
        "events_per_min": 1.2,
        "last_event_timestamp": "2026-04-19T15:50:00Z"
      },
      "fact_imported": {
        "subscribers": 3,
        "events_per_min": 0.0,
        "last_event_timestamp": null
      },
      "node_joined": {
        "subscribers": 3,
        "events_per_min": 0.4,
        "last_event_timestamp": "2026-04-19T15:48:00Z"
      },
      "node_left": {
        "subscribers": 3,
        "events_per_min": 0.0,
        "last_event_timestamp": null
      },
      "memory_sync_requested": {
        "subscribers": 3,
        "events_per_min": 0.0,
        "last_event_timestamp": null
      }
    },
    "total_subscribers": 3,
    "events_per_min": 1.6,
    "last_event_timestamp": "2026-04-19T15:50:00Z"
  }
}
```

#### Field reference

| Field | Type | Meaning |
|-------|------|---------|
| `event_bus.topics` | object | Map of topic name → per-topic stats. On a current server this map always contains every known `HiveEventKind` topic (zeroed if no events have been published). Renderers must still tolerate missing entries to remain compatible with older servers, partial mocks, and the `unknown` fallback. |
| `event_bus.topics.<topic>.subscribers` | integer | Current subscriber count. Because `HiveEventBus` uses a Tokio `broadcast` channel (fanout), this reflects the *global* receiver count and is therefore identical for every topic. Documented honestly here and in code comments. |
| `event_bus.topics.<topic>.events_per_min` | float | `events_in_last_5_min / 5.0`. Always non-negative. |
| `event_bus.topics.<topic>.last_event_timestamp` | RFC 3339 string or `null` | Timestamp of the most recent publish on that topic, or `null` if none has ever been published in this process. Renders as the em-dash literal `—` (U+2014) wherever displayed. |
| `event_bus.total_subscribers` | integer | Current global subscriber count. |
| `event_bus.events_per_min` | float | Aggregate rate: total events published across all topics in the last five minutes, divided by `5.0`. Equivalent to the sum of every topic's `events_per_min`. |
| `event_bus.last_event_timestamp` | RFC 3339 string or `null` | Timestamp of the most recent publish on any topic. Renders as the em-dash literal `—` (U+2014) when `null`. |

#### Known topics

| Topic key | Source variant |
|-----------|----------------|
| `fact_promoted` | `HiveEventKind::FactPromoted` |
| `fact_imported` | `HiveEventKind::FactImported` |
| `node_joined` | `HiveEventKind::NodeJoined` |
| `node_left` | `HiveEventKind::NodeLeft` |
| `memory_sync_requested` | `HiveEventKind::MemorySyncRequested` |
| `unknown` | Fallback for any future `#[non_exhaustive]` variant not yet mapped. Will appear only after a publish of an unmapped variant. |

## Runtime semantics

- **Window**: rolling five minutes. Each topic maintains a `VecDeque<Instant>`
  of recent publish times that is pruned (entries older than `now − 5min`)
  on every read and write.
- **Storage**: a single process-wide `BusStats` singleton, lazily initialized
  via `OnceLock<Arc<BusStats>>`. No state is threaded through Axum.
- **Concurrency**: a single mutex protects the inner state. The publish
  critical section is just a `VecDeque::push_back`, a counter bump, and an
  `Option<DateTime<Utc>>` write — no I/O, no allocation in steady state.
- **Snapshot consistency**: `stats_snapshot()` takes the mutex once and
  returns a fully consistent view across topics; readers never observe a
  torn cross-topic state.
- **Mutex poisoning**: if a publisher panics while holding the stats mutex,
  the handler recovers the inner state via `lock().unwrap_or_else(|e|
  e.into_inner())` and continues to serve snapshots from the last-known
  state. The `/api/distributed` handler never fails because of bus-stats
  poisoning.
- **Persistence**: none. Stats are in-memory only and reset when the process
  restarts. There is no rolling history beyond the five-minute window.

## DOM contract (for tests and external automation)

Inside `#cluster-topology`, the following stable `data-testid` selectors are
guaranteed when the `event_bus` payload is present:

| Selector | Element |
|----------|---------|
| `event-bus-total-subscribers` | Aggregate subscriber count line. |
| `event-bus-events-per-min` | Aggregate events-per-minute line. |
| `event-bus-last-event` | Aggregate last-event line. Renders the RFC 3339 timestamp string, or the em-dash literal `—` (U+2014) when `null`. Tests assert this exact character. |
| `event-bus-topic-fact_promoted` | Per-topic line for `fact_promoted`. |
| `event-bus-topic-fact_imported` | Per-topic line for `fact_imported`. |
| `event-bus-topic-node_joined` | Per-topic line for `node_joined`. |
| `event-bus-topic-node_left` | Per-topic line for `node_left`. |
| `event-bus-topic-memory_sync_requested` | Per-topic line for `memory_sync_requested`. |

When the API omits `event_bus` (older server), none of these selectors are
rendered. The dashboard JS uses optional chaining (`d.event_bus?.…`) so the
absence is silent.

## Configuration

This feature has **no configuration knobs**. The five-minute window, the
known topic set, and the always-on collector are baked in. There are no new
environment variables, CLI flags, or config-file keys.

## Examples

### Observing the bus from `curl`

```bash
curl -s http://localhost:8080/api/distributed | jq '.event_bus'
```

Sample output on an idle process:

```json
{
  "topics": {
    "fact_promoted":         {"subscribers": 0, "events_per_min": 0.0, "last_event_timestamp": null},
    "fact_imported":         {"subscribers": 0, "events_per_min": 0.0, "last_event_timestamp": null},
    "node_joined":           {"subscribers": 0, "events_per_min": 0.0, "last_event_timestamp": null},
    "node_left":             {"subscribers": 0, "events_per_min": 0.0, "last_event_timestamp": null},
    "memory_sync_requested": {"subscribers": 0, "events_per_min": 0.0, "last_event_timestamp": null}
  },
  "total_subscribers": 0,
  "events_per_min": 0.0,
  "last_event_timestamp": null
}
```

### Sanity-check a busy bus

```bash
# How many events per minute across all topics?
curl -s http://localhost:8080/api/distributed | jq '.event_bus.events_per_min'

# Which topic was most recently active?
curl -s http://localhost:8080/api/distributed \
  | jq '.event_bus.topics | to_entries
        | map(select(.value.last_event_timestamp != null))
        | sort_by(.value.last_event_timestamp) | last'
```

### Asserting in Playwright

A `@structural` Playwright spec at
`tests/e2e-dashboard/specs/cluster-topology.spec.ts` mocks `/api/distributed`
with a populated `event_bus` payload and asserts each `data-testid` selector
listed above is visible. The pre-existing `tabs.spec.ts` mock is patched to
include an empty `event_bus` block so the broader tab smoke test continues
to pass. The mock deliberately uses an empty `topics: {}` map to verify the
renderer tolerates missing per-topic entries; a real server always emits
every known topic key:

```ts
event_bus: {
  topics: {},
  total_subscribers: 0,
  events_per_min: 0.0,
  last_event_timestamp: null
}
```

## Compatibility & migration notes

- **Backward compatible.** Clients that don't read `event_bus` are
  unaffected; the dashboard JS treats the key as optional.
- **Forward compatible.** New `HiveEventKind` variants surface automatically
  under the `unknown` topic key until an explicit mapping is added in
  `HiveEventKind::topic()`.
- **No schema migration, no new endpoints, no auth changes.**

## Limitations

- Per-topic `subscribers` cannot be true per-topic counts — Tokio
  `broadcast` is fanout. The value mirrors the global subscriber count and
  is documented as such inline.
- Stats reset on process restart; there is no persisted history.
- The five-minute window is not user-configurable.

## For contributors

The process-wide `BusStats` singleton persists across in-process tests.
New unit tests that assert exact counts must either (a) use unique topic
strings to avoid collision with other tests, or (b) assert relative deltas
captured around the operation under test rather than absolute values.
