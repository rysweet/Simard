//! Typed goal-graph edges (and node anchors), stored as typed relationship
//! facts in cognitive memory (issue #2405). See
//! `docs/reference/goal-decomposition.md`.
//!
//! **Design choice (b) from the issue:** edges are typed relationship *facts*
//! via [`CognitiveMemoryOps`] rather than a new typed-edge trait method. Each
//! edge is one fact under a stable caller key
//! (`goal-edge:{type}:{from}->{to}`), so re-writing the same edge dedups
//! (idempotent) instead of accumulating; a changed edge supersedes its prior
//! revision via the backend's `SUPERSEDES` edge. Querying back is the ordinary
//! [`search_facts`](CognitiveMemoryOps::search_facts) path with a **strict**
//! `from` / `edge_type` filter on the parsed content — the keyword query only
//! narrows the candidate set, the parsed content is the authoritative check.

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::types::{GoalEdge, GoalEdgeType, GoalNode};

/// Source label attached to every goal-graph fact for provenance.
const GRAPH_SOURCE: &str = "goal-decomposition";

/// Generous recall limit. A parent has at most six children, but the global
/// edge set for a type can be larger, so over-fetch by concept then filter
/// strictly on the parsed content.
const RECALL_LIMIT: u32 = 1024;

/// Parse an edge fact's content back into a [`GoalEdge`]. Returns `None` for
/// any content that is not a well-formed goal edge (the rejection path the
/// `from`/`type` filter relies on).
pub fn parse_goal_edge(content: &str) -> Option<GoalEdge> {
    serde_json::from_str::<GoalEdge>(content).ok()
}

/// Validate an edge's endpoints before it is written: both ids must pass the
/// shared [`validate_goal_id`](crate::engineer_worktree::validate_goal_id)
/// charset check (so a malformed LLM decomposition cannot forge a caller key or
/// inject a path/ref), and an edge may not be a self-loop.
fn validate_edge_endpoints(edge: &GoalEdge) -> SimardResult<()> {
    for (role, id) in [("from", &edge.from), ("to", &edge.to)] {
        crate::engineer_worktree::validate_goal_id(id).map_err(|reason| {
            SimardError::InvalidGoalRecord {
                field: format!("goal_edge.{role}"),
                reason,
            }
        })?;
    }
    if edge.from == edge.to {
        return Err(SimardError::InvalidGoalRecord {
            field: "goal_edge".to_string(),
            reason: format!("self-edge is not allowed: {} -> {}", edge.from, edge.to),
        });
    }
    Ok(())
}

/// Write one typed edge into the cognitive-memory graph. Idempotent via the
/// edge's stable caller key (re-writing the same edge dedups). Returns the live
/// fact node id. Endpoints are validated first (see [`validate_edge_endpoints`]).
pub fn write_edge(mem: &dyn CognitiveMemoryOps, edge: &GoalEdge) -> SimardResult<String> {
    validate_edge_endpoints(edge)?;
    mem.store_fact_with_caller_key(
        &edge.caller_key(),
        &edge.concept(),
        &edge.content(),
        1.0,
        &edge.tags(),
        GRAPH_SOURCE,
    )
}

/// Every edge of `edge_type` whose `from` endpoint is `from`, read back out of
/// the graph. The query narrows by concept; the parsed-content `from` /
/// `edge_type` filter is authoritative and strictly type-scoped, so a
/// `decomposes_into` edge never leaks into a `depends_on` query (or vice
/// versa).
pub fn edges_of_type(
    mem: &dyn CognitiveMemoryOps,
    edge_type: GoalEdgeType,
    from: &str,
) -> SimardResult<Vec<GoalEdge>> {
    let concept = format!("goal-edge:{}", edge_type.as_str());
    let facts = mem.search_facts(&concept, RECALL_LIMIT, 0.0)?;
    let mut out: Vec<GoalEdge> = Vec::new();
    for fact in facts {
        if fact.concept != concept {
            continue;
        }
        if let Some(edge) = parse_goal_edge(&fact.content)
            && edge.edge_type == edge_type
            && edge.from == from
            && !out.contains(&edge)
        {
            out.push(edge);
        }
    }
    Ok(out)
}

/// The child goal ids of `parent`, read back from the `decomposes_into` edges.
/// This is the round-trip the acceptance bar requires: an edge written by
/// [`write_edge`] / `decompose_goal` must be queryable back here.
pub fn children_of(mem: &dyn CognitiveMemoryOps, parent: &str) -> SimardResult<Vec<String>> {
    Ok(edges_of_type(mem, GoalEdgeType::DecomposesInto, parent)?
        .into_iter()
        .map(|edge| edge.to)
        .collect())
}

/// Project a goal into the graph as a durable [`GoalNode`] anchor, carrying its
/// `done_criterion`. Idempotent via the node's caller key (`goal-node:{id}`).
/// Returns the live fact node id.
pub fn write_node(mem: &dyn CognitiveMemoryOps, node: &GoalNode) -> SimardResult<String> {
    let content = serde_json::to_string(node).map_err(|e| SimardError::InvalidGoalRecord {
        field: "goal_node".to_string(),
        reason: e.to_string(),
    })?;
    mem.store_fact_with_caller_key(
        &format!("goal-node:{}", node.id),
        "goal-node",
        &content,
        1.0,
        &["goal-node".to_string(), format!("id:{}", node.id)],
        GRAPH_SOURCE,
    )
}

/// Read a [`GoalNode`] back by goal id, if one was projected into the graph.
pub fn node_of(mem: &dyn CognitiveMemoryOps, id: &str) -> SimardResult<Option<GoalNode>> {
    let facts = mem.search_facts(&format!("goal-node id:{id}"), RECALL_LIMIT, 0.0)?;
    Ok(facts
        .into_iter()
        .filter(|fact| fact.concept == "goal-node")
        .filter_map(|fact| serde_json::from_str::<GoalNode>(&fact.content).ok())
        .find(|node| node.id == id))
}
