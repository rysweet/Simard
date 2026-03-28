# Project Context

## Project: Simard

## Overview

Simard is a terminal-native engineering runtime written in Rust.
It runs explicit engineering sessions with a visible lifecycle, layered memory, evidence capture, and truthful reflection instead of behaving like an opaque chat shell.

The current repository is intentionally narrow, but its contracts are already shaped for dependency injection, future distributed deployment, and multiple base types from day one.
Local single-process execution is the first runnable path, not the architectural boundary.

## Architecture

### Key Components

- **`prompt_assets`**: prompt identifiers plus file-backed and in-memory stores.
- **`base_types`**: base-type identifiers, capability contracts, topology support, and adapter implementations.
- **`identity`**: identity manifests, supported modes, allowed base types, and memory policy.
- **`runtime`**: control-plane composition, startup validation, topology, lifecycle, and the local runtime path.
- **`session`**: ordered session phases and transition validation.
- **`memory`**: layered memory scopes for scratch, summary, project, and benchmark storage.
- **`evidence`**: evidence records and evidence sinks with explicit provenance.
- **`reflection`**: typed runtime snapshots and reflection reports.

### Technology Stack

- **Language**: Rust
- **Runtime style**: single binary bootstrap with extractable internal seams
- **Persistence for the current scaffold**: in-memory stores plus file-backed prompt assets

## Development Guidelines

### Code Organization

- `src/lib.rs` re-exports the public scaffold API.
- `src/main.rs` stays thin and delegates to bootstrap helpers.
- `prompt_assets/` keeps prompt content separate from runtime code.
- `Specs/ProductArchitecture.md` is the canonical architecture reference.
- `tests/` focuses on contracts, lifecycle, prompt assets, and bootstrap behavior.

### Key Patterns

- Prefer explicit contracts and trait-based composition over hidden globals.
- Treat distributed readiness and multi-base-type support as architectural constraints from day one.
- Keep failures visible with typed errors; do not add silent fallbacks or hidden degradation.
- Keep the local runtime honest: it is a real composition path, not permission to bake locality into core contracts.
- Prefer explicit defaults at startup over recovery-shaped behavior after failure.

### Testing Strategy

- Use unit and integration tests to cover contract validation, lifecycle transitions, adapter selection, and memory/evidence boundaries.
- Prefer deterministic tests with local in-memory stores and controlled adapters.
- Validate behavior through explicit results and state assertions rather than log inspection.

## Domain Knowledge

### Business Context

Simard is intended to become a disciplined engineer-in-the-terminal.
It needs to inspect repositories, plan bounded work, execute with evidence, persist useful memory, and support benchmark-driven improvement without hiding its runtime shape.

### Key Terminology

- **Prompt Asset**: file-backed prompt content kept separate from code.
- **Base Type**: the execution substrate behind an identity.
- **Identity Manifest**: durable definition of prompts, allowed base types, required capabilities, and memory policy.
- **Runtime Control Plane**: component that composes dependencies, validates capabilities, and manages lifecycle.
- **Session**: one bounded run through intake, preparation, planning, execution, reflection, and persistence.

## Common Tasks

### Development Workflow

- Update `Specs/ProductArchitecture.md` when changing core contracts or terminology.
- Add or extend tests before widening runtime surfaces.
- Keep `main.rs` thin and move behavior into library modules.
- Run `cargo test`.
- When working on repository-wide quality gates, use the configured pre-commit hooks and CI verification.

### Deployment Process

For now, deployment means local single-process execution from the Rust binary.
Future multi-process or distributed deployments must reuse the same contracts rather than introducing a separate business-logic path.

## Important Notes

- Do not treat local execution as a permanent architecture limit.
- Do not assume a single base type in manifests or runtime selection.
- Do not hide unsupported capabilities behind default fallbacks.
- Explicit bootstrap defaults are allowed only when the startup mode opts in.
