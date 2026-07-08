//! Integration tests for Simard #3125 — identity as a first-class *write
//! posture* so a read-only OBSERVER identity (e.g. Crocutus observing the
//! hyenas AzDO repos) is enforced at the COGNITION level, not only at the
//! write-primitive chokepoint.
//!
//! These tests are written FIRST (TDD) and define the public contract the
//! implementation must satisfy. They exercise the crate's public boundary:
//!   * `simard::identity::{WriteAuthority, SeedGoal, IdentityPosture, ResolvedPosture}`
//!   * `simard::identity::{IdentityManifest, FileIdentityLoader, BuiltinIdentityLoader}`
//!   * `simard::{GoalBoard, seed_default_board, seed_identity_board, DEFAULT_SEED_GOALS}`
//!
//! Acceptance criteria mapped:
//!   AC1  no-identity / read-write identity ⇒ Simard's 5 named default goals,
//!        read-write authority (no behavior change for Simard herself).
//!   AC2  a read-only identity's seed goals OVERRIDE the baked-in defaults.
//!   AC4  seed goals / observations are scoped to the identity's target repo
//!        set, never `rysweet/Simard`.
//!   AC5  an undetermined posture fails CLOSED (read-only / no spawn).
//!   AC6  an absent `write_authority` parses to `ReadWrite`.
//!
//! NOTE for the implementer: the new identity types must be re-exported from
//! `src/identity/mod.rs` (alongside `MemoryPolicy`/`OperatingMode`) and
//! `seed_identity_board` re-exported at the crate root (alongside
//! `seed_default_board`). If a path below does not resolve, add the missing
//! re-export — do not weaken the test.

use std::collections::BTreeSet;
use std::fs;

use tempfile::TempDir;

use simard::identity::{
    BuiltinIdentityLoader, FileIdentityLoader, IdentityLoadRequest, IdentityLoader,
    IdentityManifest, IdentityPosture, ManifestContract, OperatingMode, ResolvedPosture, SeedGoal,
    WriteAuthority,
};
use simard::metadata::{Freshness, Provenance};
use simard::{BaseTypeId, DEFAULT_SEED_GOALS, GoalBoard};

// ── shared helpers ──────────────────────────────────────────────────────────

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

fn base_manifest(name: &str, mode: OperatingMode) -> IdentityManifest {
    IdentityManifest::new(
        name,
        "0.1.0",
        vec![],
        vec![BaseTypeId::new("local-harness")],
        BTreeSet::new(),
        mode,
        simard::identity::MemoryPolicy::default(),
        test_contract(),
    )
    .unwrap()
}

fn seed_goal(priority: u32, title: &str, repo: Option<&str>) -> SeedGoal {
    SeedGoal {
        priority,
        title: title.to_string(),
        description: format!("OBSERVE ONLY: {title}"),
        repo: repo.map(str::to_string),
    }
}

fn write_identity_toml(dir: &std::path::Path, content: &str) {
    fs::write(dir.join("identity.toml"), content).unwrap();
}

fn builtin(name: &str) -> IdentityManifest {
    BuiltinIdentityLoader
        .load(&IdentityLoadRequest::new(name, "0.1.0", test_contract()))
        .unwrap()
}

// ── WriteAuthority: default + serde (kebab-case) ─────────────────────────────

#[test]
fn write_authority_defaults_to_read_write() {
    // Simard herself is unchanged: the default posture authorizes writes.
    assert_eq!(WriteAuthority::default(), WriteAuthority::ReadWrite);
}

#[test]
fn write_authority_serializes_kebab_case() {
    assert_eq!(
        serde_json::to_string(&WriteAuthority::ReadOnly).unwrap(),
        "\"read-only\""
    );
    assert_eq!(
        serde_json::to_string(&WriteAuthority::ReadWrite).unwrap(),
        "\"read-write\""
    );
}

#[test]
fn write_authority_deserializes_kebab_case() {
    let ro: WriteAuthority = serde_json::from_str("\"read-only\"").unwrap();
    let rw: WriteAuthority = serde_json::from_str("\"read-write\"").unwrap();
    assert_eq!(ro, WriteAuthority::ReadOnly);
    assert_eq!(rw, WriteAuthority::ReadWrite);
}

// ── ResolvedPosture: boot-time fail-closed resolution (AC1 + AC5) ────────────

#[test]
fn resolved_posture_no_identity_is_read_write() {
    // AC1: no identity present is a DETERMINED state ⇒ Simard default.
    assert_eq!(
        ResolvedPosture::None.write_authority(),
        WriteAuthority::ReadWrite
    );
}

#[test]
fn resolved_posture_undetermined_fails_closed_to_read_only() {
    // AC5: an identity IS present but its posture cannot be resolved
    // (load/parse/threading gap) ⇒ fail CLOSED (read-only / no spawn).
    assert_eq!(
        ResolvedPosture::Undetermined.write_authority(),
        WriteAuthority::ReadOnly
    );
}

#[test]
fn resolved_posture_identity_uses_declared_authority() {
    let ro = IdentityPosture {
        write_authority: WriteAuthority::ReadOnly,
        targets: vec!["hyenas/repo-a".to_string()],
        seed_goals: vec![],
    };
    assert_eq!(
        ResolvedPosture::Identity(ro).write_authority(),
        WriteAuthority::ReadOnly
    );

    let rw = IdentityPosture {
        write_authority: WriteAuthority::ReadWrite,
        targets: vec![],
        seed_goals: vec![],
    };
    assert_eq!(
        ResolvedPosture::Identity(rw).write_authority(),
        WriteAuthority::ReadWrite
    );
}

// ── IdentityManifest posture fields + with_posture builder ───────────────────

#[test]
fn manifest_new_defaults_to_read_write_and_empty_scope() {
    // AC1: constructing a manifest the old way yields the Simard-unchanged
    // posture — read-write, no targets, no seed goals.
    let m = base_manifest("plain", OperatingMode::Engineer);
    assert_eq!(m.write_authority, WriteAuthority::ReadWrite);
    assert!(m.targets.is_empty());
    assert!(m.seed_goals.is_empty());
}

#[test]
fn with_posture_sets_read_only_scope_and_seed_goals() {
    let m = base_manifest("crocutus-observer", OperatingMode::Curator)
        .with_posture(
            WriteAuthority::ReadOnly,
            vec!["hyenas/repo-a".to_string(), "hyenas/repo-b".to_string()],
            vec![
                seed_goal(80, "Observe branch hygiene", Some("hyenas/repo-a")),
                seed_goal(70, "Observe CODEOWNERS", Some("hyenas/repo-b")),
            ],
        )
        .unwrap();
    assert_eq!(m.write_authority, WriteAuthority::ReadOnly);
    assert_eq!(m.targets.len(), 2);
    assert_eq!(m.seed_goals.len(), 2);
    assert!(m.seed_goals.iter().all(|g| {
        g.repo
            .as_deref()
            .map(|r| m.targets.iter().any(|t| t == r))
            .unwrap_or(false)
    }));
}

#[test]
fn with_posture_read_only_rejects_seed_goal_outside_targets() {
    // Fail-closed: a read-only identity must NOT be able to seed a goal that
    // escapes its declared target set (never silently scoped to Simard).
    let result = base_manifest("crocutus-observer", OperatingMode::Curator).with_posture(
        WriteAuthority::ReadOnly,
        vec!["hyenas/repo-a".to_string()],
        vec![seed_goal(80, "Escapes scope", Some("someone-else/repo"))],
    );
    assert!(
        result.is_err(),
        "read-only seed goal with an out-of-targets repo must fail closed"
    );
}

#[test]
fn with_posture_read_only_rejects_unscoped_seed_goal() {
    // A read-only seed goal with no repo would default to Simard's own repo —
    // exactly the bug #3125 fixes. It must fail closed instead.
    let result = base_manifest("crocutus-observer", OperatingMode::Curator).with_posture(
        WriteAuthority::ReadOnly,
        vec!["hyenas/repo-a".to_string()],
        vec![seed_goal(80, "Unscoped", None)],
    );
    assert!(
        result.is_err(),
        "read-only seed goal with repo=None must fail closed (no implicit Simard scope)"
    );
}

// ── IdentityPosture::from_manifest ───────────────────────────────────────────

#[test]
fn identity_posture_from_manifest_reads_all_fields() {
    let m = base_manifest("crocutus-observer", OperatingMode::Curator)
        .with_posture(
            WriteAuthority::ReadOnly,
            vec!["hyenas/repo-a".to_string()],
            vec![seed_goal(90, "Observe LICENSE", Some("hyenas/repo-a"))],
        )
        .unwrap();
    let posture = IdentityPosture::from_manifest(&m);
    assert_eq!(posture.write_authority, WriteAuthority::ReadOnly);
    assert_eq!(posture.targets, vec!["hyenas/repo-a".to_string()]);
    assert_eq!(posture.seed_goals.len(), 1);
    assert_eq!(posture.seed_goals[0].repo.as_deref(), Some("hyenas/repo-a"));
}

// ── Builtin identities are unchanged (AC1) ───────────────────────────────────

#[test]
fn builtin_identities_are_read_write_with_empty_scope() {
    for name in [
        "simard-engineer",
        "simard-meeting",
        "simard-gym",
        "simard-goal-curator",
        "simard-improvement-curator",
    ] {
        let m = builtin(name);
        assert_eq!(
            m.write_authority,
            WriteAuthority::ReadWrite,
            "{name} must remain read-write"
        );
        assert!(m.targets.is_empty(), "{name} must have no target scope");
        assert!(
            m.seed_goals.is_empty(),
            "{name} must not declare identity seed goals"
        );
    }
}

// ── Composition: most-restrictive authority, union targets, concat seeds ─────

#[test]
fn compose_takes_most_restrictive_authority_and_unions_scope() {
    let rw = base_manifest("comp-rw", OperatingMode::Engineer)
        .with_posture(
            WriteAuthority::ReadWrite,
            vec!["owner/rw".to_string()],
            vec![seed_goal(10, "rw goal", Some("owner/rw"))],
        )
        .unwrap();
    let ro = base_manifest("comp-ro", OperatingMode::Curator)
        .with_posture(
            WriteAuthority::ReadOnly,
            vec!["owner/ro".to_string()],
            vec![seed_goal(20, "ro goal", Some("owner/ro"))],
        )
        .unwrap();

    let composed = IdentityManifest::compose(
        "composite",
        "1.0",
        vec![rw, ro],
        OperatingMode::Engineer,
        test_contract(),
    )
    .unwrap();

    // Most restrictive wins: one read-only component makes the whole identity
    // read-only (defense in depth — a composed identity can never be more
    // permissive than its least-trusted part).
    assert_eq!(composed.write_authority, WriteAuthority::ReadOnly);
    // Targets are the union.
    assert!(composed.targets.iter().any(|t| t == "owner/rw"));
    assert!(composed.targets.iter().any(|t| t == "owner/ro"));
    // Seed goals are concatenated.
    assert_eq!(composed.seed_goals.len(), 2);
}

// ── File loader: read-only observer identity from identity.toml ──────────────

const OBSERVER_TOML: &str = r#"
[package]
name = "crocutus"
version = "0.1.0"

[[identities]]
name = "crocutus-observer"
default_mode = "curator"
supported_base_types = ["local-harness"]
required_capabilities = ["prompt-assets", "memory"]
write_authority = "read-only"
targets = ["hyenas/repo-a", "hyenas/repo-b"]

[[identities.seed_goals]]
priority = 80
title = "Observe branch hygiene"
description = "OBSERVE ONLY: branch hygiene / CODEOWNERS / LICENSE / dependabot / large blobs"
repo = "hyenas/repo-a"

[[identities.seed_goals]]
priority = 70
title = "Observe repo hygiene"
description = "OBSERVE ONLY: CODEOWNERS, LICENSE, dependabot"
repo = "hyenas/repo-b"
"#;

#[test]
fn file_loader_parses_read_only_observer_posture() {
    let prompt_root = TempDir::new().unwrap();
    let identity_dir = prompt_root.path().join("crocutus");
    fs::create_dir_all(&identity_dir).unwrap();
    write_identity_toml(&identity_dir, OBSERVER_TOML);

    let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
    let m = loader
        .load(&IdentityLoadRequest::new(
            "crocutus-observer",
            "0.1.0",
            test_contract(),
        ))
        .unwrap();

    assert_eq!(m.write_authority, WriteAuthority::ReadOnly);
    assert_eq!(
        m.targets,
        vec!["hyenas/repo-a".to_string(), "hyenas/repo-b".to_string()]
    );
    assert_eq!(m.seed_goals.len(), 2);
    // AC4: every seed goal is scoped to a target repo, never rysweet/Simard.
    assert!(m.seed_goals.iter().all(|g| {
        g.repo
            .as_deref()
            .map(|r| m.targets.iter().any(|t| t == r))
            .unwrap_or(false)
    }));
    assert!(
        m.seed_goals
            .iter()
            .all(|g| g.repo.as_deref() != Some("Simard"))
    );
}

const OBSERVER_TOML_ESCAPED_SCOPE: &str = r#"
[package]
name = "crocutus"
version = "0.1.0"

[[identities]]
name = "crocutus-observer"
default_mode = "curator"
supported_base_types = ["local-harness"]
required_capabilities = ["prompt-assets", "memory"]
write_authority = "read-only"
targets = ["hyenas/repo-a"]

[[identities.seed_goals]]
priority = 80
title = "Escapes scope"
description = "OBSERVE ONLY"
repo = "rysweet/Simard"
"#;

#[test]
fn file_loader_read_only_seed_goal_outside_targets_fails_closed() {
    // The core bug #3125 guards against: a read-only observer must never seed
    // a goal against rysweet/Simard (or any repo outside its targets).
    let prompt_root = TempDir::new().unwrap();
    let identity_dir = prompt_root.path().join("crocutus");
    fs::create_dir_all(&identity_dir).unwrap();
    write_identity_toml(&identity_dir, OBSERVER_TOML_ESCAPED_SCOPE);

    let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
    let err = loader
        .load(&IdentityLoadRequest::new(
            "crocutus-observer",
            "0.1.0",
            test_contract(),
        ))
        .unwrap_err();
    assert!(
        matches!(err, simard::SimardError::IdentityTomlParseError { .. }),
        "out-of-targets read-only seed goal must be a hard parse error, got: {err:?}"
    );
}

const RW_TOML_NO_AUTHORITY: &str = r#"
[package]
name = "plain"
version = "0.1.0"

[[identities]]
name = "plain-engineer"
default_mode = "engineer"
supported_base_types = ["local-harness"]
required_capabilities = ["prompt-assets", "memory"]
"#;

#[test]
fn file_loader_absent_write_authority_defaults_to_read_write() {
    // AC6: an identity.toml with no `write_authority` parses to ReadWrite and
    // carries no target scope / seed goals (behaviorally identical to today).
    let prompt_root = TempDir::new().unwrap();
    let identity_dir = prompt_root.path().join("plain");
    fs::create_dir_all(&identity_dir).unwrap();
    write_identity_toml(&identity_dir, RW_TOML_NO_AUTHORITY);

    let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
    let m = loader
        .load(&IdentityLoadRequest::new(
            "plain-engineer",
            "0.1.0",
            test_contract(),
        ))
        .unwrap();
    assert_eq!(m.write_authority, WriteAuthority::ReadWrite);
    assert!(m.targets.is_empty());
    assert!(m.seed_goals.is_empty());
}

const TOML_UNKNOWN_FIELD: &str = r#"
[package]
name = "plain"
version = "0.1.0"

[[identities]]
name = "plain-engineer"
default_mode = "engineer"
write_authority = "read-write"
bogus_new_field = true
"#;

#[test]
fn file_loader_still_rejects_unknown_identity_fields() {
    // AC8: the new fields are additive `#[serde(default)]`; `deny_unknown_fields`
    // must stay intact so typos remain hard errors.
    let prompt_root = TempDir::new().unwrap();
    let identity_dir = prompt_root.path().join("plain");
    fs::create_dir_all(&identity_dir).unwrap();
    write_identity_toml(&identity_dir, TOML_UNKNOWN_FIELD);

    let loader = FileIdentityLoader::new(&identity_dir, prompt_root.path());
    let err = loader
        .load(&IdentityLoadRequest::new(
            "plain-engineer",
            "0.1.0",
            test_contract(),
        ))
        .unwrap_err();
    assert!(
        matches!(err, simard::SimardError::IdentityTomlParseError { .. }),
        "unknown identity field must remain a parse error, got: {err:?}"
    );
}

// ── Seeding: identity seeds override defaults; defaults unchanged (AC1/AC2) ───

#[test]
fn seed_default_board_still_produces_the_five_named_defaults() {
    // AC1: Simard with no identity keeps her exact 5 baked-in seed goals.
    let mut board = GoalBoard::new();
    let n = simard::seed_default_board(&mut board);
    assert_eq!(n, 5);
    assert_eq!(board.active.len(), 5);

    let descriptions: Vec<&str> = board
        .active
        .iter()
        .map(|g| g.description.as_str())
        .collect();
    for (_priority, _title, description, _repo) in DEFAULT_SEED_GOALS {
        assert!(
            descriptions.contains(&description),
            "default board must include the seeded description: {description}"
        );
    }
    // None of Simard's defaults are scoped to a hyenas observer target.
    assert!(
        board
            .active
            .iter()
            .all(|g| g.repo.as_deref() != Some("hyenas/repo-a"))
    );
}

#[test]
fn seed_identity_board_scopes_goals_to_targets_not_simard() {
    // AC2 + AC4: a read-only identity seeds ITS OWN goals, scoped to its
    // target repos — replacing Simard's defaults, never touching rysweet/Simard.
    let seeds = vec![
        seed_goal(80, "Observe branch hygiene", Some("hyenas/repo-a")),
        seed_goal(70, "Observe CODEOWNERS", Some("hyenas/repo-b")),
    ];
    let mut board = GoalBoard::new();
    let n = simard::seed_identity_board(&mut board, &seeds);

    assert_eq!(n, 2);
    assert_eq!(board.active.len(), 2);

    // Every seeded goal is scoped to one of the identity's targets.
    let repos: Vec<&str> = board
        .active
        .iter()
        .filter_map(|g| g.repo.as_deref())
        .collect();
    assert!(repos.contains(&"hyenas/repo-a"));
    assert!(repos.contains(&"hyenas/repo-b"));
    assert!(
        board
            .active
            .iter()
            .all(|g| g.repo.as_deref() != Some("Simard"))
    );
    // Observe-only: proposed/seeded goals are unassigned (no engineer dispatch).
    assert!(board.active.iter().all(|g| g.assigned_to.is_none()));

    // The seeded goals are the identity's, NOT Simard's defaults.
    let descriptions: Vec<&str> = board
        .active
        .iter()
        .map(|g| g.description.as_str())
        .collect();
    for (_priority, _title, default_desc, _repo) in DEFAULT_SEED_GOALS {
        assert!(
            !descriptions.contains(&default_desc),
            "identity-seeded board must NOT contain Simard's default: {default_desc}"
        );
    }
}

#[test]
fn seed_identity_board_is_noop_on_non_empty_board() {
    // Mirrors seed_default_board: never clobber an already-populated board.
    let mut board = GoalBoard::new();
    let _ =
        simard::seed_identity_board(&mut board, &[seed_goal(80, "first", Some("hyenas/repo-a"))]);
    let n = simard::seed_identity_board(
        &mut board,
        &[seed_goal(70, "second", Some("hyenas/repo-b"))],
    );
    assert_eq!(n, 0, "seeding a non-empty board must be a no-op");
    assert_eq!(board.active.len(), 1);
}
