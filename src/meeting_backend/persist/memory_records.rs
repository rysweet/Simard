//! Companion `memory_records.json` writer for the REPL close path
//! (issue #2000).
//!
//! Without this, `simard meeting repl` produced a structured
//! `meeting_handoff.json` bundle but **no** `memory_records.json` — so
//! `simard meeting read`, which looks up `<state_root>/memory_records.json`
//! via [`crate::FileBackedMemoryStore`], hard-failed on every otherwise
//! successful REPL session. See issue #2000 for the bug report.
//!
//! Two on-disk copies are written so both the bundle-aware and the legacy
//! state-root-aware reader find the record:
//!
//! * `<bundle_dir>/memory_records.json` — alongside `meeting_handoff.json`
//!   and `transcript.json` so a bundle is self-contained.
//! * `<state_root>/memory_records.json` — the location `simard meeting read`
//!   already expects, where `<state_root>` is the parent of the
//!   `meetings/` directory under
//!   [`crate::meeting_facilitator::default_bundle_root`].
//!
//! Writes go through [`crate::FileBackedMemoryStore`], which uses the same
//! checksummed-envelope atomic-write path as the non-REPL meeting probe.
//! When no records were produced, an empty-but-valid file is still written
//! (task acceptance criterion (b)).

use std::path::{Path, PathBuf};

use serde_json::json;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::{SimardError, SimardResult};
use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::session::{SessionId, SessionPhase};

use super::super::types::HandoffActionItem;

/// Filename inside both the per-meeting bundle directory and the state-root
/// where the records are persisted. Matches the path
/// [`crate::operator_commands_meeting`] reads from.
pub const MEMORY_RECORDS_FILENAME: &str = "memory_records.json";

/// Build a `PersistedMeetingRecord`-shaped value string from a closed
/// meeting.
///
/// Delegates to the shared [`crate::build_persisted_meeting_record_value`]
/// renderer so the REPL close path and the non-REPL
/// `MeetingFacilitatorProgram` path emit byte-identical wire format for
/// equivalent inputs (issue #2003 — eliminates silent-drift risk between
/// the two persistence paths). The read companion's
/// `looks_like_persisted_meeting_record` filter and
/// `PersistedMeetingRecord::parse` both succeed on whatever this returns.
pub(crate) fn build_meeting_record_value(
    topic: &str,
    decisions: &[String],
    action_items: &[HandoffActionItem],
    open_questions: &[String],
) -> String {
    let next_steps: Vec<String> = action_items.iter().map(|a| a.description.clone()).collect();
    crate::build_persisted_meeting_record_value(topic, decisions, &next_steps, open_questions)
}

/// Write `memory_records.json` for the closing REPL meeting.
///
/// Writes to BOTH `<bundle_dir>/memory_records.json` and
/// `<state_root>/memory_records.json` (where `<state_root>` is derived from
/// `bundle_dir.parent().parent()` — i.e. one level above the `meetings/`
/// directory). The state-root copy is what `simard meeting read` opens.
///
/// The file always carries at least one record so the read path's
/// `looks_like_persisted_meeting_record` filter has something to surface —
/// even if no decisions/action items/questions were extracted, a record
/// containing the topic-as-agenda and empty lists is emitted (acceptance
/// criterion (b) on issue #2000).
///
/// Errors are returned but the caller (the close pipeline) should treat
/// a single-path failure as a partial close, not a hard failure: a missing
/// `memory_records.json` is exactly the bug this fix exists to prevent, so
/// we want loud logging but not to abort the rest of the close.
pub fn write_meeting_memory_records(
    bundle_dir: &Path,
    topic: &str,
    decisions: &[String],
    action_items: &[HandoffActionItem],
    open_questions: &[String],
) -> SimardResult<Vec<PathBuf>> {
    let value = build_meeting_record_value(topic, decisions, action_items, open_questions);
    // Stable, deterministic synthetic SessionId. Read consumers do not key
    // on session_id (they filter on scope + content shape), so anything
    // serde-valid is fine. Use a v7 UUID for monotonic ordering.
    let session_id = SessionId::from_uuid(Uuid::now_v7());
    let record = MemoryRecord {
        key: format!("{}-meeting-record", session_id.as_str()),
        scope: MemoryScope::Decision,
        value,
        session_id,
        recorded_in: SessionPhase::Complete,
        created_at: Some(chrono::Utc::now()),
    };

    let mut written = Vec::with_capacity(2);
    let mut last_err: Option<SimardError> = None;

    // ── 1. Bundle-local copy (`<bundle_dir>/memory_records.json`). ──
    let bundle_target = bundle_dir.join(MEMORY_RECORDS_FILENAME);
    match write_one_target(&bundle_target, &record) {
        Ok(()) => written.push(bundle_target),
        Err(e) => {
            warn!(
                target: "simard::meeting_backend::persist::memory_records",
                path = %bundle_target.display(),
                error = %e,
                "failed to write per-bundle memory_records.json"
            );
            last_err = Some(e);
        }
    }

    // ── 2. State-root copy (`<state_root>/memory_records.json`). ──
    // The state root is the grandparent of the bundle dir:
    //   <state_root>/meetings/<meeting_id>/  ←  bundle_dir
    if let Some(state_root) = state_root_for_bundle(bundle_dir) {
        let state_target = state_root.join(MEMORY_RECORDS_FILENAME);
        // Best-effort directory creation — the bundle dir already exists,
        // but the parent may not on test fixtures that point straight at
        // a one-off path.
        if let Some(parent) = state_target.parent()
            && !parent.exists()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            warn!(
                target: "simard::meeting_backend::persist::memory_records",
                path = %parent.display(),
                error = %e,
                "failed to create state-root directory for memory_records.json"
            );
        }
        match write_one_target(&state_target, &record) {
            Ok(()) => written.push(state_target),
            Err(e) => {
                warn!(
                    target: "simard::meeting_backend::persist::memory_records",
                    path = %state_target.display(),
                    error = %e,
                    "failed to write state-root memory_records.json"
                );
                last_err = Some(e);
            }
        }
    }

    if written.is_empty() {
        // Surface the most recent error so the caller can mark the close
        // partial.
        return Err(
            last_err.unwrap_or_else(|| SimardError::ActionExecutionFailed {
                action: "write-meeting-memory-records".to_string(),
                reason: format!(
                    "no destination resolved for memory_records.json (bundle_dir={})",
                    bundle_dir.display()
                ),
            }),
        );
    }

    info!(
        target: "simard::meeting_backend::persist::memory_records",
        paths = ?written,
        "meeting memory_records.json written"
    );
    Ok(written)
}

/// Write `record` into `target` via [`FileBackedMemoryStore`] so the on-disk
/// envelope (checksummed + atomic) matches what `simard meeting read`
/// expects.
///
/// If `target` already contains records, `record` is appended (or replaces
/// the existing entry with the same key) — same semantics as
/// [`MemoryStore::put`]. This keeps existing non-meeting records from being
/// trampled if a state-root is shared with other writers.
fn write_one_target(target: &Path, record: &MemoryRecord) -> SimardResult<()> {
    let store = FileBackedMemoryStore::try_new(target.to_path_buf())?;
    store.put(record.clone())
}

/// Derive the state root for a bundle directory of the form
/// `<state_root>/meetings/<meeting_id>/`. Returns `None` if `bundle_dir`
/// has fewer than two ancestors (which would only happen for a degenerate
/// test fixture pointing at the filesystem root).
fn state_root_for_bundle(bundle_dir: &Path) -> Option<PathBuf> {
    bundle_dir.parent()?.parent().map(Path::to_path_buf)
}

/// Convenience helper: ensure the on-disk file exists at `target` even when
/// no records were produced. Used by tests and by the partial-close path
/// when we still want a deserialize-valid (but empty) file to land.
///
/// The file is written as an empty `ChecksummedPayload`-compatible JSON
/// document; `FileBackedMemoryStore::try_new(...)` will load it back as
/// `Vec::new()` without error.
#[allow(dead_code)]
pub(crate) fn write_empty_records_file(target: &Path) -> SimardResult<()> {
    // The checksummed envelope's `records: []` payload is the canonical
    // empty state; deserializing it produces `Vec::new()` and the load
    // path's CRC check passes because crc32fast::hash(b"[]") is well-defined.
    let empty_envelope = json!({
        "crc32": crc32fast::hash(b"[]"),
        "records": [],
    });
    let bytes = serde_json::to_vec_pretty(&empty_envelope).map_err(|e| {
        SimardError::ActionExecutionFailed {
            action: "serialize-empty-memory-records".to_string(),
            reason: e.to_string(),
        }
    })?;
    crate::persistence::persist_bytes("memory", target, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn build_meeting_record_value_round_trips_empty_lists() {
        let value = build_meeting_record_value("Sprint review", &[], &[], &[]);
        // Required substrings for `looks_like_persisted_meeting_record`.
        for fragment in [
            "agenda=",
            "updates=",
            "decisions=",
            "risks=",
            "next_steps=",
            "open_questions=",
            "goals=",
        ] {
            assert!(
                value.contains(fragment),
                "missing fragment {fragment} in {value}"
            );
        }
        // Must parse cleanly through the read-side parser.
        let parsed = crate::meetings::PersistedMeetingRecord::parse(&value)
            .expect("empty-list record parses");
        assert_eq!(parsed.agenda, "Sprint review");
        assert!(parsed.decisions.is_empty());
        assert!(parsed.next_steps.is_empty());
        assert!(parsed.goals.is_empty());
    }

    #[test]
    fn build_meeting_record_value_falls_back_to_meeting_when_topic_blank() {
        let value = build_meeting_record_value("   ", &[], &[], &[]);
        let parsed =
            crate::meetings::PersistedMeetingRecord::parse(&value).expect("blank topic parses");
        assert_eq!(parsed.agenda, "meeting");
    }

    #[test]
    fn build_meeting_record_value_carries_decisions_actions_questions() {
        let decisions = vec!["Ship the fix".to_string(), "Skip the rebase".to_string()];
        let actions = vec![
            HandoffActionItem {
                description: "Write the regression test".to_string(),
                assignee: Some("alice".to_string()),
                deadline: None,
                priority: None,
                linked_goal: None,
            },
            HandoffActionItem {
                description: "Run cargo clippy".to_string(),
                assignee: None,
                deadline: None,
                priority: None,
                linked_goal: None,
            },
        ];
        let questions = vec!["Do we need an audit follow-up?".to_string()];
        let value = build_meeting_record_value("Close #2000", &decisions, &actions, &questions);
        let parsed = crate::meetings::PersistedMeetingRecord::parse(&value)
            .expect("populated record parses");
        assert_eq!(parsed.decisions, decisions);
        assert_eq!(
            parsed.next_steps,
            vec!["Write the regression test", "Run cargo clippy"]
        );
        assert_eq!(parsed.open_questions, questions);
    }

    #[test]
    fn write_meeting_memory_records_writes_both_targets() {
        let tmp = TempDir::new().unwrap();
        let state_root = tmp.path();
        let bundle_dir = state_root.join("meetings").join("20260101T000000Z-test");
        std::fs::create_dir_all(&bundle_dir).unwrap();

        let written = write_meeting_memory_records(
            &bundle_dir,
            "Sprint review",
            &["Ship the fix".to_string()],
            &[],
            &[],
        )
        .expect("write succeeds");

        assert_eq!(written.len(), 2, "expected bundle + state-root targets");
        let bundle_target = bundle_dir.join(MEMORY_RECORDS_FILENAME);
        let state_target = state_root.join(MEMORY_RECORDS_FILENAME);
        assert!(bundle_target.is_file(), "bundle file missing");
        assert!(state_target.is_file(), "state-root file missing");

        // Read back through FileBackedMemoryStore — the read companion's
        // exact code path.
        let store = FileBackedMemoryStore::try_new(state_target).expect("load state-root store");
        let decisions = store.list(MemoryScope::Decision).expect("list decisions");
        assert_eq!(decisions.len(), 1);
        assert!(
            crate::meetings::looks_like_persisted_meeting_record(&decisions[0].value),
            "record value should look like a persisted meeting record: {}",
            decisions[0].value
        );
    }

    #[test]
    fn write_meeting_memory_records_emits_empty_but_valid_record() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("meetings").join("empty-meeting");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        let written = write_meeting_memory_records(&bundle_dir, "no-content", &[], &[], &[])
            .expect("write succeeds with no extracted content");
        assert_eq!(written.len(), 2);

        let store =
            FileBackedMemoryStore::try_new(tmp.path().join(MEMORY_RECORDS_FILENAME)).unwrap();
        let recs = store.list(MemoryScope::Decision).unwrap();
        assert_eq!(
            recs.len(),
            1,
            "even with no extracted content, one record should be written"
        );
        // Round-trip parse must succeed so the read probe does not error.
        crate::meetings::PersistedMeetingRecord::parse(&recs[0].value)
            .expect("empty-list record still parses");
    }

    #[test]
    fn write_meeting_memory_records_appends_without_trampling_existing() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("meetings").join("append-test");
        std::fs::create_dir_all(&bundle_dir).unwrap();

        // Seed the state-root file with an unrelated existing record.
        let state_path = tmp.path().join(MEMORY_RECORDS_FILENAME);
        let seed_store = FileBackedMemoryStore::try_new(state_path.clone()).unwrap();
        let seed_session = SessionId::from_uuid(Uuid::now_v7());
        seed_store
            .put(MemoryRecord {
                key: format!("{}-seed", seed_session.as_str()),
                scope: MemoryScope::Decision,
                value: "unrelated-non-meeting-value".to_string(),
                session_id: seed_session,
                recorded_in: SessionPhase::Complete,
                created_at: Some(chrono::Utc::now()),
            })
            .unwrap();

        write_meeting_memory_records(&bundle_dir, "append-test", &[], &[], &[]).unwrap();

        let store = FileBackedMemoryStore::try_new(state_path).unwrap();
        let decisions = store.list(MemoryScope::Decision).unwrap();
        assert_eq!(
            decisions.len(),
            2,
            "should append; existing record preserved: {:?}",
            decisions
        );
        let meeting_records: Vec<_> = decisions
            .iter()
            .filter(|r| crate::meetings::looks_like_persisted_meeting_record(&r.value))
            .collect();
        assert_eq!(
            meeting_records.len(),
            1,
            "exactly one meeting record should be present"
        );
    }

    #[test]
    fn state_root_for_bundle_returns_grandparent() {
        let p = PathBuf::from("/tmp/state/meetings/m1");
        assert_eq!(state_root_for_bundle(&p), Some(PathBuf::from("/tmp/state")));
    }

    #[test]
    fn write_empty_records_file_round_trips_through_store() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join(MEMORY_RECORDS_FILENAME);
        write_empty_records_file(&target).unwrap();
        let store = FileBackedMemoryStore::try_new(target).unwrap();
        assert!(store.list(MemoryScope::Decision).unwrap().is_empty());
    }
}
