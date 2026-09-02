//! TDD tests for the ONE new `Intervention` variant added for the autonomous PR
//! rework loop (design component C6): `Intervention::ReworkPr`.
//!
//! Contract these tests pin:
//!   - `Intervention::ReworkPr { repo: String, pr: u32, concern_path: String }`
//!     — a THIN dispatch tag. Its `act()` (exercised elsewhere) reuses the
//!     existing `Intervention::LaunchRecipe` path against default-workflow, with
//!     the concern delivered as a ContextFile referenced by `concern_path`
//!     (never argv/env → no E2BIG).
//!   - `label()` returns the stable `"rework_pr"` used in gate messages,
//!     telemetry, and dedup.
//!   - The variant participates in `Clone`/`Debug`/`PartialEq` like its siblings.
//!
//! Only ONE new variant is added (the liaison reuses the existing
//! `LaunchRecipe`), so this file pins exactly that surface. It references the
//! not-yet-added variant and FAILS TO COMPILE until C6 lands — intended red.

use crate::overseer::intervention::Intervention;

fn sample() -> Intervention {
    Intervention::ReworkPr {
        repo: "rysweet/Simard".to_string(),
        pr: 4931,
        concern_path: "/tmp/rework-concern.txt".to_string(),
    }
}

#[test]
fn rework_pr_has_stable_label() {
    assert_eq!(
        sample().label(),
        "rework_pr",
        "ReworkPr must carry the stable label `rework_pr`"
    );
}

#[test]
fn rework_pr_is_distinct_from_launch_recipe_label() {
    // The tag is a distinct telemetry/dedup identity even though its act() reuses
    // the LaunchRecipe dispatch under the hood.
    assert_ne!(sample().label(), "launch_recipe");
}

#[test]
fn rework_pr_carries_repo_pr_and_concern_path() {
    match sample() {
        Intervention::ReworkPr {
            repo,
            pr,
            concern_path,
        } => {
            assert_eq!(repo, "rysweet/Simard");
            assert_eq!(pr, 4931);
            assert_eq!(concern_path, "/tmp/rework-concern.txt");
        }
        other => panic!("expected ReworkPr, got {other:?}"),
    }
}

#[test]
fn rework_pr_is_clonable_and_eq() {
    let a = sample();
    let b = a.clone();
    assert_eq!(
        a, b,
        "ReworkPr must derive Clone + PartialEq like its siblings"
    );
}
