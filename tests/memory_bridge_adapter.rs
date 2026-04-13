//! Outside-in integration tests for CognitiveBridgeMemoryStore.
//!
//! These tests verify the adapter implements MemoryStore correctly when backed
//! by native cognitive memory (LadybugDB), and that bootstrap correctly selects
//! the cognitive backend when bridges are available.

use std::path::PathBuf;

use simard::cognitive_memory::NativeCognitiveMemory;
use simard::memory::{MemoryRecord, MemoryScope, MemoryStore};
use simard::memory_bridge_adapter::CognitiveBridgeMemoryStore;
use simard::session::SessionPhase;

/// RAII guard that cleans up test artifacts on drop.
struct TestFixture {
    store: CognitiveBridgeMemoryStore,
    state_root: PathBuf,
    fallback: PathBuf,
}

impl TestFixture {
    fn new(_label: &str) -> Self {
        let state_root =
            std::env::temp_dir().join(format!("adapter-live-{}", uuid::Uuid::now_v7()));
        let native_mem =
            NativeCognitiveMemory::open(&state_root).expect("native memory should open");
        let fallback =
            std::env::temp_dir().join(format!("adapter-fb-{}.json", uuid::Uuid::now_v7()));
        let store = CognitiveBridgeMemoryStore::new(native_mem, &fallback).expect("adapter");
        Self {
            store,
            state_root,
            fallback,
        }
    }
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.state_root);
        let _ = std::fs::remove_file(&self.fallback);
    }
}

fn test_session_id() -> simard::session::SessionId {
    uuid::Uuid::now_v7().into()
}

fn make_record(
    key: &str,
    scope: MemoryScope,
    session: &simard::session::SessionId,
) -> MemoryRecord {
    MemoryRecord {
        key: key.to_string(),
        scope,
        value: format!("value-for-{key}"),
        session_id: session.clone(),
        recorded_in: SessionPhase::Execution,
        created_at: None,
    }
}

// ---------- Scenario 1: Adapter stores and retrieves by scope ----------

#[test]
#[ignore] // Requires Python + LadybugDB
fn live_adapter_put_and_list_by_scope() {
    let fixture = TestFixture::new("adapter-scope");
    let session = test_session_id();

    fixture
        .store
        .put(make_record("decision-1", MemoryScope::Decision, &session))
        .expect("put decision");
    fixture
        .store
        .put(make_record("project-1", MemoryScope::Project, &session))
        .expect("put project");
    fixture
        .store
        .put(make_record("decision-2", MemoryScope::Decision, &session))
        .expect("put decision 2");

    assert_eq!(
        fixture
            .store
            .list(MemoryScope::Decision)
            .expect("list decisions")
            .len(),
        2
    );
    assert_eq!(
        fixture
            .store
            .list(MemoryScope::Project)
            .expect("list projects")
            .len(),
        1
    );
    assert_eq!(
        fixture
            .store
            .list(MemoryScope::Benchmark)
            .expect("list benchmarks")
            .len(),
        0
    );
}

// ---------- Scenario 2: Dedup by key ----------

#[test]
#[ignore] // Requires Python + LadybugDB
fn live_adapter_deduplicates_by_key() {
    let fixture = TestFixture::new("adapter-dedup");
    let session = test_session_id();

    fixture
        .store
        .put(make_record("same-key", MemoryScope::Decision, &session))
        .expect("first put");
    fixture
        .store
        .put(make_record("same-key", MemoryScope::Decision, &session))
        .expect("second put");

    assert_eq!(
        fixture
            .store
            .list(MemoryScope::Decision)
            .expect("list")
            .len(),
        1,
        "duplicate keys should be deduped"
    );
}

// ---------- Scenario 3: Session isolation ----------

#[test]
#[ignore] // Requires Python + LadybugDB
fn live_adapter_session_isolation() {
    let fixture = TestFixture::new("adapter-session");
    let session_a = test_session_id();
    let session_b = test_session_id();

    fixture
        .store
        .put(make_record(
            "rec-a1",
            MemoryScope::SessionScratch,
            &session_a,
        ))
        .expect("put a1");
    fixture
        .store
        .put(make_record(
            "rec-a2",
            MemoryScope::SessionScratch,
            &session_a,
        ))
        .expect("put a2");
    fixture
        .store
        .put(make_record(
            "rec-b1",
            MemoryScope::SessionScratch,
            &session_b,
        ))
        .expect("put b1");

    assert_eq!(
        fixture
            .store
            .list_for_session(&session_a)
            .expect("list a")
            .len(),
        2
    );
    assert_eq!(
        fixture
            .store
            .list_for_session(&session_b)
            .expect("list b")
            .len(),
        1
    );
    assert_eq!(
        fixture
            .store
            .count_for_session(&session_a)
            .expect("count a"),
        2
    );
    assert_eq!(
        fixture
            .store
            .count_for_session(&session_b)
            .expect("count b"),
        1
    );
}

// ---------- Scenario 4: Descriptor identifies cognitive backend ----------

#[test]
#[ignore] // Requires Python + LadybugDB
fn live_adapter_descriptor_identifies_backend() {
    let fixture = TestFixture::new("adapter-desc");
    let desc = fixture.store.descriptor();
    assert!(
        desc.identity.contains("cognitive-bridge"),
        "descriptor should identify cognitive bridge backend, got: {}",
        desc.identity
    );
}

// ---------- Scenario 5: Full lifecycle (put, list, count, overwrite) ----------

#[test]
#[ignore] // Requires Python + LadybugDB
fn live_adapter_full_lifecycle() {
    let fixture = TestFixture::new("adapter-lifecycle");
    let session = test_session_id();

    // Empty state
    assert_eq!(
        fixture
            .store
            .count_for_session(&session)
            .expect("empty count"),
        0
    );
    assert!(
        fixture
            .store
            .list(MemoryScope::Decision)
            .expect("empty list")
            .is_empty()
    );

    // Put records
    fixture
        .store
        .put(make_record("k1", MemoryScope::Decision, &session))
        .expect("put k1");
    fixture
        .store
        .put(make_record("k2", MemoryScope::SessionSummary, &session))
        .expect("put k2");
    assert_eq!(fixture.store.count_for_session(&session).expect("count"), 2);

    // Overwrite k1
    let mut updated = make_record("k1", MemoryScope::Decision, &session);
    updated.value = "updated-value".to_string();
    fixture.store.put(updated).expect("overwrite k1");

    let decisions = fixture
        .store
        .list(MemoryScope::Decision)
        .expect("list after update");
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].value, "updated-value");

    // Total count unchanged (overwrite, not insert)
    assert_eq!(
        fixture
            .store
            .count_for_session(&session)
            .expect("final count"),
        2
    );
}
