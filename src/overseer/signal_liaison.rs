//! The Overseer's native Signal **operator-liaison** rail (issue #4911,
//! Deliverable 1).
//!
//! This module is a THIN, deterministic rail. ALL semantic judgment — interpret
//! the operator's intent, compose the reply, decide whether an intervention is
//! warranted — lives in the `operator-liaison` agentic recipe and reaches the
//! rail ONLY as a typed [`LiaisonDecisionRecord`]. The rail merely:
//!
//!   1. decides which received messages to act on (the pure acceptance filter),
//!   2. tracks a durable per-group high-water-mark so each message is handled
//!      once (dedup), and
//!   3. translates a recorded decision into concrete actions — an outbound reply
//!      and/or the EXISTING [`Intervention::LaunchRecipe`] dispatch.
//!
//! There is no classifier, no prose parsing, and no second scheduler here.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::overseer::capabilities::RecipeBrief;
use crate::overseer::intervention::Intervention;
use crate::stewardship::liaison_decision_store::LiaisonDecisionRecord;
use crate::stewardship::record_io::{atomic_write_0600, sha256_hex};

/// The pure acceptance filter: a received message is acted on **iff** it is from
/// the configured operator, in the configured group, is not a self-echo, and is
/// above the durable high-water-mark (not already handled).
///
/// `msg_group_id == None` (a direct, non-group message) can NEVER match a group,
/// so a direct message is always rejected regardless of the other conditions.
pub fn liaison_should_accept(
    authorized: bool,
    msg_group_id: Option<&str>,
    configured_group_id: &str,
    is_echo: bool,
    above_high_water_mark: bool,
) -> bool {
    authorized && msg_group_id == Some(configured_group_id) && !is_echo && above_high_water_mark
}

/// The durable per-group high-water-mark record. Monotonic — a handled marker is
/// never below the mark again.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HighWaterMark {
    mark: u64,
}

/// Hash the opaque `group_id` into a single path-safe hex segment (SHA-256), so
/// a base64 group id (which may hold `/`, `+`, `=`) never appears verbatim in
/// the path and can never escape the subtree.
fn group_id_segment(group_id: &str) -> String {
    sha256_hex(group_id.as_bytes())
}

/// Path to the durable high-water-mark for `group_id`:
/// `<state_root>/overseer/liaison_hwm/<group_id_hash>/mark.json`.
fn hwm_path(state_root: &Path, group_id: &str) -> std::path::PathBuf {
    state_root
        .join("overseer")
        .join("liaison_hwm")
        .join(group_id_segment(group_id))
        .join("mark.json")
}

/// Read the current durable high-water-mark for `group_id`. A missing or corrupt
/// record reads as `0` (fail-safe: an unreadable mark never blocks a genuinely
/// new operator message — the acceptance filter still requires operator identity
/// and group match, and the recipe run is idempotent per message id).
fn read_mark(state_root: &Path, group_id: &str) -> u64 {
    let path = hwm_path(state_root, group_id);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<HighWaterMark>(&bytes)
            .map(|m| m.mark)
            .unwrap_or(0),
        Err(_) => 0,
    }
}

/// Record `marker` as handled for `group_id`, advancing the durable high-water-
/// mark **monotonically** (never decreased). Atomic temp-write + rename, owner-
/// only `0o600`.
pub fn record_high_water_mark(
    state_root: &Path,
    group_id: &str,
    marker: u64,
) -> Result<(), String> {
    let current = read_mark(state_root, group_id);
    let next = current.max(marker);
    let path = hwm_path(state_root, group_id);

    let rec = HighWaterMark { mark: next };
    let json = serde_json::to_vec_pretty(&rec).map_err(|e| format!("serialize hwm: {e}"))?;
    atomic_write_0600(&path, &json)
}

/// Whether `marker` is strictly above the durable high-water-mark for
/// `group_id` — i.e. a not-yet-handled message. With no prior record the mark is
/// `0`, so any positive marker is new.
pub fn is_above_high_water_mark(state_root: &Path, group_id: &str, marker: u64) -> bool {
    marker > read_mark(state_root, group_id)
}

/// The concrete actions a recorded liaison decision maps to. `reply` and
/// `intervention` are independent (the brief's `and/or` rule) — a decision may
/// carry either, both, or (a valid no-op) neither.
#[derive(Debug, Default)]
pub struct LiaisonActions {
    /// A plain-English reply to post back to the operator group.
    pub reply: Option<String>,
    /// A directed intervention to dispatch through the EXISTING Overseer
    /// machinery. A directive maps to the reused [`Intervention::LaunchRecipe`].
    pub intervention: Option<Intervention>,
}

/// Translate a typed [`LiaisonDecisionRecord`] into [`LiaisonActions`]. This is a
/// pure mapping: it makes NO judgment, only carries the agent's recorded reply
/// and turns a recorded directive into the reused `LaunchRecipe` dispatch.
pub fn liaison_actions_from_decision(rec: &LiaisonDecisionRecord) -> LiaisonActions {
    let intervention = rec.directive.as_ref().map(|d| Intervention::LaunchRecipe {
        brief: RecipeBrief {
            task_description: d.task_description.clone(),
            target_repo: d.target_repo.clone(),
            sequence_group: None,
        },
    });
    LiaisonActions {
        reply: rec.reply.clone(),
        intervention,
    }
}

/// One operator-group message the [`LiaisonPort`] surfaced this tick, already
/// projected to exactly the fields the pure acceptance filter needs. The port
/// (production) parses signal-cli JSON-RPC (`parse_incoming`, groupId) and marks
/// self-echoes via `matches_recent_outbound` / `should_accept_sync_sent`.
#[derive(Debug, Clone)]
pub struct ReceivedOperatorMessage {
    /// True iff the sender is the configured operator number.
    pub authorized: bool,
    /// The message's group id, if any (`None` for a direct message).
    pub group_id: Option<String>,
    /// The monotonic per-group high-water-mark id (e.g. the message timestamp).
    pub message_id: u64,
    /// The plain-English message body handed to the liaison recipe via a
    /// ContextFile (never argv).
    pub text: String,
    /// True iff this is one of the Overseer's own recent outbound posts.
    pub is_echo: bool,
}

/// The external-I/O seam for the operator-liaison rail: receive new operator
/// messages, run the `operator-liaison` recipe (which WRITEs a typed decision
/// record), and post a plain-English reply back to the group. The rail keeps ALL
/// deterministic logic (accept filter, HWM/dedup, decision→actions mapping) in
/// the tested pure functions above; the port only performs the raw effects. This
/// mirrors the established `ecosystem_observe::EcosystemObserver` seam and is
/// `None` (inert) until `build_overseer` wires the production implementation.
pub trait LiaisonPort: Send + Sync {
    /// New operator-group messages observed since the last tick.
    fn receive(&self) -> Vec<ReceivedOperatorMessage>;

    /// Run the `operator-liaison` recipe for one accepted message. The recipe
    /// reads the message body from `context_path` and WRITEs a typed
    /// [`LiaisonDecisionRecord`] under the state root, keyed by
    /// `(group_id, message_id)` with the given `run_token`.
    fn run_liaison_recipe(
        &self,
        group_id: &str,
        message_id: u64,
        run_token: &str,
        context_path: &str,
    ) -> Result<(), String>;

    /// Post a plain-English reply back to the operator group.
    fn send_group_reply(&self, group_id: &str, text: &str) -> Result<(), String>;
}
