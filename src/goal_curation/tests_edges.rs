//! Tests (issue #2405): the typed goal-graph edge model.
//!
//! These tests pin the **durable contract** for goal-decomposition edges and
//! prove that a parent→child edge written through
//! [`super::edges::write_edge`] **round-trips back** out of the
//! cognitive-memory graph — the acceptance bar that the edges are *real and
//! queryable*, not a stub.
//!
//! They pin the implemented surface:
//!   - `super::types::{GoalEdge, GoalEdgeType, GoalNode}`
//!   - `super::edges::{write_edge, children_of, edges_of_type, parse_goal_edge}`
//!
//! The edges are exercised against the **real** library backend
//! ([`LibraryCognitiveMemory::in_memory`]) so the caller-key dedup
//! (idempotency) and `search_facts` recall paths are tested for real, not
//! against a hand-rolled mock.

use super::edges::{children_of, edges_of_type, parse_goal_edge, write_edge};
use super::types::{GoalEdge, GoalEdgeType, GoalNode};
use crate::cognitive_memory::LibraryCognitiveMemory;

/// A fresh, real-but-ephemeral cognitive memory for one test.
fn mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory cognitive memory")
}

fn decomposes(parent: &str, child: &str) -> GoalEdge {
    GoalEdge::new(parent, child, GoalEdgeType::DecomposesInto)
}

// ── Edge-type tokens (durable, cross-system contract) ──────────────────────

#[test]
fn edge_type_as_str_is_snake_case() {
    assert_eq!(GoalEdgeType::DecomposesInto.as_str(), "decomposes_into");
    assert_eq!(GoalEdgeType::DependsOn.as_str(), "depends_on");
}

// ── Concept / caller-key / tag / content format pins ───────────────────────

#[test]
fn decomposes_into_concept_key_format() {
    let e = decomposes("goal-p", "goal-c");
    assert_eq!(e.concept(), "goal-edge:decomposes_into");
}

#[test]
fn decomposes_into_caller_key_format() {
    let e = decomposes("goal-p", "goal-c");
    assert_eq!(e.caller_key(), "goal-edge:decomposes_into:goal-p->goal-c");
}

#[test]
fn decomposes_into_tags_format() {
    let e = decomposes("goal-p", "goal-c");
    assert_eq!(
        e.tags(),
        vec![
            "goal-edge".to_string(),
            "decomposes_into".to_string(),
            "from:goal-p".to_string(),
            "to:goal-c".to_string(),
        ]
    );
}

#[test]
fn edge_content_is_canonical_compact_json() {
    let e = decomposes("goal-p", "goal-c");
    assert_eq!(
        e.content(),
        r#"{"from":"goal-p","to":"goal-c","edge_type":"decomposes_into"}"#
    );
}

#[test]
fn depends_on_keys_use_its_own_type() {
    let e = GoalEdge::new("goal-b", "goal-a", GoalEdgeType::DependsOn);
    assert_eq!(e.concept(), "goal-edge:depends_on");
    assert_eq!(e.caller_key(), "goal-edge:depends_on:goal-b->goal-a");
}

// ── Parsing a stored edge back ─────────────────────────────────────────────

#[test]
fn parse_goal_edge_round_trips_content() {
    let e = decomposes("goal-7a1c", "goal-9f02");
    let parsed = parse_goal_edge(&e.content()).expect("content must parse back into a GoalEdge");
    assert_eq!(parsed, e);
    assert_eq!(parsed.from, "goal-7a1c");
    assert_eq!(parsed.to, "goal-9f02");
    assert_eq!(parsed.edge_type, GoalEdgeType::DecomposesInto);
}

#[test]
fn parse_goal_edge_rejects_non_edge_content() {
    assert!(parse_goal_edge("not json at all").is_none());
    assert!(parse_goal_edge(r#"{"unrelated":"fact"}"#).is_none());
}

// ── Round-trip through the REAL graph backend (acceptance proof) ───────────

#[test]
fn decomposes_into_edge_round_trips_through_the_graph() {
    let m = mem();
    let id1 = write_edge(&m, &decomposes("goal-p", "goal-c1")).expect("write child 1 edge");
    let id2 = write_edge(&m, &decomposes("goal-p", "goal-c2")).expect("write child 2 edge");
    assert!(
        !id1.is_empty() && !id2.is_empty(),
        "edges must get node ids"
    );

    let mut kids = children_of(&m, "goal-p").expect("query children of goal-p");
    kids.sort();
    assert_eq!(
        kids,
        vec!["goal-c1".to_string(), "goal-c2".to_string()],
        "both children must be queryable back from the parent"
    );
}

#[test]
fn children_of_filters_by_parent_id() {
    let m = mem();
    write_edge(&m, &decomposes("goal-p1", "goal-a")).unwrap();
    write_edge(&m, &decomposes("goal-p2", "goal-b")).unwrap();

    assert_eq!(
        children_of(&m, "goal-p1").unwrap(),
        vec!["goal-a".to_string()],
        "children_of must only return children of the queried parent"
    );
    assert_eq!(
        children_of(&m, "goal-p2").unwrap(),
        vec!["goal-b".to_string()]
    );
}

#[test]
fn children_of_unknown_parent_is_empty() {
    let m = mem();
    write_edge(&m, &decomposes("goal-p", "goal-c")).unwrap();
    assert!(
        children_of(&m, "goal-nope").unwrap().is_empty(),
        "a parent with no decomposes_into edges has no children"
    );
}

#[test]
fn write_edge_is_idempotent_via_caller_key() {
    let m = mem();
    let edge = decomposes("goal-p", "goal-c");
    write_edge(&m, &edge).unwrap();
    write_edge(&m, &edge).unwrap();
    write_edge(&m, &edge).unwrap();

    let kids = children_of(&m, "goal-p").unwrap();
    assert_eq!(
        kids,
        vec!["goal-c".to_string()],
        "re-writing the same edge must dedup (caller-key supersede), not accumulate"
    );
}

// ── depends_on (sibling ordering) edges ────────────────────────────────────

#[test]
fn depends_on_edge_round_trips_and_is_type_scoped() {
    let m = mem();
    // child-b depends_on child-a; and an unrelated decomposes_into from child-b.
    write_edge(
        &m,
        &GoalEdge::new("goal-b", "goal-a", GoalEdgeType::DependsOn),
    )
    .unwrap();
    write_edge(&m, &decomposes("goal-b", "goal-grandchild")).unwrap();

    let deps =
        edges_of_type(&m, GoalEdgeType::DependsOn, "goal-b").expect("query depends_on edges");
    assert_eq!(deps.len(), 1, "exactly one depends_on edge from goal-b");
    assert_eq!(deps[0].to, "goal-a");
    assert_eq!(deps[0].edge_type, GoalEdgeType::DependsOn);

    // The decomposes_into edge must NOT leak into a depends_on query.
    assert!(
        deps.iter().all(|e| e.edge_type == GoalEdgeType::DependsOn),
        "edges_of_type must scope strictly by edge type"
    );
}

// ── GoalNode (graph projection / edge anchor) ──────────────────────────────

#[test]
fn goal_node_round_trips() {
    let node = GoalNode::new(
        "goal-p",
        "Ship the umbrella goal",
        Some("all children done"),
        Vec::new(),
    );
    let json = serde_json::to_string(&node).expect("serialize GoalNode");
    let back: GoalNode = serde_json::from_str(&json).expect("deserialize GoalNode");
    assert_eq!(back, node);
    assert_eq!(back.id, "goal-p");
    assert_eq!(back.done_criterion.as_deref(), Some("all children done"));
}
