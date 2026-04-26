//! In-process pub/sub event bus for hive coordination.
//!
//! Wraps `tokio::sync::broadcast` to fan out [`HiveEventEnvelope`]s to all
//! current subscribers. See `docs/hive_event_bus/README.md` for the full
//! contract. Implements the design for issue #949.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Default broadcast channel capacity (per design spec).
pub const DEFAULT_CAPACITY: usize = 1024;

/// Rolling window over which `events_per_min` is computed (5 minutes).
const RATE_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Statically known topics surfaced by `/api/distributed`'s `event_bus` key.
/// New `HiveEventKind` variants must extend this list (and `topic()`) to be
/// surfaced as zeroed entries when silent.
pub const KNOWN_TOPICS: &[&str] = &[
    "fact_promoted",
    "fact_imported",
    "node_joined",
    "node_left",
    "memory_sync_requested",
];

/// Typed enum of hive event variants. Marked non-exhaustive to allow
/// additive evolution without breaking downstream matches.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HiveEventKind {
    FactPromoted { fact_id: String },
    FactImported { fact_id: String, source: String },
    NodeJoined { node_id: String },
    NodeLeft { node_id: String },
    MemorySyncRequested { node_id: String },
}

/// Envelope wrapping every published event with id + timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiveEventEnvelope {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub kind: HiveEventKind,
}

impl HiveEventEnvelope {
    /// Build a new envelope, stamping a fresh uuid v7 id and current UTC time.
    pub fn new(kind: HiveEventKind) -> Self {
        Self {
            id: Uuid::now_v7(),
            timestamp: Utc::now(),
            kind,
        }
    }
}

impl HiveEventKind {
    /// Stable, lower-snake-case topic name for this event variant.
    ///
    /// Used by [`HiveEventBus`] stats and `/api/distributed`'s `event_bus`
    /// key. Adding a new variant must extend this match and [`KNOWN_TOPICS`].
    pub fn topic(&self) -> &'static str {
        match self {
            HiveEventKind::FactPromoted { .. } => "fact_promoted",
            HiveEventKind::FactImported { .. } => "fact_imported",
            HiveEventKind::NodeJoined { .. } => "node_joined",
            HiveEventKind::NodeLeft { .. } => "node_left",
            HiveEventKind::MemorySyncRequested { .. } => "memory_sync_requested",
        }
    }
}

/// Per-topic stats snapshot returned by [`HiveEventBus::stats_snapshot`].
///
/// `subscribers` reflects the bus's global broadcast receiver count
/// (Tokio `broadcast` is fanout: every receiver sees every event regardless
/// of topic, so per-topic split is intentionally identical to the global
/// count).
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq)]
pub struct TopicStatsSnapshot {
    pub subscribers: usize,
    pub events_per_min: f64,
    pub last_event_timestamp: Option<DateTime<Utc>>,
}

/// Whole-bus stats snapshot returned by [`HiveEventBus::stats_snapshot`].
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq)]
pub struct BusStatsSnapshot {
    pub topics: BTreeMap<String, TopicStatsSnapshot>,
    pub total_subscribers: usize,
    pub events_per_min: f64,
    pub last_event_timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Default)]
struct TopicState {
    /// Sliding-window of recent publish instants, pruned on every read & write.
    recent: VecDeque<Instant>,
    last_event: Option<DateTime<Utc>>,
}

impl TopicState {
    /// Drop entries older than [`RATE_WINDOW`].
    fn prune(&mut self, now: Instant) {
        while let Some(&front) = self.recent.front() {
            if now.saturating_duration_since(front) > RATE_WINDOW {
                self.recent.pop_front();
            } else {
                break;
            }
        }
    }
}

#[derive(Debug, Default)]
struct BusStatsInner {
    /// Per-topic ring buffers. Keyed by `&'static str` from
    /// [`HiveEventKind::topic`] (or `"unknown"` for future variants that have
    /// not yet been wired into [`KNOWN_TOPICS`]).
    topics: BTreeMap<&'static str, TopicState>,
}

/// Process-cheap, mutex-guarded stats accumulator. Critical sections are
/// `VecDeque::push_back` + `Option` write; no I/O, microseconds in steady state.
#[derive(Debug, Default)]
pub(crate) struct BusStats {
    inner: Mutex<BusStatsInner>,
}

impl BusStats {
    fn record(&self, topic: &'static str, ts: DateTime<Utc>) {
        let now = Instant::now();
        // Recover from a poisoned mutex so a panic in another thread cannot
        // permanently break stats recording.
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = g.topics.entry(topic).or_default();
        entry.prune(now);
        entry.recent.push_back(now);
        entry.last_event = Some(ts);
    }

    fn snapshot(&self, subscriber_count: usize) -> BusStatsSnapshot {
        let now = Instant::now();
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        // Prune all topics first so reads observe a freshly-trimmed window.
        for state in g.topics.values_mut() {
            state.prune(now);
        }

        let mut topics: BTreeMap<String, TopicStatsSnapshot> = BTreeMap::new();
        // Seed all known topics with zeroed defaults so the wire format is
        // stable even before any events have been published.
        for &k in KNOWN_TOPICS {
            topics.insert(
                k.to_string(),
                TopicStatsSnapshot {
                    subscribers: subscriber_count,
                    events_per_min: 0.0,
                    last_event_timestamp: None,
                },
            );
        }
        // Overlay actual recorded state (handles unknown topics too).
        for (name, state) in g.topics.iter() {
            topics.insert(
                (*name).to_string(),
                TopicStatsSnapshot {
                    subscribers: subscriber_count,
                    events_per_min: state.recent.len() as f64 / 5.0,
                    last_event_timestamp: state.last_event,
                },
            );
        }

        let agg_count: usize = g.topics.values().map(|s| s.recent.len()).sum();
        let last = g.topics.values().filter_map(|s| s.last_event).max();

        BusStatsSnapshot {
            topics,
            total_subscribers: subscriber_count,
            events_per_min: agg_count as f64 / 5.0,
            last_event_timestamp: last,
        }
    }
}

/// Configuration for [`HiveEventBus`]. Forward-compatible holder for
/// future tunables; currently exposes only channel capacity.
#[derive(Debug, Clone)]
pub struct HiveEventBusConfig {
    pub capacity: usize,
}

impl Default for HiveEventBusConfig {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
        }
    }
}

/// Reserved error type. Current implementation of [`HiveEventBus::publish`]
/// is infallible (returns `usize`), but the type is kept for forward
/// compatibility. Marked non-exhaustive and intentionally has no variants.
#[non_exhaustive]
#[derive(Debug)]
pub enum BusError {}

impl std::fmt::Display for BusError {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {}
    }
}

impl std::error::Error for BusError {}

/// In-process pub/sub bus wrapping `tokio::sync::broadcast`.
///
/// Cloning the bus shares the underlying broadcast channel and stats store.
#[derive(Debug, Clone)]
pub struct HiveEventBus {
    sender: broadcast::Sender<HiveEventEnvelope>,
    capacity: usize,
    stats: Arc<BusStats>,
}

impl HiveEventBus {
    /// Create a bus with [`DEFAULT_CAPACITY`].
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a bus with the given capacity. Panics if `capacity == 0`.
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "HiveEventBus capacity must be > 0");
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            capacity,
            stats: Arc::new(BusStats::default()),
        }
    }

    /// Create a bus from an explicit config.
    pub fn with_config(config: HiveEventBusConfig) -> Self {
        Self::with_capacity(config.capacity)
    }

    /// Process-wide default bus, lazily initialised on first call.
    ///
    /// Used by the operator dashboard's `/api/distributed` handler so that
    /// `event_bus` stats can be surfaced without threading a bus handle
    /// through every call site. Production code that needs to publish should
    /// use this accessor; tests construct their own bus via [`Self::new`] to
    /// keep stats isolated.
    pub fn global() -> &'static HiveEventBus {
        static GLOBAL: OnceLock<HiveEventBus> = OnceLock::new();
        GLOBAL.get_or_init(HiveEventBus::new)
    }

    /// Publish an event. Returns the number of subscribers that received it.
    /// Returns `0` when there are currently no subscribers (no error).
    ///
    /// The envelope is constructed exactly once and the broadcast channel
    /// clones it per delivery. Stats are recorded before broadcast so a slow
    /// subscriber cannot delay observability.
    pub fn publish(&self, kind: HiveEventKind) -> usize {
        let envelope = HiveEventEnvelope::new(kind);
        self.stats.record(envelope.kind.topic(), envelope.timestamp);
        // No active receivers is not an error condition for this bus.
        self.sender.send(envelope).unwrap_or_default()
    }

    /// Subscribe to future events. Late subscribers do not receive events
    /// published before this call.
    pub fn subscribe(&self) -> broadcast::Receiver<HiveEventEnvelope> {
        self.sender.subscribe()
    }

    /// Number of currently active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Configured channel capacity (may be rounded up to a power of two
    /// by the underlying tokio broadcast channel).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Snapshot of current subscriber count, per-topic event rate (over the
    /// last 5 minutes) and last-event timestamp. Pure read; safe to call
    /// from request handlers.
    pub fn stats_snapshot(&self) -> BusStatsSnapshot {
        self.stats.snapshot(self.subscriber_count())
    }
}

impl Default for HiveEventBus {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests — written first (TDD). These specify the contract for Step 8 impl.
// Run with: CARGO_TARGET_DIR=/tmp/simard-ws-949 cargo test --package simard \
//           --lib hive_event_bus
// =============================================================================

#[cfg(test)]
mod tests;
