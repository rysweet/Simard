// Simard Code Atlas — Agentic Flows graph (Layer 9)
// Portable OpenCypher. Idempotent (MERGE on id). graph_backend: portable-cypher-only.
// Run AFTER schema.cypher, atlas-layers.cypher, atlas-services.cypher, atlas-relationships.cypher.
// Source: docs/atlas/agentic-flows/ (derived from src/ code truth, NOT Python AST).

// ---- Flows -----------------------------------------------------------------
MERGE (f:Flow {id:'ooda-loop'})       SET f.name='OODA loop', f.entry='run_ooda_cycle', f.evidence='src/ooda_loop/cycle.rs';
MERGE (f:Flow {id:'overseer-tick'})   SET f.name='Overseer meta-OODA tick', f.entry='run_cycle', f.evidence='src/overseer/mod.rs:609';
MERGE (f:Flow {id:'recipes'})         SET f.name='Recipe-runner invocation', f.entry='resolve_recipe_path', f.evidence='src/ooda_brain/recipe_brain.rs';
MERGE (f:Flow {id:'prompt-assets'})   SET f.name='Prompt assets', f.entry='FilePromptAssetStore::load', f.evidence='src/prompt_assets.rs';
MERGE (f:Flow {id:'typed-ooda'})      SET f.name='Typed-OODA capability/effect', f.entry='TypedGoalSessionRoute::execute', f.evidence='src/typed_ooda/route.rs';
MERGE (f:Flow {id:'memory-recall'})   SET f.name='Cognitive-memory recall', f.entry='recall_pass', f.evidence='src/overseer/mod.rs:1026';

// ---- OODA loop phases ------------------------------------------------------
MERGE (p:Phase {id:'ooda.observe'})     SET p.seq=1, p.detail='observe(state, memories)', p.evidence='src/ooda_loop/observe.rs';
MERGE (p:Phase {id:'ooda.prepare'})     SET p.seq=2, p.detail='build_objective_probe + preparation_memory_operations', p.evidence='src/ooda_loop/cycle.rs';
MERGE (p:Phase {id:'ooda.orient'})      SET p.seq=3, p.detail='orient_with_brain -> Vec<Priority>', p.evidence='src/ooda_loop/orient.rs';
MERGE (p:Phase {id:'ooda.decide'})      SET p.seq=4, p.detail='decide_with_brain -> Vec<PlannedAction>', p.evidence='src/ooda_loop/decide.rs';
MERGE (p:Phase {id:'ooda.coverage'})    SET p.seq=5, p.detail='ensure_goal_coverage', p.evidence='src/ooda_loop/coverage.rs';
MERGE (p:Phase {id:'ooda.act'})         SET p.seq=6, p.detail='act (ooda_loop/mod.rs:109) -> dispatch_actions_bounded (ooda_actions/mod.rs)', p.evidence='src/ooda_loop/mod.rs:109';
MERGE (p:Phase {id:'ooda.consolidate'}) SET p.seq=7, p.detail='execution + review + reflection memory writes; no-progress breaker; persist_board (commit_cycle is daemon-side)', p.evidence='src/ooda_loop/cycle.rs';

// ---- Overseer tick phases --------------------------------------------------
MERGE (p:Phase {id:'ovr.observe'})   SET p.seq=1, p.detail='snapshot + board + ready_prs + merge-queue + failures', p.evidence='src/overseer/mod.rs:636';
MERGE (p:Phase {id:'ovr.recall'})    SET p.seq=2, p.detail='recall_pass(keys) -> MemorySnapshot', p.evidence='src/overseer/mod.rs:1026';
MERGE (p:Phase {id:'ovr.orient'})    SET p.seq=3, p.detail='signals_from + orient -> Vec<Problem>', p.evidence='src/overseer/mod.rs:2165';
MERGE (p:Phase {id:'ovr.rootcause'}) SET p.seq=4, p.detail='root_cause::analyze + recurrence', p.evidence='src/overseer/root_cause.rs';
MERGE (p:Phase {id:'ovr.decide'})    SET p.seq=5, p.detail='decide(problem) -> Intervention', p.evidence='src/overseer/mod.rs:2467';
MERGE (p:Phase {id:'ovr.gate'})      SET p.seq=6, p.detail='gate(iv, observed, launches) -> PlannedIntervention; then observe_ecosystem appends gated LaunchRecipe', p.evidence='src/overseer/mod.rs:1156';
MERGE (p:Phase {id:'ovr.act'})       SET p.seq=7, p.detail='capabilities: notify/issue/launch/merge; RefuseDeployer; GoalCurator unblock+MeetingHost transfer', p.evidence='src/overseer/capabilities.rs';

// ---- Recipes (prompt_assets/simard/recipes/*.yaml) -------------------------
MERGE (r:Recipe {id:'ooda-orient'})          SET r.file='prompt_assets/simard/recipes/ooda-orient.yaml';
MERGE (r:Recipe {id:'ooda-decide'})          SET r.file='prompt_assets/simard/recipes/ooda-decide.yaml';
MERGE (r:Recipe {id:'ooda-engineer-lifecycle'}) SET r.file='prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml';
MERGE (r:Recipe {id:'goal-session-actor'})   SET r.file='prompt_assets/simard/recipes/goal-session-actor.yaml';
MERGE (r:Recipe {id:'observe-merge-queue'})  SET r.file='prompt_assets/simard/recipes/observe-merge-queue.yaml';
MERGE (r:Recipe {id:'ecosystem-observe'})    SET r.file='prompt_assets/simard/recipes/ecosystem-observe.yaml';
MERGE (r:Recipe {id:'merge-readiness-judge'}) SET r.file='prompt_assets/simard/recipes/merge-readiness-judge.yaml';
MERGE (r:Recipe {id:'smart-orchestrator'})   SET r.file='amplifier-bundle (downstream, not in-repo)', r.external=true;

// ---- Prompt assets (non-recipe) --------------------------------------------
MERGE (a:PromptAsset {id:'policy.goal-session-capabilities'}) SET a.file='prompt_assets/simard/policies/goal-session-capabilities.toml';
MERGE (a:PromptAsset {id:'overseer.observe'})          SET a.file='prompt_assets/simard/overseer/observe.md';
MERGE (a:PromptAsset {id:'overseer.escalation_triage'}) SET a.file='prompt_assets/simard/overseer/escalation_triage.md';
MERGE (a:PromptAsset {id:'overseer.problem_to_brief'}) SET a.file='prompt_assets/simard/overseer/problem_to_brief.md';
MERGE (a:PromptAsset {id:'overseer.pr_verify'})        SET a.file='prompt_assets/simard/overseer/pr_verify.md';
MERGE (a:PromptAsset {id:'overseer.deploy_gate'})      SET a.file='prompt_assets/simard/overseer/deploy_gate.md';
MERGE (a:PromptAsset {id:'overseer.self_diagnose'})    SET a.file='prompt_assets/simard/overseer/self_diagnose.md';

// ---- Typed-OODA capabilities + effects -------------------------------------
MERGE (c:Capability {id:'record_action.spawn_engineer'}) SET c.grant='RecordAction(SpawnEngineer)', c.evidence='src/typed_ooda/types.rs:694';
MERGE (c:Capability {id:'record_action.file_issue'})     SET c.grant='RecordAction(FileIssue)', c.evidence='src/typed_ooda/types.rs:695';
MERGE (c:Capability {id:'record_action.request_merge'})  SET c.grant='RecordAction(RequestMerge)', c.evidence='src/typed_ooda/types.rs:696';
MERGE (c:Capability {id:'record_action.request_deploy'}) SET c.grant='RecordAction(RequestDeploy)', c.evidence='src/typed_ooda/types.rs:697';
MERGE (e:Effect {id:'SpawnEngineer'}) SET e.detail='engineer worktree + agent_supervisor', e.evidence='src/typed_ooda/types.rs:200';
MERGE (e:Effect {id:'FileIssue'})     SET e.detail='gh issue create', e.evidence='src/typed_ooda/types.rs:201';
MERGE (e:Effect {id:'RequestMerge'})  SET e.detail='gated merge', e.evidence='src/typed_ooda/types.rs:202';
MERGE (e:Effect {id:'RequestDeploy'}) SET e.detail='guarded self-deploy', e.evidence='src/typed_ooda/types.rs:203';

// ---- Typed-OODA ledger data store ------------------------------------------
MERGE (d:DataStore {id:'typed-ooda-ledger'}) SET d.detail='SQLite typed-ooda/outcomes.sqlite3 (terminal_outcomes, effect_jobs, ...)', d.evidence='src/typed_ooda/schema.rs';

// ---- Layer membership ------------------------------------------------------
MATCH (l:Layer {id:'agentic-flows'}), (f:Flow)        MERGE (l)-[:CONTAINS]->(f);
MATCH (l:Layer {id:'agentic-flows'}), (r:Recipe)      MERGE (l)-[:CONTAINS]->(r);
MATCH (l:Layer {id:'agentic-flows'}), (a:PromptAsset) MERGE (l)-[:CONTAINS]->(a);
MATCH (l:Layer {id:'agentic-flows'}), (c:Capability)  MERGE (l)-[:CONTAINS]->(c);
MATCH (l:Layer {id:'agentic-flows'}), (e:Effect)      MERGE (l)-[:CONTAINS]->(e);

// ---- Flow HAS_PHASE + phase ordering ---------------------------------------
MATCH (f:Flow {id:'ooda-loop'}), (p:Phase) WHERE p.id STARTS WITH 'ooda.' MERGE (f)-[:HAS_PHASE]->(p);
MATCH (f:Flow {id:'overseer-tick'}), (p:Phase) WHERE p.id STARTS WITH 'ovr.' MERGE (f)-[:HAS_PHASE]->(p);
MATCH (a:Phase), (b:Phase)
  WHERE a.id STARTS WITH 'ooda.' AND b.id STARTS WITH 'ooda.' AND b.seq = a.seq + 1
  MERGE (a)-[:NEXT]->(b);
MATCH (a:Phase), (b:Phase)
  WHERE a.id STARTS WITH 'ovr.' AND b.id STARTS WITH 'ovr.' AND b.seq = a.seq + 1
  MERGE (a)-[:NEXT]->(b);

// ---- Phase INVOKES Recipe --------------------------------------------------
MATCH (p:Phase {id:'ooda.orient'}), (r:Recipe {id:'ooda-orient'}) MERGE (p)-[:INVOKES]->(r);
MATCH (p:Phase {id:'ooda.decide'}), (r:Recipe {id:'ooda-decide'}) MERGE (p)-[:INVOKES]->(r);
MATCH (p:Phase {id:'ovr.observe'}), (r:Recipe {id:'observe-merge-queue'}) MERGE (p)-[:INVOKES]->(r);
MATCH (p:Phase {id:'ovr.gate'}), (r:Recipe {id:'ecosystem-observe'})    MERGE (p)-[:INVOKES]->(r);
MATCH (p:Phase {id:'ovr.gate'}), (r:Recipe {id:'smart-orchestrator'})   MERGE (p)-[:INVOKES]->(r);

// ---- Production capability grant flags (goal-session-policy-v1) -------------
MATCH (c:Capability {id:'record_action.spawn_engineer'}) SET c.granted_in_production = true;
MATCH (c:Capability) WHERE c.id IN ['record_action.file_issue','record_action.request_merge','record_action.request_deploy']
  SET c.granted_in_production = false;
MATCH (e:Effect {id:'SpawnEngineer'}) SET e.granted_in_production = true;
MATCH (e:Effect) WHERE e.id IN ['FileIssue','RequestMerge','RequestDeploy'] SET e.granted_in_production = false, e.note = 'enum variant; not granted by goal-session-policy-v1';

// ---- Flow DRIVES Flow (cross-flow orchestration) ---------------------------
MATCH (f:Flow {id:'ooda-loop'}), (g:Flow {id:'memory-recall'}) MERGE (f)-[:DRIVES]->(g);
MATCH (f:Flow {id:'ooda-loop'}), (g:Flow {id:'recipes'})       MERGE (f)-[:DRIVES]->(g);
MATCH (f:Flow {id:'ooda-loop'}), (g:Flow {id:'typed-ooda'})    MERGE (f)-[:DRIVES]->(g);
MATCH (f:Flow {id:'overseer-tick'}), (g:Flow {id:'memory-recall'}) MERGE (f)-[:DRIVES]->(g);
MATCH (f:Flow {id:'overseer-tick'}), (g:Flow {id:'recipes'})   MERGE (f)-[:DRIVES]->(g);
MATCH (f:Flow {id:'typed-ooda'}), (g:Flow {id:'recipes'})      MERGE (f)-[:DRIVES]->(g);
MATCH (f:Flow {id:'recipes'}), (g:Flow {id:'prompt-assets'})   MERGE (f)-[:DRIVES]->(g);
MATCH (f:Flow {id:'typed-ooda'}), (g:Flow {id:'prompt-assets'}) MERGE (f)-[:DRIVES]->(g);

// ---- Flow READS_FROM / WRITES_TO DataStore ---------------------------------
MATCH (f:Flow {id:'memory-recall'}), (d:DataStore {id:'lbug-cognitive'}) MERGE (f)-[:READS_FROM]->(d);
MATCH (f:Flow {id:'memory-recall'}), (d:DataStore {id:'lbug-cognitive'}) MERGE (f)-[:WRITES_TO]->(d);
MATCH (f:Flow {id:'ooda-loop'}), (d:DataStore {id:'lbug-cognitive'})     MERGE (f)-[:WRITES_TO]->(d);
MATCH (f:Flow {id:'overseer-tick'}), (d:DataStore {id:'lbug-cognitive'}) MERGE (f)-[:WRITES_TO]->(d);
MATCH (f:Flow {id:'typed-ooda'}), (d:DataStore {id:'typed-ooda-ledger'}) MERGE (f)-[:WRITES_TO]->(d);

// ---- Flow USES PromptAsset -------------------------------------------------
MATCH (f:Flow {id:'typed-ooda'}), (a:PromptAsset {id:'policy.goal-session-capabilities'}) MERGE (f)-[:USES]->(a);
MATCH (f:Flow {id:'overseer-tick'}), (a:PromptAsset) WHERE a.id STARTS WITH 'overseer.' MERGE (f)-[:USES]->(a);

// ---- Capability AUTHORIZES Effect ------------------------------------------
MATCH (c:Capability {id:'record_action.spawn_engineer'}), (e:Effect {id:'SpawnEngineer'}) MERGE (c)-[:AUTHORIZES]->(e);
MATCH (c:Capability {id:'record_action.file_issue'}), (e:Effect {id:'FileIssue'})         MERGE (c)-[:AUTHORIZES]->(e);
MATCH (c:Capability {id:'record_action.request_merge'}), (e:Effect {id:'RequestMerge'})   MERGE (c)-[:AUTHORIZES]->(e);
MATCH (c:Capability {id:'record_action.request_deploy'}), (e:Effect {id:'RequestDeploy'}) MERGE (c)-[:AUTHORIZES]->(e);

// ---- Flow PRODUCES Effect (only the production-granted effect) --------------
MATCH (f:Flow {id:'typed-ooda'}), (e:Effect {id:'SpawnEngineer'}) MERGE (f)-[:PRODUCES]->(e);

// ---- Flow TOUCHES cross-layer nodes (Runtime Topology / Service Components) -
MATCH (f:Flow {id:'ooda-loop'}), (c:Component {id:'ooda-loop'})           MERGE (f)-[:TOUCHES]->(c);
MATCH (f:Flow {id:'overseer-tick'}), (c:Component {id:'overseer'})        MERGE (f)-[:TOUCHES]->(c);
MATCH (f:Flow {id:'recipes'}), (p:Process {id:'recipe-runner-rs'})        MERGE (f)-[:TOUCHES]->(p);
MATCH (f:Flow {id:'overseer-tick'}), (p:Process {id:'amplihack'})         MERGE (f)-[:TOUCHES]->(p);
MATCH (f:Flow {id:'typed-ooda'}), (p:Process {id:'gh'})                   MERGE (f)-[:TOUCHES]->(p);
// NOTE: the in-daemon memory-recall flow reads lbug in-process (memory_ipc::SharedMemory
// wrapper), NOT over the UDS server; the memory-ipc Component serves EXTERNAL clients.

// ---- Flow <-> Journey linkage (Layer 8 <-> Layer 9) ------------------------
MATCH (f:Flow {id:'ooda-loop'}), (j:Journey {id:'ooda-cycle'})           MERGE (f)-[:TOUCHES]->(j);
MATCH (f:Flow {id:'memory-recall'}), (j:Journey {id:'memory-recall'})    MERGE (f)-[:TOUCHES]->(j);
MATCH (f:Flow {id:'overseer-tick'}), (j:Journey {id:'overseer-blocked-goal'}) MERGE (f)-[:TOUCHES]->(j);
MATCH (f:Flow {id:'typed-ooda'}), (j:Journey {id:'engineer-pr'})         MERGE (f)-[:TOUCHES]->(j);
