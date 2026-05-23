//! Cognitive memory writers for meeting summaries and handoffs.

use tracing::{debug, warn};

use crate::cognitive_memory::CognitiveMemoryOps;

use super::super::types::{ConversationMessage, HandoffActionItem};

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
                    crate::meeting_backend::types::Role::User => "operator",
                    crate::meeting_backend::types::Role::Assistant => "simard",
                    crate::meeting_backend::types::Role::System => "system",
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
//
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cognitive_memory::CognitiveMemoryOps;
    use crate::error::{SimardError, SimardResult};
    use crate::meeting_backend::types::{ConversationMessage, HandoffActionItem, Role};
    use crate::memory_cognitive::{
        CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
        CognitiveWorkingSlot,
    };
    use std::sync::Mutex;

    struct MockBridge {
        episodes: Mutex<Vec<String>>,
        facts: Mutex<Vec<(String, String)>>,
        fail_episodes: bool,
        fail_facts: bool,
    }

    impl MockBridge {
        fn new() -> Self {
            Self {
                episodes: Mutex::new(Vec::new()),
                facts: Mutex::new(Vec::new()),
                fail_episodes: false,
                fail_facts: false,
            }
        }
        fn failing_episodes() -> Self {
            Self {
                fail_episodes: true,
                ..Self::new()
            }
        }
        fn failing_facts() -> Self {
            Self {
                fail_facts: true,
                ..Self::new()
            }
        }
        fn failing_all() -> Self {
            Self {
                fail_episodes: true,
                fail_facts: true,
                ..Self::new()
            }
        }
    }

    impl CognitiveMemoryOps for MockBridge {
        fn record_sensory(&self, _: &str, _: &str, _: u64) -> SimardResult<String> {
            Ok("id".to_string())
        }
        fn prune_expired_sensory(&self) -> SimardResult<usize> {
            Ok(0)
        }
        fn push_working(&self, _: &str, _: &str, _: &str, _: f64) -> SimardResult<String> {
            Ok("id".to_string())
        }
        fn get_working(&self, _: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
            Ok(vec![])
        }
        fn clear_working(&self, _: &str) -> SimardResult<usize> {
            Ok(0)
        }
        fn store_episode(
            &self,
            content: &str,
            _source_label: &str,
            _metadata: Option<&serde_json::Value>,
        ) -> SimardResult<String> {
            if self.fail_episodes {
                return Err(SimardError::ActionExecutionFailed {
                    action: "mock-episode".to_string(),
                    reason: "injected failure".to_string(),
                });
            }
            self.episodes.lock().unwrap().push(content.to_string());
            Ok("ep-id".to_string())
        }
        fn consolidate_episodes(&self, _: u32) -> SimardResult<Option<String>> {
            Ok(None)
        }
        fn store_fact(
            &self,
            concept: &str,
            content: &str,
            _confidence: f64,
            _tags: &[String],
            _source_id: &str,
        ) -> SimardResult<String> {
            if self.fail_facts {
                return Err(SimardError::ActionExecutionFailed {
                    action: "mock-fact".to_string(),
                    reason: "injected failure".to_string(),
                });
            }
            self.facts
                .lock()
                .unwrap()
                .push((concept.to_string(), content.to_string()));
            Ok("fact-id".to_string())
        }
        fn search_facts(&self, _: &str, _: u32, _: f64) -> SimardResult<Vec<CognitiveFact>> {
            Ok(vec![])
        }
        fn store_procedure(&self, _: &str, _: &[String], _: &[String]) -> SimardResult<String> {
            Ok("id".to_string())
        }
        fn recall_procedure(&self, _: &str, _: u32) -> SimardResult<Vec<CognitiveProcedure>> {
            Ok(vec![])
        }
        fn store_prospective(&self, _: &str, _: &str, _: &str, _: i64) -> SimardResult<String> {
            Ok("id".to_string())
        }
        fn check_triggers(&self, _: &str) -> SimardResult<Vec<CognitiveProspective>> {
            Ok(vec![])
        }
        fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
            Ok(CognitiveStatistics {
                sensory_count: 0,
                working_count: 0,
                episodic_count: 0,
                semantic_count: 0,
                procedural_count: 0,
                prospective_count: 0,
            })
        }
    }

    fn msg(role: Role, content: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            content: content.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn stores_transcript_episode_and_summary_fact() {
        let bridge = MockBridge::new();
        let messages = vec![msg(Role::User, "Hello"), msg(Role::Assistant, "Hi there")];
        store_cognitive_memory(&bridge, "standup", "We discussed standup.", &messages);

        let episodes = bridge.episodes.lock().unwrap();
        assert_eq!(episodes.len(), 1);
        assert!(episodes[0].contains("standup"));
        assert!(episodes[0].contains("operator: Hello"));

        let facts = bridge.facts.lock().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].0, "meeting:standup");
        assert!(facts[0].1.contains("discussed standup"));
    }

    #[test]
    fn skips_episode_when_messages_empty() {
        let bridge = MockBridge::new();
        store_cognitive_memory(&bridge, "empty", "A summary", &[]);

        let episodes = bridge.episodes.lock().unwrap();
        assert!(
            episodes.is_empty(),
            "should not store episode for empty messages"
        );

        let facts = bridge.facts.lock().unwrap();
        assert_eq!(facts.len(), 1, "summary fact should still be stored");
    }

    #[test]
    fn skips_fact_when_summary_empty() {
        let bridge = MockBridge::new();
        let messages = vec![msg(Role::User, "One message")];
        store_cognitive_memory(&bridge, "topic", "", &messages);

        let episodes = bridge.episodes.lock().unwrap();
        assert_eq!(episodes.len(), 1);

        let facts = bridge.facts.lock().unwrap();
        assert!(
            facts.is_empty(),
            "empty summary should not be stored as fact"
        );
    }

    #[test]
    fn episode_failure_does_not_panic() {
        let bridge = MockBridge::failing_episodes();
        let messages = vec![msg(Role::User, "hello")];
        store_cognitive_memory(&bridge, "topic", "summary", &messages);
    }

    #[test]
    fn fact_failure_does_not_panic() {
        let bridge = MockBridge::failing_facts();
        store_cognitive_memory(&bridge, "topic", "summary", &[]);
    }

    #[test]
    fn enriched_stores_action_items_episode() {
        let bridge = MockBridge::new();
        let messages = vec![msg(Role::User, "content")];
        let actions = vec![HandoffActionItem {
            description: "Deploy service".to_string(),
            assignee: Some("Alice".to_string()),
            deadline: Some("by friday".to_string()),
            linked_goal: Some("perf".to_string()),
            priority: None,
        }];
        let decisions = vec!["Use Rust".to_string()];

        store_enriched_cognitive_memory(
            &bridge,
            "retro",
            "Retro summary",
            &messages,
            &actions,
            &decisions,
        );

        let episodes = bridge.episodes.lock().unwrap();
        assert_eq!(episodes.len(), 3);

        let ai_ep = episodes
            .iter()
            .find(|e| e.contains("Action items"))
            .unwrap();
        assert!(ai_ep.contains("Deploy service"));
        assert!(ai_ep.contains("[assignee: Alice]"));
        assert!(ai_ep.contains("[deadline: by friday]"));
        assert!(ai_ep.contains("[goal: perf]"));

        let dec_ep = episodes.iter().find(|e| e.contains("Decisions")).unwrap();
        assert!(dec_ep.contains("Use Rust"));
    }

    #[test]
    fn enriched_skips_empty_action_items() {
        let bridge = MockBridge::new();
        let messages = vec![msg(Role::User, "content")];
        store_enriched_cognitive_memory(
            &bridge,
            "topic",
            "summary",
            &messages,
            &[],
            &["A decision".to_string()],
        );

        let episodes = bridge.episodes.lock().unwrap();
        assert_eq!(episodes.len(), 2);
        assert!(!episodes.iter().any(|e| e.contains("Action items")));
    }

    #[test]
    fn enriched_skips_empty_decisions() {
        let bridge = MockBridge::new();
        let messages = vec![msg(Role::User, "content")];
        let actions = vec![HandoffActionItem {
            description: "A task".to_string(),
            assignee: None,
            deadline: None,
            linked_goal: None,
            priority: None,
        }];
        store_enriched_cognitive_memory(&bridge, "topic", "summary", &messages, &actions, &[]);

        let episodes = bridge.episodes.lock().unwrap();
        assert_eq!(episodes.len(), 2);
        assert!(!episodes.iter().any(|e| e.contains("Decisions")));
    }

    #[test]
    fn enriched_all_failures_do_not_panic() {
        let bridge = MockBridge::failing_all();
        let messages = vec![msg(Role::User, "content")];
        let actions = vec![HandoffActionItem {
            description: "A task".to_string(),
            assignee: None,
            deadline: None,
            linked_goal: None,
            priority: None,
        }];
        let decisions = vec!["Some decision".to_string()];
        store_enriched_cognitive_memory(
            &bridge, "topic", "summary", &messages, &actions, &decisions,
        );
    }
}
