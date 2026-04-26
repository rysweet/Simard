//! Persistence for meeting transcripts and handoff artifacts.

use std::path::PathBuf;

use tracing::{debug, info, warn};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::{
    ActionItem, MeetingDecision, MeetingHandoff, OpenQuestion, default_handoff_dir,
    write_meeting_handoff,
};

use super::types::{ConversationMessage, HandoffActionItem, MeetingTranscript};

/// Maximum length for a sanitized filename component.
pub(super) const MAX_FILENAME_LEN: usize = 128;

/// Sanitize a string for safe use as a filesystem name.
///
/// Strips path separators, `..`, null bytes, and control characters. Replaces
/// spaces and unsafe characters with underscores and caps length.
pub fn sanitize_filename(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' => '_',
            _ => c,
        })
        .collect();
    // Remove .. sequences
    let sanitized = sanitized.replace("..", "");
    // Trim leading/trailing underscores/dots
    let sanitized = sanitized
        .trim_matches(|c: char| c == '_' || c == '.')
        .to_string();
    if sanitized.is_empty() {
        return "meeting".to_string();
    }
    if sanitized.len() > MAX_FILENAME_LEN {
        sanitized[..MAX_FILENAME_LEN].to_string()
    } else {
        sanitized
    }
}

/// Directory for meeting transcripts: `~/.simard/meetings/`.
fn meetings_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".simard/meetings")
}

/// Write a JSON transcript to `~/.simard/meetings/{timestamp}_{topic}.json`.
///
/// Creates the directory if it doesn't exist. Sets file permissions to 0o600
/// on Unix.
pub fn write_transcript(transcript: &MeetingTranscript) -> SimardResult<PathBuf> {
    let dir = meetings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| SimardError::ActionExecutionFailed {
        action: "create-meetings-dir".to_string(),
        reason: e.to_string(),
    })?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let safe_topic = sanitize_filename(&transcript.topic);
    let filename = format!("{timestamp}_{safe_topic}.json");
    let path = dir.join(&filename);

    let json = serde_json::to_string_pretty(transcript).map_err(|e| {
        SimardError::ActionExecutionFailed {
            action: "serialize-transcript".to_string(),
            reason: e.to_string(),
        }
    })?;

    std::fs::write(&path, &json).map_err(|e| SimardError::ActionExecutionFailed {
        action: "write-transcript".to_string(),
        reason: e.to_string(),
    })?;

    // Set permissions to 0o600 on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            warn!("Failed to set transcript permissions: {e}");
        }
    }

    info!(path = %path.display(), "Meeting transcript written");
    Ok(path)
}

/// Write an auto-save transcript to `~/.simard/meetings/_autosave_{topic}.json`.
///
/// Overwrites the same file each turn. The final `write_transcript()` on
/// `/close` writes the canonical timestamped file.
pub fn write_auto_save(transcript: &MeetingTranscript) -> SimardResult<PathBuf> {
    let dir = meetings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| SimardError::ActionExecutionFailed {
        action: "create-meetings-dir".to_string(),
        reason: e.to_string(),
    })?;

    let safe_topic = sanitize_filename(&transcript.topic);
    let filename = format!("_autosave_{safe_topic}.json");
    let path = dir.join(&filename);

    let json = serde_json::to_string_pretty(transcript).map_err(|e| {
        SimardError::ActionExecutionFailed {
            action: "serialize-autosave".to_string(),
            reason: e.to_string(),
        }
    })?;

    std::fs::write(&path, &json).map_err(|e| SimardError::ActionExecutionFailed {
        action: "write-autosave".to_string(),
        reason: e.to_string(),
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            warn!("Failed to set autosave permissions: {e}");
        }
    }

    debug!(path = %path.display(), "Auto-save transcript written");
    Ok(path)
}

/// Write a `MeetingHandoff` artifact for OODA integration.
///
/// Serializes the full structured data extracted from the meeting session —
/// decisions, action items, open questions, participants, and themes — into the
/// handoff JSON. Falls back to sensible defaults when fields are empty.
pub fn write_handoff(
    topic: &str,
    summary: &str,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[String],
) -> SimardResult<()> {
    let started_at = messages
        .first()
        .map(|m| m.timestamp.clone())
        .unwrap_or_default();
    let closed_at = chrono::Utc::now().to_rfc3339();

    let duration_secs = chrono::DateTime::parse_from_rfc3339(&started_at)
        .ok()
        .map(|start| {
            chrono::Utc::now()
                .signed_duration_since(start)
                .num_seconds()
                .max(0) as u64
        });

    // Convert backend HandoffActionItems to facilitator ActionItems for the handoff.
    let facilitator_actions: Vec<crate::meeting_facilitator::ActionItem> = action_items
        .iter()
        .map(|a| ActionItem {
            description: a.description.clone(),
            owner: a
                .assignee
                .clone()
                .unwrap_or_else(|| "unassigned".to_string()),
            priority: 0,
            due_description: a.deadline.clone(),
        })
        .collect();

    // Convert decision strings to MeetingDecision structs, extracting
    // rationale context from surrounding messages when available.
    let facilitator_decisions: Vec<MeetingDecision> = decisions
        .iter()
        .map(|d| {
            let rationale = extract_decision_rationale(d, messages);
            MeetingDecision {
                description: d.clone(),
                rationale,
                participants: extract_decision_participants(d, messages),
            }
        })
        .collect();

    // Extract open questions from message content.
    let open_questions = extract_open_questions(messages);

    // Collect unique participants from messages.
    let mut participants: Vec<String> = Vec::new();
    for msg in messages {
        let role_name = match msg.role {
            super::types::Role::User => "operator",
            super::types::Role::Assistant => "simard",
            super::types::Role::System => "system",
        };
        let s = role_name.to_string();
        if !participants.contains(&s) {
            participants.push(s);
        }
    }
    // Also include action item assignees.
    for a in action_items {
        if let Some(ref assignee) = a.assignee
            && !participants.contains(assignee)
        {
            participants.push(assignee.clone());
        }
    }

    // Extract themes from meeting content.
    let themes = extract_themes(messages);

    let handoff = MeetingHandoff {
        topic: topic.to_string(),
        started_at,
        closed_at,
        decisions: facilitator_decisions,
        action_items: facilitator_actions,
        open_questions,
        processed: false,
        duration_secs,
        transcript: vec![summary.to_string()],
        participants,
        themes,
    };

    let dir = default_handoff_dir();
    write_meeting_handoff(&dir, &handoff)?;
    info!("Meeting handoff artifact written");
    Ok(())
}

/// Store the meeting as an episodic memory via the cognitive bridge.
pub fn store_cognitive_memory(
    bridge: &dyn CognitiveMemoryOps,
    topic: &str,
    summary: &str,
    messages: &[ConversationMessage],
) {
    // Store full transcript as episodic memory
    if !messages.is_empty() {
        let transcript_text: String = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    super::types::Role::User => "operator",
                    super::types::Role::Assistant => "simard",
                    super::types::Role::System => "system",
                };
                format!("{}: {}", role, m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let episode_content = format!(
            "Meeting transcript — topic: {topic}\n\n{transcript_text}\n\nSummary: {summary}"
        );
        if let Err(e) = bridge.store_episode(
            &episode_content,
            "meeting-backend-transcript",
            Some(&serde_json::json!({
                "topic": topic,
                "type": "transcript",
                "message_count": messages.len(),
            })),
        ) {
            warn!("Failed to persist meeting episode: {e}");
        } else {
            debug!("Meeting episode stored");
        }
    }

    // Store summary as a semantic fact
    if !summary.is_empty() {
        let tags = vec![
            "meeting".to_string(),
            "summary".to_string(),
            topic.to_string(),
        ];
        if let Err(e) = bridge.store_fact(
            &format!("meeting:{topic}"),
            summary,
            0.85,
            &tags,
            "meeting-backend",
        ) {
            warn!("Failed to persist meeting summary fact: {e}");
        } else {
            debug!("Meeting summary fact stored");
        }
    }
}

/// Write a markdown export of the current meeting to `~/.simard/meetings/`.
///
/// The file includes YAML frontmatter (topic, date, participants) and the
/// conversation transcript formatted as markdown.
pub fn write_markdown_export(
    topic: &str,
    started_at: &str,
    messages: &[ConversationMessage],
) -> SimardResult<PathBuf> {
    let dir = meetings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| SimardError::ActionExecutionFailed {
        action: "create-meetings-dir".to_string(),
        reason: e.to_string(),
    })?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let safe_topic = sanitize_filename(topic);
    let filename = format!("{timestamp}_{safe_topic}.md");
    let path = dir.join(&filename);

    let mut md = String::with_capacity(4096);
    // YAML frontmatter
    md.push_str("---\n");
    md.push_str(&format!("topic: \"{}\"\n", topic.replace('"', "\\\"")));
    md.push_str(&format!("date: \"{started_at}\"\n"));
    // Collect unique participants from messages
    let mut participants: Vec<String> = Vec::new();
    for msg in messages {
        let role_name = match msg.role {
            super::types::Role::User => "operator",
            super::types::Role::Assistant => "simard",
            super::types::Role::System => "system",
        };
        let s = role_name.to_string();
        if !participants.contains(&s) {
            participants.push(s);
        }
    }
    md.push_str("participants:\n");
    for p in &participants {
        md.push_str(&format!("  - \"{p}\"\n"));
    }
    md.push_str("---\n\n");

    // Title and transcript
    md.push_str(&format!("# Meeting: {topic}\n\n"));
    md.push_str(&format!("**Date:** {started_at}\n\n"));

    if messages.is_empty() {
        md.push_str("_No messages recorded._\n");
    } else {
        md.push_str("## Transcript\n\n");
        for msg in messages {
            let role_label = match msg.role {
                super::types::Role::User => "**Operator**",
                super::types::Role::Assistant => "**Simard**",
                super::types::Role::System => "**System**",
            };
            md.push_str(&format!("{role_label}: {}\n\n", msg.content));
        }
    }

    std::fs::write(&path, &md).map_err(|e| SimardError::ActionExecutionFailed {
        action: "write-markdown-export".to_string(),
        reason: e.to_string(),
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            warn!("Failed to set export file permissions: {e}");
        }
    }

    info!(path = %path.display(), "Meeting markdown export written");
    Ok(path)
}

// ── Action-item extraction ──────────────────────────────────────────────

/// Signal phrases that indicate an action item in natural conversation.
const ACTION_SIGNALS: &[&str] = &[
    "action item:",
    "todo:",
    "to-do:",
    "task:",
    "ai:",
    " will ",
    " should ",
    " needs to ",
    " need to ",
    " must ",
    "let's ",
    "let\u{2019}s ",
    "follow up",
    "follow-up",
];

/// Deadline keywords that suggest a time constraint.
const DEADLINE_SIGNALS: &[&str] = &[
    "by friday",
    "by monday",
    "by tuesday",
    "by wednesday",
    "by thursday",
    "by saturday",
    "by sunday",
    "by tomorrow",
    "by end of day",
    "by eod",
    "by end of week",
    "by eow",
    "by next week",
    "by next sprint",
    "next sprint",
    "this week",
    "this sprint",
    "asap",
    "immediately",
];

/// Extract structured action items from a conversation transcript.
///
/// Uses heuristic signal phrases to identify action items from both user and
/// assistant messages. This is a best-effort extraction — the LLM summary
/// provides the authoritative narrative.
pub fn extract_action_items(messages: &[ConversationMessage]) -> Vec<HandoffActionItem> {
    let mut items = Vec::new();
    for msg in messages {
        let lower = msg.content.to_lowercase();
        let is_action = ACTION_SIGNALS.iter().any(|s| lower.contains(s));
        if !is_action {
            continue;
        }

        for sentence in split_sentences(&msg.content) {
            let sent_lower = sentence.to_lowercase();
            let has_signal = ACTION_SIGNALS.iter().any(|s| sent_lower.contains(s));
            if !has_signal {
                continue;
            }

            let description = clean_action_description(&sentence);
            if description.len() < 5 {
                continue;
            }

            let assignee = extract_assignee(&sentence);
            let deadline = extract_deadline(&sent_lower);

            items.push(HandoffActionItem {
                description,
                assignee,
                deadline,
                linked_goal: None,
                priority: None,
            });
        }
    }
    items
}

/// Try to extract an assignee from a sentence.
pub(super) fn extract_assignee(sentence: &str) -> Option<String> {
    let verbs = [" will ", " should ", " needs to ", " need to ", " must "];
    for verb in &verbs {
        if let Some(idx) = sentence.to_lowercase().find(verb) {
            let prefix = sentence[..idx].trim();
            if let Some(name) = prefix.split_whitespace().last() {
                let clean = name.trim_matches(|c: char| !c.is_alphanumeric());
                if !clean.is_empty()
                    && clean.len() >= 2
                    && clean.chars().next().is_some_and(|c| c.is_uppercase())
                {
                    return Some(clean.to_string());
                }
            }
        }
    }
    if let Some(idx) = sentence.to_lowercase().find("assigned to ") {
        let after = &sentence[idx + "assigned to ".len()..];
        if let Some(name) = after.split_whitespace().next() {
            let clean = name.trim_matches(|c: char| !c.is_alphanumeric());
            if !clean.is_empty() && clean.len() >= 2 {
                return Some(clean.to_string());
            }
        }
    }
    None
}

/// Extract a deadline phrase if present.
pub(super) fn extract_deadline(lower_sentence: &str) -> Option<String> {
    for signal in DEADLINE_SIGNALS {
        if lower_sentence.contains(signal) {
            return Some(signal.trim().to_string());
        }
    }
    None
}

/// Clean up an action item description — strip leading signal labels.
pub(super) fn clean_action_description(sentence: &str) -> String {
    let mut s = sentence.trim().to_string();
    let prefixes = [
        "action item:",
        "Action item:",
        "ACTION ITEM:",
        "todo:",
        "TODO:",
        "To-do:",
        "to-do:",
        "task:",
        "Task:",
        "TASK:",
        "ai:",
        "AI:",
    ];
    for prefix in &prefixes {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim().to_string();
            break;
        }
    }
    s
}

/// Split text into sentences (simple heuristic).
pub(super) fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if ch == '.' || ch == '!' || ch == '?' || ch == '\n' {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
            current.clear();
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        sentences.push(trimmed);
    }
    sentences
}

// ── Goal linkage ────────────────────────────────────────────────────────

/// Match extracted action items against active goals by keyword overlap.
pub fn link_action_items_to_goals(
    items: &mut [HandoffActionItem],
    goal_titles: &[(String, String)],
) {
    for item in items.iter_mut() {
        let item_words: Vec<String> = item
            .description
            .to_lowercase()
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .map(|w| w.to_string())
            .collect();

        let mut best_match: Option<(&str, usize)> = None;

        for (slug, title) in goal_titles {
            let goal_words: Vec<String> = title
                .to_lowercase()
                .split_whitespace()
                .filter(|w| w.len() > 2)
                .map(|w| w.to_string())
                .collect();

            let overlap = item_words.iter().filter(|w| goal_words.contains(w)).count();

            let threshold = if goal_words.len() <= 2 { 1 } else { 2 };
            if overlap >= threshold && (best_match.is_none() || overlap > best_match.unwrap().1) {
                best_match = Some((slug.as_str(), overlap));
            }
        }

        if let Some((slug, _)) = best_match {
            item.linked_goal = Some(slug.to_string());
        }
    }
}

/// Extract decision statements from transcript messages.
pub fn extract_decisions(messages: &[ConversationMessage]) -> Vec<String> {
    let decision_signals = [
        "decision:",
        "decided:",
        "we decided",
        "we agreed",
        "the decision is",
        "agreed to",
        "conclusion:",
    ];
    let mut decisions = Vec::new();
    for msg in messages {
        for sentence in split_sentences(&msg.content) {
            let lower = sentence.to_lowercase();
            if decision_signals.iter().any(|s| lower.contains(s)) {
                let clean = sentence.trim().to_string();
                if clean.len() >= 5 && !decisions.contains(&clean) {
                    decisions.push(clean);
                }
            }
        }
    }
    decisions
}

/// Extract open questions from transcript messages.
///
/// Looks for explicit question markers (`OPEN:`, `QUESTION:`, `TBD:`, etc.) and
/// genuine questions (sentences containing `?` that aren't too short/rhetorical).
pub fn extract_open_questions(messages: &[ConversationMessage]) -> Vec<OpenQuestion> {
    let explicit_prefixes = ["open:", "question:", "tbd:", "unresolved:"];
    let mut questions: Vec<OpenQuestion> = Vec::new();

    for msg in messages {
        for sentence in split_sentences(&msg.content) {
            let lower = sentence.trim().to_lowercase();

            // Check explicit markers first.
            let is_explicit = explicit_prefixes.iter().any(|p| lower.starts_with(p));
            if is_explicit {
                let text = sentence.trim().to_string();
                if !questions.iter().any(|q| q.text == text) {
                    questions.push(OpenQuestion {
                        text,
                        explicit: true,
                    });
                }
                continue;
            }

            // Genuine questions: contains `?`, long enough to not be rhetorical.
            if sentence.contains('?') && sentence.trim().len() >= 15 {
                let text = sentence.trim().to_string();
                if !questions.iter().any(|q| q.text == text) {
                    questions.push(OpenQuestion {
                        text,
                        explicit: false,
                    });
                }
            }
        }
    }
    questions
}

/// Extract high-level themes from transcript messages by frequency analysis.
///
/// Identifies recurring topic keywords (nouns/phrases that appear across multiple
/// messages) and returns them as theme strings.
pub fn extract_themes(messages: &[ConversationMessage]) -> Vec<String> {
    use std::collections::HashMap;

    // Common stop words to ignore.
    const STOP_WORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with", "by",
        "is", "it", "that", "this", "was", "are", "be", "has", "have", "had", "not", "we", "they",
        "you", "will", "can", "should", "would", "could", "do", "does", "did", "from", "about",
        "into", "out", "if", "then", "so", "up", "one", "all", "been", "just", "also", "than",
        "like", "more", "some", "what", "when", "how", "who", "which", "there", "their", "our",
        "i", "my", "me", "your", "its",
    ];

    let mut word_freq: HashMap<String, usize> = HashMap::new();
    for msg in messages {
        // Only count user and assistant messages, skip system.
        if matches!(msg.role, super::types::Role::System) {
            continue;
        }
        let words: Vec<String> = msg
            .content
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric() && c != '-')
            .filter(|w| w.len() > 3 && !STOP_WORDS.contains(w))
            .map(String::from)
            .collect();
        // Count unique words per message to avoid single-message spam.
        let mut seen = std::collections::HashSet::new();
        for w in words {
            if seen.insert(w.clone()) {
                *word_freq.entry(w).or_insert(0) += 1;
            }
        }
    }

    // Themes are words appearing in at least 2 messages.
    let min_freq = 2;
    let mut themes: Vec<(String, usize)> = word_freq
        .into_iter()
        .filter(|(_, count)| *count >= min_freq)
        .collect();
    themes.sort_by_key(|a| std::cmp::Reverse(a.1));
    themes.truncate(10);
    themes.into_iter().map(|(word, _)| word).collect()
}

/// Extract rationale context for a decision from surrounding messages.
///
/// Looks for the message containing the decision text and checks the preceding
/// message for context that explains *why* the decision was made.
fn extract_decision_rationale(decision: &str, messages: &[ConversationMessage]) -> String {
    let decision_lower = decision.to_lowercase();
    for (i, msg) in messages.iter().enumerate() {
        if msg.content.to_lowercase().contains(&decision_lower) {
            // Check the preceding message for rationale context.
            if i > 0 {
                let prev = &messages[i - 1].content;
                // Truncate long rationale to keep handoff concise.
                if prev.len() > 300 {
                    return format!("{}…", &prev[..297]);
                }
                return prev.clone();
            }
        }
    }
    String::new()
}

/// Public wrapper for extracting rationale — used by the backend on close.
pub fn extract_decision_rationale_pub(decision: &str, messages: &[ConversationMessage]) -> String {
    extract_decision_rationale(decision, messages)
}

/// Extract participant roles involved in a decision from the message that
/// contains it and the preceding message.
fn extract_decision_participants(decision: &str, messages: &[ConversationMessage]) -> Vec<String> {
    let decision_lower = decision.to_lowercase();
    let mut participants = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg.content.to_lowercase().contains(&decision_lower) {
            let role = match msg.role {
                super::types::Role::User => "operator",
                super::types::Role::Assistant => "simard",
                super::types::Role::System => "system",
            };
            participants.push(role.to_string());
            // Include the role from the preceding message if it contributed.
            if i > 0 {
                let prev_role = match messages[i - 1].role {
                    super::types::Role::User => "operator",
                    super::types::Role::Assistant => "simard",
                    super::types::Role::System => "system",
                };
                if !participants.contains(&prev_role.to_string()) {
                    participants.push(prev_role.to_string());
                }
            }
            break;
        }
    }
    participants
}

/// Public wrapper for extracting decision participants — used by the backend on close.
pub fn extract_decision_participants_pub(
    decision: &str,
    messages: &[ConversationMessage],
) -> Vec<String> {
    extract_decision_participants(decision, messages)
}

/// Write a rich markdown meeting report including summary, decisions,
/// action items table, and transcript — triggered automatically on `/end`.
pub fn write_handoff_markdown_report(
    topic: &str,
    started_at: &str,
    summary: &str,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[MeetingDecision],
) -> SimardResult<PathBuf> {
    let dir = meetings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| SimardError::ActionExecutionFailed {
        action: "create-meetings-dir".to_string(),
        reason: e.to_string(),
    })?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let safe_topic = sanitize_filename(topic);
    let filename = format!("{timestamp}_{safe_topic}_report.md");
    let path = dir.join(&filename);

    let mut md = String::with_capacity(8192);

    // YAML frontmatter
    md.push_str("---\n");
    md.push_str(&format!("topic: \"{}\"\n", topic.replace('"', "\\\"")));
    md.push_str(&format!("date: \"{started_at}\"\n"));
    md.push_str("type: meeting-report\n");
    md.push_str("---\n\n");

    md.push_str(&format!("# Meeting Report: {topic}\n\n"));
    md.push_str(&format!("**Date:** {started_at}\n\n"));

    // Participants section
    let mut participants: Vec<String> = Vec::new();
    for msg in messages {
        let role_name = match msg.role {
            super::types::Role::User => "operator",
            super::types::Role::Assistant => "simard",
            super::types::Role::System => "system",
        };
        let s = role_name.to_string();
        if !participants.contains(&s) {
            participants.push(s);
        }
    }
    for a in action_items {
        if let Some(ref assignee) = a.assignee
            && !participants.contains(assignee)
        {
            participants.push(assignee.clone());
        }
    }
    if !participants.is_empty() {
        md.push_str("## Participants\n\n");
        for p in &participants {
            md.push_str(&format!("- {p}\n"));
        }
        md.push('\n');
    }

    md.push_str("## Summary\n\n");
    md.push_str(summary);
    md.push_str("\n\n");

    md.push_str("## Decisions\n\n");
    if decisions.is_empty() {
        md.push_str("_No explicit decisions recorded._\n\n");
    } else {
        for (i, d) in decisions.iter().enumerate() {
            md.push_str(&format!("{}. **{}**\n", i + 1, d.description));
            if !d.rationale.is_empty() {
                md.push_str(&format!("   - *Rationale:* {}\n", d.rationale));
            }
            if !d.participants.is_empty() {
                md.push_str(&format!("   - *By:* {}\n", d.participants.join(", ")));
            }
        }
        md.push('\n');
    }

    md.push_str("## Action Items\n\n");
    if action_items.is_empty() {
        md.push_str("_No action items extracted._\n\n");
    } else {
        md.push_str("| # | Description | Assignee | Deadline | Priority | Goal |\n");
        md.push_str("|---|-------------|----------|----------|----------|------|\n");
        for (i, item) in action_items.iter().enumerate() {
            let assignee = item.assignee.as_deref().unwrap_or("\u{2014}");
            let deadline = item.deadline.as_deref().unwrap_or("\u{2014}");
            let priority = item
                .priority
                .map(|p| p.to_string())
                .unwrap_or_else(|| "\u{2014}".to_string());
            let goal = item.linked_goal.as_deref().unwrap_or("\u{2014}");
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                i + 1,
                item.description,
                assignee,
                deadline,
                priority,
                goal
            ));
        }
        md.push('\n');
    }

    // Open questions extracted from transcript.
    let open_questions = extract_open_questions(messages);
    md.push_str("## Open Questions\n\n");
    if open_questions.is_empty() {
        md.push_str("_No open questions identified._\n\n");
    } else {
        for q in &open_questions {
            let tag = if q.explicit { " *(explicit)*" } else { "" };
            md.push_str(&format!("- {}{tag}\n", q.text));
        }
        md.push('\n');
    }

    // Themes extracted from meeting content.
    let themes = extract_themes(messages);
    md.push_str("## Themes\n\n");
    if themes.is_empty() {
        md.push_str("_No recurring themes identified._\n\n");
    } else {
        for t in &themes {
            md.push_str(&format!("- {t}\n"));
        }
        md.push('\n');
    }

    if !messages.is_empty() {
        md.push_str("## Transcript\n\n");
        for msg in messages {
            let role_label = match msg.role {
                super::types::Role::User => "**Operator**",
                super::types::Role::Assistant => "**Simard**",
                super::types::Role::System => "**System**",
            };
            md.push_str(&format!("{role_label}: {}\n\n", msg.content));
        }
    }

    std::fs::write(&path, &md).map_err(|e| SimardError::ActionExecutionFailed {
        action: "write-handoff-report".to_string(),
        reason: e.to_string(),
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            warn!("Failed to set report file permissions: {e}");
        }
    }

    info!(path = %path.display(), "Meeting handoff report written");
    Ok(path)
}

/// Store enriched meeting data (with action items) into episodic memory.
pub fn store_enriched_cognitive_memory(
    bridge: &dyn CognitiveMemoryOps,
    topic: &str,
    summary: &str,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[String],
) {
    store_cognitive_memory(bridge, topic, summary, messages);

    if !action_items.is_empty() {
        let action_text: String = action_items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let mut line = format!("{}. {}", i + 1, item.description);
                if let Some(ref who) = item.assignee {
                    line.push_str(&format!(" [assignee: {who}]"));
                }
                if let Some(ref when) = item.deadline {
                    line.push_str(&format!(" [deadline: {when}]"));
                }
                if let Some(ref goal) = item.linked_goal {
                    line.push_str(&format!(" [goal: {goal}]"));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");

        let episode = format!("Action items from meeting \"{topic}\":\n{action_text}");
        if let Err(e) = bridge.store_episode(
            &episode,
            "meeting-action-items",
            Some(&serde_json::json!({
                "topic": topic,
                "type": "action-items",
                "count": action_items.len(),
            })),
        ) {
            warn!("Failed to persist meeting action-items episode: {e}");
        } else {
            debug!("Meeting action-items episode stored");
        }
    }

    if !decisions.is_empty() {
        let decision_text = decisions
            .iter()
            .enumerate()
            .map(|(i, d)| format!("{}. {}", i + 1, d))
            .collect::<Vec<_>>()
            .join("\n");

        let episode = format!("Decisions from meeting \"{topic}\":\n{decision_text}");
        if let Err(e) = bridge.store_episode(
            &episode,
            "meeting-decisions",
            Some(&serde_json::json!({
                "topic": topic,
                "type": "decisions",
                "count": decisions.len(),
            })),
        ) {
            warn!("Failed to persist meeting decisions episode: {e}");
        } else {
            debug!("Meeting decisions episode stored");
        }
    }
}

/// Meeting template content (agenda and prompts) for common meeting types.
pub struct MeetingTemplate {
    pub name: &'static str,
    pub description: &'static str,
    pub agenda: &'static str,
}

/// All available meeting templates.
pub const TEMPLATES: &[MeetingTemplate] = &[
    MeetingTemplate {
        name: "standup",
        description: "Daily standup / sync",
        agenda: "\
## Daily Standup

1. **What did you accomplish since last standup?**
2. **What are you working on today?**
3. **Any blockers or impediments?**

_Tip: Keep updates brief — flag blockers for offline follow-up._",
    },
    MeetingTemplate {
        name: "1on1",
        description: "One-on-one check-in",
        agenda: "\
## 1:1 Check-in

1. **How are things going?** (personal/professional)
2. **Progress on current goals**
3. **Feedback** — anything to share in either direction?
4. **Growth & development** — skills, interests, opportunities
5. **Action items from last time**

_Tip: This is their meeting — let them drive the agenda._",
    },
    MeetingTemplate {
        name: "retro",
        description: "Sprint retrospective",
        agenda: "\
## Retrospective

1. **What went well?** 🟢
2. **What didn't go well?** 🔴
3. **What can we improve?** 🔧
4. **Action items** — concrete, assigned, time-boxed

_Tip: Celebrate wins before diving into problems._",
    },
    MeetingTemplate {
        name: "planning",
        description: "Sprint / iteration planning",
        agenda: "\
## Planning Session

1. **Review previous sprint** — what carried over and why?
2. **Capacity check** — who's available, any PTO or conflicts?
3. **Backlog review** — prioritize items for this sprint
4. **Estimation** — size and assign selected items
5. **Sprint goal** — one sentence capturing the sprint's purpose
6. **Risks & dependencies** — anything that could block progress?

_Tip: Timebox estimation discussions — if it takes >2 min, take it offline._",
    },
];

/// Look up a template by name. Returns `None` if not found.
pub fn find_template(name: &str) -> Option<&'static MeetingTemplate> {
    TEMPLATES.iter().find(|t| t.name.eq_ignore_ascii_case(name))
}
