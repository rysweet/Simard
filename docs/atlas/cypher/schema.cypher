// Simard Code Atlas — Portable Graph Schema (engine-neutral OpenCypher)
//
// graph_backend: portable-cypher-only
//
// WHY NOT kuzu DDL: The upstream code-atlas skill emits kuzu-specific
// `CREATE NODE TABLE ... PRIMARY KEY(...)` / `CREATE REL TABLE` statements.
// Those are NOT standard OpenCypher and Simard's hard policy forbids kuzu
// (and Python). This schema is therefore expressed as portable OpenCypher that
// loads into any OpenCypher-compatible engine — Neo4j, Memgraph, or Simard's
// own embedded lbug graph store — without a kuzu dependency.
//
// The graph is engine-neutral by construction: node identity is carried in an
// `id` property and every write uses MERGE (idempotent, re-runnable). Where an
// engine supports uniqueness constraints they are declared below; engines that
// do not (or that reject the syntax) can skip this file — the MERGE-on-`id`
// pattern in the data files keeps identity correct regardless.
//
// ---------------------------------------------------------------------------
// NODE LABELS (cross-layer)
// ---------------------------------------------------------------------------
//   (:Layer)      one per atlas layer (the 8 slugs)
//   (:Service)    a deployable/runnable unit (daemon, unit, standalone mode)
//   (:Component)  an in-process subsystem / Rust module cluster
//   (:Process)    an external spawned process (recipe-runner-rs, gh, git, ...)
//   (:Port)       a network endpoint (host:port + protocol)
//   (:DataStore)  a persistent store (lbug graph, dashkey file, sockets)
//   (:Route)      an HTTP/WebSocket route (api-contracts layer)
//   (:Symbol)     an exported Rust symbol of interest (ast-lsp-bindings)
//   (:EnvVar)     an environment variable that gates behaviour
//   (:Journey)    an end-to-end user journey (user-journeys layer)
//   (:Flow)       an agentic flow (agentic-flows layer)
//   (:Phase)      one step/phase inside an agentic flow
//   (:Recipe)     a prompt_assets/simard/recipes/*.yaml agentic reasoning step
//   (:PromptAsset)a prompt_assets/simard/* asset (recipe/overseer md/policy)
//   (:Capability) a typed-OODA capability grant (RecordAction(kind))
//   (:Effect)     a typed-OODA durable effect (SpawnEngineer/FileIssue/...)
//   (:SourceRef)  a file:line anchor tying a node back to code truth
//
// RELATIONSHIP TYPES (the cross-layer links)
// ---------------------------------------------------------------------------
//   (:Layer)-[:CONTAINS]->(node)         layer membership
//   (:Service)-[:RUNS]->(:Component)     a service hosts a component
//   (:Component)-[:SPAWNS]->(:Process)   subprocess coordination
//   (:Service)-[:EXPOSES]->(:Port)       bind/listen
//   (:Service)-[:READS_FROM|WRITES_TO]->(:DataStore)
//   (:Service)-[:EXPOSES_ROUTE]->(:Route)
//   (:Route)-[:GUARDED_BY]->(:Symbol)    middleware/auth guard
//   (:Component)-[:DEPENDS_ON]->(:Component)
//   (:Component)-[:CALLS]->(:Symbol)
//   (:node)-[:USES_ENV]->(:EnvVar)
//   (:Journey)-[:TRAVERSES]->(node)      journey step through any layer
//   (:Flow)-[:HAS_PHASE]->(:Phase)       agentic flow decomposition (ordered by :Phase.seq)
//   (:Phase)-[:NEXT]->(:Phase)           phase ordering within a flow
//   (:Phase)-[:INVOKES]->(:Recipe)       a phase runs an agentic recipe
//   (:Recipe)-[:DEFINED_IN]->(:PromptAsset) recipe backed by a prompt asset
//   (:Flow)-[:DRIVES]->(:Flow)           one agentic flow triggers another
//   (:Flow)-[:READS_FROM|WRITES_TO]->(:DataStore)  memory/ledger access
//   (:Flow)-[:USES]->(:PromptAsset)      flow reads a prompt asset (policy/md)
//   (:Capability)-[:AUTHORIZES]->(:Effect)  capability gates an effect
//   (:Flow)-[:PRODUCES]->(:Effect)       typed-OODA effect emission
//   (:Flow)-[:TOUCHES]->(node)           cross-layer link to any atlas node
//   (:node)-[:EVIDENCED_BY]->(:SourceRef)  file:line provenance
//
// ---------------------------------------------------------------------------
// Uniqueness constraints (Neo4j/Memgraph syntax). Safe to skip on engines that
// do not support this DDL; identity is still enforced by MERGE-on-`id`.
// ---------------------------------------------------------------------------
CREATE CONSTRAINT layer_id IF NOT EXISTS FOR (n:Layer) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT service_id IF NOT EXISTS FOR (n:Service) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT component_id IF NOT EXISTS FOR (n:Component) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT process_id IF NOT EXISTS FOR (n:Process) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT port_id IF NOT EXISTS FOR (n:Port) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT datastore_id IF NOT EXISTS FOR (n:DataStore) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT route_id IF NOT EXISTS FOR (n:Route) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT symbol_id IF NOT EXISTS FOR (n:Symbol) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT envvar_id IF NOT EXISTS FOR (n:EnvVar) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT journey_id IF NOT EXISTS FOR (n:Journey) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT flow_id IF NOT EXISTS FOR (n:Flow) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT phase_id IF NOT EXISTS FOR (n:Phase) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT recipe_id IF NOT EXISTS FOR (n:Recipe) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT promptasset_id IF NOT EXISTS FOR (n:PromptAsset) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT capability_id IF NOT EXISTS FOR (n:Capability) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT effect_id IF NOT EXISTS FOR (n:Effect) REQUIRE n.id IS UNIQUE;
CREATE CONSTRAINT sourceref_id IF NOT EXISTS FOR (n:SourceRef) REQUIRE n.id IS UNIQUE;
