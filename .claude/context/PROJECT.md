# Project Context

## Project: Simard

## Overview

Simard is a terminal-native engineering runtime written in Rust.
Its job is to run explicit engineering sessions with clear lifecycle phases, layered memory, evidence capture, and reflection rather than acting like an opaque chat assistant.

The current repository intentionally starts with a small scaffold, but that scaffold is already shaped for dependency injection, future distributed deployment, and multiple base types.
Local single-process execution is the first runnable delivery mode, not the architectural boundary.

## Architecture

### Key Components

- **`prompt_assets`**: Prompt asset identifiers plus file-backed and in-memory stores.
- **`base_types`**: Base-type identifiers, capability contracts, topology support, and concrete adapter implementations.
- **`identity`**: Identity manifests, supported modes, base-type eligibility, and memory policy.
- **`runtime`**: Control-plane composition, startup validation, topology, lifecycle, and the local single-process runtime path.
- **`session`**: Ordered session phases and transition validation.
- **`memory`**: Layered memory scopes for scratch, summary, project, and benchmark storage.
- **`evidence`**: Evidence records and evidence sinks with explicit provenance.
- **`reflection`**: Typed runtime snapshots and reflection reports.

### Technology Stack

- **Language**: Rust
- **Runtime style**: single binary bootstrap with extractable internal seams
- **Persistence for v1 scaffold**: in-memory stores plus file-backed prompt assets

## Development Guidelines

### Code Organization

- `src/lib.rs` re-exports the public scaffold API.
- `src/main.rs` is thin composition only.
- `prompt_assets/` contains prompt files that stay separate from runtime code.
- `Specs/ProductArchitecture.md` is the canonical architecture reference.

### Key Patterns

- Prefer explicit contracts and trait-based composition over hidden globals.
- Treat distributed readiness and multi-base-type support as architectural constraints from day one.
- Keep failures visible with typed errors; do not add silent fallbacks.
- Keep the local runtime honest: it is a real single-process composition path, not permission to bake locality into core contracts.
- Favor ruthless simplicity over speculative infrastructure.

### Testing Strategy

- Unit and integration tests should cover contract validation, lifecycle transitions, adapter selection, and memory/evidence boundaries.
- Prefer deterministic tests using local in-memory stores and deterministic adapters.
- Verify behavior through explicit results and state assertions, not log inspection.

## Domain Knowledge

### Business Context

Simard is intended to become a disciplined engineer-in-the-terminal.
It must be able to inspect repositories, plan bounded work, execute with evidence, persist useful memory, and support benchmark-driven improvement.

### Key Terminology

- **Prompt Asset**: file-backed prompt content kept separate from code.
- **Base Type**: execution substrate behind an identity.
- **Identity Manifest**: durable definition of prompts, allowed base types, required capabilities, and memory policy.
- **Runtime Control Plane**: component that composes dependencies, validates capabilities, and manages lifecycle.
- **Session**: one bounded run through intake, preparation, planning, execution, reflection, and persistence.

## Common Tasks

### Development Workflow

- Update the architecture spec when changing core contracts or terminology.
- Add tests before widening the runtime surface.
- Keep `main.rs` thin and move behavior into the library modules.
- Run `cargo fmt` and `cargo test` before considering the scaffold complete.

### Deployment Process

For now, deployment means local single-process execution from the Rust binary.
Future multi-process or distributed deployments must reuse the same core contracts rather than introducing a separate business-logic path.

## Important Notes

- Do not treat local execution as a permanent architecture limit.
- Do not assume a single base type in manifests or runtime selection.
- Do not hide unsupported capabilities behind default fallbacks.
