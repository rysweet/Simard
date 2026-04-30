use std::path::PathBuf;

use super::{make_report_with_goals_and_outcomes, make_test_report, persist_cycle_report};

#[test]
fn persist_cycle_report_creates_directory_and_file() {
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-scratch")
        .join(format!("ooda-persist-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

    let report = make_test_report(42);
    persist_cycle_report(&scratch, &report);

    let path = scratch.join("cycle_reports").join("cycle_42.json");
    assert!(path.exists(), "cycle report file should be created");

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(
        content.contains("42"),
        "content should reference cycle number"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn persist_cycle_report_uses_cycle_number_in_filename() {
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-scratch")
        .join(format!("ooda-filename-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

    persist_cycle_report(&scratch, &make_test_report(99));
    let path = scratch.join("cycle_reports").join("cycle_99.json");
    assert!(path.exists());

    persist_cycle_report(&scratch, &make_test_report(100));
    let path2 = scratch.join("cycle_reports").join("cycle_100.json");
    assert!(path2.exists());

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn persist_cycle_report_overwrites_existing() {
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-scratch")
        .join(format!("ooda-overwrite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

    persist_cycle_report(&scratch, &make_test_report(1));
    let first = std::fs::read_to_string(scratch.join("cycle_reports/cycle_1.json")).unwrap();

    let report2 = make_report_with_goals_and_outcomes();
    let mut report2_cycle1 = report2;
    report2_cycle1.cycle_number = 1;
    persist_cycle_report(&scratch, &report2_cycle1);
    let second = std::fs::read_to_string(scratch.join("cycle_reports/cycle_1.json")).unwrap();

    assert_ne!(first, second, "second write should overwrite the first");
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn persist_cycle_report_with_high_cycle_number() {
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-scratch")
        .join(format!("ooda-high-cycle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

    let report = make_test_report(999999);
    persist_cycle_report(&scratch, &report);
    let path = scratch.join("cycle_reports").join("cycle_999999.json");
    assert!(path.exists());
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn persist_cycle_report_cycle_zero() {
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-scratch")
        .join(format!("ooda-zero-cycle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

    let report = make_test_report(0);
    persist_cycle_report(&scratch, &report);
    let path = scratch.join("cycle_reports").join("cycle_0.json");
    assert!(path.exists());
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn persist_cycle_report_with_rich_report() {
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-scratch")
        .join(format!("ooda-rich-report-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

    let report = make_report_with_goals_and_outcomes();
    persist_cycle_report(&scratch, &report);
    let path = scratch.join("cycle_reports").join("cycle_7.json");
    assert!(path.exists());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("7"), "should contain cycle number");
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn persist_cycle_report_multiple_cycles_coexist() {
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-scratch")
        .join(format!("ooda-multi-cycle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

    for i in 0..5 {
        persist_cycle_report(&scratch, &make_test_report(i));
    }
    for i in 0..5 {
        let path = scratch
            .join("cycle_reports")
            .join(format!("cycle_{i}.json"));
        assert!(path.exists(), "cycle {i} file should exist");
    }
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn persist_cycle_report_uses_serde_derive_for_all_fields() {
    // Regression for the divergence-class bug fixed in PR #1480: the
    // hand-rolled brain_judgments mapping in `persist_cycle_report`
    // silently dropped any field added to `BrainJudgmentRecord` (e.g.
    // PR #1476's `prompt_version`). Persisting via
    // `serde_json::to_value(&record)` makes the struct's `Serialize`
    // derive the single source of truth — assert that here so any future
    // field addition is caught by `cargo test` instead of going missing
    // from cycle reports for days.
    use crate::ooda_brain::{BrainJudgmentRecord, BrainPhase};

    let record = BrainJudgmentRecord {
        phase: BrainPhase::Decide,
        context_summary: "goal_id=ship-v1 urgency=0.900".to_string(),
        decision: "advance_goal".to_string(),
        rationale: "high priority".to_string(),
        confidence: 0.95,
        fallback: false,
        prompt_version: "abc123def456".to_string(),
    };

    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-scratch")
        .join(format!("ooda-serde-derive-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

    let mut report = make_test_report(77);
    report.brain_judgments.push(record.clone());
    persist_cycle_report(&scratch, &report);

    let content =
        std::fs::read_to_string(scratch.join("cycle_reports").join("cycle_77.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let persisted = parsed["brain_judgments"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_object())
        .expect("brain_judgments[0] must be a JSON object");

    // (a) Round-trip: every populated field of the input record appears in
    //     the persisted JSON with its original value.
    assert_eq!(persisted["phase"], "decide");
    assert_eq!(persisted["context_summary"], record.context_summary);
    assert_eq!(persisted["decision"], record.decision);
    assert_eq!(persisted["rationale"], record.rationale);
    assert_eq!(persisted["confidence"], record.confidence);
    assert_eq!(persisted["fallback"], record.fallback);
    assert_eq!(persisted["prompt_version"], record.prompt_version);

    // (b) Key-set equality with the struct's own Serialize derive: if
    //     anyone adds a new field to BrainJudgmentRecord, this assertion
    //     fires immediately instead of letting the field be silently
    //     dropped from cycle reports (cf. PR #1480).
    let expected: std::collections::BTreeSet<String> = serde_json::to_value(&record)
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let actual: std::collections::BTreeSet<String> = persisted.keys().cloned().collect();
    assert_eq!(
        actual, expected,
        "persisted brain_judgments[0] keys must match the struct's Serialize derive output \
         (divergence-class bug fixed in PR #1480 — keep auto-derive as source of truth)"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}
