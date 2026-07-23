//! Operator-pinned done-gates: the **positive** analog of goal tombstones.
//!
//! # Why this exists
//!
//! [`crate::goal_board_store::reconcile`] is *in-flight-wins for existing
//! goals*: for a goal present in both the operator-edited file and the daemon's
//! in-memory board, the daemon's copy wins field-for-field. That is correct for
//! progress/status the daemon is actively driving — but it means an operator who
//! rewrites a goal's **done-criteria** (binding a measurable PR/issue and a
//! plain-English finish line) is silently clobbered back to the goal's original
//! unmeasurable prose on the very next cycle. That is exactly the
//! `UNCLEAR-CRITERIA` stall this closes: the operator remedy would not *stick*.
//!
//! Tombstones already give operators a durable **negative** intent ("this goal
//! is gone, never resurrect it") that `reconcile` honours. This module is the
//! symmetric **positive** intent: "this goal's finish line is pinned, always
//! re-assert it." A pin is a tiny durable side-channel file, consulted by
//! [`crate::goal_board_store::commit_cycle`] after `reconcile`, so the measurable
//! anchor and finish line survive every in-flight-wins merge.
//!
//! The mechanism is **inert by default**: with no pins recorded the pin map is
//! empty and [`apply_done_gate_pins`] is a no-op, so the daemon hot path is
//! unchanged for the common case.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::{ActiveGoal, GoalBoard, WipRef};

/// Filename (under the state root, beside `goal_tombstones.json`) of the durable
/// operator done-gate pins.
const DONE_GATE_PINS_FILENAME: &str = "goal_done_gate_pins.json";

/// Stable marker that separates a goal's own description from the operator-
/// appended plain-English finish line. Re-application truncates at this marker
/// before re-appending, so repeated applies never stack.
pub const DONE_WHEN_MARKER: &str = "\n\nDone when: ";

/// A machine-checkable finish line an operator has pinned to a goal so the
/// daemon's in-flight-wins reconcile can never revert it to unmeasurable prose.
///
/// At least one of `pr` / `issue` should be set for the gate to be measurable
/// ([`DoneGatePin::is_measurable`]); `criteria` is the optional plain-English
/// description the operator wants surfaced on the goal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoneGatePin {
    /// PR number the completion gate checks MERGED. Stored as a string to match
    /// [`WipRef::ref_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
    /// Issue number the completion gate checks CLOSED.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// Optional plain-English finish line shown on the goal description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<String>,
}

impl DoneGatePin {
    /// Human-readable anchor phrase describing what the gate now measures.
    #[must_use]
    pub fn anchor(&self) -> String {
        match (self.pr.as_deref(), self.issue.as_deref()) {
            (Some(p), Some(i)) => format!("PR #{p} is merged and issue #{i} is closed"),
            (Some(p), None) => format!("PR #{p} is merged"),
            (None, Some(i)) => format!("issue #{i} is closed"),
            (None, None) => "its linked work lands".to_string(),
        }
    }

    /// The plain-English finish line appended to the goal description.
    #[must_use]
    pub fn finish_line(&self) -> String {
        match &self.criteria {
            Some(text) => format!("{text} (certified automatically when {})", self.anchor()),
            None => format!("Certified automatically when {}.", self.anchor()),
        }
    }

    /// True when the pin carries at least one measurable anchor (PR or issue).
    #[must_use]
    pub fn is_measurable(&self) -> bool {
        self.pr.is_some() || self.issue.is_some()
    }

    /// Durably re-assert this pin onto `goal`: bind the measurable wip-refs and
    /// (re)write the finish line onto the description.
    ///
    /// Idempotent — re-applying yields the same board. Deliberately does **not**
    /// touch `status` or the no-progress breaker: those are one-time operator
    /// effects applied when the pin is first set, not something to force every
    /// cycle (which would mask genuine no-progress or fight daemon transitions).
    pub fn apply_to(&self, goal: &mut ActiveGoal) {
        if let Some(pr) = &self.pr {
            upsert_first_ref(&mut goal.wip_refs, "pr", pr, format!("PR #{pr}"));
        }
        if let Some(issue) = &self.issue {
            upsert_first_ref(
                &mut goal.wip_refs,
                "issue",
                issue,
                format!("issue #{issue}"),
            );
        }
        let base = match goal.description.split_once(DONE_WHEN_MARKER) {
            Some((head, _)) => head.to_string(),
            None => goal.description.clone(),
        };
        goal.description = format!("{base}{DONE_WHEN_MARKER}{}", self.finish_line());
    }
}

/// Upsert `num` as the first `kind` ref, de-duplicating any prior identical ref.
///
/// Shared by the operator CLI (`goal set-done-gate`) and pin re-application so
/// the two paths bind refs identically.
pub fn upsert_first_ref(refs: &mut Vec<WipRef>, kind: &str, num: &str, label: String) {
    refs.retain(|r| !(r.kind.eq_ignore_ascii_case(kind) && r.ref_id == num));
    let insert_at = refs
        .iter()
        .position(|r| r.kind.eq_ignore_ascii_case(kind))
        .unwrap_or(0);
    refs.insert(
        insert_at,
        WipRef {
            kind: kind.to_string(),
            ref_id: num.to_string(),
            label,
            url: None,
        },
    );
}

/// Absolute path of the durable pin file under `state_root`.
fn pins_path(state_root: &Path) -> PathBuf {
    state_root.join(DONE_GATE_PINS_FILENAME)
}

/// Load the operator done-gate pins (`goal id -> pin`).
///
/// Returns an empty map if the file is absent or unparsable (fail-open: a
/// corrupt pin file must never wedge the daemon's commit path). Fail-open is
/// **observable**: a corrupt or unreadable pin file silently disables the very
/// clobber-protection this module exists for, so both branches emit a
/// `tracing::warn!` (an absent file is the expected empty-state and stays
/// silent) rather than swallowing the failure.
#[must_use]
pub fn load_done_gate_pins(state_root: &Path) -> BTreeMap<String, DoneGatePin> {
    let path = pins_path(state_root);
    match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str(&raw) {
            Ok(pins) => pins,
            Err(e) => {
                tracing::warn!(
                    target: "simard::goal",
                    path = %path.display(),
                    error = %e,
                    "done-gate pins file is corrupt; ignoring it this cycle \
                     (operator finish lines will NOT be re-asserted until it is \
                     repaired or rewritten by `goal set-done-gate`)"
                );
                BTreeMap::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => {
            tracing::warn!(
                target: "simard::goal",
                path = %path.display(),
                error = %e,
                "done-gate pins file could not be read; ignoring it this cycle \
                 (operator finish lines will NOT be re-asserted)"
            );
            BTreeMap::new()
        }
    }
}

/// Persist the full pin map with an atomic temp-file + `rename` (mirroring the
/// authoritative board's [`crate::goal_board_store`] write) so a crash mid-write
/// or a concurrent daemon read can never observe a torn, half-written pin file —
/// which would otherwise be silently discarded by [`load_done_gate_pins`],
/// re-opening the clobber this module closes.
pub fn save_done_gate_pins(
    state_root: &Path,
    pins: &BTreeMap<String, DoneGatePin>,
) -> SimardResult<()> {
    let path = pins_path(state_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SimardError::ArtifactIo {
            path: parent.to_path_buf(),
            reason: format!("creating done-gate pins dir: {e}"),
        })?;
    }
    let json = serde_json::to_string_pretty(pins).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("serializing done-gate pins: {e}"),
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes()).map_err(|e| SimardError::ArtifactIo {
        path: tmp.clone(),
        reason: format!("writing done-gate pins temp file: {e}"),
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("renaming done-gate pins temp file into place: {e}"),
    })
}

/// Record (upsert) a durable pin for `goal_id`.
pub fn record_done_gate_pin(
    state_root: &Path,
    goal_id: &str,
    pin: DoneGatePin,
) -> SimardResult<()> {
    let mut pins = load_done_gate_pins(state_root);
    pins.insert(goal_id.to_string(), pin);
    save_done_gate_pins(state_root, &pins)
}

/// Drop pins for the given goal ids (called when a goal is tombstoned/completed
/// so a finished goal's finish line is not re-asserted forever).
pub fn clear_done_gate_pins(state_root: &Path, ids: &[String]) -> SimardResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut pins = load_done_gate_pins(state_root);
    let before = pins.len();
    for id in ids {
        pins.remove(id);
    }
    if pins.len() != before {
        save_done_gate_pins(state_root, &pins)?;
    }
    Ok(())
}

/// Re-assert every operator pin onto its matching **active** goal.
///
/// No-op when `pins` is empty (the common case), keeping the daemon hot path
/// unchanged unless an operator has explicitly pinned a done-gate.
pub fn apply_done_gate_pins(board: &mut GoalBoard, pins: &BTreeMap<String, DoneGatePin>) {
    if pins.is_empty() {
        return;
    }
    for goal in board.active.iter_mut() {
        if let Some(pin) = pins.get(&goal.id) {
            pin.apply_to(goal);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal_curation::{ActiveGoal, GoalBoard};

    fn goal(id: &str, desc: &str) -> ActiveGoal {
        ActiveGoal::new(id, desc, 3)
    }

    #[test]
    fn apply_binds_refs_and_finish_line() {
        let mut g = goal("g1", "Do the thing.");
        let pin = DoneGatePin {
            pr: Some("4440".into()),
            issue: Some("4448".into()),
            criteria: Some("roster is identity-owned and deploy-durable".into()),
        };
        pin.apply_to(&mut g);
        assert!(
            g.wip_refs
                .iter()
                .any(|r| r.kind == "pr" && r.ref_id == "4440")
        );
        assert!(
            g.wip_refs
                .iter()
                .any(|r| r.kind == "issue" && r.ref_id == "4448")
        );
        assert!(
            g.description
                .contains("Done when: roster is identity-owned")
        );
        assert!(
            g.description
                .contains("PR #4440 is merged and issue #4448 is closed")
        );
    }

    #[test]
    fn apply_is_idempotent() {
        let mut g = goal("g1", "Base.");
        let pin = DoneGatePin {
            pr: None,
            issue: Some("4448".into()),
            criteria: None,
        };
        pin.apply_to(&mut g);
        let once = g.clone();
        pin.apply_to(&mut g);
        assert_eq!(
            g, once,
            "re-applying a pin must not stack refs or finish lines"
        );
        assert_eq!(
            g.wip_refs.iter().filter(|r| r.kind == "issue").count(),
            1,
            "issue ref must not duplicate"
        );
    }

    #[test]
    fn apply_pins_only_touches_matching_active_goals() {
        let mut board = GoalBoard {
            active: vec![goal("keep-me", "untouched"), goal("pin-me", "will pin")],
            backlog: vec![],
        };
        let mut pins = BTreeMap::new();
        pins.insert(
            "pin-me".to_string(),
            DoneGatePin {
                pr: None,
                issue: Some("4448".into()),
                criteria: None,
            },
        );
        apply_done_gate_pins(&mut board, &pins);
        let untouched = &board.active[0];
        let pinned = &board.active[1];
        assert!(untouched.wip_refs.is_empty());
        assert!(!untouched.description.contains("Done when:"));
        assert!(pinned.wip_refs.iter().any(|r| r.ref_id == "4448"));
        assert!(pinned.description.contains("Done when:"));
    }

    #[test]
    fn empty_pins_is_a_noop() {
        let mut board = GoalBoard {
            active: vec![goal("g1", "orig")],
            backlog: vec![],
        };
        let before = board.clone();
        apply_done_gate_pins(&mut board, &BTreeMap::new());
        assert_eq!(board, before);
    }

    #[test]
    fn record_load_clear_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(load_done_gate_pins(root).is_empty());

        record_done_gate_pin(
            root,
            "g1",
            DoneGatePin {
                pr: Some("4440".into()),
                issue: Some("4448".into()),
                criteria: None,
            },
        )
        .unwrap();
        let loaded = load_done_gate_pins(root);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded["g1"].pr.as_deref(), Some("4440"));

        clear_done_gate_pins(root, &["g1".to_string()]).unwrap();
        assert!(load_done_gate_pins(root).is_empty());
    }

    #[test]
    fn corrupt_pins_file_fails_open_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(pins_path(root), b"{ this is not valid json").unwrap();
        assert!(
            load_done_gate_pins(root).is_empty(),
            "a corrupt pins file must fail open to an empty map, never panic or wedge"
        );
    }

    #[test]
    fn is_measurable_reflects_anchor_presence() {
        assert!(
            DoneGatePin {
                issue: Some("1".into()),
                ..Default::default()
            }
            .is_measurable()
        );
        assert!(!DoneGatePin::default().is_measurable());
    }
}
