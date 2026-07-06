---
title: amplihack pin bump to upstream main (#2626)
description: "Reference record for the issue #2626 dependency-pin reconcile that bumped Simard's amplihack-agent-eval and amplihack-memory git-rev pins from behind-main revisions to the current upstream main HEADs, with the lbug lockstep, API-parity, and supply-chain re-verification that gated the bump."
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: active
related:
  - ../howto/self-maintain-dependency-pins.md
  - ../architecture/gym-eval-library-adapter.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ./dependency-trust-policy.md
  - ./supply-chain-audit.md
---

# amplihack pin bump to upstream main (#2626)

> **Status: active.** This page is the completed change record for issue
> [#2626](https://github.com/rysweet/Simard/issues/2626): the reconcile that
> re-pointed Simard's two `amplihack-*` git-rev pins from stale, behind-`main`
> revisions to the current upstream `main` HEADs, so the fixes those upstream
> repos already merged actually run in Simard's own build. It is a concrete,
> worked instance of the **proactive reconcile (Trigger B)** described in
> [How to keep Simard's dependency pins up to date](../howto/self-maintain-dependency-pins.md),
> and it doubles as the specification the bump PR is verified against.

Two of the four git-rev pins in the root `Cargo.toml` had drifted behind their
upstream default branch:

- `amplihack-agent-eval` (`rysweet/amplihack-rs`) — the sole gym/eval engine
  behind [`gym_runner_client`](../architecture/gym-eval-library-adapter.md).
- `amplihack-memory` (`rysweet/amplihack-memory-lib`) — the sole cognitive-memory
  backend behind
  [`LibraryCognitiveMemory`](../architecture/cognitive-memory-library-adapter.md).

A git-rev pin is reproducible but **frozen**: until the pin moves, merged
upstream work is absent from the daemon that depends on it. Per the operator
policy — *when Simard updates a tool she maintains she must bump her **own**
dependency and run the new code* — this reconcile moves both pins to the exact
`main` HEADs and re-verifies the whole graph.

---

## What changed

Both pins were re-pointed to the exact 40-character `main` HEAD of their
upstream repository. No other dependency, feature, or profile was touched.

| Crate | Upstream repo | Old rev (behind `main`) | New rev (`main` HEAD) |
| --- | --- | --- | --- |
| `amplihack-agent-eval` | `rysweet/amplihack-rs` | `59548a96049ab8d558110bcaf9c82a4316f1bbf0` | `2a93441d1837f9f853d5dddc56cc1088353a8872` |
| `amplihack-memory` | `rysweet/amplihack-memory-lib` | `5d7db77dd5c3bafb2846c2f50761112588a47563` | `f80037089a735bd0d394e3eec5cea9fcae1895ea` |

The `amplihack-memory` pin keeps `features = ["persistent"]`. The two
`RustyClawd` pins (`rustyclawd-core`, `rustyclawd-tools`) and the direct
`lbug = "=0.17.1"` pin are **unchanged** by this bump.

`Cargo.toml` after the bump:

```toml
amplihack-memory = { git = "https://github.com/rysweet/amplihack-memory-lib.git", rev = "f80037089a735bd0d394e3eec5cea9fcae1895ea", features = ["persistent"] }
amplihack-agent-eval = { git = "https://github.com/rysweet/amplihack-rs.git", rev = "2a93441d1837f9f853d5dddc56cc1088353a8872" }
lbug = "=0.17.1"
```

---

## Provenance: both revs are upstream `main` HEADs

Each new rev was taken from the live upstream default branch at bump time, not
from a feature branch (a feature-branch ref can be force-pushed or GC'd, which
would freeze the build against an unmergeable commit):

```bash
git ls-remote https://github.com/rysweet/amplihack-rs.git main
# 2a93441d1837f9f853d5dddc56cc1088353a8872	refs/heads/main

git ls-remote https://github.com/rysweet/amplihack-memory-lib.git main
# f80037089a735bd0d394e3eec5cea9fcae1895ea	refs/heads/main
```

After the bump, `Cargo.lock` records the identical revs for both crates. Confirm
the lock and the manifest agree with the two HEADs above:

```bash
# manifest revs
grep -oE '[0-9a-f]{40}' Cargo.toml | sort -u

# locked git revs for the two bumped crates
grep -A3 'name = "amplihack-agent-eval"' Cargo.lock   # source ... #2a93441...
grep -A3 'name = "amplihack-memory"'     Cargo.lock   # source ... #f800370...
```

The drift that motivated the bump is verifiable with the GitHub compare API
against the *old* rev (both report `behind_by > 0` before the bump, `0` after):

```bash
gh api repos/rysweet/amplihack-rs/compare/59548a96049ab8d558110bcaf9c82a4316f1bbf0...main --jq '.behind_by'
gh api repos/rysweet/amplihack-memory-lib/compare/5d7db77dd5c3bafb2846c2f50761112588a47563...main --jq '.behind_by'
```

---

## Lock refresh procedure

The bump refreshes only the two crates' git revs; the rest of the graph stays
stable:

```bash
# 1. Edit both rev = "…" values in Cargo.toml to the new HEADs (above).
# 2. Refresh only the two bumped crates in Cargo.lock:
cargo update -p amplihack-agent-eval -p amplihack-memory
# 3. Build against the new revs:
cargo build --release
```

`cargo update` is scoped with `-p` so it does not churn unrelated locked
versions. Crate `version` fields in `Cargo.lock` are **not** hand-edited: if a
new rev changes a crate version, the lock updates automatically from the rev
bump.

---

## API parity: zero consumer edits

Both upstream deltas are **API-compatible** with Simard's existing call sites,
so the bump changed *only* `Cargo.toml` and `Cargo.lock` — no `.rs` file was
edited. This was verified by a clean compile, not assumed.

| Consumer | Upstream type surface it depends on | Result of the bump |
| --- | --- | --- |
| `src/gym_runner_client.rs` | imports `amplihack_agent_eval::gym::{GymConfig, GymRunner, GymScenarioResult}` and drives the three `gym.*` handlers via `GymRunner::new(gym_config())`; scenario/suite payloads cross the wire as the client's own `crate::gym_client::{GymScenarioResult, GymSuiteResult}` mirrors, not upstream types | compiles unchanged |
| `src/cognitive_memory/library_adapter.rs` | the **sole** compile-time consumer of the crate's Rust surface — one `use amplihack_memory::{AccessKind, CognitiveMemory, DedupMode, DedupOptions, EpisodicMemory, FactInput, MemoryError, ProceduralMemory, ProspectiveMemory, RecallOptions, RecallWeights, RetentionPolicy, SemanticFact, StoreFactOptions, WorkingMemorySlot}`; this is where the `RecallWeightSet → RecallWeights` conversion is adapter-local | compiles unchanged |
| `src/cognitive_memory/mod.rs` (`RecallWeightSet`), `src/memory_cognitive.rs`, `src/ooda_loop/phase_weights.rs` | backend-agnostic **mirror** types: by the issue #2329 design the `CognitiveMemoryOps` trait and these mirrors deliberately never name a library type (`memory_cognitive.rs` mirrors the six-type model over `serde` only; `RecallWeightSet` mirrors `RecallWeights`; `phase_weights` maps `OodaPhase → RecallWeightSet`) | compile unchanged — insulated from the crate surface by construction, not merely by an API match |

The two stable seams — the
[gym client wire protocol](../architecture/gym-eval-library-adapter.md#wire-protocol)
and the
[`CognitiveMemoryOps` trait](../architecture/cognitive-memory-library-adapter.md) —
absorb the upstream revisions with no call-site drift.

> **If a future rev *does* break a call site**, fix the call site forward to the
> new API. Never paper over a signature change with a compatibility shim or a
> fallback branch — a silent fallback is a silent failure, which this repo
> treats as a defect (see
> [Eliminate deterministic fallbacks](../design/eliminate-deterministic-fallbacks.md)).

---

## lbug lockstep: exactly one engine

The binding constraint on the `amplihack-memory` bump is that the final binary
must link **exactly one** LadybugDB (`lbug`) version and therefore one on-disk
store format. Simard depends on `lbug` two ways that must agree:

- **Transitively**, through `amplihack-memory`'s `persistent` feature (the
  cognitive-memory backend).
- **Directly**, via the `lbug = "=0.17.1"` pin used *only* by the read-only
  `simard-tui` goal board (`src/bin/simard_tui/goals.rs`).

The `amplihack-memory` HEAD `f800370` carries **no engine change** — it stays on
`lbug 0.17.1` (store format **v41**) — so the direct `=0.17.1` pin is unchanged
and the two references resolve to a single version. This is asserted, not
assumed:

```bash
cargo tree -i lbug
# lbug v0.17.1   ← exactly one node; no second version
```

If a future `amplihack-memory` bump *did* move `lbug`, the reconcile would bump
the direct `lbug = "=…"` pin to match the transitive version, update every
lockstep comment in `Cargo.toml`, and re-run `cargo tree -i lbug` to confirm a
single node. **Two `lbug` versions is a hard failure** (two storage engines
linked, two store formats) and blocks the bump.

---

## Supply-chain re-verification

The bump adds no new git source and no new crate to the graph, so the standing
guardrails stay green. This is confirmed locally before the PR, mirroring the CI
jobs described in
[Supply-Chain Audit & Guardrails](./supply-chain-audit.md) and
[Dependency Trust Policy](./dependency-trust-policy.md):

```bash
cargo deny --locked check     # advisories + licenses + bans + sources
cargo audit                   # RUSTSEC scan
cargo vet --locked            # transitive trust certification
```

- **`[sources]` allowlist unchanged.** Both new revs are on the **same**
  already-allowlisted remotes (`amplihack-rs.git`, `amplihack-memory-lib.git`).
  `unknown-git = "deny"` holds; the bump never adds a git source to work around
  the allowlist.
- **Pin integrity.** Each pin is an exact 40-char SHA verified equal to its
  upstream `main` HEAD, and `Cargo.lock`'s git rev is re-confirmed to match after
  `cargo update`.
- **No new transitive crates.** `amplihack-agent-eval` stays light
  (`serde`/`serde_json`/`thiserror`/`tracing`/`chrono` only); the
  `amplihack-memory-lib` delta between `5d7db77` and `f800370` introduces no new
  Rust crate into Simard's graph, so `cargo-audit` / `cargo-deny` / `cargo-vet`
  have no new advisory, license, or certification to evaluate.
- **First-party exemptions still apply.** Both crates remain
  [exempt by ownership](./dependency-trust-policy.md#exemption-criteria) — they
  are first-party git dependencies Simard controls and pins by exact rev.

---

## Build, test, and hygiene gates

The bump is "done" only when every gate below is green.

| Gate | Command | Requirement |
| --- | --- | --- |
| Release build | `cargo build --release` | exits `0` against the new revs |
| Workspace tests | `cargo test` | exits `0` |
| Single engine | `cargo tree -i lbug` | resolves to exactly one `lbug v0.17.1` |
| Supply chain | `cargo deny --locked check` / `cargo audit` / `cargo vet --locked` | all green |
| No new stray prints | AST meta-test (`syn` scan) | no new `println!`/`eprint!`/`dbg!` in the diff |
| No new client-cased identifiers | diff review | no new CamelCase `…client` type/struct names introduced (pre-existing snake_case `gym_runner_client` names stay) |

**Process constraints (binding).** The bump PR is opened against `rysweet/Simard`
off the latest `origin/main` and is landed through the normal
[PR-finalization pipeline](./pr-finalization-pipeline.md): **all required CI
checks must be green before merge**. No `git commit --no-verify` and no
`gh pr merge --admin` anywhere — the bump earns its merge the same way every
Simard change does.

---

## Done-gate: the fix must run in Simard's own build

Under the [dependency-pin reconcile](../howto/self-maintain-dependency-pins.md),
bumping the upstream repo is **not** "done". This reconcile is done only once:

1. Both `Cargo.toml` revs equal the upstream `main` HEADs.
2. `Cargo.lock` records those same revs.
3. `cargo build --release` and `cargo test` pass.
4. `cargo tree -i lbug` shows a single `0.17.1`.
5. The supply-chain jobs are green.
6. The bump PR has **merged** to `rysweet/Simard` with all required CI green.

Rolling the merged bump into the **running daemon** remains the operator's step,
performed via [Safe Self-Update](../safe-self-update.md) — it is not required for
this reconcile's goal to report done.

---

## Reproduce / verify end-to-end

```bash
# 1. Both pins point at the current upstream main HEAD (0 drift):
for pair in \
  "amplihack-rs:amplihack-agent-eval" \
  "amplihack-memory-lib:amplihack-memory"; do
  repo=${pair%%:*}; crate=${pair##*:}
  pinned=$(grep "$crate " Cargo.toml | grep -oE '[0-9a-f]{40}')
  behind=$(gh api "repos/rysweet/$repo/compare/$pinned...main" --jq '.behind_by')
  echo "$crate: pin=$pinned behind_by=$behind"   # behind_by must be 0
done

# 2. Exactly one engine:
cargo tree -i lbug        # single lbug v0.17.1

# 3. Build + test:
cargo build --release && cargo test

# 4. Supply chain:
cargo deny --locked check && cargo audit && cargo vet --locked
```

---

## See also

- [Keep Simard's dependency pins up to date](../howto/self-maintain-dependency-pins.md) —
  the reactive done-gate and proactive reconcile this bump instantiates.
- [Library-backed Gym Evaluation Engine](../architecture/gym-eval-library-adapter.md) —
  the `amplihack-agent-eval` consumer that compiles unchanged across the bump.
- [Library-backed Cognitive Memory](../architecture/cognitive-memory-library-adapter.md) —
  the `amplihack-memory` consumer and the lbug/store-format relationship.
- [Dependency Trust Policy](./dependency-trust-policy.md) and
  [Supply-Chain Audit & Guardrails](./supply-chain-audit.md) — the guardrails the
  bump is re-verified against.
- [Safe Self-Update](../safe-self-update.md) — the operator step that rolls the
  merged bump into the running daemon.
