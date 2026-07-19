// Simard Code Atlas — Agentic Flows graph (portable OpenCypher, engine-neutral)
// graph_backend: portable-cypher-only  (NO kuzu, NO python; loads into lbug / Neo4j / Memgraph)
//
// Run AFTER schema.cypher, atlas-layers.cypher, atlas-services.cypher.
// Idempotent: every write is MERGE-on-`id`.
//
// This file models the six agentic control planes and how they interlock across
// layers. It introduces agentic node labels in addition to the cross-layer labels
// declared in schema.cypher:
//   (:Flow)        one of the six agentic flows (+ the linkage map)
//   (:Phase)       a step within a flow (OODA phase, overseer stage, ...)
//   (:Recipe)      an amplihack recipe (recipe-runner-rs target)
//   (:PromptAsset) a prompt_assets/simard/* file
//   (:Capability)  a typed-OODA capability grant / effect kind
//   (:DataStore)   reused from schema.cypher (lbug graph, sqlite ledger)
// Relationship types reused: :CONTAINS, :TRAVERSES, :DEPENDS_ON, :SPAWNS,
//   :READS_FROM, :WRITES_TO, :EVIDENCED_BY, plus agentic edges:
//   (:Flow)-[:HAS_PHASE]->(:Phase)         phase membership
//   (:Phase)-[:NEXT]->(:Phase)             ordered pipeline
//   (:Flow)-[:USES_RECIPE]->(:Recipe)      recipe invocation
//   (:Recipe)-[:EMBEDS]->(:PromptAsset)    recipe embeds prompt text
//   (:Flow)-[:LOADS]->(:PromptAsset)       runtime/compile-time asset load
//   (:Flow)-[:LINKS_TO]->(:Flow)           cross-flow seam

// ---------------------------------------------------------------------------
// Layer node (#9)
// ---------------------------------------------------------------------------
MERGE (l:Layer {id: 'agentic-flows'})
  SET l.name = 'Agentic Flows',
      l.description = 'OODA loop, overseer tick, typed-OODA capability/effect model, recipes, prompt assets, cognitive recall, and their cross-layer linkage',
      l.diagram = 'docs/atlas/agentic-flows/';

// ---------------------------------------------------------------------------
// Flows
// ---------------------------------------------------------------------------
MERGE (f:Flow {id: 'ooda-loop'})       SET f.name = 'Outer OODA loop',        f.entry = 'ooda_loop::cycle::run_ooda_cycle', f.evidence = 'src/ooda_loop/cycle.rs';
MERGE (f:Flow {id: 'overseer-tick'})   SET f.name = 'Overseer tick',          f.entry = 'overseer::wiring::overseer_tick',  f.evidence = 'src/overseer/wiring.rs';
MERGE (f:Flow {id: 'typed-ooda'})      SET f.name = 'Typed-OODA capability/effect', f.entry = 'typed_ooda::route', f.evidence = 'src/typed_ooda/route.rs';
MERGE (f:Flow {id: 'recipes'})         SET f.name = 'Recipes (recipe-runner)', f.entry = 'ooda_brain::recipe_brain', f.evidence = 'src/ooda_brain/recipe_brain.rs';
MERGE (f:Flow {id: 'prompt-assets'})   SET f.name = 'Prompt assets',          f.entry = 'prompt_assets::FilePromptAssetStore', f.evidence = 'src/prompt_assets.rs';
MERGE (f:Flow {id: 'cognitive-recall'}) SET f.name = 'Cognitive-memory recall', f.entry = 'ooda_loop::cycle::build_objective_probe', f.evidence = 'src/ooda_loop/cycle.rs';
MERGE (f:Flow {id: 'agentic-linkage'}) SET f.name = 'Cross-layer linkage',     f.entry = '(map)', f.evidence = 'docs/atlas/agentic-flows/agentic-linkage.mmd';

// ---------------------------------------------------------------------------
// OODA loop phases (ordered)
// ---------------------------------------------------------------------------
MERGE (p:Phase {id: 'ooda.observe'})  SET p.name = 'Observe',  p.evidence = 'src/ooda_loop/observe.rs';
MERGE (p:Phase {id: 'ooda.prepare'})  SET p.name = 'Prepare (recall)', p.evidence = 'src/ooda_loop/cycle.rs';
MERGE (p:Phase {id: 'ooda.orient'})   SET p.name = 'Orient',   p.detail = 'orient_with_brain | orient', p.evidence = 'src/ooda_loop/orient.rs';
MERGE (p:Phase {id: 'ooda.decide'})   SET p.name = 'Decide',   p.detail = 'decide_with_brain | decide', p.evidence = 'src/ooda_loop/decide.rs';
MERGE (p:Phase {id: 'ooda.coverage'}) SET p.name = 'Coverage', p.evidence = 'src/ooda_loop/coverage.rs';
MERGE (p:Phase {id: 'ooda.act'})      SET p.name = 'Act',      p.evidence = 'src/ooda_actions/mod.rs';
MERGE (p:Phase {id: 'ooda.review'})   SET p.name = 'Review / no-progress breaker', p.evidence = 'src/ooda_loop/no_progress.rs';
MERGE (p:Phase {id: 'ooda.curate'})   SET p.name = 'Curate',   p.evidence = 'src/ooda_loop/curate.rs';

MATCH (f:Flow {id:'ooda-loop'}), (p:Phase) WHERE p.id STARTS WITH 'ooda.' MERGE (f)-[:HAS_PHASE]->(p);
MATCH (a:Phase {id:'ooda.observe'}),(b:Phase {id:'ooda.prepare'})  MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'ooda.prepare'}),(b:Phase {id:'ooda.orient'})   MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'ooda.orient'}), (b:Phase {id:'ooda.decide'})   MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'ooda.decide'}), (b:Phase {id:'ooda.coverage'}) MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'ooda.coverage'}),(b:Phase {id:'ooda.act'})     MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'ooda.act'}),    (b:Phase {id:'ooda.review'})   MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'ooda.review'}), (b:Phase {id:'ooda.curate'})   MERGE (a)-[:NEXT]->(b);

// ---------------------------------------------------------------------------
// Overseer tick stages (ordered)
// ---------------------------------------------------------------------------
MERGE (p:Phase {id: 'overseer.reconcile'}) SET p.name = 'Reconcile + reap claims', p.evidence = 'src/overseer/mod.rs';
MERGE (p:Phase {id: 'overseer.observe'})   SET p.name = 'Observe ObservedState',    p.evidence = 'src/overseer/sensor.rs';
MERGE (p:Phase {id: 'overseer.signal'})    SET p.name = 'Signal',                   p.evidence = 'src/overseer/signal.rs';
MERGE (p:Phase {id: 'overseer.orient'})    SET p.name = 'Orient (dedup in-flight)', p.evidence = 'src/overseer/observer.rs';
MERGE (p:Phase {id: 'overseer.rootcause'}) SET p.name = 'Root-cause WHY (#2635)',   p.evidence = 'src/overseer/root_cause.rs';
MERGE (p:Phase {id: 'overseer.decide'})    SET p.name = 'Decide intervention',      p.evidence = 'src/overseer/mod.rs';
MERGE (p:Phase {id: 'overseer.gate'})      SET p.name = 'Guardrails gate',          p.evidence = 'src/overseer/guardrails.rs';
MERGE (p:Phase {id: 'overseer.act'})       SET p.name = 'Act (dispatch intervention)', p.evidence = 'src/overseer/mod.rs';
MERGE (p:Phase {id: 'overseer.record'})    SET p.name = 'Record occurrence',        p.evidence = 'src/overseer/wiring.rs';

MATCH (f:Flow {id:'overseer-tick'}), (p:Phase) WHERE p.id STARTS WITH 'overseer.' MERGE (f)-[:HAS_PHASE]->(p);
MATCH (a:Phase {id:'overseer.reconcile'}),(b:Phase {id:'overseer.observe'})   MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'overseer.observe'}),  (b:Phase {id:'overseer.signal'})    MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'overseer.signal'}),   (b:Phase {id:'overseer.orient'})    MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'overseer.orient'}),   (b:Phase {id:'overseer.rootcause'}) MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'overseer.rootcause'}),(b:Phase {id:'overseer.decide'})    MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'overseer.decide'}),   (b:Phase {id:'overseer.gate'})      MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'overseer.gate'}),     (b:Phase {id:'overseer.act'})       MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase {id:'overseer.act'}),      (b:Phase {id:'overseer.record'})    MERGE (a)-[:NEXT]->(b);

// ---------------------------------------------------------------------------
// Recipes + amplihack recipe runner
// ---------------------------------------------------------------------------
MERGE (r:Recipe {id: 'ooda-orient'})           SET r.file = 'prompt_assets/simard/recipes/ooda-orient.yaml';
MERGE (r:Recipe {id: 'ooda-decide'})           SET r.file = 'prompt_assets/simard/recipes/ooda-decide.yaml';
MERGE (r:Recipe {id: 'goal-decomposition'})    SET r.file = 'prompt_assets/simard/recipes/goal-decomposition.yaml';
MERGE (r:Recipe {id: 'goal-session-actor'})    SET r.file = 'prompt_assets/simard/recipes/goal-session-actor.yaml';
MERGE (r:Recipe {id: 'merge-readiness-judge'}) SET r.file = 'prompt_assets/simard/recipes/merge-readiness-judge.yaml';
MERGE (r:Recipe {id: 'ecosystem-observe'})     SET r.file = 'prompt_assets/simard/recipes/ecosystem-observe.yaml';
MERGE (r:Recipe {id: 'smart-orchestrator'})    SET r.detail = 'amplihack recipe run smart-orchestrator', r.evidence = 'src/overseer/launch.rs';
MERGE (r:Recipe {id: 'default-workflow'})      SET r.detail = 'amplihack default-workflow (development tasks)';
MERGE (r:Recipe {id: 'investigation-workflow'}) SET r.detail = 'amplihack investigation-workflow (analysis tasks)';

MATCH (f:Flow {id:'recipes'}), (r:Recipe) MERGE (f)-[:CONTAINS]->(r);
MATCH (f:Flow {id:'ooda-loop'}), (r:Recipe) WHERE r.id IN ['ooda-orient','ooda-decide'] MERGE (f)-[:USES_RECIPE]->(r);
MATCH (f:Flow {id:'typed-ooda'}), (r:Recipe {id:'goal-session-actor'}) MERGE (f)-[:USES_RECIPE]->(r);
MATCH (f:Flow {id:'overseer-tick'}), (r:Recipe) WHERE r.id IN ['smart-orchestrator','ecosystem-observe','merge-readiness-judge'] MERGE (f)-[:USES_RECIPE]->(r);

// recipe-runner-rs process (defined in atlas-services.cypher) is spawned by the recipes flow
MATCH (f:Flow {id:'recipes'}), (proc:Process {id:'recipe-runner-rs'}) MERGE (f)-[:SPAWNS]->(proc);
MATCH (f:Flow {id:'recipes'}), (proc:Process {id:'amplihack'})        MERGE (f)-[:SPAWNS]->(proc);

// ---------------------------------------------------------------------------
// Prompt assets
// ---------------------------------------------------------------------------
MERGE (a:PromptAsset {id: 'engineer_system.md'})       SET a.path = 'prompt_assets/simard/engineer_system.md', a.role = 'engineer session system prompt';
MERGE (a:PromptAsset {id: 'ooda_orient.md'})           SET a.path = 'prompt_assets/simard/ooda_orient.md', a.role = 'orient urgency framework';
MERGE (a:PromptAsset {id: 'ooda_decide.md'})           SET a.path = 'prompt_assets/simard/ooda_decide.md', a.role = 'decide action-kind guidelines';
MERGE (a:PromptAsset {id: 'goal_decomposition.md'})    SET a.path = 'prompt_assets/simard/goal_decomposition.md', a.role = 'goal decomposition strategy';
MERGE (a:PromptAsset {id: 'goal_session_objective.md'}) SET a.path = 'prompt_assets/simard/goal_session_objective.md', a.role = 'goal session objective + schema';
MERGE (a:PromptAsset {id: 'goal_session_identity.md'}) SET a.path = 'prompt_assets/simard/goal_session_identity.md', a.role = 'compile-time embed (src/ooda_actions/goal_session/input.rs:126)';
MERGE (a:PromptAsset {id: 'merge_readiness_judge.md'}) SET a.path = 'prompt_assets/simard/merge_readiness_judge.md', a.role = 'merge readiness criteria';
MERGE (a:PromptAsset {id: 'goal-session-capabilities.toml'}) SET a.path = 'prompt_assets/simard/policies/goal-session-capabilities.toml', a.role = 'compile-time TRUSTED_POLICY (typed_ooda/route.rs:20)';

MATCH (f:Flow {id:'prompt-assets'}), (a:PromptAsset) MERGE (f)-[:CONTAINS]->(a);
MATCH (r:Recipe {id:'ooda-orient'}), (a:PromptAsset {id:'ooda_orient.md'}) MERGE (r)-[:EMBEDS]->(a);
MATCH (r:Recipe {id:'ooda-decide'}), (a:PromptAsset {id:'ooda_decide.md'}) MERGE (r)-[:EMBEDS]->(a);
MATCH (r:Recipe {id:'goal-decomposition'}), (a:PromptAsset {id:'goal_decomposition.md'}) MERGE (r)-[:EMBEDS]->(a);
MATCH (r:Recipe {id:'goal-session-actor'}), (a:PromptAsset {id:'goal_session_objective.md'}) MERGE (r)-[:EMBEDS]->(a);
MATCH (r:Recipe {id:'merge-readiness-judge'}), (a:PromptAsset {id:'merge_readiness_judge.md'}) MERGE (r)-[:EMBEDS]->(a);
MATCH (f:Flow {id:'typed-ooda'}), (a:PromptAsset {id:'goal-session-capabilities.toml'}) MERGE (f)-[:LOADS]->(a);

// ---------------------------------------------------------------------------
// Typed-OODA capabilities / effects
// ---------------------------------------------------------------------------
MERGE (c:Capability {id: 'SpawnEngineer'}) SET c.kind = 'action', c.evidence = 'src/typed_ooda/types.rs';
MERGE (c:Capability {id: 'FileIssue'})     SET c.kind = 'action', c.evidence = 'src/typed_ooda/types.rs';
MERGE (c:Capability {id: 'RequestMerge'})  SET c.kind = 'action', c.evidence = 'src/typed_ooda/types.rs';
MERGE (c:Capability {id: 'RequestDeploy'}) SET c.kind = 'action', c.evidence = 'src/typed_ooda/types.rs';
MATCH (f:Flow {id:'typed-ooda'}), (c:Capability) MERGE (f)-[:CONTAINS]->(c);

// Typed-OODA durable stores (SQLite ledger + engineer claims)
MERGE (d:DataStore {id: 'typed-ooda-ledger'}) SET d.detail = 'typed-ooda/outcomes.sqlite3 (terminal_outcomes, effect_jobs, engineer_claims, actor_sessions)', d.evidence = 'src/typed_ooda/schema.rs';
MATCH (f:Flow {id:'typed-ooda'}), (d:DataStore {id:'typed-ooda-ledger'}) MERGE (f)-[:WRITES_TO]->(d);
MATCH (f:Flow {id:'typed-ooda'}), (d:DataStore {id:'typed-ooda-ledger'}) MERGE (f)-[:READS_FROM]->(d);
MATCH (l:Layer {id:'agentic-flows'}), (d:DataStore {id:'typed-ooda-ledger'}) MERGE (l)-[:CONTAINS]->(d);

// ---------------------------------------------------------------------------
// Cognitive-recall reads the lbug store (declared in atlas-services.cypher)
// ---------------------------------------------------------------------------
MATCH (f:Flow {id:'cognitive-recall'}), (d:DataStore {id:'lbug-cognitive'}) MERGE (f)-[:READS_FROM]->(d);
MATCH (f:Flow {id:'cognitive-recall'}), (d:DataStore {id:'lbug-cognitive'}) MERGE (f)-[:WRITES_TO]->(d);
MATCH (f:Flow {id:'ooda-loop'}), (d:DataStore {id:'lbug-cognitive'}) MERGE (f)-[:READS_FROM]->(d);
MATCH (f:Flow {id:'overseer-tick'}), (d:DataStore {id:'lbug-cognitive'}) MERGE (f)-[:READS_FROM]->(d);

// ---------------------------------------------------------------------------
// Layer membership + tie agentic flows to existing components (service-components)
// ---------------------------------------------------------------------------
MATCH (l:Layer {id:'agentic-flows'}), (f:Flow) MERGE (l)-[:CONTAINS]->(f);
MATCH (f:Flow {id:'ooda-loop'}),     (c:Component {id:'ooda-loop'}) MERGE (f)-[:DEPENDS_ON]->(c);
MATCH (f:Flow {id:'overseer-tick'}), (c:Component {id:'overseer'})  MERGE (f)-[:DEPENDS_ON]->(c);
MATCH (f:Flow {id:'cognitive-recall'}), (c:Component {id:'memory-ipc'}) MERGE (f)-[:DEPENDS_ON]->(c);

// ---------------------------------------------------------------------------
// Cross-flow seams (the "how they link together across layers" story)
// ---------------------------------------------------------------------------
MATCH (a:Flow {id:'ooda-loop'}),      (b:Flow {id:'cognitive-recall'}) MERGE (a)-[:LINKS_TO {via:'prepare phase'}]->(b);
MATCH (a:Flow {id:'ooda-loop'}),      (b:Flow {id:'recipes'})          MERGE (a)-[:LINKS_TO {via:'orient/decide brain'}]->(b);
MATCH (a:Flow {id:'ooda-loop'}),      (b:Flow {id:'typed-ooda'})       MERGE (a)-[:LINKS_TO {via:'Act: AdvanceGoal/SpawnEngineer'}]->(b);
MATCH (a:Flow {id:'typed-ooda'}),     (b:Flow {id:'recipes'})          MERGE (a)-[:LINKS_TO {via:'goal-session-actor'}]->(b);
MATCH (a:Flow {id:'recipes'}),        (b:Flow {id:'prompt-assets'})    MERGE (a)-[:LINKS_TO {via:'embeds prompt text'}]->(b);
MATCH (a:Flow {id:'overseer-tick'}),  (b:Flow {id:'recipes'})          MERGE (a)-[:LINKS_TO {via:'LaunchRecipe/Escalate'}]->(b);
MATCH (a:Flow {id:'overseer-tick'}),  (b:Flow {id:'cognitive-recall'}) MERGE (a)-[:LINKS_TO {via:'recall_pass/record_occurrence'}]->(b);
MATCH (a:Flow {id:'overseer-tick'}),  (b:Flow {id:'typed-ooda'})       MERGE (a)-[:LINKS_TO {via:'reap_stale_engineer_claims'}]->(b);
MATCH (link:Flow {id:'agentic-linkage'}), (f:Flow) WHERE f.id <> 'agentic-linkage' MERGE (link)-[:TRAVERSES]->(f);
