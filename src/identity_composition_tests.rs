//! Extended unit tests for the `identity_composition` module.
//!
//! Covers Display formatting, multi-subordinate compositions, zero-depth
//! subordinates, and field propagation through compose_identity.
//! No `skip_if_no_llm_provider` — every test here runs deterministically.

use super::*;
use crate::agent_roles::{AgentRole, identity_for_role};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_primary() -> crate::identity::IdentityManifest {
    identity_for_role(AgentRole::Engineer).expect("primary manifest should be valid")
}

fn test_sub(role: AgentRole, name_suffix: &str, depth: u32) -> SubordinateIdentity {
    let mut manifest = identity_for_role(role).expect("subordinate manifest should be valid");
    manifest.name = format!("{}-{name_suffix}", role.identity_name());
    SubordinateIdentity {
        manifest,
        role,
        max_depth: depth,
    }
}

// ===========================================================================
// SubordinateIdentity — Display
// ===========================================================================

#[test]
fn subordinate_identity_display_format() {
    let sub = test_sub(AgentRole::Reviewer, "alpha", 3);
    let display = sub.to_string();
    assert!(
        display.contains("role=reviewer"),
        "should contain role: {display}"
    );
    assert!(
        display.contains("depth=3"),
        "should contain depth: {display}"
    );
    assert!(
        display.contains(&sub.manifest.name),
        "should contain name: {display}"
    );
}

#[test]
fn subordinate_identity_display_zero_depth() {
    let sub = test_sub(AgentRole::GymRunner, "zero", 0);
    let display = sub.to_string();
    assert!(
        display.contains("depth=0"),
        "should show depth=0: {display}"
    );
}

// ===========================================================================
// CompositeIdentity — Display
// ===========================================================================

#[test]
fn composite_identity_display_no_subordinates() {
    let primary = test_primary();
    let composite = compose_identity(primary.clone(), vec![]).unwrap();
    let display = composite.to_string();
    assert!(
        display.starts_with("CompositeIdentity(primary="),
        "should start correctly: {display}"
    );
    assert!(
        !display.contains("subordinates="),
        "no subordinates section when empty: {display}"
    );
    assert!(display.ends_with(')'), "should end with ): {display}");
}

#[test]
fn composite_identity_display_with_subordinates() {
    let primary = test_primary();
    let sub = test_sub(AgentRole::Reviewer, "1", 2);
    let composite = compose_identity(primary, vec![sub]).unwrap();
    let display = composite.to_string();
    assert!(display.contains("subordinates=["));
    assert!(display.contains("role=reviewer"));
}

#[test]
fn composite_identity_display_multiple_subordinates_comma_separated() {
    let primary = test_primary();
    let sub1 = test_sub(AgentRole::Reviewer, "a", 1);
    let sub2 = test_sub(AgentRole::GymRunner, "b", 2);
    let composite = compose_identity(primary, vec![sub1, sub2]).unwrap();
    let display = composite.to_string();
    // Should have comma separation between subordinates
    assert!(
        display.contains(", "),
        "subordinates should be comma-separated: {display}"
    );
    assert!(display.contains("role=reviewer"));
    assert!(display.contains("role=gym-runner"));
}

// ===========================================================================
// compose_identity — multi-subordinate
// ===========================================================================

#[test]
fn compose_identity_three_subordinates_succeeds() {
    let primary = test_primary();
    let sub1 = test_sub(AgentRole::Reviewer, "1", 1);
    let sub2 = test_sub(AgentRole::GymRunner, "2", 2);
    let sub3 = test_sub(AgentRole::Facilitator, "3", 0);
    let composite = compose_identity(primary, vec![sub1, sub2, sub3]).unwrap();
    assert_eq!(composite.subordinates.len(), 3);
}

#[test]
fn compose_identity_preserves_subordinate_details() {
    let primary = test_primary();
    let sub = test_sub(AgentRole::Reviewer, "detail", 5);
    let sub_name = sub.manifest.name.clone();
    let composite = compose_identity(primary, vec![sub]).unwrap();
    assert_eq!(composite.subordinates[0].manifest.name, sub_name);
    assert_eq!(composite.subordinates[0].role, AgentRole::Reviewer);
    assert_eq!(composite.subordinates[0].max_depth, 5);
}

#[test]
fn compose_identity_same_role_different_names_succeeds() {
    let primary = test_primary();
    let sub1 = test_sub(AgentRole::Reviewer, "first", 1);
    let sub2 = test_sub(AgentRole::Reviewer, "second", 1);
    assert_ne!(
        sub1.manifest.name, sub2.manifest.name,
        "precondition: names must differ"
    );
    let composite = compose_identity(primary, vec![sub1, sub2]).unwrap();
    assert_eq!(composite.subordinates.len(), 2);
}

#[test]
fn compose_identity_zero_depth_subordinate_accepted() {
    let primary = test_primary();
    let sub = test_sub(AgentRole::GymRunner, "leaf", 0);
    let composite = compose_identity(primary, vec![sub]).unwrap();
    assert_eq!(composite.subordinates[0].max_depth, 0);
}

// ===========================================================================
// compose_identity — error messages
// ===========================================================================

#[test]
fn compose_identity_name_collision_error_mentions_memory_isolation() {
    let primary = test_primary();
    let mut sub = test_sub(AgentRole::Reviewer, "1", 1);
    sub.manifest.name = primary.name.clone();
    let err = compose_identity(primary, vec![sub]).unwrap_err();
    assert!(
        err.to_string().contains("memory isolation"),
        "error should mention memory isolation: {err}"
    );
}

#[test]
fn compose_identity_duplicate_name_error_mentions_unique() {
    let primary = test_primary();
    let sub1 = test_sub(AgentRole::Reviewer, "dup", 1);
    let mut sub2 = test_sub(AgentRole::GymRunner, "other", 1);
    sub2.manifest.name = sub1.manifest.name.clone();
    let err = compose_identity(primary, vec![sub1, sub2]).unwrap_err();
    assert!(
        err.to_string().contains("unique"),
        "error should mention unique: {err}"
    );
}
