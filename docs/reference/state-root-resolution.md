---
title: State-root resolution
description: How `simard` resolves the durable state root and its subdirectories — the single helper shared by `simard meeting`, `simard goal-curation`, and the OODA daemon.
last_updated: 2026-05-19
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../operations/meeting-handoffs.md
  - ./meeting-close-lifecycle.md
  - ./simard-cli.md
  - ../howto/carry-meeting-decisions-into-engineer-sessions.md
---

# State-root resolution

`simard`'s durable artifacts — meetings, handoffs, goal board, cognitive
memory backups — all hang off a single **state root** directory. The
state root is resolved by one helper module
(`crate::state_root`) that every operating mode shares. This page
documents the resolution ladder, the environment variables, and the
guarantees the helper makes.

> Before this helper existed, the meeting REPL hardcoded
> `~/.simard/meetings/` and ignored `SIMARD_STATE_ROOT` (issue #1906).
> All meeting/handoff/transcript code now flows through this helper,
> so `SIMARD_STATE_ROOT=/x simard meeting ...` writes under `/x` as
> operators expect.

---

## Public API

```rust
pub fn simard_state_root() -> PathBuf;

pub fn resolve_subdir(name: &str) -> PathBuf;
```

The crate root re-exports both functions:

```rust
use simard::state_root::{simard_state_root, resolve_subdir};
```

`goal_curation::operations::simard_state_root` is preserved as a
delegating re-export so existing `use goal_curation::operations::...`
imports continue to compile and resolve to the same path.

### `simard_state_root() -> PathBuf`

Returns the resolved root directory. Resolution order:

1. `$SIMARD_STATE_ROOT` if set, **non-empty**, **absolute**, and free
   of interior NUL bytes.
2. `~/.simard` otherwise.

A non-absolute or NUL-bearing `SIMARD_STATE_ROOT` is **ignored with a
WARN**, not an error — boot never fails on a bad state-root env. The
helper does not create the directory; callers create it on first
write.

### `resolve_subdir(name: &str) -> PathBuf`

Returns `simard_state_root().join(name)`. Used by callers that want
the meeting bundle root, the handoff root, etc., to live under a
single configurable parent.

`name` is a literal subdirectory string chosen by the caller
(`"meetings"`, `"meeting_handoffs"`, `"goals"`, ...). The helper does
no validation on `name`; callers must use static strings.

---

## Subdirectory layout

| Subdirectory | Purpose | Default absolute path |
|---|---|---|
| `<root>/meetings/` | Per-meeting bundle directories (transcript, handoff, markdown) | `~/.simard/meetings/` |
| `<root>/meeting_handoffs/` | Flat handoff drop directory consumed by OODA + engineer-loop | `~/.simard/meeting_handoffs/` |
| `<root>/goals/` | Durable goal board snapshots | `~/.simard/goals/` |

> Setting `SIMARD_STATE_ROOT=/x` relocates **all** of the above
> together; operators who want to relocate just one keep using the
> narrow override (see next section).

Cognitive-memory backups under `<root>/backups/` are **not** in scope
for this migration — they continue to resolve through the existing
`NativeCognitiveMemory` paths and are tracked for state-root unification
in a follow-up issue. Setting `SIMARD_STATE_ROOT` today does **not**
relocate the backup tree.

---

## Environment variables

### Highest-precedence narrow overrides

The narrow per-subsystem variables win over `SIMARD_STATE_ROOT`. This
preserves backward compatibility with operators who pin a single
directory (e.g., for a session-scoped handoff drop) while letting
`SIMARD_STATE_ROOT` relocate everything else.

| Variable | What it overrides | Notes |
|---|---|---|
| `SIMARD_HANDOFF_DIR` | The flat handoff drop directory (was `target/meeting_handoffs` previously) | Used by `simard meeting`, OODA daemon, engineer-loop ingestion |
| `SIMARD_MEETINGS_DIR` | The per-meeting bundle root | Used by the meeting REPL and bundle persistence |
| `SIMARD_MEETINGS_ROOT` | Alias for `SIMARD_MEETINGS_DIR`; resolves identically | Compatibility surface |

When **none** of the narrow vars are set, the subsystem asks the
helper for its subdirectory (e.g., `resolve_subdir("meetings")`) and
the path is derived from `SIMARD_STATE_ROOT`/default.

### Resolution order (per-subsystem)

For each subsystem (handoff, meeting bundle, goal board, ...), the
effective path is determined by the **first match** in this list:

1. The subsystem's narrow env var (e.g., `SIMARD_HANDOFF_DIR`), if set
   and non-empty.
2. `$SIMARD_STATE_ROOT/<subdir>`, if `SIMARD_STATE_ROOT` is set and
   valid.
3. `$HOME/.simard/<subdir>` (default).

`CARGO_MANIFEST_DIR` is **no longer** consulted at runtime; previously
`default_handoff_dir()` baked the manifest dir into release binaries,
which is fixed incidentally by routing every callsite through this
helper.

---

## Validation rules

The helper enforces three lightweight checks on `SIMARD_STATE_ROOT`:

| Check | Behavior on failure |
|---|---|
| Non-empty | Empty string is silently ignored (treated as unset) |
| Absolute path | Relative path is **ignored with a WARN**; resolution falls through to `~/.simard` |
| No interior NUL | Path containing `\0` (any platform) is **ignored with a WARN** |

The helper does **not**:

- Reject `..` components (operators are trusted on a single-user CLI).
- Reject symlinks (the residual symlink-race risk is accepted; see
  Security below).
- Create the directory (the first writer does that).
- Retroactively `chmod` an existing root (only freshly-created
  subdirectories get the explicit modes documented below).

---

## Permissions

Newly-created state directories and files are created with explicit
unix modes:

| Artifact | Mode | Note |
|---|---|---|
| New state-root subdirectory (e.g., `meetings/`, `meeting_handoffs/`) | `0o700` | Owner-only access |
| `meeting_handoff.json`, `meeting_handoff.md`, `transcript.json` | `0o600` | Owner-only read/write |
| Pre-existing directories | unchanged | Helper does **not** chmod existing trees |

On non-unix targets the explicit mode is omitted and the file inherits
the umask of the writing process.

---

## Worked examples

### Default (no env vars)

```bash
$ simard meeting repl daily backup policy
[meeting] writing handoff to: /home/azureuser/.simard/meeting_handoffs/meeting_handoff.json
[meeting] writing bundle to:  /home/azureuser/.simard/meetings/2026-05-19T17-23-16Z-daily-backup-policy/
```

### `SIMARD_STATE_ROOT` overrides everything

```bash
$ SIMARD_STATE_ROOT=/srv/simard-state simard meeting repl daily backup policy
[meeting] writing handoff to: /srv/simard-state/meeting_handoffs/meeting_handoff.json
[meeting] writing bundle to:  /srv/simard-state/meetings/2026-05-19T17-23-16Z-daily-backup-policy/
```

### Narrow override beats `SIMARD_STATE_ROOT`

```bash
$ SIMARD_STATE_ROOT=/srv/simard-state \
  SIMARD_HANDOFF_DIR=/tmp/this-session-handoffs \
  simard meeting repl daily backup policy
[meeting] writing handoff to: /tmp/this-session-handoffs/meeting_handoff.json
[meeting] writing bundle to:  /srv/simard-state/meetings/2026-05-19T17-23-16Z-daily-backup-policy/
```

### Invalid `SIMARD_STATE_ROOT` is ignored

```bash
$ SIMARD_STATE_ROOT=relative/path simard meeting repl scratch
WARN simard::state_root: ignoring SIMARD_STATE_ROOT='relative/path' reason=not_absolute
[meeting] writing handoff to: /home/azureuser/.simard/meeting_handoffs/meeting_handoff.json
```

```bash
$ SIMARD_STATE_ROOT=$'/abs/with\0nul' simard meeting repl scratch
WARN simard::state_root: ignoring SIMARD_STATE_ROOT reason=contains_nul
[meeting] writing handoff to: /home/azureuser/.simard/meeting_handoffs/meeting_handoff.json
```

---

## Verifying the resolved root

```bash
simard debug state-root
```

Prints the resolved root and the narrow overrides currently in effect
(if any), without performing any writes. Output is plain text suitable
for piping to other tools:

```
state_root=/home/azureuser/.simard
  source=default
meetings_dir=/home/azureuser/.simard/meetings
  source=state_root
meeting_handoff_dir=/home/azureuser/.simard/meeting_handoffs
  source=state_root
goals_dir=/home/azureuser/.simard/goals
  source=state_root
```

When a narrow override is active:

```
meeting_handoff_dir=/tmp/this-session-handoffs
  source=SIMARD_HANDOFF_DIR
```

---

## Security notes

- **Path validation is intentionally minimal.** The CLI is single-user
  and the operator owns the host; an operator who points
  `SIMARD_STATE_ROOT` at a path they cannot write to gets a write
  error on first persist, not a startup error.
- **Symlink race (R-9).** Following a symlinked state root inherits
  the standard local-filesystem symlink-race surface. This is
  accepted as a known residual for the single-user CLI threat model.
- **No allowlist.** The helper does not maintain a path allowlist;
  operators may point at any absolute path they have permission to
  write.

---

## See also

- [Meeting close lifecycle](./meeting-close-lifecycle.md) — the
  timeout and partial-handoff contract that writes into the resolved
  state root.
- [Meeting REPL & handoff ingestion](../operations/meeting-handoffs.md)
  — operator-facing handoff workflow.
- [`simard` CLI reference](./simard-cli.md) — every command that
  consults the state root.
- [Carry meeting decisions into engineer sessions](../howto/carry-meeting-decisions-into-engineer-sessions.md)
  — end-to-end flow that depends on shared state-root resolution.
