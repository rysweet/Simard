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
