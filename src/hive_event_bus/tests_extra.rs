use super::*;

fn sample_kind() -> HiveEventKind {
    HiveEventKind::FactPromoted {
        fact_id: "fact-1".into(),
    }
}

// --- Envelope construction (I0) ----------------------------------------
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
