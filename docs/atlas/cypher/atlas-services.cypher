// Simard Code Atlas — Service / Component / Process / Port / DataStore / Route / Journey nodes
// Portable OpenCypher. Idempotent (MERGE on id). graph_backend: portable-cypher-only.
// Source: runtime-topology, service-components, api-contracts, user-journeys layers.

// ---- Services (deployable / runnable units) --------------------------------
MERGE (s:Service {id: 'simard-ooda.service'})
  SET s.kind = 'systemd-unit', s.detail = 'user unit; ExecStart={binary} ooda run; Restart=always',
      s.evidence = 'src/install/systemd.rs:22';
MERGE (s:Service {id: 'simard-daemon'})
  SET s.kind = 'process', s.detail = 'simard ooda run [--cycles=N] [--dashboard-port=PORT] [state-root]',
      s.evidence = 'src/operator_cli/ooda.rs:31';
MERGE (s:Service {id: 'standalone-dashboard'})
  SET s.kind = 'process-mode', s.detail = 'simard dashboard serve --port=PORT (default 8080)',
      s.evidence = 'src/operator_cli/dashboard.rs:15';

// ---- Components (in-process subsystems) ------------------------------------
MERGE (c:Component {id: 'ooda-loop'})       SET c.detail = 'OODA loop / scheduler', c.evidence = 'src/operator_commands_ooda/daemon/mod.rs:173';
MERGE (c:Component {id: 'dashboard'})        SET c.detail = 'Embedded Axum dashboard (HTTP+WS)', c.evidence = 'src/operator_commands_dashboard/mod.rs:253';
MERGE (c:Component {id: 'signal-embed'})     SET c.detail = 'Embedded Signal channel (Tokio reconnect loop)', c.evidence = 'src/operator_commands_ooda/daemon/signal_embed.rs:38';
MERGE (c:Component {id: 'memory-ipc'})       SET c.detail = 'Memory IPC server (UDS, length-prefixed JSON)', c.evidence = 'src/memory_ipc/server.rs:53';
MERGE (c:Component {id: 'cognitive-threads'})SET c.detail = 'Cognitive-thread scheduler + threads', c.evidence = 'src/operator_commands_ooda/daemon/mod.rs:828';
MERGE (c:Component {id: 'overseer'})         SET c.detail = 'Overseer supervision + escalation-triage', c.evidence = 'src/operator_commands_ooda/daemon/mod.rs:1676';

// ---- External spawned processes --------------------------------------------
MERGE (p:Process {id: 'recipe-runner-rs'}) SET p.evidence = 'src/ooda_brain/recipe_brain.rs:888';
MERGE (p:Process {id: 'amplihack'})        SET p.detail = 'amplihack recipe run smart-orchestrator', p.evidence = 'src/overseer/launch.rs:225';
MERGE (p:Process {id: 'llm-provider-agent'}) SET p.evidence = 'src/typed_ooda/route.rs:246';
MERGE (p:Process {id: 'gh'})               SET p.evidence = 'src/stewardship/gh_client.rs:269';
MERGE (p:Process {id: 'git'})              SET p.evidence = 'src/ooda_loop/observe.rs:19';
MERGE (p:Process {id: 'systemctl'})        SET p.evidence = 'src/self_deploy/restart.rs:167';
MERGE (p:Process {id: 'tmux'})             SET p.evidence = 'src/subagent_sessions/mod.rs:74';
MERGE (p:Process {id: 'journalctl'})       SET p.evidence = 'src/operator_commands_dashboard/logs.rs:177';
MERGE (p:Process {id: 'curl'})             SET p.evidence = 'src/cmd_self_update/download.rs:178';
MERGE (p:Process {id: 'cargo'})            SET p.evidence = 'src/self_relaunch/canary.rs:30';

// ---- Ports / network endpoints ---------------------------------------------
MERGE (n:Port {id: 'dashboard-8080'})   SET n.endpoint = '0.0.0.0:8080', n.protocol = 'HTTP+WebSocket (Axum)', n.evidence = 'src/operator_commands_dashboard/mod.rs:253';
MERGE (n:Port {id: 'signal-rpc-7583'})  SET n.endpoint = '127.0.0.1:7583', n.protocol = 'newline-delimited JSON-RPC 2.0 over TCP', n.evidence = 'src/signal_conversation/config.rs:17';

// ---- Data stores -----------------------------------------------------------
MERGE (d:DataStore {id: 'lbug-cognitive'}) SET d.detail = 'embedded lbug graph store at <state_root>/cognitive', d.evidence = 'src/memory_ipc/launcher.rs:252';
MERGE (d:DataStore {id: 'memory-sock'})    SET d.detail = '<state_root>/memory.sock or SIMARD_MEMORY_SOCKET', d.evidence = 'src/memory_ipc/mod.rs:90';
MERGE (d:DataStore {id: 'dashkey-file'})   SET d.detail = '~/.simard/.dashkey (session/bearer guard)', d.evidence = 'src/operator_commands_dashboard/auth.rs:13';

// ---- API routes (api-contracts) --------------------------------------------
MERGE (r:Route {id: 'GET /login'})       SET r.auth = 'bypass', r.evidence = 'src/operator_commands_dashboard/auth.rs:68';
MERGE (r:Route {id: 'POST /api/login'})  SET r.auth = 'bypass', r.evidence = 'src/operator_commands_dashboard/auth.rs:68';
MERGE (r:Route {id: 'dashboard-api-*'})  SET r.auth = 'require_auth (401 if unauthenticated)', r.evidence = 'src/operator_commands_dashboard/auth.rs:68';

// ---- Guard symbols ---------------------------------------------------------
MERGE (y:Symbol {id: 'require_auth'})  SET y.evidence = 'src/operator_commands_dashboard/auth.rs:68';
MERGE (y:Symbol {id: 'signal-gating'}) SET y.detail = 'deploy / merge #NNNN require approve sign-off', y.evidence = 'src/signal_conversation/gating.rs:95';

// ---- Env vars that gate behaviour ------------------------------------------
MERGE (e:EnvVar {id: 'SIMARD_SIGNAL_ENABLED'})  SET e.evidence = 'src/operator_commands_ooda/daemon/signal_embed.rs:38';
MERGE (e:EnvVar {id: 'SIMARD_SIGNAL_RPC_ADDR'}) SET e.evidence = 'src/overseer/notify.rs:987';
MERGE (e:EnvVar {id: 'SIMARD_MEMORY_SOCKET'})   SET e.evidence = 'src/memory_ipc/mod.rs:90';

// ---- User journeys ---------------------------------------------------------
MERGE (j:Journey {id: 'ooda-cycle'})          SET j.diagram = 'docs/atlas/user-journeys/journey-ooda-cycle-mermaid.mmd';
MERGE (j:Journey {id: 'memory-recall'})       SET j.diagram = 'docs/atlas/user-journeys/journey-memory-recall-mermaid.mmd';
MERGE (j:Journey {id: 'signal-reply'})        SET j.diagram = 'docs/atlas/user-journeys/journey-signal-reply-mermaid.mmd';
MERGE (j:Journey {id: 'overseer-blocked-goal'}) SET j.diagram = 'docs/atlas/user-journeys/journey-overseer-blocked-goal-mermaid.mmd';
MERGE (j:Journey {id: 'engineer-pr'})         SET j.diagram = 'docs/atlas/user-journeys/journey-engineer-pr-mermaid.mmd';
MERGE (j:Journey {id: 'self-deploy'})         SET j.diagram = 'docs/atlas/user-journeys/journey-self-deploy-mermaid.mmd';
