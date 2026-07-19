// Simard Code Atlas — Ready-to-run example queries (portable OpenCypher)
// graph_backend: portable-cypher-only. Works on Neo4j / Memgraph / lbug.
//
// Load order:
//   1. schema.cypher          (optional constraints)
//   2. atlas-layers.cypher
//   3. atlas-services.cypher
//   4. atlas-relationships.cypher
//   5. atlas-agentic.cypher      (agentic flows: flows, phases, recipes, prompt assets)
// Then run any query below.

// Q1. What does each layer contain?
MATCH (l:Layer)-[:CONTAINS]->(n)
RETURN l.id AS layer, labels(n)[0] AS kind, n.id AS node
ORDER BY layer, kind, node;

// Q2. Every network endpoint the daemon exposes, with the component behind it.
MATCH (c:Component)-[:EXPOSES]->(n:Port)
RETURN c.id AS component, n.endpoint AS endpoint, n.protocol AS protocol, n.evidence AS source;

// Q3. Attack/coordination surface: all external processes the OODA loop spawns.
MATCH (:Component {id:'ooda-loop'})-[:SPAWNS]->(p:Process)
RETURN p.id AS process, p.evidence AS source ORDER BY process;

// Q4. Data-store reachability: which components read or write persistent state?
MATCH (c:Component)-[rel:READS_FROM|WRITES_TO]->(d:DataStore)
RETURN c.id AS component, type(rel) AS access, d.id AS store, d.detail AS detail;

// Q5. Auth coverage: unguarded dashboard routes (should be empty except explicit bypasses).
MATCH (r:Route)
WHERE r.auth CONTAINS 'bypass'
RETURN r.id AS bypass_route, r.evidence AS source;

// Q6. Trace a journey end-to-end across layers (change the id to any journey).
MATCH (j:Journey {id:'overseer-blocked-goal'})-[:TRAVERSES]->(n)
RETURN j.id AS journey, labels(n)[0] AS layer_node_kind, n.id AS node, n.evidence AS source;

// Q7. Env-var blast radius: what breaks if an env var is unset/changed?
MATCH (n)-[:USES_ENV]->(e:EnvVar)
RETURN e.id AS env_var, collect(n.id) AS affects, e.evidence AS declared_at;

// Q8. Orphan check (atlas hygiene): nodes with no relationships at all.
MATCH (n)
WHERE NOT (n)--() AND NOT n:Layer
RETURN labels(n)[0] AS kind, n.id AS orphan;

// Q9. Which journeys depend on the Signal RPC port (blast radius of :7583 down)?
MATCH (j:Journey)-[:TRAVERSES]->(:Port {id:'signal-rpc-7583'})
RETURN collect(j.id) AS journeys_needing_signal_rpc;

// Q10. Cross-layer path: from a Service to the DataStores it can reach (any depth).
MATCH path = (s:Service {id:'simard-daemon'})-[:RUNS|SPAWNS|EXPOSES|READS_FROM|WRITES_TO*1..4]->(d:DataStore)
RETURN d.id AS reachable_store, length(path) AS hops
ORDER BY hops;

// ---------------------------------------------------------------------------
// Agentic-flows queries (require atlas-agentic.cypher loaded)
// ---------------------------------------------------------------------------

// Q11. Trace an agentic flow's ordered phases (change the flow id).
MATCH (f:Flow {id:'ooda-loop'})-[:HAS_PHASE]->(p:Phase)
OPTIONAL MATCH (p)-[:NEXT]->(next:Phase)
RETURN f.id AS flow, p.id AS phase, p.name AS name, next.id AS next_phase, p.evidence AS source;

// Q12. Which recipe embeds which prompt asset (agentic decision provenance)?
MATCH (r:Recipe)-[:EMBEDS]->(a:PromptAsset)
RETURN r.id AS recipe, r.file AS recipe_file, a.id AS prompt_asset, a.role AS role
ORDER BY recipe;

// Q13. Cross-flow seams: how do the agentic flows link together across layers?
MATCH (a:Flow)-[l:LINKS_TO]->(b:Flow)
RETURN a.id AS from_flow, b.id AS to_flow, l.via AS via
ORDER BY from_flow;

// Q14. Blast radius of the lbug cognitive store: which flows read/write it?
MATCH (f:Flow)-[rel:READS_FROM|WRITES_TO]->(d:DataStore {id:'lbug-cognitive'})
RETURN d.id AS store, collect(DISTINCT f.id + '(' + type(rel) + ')') AS flows;

// Q15. Which flows spawn the amplihack recipe runner (agentic subprocess surface)?
MATCH (f:Flow)-[:SPAWNS]->(p:Process)
WHERE p.id IN ['recipe-runner-rs','amplihack']
RETURN p.id AS process, collect(f.id) AS spawned_by;

// Q16. Full agentic reachability: from the OODA loop to every flow it links to (any depth).
MATCH path = (f:Flow {id:'ooda-loop'})-[:LINKS_TO*1..4]->(other:Flow)
RETURN DISTINCT other.id AS reachable_flow, length(path) AS hops
ORDER BY hops, reachable_flow;
