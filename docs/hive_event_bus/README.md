# Hive Event Bus

In-process publish/subscribe bus for distributing **hive events** between
simulated nodes inside a single Simard process. Backed by
[`tokio::sync::broadcast`], with no new external crate dependencies.

The bus is the foundational primitive for issue [#949] — wiring multi-node
hive coordination (fact promotion, fact import, node membership, memory
sync) without yet committing to a network transport.

> **Scope:** The bus is **in-process only**. It does not cross process or
> machine boundaries. A future workstream may add a network adapter that
> bridges this bus to a remote transport; nothing in this module assumes one.

[`tokio::sync::broadcast`]: https://docs.rs/tokio/latest/tokio/sync/broadcast/index.html
[#949]: https://github.com/rysweet/Simard/issues/949

---

## At a glance

```rust
use simard::hive_event_bus::{HiveEventBus, HiveEventKind};

# tokio_test::block_on(async {
let bus = HiveEventBus::new();           // default capacity (1024)
let mut rx = bus.subscribe();

bus.publish(HiveEventKind::NodeJoined { node_id: "node-a".into() });

let envelope = rx.recv().await?;
assert!(matches!(envelope.kind, HiveEventKind::NodeJoined { .. }));
# Ok::<_, Box<dyn std::error::Error>>(())
# });
```

- Multi-producer, multi-consumer: any number of publishers, any number of
  subscribers, all in-process.
- Every published event is wrapped in a [`HiveEventEnvelope`](#hiveeventenvelope)
  with a unique id and a UTC timestamp.
- Slow subscribers receive `RecvError::Lagged(n)` rather than blocking
  publishers — the bus prefers liveness over delivery guarantees.
- Late subscribers do **not** receive events published before they
  subscribed.
- `HiveEventBus` is `Send + Sync + Clone`; share it freely across tasks.

---

## Module layout

| Item                  | Kind   | Purpose                                                  |
| --------------------- | ------ | -------------------------------------------------------- |
| `HiveEventBus`        | struct | The bus handle. Cheap to clone — clones share a channel. |
| `HiveEventBusConfig`  | struct | Capacity configuration with sensible defaults.           |
| `HiveEventEnvelope`   | struct | `{ id, timestamp, kind }` wrapper around every event.    |
| `HiveEventKind`       | enum   | Typed event variants. `#[non_exhaustive]`.               |
| `BusError`            | enum   | Reserved for future fallible operations. `#[non_exhaustive]`. |

All items are re-exported from the crate root:

```rust
use simard::{HiveEventBus, HiveEventEnvelope, HiveEventKind, BusError};
```

---

## API reference

### `HiveEventBus`

```rust
impl HiveEventBus {
    /// Construct a bus with the default capacity (1024 events).
    pub fn new() -> Self;

    /// Construct a bus with a specific channel capacity.
    ///
    /// # Panics
    /// Panics if `capacity == 0`. The bus is meaningless without buffer
    /// space and a zero capacity is a programmer error, not a runtime
    /// condition.
    pub fn with_capacity(capacity: usize) -> Self;

    /// Construct from a [`HiveEventBusConfig`]. Forward-compatible entry
    /// point for adding more knobs without breaking callers.
    pub fn with_config(config: HiveEventBusConfig) -> Self;

    /// Publish an event.
    ///
    /// Wraps `kind` in a fresh [`HiveEventEnvelope`] (new id + `Utc::now()`)
    /// **once** and broadcasts the same envelope to every current
    /// subscriber. Returns the number of subscribers the event was
    /// delivered to.
    ///
    /// **Returns `0` (not an error) when there are no subscribers** —
    /// publishing into the void is a normal condition for this bus.
    /// `publish` is infallible in the current implementation.
    pub fn publish(&self, kind: HiveEventKind) -> usize;

    /// Subscribe to all *future* events. Each call returns an independent
    /// receiver — past events are not replayed.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<HiveEventEnvelope>;

    /// Number of currently active subscribers.
    pub fn subscriber_count(&self) -> usize;

    /// Channel capacity that this bus actually uses. Note that
    /// `tokio::sync::broadcast` rounds the requested capacity up to the
    /// next power of two, so `with_capacity(1000).capacity()` returns
    /// `1024`.
    pub fn capacity(&self) -> usize;
}

impl Clone for HiveEventBus { /* cheap, shares channel */ }
impl Default for HiveEventBus { /* equivalent to new() */ }
```

#### Cloning semantics

`HiveEventBus` wraps a `tokio::sync::broadcast::Sender`, which is itself
`Clone` and shares the underlying channel on clone. Cloning the bus
produces an additional handle to the **same** channel — publishing through
any clone reaches all subscribers of any other clone. No additional `Arc`
wrapping is required.

```rust
let bus = HiveEventBus::new();
let bus_for_node = bus.clone();
tokio::spawn(async move {
    bus_for_node.publish(HiveEventKind::NodeJoined { node_id: "n1".into() });
});
```

---

### `HiveEventBusConfig`

```rust
pub struct HiveEventBusConfig {
    pub capacity: usize,
}

impl Default for HiveEventBusConfig {
    fn default() -> Self { Self { capacity: 1024 } }
}
```

`capacity` is the per-subscriber ring buffer size. A subscriber that falls
more than `capacity` events behind will receive `RecvError::Lagged(n)` on
its next `recv()` and skip ahead to the newest available event.

The actual buffer size is `tokio::sync::broadcast`'s next-power-of-two
ceiling of `capacity`. Pass `1024` (a power of two) for an exact match.

**Choosing capacity:**

| Workload                                  | Suggested capacity |
| ----------------------------------------- | ------------------ |
| Tests, small simulations                  | 16 – 256           |
| Default                                   | **1024**           |
| Bursty publishers + slow consumers        | 4096+              |

There is no benefit to capacity `0`; it panics in `with_capacity`.

---

### `HiveEventEnvelope`

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HiveEventEnvelope {
    /// Stable unique identifier for this event instance.
    /// UUID v7 (time-ordered) — Simard already depends on `uuid` with the
    /// `v7` feature enabled.
    pub id: uuid::Uuid,

    /// UTC wall-clock time at which the event was published.
    pub timestamp: chrono::DateTime<chrono::Utc>,

    /// The typed payload.
    pub kind: HiveEventKind,
}

impl HiveEventEnvelope {
    /// Build a new envelope with a fresh id + `Utc::now()`.
    pub fn new(kind: HiveEventKind) -> Self;
}
```

Envelopes implement `Serialize` and `Deserialize` and round-trip cleanly
through JSON. This makes them suitable for logging, snapshot tests, and
future network transports without further wrapping.

`publish` constructs the envelope **once** and clones it to each
subscriber, so all subscribers observe the same `id` and `timestamp` for a
given publish call (see guarantee #4).

---

### `HiveEventKind`

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HiveEventKind {
    /// A fact has been promoted to higher confidence / wider scope.
    FactPromoted {
        fact_id: String,
        promoted_by: String,
    },

    /// A fact has been imported from an external source.
    FactImported {
        fact_id: String,
        source: String,
    },

    /// A node has joined the hive.
    NodeJoined {
        node_id: String,
    },

    /// A node has left the hive (graceful or detected absence).
    NodeLeft {
        node_id: String,
    },

    /// A node is requesting a memory synchronization.
    MemorySyncRequested {
        node_id: String,
        since: Option<chrono::DateTime<chrono::Utc>>,
    },
}
```

`#[non_exhaustive]` is enforced by the compiler: downstream `match`
statements **must** include a catch-all arm, so adding new variants is
backward-compatible.

---

### `BusError`

```rust
#[derive(Debug)]
#[non_exhaustive]
pub enum BusError {}

impl std::fmt::Display for BusError { /* unreachable in current impl */ }
impl std::error::Error for BusError {}
```

`BusError` is reserved for future fallible operations. The current
implementation has no fallible bus operations:

- `publish` returns `usize` (number of deliveries; `0` when no subscribers).
- `subscribe` is infallible.
- All other accessors are infallible.

The type is exported so that future additions (e.g. a fallible
`publish_with_timeout`) can return `Result<_, BusError>` without a breaking
API change. Hand-rolled `Display` + `Error` impls keep the bus free of any
new crate dependency (notably **no `thiserror`**).

When `BusError` does gain variants, their `Display`/`Debug`
implementations will be required to **never include the event payload**,
so that bus errors can be safely logged in contexts where event contents
may be sensitive (guarantee #6).

---

## Configuration

The bus has exactly one configurable knob: channel capacity.

```rust
use simard::hive_event_bus::{HiveEventBus, HiveEventBusConfig};

// 1. Default (1024)
let a = HiveEventBus::new();

// 2. Explicit capacity
let b = HiveEventBus::with_capacity(256);

// 3. Via config (preferred for code that grows more knobs later)
let c = HiveEventBus::with_config(HiveEventBusConfig { capacity: 4096 });
```

There are no environment variables, no global state, and no on-disk
configuration. A bus's lifetime is bounded by its handles.

---

## Usage tutorials

### Tutorial 1 — One publisher, one subscriber

```rust
use simard::{HiveEventBus, HiveEventKind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = HiveEventBus::new();
    let mut rx = bus.subscribe();

    bus.publish(HiveEventKind::FactImported {
        fact_id: "f-001".into(),
        source: "ingest-cli".into(),
    });

    let env = rx.recv().await?;
    println!("got event {} at {}", env.id, env.timestamp);
    Ok(())
}
```

### Tutorial 2 — Multiple simulated nodes

Each "node" is just a task that owns its own subscriber.

```rust
use simard::{HiveEventBus, HiveEventKind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = HiveEventBus::new();

    for node_id in ["n1", "n2", "n3"] {
        let mut rx = bus.subscribe();
        let id = node_id.to_string();
        tokio::spawn(async move {
            while let Ok(env) = rx.recv().await {
                println!("[{id}] received {:?}", env.kind);
            }
        });
    }

    // Yield once so subscribers are ready.
    tokio::task::yield_now().await;

    bus.publish(HiveEventKind::NodeJoined { node_id: "n4".into() });
    Ok(())
}
```

All three subscribers receive the same envelope (same `id`, same
`timestamp`).

### Tutorial 3 — Handling lag

A subscriber that cannot keep up is told so explicitly:

```rust
use simard::{HiveEventBus, HiveEventKind};
use tokio::sync::broadcast::error::RecvError;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = HiveEventBus::with_capacity(4);
    let mut slow = bus.subscribe();

    for i in 0..16 {
        bus.publish(HiveEventKind::FactImported {
            fact_id: format!("f-{i}"),
            source: "burst".into(),
        });
    }

    loop {
        match slow.recv().await {
            Ok(env) => println!("event {:?}", env.kind),
            Err(RecvError::Lagged(skipped)) => {
                eprintln!("dropped {skipped} events; resuming from newest");
            }
            Err(RecvError::Closed) => break,
        }
    }
    Ok(())
}
```

The loop pattern above — match on `Lagged` and continue — is the
recommended consumer shape for any subscriber that may fall behind.

### Tutorial 4 — Concurrent publishers

```rust
use simard::{HiveEventBus, HiveEventKind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = HiveEventBus::new();
    let mut rx = bus.subscribe();

    let mut handles = vec![];
    for i in 0..8 {
        let bus = bus.clone();
        handles.push(tokio::spawn(async move {
            bus.publish(HiveEventKind::NodeJoined {
                node_id: format!("n{i}"),
            })
        }));
    }
    for h in handles { h.await?; }

    // All 8 publishes have returned before we count, so the buffer
    // holds exactly 8 envelopes ready to be drained synchronously.
    let mut count = 0;
    while let Ok(_) = rx.try_recv() { count += 1; }
    assert_eq!(count, 8);
    Ok(())
}
```

### Tutorial 5 — Serializing events

```rust
use simard::{HiveEventEnvelope, HiveEventKind};

let env = HiveEventEnvelope::new(HiveEventKind::FactPromoted {
    fact_id: "f-42".into(),
    promoted_by: "reviewer-1".into(),
});
let json = serde_json::to_string(&env).unwrap();
let back: HiveEventEnvelope = serde_json::from_str(&json).unwrap();
assert_eq!(env.id, back.id);
```

Stable JSON shape:

```json
{
  "id": "0190a5d3-...-...-...",
  "timestamp": "2026-04-19T04:58:42.417Z",
  "kind": { "type": "fact_promoted", "fact_id": "f-42", "promoted_by": "reviewer-1" }
}
```

---

## Behavioral guarantees

| # | Guarantee                                                                                  |
| - | ------------------------------------------------------------------------------------------ |
| 1 | `publish` never blocks on slow subscribers.                                                |
| 2 | `publish` with no subscribers returns `0` (not an error).                                  |
| 3 | Subscribers created **after** a `publish` call do **not** observe that event.              |
| 4 | All concurrent subscribers observe the same envelope (same `id`, same `timestamp`) — `publish` constructs the envelope once and clones it per delivery. |
| 5 | A subscriber more than `capacity` events behind receives `RecvError::Lagged(n)`.           |
| 6 | `BusError`'s `Display`/`Debug` never include event payload bytes (vacuously true today; an explicit constraint on any future variant). |
| 7 | `HiveEventEnvelope` round-trips losslessly through `serde_json`.                           |
| 8 | The bus introduces **no new external crate dependencies** beyond what Simard already pulls in (see "Build prerequisites" below for the one in-tree feature flag). |

These guarantees are enforced by the in-module `#[cfg(test)] mod tests`,
which runs under `cargo test --package simard --lib`.

---

## Build prerequisites

The bus uses `tokio::sync::broadcast`, which is gated behind tokio's
`sync` feature. Simard's current `Cargo.toml` enables tokio with
`["rt", "rt-multi-thread", "process", "io-util", "time", "net", "macros"]`
— this workstream additionally enables `"sync"`. No new crate is added;
this is a feature-flag delta on an existing dependency.

---

## Non-goals

- **Persistence.** Events are not stored; restart loses everything in flight.
- **Replay.** Late subscribers cannot ask for prior events. (A separate
  event log module may layer on top later.)
- **Cross-process delivery.** No IPC, no network. A future bridge module
  may forward events to a transport.
- **Backpressure on publishers.** Publishers are never blocked. If you
  need ordered, durable delivery, this is not the primitive.
- **Schema versioning.** `HiveEventKind` evolves additively under
  `#[non_exhaustive]`; consumers must use catch-all match arms.
- **Pre-built envelope publishing.** A `publish_envelope` API was
  considered and deliberately omitted (YAGNI). It can be added when a
  real consumer needs replay or test injection.

---

## Operational notes

- **Logging.** The bus itself does not log. Wrap publishes at the call
  site if you want event-level tracing — and avoid logging full payloads
  unless you have considered the data sensitivity.
- **Metrics.** `subscriber_count()` and `capacity()` are cheap and safe to
  poll from a metrics task.
- **Shutdown.** Drop all `HiveEventBus` clones to close the channel. Live
  subscribers will then see `RecvError::Closed`.
- **Thread safety.** `HiveEventBus` is `Send + Sync`; it can live inside
  an `Arc`, a `OnceLock`, or be passed by clone across tasks without
  further wrapping.

---

## See also

- Issue [#949] — original requirement.
- [`tokio::sync::broadcast`] — underlying primitive.
- `src/hive_event_bus.rs` — implementation and the authoritative test
  suite for the guarantees above.
