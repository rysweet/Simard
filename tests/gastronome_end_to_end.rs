//! End-to-end test for the `gastronome-kitchen` binary: a brief travels from
//! `sample-brief` through `plan` to a costed, scheduled plan without touching
//! the library internals — the outside-in contract the Gastronome identity
//! relies on.

use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_gastronome-kitchen")
}

fn run(args: &[&str], stdin: Option<&str>) -> (String, String, i32) {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn gastronome-kitchen");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn sample_brief_to_json_plan_end_to_end() {
    // 1. Generate a sample brief.
    let (brief_json, _err, code) = run(&["sample-brief"], None);
    assert_eq!(code, 0, "sample-brief should succeed");
    assert!(brief_json.contains("Summer Garden Luncheon"));

    // 2. Plan it, reading the brief from stdin.
    let (plan_json, err, code) = run(
        &["plan", "--brief", "-", "--format", "json"],
        Some(&brief_json),
    );
    assert_eq!(code, 0, "plan should succeed; stderr={err}");

    let plan: serde_json::Value = serde_json::from_str(&plan_json).expect("plan is valid JSON");
    assert_eq!(plan["guest_count"], 24);
    assert_eq!(plan["scaled_recipes"].as_array().unwrap().len(), 3);
    assert!(plan["cost"]["per_guest_usd"].as_f64().unwrap() > 0.0);
    assert_eq!(plan["budget"]["status"], "within_budget");

    // Every scheduled task must finish by the event start.
    let event_start = plan["event_start"].as_str().unwrap();
    for task in plan["schedule"]["tasks"].as_array().unwrap() {
        assert!(task["end"].as_str().unwrap() <= event_start);
    }
}

#[test]
fn text_format_renders_human_summary() {
    let (brief_json, _e, _c) = run(&["sample-brief"], None);
    let (text, err, code) = run(
        &["plan", "--brief", "-", "--format", "text"],
        Some(&brief_json),
    );
    assert_eq!(code, 0, "text plan should succeed; stderr={err}");
    assert!(text.contains("Menu plan — Summer Garden Luncheon (24 guests)"));
    assert!(text.contains("EVENT TOTAL"));
    assert!(text.contains("Nutrition per guest"));
    assert!(text.contains("Prep schedule"));
}

#[test]
fn invalid_brief_exits_nonzero_with_error_envelope() {
    let (out, err, code) = run(
        &["plan", "--brief", "-", "--format", "json"],
        Some("{ not json"),
    );
    assert_eq!(code, 2, "invalid JSON should exit 2");
    assert!(out.is_empty(), "no stdout on error");
    let envelope: serde_json::Value =
        serde_json::from_str(err.trim()).expect("error envelope JSON");
    assert!(
        envelope["error"]
            .as_str()
            .unwrap()
            .contains("invalid brief JSON")
    );
}

#[test]
fn missing_subcommand_exits_nonzero() {
    let (_out, err, code) = run(&[], None);
    assert_eq!(code, 2);
    assert!(err.contains("missing subcommand"));
}
