mod seed;
mod store;
mod types;

// Re-export all public items so `crate::goals::X` still works.
pub use seed::seed_default_goals;
pub use store::{FileBackedGoalStore, GoalStore, InMemoryGoalStore};
pub use types::{GoalRecord, GoalStatus, GoalUpdate, goal_slug};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionId, SessionPhase};

    fn make_record(title: &str, status: GoalStatus, priority: u8) -> GoalRecord {
        let update =
            GoalUpdate::new(title, "test rationale", status, priority).expect("valid update");
        GoalRecord::from_update(
            update,
            "test-owner",
            SessionId::parse("session-00000000-0000-0000-0000-000000000000")
                .expect("valid session id"),
            SessionPhase::Persistence,
        )
        .expect("valid record")
    }

    #[test]
    fn goal_slug_re_exported_and_works() {
        assert_eq!(goal_slug("Hello World"), "hello-world");
    }

    #[test]
    fn goal_status_parse_all_variants() {
        assert_eq!(GoalStatus::parse("proposed"), Some(GoalStatus::Proposed));
        assert_eq!(GoalStatus::parse("ACTIVE"), Some(GoalStatus::Active));
        assert_eq!(GoalStatus::parse("paused"), Some(GoalStatus::Paused));
        assert_eq!(GoalStatus::parse("completed"), Some(GoalStatus::Completed));
        assert_eq!(GoalStatus::parse("unknown"), None);
    }

    #[test]
    fn goal_status_is_active() {
        assert!(GoalStatus::Active.is_active());
        assert!(!GoalStatus::Proposed.is_active());
        assert!(!GoalStatus::Paused.is_active());
        assert!(!GoalStatus::Completed.is_active());
    }

    #[test]
    fn in_memory_store_put_and_list() {
        let store = InMemoryGoalStore::try_default().expect("store should create");
        let record = make_record("Test Goal", GoalStatus::Active, 1);
        store.put(record.clone()).unwrap();

        let all = store.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "Test Goal");
    }

    #[test]
    fn goal_record_concise_label_format() {
        let record = make_record("Ship feature X", GoalStatus::Active, 2);
        let label = record.concise_label();
        assert!(
            label.contains("p2"),
            "label should contain priority: {label}"
        );
        assert!(
            label.contains("[active]"),
            "label should contain status: {label}"
        );
        assert!(
            label.contains("Ship feature X"),
            "label should contain title: {label}"
        );
    }

    #[test]
    fn seed_default_goals_populates_empty_store() {
        let store = InMemoryGoalStore::try_default().expect("store");
        let seeded = seed_default_goals(&store).expect("seed");
        assert_eq!(seeded.len(), 5);
    }
}
