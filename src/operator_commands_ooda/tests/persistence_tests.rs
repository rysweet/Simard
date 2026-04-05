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
