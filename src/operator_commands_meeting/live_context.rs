use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;

/// Search the memory for facts matching `query`.
///
/// Client errors propagate per PHILOSOPHY.md — no silent degradation.
fn search_memory(
    memory: &dyn CognitiveMemoryOps,
    query: &str,
    limit: u32,
) -> SimardResult<Vec<crate::memory_cognitive::CognitiveFact>> {
    memory.search_facts(query, limit, 0.0)
}

/// Resolve the operator display name.
///
/// Precedence:
/// 1. `SIMARD_OPERATOR_NAME` environment variable (if set and non-empty)
/// 2. Falls back to `"operator"`
fn resolve_operator_name() -> String {
    std::env::var("SIMARD_OPERATOR_NAME")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "operator".to_string())
}

/// Build live context from cognitive memory, goals, and project state to
/// enrich the meeting system prompt so Simard knows her own state.
pub(super) fn build_live_meeting_context(memory: &dyn CognitiveMemoryOps) -> SimardResult<String> {
    let mut sections = Vec::new();

    // Recent meeting summaries (decisions from past meetings)
    let past_meetings = search_memory(memory, "meeting:", 10)?;
    if !past_meetings.is_empty() {
        let mut meeting_text = String::from("## Previous Meeting Summaries\n");
        for (i, m) in past_meetings.iter().enumerate().take(5) {
            meeting_text.push_str(&format!("{}. [{}] {}\n", i + 1, m.concept, m.content));
        }
        sections.push(meeting_text);
    }

    // Recent decisions from meetings (individually stored by REPL)
    let past_decisions = search_memory(memory, "decision:", 10)?;
    if !past_decisions.is_empty() {
        let mut dec_text = String::from("## Past Decisions\n");
        for (i, d) in past_decisions.iter().enumerate().take(10) {
            dec_text.push_str(&format!("{}. {}\n", i + 1, d.content));
        }
        sections.push(dec_text);
    }

    // Active goals
    let goals = search_memory(memory, "goal:", 10)?;
    if !goals.is_empty() {
        let mut goal_text = String::from("## Active Goals\n");
        for (i, g) in goals.iter().enumerate().take(5) {
            goal_text.push_str(&format!("{}. {}\n", i + 1, g.content));
        }
        sections.push(goal_text);
    }

    // Operator identity — from memory, env var, or resolved name
    let operator = search_memory(memory, "operator:", 3)?;
    if !operator.is_empty() {
        let mut op_text = String::from("## Operator Context\n");
        for fact in &operator {
            op_text.push_str(&format!("- {}\n", fact.content));
        }
        sections.push(op_text);
    } else {
        let name = resolve_operator_name();
        sections.push(format!("## Operator Context\nYour operator is {name}.\n"));
    }

    // Known projects — only shown when memory has project facts
    let projects = search_memory(memory, "project:", 10)?;
    if !projects.is_empty() {
        let mut proj_text = String::from("## Known Projects\n");
        for p in &projects {
            proj_text.push_str(&format!("- {}\n", p.content));
        }
        sections.push(proj_text);
    }

    // Research tracker / watched developers
    let research = search_memory(memory, "research:", 5)?;
    if !research.is_empty() {
        let mut res_text = String::from("## Research Topics\n");
        for r in &research {
            res_text.push_str(&format!("- {}\n", r.content));
        }
        sections.push(res_text);
    }

    // Recent improvements
    let improvements = search_memory(memory, "improvement:", 5)?;
    if !improvements.is_empty() {
        let mut imp_text = String::from("## Improvement Backlog\n");
        for imp in &improvements {
            imp_text.push_str(&format!("- {}\n", imp.content));
        }
        sections.push(imp_text);
    }

    if sections.is_empty() {
        Ok(String::from(
            "## Live State\nNo cognitive memory available for this session.\n",
        ))
    } else {
        Ok(format!(
            "## Live State (from cognitive memory)\n\n{}",
            sections.join("\n")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_client::CognitiveMemoryClient;
    use crate::rpc::RpcErrorPayload;
    use crate::rpc_transport::InMemoryRpcTransport;

    /// Mutex to serialize tests that mutate the `SIMARD_OPERATOR_NAME` env var.
    /// `set_var`/`remove_var` are process-global so concurrent tests would race.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ── resolve_operator_name ───────────────────────────────────────

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn resolve_operator_name_default() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };
        let name = resolve_operator_name();
        assert_eq!(name, "operator");
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn resolve_operator_name_from_env() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("SIMARD_OPERATOR_NAME", "alice") };
        let name = resolve_operator_name();
        assert_eq!(name, "alice");
        unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn resolve_operator_name_empty_env_falls_back() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("SIMARD_OPERATOR_NAME", "") };
        let name = resolve_operator_name();
        assert_eq!(name, "operator");
        unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };
    }

    // ── search_memory ─────────────────────────────────────────────

    fn empty_memory() -> CognitiveMemoryClient {
        let transport = InMemoryRpcTransport::new("test-ctx", |method, _params| match method {
            "memory.search_facts" => Ok(serde_json::json!({"facts": []})),
            _ => Err(RpcErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        });
        CognitiveMemoryClient::new(Box::new(transport))
    }

    fn failing_memory() -> CognitiveMemoryClient {
        let transport = InMemoryRpcTransport::new("test-fail", |_method, _params| {
            Err(RpcErrorPayload {
                code: -1,
                message: "forced error".to_string(),
            })
        });
        CognitiveMemoryClient::new(Box::new(transport))
    }

    #[test]
    fn search_memory_empty_result() {
        let memory = empty_memory();
        let facts = search_memory(&memory, "anything", 5).unwrap();
        assert!(facts.is_empty());
    }

    #[test]
    fn search_memory_failure_propagates() {
        let memory = failing_memory();
        let result = search_memory(&memory, "query", 5);
        assert!(result.is_err());
    }

    // ── build_live_meeting_context ──────────────────────────────────

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn build_context_empty_memory() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let memory = empty_memory();
        unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };
        let ctx = build_live_meeting_context(&memory).unwrap();
        assert!(ctx.contains("Operator Context"));
        assert!(ctx.contains("operator"));
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn build_context_failing_memory_propagates_error() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let memory = failing_memory();
        unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };
        let result = build_live_meeting_context(&memory);
        assert!(
            result.is_err(),
            "memory failures must propagate, not silently degrade"
        );
    }

    #[test]
    fn build_context_always_has_operator_section() {
        let memory = empty_memory();
        let ctx = build_live_meeting_context(&memory).unwrap();
        assert!(ctx.contains("Operator Context"));
    }

    #[test]
    fn build_context_result_is_not_empty() {
        let memory = empty_memory();
        let ctx = build_live_meeting_context(&memory).unwrap();
        assert!(!ctx.is_empty());
    }

    #[test]
    fn build_context_contains_live_state_header() {
        let memory = empty_memory();
        let ctx = build_live_meeting_context(&memory).unwrap();
        assert!(ctx.contains("Live State"));
    }
}
