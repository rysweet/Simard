//! Outside-in contract tests for the next product block after the bounded
//! engineer loop landed in PR #55.
//!
//! `Specs/ProductArchitecture.md` defines five operator-visible modes plus a
//! terminal-backed engineer substrate. The canonical `simard` CLI is now the
//! primary surface for those shipped behaviors, while `simard_operator_probe`
//! and `simard-gym` remain compatibility surfaces for older scripts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn engineer_loop_objective() -> &'static str {
    "inspect the repository state, execute one safe local engineering action, verify the outcome explicitly, and persist truthful local evidence and memory"
}

fn rendered_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}{stderr}")
}

fn output_line_value<'a>(rendered: &'a str, prefix: &str) -> Option<&'a str> {
    rendered
        .lines()
        .find_map(|line| line.strip_prefix(prefix).map(str::trim))
}

fn load_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_str(&fs::read_to_string(path.as_ref()).expect("artifact should be readable"))
        .expect("artifact should deserialize as JSON")
}

fn command_in_dir(binary_path: &str, dir: &Path) -> Command {
    let mut command = Command::new(binary_path);
    command.current_dir(dir);
    command
}

fn resolve_cli_artifact_path(command_dir: &Path, surfaced_path: &str) -> PathBuf {
    let surfaced_path = Path::new(surfaced_path);
    if surfaced_path.is_absolute() {
        surfaced_path.to_path_buf()
    } else {
        command_dir.join(surfaced_path)
    }
}

fn replace_output_line_value(rendered: &str, prefix: &str, replacement: &str) -> String {
    rendered
        .lines()
        .map(|line| {
            if line.starts_with(prefix) {
                format!("{prefix}{replacement}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_benchmark_run_output(rendered: &str) -> String {
    let rendered = replace_output_line_value(rendered, "Session: ", "<session>");
    let rendered = replace_output_line_value(
        &rendered,
        "Artifact report: ",
        "target/simard-gym/repo-exploration-local/<session>/report.json",
    );
    let rendered = replace_output_line_value(
        &rendered,
        "Artifact summary: ",
        "target/simard-gym/repo-exploration-local/<session>/report.txt",
    );
    replace_output_line_value(
        &rendered,
        "Review artifact: ",
        "target/simard-gym/repo-exploration-local/<session>/review.json",
    )
}

fn write_legacy_benchmark_report(command_dir: &Path, scenario_id: &str, session_id: &str) {
    let report_dir = command_dir
        .join("target/simard-gym")
        .join(scenario_id)
        .join(session_id);
    fs::create_dir_all(&report_dir).expect("legacy benchmark artifact directory should exist");
    fs::write(
        report_dir.join("report.json"),
        serde_json::to_string_pretty(&json!({
            "suite_id": "starter",
            "scenario": {
                "id": scenario_id,
                "title": "Repo exploration on local harness",
            },
            "session_id": session_id,
            "run_started_at_unix_ms": 1_u128,
            "passed": true,
            "scorecard": {
                "correctness_checks_passed": 8,
                "correctness_checks_total": 8,
                "evidence_quality": "sufficient"
            },
            "handoff": {
                "exported_memory_records": 3,
                "exported_evidence_records": 4
            }
        }))
        .expect("legacy benchmark report should serialize"),
    )
    .expect("legacy benchmark report should be written");
}

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{label}-{unique}"));
        fs::create_dir_all(&path).expect("temp dir should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[test]
fn simard_help_surfaces_the_five_product_modes_and_operator_utilities() {
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("--help")
        .output()
        .expect("simard help should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "the primary simard binary should expose operator help instead of bootstrap env failures:\n{rendered}"
    );
    for expected in [
        "engineer",
        "terminal",
        "meeting",
        "goal-curation",
        "improvement-curation",
        "gym",
        "compare",
        "review",
        "bootstrap",
    ] {
        assert!(
            rendered.contains(expected),
            "simard help should document '{expected}' on the unified operator surface:\n{rendered}"
        );
    }
    assert!(
        !rendered.contains("SIMARD_PROMPT_ROOT"),
        "help should not fall through to the legacy env-only bootstrap entrypoint:\n{rendered}"
    );
}

#[test]
fn bare_simard_shows_unified_help_instead_of_bootstrap_env_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .output()
        .expect("bare simard should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "bare simard should stay on the unified CLI surface:\n{rendered}"
    );
    assert!(
        rendered.contains("Simard unified operator CLI"),
        "bare simard should show the operator help text:\n{rendered}"
    );
    assert!(
        !rendered.contains("SIMARD_PROMPT_ROOT"),
        "bare simard should not fall back to the legacy env-only bootstrap path:\n{rendered}"
    );
}

#[test]
fn simard_engineer_run_drives_the_bounded_engineer_loop_from_the_primary_cli() {
    let state_root = TempDirGuard::new("simard-cli-engineer");
    let repo_root = repo_root();
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("run")
        .arg("single-process")
        .arg(&repo_root)
        .arg(engineer_loop_objective())
        .arg(state_root.path())
        .output()
        .expect("simard engineer run should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "simard engineer run should expose the shipped bounded engineer loop through the primary CLI:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("Repo root: {}", repo_root.display())),
        "engineer mode should expose the repo root it inspected:\n{rendered}"
    );
    assert!(
        rendered.contains("Execution scope: local-only"),
        "engineer mode should stay honest about the shipped local-only execution scope:\n{rendered}"
    );
    assert!(
        rendered.contains("Verification status: verified"),
        "engineer mode should preserve explicit verification output:\n{rendered}"
    );
    assert!(
        state_root.path().join("memory_records.json").is_file(),
        "engineer mode should persist durable memory under the chosen state root"
    );
    assert!(
        state_root.path().join("evidence_records.json").is_file(),
        "engineer mode should persist durable evidence under the chosen state root"
    );
    assert!(
        state_root.path().join("latest_handoff.json").is_file(),
        "engineer mode should persist the latest handoff under the chosen state root"
    );
}

#[test]
fn simard_engineer_terminal_exposes_the_terminal_backed_engineer_surface() {
    let state_root = TempDirGuard::new("simard-cli-terminal");
    let objective = "working-directory: .\ncommand: pwd\ncommand: printf \"terminal-cli-ok\\n\"";
    let simard_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("terminal")
        .arg("single-process")
        .arg(objective)
        .arg(state_root.path())
        .output()
        .expect("simard engineer terminal should launch");
    let simard_rendered = rendered_output(&simard_output);

    assert!(
        simard_output.status.success(),
        "simard engineer terminal should expose the terminal-backed engineer substrate through the canonical CLI:\n{simard_rendered}"
    );
    for expected in [
        "Selected base type: terminal-shell",
        "Adapter implementation: terminal-shell::local-pty",
        "terminal-cli-ok",
        &format!("State root: {}", state_root.path().display()),
    ] {
        assert!(
            simard_rendered.contains(expected),
            "terminal engineer mode should surface '{expected}' for operators:\n{simard_rendered}"
        );
    }

    let legacy_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("terminal-run")
        .arg("single-process")
        .arg(objective)
        .arg(state_root.path())
        .output()
        .expect("legacy terminal-run should launch");
    let legacy_rendered = rendered_output(&legacy_output);

    assert!(
        legacy_output.status.success(),
        "legacy terminal-run should remain functional while operators migrate:\n{legacy_rendered}"
    );
    assert_eq!(
        simard_rendered, legacy_rendered,
        "the canonical terminal engineer surface should preserve the legacy terminal-run output exactly until operators migrate"
    );
    for expected in [
        "memory_records.json",
        "evidence_records.json",
        "latest_handoff.json",
    ] {
        assert!(
            state_root.path().join(expected).is_file(),
            "terminal engineer mode should persist {expected} under the selected state root"
        );
    }
}

#[test]
fn simard_meeting_and_goal_curation_run_as_primary_modes_not_probe_only_commands() {
    let meeting_state = TempDirGuard::new("simard-cli-meeting");
    let meeting_objective = "\
agenda: align the next Simard workstream\n\
decision: preserve meeting-to-engineer continuity\n\
risk: workflow routing is still unreliable\n\
next-step: keep durable priorities visible\n\
open-question: how aggressively should Simard reprioritize?\n\
goal: Preserve meeting handoff | priority=1 | status=active | rationale=meeting decisions must shape later work\n\
goal: Keep outside-in verification strong | priority=2 | status=active | rationale=operator confidence depends on real product exercise";

    let meeting_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(meeting_objective)
        .arg(meeting_state.path())
        .output()
        .expect("simard meeting run should launch");
    let meeting_rendered = rendered_output(&meeting_output);

    assert!(
        meeting_output.status.success(),
        "meeting mode should be available through simard itself:\n{meeting_rendered}"
    );
    assert!(
        meeting_rendered.contains("Identity: simard-meeting"),
        "meeting mode should preserve the meeting identity surface:\n{meeting_rendered}"
    );
    assert!(
        meeting_rendered.contains("Decision records: 1"),
        "meeting mode should persist a decision record when the objective includes one:\n{meeting_rendered}"
    );
    assert!(
        meeting_rendered.contains("Active goals count: 2"),
        "meeting mode should surface durable goals from the structured objective:\n{meeting_rendered}"
    );

    let goal_state = TempDirGuard::new("simard-cli-goal-curation");
    let goal_objective = "\
goal: Maintain a truthful top 5 | priority=1 | status=active | rationale=core Simard stewardship\n\
goal: Keep meeting handoff durable | priority=2 | status=active | rationale=meeting updates must influence engineering\n\
goal: Preserve outside-in operator coverage | priority=3 | status=active | rationale=user requires real operator validation\n\
goal: Improve composite identities | priority=4 | status=active | rationale=composite roles should stay explicit\n\
goal: Build realistic tool driving | priority=5 | status=active | rationale=terminal-native behavior is central\n\
goal: Track future remote orchestration | priority=6 | status=active | rationale=important but not top-five current work";

    let goal_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(goal_objective)
        .arg(goal_state.path())
        .output()
        .expect("simard goal-curation run should launch");
    let goal_rendered = rendered_output(&goal_output);

    assert!(
        goal_output.status.success(),
        "goal-curation mode should be available through simard itself:\n{goal_rendered}"
    );
    assert!(
        goal_rendered.contains("Identity: simard-goal-curator"),
        "goal-curation mode should preserve the curator identity surface:\n{goal_rendered}"
    );
    assert!(
        goal_rendered.contains("Active goals count: 5"),
        "goal-curation mode should preserve the active top five contract:\n{goal_rendered}"
    );
    assert!(
        !goal_rendered.contains("Track future remote orchestration"),
        "goal-curation mode should omit lower-priority active goals from the top-five surface:\n{goal_rendered}"
    );
}

#[test]
fn simard_rejects_invalid_state_roots_before_any_persistence_write() {
    let temp_dir = TempDirGuard::new("simard-cli-invalid-state-root");
    let bad_parent_dir_root = temp_dir.path().join("../escape");
    let traversal_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg("decision: keep the state root honest")
        .arg(&bad_parent_dir_root)
        .output()
        .expect("traversal rejection should launch");
    let traversal_rendered = rendered_output(&traversal_output);

    assert!(
        !traversal_output.status.success(),
        "state-root traversal must fail visibly:\n{traversal_rendered}"
    );
    assert!(
        traversal_rendered.contains("must not contain '..'"),
        "state-root traversal should explain why the path was rejected:\n{traversal_rendered}"
    );
    assert!(
        traversal_rendered.contains("InvalidStateRoot")
            || traversal_rendered.contains("invalid state root"),
        "state-root validation should stay explicit about the failing contract:\n{traversal_rendered}"
    );

    let file_path = temp_dir.path().join("not-a-directory");
    fs::write(&file_path, "file").expect("state-root file should be created");
    let file_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg("decision: keep the state root honest")
        .arg(&file_path)
        .output()
        .expect("file rejection should launch");
    let file_rendered = rendered_output(&file_output);

    assert!(
        !file_output.status.success(),
        "non-directory state roots must fail visibly:\n{file_rendered}"
    );
    assert!(
        file_rendered.contains("state root must resolve to a directory"),
        "existing file state roots should be rejected explicitly:\n{file_rendered}"
    );
}

#[cfg(unix)]
#[test]
fn simard_rejects_symlink_state_roots() {
    use std::os::unix::fs::symlink;

    let temp_dir = TempDirGuard::new("simard-cli-symlink-state-root");
    let real_dir = temp_dir.path().join("real");
    let link_dir = temp_dir.path().join("link");
    fs::create_dir_all(&real_dir).expect("real state directory should be created");
    symlink(&real_dir, &link_dir).expect("symlink state root should be created");

    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("review")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&link_dir)
        .output()
        .expect("symlink rejection should launch");
    let rendered = rendered_output(&output);

    assert!(
        !output.status.success(),
        "symlink state roots must fail visibly:\n{rendered}"
    );
    assert!(
        rendered.contains("state root must not be a symlink"),
        "symlink state roots should be rejected explicitly:\n{rendered}"
    );
}

#[test]
fn simard_sanitizes_persisted_operator_output_before_printing() {
    let state_root = TempDirGuard::new("simard-cli-sanitized-output");
    let objective = "decision: keep \u{1b}[31mred\u{1b}[0m output safe";
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(objective)
        .arg(state_root.path())
        .output()
        .expect("meeting sanitization check should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "meeting mode should still succeed while sanitizing operator output:\n{rendered}"
    );
    assert!(
        !rendered.contains('\u{1b}'),
        "operator-visible output should strip ANSI escape sequences:\n{rendered}"
    );
    assert!(
        rendered.contains("decisions=[keep red output safe]"),
        "persisted decision records should be rendered in sanitized form:\n{rendered}"
    );
}

#[test]
fn simard_review_and_improvement_curation_share_one_operator_facing_cli() {
    let state_root = TempDirGuard::new("simard-cli-improvement-curation");

    let review_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("review")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg("inspect the current Simard review surface and preserve concrete proposals")
        .arg(state_root.path())
        .output()
        .expect("simard review run should launch");
    let review_rendered = rendered_output(&review_output);

    assert!(
        review_output.status.success(),
        "review mode should be reachable from the primary CLI:\n{review_rendered}"
    );
    assert!(
        review_rendered.contains("Review proposals: 2"),
        "review mode should surface the concrete proposals produced by the shipped review loop:\n{review_rendered}"
    );

    let review_read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("review")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard review read should launch");
    let review_read_rendered = rendered_output(&review_read_output);

    assert!(
        review_read_output.status.success(),
        "review read should expose the latest persisted review artifact through simard:\n{review_read_rendered}"
    );
    assert!(
        review_read_rendered.contains("Latest review artifact:"),
        "review read should surface the persisted artifact path:\n{review_read_rendered}"
    );

    let improvement_objective = "\
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now\n\
approve: Promote this pattern into a repeatable benchmark | priority=2 | status=proposed | rationale=carry this into the next benchmark planning pass";
    let improvement_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("improvement-curation")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(improvement_objective)
        .arg(state_root.path())
        .output()
        .expect("simard improvement-curation run should launch");
    let improvement_rendered = rendered_output(&improvement_output);

    assert!(
        improvement_output.status.success(),
        "improvement-curation mode should be reachable from the primary CLI:\n{improvement_rendered}"
    );
    assert!(
        improvement_rendered.contains("Identity: simard-improvement-curator"),
        "improvement-curation mode should preserve the curator identity surface:\n{improvement_rendered}"
    );
    assert!(
        improvement_rendered.contains("Approved proposals: 2"),
        "improvement-curation mode should honor explicit operator approvals:\n{improvement_rendered}"
    );
    assert!(
        improvement_rendered.contains("Active goals count: 1"),
        "approved active improvements should become durable active goals:\n{improvement_rendered}"
    );
}

#[test]
fn simard_gym_list_matches_the_legacy_benchmark_binary_for_compatibility() {
    let simard_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("gym")
        .arg("list")
        .output()
        .expect("simard gym list should launch");
    let simard_rendered = rendered_output(&simard_output);

    assert!(
        simard_output.status.success(),
        "gym mode should be exposed through the primary simard CLI:\n{simard_rendered}"
    );
    assert!(
        simard_rendered.contains("repo-exploration-local")
            && simard_rendered.contains("docs-refresh-copilot")
            && simard_rendered.contains("safe-code-change-rusty-clawd")
            && simard_rendered.contains("composite-session-review"),
        "gym list should surface the shipped benchmark scenarios:\n{simard_rendered}"
    );

    let legacy_output = Command::new(env!("CARGO_BIN_EXE_simard-gym"))
        .arg("list")
        .output()
        .expect("legacy simard-gym list should launch");
    let legacy_rendered = rendered_output(&legacy_output);

    assert!(
        legacy_output.status.success(),
        "legacy simard-gym should remain functional while the unified CLI becomes canonical:\n{legacy_rendered}"
    );
    assert_eq!(
        simard_rendered, legacy_rendered,
        "the unified gym surface should preserve the legacy list output exactly until operators migrate"
    );
}

#[test]
fn simard_gym_run_surfaces_measured_action_and_retry_metrics_and_persists_them_truthfully() {
    let artifact_root = TempDirGuard::new("simard-cli-gym-run");
    let output = command_in_dir(env!("CARGO_BIN_EXE_simard"), artifact_root.path())
        .arg("gym")
        .arg("run")
        .arg("repo-exploration-local")
        .output()
        .expect("simard gym run should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "gym run should succeed on the primary operator surface:\n{rendered}"
    );
    for expected in ["Unnecessary actions:", "Retry count:"] {
        assert!(
            rendered.contains(expected),
            "gym run should surface '{expected}' through the operator-facing CLI instead of hiding the metric in artifacts only:\n{rendered}"
        );
    }

    let report_path = output_line_value(&rendered, "Artifact report: ")
        .expect("gym run should surface the report.json artifact path");
    let review_path = output_line_value(&rendered, "Review artifact: ")
        .expect("gym run should surface the review.json artifact path");
    let report = load_json(resolve_cli_artifact_path(artifact_root.path(), report_path));
    let review = load_json(resolve_cli_artifact_path(artifact_root.path(), review_path));

    assert!(
        report["scorecard"]["unnecessary_action_count"].is_number(),
        "fresh benchmark runs should persist a measured unnecessary_action_count instead of null:\n{}",
        serde_json::to_string_pretty(&report).expect("report should render")
    );
    assert!(
        report["scorecard"]["retry_count"].is_number(),
        "fresh benchmark runs should persist retry_count as a structured number:\n{}",
        serde_json::to_string_pretty(&report).expect("report should render")
    );

    let measurement_notes = report["scorecard"]["measurement_notes"]
        .as_array()
        .expect("scorecard.measurement_notes should be an array");
    assert!(
        !measurement_notes
            .iter()
            .filter_map(Value::as_str)
            .any(|note| {
                note.contains("unnecessary_action_count") || note.contains("retry_count")
            }),
        "fresh benchmark runs should stop persisting legacy metric-gap notes once these fields are measured:\n{}",
        serde_json::to_string_pretty(&report).expect("report should render")
    );

    let human_review_notes = report["scorecard"]["human_review_notes"]
        .as_array()
        .expect("scorecard.human_review_notes should be an array");
    assert!(
        !human_review_notes
            .iter()
            .filter_map(Value::as_str)
            .any(
                |note| note.contains("Measure unnecessary action count explicitly")
                    || note.contains("Track bounded retries in benchmark runs")
            ),
        "fresh benchmark runs should stop echoing stale metric-gap proposals into the scorecard:\n{}",
        serde_json::to_string_pretty(&report).expect("report should render")
    );

    let proposal_titles = review["proposals"]
        .as_array()
        .expect("review.proposals should be an array")
        .iter()
        .filter_map(|proposal| proposal.get("title").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(
        !proposal_titles.contains(&"Measure unnecessary action count explicitly"),
        "fresh benchmark review artifacts should stop proposing work for unnecessary_action_count once the metric is measured:\n{}",
        serde_json::to_string_pretty(&review).expect("review should render")
    );
    assert!(
        !proposal_titles.contains(&"Track bounded retries in benchmark runs"),
        "fresh benchmark review artifacts should stop proposing work for retry_count once the metric is measured:\n{}",
        serde_json::to_string_pretty(&review).expect("review should render")
    );
}

#[test]
fn simard_gym_run_matches_legacy_binary_output_shape() {
    let artifact_root = TempDirGuard::new("simard-cli-gym-run-parity");
    let simard_output = command_in_dir(env!("CARGO_BIN_EXE_simard"), artifact_root.path())
        .arg("gym")
        .arg("run")
        .arg("repo-exploration-local")
        .output()
        .expect("simard gym run should launch");
    let simard_rendered = rendered_output(&simard_output);
    assert!(
        simard_output.status.success(),
        "simard gym run should succeed before parity comparison:\n{simard_rendered}"
    );

    let legacy_output = command_in_dir(env!("CARGO_BIN_EXE_simard-gym"), artifact_root.path())
        .arg("run")
        .arg("repo-exploration-local")
        .output()
        .expect("legacy simard-gym run should launch");
    let legacy_rendered = rendered_output(&legacy_output);
    assert!(
        legacy_output.status.success(),
        "legacy simard-gym run should succeed before parity comparison:\n{legacy_rendered}"
    );

    assert_eq!(
        normalize_benchmark_run_output(&simard_rendered),
        normalize_benchmark_run_output(&legacy_rendered),
        "the canonical simard gym run output should preserve the legacy operator-visible shape aside from run-specific session ids and artifact paths"
    );
}

#[test]
fn simard_gym_compare_reports_the_latest_two_runs_and_matches_legacy_binary() {
    let artifact_root = TempDirGuard::new("simard-cli-gym-compare");
    let first_run = command_in_dir(env!("CARGO_BIN_EXE_simard"), artifact_root.path())
        .arg("gym")
        .arg("run")
        .arg("repo-exploration-local")
        .output()
        .expect("first gym scenario run should launch");
    let first_rendered = rendered_output(&first_run);
    assert!(
        first_run.status.success(),
        "first benchmark run should succeed before comparison:\n{first_rendered}"
    );

    thread::sleep(Duration::from_millis(5));

    let second_run = command_in_dir(env!("CARGO_BIN_EXE_simard"), artifact_root.path())
        .arg("gym")
        .arg("run")
        .arg("repo-exploration-local")
        .output()
        .expect("second gym scenario run should launch");
    let second_rendered = rendered_output(&second_run);
    assert!(
        second_run.status.success(),
        "second benchmark run should succeed before comparison:\n{second_rendered}"
    );

    let simard_output = command_in_dir(env!("CARGO_BIN_EXE_simard"), artifact_root.path())
        .arg("gym")
        .arg("compare")
        .arg("repo-exploration-local")
        .output()
        .expect("simard gym compare should launch");
    let simard_rendered = rendered_output(&simard_output);

    assert!(
        simard_output.status.success(),
        "gym compare should surface the latest two scenario runs through the primary CLI:\n{simard_rendered}"
    );
    for expected in [
        "Scenario: repo-exploration-local",
        "Comparison status: unchanged",
        "Current report: target/simard-gym/repo-exploration-local/",
        "Current unnecessary actions:",
        "Current retry count:",
        "Previous report: target/simard-gym/repo-exploration-local/",
        "Previous unnecessary actions:",
        "Previous retry count:",
        "Delta unnecessary actions:",
        "Delta retry count:",
        "Comparison artifact report: target/simard-gym/comparisons/repo-exploration-local/",
    ] {
        assert!(
            simard_rendered.contains(expected),
            "gym compare should surface '{expected}' for operators:\n{simard_rendered}"
        );
    }

    let legacy_output = command_in_dir(env!("CARGO_BIN_EXE_simard-gym"), artifact_root.path())
        .arg("compare")
        .arg("repo-exploration-local")
        .output()
        .expect("legacy simard-gym compare should launch");
    let legacy_rendered = rendered_output(&legacy_output);

    assert!(
        legacy_output.status.success(),
        "legacy simard-gym compare should remain functional while the unified CLI becomes canonical:\n{legacy_rendered}"
    );
    assert_eq!(
        simard_rendered, legacy_rendered,
        "the unified compare surface should preserve the legacy compare output exactly until operators migrate"
    );
}

#[test]
fn simard_gym_compare_renders_unmeasured_for_legacy_artifacts_on_public_cli() {
    let artifact_root = TempDirGuard::new("simard-cli-gym-legacy-compare");
    write_legacy_benchmark_report(
        artifact_root.path(),
        "repo-exploration-local",
        "legacy-session",
    );

    let fresh_run = command_in_dir(env!("CARGO_BIN_EXE_simard"), artifact_root.path())
        .arg("gym")
        .arg("run")
        .arg("repo-exploration-local")
        .output()
        .expect("fresh gym run should launch");
    let fresh_rendered = rendered_output(&fresh_run);
    assert!(
        fresh_run.status.success(),
        "fresh benchmark run should succeed before comparing against a legacy artifact:\n{fresh_rendered}"
    );

    let simard_output = command_in_dir(env!("CARGO_BIN_EXE_simard"), artifact_root.path())
        .arg("gym")
        .arg("compare")
        .arg("repo-exploration-local")
        .output()
        .expect("simard gym compare should launch");
    let simard_rendered = rendered_output(&simard_output);
    assert!(
        simard_output.status.success(),
        "simard gym compare should succeed when one artifact predates the new metric fields:\n{simard_rendered}"
    );
    for expected in [
        "Current unnecessary actions: 0",
        "Current retry count: 0",
        "Previous unnecessary actions: unmeasured",
        "Previous retry count: unmeasured",
        "Delta unnecessary actions: unmeasured",
        "Delta retry count: unmeasured",
    ] {
        assert!(
            simard_rendered.contains(expected),
            "simard gym compare should surface '{expected}' instead of fabricating zeroes for legacy artifacts:\n{simard_rendered}"
        );
    }

    let legacy_output = command_in_dir(env!("CARGO_BIN_EXE_simard-gym"), artifact_root.path())
        .arg("compare")
        .arg("repo-exploration-local")
        .output()
        .expect("legacy simard-gym compare should launch");
    let legacy_rendered = rendered_output(&legacy_output);
    assert!(
        legacy_output.status.success(),
        "legacy simard-gym compare should succeed against the same legacy artifact set:\n{legacy_rendered}"
    );
    assert_eq!(
        simard_rendered, legacy_rendered,
        "the canonical compare surface should match the legacy compare output even when rendering unmeasured legacy metric fields"
    );
}

#[test]
fn simard_gym_rejects_unregistered_scenarios_before_accessing_artifacts() {
    let artifact_root = TempDirGuard::new("simard-cli-gym-invalid-scenario");

    for subcommand in ["run", "compare"] {
        let output = command_in_dir(env!("CARGO_BIN_EXE_simard"), artifact_root.path())
            .arg("gym")
            .arg(subcommand)
            .arg("../repo-exploration-local")
            .output()
            .expect("invalid benchmark scenario should launch");
        let rendered = rendered_output(&output);

        assert!(
            !output.status.success(),
            "gym {subcommand} must reject unregistered scenario ids visibly:\n{rendered}"
        );
        assert!(
            rendered.contains("BenchmarkScenarioNotFound")
                && rendered.contains("../repo-exploration-local"),
            "gym {subcommand} should reject invalid scenario ids with an explicit registry lookup failure:\n{rendered}"
        );
    }

    assert!(
        !artifact_root.path().join("target/simard-gym").exists(),
        "invalid scenario ids should fail before the CLI touches benchmark artifact storage"
    );
}

#[test]
fn simard_bootstrap_run_accepts_positional_operator_arguments_without_env_only_fallback() {
    let state_root = TempDirGuard::new("simard-cli-bootstrap");
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("bootstrap")
        .arg("run")
        .arg("simard-engineer")
        .arg("local-harness")
        .arg("single-process")
        .arg("bootstrap the Simard engineer loop")
        .arg(state_root.path())
        .output()
        .expect("simard bootstrap run should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "bootstrap should accept positional CLI arguments on the unified operator surface:\n{rendered}"
    );
    for expected in ["simard-engineer", "local-harness", "single-process"] {
        assert!(
            rendered.contains(expected),
            "bootstrap output should surface the requested runtime selection '{expected}':\n{rendered}"
        );
    }
    assert!(
        !rendered.contains("SIMARD_PROMPT_ROOT"),
        "bootstrap run should not bounce back to the env-only entrypoint when positional args were supplied:\n{rendered}"
    );
    assert!(
        state_root.path().join("latest_handoff.json").is_file(),
        "bootstrap run should persist a durable handoff under the supplied state root"
    );
}

#[test]
fn simard_rejects_unknown_commands_and_missing_required_arguments_explicitly() {
    let unknown_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("mystery-mode")
        .output()
        .expect("unknown simard command should launch");
    let unknown_rendered = rendered_output(&unknown_output);

    assert!(
        !unknown_output.status.success(),
        "unsupported top-level commands must fail visibly:\n{unknown_rendered}"
    );
    assert!(
        unknown_rendered.contains("unsupported command 'mystery-mode'"),
        "the unified CLI should reject unknown commands with an explicit allowlist failure:\n{unknown_rendered}"
    );
    assert!(
        !unknown_rendered.contains("SIMARD_PROMPT_ROOT"),
        "unknown-command failures should come from CLI parsing, not bootstrap env handling:\n{unknown_rendered}"
    );

    let arity_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("run")
        .arg("local-harness")
        .output()
        .expect("arity validation should launch");
    let arity_rendered = rendered_output(&arity_output);

    assert!(
        !arity_output.status.success(),
        "missing required arguments must fail visibly:\n{arity_rendered}"
    );
    assert!(
        arity_rendered.contains("expected topology"),
        "meeting mode should name the first missing required argument explicitly:\n{arity_rendered}"
    );
    assert!(
        !arity_rendered.contains("SIMARD_PROMPT_ROOT"),
        "arity failures should come from CLI validation, not the legacy bootstrap env path:\n{arity_rendered}"
    );
}
