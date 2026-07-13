use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteNodeKind {
    GoalSessionEntry,
    RecipeRunner,
    SemanticAgentStep,
    ScopedCapabilityTools,
    TerminalHandler,
    OutcomeLedger,
    EffectOutbox,
    EffectExecutor,
    ProseParser,
    LegacyFallback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteNode {
    pub id: &'static str,
    pub kind: RouteNodeKind,
}

#[derive(Clone, Debug)]
pub struct RouteGraph {
    nodes: BTreeMap<&'static str, RouteNode>,
    edges: BTreeMap<&'static str, Vec<&'static str>>,
}

impl RouteGraph {
    pub fn goal_session_entry(&self) -> &'static str {
        "goal-session"
    }

    pub fn reachable_from(&self, start: &'static str) -> Vec<RouteNode> {
        let mut pending = vec![start];
        let mut visited = BTreeSet::new();
        let mut result = Vec::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            if let Some(node) = self.nodes.get(id) {
                result.push(node.clone());
            }
            if let Some(next) = self.edges.get(id) {
                pending.extend(next.iter().copied());
            }
        }
        result
    }
}

pub struct TypedGoalSessionRoute;

impl TypedGoalSessionRoute {
    pub fn dependency_graph() -> RouteGraph {
        let nodes = [
            ("goal-session", RouteNodeKind::GoalSessionEntry),
            ("recipe-runner", RouteNodeKind::RecipeRunner),
            ("observe", RouteNodeKind::SemanticAgentStep),
            ("orient", RouteNodeKind::SemanticAgentStep),
            ("decide", RouteNodeKind::SemanticAgentStep),
            ("actor", RouteNodeKind::SemanticAgentStep),
            ("tools", RouteNodeKind::ScopedCapabilityTools),
            ("handler", RouteNodeKind::TerminalHandler),
            ("ledger", RouteNodeKind::OutcomeLedger),
            ("outbox", RouteNodeKind::EffectOutbox),
            ("effects", RouteNodeKind::EffectExecutor),
        ]
        .into_iter()
        .map(|(id, kind)| (id, RouteNode { id, kind }))
        .collect();
        let edges = BTreeMap::from([
            ("goal-session", vec!["recipe-runner"]),
            ("recipe-runner", vec!["observe"]),
            ("observe", vec!["orient"]),
            ("orient", vec!["decide"]),
            ("decide", vec!["actor"]),
            ("actor", vec!["tools"]),
            ("tools", vec!["handler"]),
            ("handler", vec!["ledger", "outbox"]),
            ("outbox", vec!["effects"]),
        ]);
        RouteGraph { nodes, edges }
    }
}
