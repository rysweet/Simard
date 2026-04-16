use crate::error::SimardResult;
use crate::session::{SessionId, SessionPhase};

use super::{GoalRecord, GoalStatus, GoalStore, GoalUpdate};

/// Seed the goal store with 5 default starter goals if the store is empty.
/// Returns the seeded goals, or an empty vec if the store already had goals.
pub fn seed_default_goals(store: &dyn GoalStore) -> SimardResult<Vec<GoalRecord>> {
    let existing = store.list()?;
    if !existing.is_empty() {
        return Ok(Vec::new());
    }

    let session_id = SessionId::parse("session-00000000-0000-0000-0000-000000000000")
        .expect("static seed session id");

    let mut seeded = Vec::with_capacity(crate::goal_curation::DEFAULT_SEED_GOALS.len());
    for (priority, title, description) in crate::goal_curation::DEFAULT_SEED_GOALS {
        let update = GoalUpdate::new(
            title,
            description,
            GoalStatus::Active,
            u8::try_from(priority).unwrap_or(u8::MAX),
        )?;
        let record = GoalRecord::from_update(
            update,
            "simard-seed",
            session_id.clone(),
            SessionPhase::Persistence,
        )?;
        store.put(record.clone())?;
        seeded.push(record);
    }

    Ok(seeded)
}

#[cfg(test)]
mod tests {
    use crate::goals::{GoalRecord, GoalStatus, GoalStore, GoalUpdate, InMemoryGoalStore};
    use crate::session::{SessionId, SessionPhase};

    use super::seed_default_goals;

    fn goal_record(title: &str, status: GoalStatus, priority: u8) -> GoalRecord {
        GoalRecord::from_update(
            GoalUpdate::new(title, "keep Simard pointed at user goals", status, priority)
                .expect("goal update should be valid"),
            "simard-goal-curator",
            SessionId::parse("session-018f1f7e-4c5d-7b2a-8f10-b5c0d4f7b123")
                .expect("session id should parse"),
            SessionPhase::Persistence,
        )
        .expect("goal record should be valid")
    }

    #[test]
    fn seed_default_goals_creates_five_when_empty() {
        let store = InMemoryGoalStore::try_default().expect("store should create");
        let seeded = seed_default_goals(&store).expect("seeding should succeed");
        assert_eq!(seeded.len(), 5);

        let all = store.list().expect("list should work");
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn seed_default_goals_all_active_status() {
        let store = InMemoryGoalStore::try_default().expect("store should create");
        let seeded = seed_default_goals(&store).expect("seeding should succeed");
        for record in &seeded {
            assert_eq!(
                record.status,
                GoalStatus::Active,
                "goal '{}' should be active",
                record.title
            );
        }
    }

    #[test]
    fn seed_default_goals_priorities_are_1_through_5() {
        let store = InMemoryGoalStore::try_default().expect("store should create");
        let seeded = seed_default_goals(&store).expect("seeding should succeed");
        let priorities: Vec<u8> = seeded.iter().map(|g| g.priority).collect();
        assert_eq!(priorities, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn seed_default_goals_noop_when_store_has_goals() {
        let store = InMemoryGoalStore::try_default().expect("store should create");
        store
            .put(goal_record("Existing goal", GoalStatus::Active, 1))
            .expect("put should work");

        let seeded = seed_default_goals(&store).expect("seeding should succeed");
        assert!(seeded.is_empty(), "should not seed when store is non-empty");

        let all = store.list().expect("list should work");
        assert_eq!(
            all.len(),
            1,
            "store should still have only the original goal"
        );
    }

    #[test]
    fn seed_default_goals_is_idempotent() {
        let store = InMemoryGoalStore::try_default().expect("store should create");
        seed_default_goals(&store).expect("first seed should succeed");
        let second = seed_default_goals(&store).expect("second seed should succeed");
        assert!(second.is_empty(), "second call should be a no-op");

        let all = store.list().expect("list should work");
        assert_eq!(all.len(), 5, "should still have exactly 5 goals");
    }

    #[test]
    fn seed_default_goals_owner_is_simard_seed() {
        let store = InMemoryGoalStore::try_default().expect("store should create");
        let seeded = seed_default_goals(&store).expect("seeding should succeed");
        for record in &seeded {
            assert_eq!(record.owner_identity, "simard-seed");
        }
    }
}
