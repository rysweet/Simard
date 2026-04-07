use chrono::Utc;
use proptest::prelude::*;
use uuid::Uuid;

use crate::memory::{InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::session::{SessionId, SessionPhase};

fn arb_scope() -> impl Strategy<Value = MemoryScope> {
    prop_oneof![
        Just(MemoryScope::SessionScratch),
        Just(MemoryScope::SessionSummary),
        Just(MemoryScope::Decision),
        Just(MemoryScope::Project),
        Just(MemoryScope::Benchmark),
        Just(MemoryScope::Untagged),
    ]
}

fn arb_phase() -> impl Strategy<Value = SessionPhase> {
    prop_oneof![
        Just(SessionPhase::Intake),
        Just(SessionPhase::Preparation),
        Just(SessionPhase::Planning),
        Just(SessionPhase::Execution),
        Just(SessionPhase::Reflection),
        Just(SessionPhase::Persistence),
        Just(SessionPhase::Complete),
        Just(SessionPhase::Failed),
    ]
}

fn arb_session_id() -> impl Strategy<Value = SessionId> {
    any::<u128>().prop_map(|n| SessionId::from_uuid(Uuid::from_u128(n)))
}

fn arb_record() -> impl Strategy<Value = MemoryRecord> {
    ("\\PC+", arb_scope(), "\\PC*", arb_session_id(), arb_phase()).prop_map(
        |(key, scope, value, session_id, recorded_in)| MemoryRecord {
            key,
            scope,
            value,
            session_id,
            recorded_in,
            created_at: Some(Utc::now()),
        },
    )
}

fn new_store() -> InMemoryMemoryStore {
    InMemoryMemoryStore::try_default().expect("store creation should succeed")
}

proptest! {
    /// Any stored record can be recalled by its scope.
    #[test]
    fn roundtrip_by_scope(record in arb_record()) {
        let store = new_store();
        let scope = record.scope;
        store.put(record.clone()).unwrap();
        let found = store.list(scope).unwrap();
        prop_assert!(found.contains(&record));
    }

    /// Any stored record can be recalled by its session id.
    #[test]
    fn roundtrip_by_session(record in arb_record()) {
        let store = new_store();
        let sid = record.session_id.clone();
        store.put(record.clone()).unwrap();
        let found = store.list_for_session(&sid).unwrap();
        prop_assert!(found.contains(&record));
    }

    /// count_for_session equals the length of list_for_session.
    #[test]
    fn count_matches_list_len(records in prop::collection::vec(arb_record(), 1..30)) {
        let store = new_store();
        let sid = records[0].session_id.clone();
        for r in &records {
            store.put(r.clone()).unwrap();
        }
        let list = store.list_for_session(&sid).unwrap();
        let count = store.count_for_session(&sid).unwrap();
        prop_assert_eq!(list.len(), count);
    }

    /// Storing the same key twice keeps both; the latest value appears last.
    #[test]
    fn duplicate_key_returns_latest_last(
        key in "\\PC+",
        scope in arb_scope(),
        v1 in "\\PC*",
        v2 in "\\PC*",
        sid in arb_session_id(),
        phase in arb_phase(),
    ) {
        let store = new_store();
        let r1 = MemoryRecord {
            key: key.clone(), scope, value: v1,
            session_id: sid.clone(), recorded_in: phase,
            created_at: None,
        };
        let r2 = MemoryRecord {
            key: key.clone(), scope, value: v2,
            session_id: sid.clone(), recorded_in: phase,
            created_at: None,
        };
        store.put(r1).unwrap();
        store.put(r2.clone()).unwrap();
        let found = store.list(scope).unwrap();
        let last = found.last().unwrap();
        prop_assert_eq!(&last.key, &r2.key);
        prop_assert_eq!(&last.value, &r2.value);
        prop_assert_eq!(&last.scope, &r2.scope);
        prop_assert_eq!(&last.session_id, &r2.session_id);
        // created_at is auto-stamped by put(), so skip comparing it
    }

    /// Records are partitioned correctly across scopes.
    #[test]
    fn scope_partitioning(records in prop::collection::vec(arb_record(), 1..30)) {
        let store = new_store();
        for r in &records {
            store.put(r.clone()).unwrap();
        }
        let all_scopes = [
            MemoryScope::SessionScratch,
            MemoryScope::SessionSummary,
            MemoryScope::Decision,
            MemoryScope::Project,
            MemoryScope::Benchmark,
            MemoryScope::Untagged,
        ];
        let total: usize = all_scopes
            .iter()
            .map(|s| store.list(*s).unwrap().len())
            .sum();
        prop_assert_eq!(total, records.len());
    }

    /// Arbitrary keys and values survive storage without corruption.
    #[test]
    fn arbitrary_key_value_integrity(
        key in "\\PC{1,200}",
        value in "\\PC{0,500}",
        scope in arb_scope(),
        sid in arb_session_id(),
        phase in arb_phase(),
    ) {
        let store = new_store();
        let record = MemoryRecord {
            key: key.clone(), scope, value: value.clone(),
            session_id: sid.clone(), recorded_in: phase,
            created_at: None,
        };
        store.put(record).unwrap();
        let found = store.list(scope).unwrap();
        prop_assert_eq!(&found[0].key, &key);
        prop_assert_eq!(&found[0].value, &value);
        prop_assert_eq!(&found[0].session_id, &sid);
    }
}
