use std::io::Write;

use super::target_loader::{
    DemoScenario, FixtureTargetSource, InMemoryTargetSource, SAMPLE_SNAPSHOT_JSON, TargetSet,
    TargetSource,
};
use super::types::TargetFamily;

#[test]
fn sample_snapshot_parses_and_has_both_slices() {
    let set = InMemoryTargetSource::sample().load().unwrap();
    assert_eq!(set.snapshot, "you/coin@v1-sample");
    assert!(!set.pinned.is_empty(), "sample must have pinned targets");
    assert!(
        !set.held_out_fresh.is_empty(),
        "sample must reserve a held-out fresh slice"
    );
    assert_eq!(set.total(), set.pinned.len() + set.held_out_fresh.len());
}

#[test]
fn sample_has_both_families_in_pinned() {
    let set = InMemoryTargetSource::sample().load().unwrap();
    assert!(
        set.pinned
            .iter()
            .any(|t| t.family == TargetFamily::Frontier)
    );
    assert!(
        set.pinned
            .iter()
            .any(|t| t.family == TargetFamily::NonTrivialReachable)
    );
}

#[test]
fn demo_scenario_sample_has_oracle_and_script() {
    let scenario = DemoScenario::sample().unwrap();
    // Every scripted target must exist in the pinned set.
    for id in scenario.script.keys() {
        assert!(
            scenario.targets.pinned.iter().any(|t| &t.id == id),
            "scripted id {id} missing from pinned targets"
        );
    }
    // Oracle covers every pinned target so offline grading is well-defined.
    for t in &scenario.targets.pinned {
        assert!(
            scenario.oracle.contains_key(&t.id),
            "oracle missing reaching input for {}",
            t.id
        );
    }
}

#[test]
fn malformed_manifest_is_a_parse_error() {
    let src = InMemoryTargetSource::new("{ not json ");
    let err = src.load().unwrap_err();
    assert!(err.to_string().contains("parse error"));
}

#[test]
fn fixture_source_reads_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snap.json");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(SAMPLE_SNAPSHOT_JSON.as_bytes()).unwrap();
    drop(f);

    let set = FixtureTargetSource::new(&path).load().unwrap();
    assert_eq!(set.snapshot, "you/coin@v1-sample");

    let scenario = DemoScenario::from_path(&path).unwrap();
    assert_eq!(scenario.targets.snapshot, "you/coin@v1-sample");
}

#[test]
fn missing_file_is_io_error() {
    let src = FixtureTargetSource::new("/no/such/coin/snapshot.json");
    let err = src.load().unwrap_err();
    assert!(err.to_string().contains("io error"));
}

#[test]
fn target_set_total_counts_both_slices() {
    let raw = r#"{
        "snapshot": "s",
        "targets": {
            "pinned": [
                {"id":"a","project":"p","commit":"c","harness":"h","file":"f","line":1,"family":"frontier"}
            ],
            "held_out_fresh": [
                {"id":"b","project":"p","commit":"c","harness":"h","file":"f","line":2,"family":"non-trivial-reachable"}
            ]
        }
    }"#;
    let set: TargetSet = InMemoryTargetSource::new(raw).load().unwrap();
    assert_eq!(set.pinned.len(), 1);
    assert_eq!(set.held_out_fresh.len(), 1);
    assert_eq!(set.total(), 2);
}
