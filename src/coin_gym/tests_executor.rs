use super::executor::{
    ALLOWED_COIN_SUBCOMMANDS, ANSWER_BLOB_BIN, ANSWER_BLOB_HARNESS, ANSWER_UNREACHABLE_MD,
    AgentAnswer, CoinEvaluateConfig, CoinEvaluateExecutor, CoinResultJson, CoinResultsExecutor,
    EvaluateSource, GradeResult, HarnessExecutor, LOCAL_ONLY, MockHarnessExecutor,
    grade_from_result, outcome_from_result, parse_experiment_id, write_answer,
};
use super::types::{OutcomeCode, Target, TargetFamily};

fn target(id: &str) -> Target {
    Target {
        id: id.to_string(),
        project: "libraw".to_string(),
        commit: "abc".to_string(),
        harness: "libraw_raf_fuzzer".to_string(),
        file: "src/metadata/fuji.cpp".to_string(),
        line: 480,
        line_end: None,
        family: TargetFamily::Frontier,
    }
}

#[test]
fn grade_result_maps_to_outcome_code() {
    assert_eq!(GradeResult::Reached.to_outcome_code(), OutcomeCode::Reached);
    assert_eq!(
        GradeResult::WrongInput.to_outcome_code(),
        OutcomeCode::WrongInput
    );
    assert_eq!(
        GradeResult::TimedOut.to_outcome_code(),
        OutcomeCode::TimedOut
    );
    assert_eq!(GradeResult::Error.to_outcome_code(), OutcomeCode::Error);
}

#[test]
fn mock_reaches_only_on_matching_input() {
    let exec = MockHarnessExecutor::new().with_reaching_input("t1", "magic-bytes");
    let t = target("t1");
    assert_eq!(exec.grade(&t, "magic-bytes").unwrap(), GradeResult::Reached);
    assert_eq!(exec.grade(&t, "wrong").unwrap(), GradeResult::WrongInput);
}

#[test]
fn mock_unknown_target_is_wrong_input() {
    let exec = MockHarnessExecutor::new();
    assert_eq!(
        exec.grade(&target("unknown"), "anything").unwrap(),
        GradeResult::WrongInput
    );
}

#[test]
fn mock_injected_timeout_and_error_take_precedence() {
    let exec = MockHarnessExecutor::new()
        .with_reaching_input("t1", "ok")
        .with_timeout("t1")
        .with_error("t2");
    // error dominates for t2
    assert_eq!(exec.grade(&target("t2"), "x").unwrap(), GradeResult::Error);
    // timeout dominates even the correct input for t1
    assert_eq!(
        exec.grade(&target("t1"), "ok").unwrap(),
        GradeResult::TimedOut
    );
}

#[test]
fn mock_is_flagged_offline_scaffold() {
    assert!(MockHarnessExecutor::new().is_offline_scaffold());
}

#[test]
fn coin_evaluate_delegate_is_phase3_gated() {
    let exec = CoinEvaluateExecutor::new(CoinEvaluateConfig::new("COIN-Bench/coin", "v2026-07"));
    assert!(!exec.is_offline_scaffold());
    let err = exec.grade(&target("t1"), "bytes").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Docker"));
    assert!(msg.contains("Phase 3"));
}

#[test]
fn coin_evaluate_builds_snapshot_argv() {
    // Real contract: --dataset/--revision, optional repeatable --split/--project,
    // and --source; NO fictional --target/--input flags.
    let exec = CoinEvaluateExecutor::new(
        CoinEvaluateConfig::new("COIN-Bench/coin", "v2026-07")
            .with_split("codeql_only")
            .with_project("cups")
            .with_source(EvaluateSource::Image),
    );
    let argv = exec.build_evaluate_argv();
    assert_eq!(
        argv,
        vec![
            "coin".to_string(),
            "evaluate".to_string(),
            "--dataset".to_string(),
            "COIN-Bench/coin".to_string(),
            "--revision".to_string(),
            "v2026-07".to_string(),
            "--split".to_string(),
            "codeql_only".to_string(),
            "--project".to_string(),
            "cups".to_string(),
            "--source".to_string(),
            "image".to_string(),
        ]
    );
    // The evaluate argv must never re-introduce the fictional per-input flags.
    assert!(!argv.iter().any(|a| a == "--target" || a == "--input"));
}

#[test]
fn coin_evaluate_defaults_to_all_splits_and_rebuild() {
    let exec = CoinEvaluateExecutor::new(CoinEvaluateConfig::new("COIN-Bench/coin", "v2026-07"));
    let argv = exec.build_evaluate_argv();
    assert_eq!(
        argv,
        vec![
            "coin",
            "evaluate",
            "--dataset",
            "COIN-Bench/coin",
            "--revision",
            "v2026-07",
            "--source",
            "rebuild",
        ]
    );
}

#[test]
fn coin_evaluate_repeats_splits_and_projects() {
    let exec = CoinEvaluateExecutor::new(
        CoinEvaluateConfig::new("COIN-Bench/coin", "v2026-07")
            .with_split("codeql_only")
            .with_split("gcs_reachable")
            .with_project("cups")
            .with_project("libraw"),
    );
    let argv = exec.build_evaluate_argv();
    assert_eq!(argv.iter().filter(|a| *a == "--split").count(), 2);
    assert_eq!(argv.iter().filter(|a| *a == "--project").count(), 2);
}

#[test]
fn dataset_ref_shorthand_parses_repo_and_revision() {
    let cfg = CoinEvaluateConfig::from_dataset_ref("COIN-Bench/coin@v2026-07").unwrap();
    assert_eq!(cfg.dataset, "COIN-Bench/coin");
    assert_eq!(cfg.revision, "v2026-07");
    assert_eq!(cfg.source, EvaluateSource::Rebuild);
    // Missing '@revision' is a usage error, not a silent default.
    assert!(CoinEvaluateConfig::from_dataset_ref("COIN-Bench/coin").is_err());
    assert!(CoinEvaluateConfig::from_dataset_ref("COIN-Bench/coin@").is_err());
    assert!(CoinEvaluateConfig::from_dataset_ref("@v1").is_err());
}

#[test]
fn coin_verify_argv_targets_the_minted_experiment() {
    let exec = CoinEvaluateExecutor::new(CoinEvaluateConfig::new("COIN-Bench/coin", "v2026-07"));
    assert_eq!(
        exec.build_verify_argv("exp-123", None),
        vec!["coin", "verify", "--experiment", "exp-123"]
    );
    assert_eq!(
        exec.build_verify_argv("exp-123", Some(4)),
        vec![
            "coin".to_string(),
            "verify".to_string(),
            "--experiment".to_string(),
            "exp-123".to_string(),
            "--max-concurrent".to_string(),
            "4".to_string(),
        ]
    );
}

#[test]
fn experiment_id_is_parsed_from_evaluate_output() {
    assert_eq!(
        parse_experiment_id("running...\nexperiment: exp-2026-07-abc\ndone"),
        Some("exp-2026-07-abc".to_string())
    );
    assert_eq!(
        parse_experiment_id("wrote output/experiments/exp-xyz/results/"),
        Some("exp-xyz".to_string())
    );
    // A trailing note after the id must not leak into the parsed value.
    assert_eq!(
        parse_experiment_id("experiment: exp-1 (70 items queued)"),
        Some("exp-1".to_string())
    );
    assert_eq!(parse_experiment_id("no id here"), None);
}

#[test]
fn local_only_guardrail_excludes_publish_and_submit() {
    const { assert!(LOCAL_ONLY) };
    // The crate only ever drives the read/measure subcommands.
    assert_eq!(ALLOWED_COIN_SUBCOMMANDS, &["evaluate", "verify"]);
    let exec = CoinEvaluateExecutor::new(CoinEvaluateConfig::new("COIN-Bench/coin", "v2026-07"));
    let mut argv = exec.build_evaluate_argv();
    argv.extend(exec.build_verify_argv("exp-1", Some(2)));
    for forbidden in [
        "publish",
        "--hf-repo",
        "--registry",
        "submit",
        "leaderboard",
    ] {
        assert!(
            !argv.iter().any(|a| a.contains(forbidden)),
            "LOCAL-ONLY: argv must never contain '{forbidden}'"
        );
    }
}

// ── Submission contract (`/answer/`) ─────────────────────────────────────────

#[test]
fn write_answer_attempt_writes_blob_and_harness() {
    let dir = tempfile::tempdir().unwrap();
    let answer = dir.path().join("answer");
    write_answer(
        &answer,
        &AgentAnswer::Attempt {
            blob: b"\x00magic-bytes".to_vec(),
            harness: "libraw_raf_fuzzer".to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        std::fs::read(answer.join(ANSWER_BLOB_BIN)).unwrap(),
        b"\x00magic-bytes"
    );
    assert_eq!(
        std::fs::read_to_string(answer.join(ANSWER_BLOB_HARNESS)).unwrap(),
        "libraw_raf_fuzzer"
    );
    assert!(!answer.join(ANSWER_UNREACHABLE_MD).exists());
}

#[test]
fn write_answer_attempt_removes_stale_unreachable_marker() {
    let dir = tempfile::tempdir().unwrap();
    let answer = dir.path().join("answer");
    // A prior abstention left an UNREACHABLE.md behind.
    write_answer(
        &answer,
        &AgentAnswer::Abstain {
            unreachable_md: "unreachable: no path".to_string(),
        },
    )
    .unwrap();
    // A subsequent attempt must clear it so COIN grades the blob, not abstain.
    write_answer(
        &answer,
        &AgentAnswer::Attempt {
            blob: b"bytes".to_vec(),
            harness: "h".to_string(),
        },
    )
    .unwrap();
    assert!(answer.join(ANSWER_BLOB_BIN).exists());
    assert!(!answer.join(ANSWER_UNREACHABLE_MD).exists());
}

#[test]
fn write_answer_abstain_removes_stale_blob() {
    let dir = tempfile::tempdir().unwrap();
    let answer = dir.path().join("answer");
    write_answer(
        &answer,
        &AgentAnswer::Attempt {
            blob: b"bytes".to_vec(),
            harness: "h".to_string(),
        },
    )
    .unwrap();
    // Abstaining after an attempt must remove blob.bin (COIN treats a present
    // blob.bin as an attempt and ignores UNREACHABLE.md otherwise).
    write_answer(
        &answer,
        &AgentAnswer::Abstain {
            unreachable_md: "unreachable: guarded by an unsatisfiable constant".to_string(),
        },
    )
    .unwrap();
    assert!(!answer.join(ANSWER_BLOB_BIN).exists());
    assert!(!answer.join(ANSWER_BLOB_HARNESS).exists());
    assert!(answer.join(ANSWER_UNREACHABLE_MD).exists());
}

#[test]
fn write_answer_rejects_empty_or_multiline_harness() {
    let dir = tempfile::tempdir().unwrap();
    let answer = dir.path().join("answer");
    let empty = write_answer(
        &answer,
        &AgentAnswer::Attempt {
            blob: b"x".to_vec(),
            harness: "  ".to_string(),
        },
    );
    assert!(empty.is_err());
    let multiline = write_answer(
        &answer,
        &AgentAnswer::Attempt {
            blob: b"x".to_vec(),
            harness: "a\nb".to_string(),
        },
    );
    assert!(multiline.is_err());
}

// ── Reading `reached` from result.json ───────────────────────────────────────

#[test]
fn outcome_from_result_maps_reached_and_status() {
    let reached = CoinResultJson {
        reached: Some(true),
        ..Default::default()
    };
    assert_eq!(outcome_from_result(&reached), OutcomeCode::Reached);
    let wrong = CoinResultJson {
        reached: Some(false),
        ..Default::default()
    };
    assert_eq!(outcome_from_result(&wrong), OutcomeCode::WrongInput);
    for (status, code) in [
        ("timeout", OutcomeCode::TimedOut),
        ("error", OutcomeCode::Error),
        ("abstained", OutcomeCode::Abstained),
        ("no_submission", OutcomeCode::NoSubmission),
    ] {
        let r = CoinResultJson {
            status: Some(status.to_string()),
            // A decisive status wins even if a stale reached bool is present.
            reached: Some(true),
            ..Default::default()
        };
        assert_eq!(outcome_from_result(&r), code, "status {status}");
    }
}

#[test]
fn outcome_from_result_without_verdict_is_error_or_no_submission() {
    // verify never ran and nothing submitted → N.
    let none_sub = CoinResultJson {
        submitted: Some(false),
        ..Default::default()
    };
    assert_eq!(outcome_from_result(&none_sub), OutcomeCode::NoSubmission);
    // submitted but no verdict → honest Error (never a silent pass).
    let no_verdict = CoinResultJson {
        submitted: Some(true),
        ..Default::default()
    };
    assert_eq!(outcome_from_result(&no_verdict), OutcomeCode::Error);
}

#[test]
fn grade_from_result_collapses_non_submission_outcomes() {
    assert_eq!(
        grade_from_result(&CoinResultJson {
            reached: Some(true),
            ..Default::default()
        }),
        GradeResult::Reached
    );
    assert_eq!(
        grade_from_result(&CoinResultJson {
            status: Some("abstained".to_string()),
            ..Default::default()
        }),
        GradeResult::Error
    );
}

#[test]
fn result_json_parses_the_reached_field() {
    let r = CoinResultJson::parse(r#"{"target_id":"libraw:h:f:1","reached":true}"#).unwrap();
    assert_eq!(r.reached, Some(true));
    assert_eq!(r.target_id.as_deref(), Some("libraw:h:f:1"));
    assert!(CoinResultJson::parse("{ not json").is_err());
}

// ── CoinResultsExecutor: read real grades from disk ──────────────────────────

fn write_result_json(results_dir: &std::path::Path, target_id: &str, body: &str) {
    let dir = results_dir.join(CoinResultsExecutor::target_slug(target_id));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("result.json"), body).unwrap();
}

#[test]
fn results_executor_reads_reached_from_result_json() {
    let dir = tempfile::tempdir().unwrap();
    let results = dir.path();
    let t = target("libraw:libraw_raf_fuzzer:src/metadata/fuji.cpp:480");
    write_result_json(results, &t.id, r#"{"reached":true}"#);
    let exec = CoinResultsExecutor::new(results);
    assert!(!exec.is_offline_scaffold(), "real grades are not scaffold");
    assert_eq!(exec.grade(&t, "ignored").unwrap(), GradeResult::Reached);

    write_result_json(results, &t.id, r#"{"reached":false}"#);
    assert_eq!(exec.grade(&t, "ignored").unwrap(), GradeResult::WrongInput);

    write_result_json(results, &t.id, r#"{"status":"timeout"}"#);
    assert_eq!(exec.grade(&t, "ignored").unwrap(), GradeResult::TimedOut);
}

#[test]
fn results_executor_full_outcome_reads_abstain_and_no_submission() {
    let dir = tempfile::tempdir().unwrap();
    let results = dir.path();
    let t = target("proj:h:f:1");
    write_result_json(results, &t.id, r#"{"status":"abstained"}"#);
    let exec = CoinResultsExecutor::new(results);
    assert_eq!(exec.outcome(&t.id).unwrap(), OutcomeCode::Abstained);
}

#[test]
fn results_executor_missing_result_is_executor_error() {
    let dir = tempfile::tempdir().unwrap();
    let exec = CoinResultsExecutor::new(dir.path());
    let err = exec.grade(&target("nope:h:f:1"), "x").unwrap_err();
    assert!(err.to_string().contains("executor error"));
}

#[test]
fn results_executor_slug_is_path_safe() {
    // A raw target id (with ':' and '/') must never escape the results dir.
    let slug = CoinResultsExecutor::target_slug("proj:harness:src/../../etc/passwd:1");
    assert!(!slug.contains('/') && !slug.contains(':') && !slug.contains(".."));
}

#[test]
fn from_oracle_seeds_reaching_inputs() {
    let mut oracle = std::collections::HashMap::new();
    oracle.insert("t1".to_string(), "seed".to_string());
    let exec = MockHarnessExecutor::from_oracle(oracle);
    assert_eq!(
        exec.grade(&target("t1"), "seed").unwrap(),
        GradeResult::Reached
    );
}
