//! Outside-in contract tests for the next product block after the bounded
//! engineer loop landed in PR #55.
//!
//! `Specs/ProductArchitecture.md` defines five operator-visible modes
//! (engineer, meeting, goal-curation, improvement-curation, gym). The current
//! implementation still fragments those modes across `simard`,
//! `simard_operator_probe`, and `simard-gym`. These tests lock the next block
//! as one primary `simard` CLI while preserving the legacy specialist binaries
//! as compatibility surfaces.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

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
        "meeting",
        "goal-curation",
        "improvement-curation",
        "gym",
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
