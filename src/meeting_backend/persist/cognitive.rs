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
        facts: Mutex<Vec<String>>,
        should_fail: bool,
    }

    impl MockBridge {
        fn new() -> Self {
            Self {
                episodes: Mutex::new(Vec::new()),
                facts: Mutex::new(Vec::new()),
                should_fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                episodes: Mutex::new(Vec::new()),
                facts: Mutex::new(Vec::new()),
                should_fail: true,
            }
        }
    }

    impl CognitiveMemoryOps for MockBridge {
        fn record_sensory(&self, _: &str, _: &str, _: u64) -> SimardResult<String> {
            Ok("ok".into())
        }
        fn prune_expired_sensory(&self) -> SimardResult<usize> {
            Ok(0)
        }
        fn push_working(&self, _: &str, _: &str, _: &str, _: f64) -> SimardResult<String> {
            Ok("ok".into())
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
            if self.should_fail {
                return Err(SimardError::ActionExecutionFailed {
                    action: "store-episode".into(),
                    reason: "mock failure".into(),
                });
            }
            self.episodes.lock().unwrap().push(content.to_string());
            Ok("ep-id".into())
        }
        fn consolidate_episodes(&self, _: u32) -> SimardResult<Option<String>> {
            Ok(None)
        }
        fn store_fact(
            &self,
            concept: &str,
            content: &str,
            _: f64,
            _: &[String],
            _: &str,
        ) -> SimardResult<String> {
            if self.should_fail {
                return Err(SimardError::ActionExecutionFailed {
                    action: "store-fact".into(),
                    reason: "mock failure".into(),
                });
            }
            self.facts
                .lock()
                .unwrap()
                .push(format!("{concept}:{content}"));
            Ok("fact-id".into())
        }
        fn search_facts(&self, _: &str, _: u32, _: f64) -> SimardResult<Vec<CognitiveFact>> {
            Ok(vec![])
        }
        fn store_procedure(&self, _: &str, _: &[String], _: &[String]) -> SimardResult<String> {
            Ok("ok".into())
        }
        fn recall_procedure(&self, _: &str, _: u32) -> SimardResult<Vec<CognitiveProcedure>> {
            Ok(vec![])
        }
        fn store_prospective(&self, _: &str, _: &str, _: &str, _: i64) -> SimardResult<String> {
            Ok("ok".into())
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

    fn sample_messages() -> Vec<ConversationMessage> {
        vec![
            ConversationMessage {
                role: Role::User,
                content: "Let's discuss testing.".into(),
                timestamp: "2026-01-15T10:00:00Z".into(),
            },
            ConversationMessage {
                role: Role::Assistant,
                content: "Agreed, TDD is important.".into(),
                timestamp: "2026-01-15T10:01:00Z".into(),
            },
        ]
    }

    #[test]
    fn store_cognitive_memory_stores_episode_and_fact() {
        let bridge = MockBridge::new();
        store_cognitive_memory(&bridge, "Sprint", "We decided on TDD", &sample_messages());
        let episodes = bridge.episodes.lock().unwrap();
        assert_eq!(episodes.len(), 1);
        assert!(episodes[0].contains("Sprint"));
        let facts = bridge.facts.lock().unwrap();
        assert_eq!(facts.len(), 1);
        assert!(facts[0].contains("TDD"));
    }

    #[test]
    fn store_cognitive_memory_empty_messages_skips_episode() {
        let bridge = MockBridge::new();
        store_cognitive_memory(&bridge, "empty", "Summary only", &[]);
        assert!(bridge.episodes.lock().unwrap().is_empty());
    }

    #[test]
    fn store_cognitive_memory_empty_summary_skips_fact() {
        let bridge = MockBridge::new();
        store_cognitive_memory(&bridge, "topic", "", &sample_messages());
        assert!(bridge.facts.lock().unwrap().is_empty());
    }

    #[test]
    fn store_cognitive_memory_bridge_error_does_not_panic() {
        let bridge = MockBridge::failing();
        store_cognitive_memory(&bridge, "topic", "summary", &sample_messages());
    }

    #[test]
    fn store_enriched_stores_action_items_episode() {
        let bridge = MockBridge::new();
        let items = vec![HandoffActionItem {
            description: "Deploy to staging".into(),
            assignee: Some("Bob".into()),
            deadline: Some("by friday".into()),
            linked_goal: None,
            priority: None,
        }];
        store_enriched_cognitive_memory(
            &bridge,
            "Sprint",
            "Summary",
            &sample_messages(),
            &items,
            &[],
        );
        let episodes = bridge.episodes.lock().unwrap();
        assert_eq!(episodes.len(), 2);
        assert!(episodes[1].contains("Deploy to staging"));
        assert!(episodes[1].contains("[assignee: Bob]"));
    }

    #[test]
    fn store_enriched_stores_decisions_episode() {
        let bridge = MockBridge::new();
        let decisions = vec!["Adopt TDD".to_string(), "Use Rust".to_string()];
        store_enriched_cognitive_memory(
            &bridge,
            "retro",
            "Good session",
            &sample_messages(),
            &[],
            &decisions,
        );
        let episodes = bridge.episodes.lock().unwrap();
        assert_eq!(episodes.len(), 2);
        assert!(episodes[1].contains("Adopt TDD"));
    }

    #[test]
    fn store_enriched_empty_extras_only_stores_base() {
        let bridge = MockBridge::new();
        store_enriched_cognitive_memory(&bridge, "topic", "summary", &sample_messages(), &[], &[]);
        assert_eq!(bridge.episodes.lock().unwrap().len(), 1);
    }

    #[test]
    fn store_enriched_action_fields_all_present() {
        let bridge = MockBridge::new();
        let items = vec![HandoffActionItem {
            description: "Write docs".into(),
            assignee: Some("Charlie".into()),
            deadline: Some("next sprint".into()),
            linked_goal: Some("docs-goal".into()),
            priority: Some(2),
        }];
        store_enriched_cognitive_memory(&bridge, "T", "S", &sample_messages(), &items, &[]);
        let episodes = bridge.episodes.lock().unwrap();
        let ep = &episodes[1];
        assert!(ep.contains("[assignee: Charlie]"));
        assert!(ep.contains("[deadline: next sprint]"));
        assert!(ep.contains("[goal: docs-goal]"));
    }

    #[test]
    fn store_enriched_bridge_error_does_not_panic() {
        let bridge = MockBridge::failing();
        store_enriched_cognitive_memory(
            &bridge,
            "t",
            "s",
            &sample_messages(),
            &[HandoffActionItem {
                description: "x".into(),
                assignee: None,
                deadline: None,
                linked_goal: None,
                priority: None,
            }],
            &["d".into()],
        );
    }
}
