// Simard Code Atlas — Layer nodes (the 9 slugs)
// Portable OpenCypher. Idempotent (MERGE on id). graph_backend: portable-cypher-only.

MERGE (l:Layer {id: 'repo-surface'})
  SET l.name = 'Repository Surface',
      l.description = 'Source tree, build entry points, bins, crate layout',
      l.diagram = 'docs/atlas/repo-surface/';
MERGE (l:Layer {id: 'ast-lsp-bindings'})
  SET l.name = 'AST+LSP Symbol Bindings',
      l.description = 'Exported symbols, cross-file references, dead code',
      l.mode = 'static-approximation',
      l.diagram = 'docs/atlas/ast-lsp-bindings/';
MERGE (l:Layer {id: 'compile-deps'})
  SET l.name = 'Compile-time Dependencies',
      l.description = 'Cargo crate deps (runtime, dev/build) and internal module graph',
      l.diagram = 'docs/atlas/compile-deps/';
MERGE (l:Layer {id: 'runtime-topology'})
  SET l.name = 'Runtime Topology',
      l.description = 'Single native Rust daemon, in-process subsystems, spawned processes, ports, stores',
      l.diagram = 'docs/atlas/runtime-topology/';
MERGE (l:Layer {id: 'api-contracts'})
  SET l.name = 'API Contracts',
      l.description = 'Dashboard HTTP/WebSocket routes, auth guard, Signal command surface',
      l.diagram = 'docs/atlas/api-contracts/';
MERGE (l:Layer {id: 'data-flow'})
  SET l.name = 'Data Flow',
      l.description = 'Memory, goals, engineer, signal, self-deploy read/write chains',
      l.diagram = 'docs/atlas/data-flow/';
MERGE (l:Layer {id: 'service-components'})
  SET l.name = 'Service Component Architecture',
      l.description = 'Daemon internal module clusters: runtime, operator, self-improvement',
      l.diagram = 'docs/atlas/service-components/';
MERGE (l:Layer {id: 'user-journeys'})
  SET l.name = 'User Journey Scenarios',
      l.description = 'OODA cycle, memory recall, signal reply, blocked-goal, engineer-PR, self-deploy',
      l.diagram = 'docs/atlas/user-journeys/';
MERGE (l:Layer {id: 'agentic-flows'})
  SET l.name = 'Agentic Flows',
      l.description = 'OODA loop, overseer tick, recipes, prompt assets, typed-OODA capability/effect, cognitive-memory recall',
      l.diagram = 'docs/atlas/agentic-flows/';
