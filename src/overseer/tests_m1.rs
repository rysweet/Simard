//! M1 unit tests for the pure Observe→Orient core: `signals_from` threshold
//! boundaries and `orient` dedup / merge / prioritisation.
//!
//! These are the two unit tests M1's exit criteria call out explicitly
//! ("`signals_from` thresholds; `orient` dedup vs in-flight"). They are pure —
//! no sleeps, no network, no clock — matching the shipped cognitive-thread test
//! discipline. Boundary cases assert the fence sits exactly where the threshold
//! constant says (`>=` fires, one-below does not).

use crate::overseer::capabilities::{CiFailure, InFlightItem, ObservedState, PrRef};
use crate::overseer::orient;
use crate::overseer::signal::{Priority, Signal, signals_from};

// ─────────────────────────── signals_from thresholds ───────────────────────

#[test]
fn no_signals_from_a_default_snapshot() {
    assert!(signals_from(&ObservedState::default()).is_empty());
}

#[test]
fn distill_failure_fires_at_and_above_threshold_only() {
    // Threshold is 20.0.
    let below = ObservedState {
        distill_fail_pct: Some(19.9),
        ..ObservedState::default()
    };
    assert!(signals_from(&below).is_empty(), "19.9% is below the fence");

    let at = ObservedState {
        distill_fail_pct: Some(20.0),
        ..ObservedState::default()
    };
    assert_eq!(
        signals_from(&at),
        vec![Signal::DistillFailureRate { pct: 20.0 }],
        "exactly at threshold must fire"
    );

    let high = ObservedState {
        distill_fail_pct: Some(62.0),
        ..ObservedState::default()
    };
    assert_eq!(
        signals_from(&high),
        vec![Signal::DistillFailureRate { pct: 62.0 }],
        "the observed ~62% case must fire"
    );
}

#[test]
fn restart_churn_fires_at_and_above_threshold_only() {
    // Threshold is 3.
    let below = ObservedState {
        restart_churn: Some(2),
        ..ObservedState::default()
    };
    assert!(signals_from(&below).is_empty());

    let at = ObservedState {
        restart_churn: Some(3),
        ..ObservedState::default()
    };
    assert_eq!(
        signals_from(&at),
        vec![Signal::RestartChurn { restarts: 3 }]
    );
}

#[test]
fn ladder_exhausted_fires_above_zero() {
    let zero = ObservedState {
        ladder_exhausted: Some(0),
        ..ObservedState::default()
    };
    assert!(signals_from(&zero).is_empty(), "zero exhaustions is quiet");

    let some = ObservedState {
        ladder_exhausted: Some(1),
        ..ObservedState::default()
    };
    assert_eq!(
        signals_from(&some),
        vec![Signal::LadderExhausted { count: 1 }]
    );
}

#[test]
fn budget_pressure_fires_at_eighty_percent_and_guards_zero_budget() {
    // Fraction is 0.8 of the daily budget.
    let below = ObservedState {
        spent_today_usd: Some(399.0),
        daily_budget_usd: Some(500.0),
        ..ObservedState::default()
    };
    assert!(signals_from(&below).is_empty(), "79.8% is below the fence");

    let at = ObservedState {
        spent_today_usd: Some(400.0),
        daily_budget_usd: Some(500.0),
        ..ObservedState::default()
    };
    assert_eq!(
        signals_from(&at),
        vec![Signal::BudgetPressure {
            spent_usd: 400.0,
            budget_usd: 500.0
        }],
        "80% of budget must fire"
    );

    // A zero/negative budget must never divide-or-fire (guard against bad config).
    let zero_budget = ObservedState {
        spent_today_usd: Some(400.0),
        daily_budget_usd: Some(0.0),
        ..ObservedState::default()
    };
    assert!(
        signals_from(&zero_budget).is_empty(),
        "zero budget cannot fire"
    );

    // Missing either side → no signal.
    let missing_budget = ObservedState {
        spent_today_usd: Some(400.0),
        daily_budget_usd: None,
        ..ObservedState::default()
    };
    assert!(signals_from(&missing_budget).is_empty());
}

#[test]
fn engineer_spawn_fires_at_and_above_threshold_only() {
    // Threshold is 8.
    let below = ObservedState {
        live_engineers: Some(7),
        ..ObservedState::default()
    };
    assert!(signals_from(&below).is_empty());

    let at = ObservedState {
        live_engineers: Some(8),
        ..ObservedState::default()
    };
    assert_eq!(
        signals_from(&at),
        vec![Signal::EngineerSpawnRate { live: 8 }]
    );
}

#[test]
fn gym_skip_fires_when_true() {
    let skipped = ObservedState {
        gym_skipped: true,
        ..ObservedState::default()
    };
    assert_eq!(signals_from(&skipped), vec![Signal::GymSkipped]);
}

#[test]
fn ci_failures_ready_prs_and_anomalies_fan_out_one_signal_each() {
    let state = ObservedState {
        ci_failures: vec![
            CiFailure {
                repo: "rysweet/Simard".to_string(),
                failing: 3,
            },
            CiFailure {
                repo: "rysweet/amplihack".to_string(),
                failing: 1,
            },
        ],
        ready_prs: vec![PrRef {
            repo: "rysweet/Simard".to_string(),
            pr: 42,
        }],
        anomalies: vec!["banner pollution".to_string()],
        ..ObservedState::default()
    };
    let sigs = signals_from(&state);
    assert!(sigs.contains(&Signal::CiFailureCluster {
        repo: "rysweet/Simard".to_string(),
        failing: 3
    }));
    assert!(sigs.contains(&Signal::CiFailureCluster {
        repo: "rysweet/amplihack".to_string(),
        failing: 1
    }));
    assert!(sigs.contains(&Signal::PrReadyToMerge {
        repo: "rysweet/Simard".to_string(),
        pr: 42
    }));
    assert!(sigs.contains(&Signal::Anomaly {
        detail: "banner pollution".to_string()
    }));
    assert_eq!(sigs.len(), 4, "one signal per observed item, no more");
}

// ─────────────────────────── orient dedup / merge / order ──────────────────

#[test]
fn orient_raises_one_problem_per_distinct_key_when_nothing_in_flight() {
    let signals = signals_from(&ObservedState {
        distill_fail_pct: Some(62.0),
        gym_skipped: true,
        ..ObservedState::default()
    });
    let problems = orient(&signals, &[]);
    assert_eq!(
        problems.len(),
        2,
        "two distinct problems, nothing in flight"
    );
}

#[test]
fn orient_dedups_against_matching_in_flight_only() {
    let signals = vec![Signal::DistillFailureRate { pct: 62.0 }, Signal::GymSkipped];
    // An engineer already owns the distill problem (same dedup key) — drop it,
    // keep the gym problem (nobody on it).
    let in_flight = vec![InFlightItem {
        id: "g1".to_string(),
        source: "ooda".to_string(),
        refs: vec!["process:distill_fail".to_string()],
    }];
    let problems = orient(&signals, &in_flight);
    assert_eq!(problems.len(), 1, "only the un-owned problem survives");
    assert_eq!(problems[0].dedup_key, "quality:gym_skipped");
}

#[test]
fn orient_merges_same_key_signals_into_one_problem() {
    // Two distill signals collapse to a single problem carrying both as evidence.
    let signals = vec![
        Signal::DistillFailureRate { pct: 40.0 },
        Signal::DistillFailureRate { pct: 62.0 },
    ];
    let problems = orient(&signals, &[]);
    assert_eq!(problems.len(), 1, "same dedup key → one problem");
    assert_eq!(
        problems[0].evidence.len(),
        2,
        "both signals kept as evidence"
    );
    assert_eq!(problems[0].dedup_key, "process:distill_fail");
}

#[test]
fn orient_sorts_problems_by_priority_ascending() {
    // A mix spanning High / Normal / Low priorities.
    let signals = vec![
        Signal::GymSkipped,                       // Low
        Signal::DistillFailureRate { pct: 62.0 }, // High
        Signal::LadderExhausted { count: 2 },     // Normal
    ];
    let problems = orient(&signals, &[]);
    assert_eq!(problems.len(), 3);
    // Ord sorts Critical < High < Normal < Low, so the vector is non-decreasing.
    assert!(
        problems.windows(2).all(|w| w[0].priority <= w[1].priority),
        "problems must be ordered most-important first"
    );
    assert_eq!(
        problems[0].priority,
        Priority::High,
        "High comes first here"
    );
    assert_eq!(problems[2].priority, Priority::Low, "Low comes last");
}

#[test]
fn orient_is_empty_when_every_problem_is_in_flight() {
    let signals = vec![Signal::DistillFailureRate { pct: 62.0 }];
    let in_flight = vec![InFlightItem {
        id: "g1".to_string(),
        source: "ooda".to_string(),
        refs: vec!["process:distill_fail".to_string()],
    }];
    assert!(orient(&signals, &in_flight).is_empty());
}
