use std::io::{self, BufReader};
use std::path::PathBuf;

use crate::bridge_launcher::{cognitive_memory_db_path, find_python_dir, launch_memory_bridge};
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::goals::{FileBackedGoalStore, GoalStatus, GoalStore};
use crate::greeting_banner::print_greeting_banner;
use crate::identity::OperatingMode;
use crate::improvements::PersistedImprovementRecord;
use crate::meeting_repl::run_meeting_repl;
use crate::meetings::PersistedMeetingRecord;
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::operator_commands::{
    GoalRegisterView, print_display, print_goal_section, print_meeting_goal_section,
    print_string_section, print_text, prompt_root, resolved_goal_curation_state_root,
    resolved_improvement_curation_read_state_root, resolved_meeting_read_state_root,
    resolved_state_root,
};
use crate::sanitization::sanitize_terminal_text;
use crate::{
    BootstrapConfig, BootstrapInputs, FileBackedMemoryStore, MemoryScope, MemoryStore,
    latest_local_handoff, latest_review_artifact, render_review_context_directives,
    run_local_session,
};

pub fn run_meeting_probe(
    base_type: &str,
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-meeting";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_state_root(
            state_root_override,
            identity,
            base_type,
            topology,
            "meeting-run",
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    // Display greeting banner before starting the meeting session (no bridge available here)
    print_greeting_banner(None);

    let execution = run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable meeting handoff")?;
    let decision_records = exported
        .memory_records
        .iter()
        .filter(|record| record.scope == MemoryScope::Decision)
        .map(|record| record.value.clone())
        .collect::<Vec<_>>();

    println!("Probe mode: meeting-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    print_display("State root", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!("Decision records: {}", decision_records.len());
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal);
    }
    for (index, value) in decision_records.iter().enumerate() {
        print_text(&format!("Decision record {}", index + 1), value);
    }
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

pub fn run_meeting_read_probe(
    base_type: &str,
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_meeting_read_state_root(state_root_override, base_type, topology)?;
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let meeting_records = memory_store
        .list(MemoryScope::Decision)?
        .into_iter()
        .filter(|record| crate::looks_like_persisted_meeting_record(&record.value))
        .collect::<Vec<_>>();
    let latest_record = meeting_records
        .last()
        .ok_or("expected persisted meeting decision record")?;
    let parsed_record =
        PersistedMeetingRecord::parse(&latest_record.value).map_err(|error| format!("{error}"))?;

    println!("Probe mode: meeting-read");
    println!("Identity: simard-meeting");
    print_text("Selected base type", base_type);
    print_text("Topology", topology);
    print_display("State root", state_root.display());
    println!("Meeting records: {}", meeting_records.len());
    print_text("Latest agenda", &parsed_record.agenda);
    print_string_section("Updates", &parsed_record.updates);
    print_string_section("Decisions", &parsed_record.decisions);
    print_string_section("Risks", &parsed_record.risks);
    print_string_section("Next steps", &parsed_record.next_steps);
    print_string_section("Open questions", &parsed_record.open_questions);
    print_meeting_goal_section(&parsed_record.goals);
    print_text("Latest meeting record", &latest_record.value);
    Ok(())
}

pub fn run_goal_curation_probe(
    base_type: &str,
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-goal-curator";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_goal_curation_state_root(
            state_root_override,
            base_type,
            topology,
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
    println!("Probe mode: goal-curation-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    print_display("State root", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal);
    }
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

pub fn run_goal_curation_read_probe(
    base_type: &str,
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_goal_curation_state_root(state_root_override, base_type, topology)?;
    let goal_store = FileBackedGoalStore::try_new(state_root.join("goal_records.json"))?;
    let goal_records = goal_store.list()?;
    let register = GoalRegisterView::from_records(goal_records);

    println!("Goal register: durable");
    print_text("Selected base type", base_type);
    print_text("Topology", topology);
    print_display("State root", state_root.display());
    register.print();
    Ok(())
}

pub fn run_improvement_curation_probe(
    base_type: &str,
    topology: &str,
    operator_objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = crate::operator_commands::resolved_review_state_root(
        state_root_override,
        base_type,
        topology,
    )?;
    let (review_artifact_path, review) =
        latest_review_artifact(&state_root)?.ok_or("expected persisted review artifact")?;
    let objective = format!(
        "{}\n{}",
        render_review_context_directives(&review),
        operator_objective
    );
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.clone()),
        state_root: Some(state_root.clone()),
        identity: Some("simard-improvement-curator".to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
    let plan = crate::ImprovementPromotionPlan::parse(&objective)?;
    let memory_store = FileBackedMemoryStore::try_new(config.memory_store_path())?;
    let improvement_records = memory_store
        .list(MemoryScope::Decision)?
        .into_iter()
        .filter(|record| record.key.ends_with("improvement-curation-record"))
        .collect::<Vec<_>>();

    println!("Probe mode: improvement-curation-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    print_display("State root", config.state_root_path().display());
    print_display("Review artifact", review_artifact_path.display());
    print_text("Review id", &review.review_id);
    print_text("Review target", &review.target_label);
    println!("Review proposals: {}", review.proposals.len());
    println!("Approved proposals: {}", plan.approvals.len());
    for (index, approval) in plan.approvals.iter().enumerate() {
        println!(
            "Approved proposal {}: p{} [{}] {}",
            index + 1,
            approval.priority,
            approval.status,
            sanitize_terminal_text(&approval.title)
        );
    }
    println!("Deferred proposals: {}", plan.deferrals.len());
    for (index, deferral) in plan.deferrals.iter().enumerate() {
        println!(
            "Deferred proposal {}: {} ({})",
            index + 1,
            sanitize_terminal_text(&deferral.title),
            sanitize_terminal_text(&deferral.rationale)
        );
    }
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal);
    }
    println!(
        "Proposed goals count: {}",
        execution.snapshot.proposed_goal_count
    );
    for (index, goal) in execution.snapshot.proposed_goals.iter().enumerate() {
        print_text(&format!("Proposed goal {}", index + 1), goal);
    }
    println!("Decision records: {}", improvement_records.len());
    if let Some(record) = improvement_records.last() {
        print_text("Latest improvement record", &record.value);
    }
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

pub fn run_improvement_curation_read_probe(
    base_type: &str,
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root =
        resolved_improvement_curation_read_state_root(state_root_override, base_type, topology)?;
    let (review_artifact_path, review) =
        latest_review_artifact(&state_root)?.ok_or("expected persisted review artifact")?;
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let latest_record = memory_store
        .list(MemoryScope::Decision)?
        .into_iter()
        .rfind(|record| record.key.ends_with("improvement-curation-record"))
        .ok_or("expected persisted improvement decision record")?;
    let parsed_record = PersistedImprovementRecord::parse(&latest_record.value)
        .map_err(|error| format!("{error}"))?;
    let goal_store = FileBackedGoalStore::try_new(state_root.join("goal_records.json"))?;
    let goal_records = goal_store.list()?;

    println!("Probe mode: improvement-curation-read");
    println!("Identity: simard-improvement-curator");
    print_text(
        "Selected base type",
        parsed_record
            .selected_base_type
            .as_deref()
            .unwrap_or(&review.selected_base_type),
    );
    print_text(
        "Topology",
        parsed_record
            .topology
            .as_deref()
            .unwrap_or(&review.topology),
    );
    print_display("State root", state_root.display());
    print_display("Latest review artifact", review_artifact_path.display());
    print_text("Review id", &review.review_id);
    print_text("Review target", &review.target_label);
    println!("Review proposals: {}", review.proposals.len());
    println!(
        "Approved proposals: {}",
        parsed_record.approved_proposals.len()
    );
    if parsed_record.approved_proposals.is_empty() {
        println!("Approved proposals: <none>");
    } else {
        for (index, approval) in parsed_record.approved_proposals.iter().enumerate() {
            print_text(
                &format!("Approved proposal {}", index + 1),
                approval.concise_label(),
            );
        }
    }
    println!(
        "Deferred proposals: {}",
        parsed_record.deferred_proposals.len()
    );
    if parsed_record.deferred_proposals.is_empty() {
        println!("Deferred proposals: <none>");
    } else {
        for (index, deferral) in parsed_record.deferred_proposals.iter().enumerate() {
            print_text(
                &format!("Deferred proposal {}", index + 1),
                format!("{} ({})", deferral.title, deferral.rationale),
            );
        }
    }
    print_goal_section(&goal_records, GoalStatus::Active, "Active");
    print_goal_section(&goal_records, GoalStatus::Proposed, "Proposed");
    print_text("Latest improvement record", parsed_record.concise_record());
    Ok(())
}

/// Load the meeting system prompt from prompt_assets/simard/meeting_system.md.
fn load_meeting_system_prompt() -> String {
    let path = prompt_root().join("simard/meeting_system.md");
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// Attempt to launch the real Python memory bridge for meeting mode.
///
/// Uses the same `BridgeLauncher` infrastructure as engineer mode: locates the
/// `python/` directory, starts `simard_memory_bridge.py`, and connects to Kuzu.
/// Returns `None` if any step fails so the caller can fall back gracefully.
fn launch_real_meeting_bridge() -> Option<CognitiveMemoryBridge> {
    let python_dir = find_python_dir().ok()?;
    let state_root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/simard-state"));
    let _ = std::fs::create_dir_all(&state_root);
    let db_path = cognitive_memory_db_path(&state_root);
    launch_memory_bridge("simard-meeting", &db_path, &python_dir).ok()
}

/// Auto-detect the best available base type and open a session for the meeting.
///
/// Priority: RustyClawd (needs ANTHROPIC_API_KEY) → local-harness fallback.
/// Returns `None` if no agent backend can be initialised — the REPL will then
/// degrade to note-taking mode.
pub fn run_meeting_repl_command(topic: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Try to launch the real Python memory bridge backed by Kuzu graph database.
    // Falls back to an in-memory stub if the bridge is unavailable (no Python,
    // missing bridge_server.py, etc.).
    let bridge = match launch_real_meeting_bridge() {
        Some(b) => {
            eprintln!("  Memory: cognitive bridge active (Kuzu backend)");
            b
        }
        None => {
            eprintln!(
                "  \u{26a0} Memory bridge unavailable \u{2014} using in-memory stub (memories will not persist to Kuzu)"
            );
            let transport =
                InMemoryBridgeTransport::new("meeting-repl", |method, _params| match method {
                    "memory.record_sensory" => Ok(serde_json::json!({"id": "sen_repl"})),
                    "memory.store_episode" => Ok(serde_json::json!({"id": "epi_repl"})),
                    "memory.store_fact" => Ok(serde_json::json!({"id": "sem_repl"})),
                    "memory.store_prospective" => Ok(serde_json::json!({"id": "pro_repl"})),
                    "memory.search_facts" => Ok(serde_json::json!({"facts": []})),
                    "memory.get_statistics" => Ok(serde_json::json!({
                        "sensory_count": 0, "working_count": 0, "episodic_count": 0,
                        "semantic_count": 0, "procedural_count": 0, "prospective_count": 0
                    })),
                    _ => Err(crate::bridge::BridgeErrorPayload {
                        code: -32601,
                        message: format!("unknown method: {method}"),
                    }),
                });
            CognitiveMemoryBridge::new(Box::new(transport))
        }
    };

    // Display greeting banner with memory bridge context
    print_greeting_banner(Some(&bridge));

    // Open an agent session for conversational meeting mode.
    // Uses the same base type infrastructure as engineer mode.
    let mut agent_session = open_meeting_agent_session();
    let base_prompt = load_meeting_system_prompt();
    let live_context = build_live_meeting_context(&bridge);
    let meeting_system_prompt = format!("{base_prompt}\n\n{live_context}");

    if agent_session.is_some() {
        eprintln!("  Agent: ready");
    } else {
        eprintln!("  ⚠ No agent backend available — meeting will be note-taking only.");
        eprintln!("    Set ANTHROPIC_API_KEY for RustyClawd conversation.");
    }

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    let session = match agent_session {
        Some(ref mut boxed_agent) => run_meeting_repl(
            topic,
            &bridge,
            Some(&mut **boxed_agent),
            &meeting_system_prompt,
            &mut reader,
            &mut writer,
        )?,
        None => run_meeting_repl(
            topic,
            &bridge,
            None,
            &meeting_system_prompt,
            &mut reader,
            &mut writer,
        )?,
    };

    println!("Meeting closed.");
    println!("Decisions: {}", session.decisions.len());
    println!("Action items: {}", session.action_items.len());
    println!("Notes: {}", session.notes.len());
    Ok(())
}

/// Build live context from cognitive memory, goals, and project state to
/// enrich the meeting system prompt so Simard knows her own state.
fn build_live_meeting_context(bridge: &CognitiveMemoryBridge) -> String {
    let mut sections = Vec::new();

    // Recent meeting summaries (decisions from past meetings)
    let past_meetings = bridge.search_facts("meeting:", 10, 0.0).unwrap_or_default();
    if !past_meetings.is_empty() {
        let mut meeting_text = String::from("## Previous Meeting Summaries\n");
        for (i, m) in past_meetings.iter().enumerate().take(5) {
            meeting_text.push_str(&format!("{}. [{}] {}\n", i + 1, m.concept, m.content));
        }
        sections.push(meeting_text);
    }

    // Recent decisions from meetings (individually stored by REPL)
    let past_decisions = bridge
        .search_facts("decision:", 10, 0.0)
        .unwrap_or_default();
    if !past_decisions.is_empty() {
        let mut dec_text = String::from("## Past Decisions\n");
        for (i, d) in past_decisions.iter().enumerate().take(10) {
            dec_text.push_str(&format!("{}. {}\n", i + 1, d.content));
        }
        sections.push(dec_text);
    }

    // Active goals
    let goals = bridge.search_facts("goal:", 10, 0.0).unwrap_or_default();
    if !goals.is_empty() {
        let mut goal_text = String::from("## Active Goals\n");
        for (i, g) in goals.iter().enumerate().take(5) {
            goal_text.push_str(&format!("{}. {}\n", i + 1, g.content));
        }
        sections.push(goal_text);
    }

    // Operator identity
    let operator = bridge.search_facts("operator:", 3, 0.0).unwrap_or_default();
    if !operator.is_empty() {
        let mut op_text = String::from("## Operator Context\n");
        for fact in &operator {
            op_text.push_str(&format!("- {}\n", fact.content));
        }
        sections.push(op_text);
    } else {
        sections.push(
            "## Operator Context\nYour operator is Ryan Sweet (GitHub: rysweet). \
             He is your creator and principal architect. He manages the Simard, \
             RustyClawd, amplihack, and azlin repositories.\n"
                .to_string(),
        );
    }

    // Known projects
    let projects = bridge.search_facts("project:", 10, 0.0).unwrap_or_default();
    if !projects.is_empty() {
        let mut proj_text = String::from("## Known Projects\n");
        for p in &projects {
            proj_text.push_str(&format!("- {}\n", p.content));
        }
        sections.push(proj_text);
    } else {
        sections.push(
            "## Known Projects\n\
             - Simard (this project) — autonomous engineering agent in Rust\n\
             - RustyClawd — LLM + tool calling SDK\n\
             - amplihack — agentic coding framework\n\
             - azlin — Azure VM orchestration CLI\n\
             - amplihack-memory-lib — 6-type cognitive memory system\n"
                .to_string(),
        );
    }

    // Research tracker / watched developers
    let research = bridge.search_facts("research:", 5, 0.0).unwrap_or_default();
    if !research.is_empty() {
        let mut res_text = String::from("## Research Topics\n");
        for r in &research {
            res_text.push_str(&format!("- {}\n", r.content));
        }
        sections.push(res_text);
    }

    // Recent improvements
    let improvements = bridge
        .search_facts("improvement:", 5, 0.0)
        .unwrap_or_default();
    if !improvements.is_empty() {
        let mut imp_text = String::from("## Improvement Backlog\n");
        for imp in &improvements {
            imp_text.push_str(&format!("- {}\n", imp.content));
        }
        sections.push(imp_text);
    }

    if sections.is_empty() {
        String::from("## Live State\nNo cognitive memory available for this session.\n")
    } else {
        format!(
            "## Live State (from cognitive memory)\n\n{}",
            sections.join("\n")
        )
    }
}

/// Open an agent session for the meeting REPL using the standard base type
/// infrastructure. Same agent identity, same platform — just meeting mode.
fn open_meeting_agent_session() -> Option<Box<dyn crate::base_types::BaseTypeSession>> {
    crate::session_builder::SessionBuilder::new(OperatingMode::Meeting)
        .node_id("meeting-repl")
        .address("meeting-repl://local")
        .adapter_tag("meeting-rustyclawd")
        .open()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a `CognitiveMemoryBridge` backed by an in-memory stub that
    /// returns empty results for all `search_facts` queries.
    fn empty_bridge() -> CognitiveMemoryBridge {
        let transport =
            InMemoryBridgeTransport::new("test-empty", |method, _params| match method {
                "memory.search_facts" => Ok(serde_json::json!({"facts": []})),
                "memory.get_statistics" => Ok(serde_json::json!({
                    "sensory_count": 0, "working_count": 0, "episodic_count": 0,
                    "semantic_count": 0, "procedural_count": 0, "prospective_count": 0
                })),
                _ => Err(crate::bridge::BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown method: {method}"),
                }),
            });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    /// Create a bridge that returns a single meeting fact for `"meeting:"`
    /// queries and empty results for everything else.
    fn bridge_with_meeting_facts() -> CognitiveMemoryBridge {
        let transport = InMemoryBridgeTransport::new("test-facts", |method, params| match method {
            "memory.search_facts" => {
                let query = params["query"].as_str().unwrap_or("");
                if query.starts_with("meeting:") {
                    Ok(serde_json::json!({
                        "facts": [{
                            "node_id": "f1",
                            "concept": "weekly-sync",
                            "content": "Discussed deployment timeline",
                            "confidence": 0.9,
                            "source_id": "s1",
                            "tags": []
                        }]
                    }))
                } else {
                    Ok(serde_json::json!({"facts": []}))
                }
            }
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    // ── build_live_meeting_context ──────────────────────────────────────

    #[test]
    fn build_live_meeting_context_defaults_with_empty_bridge() {
        let bridge = empty_bridge();
        let ctx = build_live_meeting_context(&bridge);

        assert!(
            ctx.starts_with("## Live State (from cognitive memory)"),
            "expected live-state header, got: {ctx}"
        );
        assert!(
            ctx.contains("## Operator Context"),
            "expected default operator section"
        );
        assert!(ctx.contains("Ryan Sweet"), "expected default operator name");
        assert!(
            ctx.contains("## Known Projects"),
            "expected default projects section"
        );
        assert!(
            ctx.contains("Simard"),
            "expected Simard in default projects"
        );
    }

    #[test]
    fn build_live_meeting_context_includes_bridge_meeting_facts() {
        let bridge = bridge_with_meeting_facts();
        let ctx = build_live_meeting_context(&bridge);

        assert!(
            ctx.contains("Previous Meeting Summaries"),
            "expected meeting summaries section"
        );
        assert!(
            ctx.contains("Discussed deployment timeline"),
            "expected meeting content from bridge"
        );
    }

    // ── load_meeting_system_prompt ──────────────────────────────────────

    #[test]
    fn load_meeting_system_prompt_does_not_panic() {
        // Uses unwrap_or_default internally so must never panic even when
        // the prompt asset file is absent (e.g. in CI).
        let _prompt = load_meeting_system_prompt();
    }

    // ── run_meeting_read_probe error paths ─────────────────────────────

    #[test]
    fn meeting_read_probe_rejects_nonexistent_state_root() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        let result = run_meeting_read_probe("local-harness", "single-process", Some(missing));
        assert!(result.is_err(), "expected error for nonexistent state root");
    }

    #[test]
    fn meeting_read_probe_rejects_missing_memory_file() {
        let dir = TempDir::new().unwrap();
        let result = run_meeting_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_err(),
            "expected error when memory_records.json is absent"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("memory_records.json"),
            "error should mention the missing file: {msg}"
        );
    }

    #[test]
    fn meeting_read_probe_rejects_empty_meeting_store() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("memory_records.json"), "[]").unwrap();
        let result = run_meeting_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_err(),
            "expected error when no meeting records exist"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("expected persisted meeting decision record"),
            "error should explain the missing record: {msg}"
        );
    }

    // ── run_goal_curation_read_probe ───────────────────────────────────

    #[test]
    fn goal_curation_read_probe_succeeds_with_empty_state() {
        let dir = TempDir::new().unwrap();
        let result = run_goal_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_ok(),
            "expected success with empty state: {:?}",
            result.err()
        );
    }

    // ── run_improvement_curation_read_probe ─────────────────────────────

    #[test]
    fn improvement_curation_read_probe_rejects_incomplete_state() {
        let dir = TempDir::new().unwrap();
        let result = run_improvement_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_err(),
            "expected error when review artifacts are missing"
        );
    }
}
