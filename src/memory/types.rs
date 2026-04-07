use chrono::{DateTime, Utc};
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
    /// Explicit marker for records recovered from cognitive memory where the
    /// original scope tag was missing or unparseable.
    Untagged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub key: String,
    pub scope: MemoryScope,
    pub value: String,
    pub session_id: SessionId,
    pub recorded_in: SessionPhase,
    /// When this record was created. `None` for records deserialized from
    /// older files that predate this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}
