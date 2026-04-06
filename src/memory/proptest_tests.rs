use proptest::prelude::*;
use uuid::Uuid;

use crate::memory::{CognitiveMemoryType, InMemoryMemoryStore, MemoryRecord, MemoryStore};
use crate::session::{SessionId, SessionPhase};

fn arb_memory_type() -> impl Strategy<Value = CognitiveMemoryType> {
    prop_oneof![
        Just(CognitiveMemoryType::Sensory),
        Just(CognitiveMemoryType::Working),
        Just(CognitiveMemoryType::Episodic),
        Just(CognitiveMemoryType::Semantic),
        Just(CognitiveMemoryType::Procedural),
        Just(CognitiveMemoryType::Prospective),
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
    (
        "\\PC+",
        arb_memory_type(),
        "\\PC*",
        arb_session_id(),
        arb_phase(),
    )
        .prop_map(
            |(key, memory_type, value, session_id, recorded_in)| MemoryRecord {
                key,
                memory_type,
                value,
                session_id,
                recorded_in,
            },
        )
}

fn new_store() -> InMemoryMemoryStore {
    InMemoryMemoryStore::try_default().expect("store creation should succeed")
}

proptest! {
    /// Any stored record can be recalled by its cognitive memory type.
    #[test]
    fn roundtrip_by_type(record in arb_record()) {
        let store = new_store();
        let mt = record.memory_type;
        store.put(record.clone()).unwrap();
        let found = store.list(mt).unwrap();
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
        mt in arb_memory_type(),
        v1 in "\\PC*",
        v2 in "\\PC*",
        sid in arb_session_id(),
        phase in arb_phase(),
    ) {
        let store = new_store();
        let r1 = MemoryRecord {
            key: key.clone(), memory_type: mt, value: v1,
            session_id: sid.clone(), recorded_in: phase,
        };
        let r2 = MemoryRecord {
            key: key.clone(), memory_type: mt, value: v2,
            session_id: sid.clone(), recorded_in: phase,
        };
        store.put(r1).unwrap();
        store.put(r2.clone()).unwrap();
        let found = store.list(mt).unwrap();
        prop_assert_eq!(found.last().unwrap(), &r2);
    }

    /// Records are partitioned correctly across memory types.
    #[test]
    fn type_partitioning(records in prop::collection::vec(arb_record(), 1..30)) {
        let store = new_store();
        for r in &records {
            store.put(r.clone()).unwrap();
        }
        let all_types = [
            CognitiveMemoryType::Sensory,
            CognitiveMemoryType::Working,
            CognitiveMemoryType::Episodic,
            CognitiveMemoryType::Semantic,
            CognitiveMemoryType::Procedural,
            CognitiveMemoryType::Prospective,
        ];
        let total: usize = all_types
            .iter()
            .map(|t| store.list(*t).unwrap().len())
            .sum();
        prop_assert_eq!(total, records.len());
    }

    /// Arbitrary keys and values survive storage without corruption.
    #[test]
    fn arbitrary_key_value_integrity(
        key in "\\PC{1,200}",
        value in "\\PC{0,500}",
        mt in arb_memory_type(),
        sid in arb_session_id(),
        phase in arb_phase(),
    ) {
        let store = new_store();
        let record = MemoryRecord {
            key: key.clone(), memory_type: mt, value: value.clone(),
            session_id: sid.clone(), recorded_in: phase,
        };
        store.put(record).unwrap();
        let found = store.list(mt).unwrap();
        prop_assert_eq!(&found[0].key, &key);
        prop_assert_eq!(&found[0].value, &value);
        prop_assert_eq!(&found[0].session_id, &sid);
    }
}
