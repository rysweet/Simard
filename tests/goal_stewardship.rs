use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rendered_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}{stderr}")
}

/// Returns true when the rendered output indicates the test cannot run
/// because the CI environment lacks a configured LLM provider.
/// Mirrors tests/engineer_loop.rs::skip_if_no_llm_provider — duplicated
/// because tests/ files compile as separate crates and there is no shared
/// test-support crate.
fn skip_if_no_llm_provider(rendered: &str) -> bool {
    if rendered.contains("No API key found")
        || rendered.contains("LLM-based review is unavailable")
        || rendered.contains("LLM session but open() failed")
        || rendered.contains("base type 'review-pipeline-rustyclawd' failed")
        || rendered.contains("missing required configuration 'SIMARD_LLM_PROVIDER'")
    {
        eprintln!("SKIP: no LLM provider available (CI environment)");
        return true;
    }
    false
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
fn goal_curator_probe_surfaces_active_top_five_priorities() {
    let state_root = TempDirGuard::new("simard-goal-curator-state");
    let objective = "\
goal: Maintain a truthful top 5 | priority=1 | status=active | rationale=core Simard stewardship\n\
goal: Keep meeting handoff durable | priority=2 | status=active | rationale=meeting updates must influence engineering\n\
goal: Preserve outside-in operator coverage | priority=3 | status=active | rationale=user requires real operator validation\n\
goal: Improve composite identities | priority=4 | status=active | rationale=composite roles should stay explicit\n\
goal: Build realistic tool driving | priority=5 | status=active | rationale=terminal-native behavior is central\n\
goal: Track future remote orchestration | priority=6 | status=active | rationale=important but not top-five current work";

    let output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
        .arg("goal-curation-run")
        .arg("local-harness")
        .arg("single-process")
        .arg(objective)
        .arg(state_root.path())
        .output()
        .expect("goal-curation probe should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "goal-curation probe should succeed:\n{rendered}"
    );
    assert!(rendered.contains("Probe mode: goal-curation-run"));
    assert!(rendered.contains("Identity: simard-goal-curator"));
    assert!(rendered.contains("Active goals count: 5"));
    assert!(rendered.contains("Active goal 1: p1 [active] Maintain a truthful top 5"));
    assert!(rendered.contains("Active goal 5: p5 [active] Build realistic tool driving"));
    assert!(
        !rendered.contains("Track future remote orchestration"),
        "top-five reflection should omit lower-priority active goals:\n{rendered}"
    );
}

#[test]
fn meeting_goal_updates_flow_into_later_engineer_loop_runs() {
    let state_root = TempDirGuard::new("simard-goal-flow-state");
    let meeting_objective = "\
agenda: align the next Simard workstream\n\
decision: preserve meeting-to-engineer continuity\n\
risk: workflow routing is still unreliable\n\
next-step: keep durable priorities visible\n\
open-question: how aggressively should Simard reprioritize?\n\
goal: Preserve meeting handoff | priority=1 | status=active | rationale=meeting decisions must shape later work\n\
goal: Keep outside-in verification strong | priority=2 | status=active | rationale=operator confidence depends on real product exercise";

    let handoff_dir = state_root.path().join("handoffs");
    let meeting_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
        .arg("meeting-run")
        .arg("local-harness")
        .arg("single-process")
        .arg(meeting_objective)
        .arg(state_root.path())
        .env("SIMARD_HANDOFF_DIR", &handoff_dir)
        .output()
        .expect("meeting probe should launch");
    let meeting_rendered = rendered_output(&meeting_output);
    assert!(
        meeting_output.status.success(),
        "meeting probe should succeed:\n{meeting_rendered}"
    );
    assert!(meeting_rendered.contains("Active goals count: 2"));

    let engineer_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
        .arg("engineer-loop-run")
        .arg("single-process")
        .arg(repo_root())
        .arg("inspect the repo and preserve explicit goal context")
        .arg(state_root.path())
        .env("SIMARD_HANDOFF_DIR", &handoff_dir)
        .output()
        .expect("engineer loop probe should launch");
    let engineer_rendered = rendered_output(&engineer_output);

    if skip_if_no_llm_provider(&engineer_rendered) {
        return;
    }
    assert!(
        engineer_output.status.success(),
        "engineer loop probe should succeed with shared goal state:\n{engineer_rendered}"
    );
    assert!(engineer_rendered.contains("Active goals count: 2"));
    assert!(engineer_rendered.contains("Active goal 1: p1 [active] Preserve meeting handoff"));
    assert!(
        engineer_rendered
            .contains("Active goal 2: p2 [active] Keep outside-in verification strong")
    );
    assert!(engineer_rendered.contains("Carried meeting decisions: 1"));
    assert!(
        engineer_rendered
            .contains("Carried meeting decision 1: agenda=align the next Simard workstream;")
    );
    assert!(engineer_rendered.contains("decisions=[preserve meeting-to-engineer continuity]"));
    assert!(engineer_rendered.contains("next_steps=[keep durable priorities visible]"));
    assert!(
        engineer_rendered.contains("Verification status: verified"),
        "engineer loop must still verify bounded work while reading curated goals:\n{engineer_rendered}"
    );
}

#[test]
fn engineer_loop_only_carries_the_three_most_recent_meeting_records() {
    let state_root = TempDirGuard::new("simard-meeting-carry-limit");
    let handoff_dir = state_root.path().join("handoffs");

    for meeting_number in 1..=4 {
        let meeting_objective = format!(
            "\
agenda: alignment meeting {meeting_number}\n\
decision: preserve priority {meeting_number}\n\
risk: workflow risk {meeting_number}\n\
next-step: verify carryover {meeting_number}\n\
open-question: what changes after meeting {meeting_number}?"
        );

        let output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
            .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
            .arg("meeting-run")
            .arg("local-harness")
            .arg("single-process")
            .arg(&meeting_objective)
            .arg(state_root.path())
            .env("SIMARD_HANDOFF_DIR", &handoff_dir)
            .output()
            .expect("meeting probe should launch");
        let rendered = rendered_output(&output);

        assert!(
            output.status.success(),
            "meeting probe should succeed for carry-limit setup:\n{rendered}"
        );
        assert!(rendered.contains("Decision records: 1"));
    }

    let engineer_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
        .arg("engineer-loop-run")
        .arg("single-process")
        .arg(repo_root())
        .arg("inspect the repo and preserve bounded meeting memory")
        .arg(state_root.path())
        .env("SIMARD_HANDOFF_DIR", &handoff_dir)
        .output()
        .expect("engineer loop probe should launch");
    let engineer_rendered = rendered_output(&engineer_output);

    if skip_if_no_llm_provider(&engineer_rendered) {
        return;
    }
    assert!(
        engineer_output.status.success(),
        "engineer loop probe should succeed with bounded meeting carryover:\n{engineer_rendered}"
    );
    assert!(engineer_rendered.contains("Active goals count: 0"));
    assert!(engineer_rendered.contains("Carried meeting decisions: 3"));
    assert!(
        !engineer_rendered.contains("agenda=alignment meeting 1;"),
        "engineer loop should drop the oldest carried meeting record once the cap is exceeded:\n{engineer_rendered}"
    );
    assert!(engineer_rendered.contains("Carried meeting decision 1: agenda=alignment meeting 2;"));
    assert!(engineer_rendered.contains("Carried meeting decision 2: agenda=alignment meeting 3;"));
    assert!(engineer_rendered.contains("Carried meeting decision 3: agenda=alignment meeting 4;"));
    assert!(engineer_rendered.contains("Verification status: verified"));
}
