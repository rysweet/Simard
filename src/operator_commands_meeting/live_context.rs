use crate::memory_bridge::CognitiveMemoryBridge;

/// Build live context from cognitive memory, goals, and project state to
/// enrich the meeting system prompt so Simard knows her own state.
pub(super) fn build_live_meeting_context(bridge: &CognitiveMemoryBridge) -> String {
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
