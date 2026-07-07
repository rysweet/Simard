//! Extended unit tests for the `goals` module.
//!
//! Covers error paths, edge cases, serde round-trips, and store
//! behaviours that the existing inline tests do not exercise.
//! No `skip_if_no_llm_provider` — every test here runs deterministically.

use super::*;
use crate::error::SimardError;
use crate::session::{SessionId, SessionPhase};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_session_id() -> SessionId {
    SessionId::parse("session-00000000-0000-0000-0000-000000000000").expect("valid session id")
}

fn make_update(title: &str, status: GoalStatus, priority: u8) -> GoalUpdate {
    GoalUpdate::new(title, "test rationale", status, priority).expect("valid update")
}

fn make_record(title: &str, status: GoalStatus, priority: u8) -> GoalRecord {
    GoalRecord::from_update(
        make_update(title, status, priority),
        "test-owner",
        test_session_id(),
        SessionPhase::Persistence,
    )
    .expect("valid record")
}

// ===========================================================================
// GoalUpdate::new — error paths
// ===========================================================================

#[test]
fn goal_record_labels_default_empty_and_serde_back_compatible() {
    // Issue #2743: `labels` is additive + serde-back-compatible on GoalRecord.
    // from_update yields an empty label set...
    let record = make_record("Ship it", GoalStatus::Active, 1);
    assert!(
        record.labels.is_empty(),
        "from_update records start unlabelled"
    );

    // ...and an unlabelled record serializes WITHOUT a `labels` key, so
    // pre-#2743 goal-store snapshots stay byte-identical.
    let json = serde_json::to_string(&record).expect("serialize");
    assert!(
        !json.contains("\"labels\""),
        "empty labels must be omitted (skip_serializing_if): {json}",
    );

    // A legacy JSON blob (no `labels` key) deserializes to an empty Vec.
    let back: GoalRecord = serde_json::from_str(&json).expect("deserialize legacy");
    assert!(
        back.labels.is_empty(),
        "missing labels key -> empty via serde(default)"
    );

    // A labelled record round-trips exactly.
    let mut labelled = make_record("Tag me", GoalStatus::Active, 2);
    labelled.labels = vec!["source:creative-ideas".to_string(), "area:x".to_string()];
    let round: GoalRecord =
        serde_json::from_str(&serde_json::to_string(&labelled).expect("ser")).expect("de");
    assert_eq!(round.labels, labelled.labels);
}

#[test]
fn goal_update_rejects_empty_title() {
    let err = GoalUpdate::new("", "rationale", GoalStatus::Active, 1).unwrap_err();
    assert!(
        matches!(err, SimardError::InvalidGoalRecord { ref field, .. } if field == "title"),
        "expected InvalidGoalRecord for title, got: {err}"
    );
}

#[test]
fn goal_update_rejects_whitespace_only_title() {
    let err = GoalUpdate::new("   ", "rationale", GoalStatus::Active, 1).unwrap_err();
    assert!(matches!(err, SimardError::InvalidGoalRecord { ref field, .. } if field == "title"),);
}

#[test]
fn goal_update_rejects_empty_rationale() {
    let err = GoalUpdate::new("Title", "", GoalStatus::Active, 1).unwrap_err();
    assert!(
        matches!(err, SimardError::InvalidGoalRecord { ref field, .. } if field == "rationale"),
    );
}

#[test]
fn goal_update_rejects_whitespace_only_rationale() {
    let err = GoalUpdate::new("Title", "  \t\n  ", GoalStatus::Active, 1).unwrap_err();
    assert!(
        matches!(err, SimardError::InvalidGoalRecord { ref field, .. } if field == "rationale"),
    );
}

#[test]
fn goal_update_rejects_priority_zero() {
    let err = GoalUpdate::new("Title", "rationale", GoalStatus::Active, 0).unwrap_err();
    assert!(matches!(err, SimardError::InvalidGoalRecord { ref field, .. } if field == "priority"),);
}

#[test]
fn goal_update_accepts_priority_one() {
    let update = GoalUpdate::new("Title", "rationale", GoalStatus::Active, 1).unwrap();
    assert_eq!(update.priority, 1);
}

#[test]
fn goal_update_accepts_priority_max() {
    let update = GoalUpdate::new("Title", "rationale", GoalStatus::Active, u8::MAX).unwrap();
    assert_eq!(update.priority, u8::MAX);
}

#[test]
fn goal_update_trims_whitespace_from_fields() {
    let update = GoalUpdate::new(
        "  Padded Title  ",
        "  padded rationale  ",
        GoalStatus::Active,
        1,
    )
    .unwrap();
    assert_eq!(update.title, "Padded Title");
    assert_eq!(update.rationale, "padded rationale");
}

#[test]
fn goal_update_slug_derived_from_title() {
    let update = GoalUpdate::new("Fix Broken Tests", "reason", GoalStatus::Active, 1).unwrap();
    assert_eq!(update.slug, "fix-broken-tests");
}

// ===========================================================================
// GoalRecord::from_update — error paths
// ===========================================================================

#[test]
fn goal_record_rejects_empty_owner_identity() {
    let update = make_update("Title", GoalStatus::Active, 1);
    let err = GoalRecord::from_update(update, "", test_session_id(), SessionPhase::Persistence)
        .unwrap_err();
    assert!(
        matches!(err, SimardError::InvalidGoalRecord { ref field, .. } if field == "owner_identity"),
    );
}

#[test]
fn goal_record_rejects_whitespace_only_owner() {
    let update = make_update("Title", GoalStatus::Active, 1);
    let err = GoalRecord::from_update(update, "   ", test_session_id(), SessionPhase::Persistence)
        .unwrap_err();
    assert!(
        matches!(err, SimardError::InvalidGoalRecord { ref field, .. } if field == "owner_identity"),
    );
}

#[test]
fn goal_record_from_update_with_manually_empty_slug_is_rejected() {
    // Bypass GoalUpdate::new validation by constructing directly.
    let update = GoalUpdate {
        slug: String::new(),
        title: "Valid Title".into(),
        rationale: "Valid rationale".into(),
        status: GoalStatus::Active,
        priority: 1,
        evidence: Vec::new(),
    };
    let err = GoalRecord::from_update(
        update,
        "owner",
        test_session_id(),
        SessionPhase::Persistence,
    )
    .unwrap_err();
    assert!(matches!(err, SimardError::InvalidGoalRecord { ref field, .. } if field == "slug"),);
}

#[test]
fn goal_record_from_update_with_manually_empty_title_is_rejected() {
    let update = GoalUpdate {
        slug: "valid-slug".into(),
        title: String::new(),
        rationale: "Valid rationale".into(),
        status: GoalStatus::Active,
        priority: 1,
        evidence: Vec::new(),
    };
    let err = GoalRecord::from_update(
        update,
        "owner",
        test_session_id(),
        SessionPhase::Persistence,
    )
    .unwrap_err();
    assert!(matches!(err, SimardError::InvalidGoalRecord { ref field, .. } if field == "title"),);
}

#[test]
fn goal_record_from_update_with_manually_empty_rationale_is_rejected() {
    let update = GoalUpdate {
        slug: "valid-slug".into(),
        title: "Valid Title".into(),
        rationale: String::new(),
        status: GoalStatus::Active,
        priority: 1,
        evidence: Vec::new(),
    };
    let err = GoalRecord::from_update(
        update,
        "owner",
        test_session_id(),
        SessionPhase::Persistence,
    )
    .unwrap_err();
    assert!(
        matches!(err, SimardError::InvalidGoalRecord { ref field, .. } if field == "rationale"),
    );
}

// ===========================================================================
// GoalStatus — Display, rank ordering, serde
// ===========================================================================

#[test]
fn goal_status_display_all_variants() {
    assert_eq!(GoalStatus::Proposed.to_string(), "proposed");
    assert_eq!(GoalStatus::Active.to_string(), "active");
    assert_eq!(GoalStatus::Paused.to_string(), "paused");
    assert_eq!(GoalStatus::Completed.to_string(), "completed");
}

#[test]
fn goal_status_rank_ordering_active_first_completed_last() {
    // rank() is pub(super), accessible from this sibling module.
    let mut statuses = vec![
        GoalStatus::Completed,
        GoalStatus::Proposed,
        GoalStatus::Active,
        GoalStatus::Paused,
    ];
    statuses.sort_by_key(|s| s.rank());
    assert_eq!(
        statuses,
        vec![
            GoalStatus::Active,
            GoalStatus::Proposed,
            GoalStatus::Paused,
            GoalStatus::Completed,
        ]
    );
}

#[test]
fn goal_status_serde_round_trip_all_variants() {
    for status in [
        GoalStatus::Proposed,
        GoalStatus::Active,
        GoalStatus::Paused,
        GoalStatus::Completed,
    ] {
        let json = serde_json::to_string(&status).expect("serialize");
        let back: GoalStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, status, "round-trip failed for {status}");
    }
}

#[test]
fn goal_status_serde_uses_kebab_case() {
    assert_eq!(
        serde_json::to_string(&GoalStatus::Proposed).unwrap(),
        "\"proposed\""
    );
    assert_eq!(
        serde_json::to_string(&GoalStatus::Active).unwrap(),
        "\"active\""
    );
    assert_eq!(
        serde_json::to_string(&GoalStatus::Paused).unwrap(),
        "\"paused\""
    );
    assert_eq!(
        serde_json::to_string(&GoalStatus::Completed).unwrap(),
        "\"completed\""
    );
}

#[test]
fn goal_status_parse_trims_whitespace() {
    assert_eq!(GoalStatus::parse("  active  "), Some(GoalStatus::Active));
}

#[test]
fn goal_status_parse_is_case_insensitive() {
    assert_eq!(GoalStatus::parse("PROPOSED"), Some(GoalStatus::Proposed));
    assert_eq!(GoalStatus::parse("Active"), Some(GoalStatus::Active));
    assert_eq!(GoalStatus::parse("pAuSeD"), Some(GoalStatus::Paused));
    assert_eq!(GoalStatus::parse("COMPLETED"), Some(GoalStatus::Completed));
}

// ===========================================================================
// goal_slug — edge cases
// ===========================================================================

#[test]
fn goal_slug_empty_string() {
    assert_eq!(goal_slug(""), "");
}

#[test]
fn goal_slug_whitespace_only() {
    // Spaces become dashes, then trim_matches('-') strips them all.
    assert_eq!(goal_slug("   "), "");
}

#[test]
fn goal_slug_single_char() {
    assert_eq!(goal_slug("a"), "a");
    assert_eq!(goal_slug("Z"), "z");
    assert_eq!(goal_slug("5"), "5");
}

#[test]
fn goal_slug_special_chars_only() {
    // Only non-alphanumeric chars → all become dashes → stripped.
    assert_eq!(goal_slug("!@#$%^&*()"), "");
}

#[test]
fn goal_slug_leading_trailing_special_chars() {
    assert_eq!(goal_slug("--hello--"), "hello");
    assert_eq!(goal_slug("  hello  "), "hello");
    assert_eq!(goal_slug("!!!hello!!!"), "hello");
}

#[test]
fn goal_slug_consecutive_special_chars_collapse_to_one_dash() {
    assert_eq!(goal_slug("a   b"), "a-b");
    assert_eq!(goal_slug("a---b"), "a-b");
    assert_eq!(goal_slug("a!@#b"), "a-b");
}

#[test]
fn goal_slug_unicode_non_ascii_alpha_stripped() {
    // Non-ASCII alphanumeric chars are not included (only ASCII retained).
    assert_eq!(goal_slug("café"), "caf");
    assert_eq!(goal_slug("über"), "ber");
}

#[test]
fn goal_slug_exactly_at_max_len_boundary_no_hash() {
    // Create a title whose slug is exactly GOAL_SLUG_MAX_LEN chars.
    let title = "a".repeat(GOAL_SLUG_MAX_LEN);
    let slug = goal_slug(&title);
    assert_eq!(slug.len(), GOAL_SLUG_MAX_LEN);
    assert!(
        !slug.contains('-'),
        "no hash suffix for exactly-at-boundary slug"
    );
}

#[test]
fn goal_slug_one_over_max_len_gets_hash() {
    let title = "a".repeat(GOAL_SLUG_MAX_LEN + 1);
    let slug = goal_slug(&title);
    assert!(slug.len() <= GOAL_SLUG_MAX_LEN);
    // Has a hash suffix (8 hex chars after the last dash).
    let parts: Vec<&str> = slug.rsplitn(2, '-').collect();
    assert_eq!(parts[0].len(), 8, "hash suffix must be 8 chars");
}

#[test]
fn goal_slug_numeric_title() {
    assert_eq!(goal_slug("12345"), "12345");
}

#[test]
fn goal_slug_mixed_case_lowercased() {
    assert_eq!(goal_slug("FooBarBaz"), "foobarbaz");
}

// ===========================================================================
// GoalRecord / GoalUpdate serde round-trip
// ===========================================================================

#[test]
fn goal_record_serde_round_trip() {
    let record = make_record("Serde Test", GoalStatus::Active, 3);
    let json = serde_json::to_string(&record).expect("serialize");
    let back: GoalRecord = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, record);
}

#[test]
fn goal_update_serde_round_trip() {
    let update = make_update("Serde Update", GoalStatus::Proposed, 2);
    let json = serde_json::to_string(&update).expect("serialize");
    let back: GoalUpdate = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, update);
}

#[test]
fn goal_record_concise_label_all_statuses() {
    for (status, expected_label) in [
        (GoalStatus::Proposed, "[proposed]"),
        (GoalStatus::Active, "[active]"),
        (GoalStatus::Paused, "[paused]"),
        (GoalStatus::Completed, "[completed]"),
    ] {
        let record = make_record("Test", status, 1);
        let label = record.concise_label();
        assert!(
            label.contains(expected_label),
            "label for {status} should contain {expected_label}, got: {label}"
        );
        assert!(
            label.contains("p1"),
            "label should contain priority: {label}"
        );
        assert!(
            label.contains("Test"),
            "label should contain title: {label}"
        );
    }
}

// ===========================================================================
// InMemoryGoalStore — filtering and limits
// ===========================================================================

fn populated_in_memory_store() -> InMemoryGoalStore {
    let store = InMemoryGoalStore::try_default().expect("store");
    let goals = [
        ("Alpha", GoalStatus::Active, 2),
        ("Beta", GoalStatus::Active, 1),
        ("Gamma", GoalStatus::Proposed, 1),
        ("Delta", GoalStatus::Paused, 1),
        ("Epsilon", GoalStatus::Completed, 1),
    ];
    for (title, status, priority) in goals {
        store.put(make_record(title, status, priority)).unwrap();
    }
    store
}

#[test]
fn in_memory_store_top_goals_by_proposed() {
    let store = populated_in_memory_store();
    let proposed = store.top_goals_by_status(GoalStatus::Proposed, 10).unwrap();
    assert_eq!(proposed.len(), 1);
    assert_eq!(proposed[0].title, "Gamma");
}

#[test]
fn in_memory_store_top_goals_by_paused() {
    let store = populated_in_memory_store();
    let paused = store.top_goals_by_status(GoalStatus::Paused, 10).unwrap();
    assert_eq!(paused.len(), 1);
    assert_eq!(paused[0].title, "Delta");
}

#[test]
fn in_memory_store_top_goals_by_completed() {
    let store = populated_in_memory_store();
    let completed = store
        .top_goals_by_status(GoalStatus::Completed, 10)
        .unwrap();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].title, "Epsilon");
}

#[test]
fn in_memory_store_active_top_goals_respects_limit() {
    let store = populated_in_memory_store();
    let top1 = store.active_top_goals(1).unwrap();
    assert_eq!(top1.len(), 1);
    // Beta (priority 1) should come before Alpha (priority 2).
    assert_eq!(top1[0].title, "Beta");
}

#[test]
fn in_memory_store_top_goals_limit_zero_returns_empty() {
    let store = populated_in_memory_store();
    let empty = store.active_top_goals(0).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn in_memory_store_top_goals_limit_exceeds_count() {
    let store = populated_in_memory_store();
    let all_active = store.active_top_goals(100).unwrap();
    assert_eq!(all_active.len(), 2); // Only Alpha and Beta are active.
}

#[test]
fn in_memory_store_list_returns_all_records() {
    let store = populated_in_memory_store();
    let all = store.list().unwrap();
    assert_eq!(all.len(), 5);
}

#[test]
fn in_memory_store_upsert_replaces_by_slug() {
    let store = InMemoryGoalStore::try_default().expect("store");
    store
        .put(make_record("Same Goal", GoalStatus::Proposed, 3))
        .unwrap();
    store
        .put(make_record("Same Goal", GoalStatus::Active, 1))
        .unwrap();

    let all = store.list().unwrap();
    assert_eq!(all.len(), 1, "upsert should replace, not duplicate");
    assert_eq!(all[0].status, GoalStatus::Active);
    assert_eq!(all[0].priority, 1);
}

// ===========================================================================
// InMemoryGoalStore — sorting order through public API
// ===========================================================================

#[test]
fn in_memory_store_active_goals_sorted_by_priority_then_title() {
    let store = InMemoryGoalStore::try_default().expect("store");
    store
        .put(make_record("Zebra", GoalStatus::Active, 2))
        .unwrap();
    store
        .put(make_record("Apple", GoalStatus::Active, 2))
        .unwrap();
    store
        .put(make_record("Middle", GoalStatus::Active, 1))
        .unwrap();

    let active = store.active_top_goals(10).unwrap();
    assert_eq!(active.len(), 3);
    // Priority 1 first, then priority 2 sorted by title.
    assert_eq!(active[0].title, "Middle");
    assert_eq!(active[1].title, "Apple");
    assert_eq!(active[2].title, "Zebra");
}

#[test]
fn in_memory_store_top_goals_sort_order_status_then_priority_then_title() {
    // When requesting a single status, records sort by priority then title.
    let store = InMemoryGoalStore::try_default().expect("store");
    store
        .put(make_record("C", GoalStatus::Proposed, 3))
        .unwrap();
    store
        .put(make_record("A", GoalStatus::Proposed, 1))
        .unwrap();
    store
        .put(make_record("B", GoalStatus::Proposed, 1))
        .unwrap();

    let proposed = store.top_goals_by_status(GoalStatus::Proposed, 10).unwrap();
    assert_eq!(proposed[0].title, "A");
    assert_eq!(proposed[1].title, "B");
    assert_eq!(proposed[2].title, "C");
}

// ===========================================================================
// FileBackedGoalStore — round-trip, filtering, upsert
// ===========================================================================

#[test]
fn file_backed_store_put_and_list_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("goals.json");
    let store = FileBackedGoalStore::try_new(&path).unwrap();

    store
        .put(make_record("Roundtrip", GoalStatus::Active, 1))
        .unwrap();

    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Roundtrip");
}

#[test]
fn file_backed_store_persists_to_disk() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("goals.json");

    // Write with one store instance.
    {
        let store = FileBackedGoalStore::try_new(&path).unwrap();
        store
            .put(make_record("Persistent", GoalStatus::Active, 1))
            .unwrap();
    }

    // Read with a new store instance to confirm on-disk persistence.
    let store2 = FileBackedGoalStore::try_new(&path).unwrap();
    let listed = store2.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Persistent");
}

#[test]
fn file_backed_store_upsert_replaces_by_slug() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("goals.json");
    let store = FileBackedGoalStore::try_new(&path).unwrap();

    store
        .put(make_record("Upsert Me", GoalStatus::Proposed, 5))
        .unwrap();
    store
        .put(make_record("Upsert Me", GoalStatus::Active, 1))
        .unwrap();

    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 1, "upsert should not duplicate");
    assert_eq!(listed[0].status, GoalStatus::Active);
    assert_eq!(listed[0].priority, 1);
}

#[test]
fn file_backed_store_top_goals_by_status_filters() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("goals.json");
    let store = FileBackedGoalStore::try_new(&path).unwrap();

    store
        .put(make_record("Active One", GoalStatus::Active, 1))
        .unwrap();
    store
        .put(make_record("Proposed One", GoalStatus::Proposed, 1))
        .unwrap();
    store
        .put(make_record("Active Two", GoalStatus::Active, 2))
        .unwrap();

    let active = store.top_goals_by_status(GoalStatus::Active, 10).unwrap();
    assert_eq!(active.len(), 2);
    assert!(active.iter().all(|r| r.status == GoalStatus::Active));
}

#[test]
fn file_backed_store_active_top_goals_respects_limit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("goals.json");
    let store = FileBackedGoalStore::try_new(&path).unwrap();

    store.put(make_record("A", GoalStatus::Active, 1)).unwrap();
    store.put(make_record("B", GoalStatus::Active, 2)).unwrap();
    store.put(make_record("C", GoalStatus::Active, 3)).unwrap();

    let top2 = store.active_top_goals(2).unwrap();
    assert_eq!(top2.len(), 2);
    assert_eq!(top2[0].title, "A");
    assert_eq!(top2[1].title, "B");
}

#[test]
fn file_backed_store_limit_zero_returns_empty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("goals.json");
    let store = FileBackedGoalStore::try_new(&path).unwrap();

    store
        .put(make_record("Goal", GoalStatus::Active, 1))
        .unwrap();

    let empty = store.active_top_goals(0).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn file_backed_store_cross_instance_visibility() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("goals.json");

    let store1 = FileBackedGoalStore::try_new(&path).unwrap();
    let store2 = FileBackedGoalStore::try_new(&path).unwrap();

    // Write through store1.
    store1
        .put(make_record("Cross-visible", GoalStatus::Active, 1))
        .unwrap();

    // store2 should see the record (reload from disk on list).
    let listed = store2.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Cross-visible");
}

#[test]
fn file_backed_store_path_accessor() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("goals.json");
    let store = FileBackedGoalStore::try_new(&path).unwrap();
    assert_eq!(store.path(), path);
}

// ===========================================================================
// seed_default_goals — additional coverage
// ===========================================================================

#[test]
fn seed_default_goals_all_slugs_are_non_empty() {
    let store = InMemoryGoalStore::try_default().expect("store");
    let seeded = seed_default_goals(&store).expect("seed");
    for record in &seeded {
        assert!(
            !record.slug.is_empty(),
            "slug must not be empty for goal: {}",
            record.title
        );
    }
}

#[test]
fn seed_default_goals_all_slugs_within_max_len() {
    let store = InMemoryGoalStore::try_default().expect("store");
    let seeded = seed_default_goals(&store).expect("seed");
    for record in &seeded {
        assert!(
            record.slug.len() <= GOAL_SLUG_MAX_LEN,
            "slug too long ({} chars) for goal: {}",
            record.slug.len(),
            record.title
        );
    }
}

#[test]
fn seed_default_goals_all_titles_non_empty() {
    let store = InMemoryGoalStore::try_default().expect("store");
    let seeded = seed_default_goals(&store).expect("seed");
    for record in &seeded {
        assert!(
            !record.title.is_empty(),
            "title must not be empty for slug: {}",
            record.slug
        );
    }
}

#[test]
fn seed_default_goals_all_rationales_non_empty() {
    let store = InMemoryGoalStore::try_default().expect("store");
    let seeded = seed_default_goals(&store).expect("seed");
    for record in &seeded {
        assert!(
            !record.rationale.is_empty(),
            "rationale must not be empty for goal: {}",
            record.title
        );
    }
}

#[test]
fn seed_default_goals_session_phase_is_persistence() {
    let store = InMemoryGoalStore::try_default().expect("store");
    let seeded = seed_default_goals(&store).expect("seed");
    for record in &seeded {
        assert_eq!(
            record.updated_in,
            SessionPhase::Persistence,
            "seed goals should be recorded in Persistence phase"
        );
    }
}
