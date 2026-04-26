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
