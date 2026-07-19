# Simard Code Atlas

A living, regeneratable architecture map of **Simard** — a single-crate native Rust
OODA daemon (graph store = embedded **lbug**; **NO kuzu, NO Python**).

> Diagramming is investigation, not just documentation. This atlas exists to make the
> whole system traceable from code truth and to surface structural contradictions.

## Graph backend

`graph_backend: portable-cypher-only`

The upstream code-atlas skill mandates Kuzu ingestion + a Python AST analyzer. Simard's
hard policy forbids both. This atlas therefore ships the **engine-neutral** deliverable —
portable OpenCypher under [`cypher/`](./cypher/) — and **skips Kuzu ingestion and the
Python code-visualizer** deliberately and visibly (never silently). The cypher loads into
any OpenCypher engine, including Simard's own embedded lbug store, Neo4j, or Memgraph.
Rust structure was derived from `cargo metadata` + ripgrep/rust-analyzer, not Python AST.

## Layers

| # | Layer | Slug | Diagrams |
| - | ----- | ---- | -------- |
| 1 | Repository Surface | [repo-surface](./repo-surface/README.md) | Mermaid + DOT |
| 2 | AST+LSP Symbol Bindings | [ast-lsp-bindings](./ast-lsp-bindings/README.md) | Mermaid + DOT (mode: `static-approximation`) |
| 3 | Compile-time Dependencies | [compile-deps](./compile-deps/README.md) | Mermaid + DOT (split) |
| 4 | Runtime Topology | [runtime-topology](./runtime-topology/README.md) | Mermaid + DOT |
| 5 | API Contracts | [api-contracts](./api-contracts/README.md) | Mermaid + DOT |
| 6 | Data Flow | [data-flow](./data-flow/README.md) | Mermaid + DOT (split ×5) |
| 7 | Service Component Architecture | [service-components](./service-components/README.md) | Mermaid + DOT (split ×4) |
| 8 | User Journey Scenarios | [user-journeys](./user-journeys/README.md) | Mermaid + DOT (×6 journeys) |
| 9 | Agentic Flows | [agentic-flows](./agentic-flows/README.md) | Mermaid + DOT (×7 flows) |

Every layer directory contains `.mmd` + `.dot` source and rendered `*-mermaid.svg` +
`*-dot.svg`. Both formats are kept on purpose: they find different bugs (~15% overlap).

Layer 9 (**Agentic Flows**) is the emphasis of this revision: it maps the autonomous
decision machinery — the OODA loop, the overseer tick, the typed-OODA capability/effect
model, recipes (amplihack `recipe-runner`: `smart-orchestrator` / `default-workflow` /
`investigation-workflow`), prompt assets (`prompt_assets/simard/*`), and the
cognitive-memory recall path — and how they link together across layers.

## Portable graph (cypher/)

| File | Purpose |
| ---- | ------- |
| [`cypher/schema.cypher`](./cypher/schema.cypher) | Node labels, relationship types, optional uniqueness constraints |
| [`cypher/atlas-layers.cypher`](./cypher/atlas-layers.cypher) | The 9 `:Layer` nodes |
| [`cypher/atlas-services.cypher`](./cypher/atlas-services.cypher) | Services, components, processes, ports, stores, routes, journeys |
| [`cypher/atlas-relationships.cypher`](./cypher/atlas-relationships.cypher) | Cross-layer links (the edges between layers) |
| [`cypher/atlas-agentic.cypher`](./cypher/atlas-agentic.cypher) | Agentic flows: flows, phases, recipes, prompt assets, capabilities + cross-flow seams |
| [`cypher/queries.cypher`](./cypher/queries.cypher) | Ready-to-run example queries (endpoints, blast radius, orphans, journey traces, agentic-flow traces) |

Load order: `schema` → `atlas-layers` → `atlas-services` → `atlas-relationships` →
`atlas-agentic`, then any query.

## Bug hunt

Structural findings are **never stored in this atlas**. They are filed as GitHub issues
labeled [`code-atlas-bughunt`](https://github.com/rysweet/Simard/issues?q=label%3Acode-atlas-bughunt).

## Regenerating

See [`staleness-map.yaml`](./staleness-map.yaml) for per-layer rebuild triggers. Render
commands:

```bash
# Mermaid (Chrome sandbox is disabled on this host via a puppeteer config)
mmdc -p <puppeteer-noSandbox.json> -i LAYER.mmd -o LAYER-mermaid.svg
# Graphviz
dot -Tsvg LAYER.dot -o LAYER-dot.svg
```
