use serde::{Deserialize, Serialize};

use crate::session::{SessionId, SessionPhase};

pub(crate) const MEMORY_STORE_NAME: &str = "memory";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryScope {
    SessionScratch,
    SessionSummary,
    Decision,
    Project,
    Benchmark,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub key: String,
    pub scope: MemoryScope,
    pub value: String,
    pub session_id: SessionId,
    pub recorded_in: SessionPhase,
}
