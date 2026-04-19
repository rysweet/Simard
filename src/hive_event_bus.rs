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
        self.sender.send(envelope).unwrap_or(0)
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
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Barrier;
    use tokio::time::timeout;

    fn sample_kind() -> HiveEventKind {
        HiveEventKind::FactPromoted {
            fact_id: "fact-1".into(),
        }
    }

    // --- Envelope construction (I0) ----------------------------------------
    #[test]
    fn envelope_new_stamps_id_and_timestamp() {
        let before = Utc::now();
        let env = HiveEventEnvelope::new(sample_kind());
        let after = Utc::now();

        assert_eq!(env.kind, sample_kind());
        assert!(env.timestamp >= before && env.timestamp <= after);
        // uuid v7 is non-nil
        assert_ne!(env.id, Uuid::nil());
    }

    #[test]
    fn envelope_new_generates_unique_ids() {
        let a = HiveEventEnvelope::new(sample_kind());
        let b = HiveEventEnvelope::new(sample_kind());
        assert_ne!(a.id, b.id);
    }

    // --- Construction & config (I7) ----------------------------------------
    #[test]
    fn new_uses_default_capacity() {
        let bus = HiveEventBus::new();
        assert_eq!(bus.capacity(), DEFAULT_CAPACITY);
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn with_config_honours_capacity() {
        let bus = HiveEventBus::with_config(HiveEventBusConfig { capacity: 16 });
        assert_eq!(bus.capacity(), 16);
    }

    #[test]
    #[should_panic]
    fn with_capacity_zero_panics() {
        let _ = HiveEventBus::with_capacity(0);
    }

    // --- Invariant I1: single subscriber round-trip ------------------------
    #[tokio::test]
    async fn single_subscriber_round_trip() {
        let bus = HiveEventBus::new();
        let mut rx = bus.subscribe();

        let n = bus.publish(sample_kind());
        assert_eq!(n, 1);

        let env = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("recv timed out")
            .expect("recv error");
        assert_eq!(env.kind, sample_kind());
    }

    // --- Invariant I2: multi-subscriber broadcast (≥3 nodes) --------------
    #[tokio::test]
    async fn multi_subscriber_broadcast_three_nodes() {
        let bus = HiveEventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        let mut rx3 = bus.subscribe();

        let kind = HiveEventKind::NodeJoined {
            node_id: "n42".into(),
        };
        let n = bus.publish(kind.clone());
        assert_eq!(n, 3, "all three simulated nodes must receive");

        for rx in [&mut rx1, &mut rx2, &mut rx3] {
            let env = timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("recv timed out")
                .expect("recv error");
            assert_eq!(env.kind, kind);
        }
    }

    #[tokio::test]
    async fn all_subscribers_see_identical_envelope_id() {
        let bus = HiveEventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.publish(sample_kind());

        let e1 = rx1.recv().await.expect("rx1");
        let e2 = rx2.recv().await.expect("rx2");
        assert_eq!(e1.id, e2.id, "envelope built once and cloned per delivery");
        assert_eq!(e1.timestamp, e2.timestamp);
    }

    // --- Invariant I3: late subscriber misses prior events -----------------
    #[tokio::test]
    async fn late_subscriber_does_not_receive_prior_events() {
        let bus = HiveEventBus::new();
        let mut early = bus.subscribe();

        bus.publish(sample_kind());
        let _ = early.recv().await.expect("early should receive");

        let mut late = bus.subscribe();
        let result = timeout(Duration::from_millis(100), late.recv()).await;
        assert!(
            result.is_err(),
            "late subscriber must not receive events published before subscribe()"
        );
    }

    // --- Invariant I4: publish with no subscribers returns 0 ---------------
    #[tokio::test]
    async fn publish_with_no_subscribers_returns_zero() {
        let bus = HiveEventBus::new();
        let n = bus.publish(sample_kind());
        assert_eq!(n, 0);
    }

    // --- Invariant I5: lag/capacity behaviour ------------------------------
    #[tokio::test]
    async fn slow_subscriber_gets_lagged_error() {
        let bus = HiveEventBus::with_capacity(2);
        let mut slow = bus.subscribe();

        // Overflow: publish more than capacity without draining slow.
        for i in 0..10 {
            bus.publish(HiveEventKind::FactPromoted {
                fact_id: format!("f{i}"),
            });
        }

        match slow.recv().await {
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                assert!(skipped > 0, "lagged count must be positive");
            }
            other => panic!("expected Lagged, got {other:?}"),
        }
    }

    // --- Invariant I6: serde JSON round-trip -------------------------------
    #[test]
    fn envelope_serde_json_round_trip() {
        let env = HiveEventEnvelope::new(HiveEventKind::FactImported {
            fact_id: "f1".into(),
            source: "peer-A".into(),
        });

        let json = serde_json::to_string(&env).expect("serialize");
        let back: HiveEventEnvelope = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back, env);
    }

    #[test]
    fn all_event_kinds_serde_round_trip() {
        let kinds = vec![
            HiveEventKind::FactPromoted {
                fact_id: "a".into(),
            },
            HiveEventKind::FactImported {
                fact_id: "b".into(),
                source: "s".into(),
            },
            HiveEventKind::NodeJoined {
                node_id: "n1".into(),
            },
            HiveEventKind::NodeLeft {
                node_id: "n2".into(),
            },
            HiveEventKind::MemorySyncRequested {
                node_id: "n3".into(),
            },
        ];
        for k in kinds {
            let env = HiveEventEnvelope::new(k.clone());
            let s = serde_json::to_string(&env).expect("ser");
            let back: HiveEventEnvelope = serde_json::from_str(&s).expect("de");
            assert_eq!(back.kind, k);
        }
    }

    // --- Concurrent publishers from multiple tasks -------------------------
    #[tokio::test]
    async fn concurrent_publishers_all_events_delivered() {
        let bus = HiveEventBus::with_capacity(256);
        let mut rx = bus.subscribe();

        let publishers = 8usize;
        let per_publisher = 10usize;
        let total = publishers * per_publisher;

        let barrier = Arc::new(Barrier::new(publishers));
        let mut handles = Vec::with_capacity(publishers);
        for p in 0..publishers {
            let bus = bus.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                for i in 0..per_publisher {
                    bus.publish(HiveEventKind::FactPromoted {
                        fact_id: format!("p{p}-i{i}"),
                    });
                }
            }));
        }
        for h in handles {
            h.await.expect("publisher task");
        }

        let mut received = 0usize;
        for _ in 0..total {
            match timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Ok(_)) => received += 1,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    panic!(
                        "subscriber lagged with capacity {} >= total {total}",
                        bus.capacity()
                    )
                }
                Ok(Err(e)) => panic!("recv err: {e:?}"),
                Err(_) => panic!("timeout after receiving {received}/{total}"),
            }
        }
        assert_eq!(received, total);
    }

    // --- Subscriber count tracking -----------------------------------------
    #[tokio::test]
    async fn subscriber_count_reflects_active_receivers() {
        let bus = HiveEventBus::new();
        assert_eq!(bus.subscriber_count(), 0);

        let r1 = bus.subscribe();
        let r2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);

        drop(r1);
        // tokio updates receiver_count synchronously on drop
        assert_eq!(bus.subscriber_count(), 1);

        drop(r2);
        assert_eq!(bus.subscriber_count(), 0);
    }

    // --- Bus is Clone and shares the underlying channel --------------------
    #[tokio::test]
    async fn cloned_bus_shares_channel() {
        let bus_a = HiveEventBus::new();
        let bus_b = bus_a.clone();
        let mut rx = bus_a.subscribe();

        let n = bus_b.publish(sample_kind());
        assert_eq!(n, 1);

        let env = rx.recv().await.expect("recv");
        assert_eq!(env.kind, sample_kind());
    }

    // =========================================================================
    // WS-4 — BusStats / stats_snapshot() contract (TDD: FAILING until P1 lands)
    // =========================================================================
    // These tests pin the additive observability contract surfaced via
    // `/api/distributed`'s `event_bus` key. They reference symbols that do not
    // yet exist; compilation must fail until Step 8 (P1) implements them.
    //
    // Contract under test:
    //   - `HiveEventKind::topic(&self) -> &'static str` for all known variants.
    //   - `HiveEventBus::stats_snapshot() -> BusStatsSnapshot`.
    //   - Snapshot lists all 5 known topics with zeroed defaults when silent.
    //   - `events_per_min` = count_in_last_5_min / 5.0.
    //   - `last_event_timestamp` is `Some(_)` after a publish, `None` otherwise.
    //   - Aggregate `total_subscribers` reflects active receiver count.
    //   - Aggregate `events_per_min` equals the sum of per-topic rates.

    const KNOWN_TOPICS: &[&str] = &[
        "fact_promoted",
        "fact_imported",
        "node_joined",
        "node_left",
        "memory_sync_requested",
    ];

    #[test]
    fn topic_str_for_each_known_kind() {
        // Round-trips each known variant through `.topic()`.
        let pairs: &[(HiveEventKind, &str)] = &[
            (
                HiveEventKind::FactPromoted {
                    fact_id: "x".into(),
                },
                "fact_promoted",
            ),
            (
                HiveEventKind::FactImported {
                    fact_id: "x".into(),
                    source: "s".into(),
                },
                "fact_imported",
            ),
            (
                HiveEventKind::NodeJoined {
                    node_id: "n".into(),
                },
                "node_joined",
            ),
            (
                HiveEventKind::NodeLeft {
                    node_id: "n".into(),
                },
                "node_left",
            ),
            (
                HiveEventKind::MemorySyncRequested {
                    node_id: "n".into(),
                },
                "memory_sync_requested",
            ),
        ];
        for (kind, expected) in pairs {
            assert_eq!(kind.topic(), *expected, "topic mismatch for {kind:?}");
        }
    }

    #[tokio::test]
    async fn stats_snapshot_lists_all_known_topics_zeroed_when_silent() {
        let bus = HiveEventBus::new();
        let snap = bus.stats_snapshot();

        for t in KNOWN_TOPICS {
            let entry = snap
                .topics
                .get(*t)
                .unwrap_or_else(|| panic!("missing topic key '{t}' in snapshot"));
            assert_eq!(
                entry.events_per_min, 0.0,
                "silent topic '{t}' rate must be 0"
            );
            assert!(
                entry.last_event_timestamp.is_none(),
                "silent topic '{t}' last_event must be None",
            );
        }
        assert!(snap.last_event_timestamp.is_none());
        assert_eq!(snap.events_per_min, 0.0);
    }

    #[tokio::test]
    async fn stats_snapshot_records_publish_timestamp_and_rate() {
        let bus = HiveEventBus::new();
        let _rx = bus.subscribe();

        let before = Utc::now();
        bus.publish(HiveEventKind::FactPromoted {
            fact_id: "f1".into(),
        });
        bus.publish(HiveEventKind::FactPromoted {
            fact_id: "f2".into(),
        });
        let after = Utc::now();

        let snap = bus.stats_snapshot();
        let topic = snap
            .topics
            .get("fact_promoted")
            .expect("fact_promoted entry must exist");

        // Rate: 2 events over a 5-minute window = 0.4/min.
        assert!(
            (topic.events_per_min - 0.4).abs() < 1e-9,
            "expected 0.4/min, got {}",
            topic.events_per_min,
        );

        let ts = topic
            .last_event_timestamp
            .expect("last_event_timestamp must be Some after publish");
        assert!(ts >= before && ts <= after, "timestamp out of window: {ts}");

        // Aggregate last_event must equal the most recent topic timestamp.
        let agg_ts = snap
            .last_event_timestamp
            .expect("aggregate last_event must be Some");
        assert_eq!(agg_ts, ts);
    }

    #[tokio::test]
    async fn stats_snapshot_aggregate_rate_equals_sum_of_topic_rates() {
        let bus = HiveEventBus::new();
        let _rx = bus.subscribe();

        bus.publish(HiveEventKind::FactPromoted {
            fact_id: "a".into(),
        });
        bus.publish(HiveEventKind::NodeJoined {
            node_id: "n1".into(),
        });
        bus.publish(HiveEventKind::NodeJoined {
            node_id: "n2".into(),
        });

        let snap = bus.stats_snapshot();
        let sum: f64 = snap.topics.values().map(|t| t.events_per_min).sum();
        assert!(
            (snap.events_per_min - sum).abs() < 1e-9,
            "aggregate {} must equal sum {}",
            snap.events_per_min,
            sum,
        );
    }

    #[tokio::test]
    async fn stats_snapshot_total_subscribers_reflects_receivers() {
        let bus = HiveEventBus::new();
        let snap0 = bus.stats_snapshot();
        let baseline = snap0.total_subscribers;

        let r1 = bus.subscribe();
        let r2 = bus.subscribe();
        let snap2 = bus.stats_snapshot();
        assert_eq!(snap2.total_subscribers, baseline + 2);

        // Per-topic `subscribers` reflects the global broadcast count
        // (Tokio broadcast is fanout; per-topic split is documented honestly).
        for t in KNOWN_TOPICS {
            let entry = snap2.topics.get(*t).expect("topic key");
            assert_eq!(entry.subscribers, snap2.total_subscribers);
        }

        drop(r1);
        drop(r2);
        let snap3 = bus.stats_snapshot();
        assert_eq!(snap3.total_subscribers, baseline);
    }

    #[test]
    fn stats_snapshot_serializes_to_expected_json_shape() {
        // Build a snapshot via the public API and validate the JSON layout
        // matches the documented `/api/distributed` `event_bus` contract.
        let bus = HiveEventBus::new();
        let snap = bus.stats_snapshot();
        let v = serde_json::to_value(&snap).expect("serialize snapshot");

        assert!(
            v.get("topics").and_then(|x| x.as_object()).is_some(),
            "snapshot must have 'topics' object"
        );
        assert!(
            v.get("total_subscribers")
                .and_then(|x| x.as_u64())
                .is_some(),
            "snapshot must have 'total_subscribers' u64"
        );
        assert!(
            v.get("events_per_min").and_then(|x| x.as_f64()).is_some(),
            "snapshot must have 'events_per_min' f64"
        );
        // last_event_timestamp may be null OR a string.
        assert!(
            v.get("last_event_timestamp").is_some(),
            "snapshot must include 'last_event_timestamp' key"
        );

        let topics = v.get("topics").unwrap().as_object().unwrap();
        for t in KNOWN_TOPICS {
            let entry = topics
                .get(*t)
                .unwrap_or_else(|| panic!("missing topic '{t}' in JSON shape"));
            assert!(entry.get("subscribers").and_then(|x| x.as_u64()).is_some());
            assert!(
                entry
                    .get("events_per_min")
                    .and_then(|x| x.as_f64())
                    .is_some()
            );
            assert!(entry.get("last_event_timestamp").is_some());
        }
    }
}
