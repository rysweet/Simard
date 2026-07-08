//! Integration tests for #3125 — identity-scoped cognition.
//!
//! These prove the two acceptance criteria at the public API level:
//!
//! (a) **Simard is unchanged by default.** With no identity (or a `full`
//!     identity that declares no seed goals) the five `DEFAULT_SEED_GOALS` seed
//!     exactly as before and the deterministic spawn rail permits engineer
//!     dispatch.
//!
//! (b) **A read-only identity shapes cognition.** It seeds its OWN goals
//!     (overriding the defaults), scopes them to its target repos (never
//!     `rysweet/Simard`), and its write-authority posture makes the observe-only
//!     rail deny engineer dispatch (fail-closed).
//!
//! The Act-phase dispatch rail itself (`dispatch_spawn_engineer` taking the
//! observe-only branch without ever consulting the brain) is proven by the
//! in-crate unit tests in `src/ooda_actions/advance_goal/spawn.rs`; that
//! function is `pub(crate)` and cannot be reached from an integration test.

use simard::goal_curation::{
    DEFAULT_SEED_GOALS, GoalBoard, default_seed_goals, resolve_seed_goals,
    seed_board_from_seed_goals, seed_default_board,
};
use simard::ooda_loop::IdentityCognition;
use simard::{
    BuiltinIdentityLoader, Freshness, IdentityAuthority, IdentityLoadRequest, IdentityLoader,
    IdentityManifest, ManifestContract, Provenance, SeedGoal, WritePosture,
};

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "test::entrypoint",
        "a -> b",
        vec!["key:value".to_string()],
        Provenance::new("test-source", "test-locator"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

/// A read-only observer identity (Crocutus-shaped) built through the public
/// builder API — the same surface `FileIdentityLoader` produces from
/// `identity.toml`.
fn crocutus_manifest() -> IdentityManifest {
    IdentityManifest::new(
        "crocutus",
        "0.1.0",
        vec![],
        vec![],
        std::collections::BTreeSet::new(),
        simard::OperatingMode::Engineer,
        simard::MemoryPolicy::default(),
        test_contract(),
    )
    .unwrap()
    .with_seed_goals(vec![
        SeedGoal::new(
            1,
            "Observe hyenas repo health",
            "Assess branch hygiene, CODEOWNERS, LICENSE, dependabot, large blobs. OBSERVE ONLY.",
            Some("hyenas".to_string()),
        ),
        SeedGoal::new(
            2,
            "Articulate repo-hygiene backlog",
            "Turn observations into prioritized, target-scoped repo-hygiene goals.",
            Some("hyenas".to_string()),
        ),
    ])
    .with_authority(IdentityAuthority::read_only())
}

// ─────────────────────────── (a) Simard unchanged ───────────────────────────

#[test]
fn default_seed_goals_mirror_the_baked_in_defaults() {
    let goals = default_seed_goals();
    assert_eq!(goals.len(), 5, "Simard keeps her five defaults");
    assert_eq!(goals.len(), DEFAULT_SEED_GOALS.len());
    for (goal, (priority, title, description, repo)) in goals.iter().zip(DEFAULT_SEED_GOALS.iter())
    {
        assert_eq!(goal.priority, *priority);
        assert_eq!(goal.title, *title);
        assert_eq!(goal.description, *description);
        assert_eq!(goal.repo.as_deref(), *repo);
    }
}

#[test]
fn resolve_seed_goals_falls_through_to_defaults_when_identity_declares_none() {
    // Empty identity seed goals => Simard's defaults, unchanged.
    assert_eq!(resolve_seed_goals(&[]), default_seed_goals());
}

#[test]
fn identity_seeding_from_defaults_matches_seed_default_board() {
    // The identity-shaped seeding path, when fed the defaults, is byte-for-byte
    // the historical `seed_default_board` result: same ids, priorities, repos.
    let mut baseline = GoalBoard::new();
    let n_baseline = seed_default_board(&mut baseline);

    let mut via_identity = GoalBoard::new();
    let n_identity = seed_board_from_seed_goals(&mut via_identity, &resolve_seed_goals(&[]));

    assert_eq!(n_baseline, 5);
    assert_eq!(n_identity, 5);
    assert_eq!(via_identity.active.len(), baseline.active.len());
    for (a, b) in via_identity.active.iter().zip(baseline.active.iter()) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.priority, b.priority);
        assert_eq!(a.description, b.description);
        assert_eq!(a.repo, b.repo);
    }
}

#[test]
fn no_identity_cognition_permits_spawn() {
    let cognition = IdentityCognition::default();
    assert!(cognition.authority.is_none());
    assert!(cognition.seed_goals.is_empty());
    assert!(cognition.target_repos.is_empty());
    // No identity => `full` => the deterministic rail permits engineer dispatch.
    assert!(cognition.permits_spawn());
}

#[test]
fn builtin_simard_engineer_identity_is_full_and_declares_no_seed_goals() {
    let loader = BuiltinIdentityLoader;
    let manifest = loader
        .load(&IdentityLoadRequest::new(
            "simard-engineer",
            "0.1.0",
            test_contract(),
        ))
        .unwrap();
    assert_eq!(manifest.authority.posture, WritePosture::Full);
    assert!(manifest.authority.permits_spawn());
    assert!(
        manifest.seed_goals.is_empty(),
        "Simard's built-in identity declares no seed-goal override"
    );
    assert!(manifest.target_repos.is_empty());

    let cognition = IdentityCognition::from_manifest(&manifest);
    assert!(
        cognition.permits_spawn(),
        "a full built-in identity must still dispatch engineers"
    );
    // With no seed-goal override, cold-start seeding uses the five defaults.
    assert_eq!(
        resolve_seed_goals(&cognition.seed_goals),
        default_seed_goals()
    );
}

// ────────────────────── (b) read-only identity shapes cognition ─────────────

#[test]
fn read_only_identity_seeds_its_own_target_scoped_goals() {
    let crocutus = crocutus_manifest();
    let cognition = IdentityCognition::from_manifest(&crocutus);

    // Its own goals OVERRIDE Simard's defaults (no merge).
    let resolved = resolve_seed_goals(&cognition.seed_goals);
    assert_eq!(resolved.len(), 2, "override replaces the 5 defaults with 2");
    assert_eq!(resolved[0].title, "Observe hyenas repo health");

    // The override is distinct from Simard's defaults.
    let default_titles: Vec<&str> = DEFAULT_SEED_GOALS.iter().map(|(_, t, _, _)| *t).collect();
    for goal in &resolved {
        assert!(
            !default_titles.contains(&goal.title.as_str()),
            "observer goal '{}' must not be one of Simard's defaults",
            goal.title
        );
    }

    // Seeding a fresh board yields ONLY the hyenas-scoped observer goals.
    let mut board = GoalBoard::new();
    let n = seed_board_from_seed_goals(&mut board, &resolved);
    assert_eq!(n, 2);
    assert_eq!(board.active.len(), 2);
    for goal in &board.active {
        assert_eq!(
            goal.repo.as_deref(),
            Some("hyenas"),
            "every observer goal is scoped to the target repo, never rysweet/Simard"
        );
    }
}

#[test]
fn read_only_identity_target_scope_resolves_to_targets_not_simard() {
    let crocutus = crocutus_manifest();
    let cognition = IdentityCognition::from_manifest(&crocutus);
    assert_eq!(cognition.target_repos, vec!["hyenas".to_string()]);
    assert!(
        !cognition.target_repos.iter().any(|r| r.contains("Simard")),
        "target scope must never point at rysweet/Simard"
    );
}

#[test]
fn read_only_posture_denies_engineer_dispatch_fail_closed() {
    let crocutus = crocutus_manifest();
    assert_eq!(crocutus.authority.posture, WritePosture::ReadOnly);
    assert!(!crocutus.authority.permits_spawn());

    let cognition = IdentityCognition::from_manifest(&crocutus);
    assert_eq!(cognition.identity_name.as_deref(), Some("crocutus"));
    // The Act-phase cognition rail denies dispatch for this identity.
    assert!(
        !cognition.permits_spawn(),
        "a read-only identity must never dispatch a write-bearing engineer"
    );
}

#[test]
fn simard_and_read_only_identity_seed_divergent_boards() {
    // Same cold-start seeding site, two identities, two different boards —
    // the crux of #3125: identity shapes cognition.
    let mut simard_board = GoalBoard::new();
    seed_board_from_seed_goals(&mut simard_board, &resolve_seed_goals(&[]));

    let crocutus = crocutus_manifest();
    let mut crocutus_board = GoalBoard::new();
    seed_board_from_seed_goals(
        &mut crocutus_board,
        &resolve_seed_goals(&crocutus.seed_goals),
    );

    assert_eq!(simard_board.active.len(), 5);
    assert_eq!(crocutus_board.active.len(), 2);
    let simard_ids: Vec<&str> = simard_board.active.iter().map(|g| g.id.as_str()).collect();
    let crocutus_ids: Vec<&str> = crocutus_board
        .active
        .iter()
        .map(|g| g.id.as_str())
        .collect();
    assert!(
        crocutus_ids.iter().all(|id| !simard_ids.contains(id)),
        "the observer's goals are disjoint from Simard's defaults"
    );
}
