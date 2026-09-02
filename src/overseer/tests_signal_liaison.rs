//! TDD tests for the Signal operator-liaison RAIL (Deliverable 1, design
//! component C2): `crate::overseer::signal_liaison`.
//!
//! The rail is thin and deterministic — ALL semantic judgment (interpret intent,
//! compose the reply, pick the intervention) lives in the `operator-liaison`
//! recipe and reaches the rail ONLY as a typed
//! [`LiaisonDecisionRecord`]. This file pins the three deterministic sub-rails
//! the rail composes (all in `crate::overseer::signal_liaison`):
//!
//!   1. Acceptance filter (pure):
//!      `liaison_should_accept(authorized, msg_group_id, configured_group_id,
//!         is_echo, above_high_water_mark) -> bool`
//!      accept ⟺ authorized ∧ group_id == configured ∧ ¬echo ∧ above HWM.
//!      `group_id: None` never matches a group.
//!   2. Durable high-water-mark / dedup:
//!      `record_high_water_mark(state_root, group_id, marker) -> Result<(),String>`
//!      `is_above_high_water_mark(state_root, group_id, marker) -> bool`
//!      monotonic — a handled marker (or lower) is never above the mark again.
//!   3. Decision → actions (pure):
//!      `LiaisonActions { reply: Option<String>, intervention: Option<Intervention> }`
//!      `liaison_actions_from_decision(&LiaisonDecisionRecord) -> LiaisonActions`
//!      a `reply` becomes an outbound reply; a `directive` becomes the EXISTING
//!      `Intervention::LaunchRecipe` (default-workflow). The two are NOT mutually
//!      exclusive.
//!
//! References not-yet-existent symbols → FAILS TO COMPILE until C2 lands.

use std::path::{Path, PathBuf};

use crate::overseer::intervention::Intervention;
use crate::overseer::signal_liaison::{
    LiaisonActions, is_above_high_water_mark, liaison_actions_from_decision, liaison_should_accept,
    record_high_water_mark,
};
use crate::stewardship::liaison_decision_store::{Directive, LiaisonDecisionRecord};

const GROUP: &str = "cGxheS9ncm91cCsx==";
const OTHER_GROUP: &str = "b3RoZXItZ3JvdXA=";

fn temp_state_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-liaison-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

fn directive() -> Directive {
    Directive {
        recipe: "default-workflow".to_string(),
        task_description: "Investigate and fix the flaky deploy canary.".to_string(),
        target_repo: "rysweet/Simard".to_string(),
        context_path: "/tmp/liaison-directive-context.txt".to_string(),
    }
}

// ───────────────────────── acceptance filter ────────────────────────────────

#[test]
fn accepts_only_when_all_conditions_hold() {
    assert!(
        liaison_should_accept(true, Some(GROUP), GROUP, false, true),
        "authorized ∧ matching group ∧ not-echo ∧ above HWM must accept"
    );
}

#[test]
fn rejects_when_any_condition_fails() {
    // unauthorized
    assert!(!liaison_should_accept(
        false,
        Some(GROUP),
        GROUP,
        false,
        true
    ));
    // wrong group
    assert!(!liaison_should_accept(
        true,
        Some(OTHER_GROUP),
        GROUP,
        false,
        true
    ));
    // echo (our own post synced back)
    assert!(!liaison_should_accept(true, Some(GROUP), GROUP, true, true));
    // not above the high-water-mark (already handled)
    assert!(!liaison_should_accept(
        true,
        Some(GROUP),
        GROUP,
        false,
        false
    ));
}

#[test]
fn none_group_id_never_matches_a_group() {
    // A direct (non-group) message must never be accepted as a group message,
    // even if everything else looks fine.
    assert!(!liaison_should_accept(true, None, GROUP, false, true));
}

// ───────────────────────── high-water-mark / dedup ──────────────────────────

#[test]
fn unseen_marker_is_above_the_high_water_mark() {
    let root = temp_state_root("hwm-new");
    // With no prior record, ANY marker is new ⇒ above the (empty) mark.
    assert!(is_above_high_water_mark(&root, GROUP, 1));
    assert!(is_above_high_water_mark(&root, GROUP, 9_999_999));
    cleanup(&root);
}

#[test]
fn recorded_marker_is_not_above_again_but_a_higher_one_is() {
    let root = temp_state_root("hwm-mono");
    record_high_water_mark(&root, GROUP, 100).expect("record marker");
    assert!(
        !is_above_high_water_mark(&root, GROUP, 100),
        "the exact handled marker must NOT be above the mark again (handled once)"
    );
    assert!(
        !is_above_high_water_mark(&root, GROUP, 50),
        "an older marker must NOT be above the mark (monotonic)"
    );
    assert!(
        is_above_high_water_mark(&root, GROUP, 101),
        "a newer marker must be above the mark"
    );
    cleanup(&root);
}

#[test]
fn high_water_mark_is_per_group() {
    let root = temp_state_root("hwm-pergroup");
    record_high_water_mark(&root, GROUP, 100).unwrap();
    // A different group has its own independent mark.
    assert!(is_above_high_water_mark(&root, OTHER_GROUP, 1));
    cleanup(&root);
}

// ───────────────────────── decision → actions ───────────────────────────────

#[test]
fn reply_only_decision_yields_reply_and_no_intervention() {
    let rec = LiaisonDecisionRecord::new(GROUP, "1", "t", Some("On it.".to_string()), None);
    let LiaisonActions {
        reply,
        intervention,
    } = liaison_actions_from_decision(&rec);
    assert_eq!(reply.as_deref(), Some("On it."));
    assert!(intervention.is_none());
}

#[test]
fn directive_only_decision_yields_launch_recipe_and_no_reply() {
    let rec = LiaisonDecisionRecord::new(GROUP, "1", "t", None, Some(directive()));
    let LiaisonActions {
        reply,
        intervention,
    } = liaison_actions_from_decision(&rec);
    assert!(reply.is_none());
    match intervention {
        Some(Intervention::LaunchRecipe { brief }) => {
            assert_eq!(
                brief.task_description,
                "Investigate and fix the flaky deploy canary."
            );
            assert_eq!(brief.target_repo, "rysweet/Simard");
        }
        other => panic!("a directive must map to LaunchRecipe, got {other:?}"),
    }
}

#[test]
fn both_reply_and_directive_are_not_mutually_exclusive() {
    let rec = LiaisonDecisionRecord::new(
        GROUP,
        "1",
        "t",
        Some("Kicking off a fix now.".to_string()),
        Some(directive()),
    );
    let actions = liaison_actions_from_decision(&rec);
    assert!(actions.reply.is_some(), "reply must be carried");
    assert!(
        matches!(
            actions.intervention,
            Some(Intervention::LaunchRecipe { .. })
        ),
        "directive must ALSO produce a LaunchRecipe in the same run"
    );
}

#[test]
fn neither_reply_nor_directive_is_a_noop() {
    let rec = LiaisonDecisionRecord::new(GROUP, "1", "t", None, None);
    let actions = liaison_actions_from_decision(&rec);
    assert!(actions.reply.is_none() && actions.intervention.is_none());
}
