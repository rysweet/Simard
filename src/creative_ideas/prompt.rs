//! Prompt rendering + tolerant JSON extraction for the Creative Ideas agents.
//!
//! The idea source and the reviewer adapters follow guideline **G3**: they ship
//! a prompt asset with an explicit JSON output contract, invoke an agent, and
//! extract the structured envelope tolerantly (fenced ```json block, or the
//! widest brace/bracket span) rather than brittle line parsing. Extraction is
//! **fail-closed** at the call site: a response with no JSON envelope is a hard
//! error, never a silent default.
#![allow(dead_code)]

use crate::cognitive_threads::threads::creative_ideas::GenerationInputs;

/// Maximum entries rendered per context section (keeps prompts bounded).
const MAX_SECTION_ENTRIES: usize = 12;

/// Extract the first parseable JSON value from an agent's raw response.
///
/// Tries, in order: fenced code blocks, the widest `{...}` object span, the
/// widest `[...]` array span, then the whole trimmed string. Returns `None`
/// when nothing parses — callers treat that as a fail-closed error.
#[must_use]
pub fn extract_json_value(raw: &str) -> Option<serde_json::Value> {
    for block in fenced_blocks(raw) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(block.trim()) {
            return Some(v);
        }
    }
    // Prefer whichever structure opens first (outermost), falling back to the
    // other so an inner object inside an array is not mistaken for the whole.
    let obj_start = raw.find('{');
    let arr_start = raw.find('[');
    let object = || widest_span(raw, '{', '}');
    let array = || widest_span(raw, '[', ']');
    let (first, second): (
        &dyn Fn() -> Option<serde_json::Value>,
        &dyn Fn() -> Option<serde_json::Value>,
    ) = match (obj_start, arr_start) {
        (Some(o), Some(a)) if a < o => (&array, &object),
        _ => (&object, &array),
    };
    if let Some(v) = first() {
        return Some(v);
    }
    if let Some(v) = second() {
        return Some(v);
    }
    serde_json::from_str::<serde_json::Value>(raw.trim()).ok()
}

/// Yield the bodies of ```` ``` ````-fenced blocks (with any leading `json`
/// info-string stripped), longest-looking first is not required — callers try
/// each in order.
fn fenced_blocks(raw: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = raw;
    while let Some(open) = rest.find("```") {
        let after_open = &rest[open + 3..];
        let Some(close_rel) = after_open.find("```") else {
            break;
        };
        let body = &after_open[..close_rel];
        // Strip an optional info string on the same line as the opening fence.
        let body = match body.split_once('\n') {
            Some((first, tail)) if first.trim().chars().all(|c| c.is_alphanumeric()) => tail,
            _ => body,
        };
        blocks.push(body.to_string());
        rest = &after_open[close_rel + 3..];
    }
    blocks
}

/// Parse the widest balanced span delimited by `open`/`close` as JSON.
fn widest_span(raw: &str, open: char, close: char) -> Option<serde_json::Value> {
    let start = raw.find(open)?;
    let end = raw.rfind(close)?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(&raw[start..=end]).ok()
}

/// Render the review context block from the generation inputs (bounded).
#[must_use]
pub fn render_review_context(inputs: &GenerationInputs) -> String {
    let mut out = String::new();
    section(&mut out, "Current goals", &inputs.current_goals);
    section(&mut out, "Recent activity", &inputs.recent_activity.entries);
    section(&mut out, "Episodic summaries", &inputs.episodic_summaries);
    section(&mut out, "Works in progress", &inputs.works_in_progress);
    section(
        &mut out,
        "Overseer observations",
        &inputs.overseer_observations,
    );
    section(
        &mut out,
        "Conversation insights",
        &inputs.conversation_insights,
    );
    if out.is_empty() {
        out.push_str("(no additional context available)\n");
    }
    out
}

/// Render the full generation prompt from a template + inputs, targeting
/// `count` ideas.
#[must_use]
pub fn render_generation_prompt(template: &str, inputs: &GenerationInputs, count: usize) -> String {
    let previous: Vec<String> = inputs
        .previous_ideas
        .iter()
        .map(|i| i.idea.clone())
        .collect();
    template
        .replace("{{COUNT}}", &count.to_string())
        .replace("{{GOALS}}", &bullet_list(&inputs.current_goals))
        .replace("{{RECENT}}", &bullet_list(&inputs.recent_activity.entries))
        .replace("{{EPISODIC}}", &bullet_list(&inputs.episodic_summaries))
        .replace("{{WIP}}", &bullet_list(&inputs.works_in_progress))
        .replace("{{OVERSEER}}", &bullet_list(&inputs.overseer_observations))
        .replace(
            "{{CONVERSATIONS}}",
            &bullet_list(&inputs.conversation_insights),
        )
        .replace("{{PREVIOUS}}", &bullet_list(&previous))
}

fn section(out: &mut String, heading: &str, entries: &[String]) {
    let items: Vec<&String> = entries.iter().filter(|e| !e.trim().is_empty()).collect();
    if items.is_empty() {
        return;
    }
    out.push_str("### ");
    out.push_str(heading);
    out.push('\n');
    for item in items.iter().take(MAX_SECTION_ENTRIES) {
        out.push_str("- ");
        out.push_str(item.trim());
        out.push('\n');
    }
    out.push('\n');
}

fn bullet_list(entries: &[String]) -> String {
    let items: Vec<String> = entries
        .iter()
        .filter(|e| !e.trim().is_empty())
        .take(MAX_SECTION_ENTRIES)
        .map(|e| format!("- {}", e.trim()))
        .collect();
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fenced_json_object() {
        let raw = "some preamble\n```json\n{\"verdict\": \"Support\"}\n```\ntrailer";
        let v = extract_json_value(raw).expect("json");
        assert_eq!(v["verdict"], "Support");
    }

    #[test]
    fn extracts_bare_object_span() {
        let raw = "noise {\"a\": 1, \"b\": [2,3]} noise";
        let v = extract_json_value(raw).expect("json");
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn extracts_bare_array() {
        let raw = "here: [ {\"idea\": \"x\"} ] done";
        let v = extract_json_value(raw).expect("json");
        assert!(v.is_array());
    }

    #[test]
    fn returns_none_without_json() {
        assert!(extract_json_value("no structured output here").is_none());
    }
}
