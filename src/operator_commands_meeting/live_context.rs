use crate::memory_bridge::CognitiveMemoryBridge;

/// Search the bridge for facts matching `query`, logging a warning on failure.
///
/// Returns the matching facts, or an empty `Vec` when the bridge call fails
/// (after logging the error so operators can diagnose memory issues).
fn search_or_warn(
    bridge: &CognitiveMemoryBridge,
    query: &str,
    limit: u32,
) -> Vec<crate::memory_cognitive::CognitiveFact> {
    match bridge.search_facts(query, limit, 0.0) {
        Ok(facts) => facts,
        Err(e) => {
            eprintln!("[simard] live_context: memory search failed for \"{query}\": {e}");
            Vec::new()
        }
    }
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
pub(super) fn build_live_meeting_context(bridge: &CognitiveMemoryBridge) -> String {
    let mut sections = Vec::new();

    // Recent meeting summaries (decisions from past meetings)
    let past_meetings = search_or_warn(bridge, "meeting:", 10);
    if !past_meetings.is_empty() {
        let mut meeting_text = String::from("## Previous Meeting Summaries\n");
        for (i, m) in past_meetings.iter().enumerate().take(5) {
            meeting_text.push_str(&format!("{}. [{}] {}\n", i + 1, m.concept, m.content));
        }
        sections.push(meeting_text);
    }

    // Recent decisions from meetings (individually stored by REPL)
    let past_decisions = search_or_warn(bridge, "decision:", 10);
    if !past_decisions.is_empty() {
        let mut dec_text = String::from("## Past Decisions\n");
        for (i, d) in past_decisions.iter().enumerate().take(10) {
            dec_text.push_str(&format!("{}. {}\n", i + 1, d.content));
        }
        sections.push(dec_text);
    }

    // Active goals
    let goals = search_or_warn(bridge, "goal:", 10);
    if !goals.is_empty() {
        let mut goal_text = String::from("## Active Goals\n");
        for (i, g) in goals.iter().enumerate().take(5) {
            goal_text.push_str(&format!("{}. {}\n", i + 1, g.content));
        }
        sections.push(goal_text);
    }

    // Operator identity — from memory, env var, or generic fallback (never hardcoded name)
    let operator = search_or_warn(bridge, "operator:", 3);
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
    let projects = search_or_warn(bridge, "project:", 10);
    if !projects.is_empty() {
        let mut proj_text = String::from("## Known Projects\n");
        for p in &projects {
            proj_text.push_str(&format!("- {}\n", p.content));
        }
        sections.push(proj_text);
    }
    // No hardcoded fallback — if memory has no projects, the section is omitted.

    // Research tracker / watched developers
    let research = search_or_warn(bridge, "research:", 5);
    if !research.is_empty() {
        let mut res_text = String::from("## Research Topics\n");
        for r in &research {
            res_text.push_str(&format!("- {}\n", r.content));
        }
        sections.push(res_text);
    }

    // Recent improvements
    let improvements = search_or_warn(bridge, "improvement:", 5);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeErrorPayload;
    use crate::bridge_subprocess::InMemoryBridgeTransport;

    // ── resolve_operator_name ───────────────────────────────────────

    #[test]
    fn resolve_operator_name_default_fallback() {
        unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };
        let name = resolve_operator_name();
        assert_eq!(name, "operator");
    }

    #[test]
    fn resolve_operator_name_from_env() {
        unsafe { std::env::set_var("SIMARD_OPERATOR_NAME", "alice") };
        let name = resolve_operator_name();
        assert_eq!(name, "alice");
        unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };
    }

    #[test]
    fn resolve_operator_name_empty_env_falls_back() {
        unsafe { std::env::set_var("SIMARD_OPERATOR_NAME", "") };
        let name = resolve_operator_name();
        assert_eq!(name, "operator");
        unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };
    }

    // ── search_or_warn ──────────────────────────────────────────────

    fn empty_bridge() -> CognitiveMemoryBridge {
        let transport = InMemoryBridgeTransport::new("test-ctx", |method, _params| match method {
            "memory.search_facts" => Ok(serde_json::json!({"facts": []})),
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    fn failing_bridge() -> CognitiveMemoryBridge {
        let transport = InMemoryBridgeTransport::new("test-fail", |_method, _params| {
            Err(BridgeErrorPayload {
                code: -1,
                message: "forced error".to_string(),
            })
        });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    #[test]
    fn search_or_warn_empty_result() {
        let bridge = empty_bridge();
        let facts = search_or_warn(&bridge, "anything", 5);
        assert!(facts.is_empty());
    }

    #[test]
    fn search_or_warn_bridge_failure_returns_empty() {
        let bridge = failing_bridge();
        let facts = search_or_warn(&bridge, "query", 5);
        assert!(facts.is_empty());
    }

    // ── build_live_meeting_context ──────────────────────────────────

    #[test]
    fn build_context_empty_memory() {
        let bridge = empty_bridge();
        unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };
        let ctx = build_live_meeting_context(&bridge);
        assert!(ctx.contains("Operator Context"));
        assert!(ctx.contains("operator"));
    }

    #[test]
    fn build_context_failing_bridge_shows_operator_fallback() {
        let bridge = failing_bridge();
        unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };
        let ctx = build_live_meeting_context(&bridge);
        assert!(ctx.contains("Operator Context"));
    }

    #[test]
    fn build_context_always_has_operator_section() {
        let bridge = empty_bridge();
        let ctx = build_live_meeting_context(&bridge);
        assert!(ctx.contains("Operator Context"));
    }

    #[test]
    fn build_context_result_is_not_empty() {
        let bridge = empty_bridge();
        let ctx = build_live_meeting_context(&bridge);
        assert!(!ctx.is_empty());
    }

    #[test]
    fn build_context_contains_live_state_header() {
        let bridge = empty_bridge();
        let ctx = build_live_meeting_context(&bridge);
        // Either shows "Live State" or "Live State (from cognitive memory)"
        assert!(ctx.contains("Live State"));
    }
}
