use std::fmt;

use serde::{Deserialize, Serialize};

use crate::session::{SessionId, SessionPhase};

pub(crate) const MEMORY_STORE_NAME: &str = "memory";

/// The six cognitive memory types from the cognitive psychology model.
///
/// This replaces the old ad-hoc `MemoryScope` with the scientifically-grounded
/// memory taxonomy used by `amplihack-memory-lib`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CognitiveMemoryType {
    /// Transient sensory buffer — raw observations with short TTL.
    Sensory,
    /// Active task context — ephemeral slots bound to the current task.
    Working,
    /// Event records — session summaries, experiences, temporal sequences.
    Episodic,
    /// Factual knowledge — decisions, project context, domain facts.
    Semantic,
    /// Skill and process knowledge — benchmarks, procedures, how-to.
    Procedural,
    /// Future intentions — goals, triggers, deferred plans.
    Prospective,
}

impl fmt::Display for CognitiveMemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sensory => write!(f, "sensory"),
            Self::Working => write!(f, "working"),
            Self::Episodic => write!(f, "episodic"),
            Self::Semantic => write!(f, "semantic"),
            Self::Procedural => write!(f, "procedural"),
            Self::Prospective => write!(f, "prospective"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub key: String,
    pub memory_type: CognitiveMemoryType,
    pub value: String,
    pub session_id: SessionId,
    pub recorded_in: SessionPhase,
}
