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
use std::sync::{Mutex, OnceLock};
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

fn goal_curation_default_root_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn meeting_default_root_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn engineer_default_root_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn default_engineer_state_root(topology: &str) -> PathBuf {
    repo_root()
        .join("target/operator-probe-state")
        .join("engineer-loop-run")
        .join("simard-engineer")
        .join("terminal-shell")
        .join(topology)
}

fn default_meeting_state_root(base_type: &str, topology: &str) -> PathBuf {
    repo_root()
        .join("target/operator-probe-state")
        .join("meeting-run")
        .join("simard-meeting")
        .join(base_type)
        .join(topology)
}

fn default_goal_curation_state_root(base_type: &str, topology: &str) -> PathBuf {
    repo_root()
        .join("target/operator-probe-state")
        .join("goal-curation-run")
        .join("simard-goal-curator")
        .join(base_type)
        .join(topology)
}

fn review_default_root_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn default_review_state_root(base_type: &str, topology: &str) -> PathBuf {
    repo_root()
        .join("target/operator-probe-state")
        .join("review-run")
        .join("simard-engineer")
        .join(base_type)
        .join(topology)
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

struct CleanupDirGuard {
    path: PathBuf,
}

impl CleanupDirGuard {
    fn new(path: PathBuf) -> Self {
        let _ = fs::remove_dir_all(&path);
        Self { path }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

impl Drop for CleanupDirGuard {
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
fn simard_help_documents_meeting_read_as_the_durable_meeting_audit_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("--help")
        .output()
        .expect("simard help should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "simard help should stay readable while the durable meeting read command is added:\n{rendered}"
    );
    assert!(
        rendered.contains("meeting read <base-type> <topology> [state-root]"),
        "simard help should document the canonical read-only meeting workflow:\n{rendered}"
    );
}

#[test]
fn simard_help_documents_goal_curation_read_as_the_durable_register_inspection_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("--help")
        .output()
        .expect("simard help should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "simard help should stay readable while the durable goal register command is added:\n{rendered}"
    );
    assert!(
        rendered.contains("goal-curation read <base-type> <topology> [state-root]"),
        "simard help should document the canonical read-only goal register workflow:\n{rendered}"
    );
}

#[test]
fn simard_help_documents_improvement_curation_read_as_the_durable_review_decision_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("--help")
        .output()
        .expect("simard help should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "simard help should stay readable while the durable improvement readback command is added:\n{rendered}"
    );
    assert!(
        rendered.contains("improvement-curation read <base-type> <topology> [state-root]"),
        "simard help should document the canonical read-only improvement curation workflow:\n{rendered}"
    );
}

#[test]
fn simard_help_documents_engineer_read_as_the_durable_engineer_audit_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("--help")
        .output()
        .expect("simard help should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "simard help should stay readable while the engineer read audit command is added:\n{rendered}"
    );
    assert!(
        rendered.contains("engineer read <topology> [state-root]"),
        "simard help should document the canonical read-only engineer audit workflow:\n{rendered}"
    );
    assert!(
        rendered.contains("engineer terminal-read <topology> [state-root]"),
        "simard help should document the canonical read-only terminal audit workflow:\n{rendered}"
    );
    assert!(
        rendered.contains("engineer terminal-file <topology> <objective-file> [state-root]"),
        "simard help should document the file-backed terminal session workflow:\n{rendered}"
    );
    assert!(
        rendered.contains("engineer terminal-recipe-list"),
        "simard help should document terminal recipe discovery:\n{rendered}"
    );
    assert!(
        rendered.contains("engineer terminal-recipe-show <recipe-name>"),
        "simard help should document terminal recipe inspection:\n{rendered}"
    );
    assert!(
        rendered.contains("engineer terminal-recipe <topology> <recipe-name> [state-root]"),
        "simard help should document named terminal recipe execution:\n{rendered}"
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
fn simard_engineer_read_reuses_the_run_default_state_root_and_stays_read_only() {
    let _lock = engineer_default_root_lock()
        .lock()
        .expect("engineer default root test lock should not be poisoned");
    let state_root = default_engineer_state_root("single-process");
    let _cleanup = CleanupDirGuard::new(state_root.clone());
    let repo_root = repo_root();
    let objective = "\
inspect the repository state, execute one safe local engineering action, verify the outcome explicitly, and persist truthful local evidence and memory
raw-secret-token=shh \u{1b}[31mdo not replay this\u{1b}[0m";

    let run_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("run")
        .arg("single-process")
        .arg(&repo_root)
        .arg(objective)
        .output()
        .expect("simard engineer run should launch with its default state root");
    let run_rendered = rendered_output(&run_output);

    assert!(
        run_output.status.success(),
        "engineer run should succeed with its canonical default state root:\n{run_rendered}"
    );
    assert!(
        run_rendered.contains(&format!("State root: {}", state_root.display())),
        "engineer run should surface the canonical default durable root that engineer read later inspects:\n{run_rendered}"
    );

    let handoff = load_json(state_root.join("latest_handoff.json"));
    let objective_metadata = handoff["session"]["objective"]
        .as_str()
        .expect("handoff should persist redacted objective metadata")
        .to_string();
    let memory_count = handoff["memory_records"]
        .as_array()
        .expect("handoff should persist exported memory records")
        .len();
    let evidence_count = handoff["evidence_records"]
        .as_array()
        .expect("handoff should persist exported evidence records")
        .len();
    let memory_before =
        fs::read(state_root.join("memory_records.json")).expect("memory store should exist");
    let evidence_before =
        fs::read(state_root.join("evidence_records.json")).expect("evidence store should exist");
    let handoff_before =
        fs::read(state_root.join("latest_handoff.json")).expect("handoff store should exist");

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("read")
        .arg("single-process")
        .output()
        .expect("simard engineer read should launch with its default state root");
    let read_rendered = rendered_output(&read_output);

    assert!(
        read_output.status.success(),
        "engineer read should inspect the same canonical default durable root that engineer run populates:\n{read_rendered}"
    );
    for expected in [
        "Probe mode: engineer-read",
        "Identity: simard-engineer",
        "Selected base type: terminal-shell",
        "Topology: single-process",
        &format!("State root: {}", state_root.display()),
        "Session phase: complete",
        &format!("Objective metadata: {objective_metadata}"),
        &format!("Repo root: {}", repo_root.display()),
        &format!("Memory records: {memory_count}"),
        &format!("Evidence records: {evidence_count}"),
    ] {
        assert!(
            read_rendered.contains(expected),
            "engineer read should surface '{expected}' for operators:\n{read_rendered}"
        );
    }
    for prefix in [
        "Repo branch: ",
        "Repo head: ",
        "Worktree dirty: ",
        "Changed files: ",
        "Active goals count: ",
        "Carried meeting decisions: ",
        "Selected action: ",
        "Action plan: ",
        "Verification steps: ",
        "Action status: ",
        "Changed files after action: ",
        "Verification status: ",
        "Verification summary: ",
    ] {
        let value = output_line_value(&run_rendered, prefix)
            .unwrap_or_else(|| panic!("engineer run should surface '{prefix}' before readback"));
        assert!(
            read_rendered.contains(&format!("{prefix}{value}")),
            "engineer read should replay the persisted '{prefix}' summary from durable state:\n{read_rendered}"
        );
    }
    for forbidden in ['\u{1b}', '\u{7}'] {
        assert!(
            !read_rendered.contains(forbidden),
            "engineer read should sanitize persisted operator-visible text before printing it:\n{read_rendered}"
        );
    }
    for raw in ["raw-secret-token=shh", "do not replay this"] {
        assert!(
            !read_rendered.contains(raw),
            "engineer read must not reconstruct raw redacted objectives in operator output:\n{read_rendered}"
        );
    }

    let memory_after =
        fs::read(state_root.join("memory_records.json")).expect("memory store should exist");
    let evidence_after =
        fs::read(state_root.join("evidence_records.json")).expect("evidence store should exist");
    let handoff_after =
        fs::read(state_root.join("latest_handoff.json")).expect("handoff store should exist");
    assert_eq!(
        memory_before, memory_after,
        "engineer read must not rewrite the durable memory store"
    );
    assert_eq!(
        evidence_before, evidence_after,
        "engineer read must not rewrite the durable evidence store"
    );
    assert_eq!(
        handoff_before, handoff_after,
        "engineer read must not rewrite the durable handoff snapshot"
    );
}

#[test]
fn simard_engineer_read_surfaces_carried_context_from_explicit_state_root_and_matches_probe_parity()
{
    let state_root = TempDirGuard::new("simard-cli-engineer-read-explicit");
    let repo_root = repo_root();
    let meeting_objective = "\
agenda: align the next Simard engineer read workstream\n\
decision: preserve meeting-to-engineer \u{1b}[31mcontinuity\u{1b}[0m\n\
risk: readback might replay unsanitized durable state\n\
next-step: add an operator-facing engineer audit surface\n\
goal: Preserve \u{1b}]8;;https://example.invalid\u{7}meeting handoff\u{1b}]8;;\u{7} | priority=1 | status=active | rationale=meeting decisions must stay visible to later engineer reads";

    let meeting_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(meeting_objective)
        .arg(state_root.path())
        .output()
        .expect("simard meeting run should launch against the shared engineer-read state root");
    let meeting_rendered = rendered_output(&meeting_output);

    assert!(
        meeting_output.status.success(),
        "meeting run should seed durable carried context for engineer read:\n{meeting_rendered}"
    );

    let engineer_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("run")
        .arg("single-process")
        .arg(&repo_root)
        .arg(engineer_loop_objective())
        .arg(state_root.path())
        .output()
        .expect("simard engineer run should launch against the shared engineer-read state root");
    let engineer_rendered = rendered_output(&engineer_output);

    assert!(
        engineer_output.status.success(),
        "engineer run should preserve carried meeting context in the shared durable root:\n{engineer_rendered}"
    );

    let simard_read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("read")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard engineer read should launch against an explicit state root");
    let simard_read_rendered = rendered_output(&simard_read_output);

    let probe_read_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("engineer-read")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard_operator_probe engineer-read should launch");
    let probe_read_rendered = rendered_output(&probe_read_output);

    assert!(
        simard_read_output.status.success(),
        "engineer read should expose carried meeting context through the canonical CLI:\n{simard_read_rendered}"
    );
    assert!(
        probe_read_output.status.success(),
        "the compatibility engineer-read probe should remain available while operators migrate:\n{probe_read_rendered}"
    );
    assert_eq!(
        simard_read_rendered, probe_read_rendered,
        "engineer read should preserve probe parity exactly so the canonical and compatibility surfaces do not drift"
    );
    for expected in [
        &format!("State root: {}", state_root.path().display()),
        "Active goals count: 1",
        "Active goal 1: p1 [active] Preserve meeting handoff",
        "Carried meeting decisions: 1",
        "Carried meeting decision 1: preserve meeting-to-engineer continuity",
    ] {
        assert!(
            simard_read_rendered.contains(expected),
            "engineer read should surface '{expected}' from the shared durable state root:\n{simard_read_rendered}"
        );
    }
    for prefix in [
        "Selected action: ",
        "Action plan: ",
        "Verification steps: ",
        "Action status: ",
        "Verification status: ",
        "Verification summary: ",
    ] {
        let value = output_line_value(&engineer_rendered, prefix)
            .unwrap_or_else(|| panic!("engineer run should surface '{prefix}' before readback"));
        assert!(
            simard_read_rendered.contains(&format!("{prefix}{value}")),
            "engineer read should keep the persisted '{prefix}' summary visible with explicit state roots:\n{simard_read_rendered}"
        );
    }
    for forbidden in ['\u{1b}', '\u{7}'] {
        assert!(
            !simard_read_rendered.contains(forbidden),
            "engineer read should sanitize carried context before printing it:\n{simard_read_rendered}"
        );
    }
}

#[test]
fn simard_engineer_terminal_exposes_the_terminal_backed_engineer_surface() {
    let state_root = TempDirGuard::new("simard-cli-terminal");
    let objective = "\
working-directory: .\n\
command: printf \"terminal-cli-ready\\n\"\n\
wait-for: terminal-cli-ready\n\
command: printf \"terminal-cli-ok\\n\"";
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
        "Adapter capabilities: prompt-assets, session-lifecycle, memory, evidence, reflection, terminal-session",
        "Terminal command count: 2",
        "Terminal wait count: 1",
        "Terminal steps count: 3",
        "Terminal step 1: input: printf \"terminal-cli-ready\\n\"",
        "Terminal step 2: wait-for: terminal-cli-ready",
        "Terminal step 3: input: printf \"terminal-cli-ok\\n\"",
        "Terminal checkpoints count: 1",
        "Terminal checkpoint 1: terminal-cli-ready",
        "Terminal last output line: terminal-cli-ok",
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
    let simard_normalized = replace_output_line_value(
        &simard_rendered,
        "Terminal transcript preview: ",
        "<preview>",
    );
    let legacy_normalized = replace_output_line_value(
        &legacy_rendered,
        "Terminal transcript preview: ",
        "<preview>",
    );
    assert_eq!(
        simard_normalized, legacy_normalized,
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
fn simard_engineer_terminal_file_runs_a_bounded_terminal_session_from_a_recipe_file() {
    let state_root = TempDirGuard::new("simard-cli-terminal-file");
    let objective_dir = TempDirGuard::new("simard-cli-terminal-file-objective");
    let objective_path = objective_dir.path().join("session.simard-terminal");
    fs::write(
        &objective_path,
        "working-directory: .\ncommand: printf \"terminal-file-ready\\n\"\nwait-for: terminal-file-ready\ninput: printf \"terminal-file-ok\\n\"",
    )
    .expect("terminal objective file should be written");

    let simard_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("terminal-file")
        .arg("single-process")
        .arg(&objective_path)
        .arg(state_root.path())
        .output()
        .expect("simard engineer terminal-file should launch");
    let simard_rendered = rendered_output(&simard_output);

    assert!(
        simard_output.status.success(),
        "terminal-file should expose the same bounded terminal substrate through a reusable file-backed recipe:\n{simard_rendered}"
    );
    for expected in [
        "Selected base type: terminal-shell",
        "Terminal steps count: 3",
        "Terminal checkpoint 1: terminal-file-ready",
        "Terminal last output line: terminal-file-ok",
        "terminal-file-ok",
    ] {
        assert!(
            simard_rendered.contains(expected),
            "terminal-file should surface '{expected}' for operators:\n{simard_rendered}"
        );
    }

    let legacy_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("terminal-run-file")
        .arg("single-process")
        .arg(&objective_path)
        .arg(state_root.path())
        .output()
        .expect("legacy terminal-run-file should launch");
    let legacy_rendered = rendered_output(&legacy_output);

    assert!(
        legacy_output.status.success(),
        "terminal-run-file compatibility path should remain available while operators migrate:\n{legacy_rendered}"
    );
    let simard_normalized = replace_output_line_value(
        &simard_rendered,
        "Terminal transcript preview: ",
        "<preview>",
    );
    let legacy_normalized = replace_output_line_value(
        &legacy_rendered,
        "Terminal transcript preview: ",
        "<preview>",
    );
    assert_eq!(
        simard_normalized, legacy_normalized,
        "terminal-file should preserve terminal-run-file parity so canonical and compatibility surfaces stay aligned"
    );
}

#[test]
fn simard_engineer_terminal_file_rejects_missing_or_unreadable_recipe_files() {
    let state_root = TempDirGuard::new("simard-cli-terminal-file-missing");
    let missing_path = state_root.path().join("missing.simard-terminal");
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("terminal-file")
        .arg("single-process")
        .arg(&missing_path)
        .arg(state_root.path())
        .output()
        .expect("simard engineer terminal-file missing-path case should launch");
    let rendered = rendered_output(&output);

    assert!(
        !output.status.success(),
        "terminal-file must fail when the recipe file cannot be inspected:\n{rendered}"
    );
    assert!(
        rendered.contains("terminal objective file") && rendered.contains("could not be inspected"),
        "terminal-file should explain why the requested recipe file could not be loaded:\n{rendered}"
    );
}

#[test]
fn simard_engineer_terminal_recipe_list_and_show_surface_builtin_named_recipes() {
    let list_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("terminal-recipe-list")
        .output()
        .expect("simard engineer terminal-recipe-list should launch");
    let list_rendered = rendered_output(&list_output);

    assert!(
        list_output.status.success(),
        "terminal-recipe-list should expose built-in named session recipes:\n{list_rendered}"
    );
    for expected in [
        "Terminal recipes: 2",
        "foundation-check",
        "copilot-status-check",
        "simard/terminal_recipes/foundation-check.simard-terminal",
    ] {
        assert!(
            list_rendered.contains(expected),
            "terminal-recipe-list should surface '{expected}':\n{list_rendered}"
        );
    }
    let legacy_list_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("terminal-recipe-list")
        .output()
        .expect("simard_operator_probe terminal-recipe-list should launch");
    let legacy_list_rendered = rendered_output(&legacy_list_output);
    assert!(
        legacy_list_output.status.success(),
        "terminal-recipe-list compatibility path should remain available:\n{legacy_list_rendered}"
    );
    assert_eq!(
        list_rendered, legacy_list_rendered,
        "terminal-recipe-list should preserve parity with the compatibility probe"
    );

    let show_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("terminal-recipe-show")
        .arg("foundation-check")
        .output()
        .expect("simard engineer terminal-recipe-show should launch");
    let show_rendered = rendered_output(&show_output);

    assert!(
        show_output.status.success(),
        "terminal-recipe-show should print the selected recipe asset and contents:\n{show_rendered}"
    );
    for expected in [
        "Terminal recipe: foundation-check",
        "Recipe asset: simard/terminal_recipes/foundation-check.simard-terminal",
        "command: printf \"terminal-recipe-ready\\n\"",
        "wait-for: terminal-recipe-ready",
    ] {
        assert!(
            show_rendered.contains(expected),
            "terminal-recipe-show should surface '{expected}':\n{show_rendered}"
        );
    }
    let legacy_show_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("terminal-recipe-show")
        .arg("foundation-check")
        .output()
        .expect("simard_operator_probe terminal-recipe-show should launch");
    let legacy_show_rendered = rendered_output(&legacy_show_output);
    assert!(
        legacy_show_output.status.success(),
        "terminal-recipe-show compatibility path should remain available:\n{legacy_show_rendered}"
    );
    assert_eq!(
        show_rendered, legacy_show_rendered,
        "terminal-recipe-show should preserve parity with the compatibility probe"
    );
}

#[test]
fn simard_engineer_terminal_recipe_runs_builtin_named_recipe_with_probe_parity() {
    let state_root = TempDirGuard::new("simard-cli-terminal-recipe");
    let simard_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("terminal-recipe")
        .arg("single-process")
        .arg("foundation-check")
        .arg(state_root.path())
        .output()
        .expect("simard engineer terminal-recipe should launch");
    let simard_rendered = rendered_output(&simard_output);

    assert!(
        simard_output.status.success(),
        "terminal-recipe should execute the built-in named recipe through the canonical CLI:\n{simard_rendered}"
    );
    for expected in [
        "Selected base type: terminal-shell",
        "Terminal checkpoint 1: terminal-recipe-ready",
        "Terminal last output line: terminal-recipe-ok",
        "terminal-recipe-ok",
    ] {
        assert!(
            simard_rendered.contains(expected),
            "terminal-recipe should surface '{expected}':\n{simard_rendered}"
        );
    }

    let legacy_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("terminal-recipe-run")
        .arg("single-process")
        .arg("foundation-check")
        .arg(state_root.path())
        .output()
        .expect("legacy terminal-recipe-run should launch");
    let legacy_rendered = rendered_output(&legacy_output);

    assert!(
        legacy_output.status.success(),
        "terminal-recipe-run compatibility path should remain available:\n{legacy_rendered}"
    );
    let simard_normalized = replace_output_line_value(
        &simard_rendered,
        "Terminal transcript preview: ",
        "<preview>",
    );
    let legacy_normalized = replace_output_line_value(
        &legacy_rendered,
        "Terminal transcript preview: ",
        "<preview>",
    );
    assert_eq!(
        simard_normalized, legacy_normalized,
        "terminal-recipe should preserve compatibility parity with terminal-recipe-run"
    );
}

#[test]
fn simard_engineer_terminal_read_replays_persisted_terminal_state_and_matches_probe_parity() {
    let state_root = TempDirGuard::new("simard-cli-terminal-read");
    let objective = "\
working-directory: .\n\
command: printf \"terminal-read-ready\\n\"\n\
wait-for: terminal-read-ready\n\
input: printf \"terminal-read-ok\\n\"";
    let run_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("terminal")
        .arg("single-process")
        .arg(objective)
        .arg(state_root.path())
        .output()
        .expect("simard engineer terminal should seed durable terminal state");
    let run_rendered = rendered_output(&run_output);

    assert!(
        run_output.status.success(),
        "terminal engineer mode should succeed before terminal readback:\n{run_rendered}"
    );

    let simard_read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("terminal-read")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard engineer terminal-read should launch");
    let simard_read_rendered = rendered_output(&simard_read_output);

    let probe_read_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("terminal-read")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard_operator_probe terminal-read should launch");
    let probe_read_rendered = rendered_output(&probe_read_output);

    assert!(
        simard_read_output.status.success(),
        "terminal-read should expose persisted terminal session state through the canonical CLI:\n{simard_read_rendered}"
    );
    assert!(
        probe_read_output.status.success(),
        "the compatibility terminal-read probe should remain available while operators migrate:\n{probe_read_rendered}"
    );
    assert_eq!(
        simard_read_rendered, probe_read_rendered,
        "terminal-read should preserve probe parity so the canonical and compatibility surfaces do not drift"
    );
    for expected in [
        "Probe mode: terminal-read",
        "Identity: simard-engineer",
        "Selected base type: terminal-shell",
        "Topology: single-process",
        &format!("State root: {}", state_root.path().display()),
        "Session phase: complete",
        "Adapter implementation: terminal-shell::local-pty",
        "Terminal command count: 2",
        "Terminal wait count: 1",
        "Terminal steps count: 3",
        "Terminal step 1: input: printf \"terminal-read-ready\\n\"",
        "Terminal step 2: wait-for: terminal-read-ready",
        "Terminal step 3: input: printf \"terminal-read-ok\\n\"",
        "Terminal checkpoints count: 1",
        "Terminal checkpoint 1: terminal-read-ready",
        "Terminal last output line: terminal-read-ok",
        "terminal-read-ok",
    ] {
        assert!(
            simard_read_rendered.contains(expected),
            "terminal-read should surface '{expected}' for operators:\n{simard_read_rendered}"
        );
    }
    for forbidden in ['\u{1b}', '\u{7}'] {
        assert!(
            !simard_read_rendered.contains(forbidden),
            "terminal-read should sanitize persisted terminal output before printing it:\n{simard_read_rendered}"
        );
    }
}

#[test]
fn simard_engineer_terminal_fails_closed_when_wait_for_output_never_arrives() {
    let state_root = TempDirGuard::new("simard-cli-terminal-wait-timeout");
    let objective = "\
working-directory: .\n\
command: printf \"terminal-cli-ok\\n\"\n\
wait-for: terminal-cli-missing";
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("terminal")
        .arg("single-process")
        .arg(objective)
        .arg(state_root.path())
        .output()
        .expect("simard engineer terminal timeout case should launch");
    let rendered = rendered_output(&output);

    assert!(
        !output.status.success(),
        "terminal engineer mode must fail when a wait-for checkpoint is never satisfied:\n{rendered}"
    );
    assert!(
        rendered.contains(
            "terminal-shell did not emit expected output 'terminal-cli-missing' within 5s"
        ),
        "terminal engineer mode should explain which expected output never arrived:\n{rendered}"
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
fn simard_engineer_read_rejects_missing_and_incomplete_state_roots_before_snapshot_loading() {
    let temp_dir = TempDirGuard::new("simard-cli-engineer-read-invalid-root");
    let missing_root = temp_dir.path().join("missing-layout");
    let empty_root = temp_dir.path().join("empty-layout");
    let handoff_only_root = temp_dir.path().join("handoff-only-layout");
    let handoff_and_memory_only_root = temp_dir.path().join("handoff-and-memory-only-layout");
    fs::create_dir_all(&empty_root).expect("empty state root fixture should be created");
    fs::create_dir_all(&handoff_only_root).expect("handoff-only fixture should be created");
    fs::create_dir_all(&handoff_and_memory_only_root)
        .expect("handoff-and-memory-only fixture should be created");
    fs::write(handoff_only_root.join("latest_handoff.json"), "{}")
        .expect("handoff-only fixture should include a placeholder handoff");
    fs::write(
        handoff_and_memory_only_root.join("latest_handoff.json"),
        "{}",
    )
    .expect("handoff-and-memory-only fixture should include a placeholder handoff");
    fs::write(
        handoff_and_memory_only_root.join("memory_records.json"),
        "[]",
    )
    .expect("handoff-and-memory-only fixture should include a placeholder memory store");

    let missing_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("read")
        .arg("single-process")
        .arg(&missing_root)
        .output()
        .expect("simard engineer read missing-root check should launch");
    let missing_rendered = rendered_output(&missing_output);

    assert!(
        !missing_output.status.success(),
        "engineer read must fail visibly for a nonexistent explicit state root:\n{missing_rendered}"
    );
    assert!(
        missing_rendered.contains("invalid state root")
            || missing_rendered.contains("InvalidStateRoot"),
        "engineer read should keep the failing state-root contract explicit:\n{missing_rendered}"
    );
    assert!(
        missing_rendered.contains("requires an existing state root directory"),
        "engineer read should explain why a nonexistent explicit root was rejected:\n{missing_rendered}"
    );
    assert!(
        !missing_rendered.contains("unsupported command"),
        "engineer read should fail through the read-state contract, not command dispatch:\n{missing_rendered}"
    );

    let empty_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("read")
        .arg("single-process")
        .arg(&empty_root)
        .output()
        .expect("simard engineer read empty-root check should launch");
    let empty_rendered = rendered_output(&empty_output);

    assert!(
        !empty_output.status.success(),
        "engineer read must fail visibly for an empty explicit state root:\n{empty_rendered}"
    );
    assert!(
        empty_rendered.contains("latest_handoff.json"),
        "engineer read should reject empty roots before parsing anything by requiring latest_handoff.json:\n{empty_rendered}"
    );
    assert!(
        !empty_rendered.contains("missing field"),
        "engineer read should validate read-layout files before attempting to deserialize them:\n{empty_rendered}"
    );

    let handoff_only_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("read")
        .arg("single-process")
        .arg(&handoff_only_root)
        .output()
        .expect("simard engineer read handoff-only check should launch");
    let handoff_only_rendered = rendered_output(&handoff_only_output);

    assert!(
        !handoff_only_output.status.success(),
        "engineer read must fail visibly for a state root that lacks the durable memory store:\n{handoff_only_rendered}"
    );
    assert!(
        handoff_only_rendered.contains("memory_records.json"),
        "engineer read should require memory_records.json before attempting snapshot parsing:\n{handoff_only_rendered}"
    );
    assert!(
        !handoff_only_rendered.contains("missing field"),
        "engineer read should not deserialize placeholder handoff JSON before validating required files:\n{handoff_only_rendered}"
    );

    let handoff_and_memory_only_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("read")
        .arg("single-process")
        .arg(&handoff_and_memory_only_root)
        .output()
        .expect("simard engineer read handoff-and-memory-only check should launch");
    let handoff_and_memory_only_rendered = rendered_output(&handoff_and_memory_only_output);

    assert!(
        !handoff_and_memory_only_output.status.success(),
        "engineer read must fail visibly for a state root that lacks the durable evidence store:\n{handoff_and_memory_only_rendered}"
    );
    assert!(
        handoff_and_memory_only_rendered.contains("evidence_records.json"),
        "engineer read should require evidence_records.json before attempting snapshot parsing:\n{handoff_and_memory_only_rendered}"
    );
    assert!(
        !handoff_and_memory_only_rendered.contains("missing field"),
        "engineer read should fail on incomplete layout before deserializing placeholder handoff JSON:\n{handoff_and_memory_only_rendered}"
    );
}

#[test]
fn simard_engineer_read_rejects_tampered_persisted_objective_metadata() {
    let state_root = TempDirGuard::new("simard-cli-engineer-read-tampered-objective");
    let repo_root = repo_root();
    let run_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("run")
        .arg("single-process")
        .arg(&repo_root)
        .arg(engineer_loop_objective())
        .arg(state_root.path())
        .output()
        .expect("simard engineer run should seed durable state");
    let run_rendered = rendered_output(&run_output);

    assert!(
        run_output.status.success(),
        "engineer run should succeed before objective-metadata tampering:\n{run_rendered}"
    );

    let handoff_path = state_root.path().join("latest_handoff.json");
    let mut handoff = load_json(&handoff_path);
    handoff["session"]["objective"] =
        json!("objective-metadata(chars=9, words=1, lines=1, token=LEAKME)");
    fs::write(
        &handoff_path,
        serde_json::to_vec_pretty(&handoff).expect("tampered handoff should serialize"),
    )
    .expect("tampered handoff should be written");

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("read")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard engineer read should launch against the tampered handoff");
    let read_rendered = rendered_output(&read_output);

    assert!(
        !read_output.status.success(),
        "engineer read must fail visibly for untrusted persisted objective metadata:\n{read_rendered}"
    );
    assert!(
        read_rendered.contains("session.objective") || read_rendered.contains("objective metadata"),
        "engineer read should explain why the persisted objective metadata was rejected:\n{read_rendered}"
    );
    assert!(
        !read_rendered.contains("LEAKME"),
        "engineer read must not echo the tampered objective metadata payload:\n{read_rendered}"
    );
}

#[cfg(unix)]
#[test]
fn simard_engineer_read_rejects_symlinked_required_artifacts_without_leaking_paths() {
    use std::os::unix::fs::symlink;

    let state_root = TempDirGuard::new("simard-cli-engineer-read-symlink-artifact");
    let real_memory_path = state_root.path().join("real-memory-records.json");
    let linked_memory_path = state_root.path().join("memory_records.json");

    fs::write(state_root.path().join("latest_handoff.json"), "{}")
        .expect("handoff placeholder should be created");
    fs::write(state_root.path().join("evidence_records.json"), "[]")
        .expect("evidence placeholder should be created");
    fs::write(&real_memory_path, "[]").expect("real memory store should be created");
    symlink(&real_memory_path, &linked_memory_path)
        .expect("symlinked memory store should be created");

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("read")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard engineer read symlink rejection should launch");
    let read_rendered = rendered_output(&read_output);

    assert!(
        !read_output.status.success(),
        "engineer read must fail visibly when a required artifact is a symlink:\n{read_rendered}"
    );
    assert!(
        read_rendered.contains("memory_records.json")
            && read_rendered.contains("regular file")
            && read_rendered.contains("symlink"),
        "engineer read should reject symlinked required artifacts explicitly:\n{read_rendered}"
    );
    assert!(
        !read_rendered.contains(&real_memory_path.display().to_string())
            && !read_rendered.contains(&linked_memory_path.display().to_string()),
        "engineer read should name the failing artifact without leaking absolute artifact paths:\n{read_rendered}"
    );
}

#[test]
fn simard_engineer_read_rejects_malformed_carried_meeting_records() {
    let state_root = TempDirGuard::new("simard-cli-engineer-read-malformed-meeting");
    let repo_root = repo_root();
    let meeting_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(
            "agenda: align\n\
decision: preserve continuity\n\
goal: Preserve meeting handoff | priority=1 | status=active | rationale=carry decisions into engineer read",
        )
        .arg(state_root.path())
        .output()
        .expect("meeting run should seed carried context");
    let meeting_rendered = rendered_output(&meeting_output);

    assert!(
        meeting_output.status.success(),
        "meeting run should succeed before carried-meeting tampering:\n{meeting_rendered}"
    );

    let engineer_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("run")
        .arg("single-process")
        .arg(&repo_root)
        .arg(engineer_loop_objective())
        .arg(state_root.path())
        .output()
        .expect("engineer run should seed a readable engineer handoff");
    let engineer_rendered = rendered_output(&engineer_output);

    assert!(
        engineer_output.status.success(),
        "engineer run should succeed before carried-meeting tampering:\n{engineer_rendered}"
    );

    let handoff_path = state_root.path().join("latest_handoff.json");
    let mut handoff = load_json(&handoff_path);
    let evidence_records = handoff["evidence_records"]
        .as_array_mut()
        .expect("handoff should persist evidence records");
    let mut replaced = false;
    for record in evidence_records.iter_mut() {
        let is_carried_detail = record["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("carried-meeting-decisions="));
        if is_carried_detail {
            record["detail"] = json!(
                "carried-meeting-decisions=agenda=align; updates=[]; decisions=not-a-list; risks=[]; next_steps=[]; open_questions=[]; goals=[]"
            );
            replaced = true;
            break;
        }
    }
    assert!(
        replaced,
        "handoff should persist carried meeting decisions before tampering"
    );
    fs::write(
        &handoff_path,
        serde_json::to_vec_pretty(&handoff).expect("tampered handoff should serialize"),
    )
    .expect("tampered handoff should be written");

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("engineer")
        .arg("read")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard engineer read should launch against the malformed carried meeting data");
    let read_rendered = rendered_output(&read_output);

    assert!(
        !read_output.status.success(),
        "engineer read must fail visibly for malformed carried meeting data:\n{read_rendered}"
    );
    assert!(
        read_rendered.contains("carried-meeting-decisions")
            && read_rendered.contains("invalid meeting record"),
        "engineer read should keep malformed carried meeting state explicit:\n{read_rendered}"
    );
}

#[test]
fn simard_meeting_read_reuses_the_run_default_state_root_and_stays_read_only() {
    let _lock = meeting_default_root_lock()
        .lock()
        .expect("meeting default root test lock should not be poisoned");
    let state_root = default_meeting_state_root("local-harness", "single-process");
    let _cleanup = CleanupDirGuard::new(state_root.clone());
    let meeting_objective = "\
agenda: align the next Simard workstream\n\
update: durable \u{1b}[31mmemory\u{1b}[0m merged\n\
decision: preserve meeting-to-engineer continuity\n\
risk: workflow routing is still unreliable\n\
next-step: keep durable priorities visible\n\
open-question: how aggressively should Simard reprioritize?\n\
goal: Preserve \u{1b}]8;;https://example.invalid\u{7}meeting handoff\u{1b}]8;;\u{7} | priority=1 | status=active | rationale=meeting decisions must shape later work";

    let run_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(meeting_objective)
        .output()
        .expect("simard meeting run should launch with its default state root");
    let run_rendered = rendered_output(&run_output);

    assert!(
        run_output.status.success(),
        "meeting run should succeed with its canonical default state root:\n{run_rendered}"
    );
    assert!(
        run_rendered.contains(&format!("State root: {}", state_root.display())),
        "meeting run should surface the canonical default durable root it writes:\n{run_rendered}"
    );

    let memory_before =
        fs::read(state_root.join("memory_records.json")).expect("memory store should exist");
    let goals_before =
        fs::read(state_root.join("goal_records.json")).expect("goal store should exist");

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .output()
        .expect("simard meeting read should launch with its default state root");
    let read_rendered = rendered_output(&read_output);

    assert!(
        read_output.status.success(),
        "meeting read should inspect the same canonical default durable root that run populates:\n{read_rendered}"
    );
    for expected in [
        "Probe mode: meeting-read",
        "Identity: simard-meeting",
        &format!("State root: {}", state_root.display()),
        "Meeting records: 1",
        "Latest agenda: align the next Simard workstream",
        "Updates count: 1",
        "Update 1: durable memory merged",
        "Decisions count: 1",
        "Decision 1: preserve meeting-to-engineer continuity",
        "Risks count: 1",
        "Risk 1: workflow routing is still unreliable",
        "Next steps count: 1",
        "Next step 1: keep durable priorities visible",
        "Open questions count: 1",
        "Open question 1: how aggressively should Simard reprioritize?",
        "Goal updates count: 1",
        "Goal update 1: p1 [active] Preserve meeting handoff",
        "Latest meeting record: agenda=align the next Simard workstream;",
    ] {
        assert!(
            read_rendered.contains(expected),
            "meeting read should surface '{expected}' for operators:\n{read_rendered}"
        );
    }
    for forbidden in ['\u{1b}', '\u{7}'] {
        assert!(
            !read_rendered.contains(forbidden),
            "meeting read should sanitize persisted operator-visible text before printing it:\n{read_rendered}"
        );
    }

    let memory_after =
        fs::read(state_root.join("memory_records.json")).expect("memory store should exist");
    let goals_after =
        fs::read(state_root.join("goal_records.json")).expect("goal store should exist");
    assert_eq!(
        memory_before, memory_after,
        "meeting read must not rewrite the durable memory store"
    );
    assert_eq!(
        goals_before, goals_after,
        "meeting read must not rewrite the durable goal store"
    );
}

#[test]
fn simard_goal_curation_read_reuses_the_run_default_state_root_and_sanitizes_control_sequences() {
    let _lock = goal_curation_default_root_lock()
        .lock()
        .expect("goal-curation default root test lock should not be poisoned");
    let state_root = default_goal_curation_state_root("local-harness", "single-process");
    let _cleanup = CleanupDirGuard::new(state_root.clone());
    let goal_objective = "goal: Keep \u{1b}]8;;https://example.invalid\u{7}active\u{1b}]8;;\u{7}\u{1} priorities inspectable | priority=1 | status=active | rationale=operators need a safe register view";

    let run_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(goal_objective)
        .output()
        .expect("simard goal-curation run should launch with its default state root");
    let run_rendered = rendered_output(&run_output);

    assert!(
        run_output.status.success(),
        "goal-curation run should succeed with its canonical default state root:\n{run_rendered}"
    );
    assert!(
        run_rendered.contains(&format!("State root: {}", state_root.display())),
        "goal-curation run should surface the canonical default durable root it writes:\n{run_rendered}"
    );

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .output()
        .expect("simard goal-curation read should launch with its default state root");
    let read_rendered = rendered_output(&read_output);

    assert!(
        read_output.status.success(),
        "goal-curation read should inspect the same canonical default durable root that run populates:\n{read_rendered}"
    );
    assert!(
        read_rendered.contains(&format!("State root: {}", state_root.display())),
        "goal-curation read should reuse the canonical default durable root from goal-curation run:\n{read_rendered}"
    );
    assert!(
        read_rendered.contains("Active goals count: 1")
            && read_rendered
                .contains("Active goal 1: p1 [active] Keep active priorities inspectable"),
        "goal-curation read should surface the record persisted by goal-curation run even when no explicit state root is passed:\n{read_rendered}"
    );
    for forbidden in ['\u{1b}', '\u{7}', '\u{1}'] {
        assert!(
            !read_rendered.contains(forbidden),
            "goal-curation read should strip terminal control characters before printing persisted goal text:\n{read_rendered}"
        );
    }
}

#[test]
fn simard_goal_curation_read_lists_the_durable_register_across_all_statuses() {
    let state_root = TempDirGuard::new("simard-cli-goal-curation-read");
    let goal_objective = "\
goal: Keep \u{1b}[31mactive\u{1b}[0m priorities inspectable | priority=1 | status=active | rationale=operators need a safe register view\n\
goal: Stage the next backlog slice | priority=2 | status=proposed | rationale=show backlog shape across statuses\n\
goal: Pause brittle experiments | priority=3 | status=paused | rationale=work is blocked pending better infra\n\
goal: Close shipped benchmark truthfulness work | priority=4 | status=completed | rationale=done work should remain inspectable\n\
goal: Preserve operator-visible stewardship | priority=5 | status=active | rationale=active priorities still need top-five carryover";

    let run_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(goal_objective)
        .arg(state_root.path())
        .output()
        .expect("simard goal-curation run should launch");
    let run_rendered = rendered_output(&run_output);

    assert!(
        run_output.status.success(),
        "goal-curation run should remain available for durable state setup:\n{run_rendered}"
    );
    assert!(
        run_rendered.contains("Active goals count: 2"),
        "goal-curation run should keep its shipped active-only top-five summary:\n{run_rendered}"
    );
    assert!(
        !run_rendered.contains("Stage the next backlog slice"),
        "goal-curation run should stay focused on the active top-five surface instead of becoming the inspection workflow:\n{run_rendered}"
    );

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard goal-curation read should launch");
    let read_rendered = rendered_output(&read_output);

    assert!(
        read_output.status.success(),
        "goal-curation read should expose the durable goal register through the canonical CLI:\n{read_rendered}"
    );
    assert!(
        read_rendered.contains("Goal register: durable"),
        "goal-curation read should identify the durable register it is surfacing:\n{read_rendered}"
    );
    assert!(
        read_rendered.contains(&format!("State root: {}", state_root.path().display())),
        "goal-curation read should tell operators which state root was inspected:\n{read_rendered}"
    );
    for expected in [
        "Active goals count: 2",
        "Proposed goals count: 1",
        "Paused goals count: 1",
        "Completed goals count: 1",
        "Active goal 1: p1 [active] Keep active priorities inspectable",
        "Active goal 2: p5 [active] Preserve operator-visible stewardship",
        "Proposed goal 1: p2 [proposed] Stage the next backlog slice",
        "Paused goal 1: p3 [paused] Pause brittle experiments",
        "Completed goal 1: p4 [completed] Close shipped benchmark truthfulness work",
    ] {
        assert!(
            read_rendered.contains(expected),
            "goal-curation read should surface '{expected}' for operators:\n{read_rendered}"
        );
    }
    assert!(
        !read_rendered.contains('\u{1b}'),
        "goal-curation read should sanitize persisted goal text before printing it:\n{read_rendered}"
    );

    let active_index = read_rendered
        .find("Active goals count: 2")
        .expect("active section should be present");
    let proposed_index = read_rendered
        .find("Proposed goals count: 1")
        .expect("proposed section should be present");
    let paused_index = read_rendered
        .find("Paused goals count: 1")
        .expect("paused section should be present");
    let completed_index = read_rendered
        .find("Completed goals count: 1")
        .expect("completed section should be present");
    assert!(
        active_index < proposed_index
            && proposed_index < paused_index
            && paused_index < completed_index,
        "goal-curation read should present statuses in fixed operator order active -> proposed -> paused -> completed:\n{read_rendered}"
    );
}

#[test]
fn simard_goal_curation_read_keeps_operator_visible_paths_and_titles_with_security_words() {
    let state_root = TempDirGuard::new("simard-cli-goal-curation-read-secret");
    let goal_objective = "goal: Secret scanning follow-up | priority=1 | status=active | rationale=operators still need readable titles and paths";

    let run_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(goal_objective)
        .arg(state_root.path())
        .output()
        .expect("simard goal-curation run should launch");
    let run_rendered = rendered_output(&run_output);

    assert!(
        run_output.status.success(),
        "goal-curation run should succeed when goal text contains routine security vocabulary:\n{run_rendered}"
    );

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard goal-curation read should launch");
    let read_rendered = rendered_output(&read_output);

    assert!(
        read_output.status.success(),
        "goal-curation read should still succeed when operator-visible text contains routine security vocabulary:\n{read_rendered}"
    );
    assert!(
        read_rendered.contains(&format!("State root: {}", state_root.path().display())),
        "goal-curation read should keep the inspected state root visible even when the path contains 'secret':\n{read_rendered}"
    );
    assert!(
        read_rendered.contains("Active goal 1: p1 [active] Secret scanning follow-up"),
        "goal-curation read should not redact ordinary goal titles that happen to contain the word 'secret':\n{read_rendered}"
    );
    assert!(
        !read_rendered.contains("[REDACTED]"),
        "goal-curation read should reserve redaction for actual secret-bearing fields, not normal operator data:\n{read_rendered}"
    );
}

#[test]
fn simard_goal_curation_read_rejects_absolute_base_type_and_topology_segments() {
    let absolute_base_type = repo_root().join("absolute-base-type-segment");
    let base_type_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("read")
        .arg(&absolute_base_type)
        .arg("single-process")
        .output()
        .expect("goal-curation read absolute-base-type check should launch");
    let base_type_rendered = rendered_output(&base_type_output);

    assert!(
        !base_type_output.status.success(),
        "goal-curation read must fail when an absolute base-type segment is used to derive the default state root:\n{base_type_rendered}"
    );
    assert!(
        base_type_rendered.contains("no adapter is registered for base type"),
        "goal-curation read should reject absolute base-type segments through base-type validation instead of treating them like filesystem paths:\n{base_type_rendered}"
    );
    assert!(
        !base_type_rendered.contains("Goal register: durable"),
        "goal-curation read must fail before any register rendering when the default path inputs are invalid:\n{base_type_rendered}"
    );

    let absolute_topology = repo_root().join("absolute-topology-segment");
    let topology_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("read")
        .arg("local-harness")
        .arg(&absolute_topology)
        .output()
        .expect("goal-curation read absolute-topology check should launch");
    let topology_rendered = rendered_output(&topology_output);

    assert!(
        !topology_output.status.success(),
        "goal-curation read must fail when an absolute topology segment is used to derive the default state root:\n{topology_rendered}"
    );
    assert!(
        topology_rendered.contains("expected 'single-process', 'multi-process', or 'distributed'"),
        "goal-curation read should reject absolute topology segments through topology validation instead of treating them like filesystem paths:\n{topology_rendered}"
    );
    assert!(
        !topology_rendered.contains("Goal register: durable"),
        "goal-curation read must fail before any register rendering when the default topology is invalid:\n{topology_rendered}"
    );
}

#[test]
fn simard_goal_curation_read_shows_explicit_zero_state_sections_for_an_empty_register() {
    let state_root = TempDirGuard::new("simard-cli-goal-curation-read-empty");

    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("simard goal-curation read should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "goal-curation read should treat a missing durable goal file as an empty register, not a crash:\n{rendered}"
    );
    for expected in [
        "Goal register: durable",
        "Active goals count: 0",
        "Proposed goals count: 0",
        "Paused goals count: 0",
        "Completed goals count: 0",
        "Active goals: <none>",
        "Proposed goals: <none>",
        "Paused goals: <none>",
        "Completed goals: <none>",
    ] {
        assert!(
            rendered.contains(expected),
            "goal-curation read should make empty sections explicit with '{expected}':\n{rendered}"
        );
    }
}

#[test]
fn simard_meeting_read_rejects_nonexistent_and_empty_state_roots_before_store_access() {
    let temp_dir = TempDirGuard::new("simard-cli-meeting-read-invalid-root");
    let missing_root = temp_dir.path().join("missing-layout");
    let empty_root = temp_dir.path().join("empty-layout");
    fs::create_dir_all(&empty_root).expect("empty state root fixture should be created");

    let missing_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&missing_root)
        .output()
        .expect("simard meeting read missing-root check should launch");
    let missing_rendered = rendered_output(&missing_output);

    assert!(
        !missing_output.status.success(),
        "meeting read must fail visibly for a nonexistent explicit state root:\n{missing_rendered}"
    );
    assert!(
        missing_rendered.contains("invalid state root")
            || missing_rendered.contains("InvalidStateRoot"),
        "meeting read should keep the failing state-root contract explicit:\n{missing_rendered}"
    );
    assert!(
        missing_rendered.contains("requires an existing state root directory"),
        "meeting read should explain why a nonexistent explicit root was rejected:\n{missing_rendered}"
    );
    assert!(
        !missing_rendered.contains("expected persisted meeting decision record"),
        "meeting read should reject a nonexistent root before probing for persisted meeting records:\n{missing_rendered}"
    );

    let empty_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("meeting")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&empty_root)
        .output()
        .expect("simard meeting read empty-root check should launch");
    let empty_rendered = rendered_output(&empty_output);

    assert!(
        !empty_output.status.success(),
        "meeting read must fail visibly for an empty explicit state root:\n{empty_rendered}"
    );
    assert!(
        empty_rendered.contains("invalid state root")
            || empty_rendered.contains("InvalidStateRoot"),
        "meeting read should keep empty-root failures in the state-root contract:\n{empty_rendered}"
    );
    assert!(
        empty_rendered.contains("memory_records.json"),
        "meeting read should pinpoint the missing persisted meeting store entry for an empty root:\n{empty_rendered}"
    );
    assert!(
        !empty_rendered.contains("expected persisted meeting decision record"),
        "meeting read should reject an empty root before probing for meeting records:\n{empty_rendered}"
    );
}

#[test]
fn simard_goal_curation_read_rejects_invalid_state_roots_before_any_store_access() {
    let temp_dir = TempDirGuard::new("simard-cli-goal-curation-read-invalid-root");
    let bad_parent_dir_root = temp_dir.path().join("../escape");

    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&bad_parent_dir_root)
        .output()
        .expect("goal-curation read invalid-state-root check should launch");
    let rendered = rendered_output(&output);

    assert!(
        !output.status.success(),
        "goal-curation read must fail visibly for invalid state roots:\n{rendered}"
    );
    assert!(
        rendered.contains("must not contain '..'"),
        "goal-curation read should explain why a traversal root was rejected:\n{rendered}"
    );
    assert!(
        rendered.contains("InvalidStateRoot") || rendered.contains("invalid state root"),
        "goal-curation read should keep state-root validation explicit:\n{rendered}"
    );
}

#[test]
fn simard_goal_curation_read_fails_closed_for_malformed_goal_store_contents() {
    let state_root = TempDirGuard::new("simard-cli-goal-curation-read-malformed-store");
    fs::write(
        state_root.path().join("goal_records.json"),
        "{ not-valid-json",
    )
    .expect("malformed goal store fixture should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("goal-curation read malformed-store check should launch");
    let rendered = rendered_output(&output);

    assert!(
        !output.status.success(),
        "goal-curation read must fail closed when the durable goal store is malformed:\n{rendered}"
    );
    assert!(
        rendered.contains("persistent store 'goals' failed during 'deserialize'"),
        "goal-curation read should surface the durable store boundary instead of silently pretending the register is empty:\n{rendered}"
    );
    assert!(
        !rendered.contains("Active goals count: 0"),
        "goal-curation read should not quietly degrade malformed state into an empty success:\n{rendered}"
    );
}

#[test]
fn simard_goal_curation_read_sanitizes_explicit_runtime_labels_before_printing() {
    let state_root = TempDirGuard::new("simard-cli-goal-curation-read-sanitized-labels");
    let base_type = "local-harness\u{1b}[31m-injected";
    let topology = "single-process\u{1b}]8;;https://example.invalid\u{7}-linked\u{1b}]8;;\u{7}";

    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("goal-curation")
        .arg("read")
        .arg(base_type)
        .arg(topology)
        .arg(state_root.path())
        .output()
        .expect("goal-curation read sanitization check should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "goal-curation read should still succeed with an explicit state root while sanitizing operator-visible labels:\n{rendered}"
    );
    assert!(
        !rendered.contains('\u{1b}'),
        "goal-curation read should strip terminal control sequences from explicit runtime labels:\n{rendered}"
    );
    assert!(
        rendered.contains("Selected base type: local-harness-injected"),
        "goal-curation read should sanitize the base-type label before printing it:\n{rendered}"
    );
    assert!(
        rendered.contains("Topology: single-process-linked"),
        "goal-curation read should sanitize the topology label before printing it:\n{rendered}"
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
fn simard_improvement_curation_read_reuses_the_review_default_state_root_and_stays_read_only() {
    let _lock = review_default_root_lock()
        .lock()
        .expect("review default root test lock should not be poisoned");
    let state_root = default_review_state_root("local-harness", "single-process");
    let _cleanup = CleanupDirGuard::new(state_root.clone());

    let review_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("review")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg("inspect the current Simard review surface and preserve concrete proposals")
        .output()
        .expect("simard review run should launch with its default state root");
    let review_rendered = rendered_output(&review_output);

    assert!(
        review_output.status.success(),
        "review run should succeed with its canonical default state root:\n{review_rendered}"
    );
    assert!(
        review_rendered.contains(&format!("State root: {}", state_root.display())),
        "review run should surface the canonical durable root that improvement read later inspects:\n{review_rendered}"
    );

    let improvement_objective = "\
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now\n\
defer: Promote this pattern into a repeatable benchmark | rationale=wait for the next benchmark planning pass";
    let improvement_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("improvement-curation")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(improvement_objective)
        .output()
        .expect("simard improvement-curation run should launch with the review default state root");
    let improvement_rendered = rendered_output(&improvement_output);

    assert!(
        improvement_output.status.success(),
        "improvement-curation run should share the canonical review/improvement durable root:\n{improvement_rendered}"
    );
    assert!(
        improvement_rendered.contains(&format!("State root: {}", state_root.display())),
        "improvement-curation run should surface the shared durable root that readback reuses:\n{improvement_rendered}"
    );

    let memory_before =
        fs::read(state_root.join("memory_records.json")).expect("memory store should exist");
    let goals_before =
        fs::read(state_root.join("goal_records.json")).expect("goal store should exist");

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("improvement-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .output()
        .expect(
            "simard improvement-curation read should launch with the review default state root",
        );
    let read_rendered = rendered_output(&read_output);

    assert!(
        read_output.status.success(),
        "improvement-curation read should expose durable review and promotion state through the primary CLI:\n{read_rendered}"
    );
    for expected in [
        "Probe mode: improvement-curation-read",
        &format!("State root: {}", state_root.display()),
        "Latest review artifact:",
        "Review id:",
        "Review target:",
        "Approved proposals: 1",
        "Approved proposal 1: p1 [active] Capture denser execution evidence",
        "Deferred proposals: 1",
        "Deferred proposal 1: Promote this pattern into a repeatable benchmark (wait for the next benchmark planning pass)",
        "Active goals count: 1",
        "Active goal 1: p1 [active] Capture denser execution evidence",
        "Proposed goals count: 0",
        "Latest improvement record: review=",
    ] {
        assert!(
            read_rendered.contains(expected),
            "improvement-curation read should surface '{expected}' for operators:\n{read_rendered}"
        );
    }

    let memory_after =
        fs::read(state_root.join("memory_records.json")).expect("memory store should exist");
    let goals_after =
        fs::read(state_root.join("goal_records.json")).expect("goal store should exist");
    assert_eq!(
        memory_before, memory_after,
        "improvement-curation read must not rewrite the durable memory store"
    );
    assert_eq!(
        goals_before, goals_after,
        "improvement-curation read must not rewrite the durable goal store"
    );
}

#[test]
fn simard_improvement_curation_read_rejects_nonexistent_and_empty_state_roots_before_probe_access()
{
    let temp_dir = TempDirGuard::new("simard-cli-improvement-curation-read-invalid-root");
    let missing_root = temp_dir.path().join("missing-layout");
    let empty_root = temp_dir.path().join("empty-layout");
    fs::create_dir_all(&empty_root).expect("empty state root fixture should be created");

    let missing_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("improvement-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&missing_root)
        .output()
        .expect("simard improvement-curation read missing-root check should launch");
    let missing_rendered = rendered_output(&missing_output);

    assert!(
        !missing_output.status.success(),
        "improvement-curation read must fail visibly for a nonexistent explicit state root:\n{missing_rendered}"
    );
    assert!(
        missing_rendered.contains("invalid state root")
            || missing_rendered.contains("InvalidStateRoot"),
        "improvement-curation read should keep the failing state-root contract explicit:\n{missing_rendered}"
    );
    assert!(
        missing_rendered.contains("requires an existing state root directory"),
        "improvement-curation read should explain why a nonexistent explicit root was rejected:\n{missing_rendered}"
    );
    assert!(
        !missing_rendered.contains("expected persisted review artifact"),
        "improvement-curation read should reject a nonexistent root before probing for artifacts:\n{missing_rendered}"
    );

    let empty_output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("improvement-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&empty_root)
        .output()
        .expect("simard improvement-curation read empty-root check should launch");
    let empty_rendered = rendered_output(&empty_output);

    assert!(
        !empty_output.status.success(),
        "improvement-curation read must fail visibly for an empty explicit state root:\n{empty_rendered}"
    );
    assert!(
        empty_rendered.contains("invalid state root")
            || empty_rendered.contains("InvalidStateRoot"),
        "improvement-curation read should keep empty-root failures in the state-root contract:\n{empty_rendered}"
    );
    assert!(
        empty_rendered.contains("review-artifacts"),
        "improvement-curation read should pinpoint the missing read-only layout entry for an empty root:\n{empty_rendered}"
    );
    assert!(
        !empty_rendered.contains("expected persisted review artifact"),
        "improvement-curation read should reject an empty root before probing for review artifacts:\n{empty_rendered}"
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
            && simard_rendered.contains("composite-session-review")
            && simard_rendered.contains("interactive-terminal-driving"),
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
fn simard_gym_run_exercises_the_interactive_terminal_driving_scenario() {
    let artifact_root = TempDirGuard::new("simard-cli-gym-interactive-terminal");
    let output = command_in_dir(env!("CARGO_BIN_EXE_simard"), artifact_root.path())
        .arg("gym")
        .arg("run")
        .arg("interactive-terminal-driving")
        .output()
        .expect("simard interactive terminal benchmark should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "interactive terminal benchmark should succeed on the canonical CLI:\n{rendered}"
    );
    for expected in [
        "Scenario: interactive-terminal-driving",
        "Passed: true",
        "Checks passed:",
        "Artifact report:",
        "Review artifact:",
    ] {
        assert!(
            rendered.contains(expected),
            "interactive terminal benchmark should surface '{expected}' for operators:\n{rendered}"
        );
    }

    let report_path = output_line_value(&rendered, "Artifact report: ")
        .expect("interactive terminal benchmark should surface the report.json artifact path");
    let report = load_json(resolve_cli_artifact_path(artifact_root.path(), report_path));
    assert_eq!(
        report["runtime"]["selected_base_type"].as_str(),
        Some("terminal-shell"),
        "interactive terminal benchmark should report terminal-shell as the selected base type:\n{}",
        serde_json::to_string_pretty(&report).expect("report should render")
    );
    assert_eq!(
        report["scenario"]["identity"].as_str(),
        Some("simard-engineer"),
        "interactive terminal benchmark should exercise the engineer identity rather than the gym harness identity:\n{}",
        serde_json::to_string_pretty(&report).expect("report should render")
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
            rendered.contains("benchmark scenario '../repo-exploration-local' is not registered"),
            "gym {subcommand} should reject invalid scenario ids with the restored single-line operator-facing error contract:\n{rendered}"
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
