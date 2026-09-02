// Simard Code Atlas — Cross-layer relationships (the links between layers)
// Portable OpenCypher. Idempotent (MATCH + MERGE). graph_backend: portable-cypher-only.
// Run AFTER schema.cypher, atlas-layers.cypher, atlas-services.cypher.

// ---- Layer membership (CONTAINS) -------------------------------------------
MATCH (l:Layer {id:'runtime-topology'}), (s:Service)     MERGE (l)-[:CONTAINS]->(s);
MATCH (l:Layer {id:'runtime-topology'}), (p:Process)     MERGE (l)-[:CONTAINS]->(p);
MATCH (l:Layer {id:'runtime-topology'}), (n:Port)        MERGE (l)-[:CONTAINS]->(n);
MATCH (l:Layer {id:'runtime-topology'}), (d:DataStore)   MERGE (l)-[:CONTAINS]->(d);
MATCH (l:Layer {id:'service-components'}), (c:Component)  MERGE (l)-[:CONTAINS]->(c);
MATCH (l:Layer {id:'api-contracts'}), (r:Route)          MERGE (l)-[:CONTAINS]->(r);
MATCH (l:Layer {id:'user-journeys'}), (j:Journey)        MERGE (l)-[:CONTAINS]->(j);

// ---- Service RUNS Component -------------------------------------------------
MATCH (s:Service {id:'simard-daemon'}), (c:Component)
  WHERE c.id IN ['ooda-loop','dashboard','signal-embed','memory-ipc','cognitive-threads','overseer']
  MERGE (s)-[:RUNS]->(c);
MATCH (s:Service {id:'simard-ooda.service'}), (d:Service {id:'simard-daemon'}) MERGE (s)-[:RUNS]->(d);
MATCH (s:Service {id:'standalone-dashboard'}), (c:Component {id:'dashboard'})  MERGE (s)-[:RUNS]->(c);

// ---- Service EXPOSES Port ---------------------------------------------------
MATCH (c:Component {id:'dashboard'}), (n:Port {id:'dashboard-8080'})   MERGE (c)-[:EXPOSES]->(n);
MATCH (c:Component {id:'signal-embed'}), (n:Port {id:'signal-rpc-7583'}) MERGE (c)-[:EXPOSES]->(n);
MATCH (c:Component {id:'overseer'}), (n:Port {id:'signal-rpc-7583'})   MERGE (c)-[:EXPOSES]->(n);

// ---- Component SPAWNS Process (subprocess coordination) --------------------
MATCH (c:Component {id:'ooda-loop'}), (p:Process)
  WHERE p.id IN ['recipe-runner-rs','amplihack','llm-provider-agent','git','gh']
  MERGE (c)-[:SPAWNS]->(p);
MATCH (c:Component {id:'overseer'}), (p:Process)
  WHERE p.id IN ['amplihack','gh','git']
  MERGE (c)-[:SPAWNS]->(p);
MATCH (c:Component {id:'ooda-loop'}), (p:Process)
  WHERE p.id IN ['systemctl','cargo','curl','tmux','journalctl']
  MERGE (c)-[:SPAWNS]->(p);

// ---- Component READS_FROM / WRITES_TO DataStore ---------------------------
MATCH (c:Component {id:'memory-ipc'}), (d:DataStore {id:'lbug-cognitive'}) MERGE (c)-[:WRITES_TO]->(d);
MATCH (c:Component {id:'memory-ipc'}), (d:DataStore {id:'lbug-cognitive'}) MERGE (c)-[:READS_FROM]->(d);
MATCH (c:Component {id:'memory-ipc'}), (d:DataStore {id:'memory-sock'})    MERGE (c)-[:EXPOSES]->(d);
MATCH (c:Component {id:'dashboard'}), (d:DataStore {id:'dashkey-file'})    MERGE (c)-[:READS_FROM]->(d);

// ---- Route GUARDED_BY Symbol -----------------------------------------------
MATCH (r:Route {id:'dashboard-api-*'}), (y:Symbol {id:'require_auth'}) MERGE (r)-[:GUARDED_BY]->(y);
MATCH (c:Component {id:'signal-embed'}), (y:Symbol {id:'signal-gating'}) MERGE (c)-[:GUARDED_BY]->(y);
MATCH (c:Component {id:'dashboard'}), (r:Route) MERGE (c)-[:EXPOSES_ROUTE]->(r);

// ---- USES_ENV --------------------------------------------------------------
MATCH (c:Component {id:'signal-embed'}), (e:EnvVar {id:'SIMARD_SIGNAL_ENABLED'})  MERGE (c)-[:USES_ENV]->(e);
MATCH (c:Component {id:'overseer'}), (e:EnvVar {id:'SIMARD_SIGNAL_RPC_ADDR'})     MERGE (c)-[:USES_ENV]->(e);
MATCH (c:Component {id:'memory-ipc'}), (e:EnvVar {id:'SIMARD_MEMORY_SOCKET'})     MERGE (c)-[:USES_ENV]->(e);

// ---- Journey TRAVERSES nodes across layers (the cross-layer story) ---------
MATCH (j:Journey {id:'ooda-cycle'}), (c:Component {id:'ooda-loop'})        MERGE (j)-[:TRAVERSES]->(c);
MATCH (j:Journey {id:'ooda-cycle'}), (p:Process {id:'recipe-runner-rs'})   MERGE (j)-[:TRAVERSES]->(p);
MATCH (j:Journey {id:'memory-recall'}), (c:Component {id:'memory-ipc'})    MERGE (j)-[:TRAVERSES]->(c);
MATCH (j:Journey {id:'memory-recall'}), (d:DataStore {id:'lbug-cognitive'}) MERGE (j)-[:TRAVERSES]->(d);
MATCH (j:Journey {id:'signal-reply'}), (c:Component {id:'signal-embed'})   MERGE (j)-[:TRAVERSES]->(c);
MATCH (j:Journey {id:'signal-reply'}), (n:Port {id:'signal-rpc-7583'})     MERGE (j)-[:TRAVERSES]->(n);
MATCH (j:Journey {id:'overseer-blocked-goal'}), (c:Component {id:'overseer'}) MERGE (j)-[:TRAVERSES]->(c);
MATCH (j:Journey {id:'overseer-blocked-goal'}), (p:Process {id:'amplihack'}) MERGE (j)-[:TRAVERSES]->(p);
MATCH (j:Journey {id:'engineer-pr'}), (c:Component {id:'ooda-loop'})       MERGE (j)-[:TRAVERSES]->(c);
MATCH (j:Journey {id:'engineer-pr'}), (p:Process {id:'gh'})               MERGE (j)-[:TRAVERSES]->(p);
MATCH (j:Journey {id:'self-deploy'}), (p:Process {id:'systemctl'})         MERGE (j)-[:TRAVERSES]->(p);
MATCH (j:Journey {id:'self-deploy'}), (p:Process {id:'cargo'})            MERGE (j)-[:TRAVERSES]->(p);
