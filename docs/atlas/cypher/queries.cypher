// Simard Code Atlas — Ready-to-run example queries (portable OpenCypher)
// graph_backend: portable-cypher-only. Works on Neo4j / Memgraph / lbug.
//
// Load order:
//   1. schema.cypher          (optional constraints)
//   2. atlas-layers.cypher
//   3. atlas-services.cypher
//   4. atlas-relationships.cypher
//   5. atlas-agentic.cypher   (Layer 9 agentic flows)
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

// ---- Agentic flows (Layer 9) ----------------------------------------------

// Q11. Trace an agentic flow's phases in order (change the id to any flow).
MATCH (f:Flow {id:'ooda-loop'})-[:HAS_PHASE]->(p:Phase)
RETURN f.id AS flow, p.seq AS step, p.id AS phase, p.detail AS detail, p.evidence AS source
ORDER BY p.seq;

// Q12. Which agentic phases invoke a recipe, and which prompt asset backs it?
MATCH (p:Phase)-[:INVOKES]->(r:Recipe)
OPTIONAL MATCH (r)-[:DEFINED_IN]->(a:PromptAsset)
RETURN p.id AS phase, r.id AS recipe, r.file AS recipe_file, a.id AS backing_policy;

// Q13. Capability -> effect authorization matrix (typed-OODA blast radius).
MATCH (c:Capability)-[:AUTHORIZES]->(e:Effect)
RETURN c.grant AS capability, e.id AS effect, e.detail AS does, e.evidence AS source
ORDER BY effect;

// Q14. Cross-flow orchestration graph: which flow drives which, plus stores touched.
MATCH (f:Flow)-[:DRIVES]->(g:Flow)
OPTIONAL MATCH (f)-[rel:READS_FROM|WRITES_TO]->(d:DataStore)
RETURN f.id AS flow, collect(DISTINCT g.id) AS drives, collect(DISTINCT d.id) AS stores;

// Q15. Flow-to-journey linkage (how Layer 9 flows surface as Layer 8 journeys).
MATCH (f:Flow)-[:TOUCHES]->(j:Journey)
RETURN f.id AS flow, collect(j.id) AS journeys;

// Q16. Trace the overseer autonomous verify+merge sub-pipeline in order (merge_ops
// M2): verify -> poll-until-green -> agentic MergeJudge -> squash-merge ->
// dual-channel notify. (PR draft-exclusion narrowing #4339 is an observe-stage
// projection — see ovr.observe — not part of this act sub-pipeline.)
MATCH (p:Phase) WHERE p.id STARTS WITH 'ovr.act.merge.'
RETURN p.seq AS step, p.id AS phase, p.detail AS detail, p.evidence AS source
ORDER BY p.seq;

// Q17. The cognitive-memory recall precision gate (MIN_CLEAN_NEEDLE_LEN) as a
// phase of the memory-recall flow.
MATCH (f:Flow {id:'memory-recall'})-[:HAS_PHASE]->(p:Phase)
RETURN f.id AS flow, p.id AS phase, p.detail AS detail, p.evidence AS source;
