//! In-process pub/sub event bus for hive coordination.
//!
//! Wraps `tokio::sync::broadcast` to fan out [`HiveEventEnvelope`]s to all
//! current subscribers. See `docs/hive_event_bus/README.md` for the full
//! contract. Implements the design for issue #949.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Default broadcast channel capacity (per design spec).
pub const DEFAULT_CAPACITY: usize = 1024;

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

/// Configuration for [`HiveEventBus`]. Forward-compatible holder for
/// future tunables; currently exposes only channel capacity.
#[derive(Debug, Clone)]
pub struct HiveEventBusConfig {
    pub capacity: usize,
}

impl Default for HiveEventBusConfig {
    fn default() -> Self {
        Self { capacity: DEFAULT_CAPACITY }
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
/// Cloning the bus shares the underlying broadcast channel.
#[derive(Debug, Clone)]
pub struct HiveEventBus {
    sender: broadcast::Sender<HiveEventEnvelope>,
    capacity: usize,
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
        Self { sender, capacity }
    }

    /// Create a bus from an explicit config.
    pub fn with_config(config: HiveEventBusConfig) -> Self {
        Self::with_capacity(config.capacity)
    }

    /// Publish an event. Returns the number of subscribers that received it.
    /// Returns `0` when there are currently no subscribers (no error).
    ///
    /// The envelope is constructed exactly once and the broadcast channel
    /// clones it per delivery.
    pub fn publish(&self, kind: HiveEventKind) -> usize {
        let envelope = HiveEventEnvelope::new(kind);
        match self.sender.send(envelope) {
            Ok(n) => n,
            // No active receivers is not an error condition for this bus.
            Err(_) => 0,
        }
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
        HiveEventKind::FactPromoted { fact_id: "fact-1".into() }
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

        let kind = HiveEventKind::NodeJoined { node_id: "n42".into() };
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
            bus.publish(HiveEventKind::FactPromoted { fact_id: format!("f{i}") });
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
            HiveEventKind::FactPromoted { fact_id: "a".into() },
            HiveEventKind::FactImported { fact_id: "b".into(), source: "s".into() },
            HiveEventKind::NodeJoined { node_id: "n1".into() },
            HiveEventKind::NodeLeft { node_id: "n2".into() },
            HiveEventKind::MemorySyncRequested { node_id: "n3".into() },
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
                    panic!("subscriber lagged with capacity {} >= total {total}", bus.capacity())
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
}
