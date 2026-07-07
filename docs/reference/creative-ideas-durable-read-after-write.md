---
title: Creative Ideas durable read-after-write
description: >
  How persisted creative ideas become immediately visible to the dashboard
  reader (read-after-write) and survive a non-graceful daemon restart
  (durability). Documents the unified dashboard state-root resolver (D1) that was
  the root cause, the engine's write-through WAL that already guarantees
  durability, the `GET /api/creative-ideas` contract, operator verification, and
  the three-layer regression suite. Fixes issue #2798.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./creative-ideas-api.md
  - ./state-root-resolution.md
  - ./operator-read-state-root-contract.md
  - ../operations/cognitive-memory-durability.md
  - ../design/creative-ideas-thread.md
  - ../howto/configure-creative-ideas-thread.md
  - ../testing/cognitive-memory-serial-isolation.md
---

# Creative Ideas durable read-after-write

The Creative Ideas thread generates a batch of ten candidate ideas per run and
persists each one to prospective memory. Before issue #2798 the dashboard
**Creative Ideas** tab was **always empty**: `GET /api/creative-ideas` returned
`ideas: []` and every status count `0`, even while journald showed the thread
reporting `10 persisted` on every tick.

This page documents the shipped fix. The symptom was **not** a UI bug, a
status-filter bug, or a "no ideas were generated" bug. It was a single
root-cause defect in the **prospective-memory persistence seam** for creative
ideas — a read-after-write miss (D1). The second, durability half of the
requirement was *already* satisfied by the memory engine and needed no Simard
code:

| Concern | Finding | Resolution |
|---|---|---|
| **D1 — read-after-write** (the bug) | The dashboard reader opened a *different* store view than the daemon's in-process writer, so freshly-persisted ideas were invisible in the same running process. | Route the dashboard reader through the **same** state-root resolver the daemon registers with, so `open_reader_client` tier-0 shares the live in-process writer handle. |
| **Durability across restart** (already held) | An early hypothesis blamed *buffer-only* prospective writes lost on `SIGKILL`. This was **disproved**: the pinned engine's WAL is *write-through* and replayed on open, so persisted ideas already survive a non-graceful restart with no explicit checkpoint. | **No Simard change.** The durability requirement is met by the engine; a regression test pins it so a future engine bump can't silently break it. |

The fix is a **Simard-side seam only** (the dashboard resolver). No
`amplihack-memory` engine change was required and the store format (v41) is
unchanged — see [Memory-architecture policy (G2)](#memory-architecture-policy-g2).

---

## Architecture

### The two paths that must agree

```
WRITE PATH (OODA daemon, creative-ideas thread)          READ PATH (dashboard)
--------------------------------------------------       -------------------------------
CreativeIdeasThread::tick                                GET /api/creative-ideas
  ProspectiveCreativeIdeaStore::store(&idea)               load_ideas(state_root)
    CognitiveMemoryOps::store_prospective(                   open_reader_client(state_root)
      &idea.idea, CREATIVE_IDEA_TRIGGER, &action, prio)        tier 0: lookup_in_process_writer  <-- must hit
    (engine WAL is write-through: durable on commit)         ProspectiveCreativeIdeaStore::list
                                                                 (filter trigger == CREATIVE_IDEA_TRIGGER)
```

The daemon registers the live writer under its resolved state root:

```rust
// src/operator_commands_ooda/daemon/mod.rs
let state_root = state_root_override.unwrap_or_else(memory_ipc::default_state_root);
// default_state_root() delegates to crate::state_root::simard_state_root()
memory_ipc::register_in_process_writer(state_root.clone(), Arc::clone(&shared_mem));
```

`open_reader_client` tier-0 only returns the shared writer when the reader's
`state_root` **canonicalises to the same path** as the registered key:

```rust
// src/memory_ipc/launcher.rs
fn lookup_in_process_writer(state_root: &Path) -> Option<Arc<dyn CognitiveMemoryOps>> {
    // ...
    if canonical_or_self(state_root) != canonical_or_self(registered_root) {
        return None; // tier-0 MISS -> falls through to a separate on-disk view
    }
    // ...
}
```

If the dashboard hands tier-0 a **different** path than the daemon registered,
the lookup misses, the reader falls through to a separate tier-2 on-disk store
handle, and it never observes the daemon's buffered prospective writes. That is
exactly what happened.

### Root cause of D1: two divergent resolvers

Before the fix the dashboard used a **private copy** of the resolver that took
`$SIMARD_STATE_ROOT` verbatim, with a hardcoded `/home/azureuser` fallback:

```rust
// BEFORE (src/operator_commands_dashboard/routes.rs) — divergent
pub(crate) fn resolve_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
            std::path::PathBuf::from(home).join(".simard")
        })
}
```

The daemon instead used the canonical `crate::state_root::simard_state_root()`,
which **validates** `SIMARD_STATE_ROOT` (ignores empty / relative / NUL-bearing
values with a WARN and falls back to `~/.simard`). Any environment where the
two resolvers disagreed — an empty or relative `SIMARD_STATE_ROOT`, or an unset
`HOME` — produced a tier-0 key mismatch and a permanently-empty tab.

### D1 fix: single resolver, single source of truth

The dashboard resolver now **delegates** to the canonical resolver, so the
reader's tier-0 key is byte-for-byte the daemon's registration key:

```rust
// AFTER (src/operator_commands_dashboard/routes.rs) — unified
pub(crate) fn resolve_state_root() -> std::path::PathBuf {
    crate::state_root::simard_state_root()
}
```

This eliminates the entire divergence class (validation mismatch, HOME-fallback
mismatch) at the source. See
[State-root resolution](./state-root-resolution.md) for the resolver's full
validation ladder.

> **Auth is intentionally *not* affected.** The dashboard login secret is still
> resolved from a hardcoded `$HOME/.simard/.dashkey`
> (`operator_commands_dashboard::auth::dashkey_path`), fully decoupled from
> `resolve_state_root()`. The D1 change must never relocate or expose the auth
> key; a regression guard asserts this decoupling.

### Durability: provided by the engine's write-through WAL

`ProspectiveCreativeIdeaStore::store` calls the backend's `store_prospective`,
which commits the write to lbug's WAL. The pinned engine opens the store with
`CognitiveMemory::open_persistent_with_recovery`, whose **WAL is write-through
and replayed on open** — so a committed prospective write survives a
non-graceful process exit (`SIGKILL` during deploy) and is recovered when the
store is reopened. An explicit `checkpoint()` (== the library's `close()`, which
folds the WAL into the main file) is **not** required for durability.

This was verified empirically and is pinned by a regression test: persist an
idea, `std::mem::forget` the writer handle so its graceful-`Drop` checkpoint
never fires, clear the process store cache, cold-reopen from disk — the idea is
still listable (Layer C, below). The creative-ideas thread therefore adds **no
per-batch checkpoint**; the minimal correct fix is the D1 resolver change alone.

Decision-scope memory and creative-idea prospective memory are durable by the
*same* write-through mechanism; the always-empty tab was never a durability gap.
If a future engine bump moved durability back behind an explicit checkpoint,
Layer C fails RED and the durable fix belongs in the engine (G2), not a
Simard-side checkpoint.

---

## Endpoint contract

Two authenticated read endpoints serve the dashboard tab. Neither writes.

### `GET /api/creative-ideas`

Returns the current idea pool (newest first) with per-status counts.

`counts` is **zero-filled over every `IdeaStatus`** (all eight variants always
appear, even at `0`); `ideas` entries carry the compact per-idea summary
(`idea_summary`) — note there is no `priority` field on the summary.

```json
{
  "counts": {
    "New": 7, "NeedsRevision": 0, "NeedsHumanReview": 2,
    "AcceptedForImplementation": 0, "Rejected": 1, "Deferred": 0,
    "ImplementationStarted": 0, "ImplementationCompleted": 0
  },
  "ideas": [
    {
      "idea_id": "…", "idea": "…", "status": "New",
      "rationale": "…", "links": 0, "reviews": 1,
      "has_metric": true, "metric": "…", "created_epoch": 1751000000
    }
  ]
}
```

### `POST /api/creative-ideas/search`

Filters the pool by `status` (one of the `IdeaStatus` names) and/or a
case-insensitive `query` substring:

```json
{ "status": "New", "query": "worktrees" }
```

Returns `{ "results": [ … ] }`.

### No-fallback / no-silent-success contract

Both endpoints **fail loud**. If the pool cannot be loaded, the response is an
explicit error object — never a silent empty array:

```json
{ "error": "<load error>", "ideas": [], "counts": {} }
```

An explicit-but-unknown `status` in a search is a **fail-closed** error, not a
silent "return everything" (the exact `error` text is the wrapped
`parse_idea_status` failure — illustrative below):

```json
{ "error": "<invalid status 'Bogus'>", "results": [] }
```

`counts` reflecting **all-zero across every status** is therefore only ever
produced by a genuinely empty pool — it can no longer be produced by a
read-after-write miss, because the reader now shares the writer handle.

---

## Operator verification

The Creative Ideas endpoints sit behind dashboard auth. Verify the fix
end-to-end against a running daemon (`:8080`) as follows.

### 1. Authenticate with the dashboard login code

The login code is generated at startup and persisted to `~/.simard/.dashkey`.
Exchange it for a `simard_session` cookie:

```bash
CODE="$(cat ~/.simard/.dashkey)"
curl -s -c /tmp/simard.cookies \
  -H 'content-type: application/json' \
  -d "{\"code\":\"$CODE\"}" \
  http://127.0.0.1:8080/api/login
# -> {"ok":true}   (sets simard_session cookie)
```

### 2. Read the pool

```bash
curl -s -b /tmp/simard.cookies http://127.0.0.1:8080/api/creative-ideas | jq '.counts, (.ideas | length)'
```

**Expected after a thread run:** non-zero status counts and a populated `ideas`
array. A response of all-zero counts with an empty `ideas` array — while
journald shows `creative_ideas: … 10 persisted` — indicates the bug has
regressed (a read-after-write miss).

### 3. Confirm the thread persisted this run

```bash
journalctl -u simard-ooda --since "1 hour ago" \
  | grep -E 'cognitive-thread: creative_ideas: .* persisted'
# cognitive-thread: creative_ideas: generated 10 idea(s), 10 survived dedup, 10 persisted, N reviewed (N -> goals)
```

### 4. Verify durability across a restart

Restart the daemon and re-read. The ideas must persist — they are durable via
the engine's write-through WAL:

```bash
sudo systemctl restart simard-ooda
# wait for the daemon to come back up, then:
curl -s -c /tmp/simard.cookies -H 'content-type: application/json' \
  -d "{\"code\":\"$(cat ~/.simard/.dashkey)\"}" http://127.0.0.1:8080/api/login >/dev/null
curl -s -b /tmp/simard.cookies http://127.0.0.1:8080/api/creative-ideas | jq '.ideas | length'
# -> still > 0
```

Even a **non-graceful** exit (`SIGKILL` during deploy) preserves already-persisted
ideas: the engine commits each prospective write to its WAL and replays the WAL
on the next open, so no explicit checkpoint is needed.

---

## Configuration

No new configuration is introduced. The relevant existing knobs:

| Env var | Default | Effect |
|---------|---------|--------|
| `SIMARD_CREATIVE_IDEAS_ENABLED` | `false` | Master switch for the thread. Ideas are only generated/persisted when truthy. |
| `SIMARD_CREATIVE_IDEAS_INTERVAL_SECS` | `86400` | Generator cadence. |
| `SIMARD_CREATIVE_IDEAS_BATCH` | `10` | Ideas targeted per run. |
| `SIMARD_STATE_ROOT` | *(unset → `~/.simard`)* | Resolved identically by the daemon **and** the dashboard reader after D1. See [State-root resolution](./state-root-resolution.md). |

> **Behavioral note on `SIMARD_STATE_ROOT`.** After D1 the dashboard honors the
> *validated* resolution rules: an **empty or relative** `SIMARD_STATE_ROOT` is
> ignored (WARN) and falls back to `~/.simard`, matching the daemon. A
> deployment that previously relied on the dashboard's old CWD-relative or
> verbatim-empty behavior will now resolve to `~/.simard` — this is the intended
> correction, and it is what makes the reader and writer agree.

The live store lives at `state_root/cognitive` (the `LIVE_STORE_SUBDIR`);
prospective creative-idea rows are filtered on the `CREATIVE_IDEA_TRIGGER`
(`"creative-idea"`) sentinel. See
[Creative Ideas subsystem API reference](./creative-ideas-api.md) for the full
type surface and the `CreativeIdeaStore` seam.

---

## Regression coverage

The fix is guarded by a **three-layer RED→GREEN** suite that isolates which
seam a future regression breaks. Each layer fails RED on the corresponding
unpatched defect and passes GREEN after the fix.

| Layer | Location | Asserts | Guards |
|---|---|---|---|
| **A — engine read-after-write** | `src/cognitive_memory/tests_library_parity.rs` | One handle: `store_prospective(CREATIVE_IDEA_TRIGGER)` → same-handle `list_all_prospective` is non-empty. | The engine buffers-but-serves its own writes. If this ever fails RED the defect is engine-level and escalates to `amplihack-memory` (see G2). |
| **B — reader seam (D1)** | `src/operator_commands_dashboard/tests_state_root_parity.rs` | Dashboard `resolve_state_root()` == daemon `default_state_root()` / `simard_state_root()` across env permutations (empty / relative / unset `HOME`). Store on the registered writer, then a **fresh** `open_reader_client(same state_root)` `list` is non-empty. | The read-after-write miss (divergent resolver → tier-0 miss). |
| **C — engine write-through durability** | `src/cognitive_memory/tests_library_parity.rs` (plus an end-to-end tick variant in `src/creative_ideas/tests.rs`) | Persist → **simulated non-graceful restart** (`std::mem::forget` the writer so no graceful checkpoint fires; `clear_tier2_store_cache()` between) → cold-reopen `list` is non-empty, with **no explicit checkpoint**. | Durability regressing in the engine (WAL no longer write-through / replayed). Escalates to `amplihack-memory` per G2. |

Additional guards:

- **Fail-loud read contract.** The read endpoint never masks a load failure as
  an empty pool: on any load error it returns
  `{ "error": …, "ideas": [], "counts": {} }` rather than a silent empty pool.
- **Auth decoupling.** The `.dashkey` login path stays hardcoded to
  `$HOME/.simard/.dashkey` and independent of `resolve_state_root()`.

### Test isolation

These tests mutate/read the process-global state-root env surface and register
the in-process writer, so they run under the `cognitive_memory` **serial** group
and use `test_support::HermeticState`. Always `clear_in_process_writer()` (and
`clear_tier2_store_cache()` for durability tests) between runs. The
non-graceful-restart simulation `std::mem::forget`s the writer so `Database::drop`
never fires an implicit checkpoint — that is what makes it a real `SIGKILL`
simulation and proves the engine's WAL (not a checkpoint) provides durability. See
[serial(cognitive_memory) test isolation](../testing/cognitive-memory-serial-isolation.md)
and [Writing hermetic tests against cognitive memory](../testing/hermetic-tests.md).

---

## Memory-architecture policy (G2)

Per the memory-architecture policy, durability and read-after-write semantics of
the memory **engine** (lbug checkpoint/flush, the `memory_ipc` reader-view tier
ladder) belong in the engine, **rysweet/amplihack-memory-lib** — Simard must not
fork engine logic. This fix respects that boundary:

- **Layer A passes** on the pinned engine: the engine already serves its own
  buffered prospective writes on the same handle. No engine defect was found, so
  **no engine change and no dependency-pin bump** were made. Store format v41 is
  frozen.
- The Simard-side seam — the dashboard resolver (D1) — is the only change. It
  routes the reader through the canonical resolver; it does not touch engine
  durability logic. Durability is left entirely to the engine's write-through WAL.
- **Escalation rule:** if Layer A or C ever fails RED (the engine drops or fails
  to replay prospective writes it commits), the durable fix moves to
  `amplihack-memory-lib` and Simard bumps its pinned dependency — never a
  Simard-side fork of engine logic or a compensating checkpoint.

---

## Troubleshooting

### The tab is empty but journald says ideas were persisted

A read-after-write miss. Confirm the daemon and dashboard resolve the **same**
state root. Both use the same resolver (`crate::state_root::simard_state_root()`);
inspect the resolved root that the CLI (and, after D1, the dashboard) uses:

```bash
simard memory stats --json | jq -r .state_root   # resolver-resolved state root
echo "SIMARD_STATE_ROOT=${SIMARD_STATE_ROOT:-<unset>}"
```

If `SIMARD_STATE_ROOT` is set to an empty or relative value, both resolvers now
fall back to `~/.simard` (WARN logged) — the dashboard no longer diverges. If
they still differ, the daemon and dashboard are running as different users or
with different `HOME`; align them.

### The tab empties after a restart

Persisted creative ideas are durable via the engine's write-through WAL, so this
should not happen. If ideas were persisted (journald shows `… persisted`) but
vanish after a `SIGKILL`/deploy, durability has regressed in the memory **engine**
(the WAL is no longer write-through or is not being replayed on open) — that is a
`amplihack-memory` issue (G2), not a Simard seam. Confirm the persist ran:

```bash
journalctl -u simard-ooda --since "2 hours ago" \
  | grep -E 'creative_ideas: .* persisted'
```

As a recovery point, the most recent verified backup under `state_root/backups`
still contains the pool; see
[Cognitive memory durability](../operations/cognitive-memory-durability.md).

### The endpoint returns `{"error": …}`

This is the **intended** fail-loud contract, not a silent empty. The `error`
string names the load failure (permissions, a corrupt store, a resolver
mismatch). Fix the underlying cause; the endpoint never masks a load failure as
an empty pool.

---

## See also

- [Creative Ideas subsystem API reference](./creative-ideas-api.md) — the full
  type surface, `CreativeIdeaStore`, the reviewer pipeline, and configuration.
- [Creative Ideas background thread (design)](../design/creative-ideas-thread.md)
  — motivation, decisions, and roadmap.
- [Configure and operate the Creative Ideas thread](../howto/configure-creative-ideas-thread.md)
  — turning the thread on and operating it.
- [State-root resolution](./state-root-resolution.md) — the canonical resolver
  the dashboard now delegates to.
- [Cognitive memory durability](../operations/cognitive-memory-durability.md) —
  checkpoint, verified backups, and restore.
- [serial(cognitive_memory) test isolation](../testing/cognitive-memory-serial-isolation.md)
  — the serial-group contract the regression suite runs under.
- [GitHub issue #2798](https://github.com/rysweet/Simard/issues/2798) — the
  always-empty Creative Ideas tab bug this page fixes.
