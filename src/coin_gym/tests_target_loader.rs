use std::io::Write;

use super::target_loader::{
    CoinDatasetRow, DatasetManifest, DatasetSource, DemoScenario, FixtureTargetSource,
    InMemoryTargetSource, ParsedTargetId, SAMPLE_SNAPSHOT_JSON, TargetSet, TargetSource,
    family_for_split, parse_target_id,
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

// ── Real COIN dataset schema ─────────────────────────────────────────────────

#[test]
fn parse_target_id_single_line() {
    let p = parse_target_id("cups:ipp_fuzzer:cups/ipp.c:1523").unwrap();
    assert_eq!(
        p,
        ParsedTargetId {
            project: "cups".to_string(),
            harness: "ipp_fuzzer".to_string(),
            file: "cups/ipp.c".to_string(),
            line_start: 1523,
            line_end: None,
        }
    );
}

#[test]
fn parse_target_id_line_range() {
    let p = parse_target_id("libraw:libraw_raf_fuzzer:src/metadata/fuji.cpp:480-495").unwrap();
    assert_eq!(p.file, "src/metadata/fuji.cpp");
    assert_eq!(p.line_start, 480);
    assert_eq!(p.line_end, Some(495));
    // A start==end range collapses to a single-line target.
    assert_eq!(parse_target_id("p:h:f:10-10").unwrap().line_end, None);
}

#[test]
fn parse_target_id_rejects_malformed() {
    assert!(parse_target_id("no-colons").is_err());
    assert!(parse_target_id("p:h:f:notaline").is_err());
    assert!(parse_target_id("p:h:f:20-10").is_err()); // end < start
    assert!(parse_target_id("p::f:1").is_err()); // empty harness
}

#[test]
fn family_for_split_maps_published_splits() {
    assert_eq!(
        family_for_split("codeql_only"),
        Some(TargetFamily::Frontier)
    );
    assert_eq!(
        family_for_split("gcs_reachable"),
        Some(TargetFamily::NonTrivialReachable)
    );
    assert_eq!(family_for_split("mystery"), None);
}

#[test]
fn dataset_row_to_target_pins_revision_and_derives_family() {
    let row = CoinDatasetRow {
        target_id: "cups:ipp_fuzzer:cups/ipp.c:1523".to_string(),
        coin_version: Some("v2026-07".to_string()),
        oss_fuzz_commit: Some("deadbeef".to_string()),
        coin_commit: None,
        project: None,
        harness: None,
        file: None,
        line_start: None,
        line_end: None,
        split: Some("codeql_only".to_string()),
        family: None,
    };
    let t = row.to_target("v2026-07").unwrap();
    assert_eq!(t.project, "cups");
    assert_eq!(t.harness, "ipp_fuzzer");
    assert_eq!(t.file, "cups/ipp.c");
    assert_eq!(t.line, 1523);
    assert_eq!(t.commit, "deadbeef");
    assert_eq!(t.family, TargetFamily::Frontier);
    // Pinning is enforced: a mismatched revision is rejected.
    assert!(row.to_target("v2026-08").is_err());
}

#[test]
fn dataset_row_preserves_line_range() {
    let row: CoinDatasetRow = serde_json::from_str(
        r#"{"target_id":"libraw:raf:src/fuji.cpp:480-495","split":"gcs_reachable"}"#,
    )
    .unwrap();
    let t = row.to_target("v2026-07").unwrap();
    assert_eq!(t.line_start(), 480);
    assert_eq!(t.line_end_inclusive(), 495);
    assert_eq!(t.line_count(), 16);
    assert_eq!(t.locator(), "libraw:src/fuji.cpp:480-495");
}

#[test]
fn dataset_source_loads_pinned_and_held_out_slices() {
    let raw = r#"{
        "dataset": "COIN-Bench/coin",
        "revision": "v2026-07",
        "held_out_revision": "v2026-08",
        "pinned": [
            {"target_id":"cups:ipp_fuzzer:cups/ipp.c:1523","split":"codeql_only"},
            {"target_id":"liboqs:kem_fuzzer:src/kem/kem.c:88","split":"gcs_reachable"}
        ],
        "held_out_fresh": [
            {"target_id":"libpng:read_fuzzer:pngrutil.c:903","coin_version":"v2026-08","split":"codeql_only"}
        ]
    }"#;
    let set = DatasetSource::from_manifest(raw).unwrap().load().unwrap();
    assert_eq!(set.snapshot, "COIN-Bench/coin@v2026-07");
    assert_eq!(set.pinned.len(), 2);
    assert_eq!(set.held_out_fresh.len(), 1);
    assert_eq!(set.total(), 3);
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
    // Held-out slice is pinned to the fresh revision, not the eval revision.
    assert_eq!(set.held_out_fresh[0].project, "libpng");
}

#[test]
fn dataset_source_rejects_row_pinned_to_wrong_revision() {
    let raw = r#"{
        "dataset": "COIN-Bench/coin",
        "revision": "v2026-07",
        "pinned": [
            {"target_id":"cups:ipp_fuzzer:cups/ipp.c:1523","coin_version":"v2025-01","split":"codeql_only"}
        ]
    }"#;
    let err = DatasetSource::from_manifest(raw)
        .unwrap()
        .load()
        .unwrap_err();
    assert!(err.to_string().contains("pinned to revision"));
}

#[test]
fn dataset_source_from_path_reads_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dataset.json");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(
        br#"{"dataset":"COIN-Bench/coin","revision":"v2026-07",
             "pinned":[{"target_id":"cups:ipp_fuzzer:cups/ipp.c:1523","split":"codeql_only"}]}"#,
    )
    .unwrap();
    drop(f);
    let set = DatasetSource::from_path(&path).unwrap().load().unwrap();
    assert_eq!(set.pinned.len(), 1);
}

#[test]
fn dataset_manifest_deserializes_minimally() {
    // A manifest with no held-out slice still parses (empty vecs default).
    let m: DatasetManifest = serde_json::from_str(r#"{"dataset":"d","revision":"r"}"#).unwrap();
    assert!(m.pinned.is_empty());
    assert!(m.held_out_fresh.is_empty());
}

#[test]
fn dataset_row_rejects_explicit_backwards_line_range() {
    let row = CoinDatasetRow {
        target_id: "p:h:f:100".to_string(),
        coin_version: None,
        oss_fuzz_commit: None,
        coin_commit: None,
        project: None,
        harness: None,
        file: None,
        line_start: Some(100),
        line_end: Some(50),
        split: Some("codeql_only".to_string()),
        family: None,
    };
    let err = row.to_target("v2026-07").unwrap_err();
    assert!(err.to_string().contains("line_end 50 < line_start 100"));
}
