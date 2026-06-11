# Meeting REPL & Handoff Ingestion

How to drive a real meeting through the REPL, what the structured
handoff JSON contains, and how the OODA daemon (and the engineer loop)
ingest handoff artifacts on their next cycle.

> Quick reference: see the
> [Real-Meeting & Dashboard E2E Verification section in CONTRIBUTING.md](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#real-meeting--dashboard-e2e-verification).

---

## Overview

A "meeting" is an interactive REPL session between an operator and
Simard's brain (the LLM-driven decision layer). Meetings are scoped to
a topic and produce, on `/close`, a single structured handoff JSON
file (`meeting_handoff.json`) under the configured handoff directory.

Two consumers ingest the handoff:

1. **The OODA daemon** scans the handoff directory at the start of
   each cycle (`src/ooda_loop/cycle.rs`) and ingests unprocessed
   decisions as goals/backlog items. It logs the count to journal.
2. **The engineer loop** also scans on startup
   (`src/engineer_loop/meeting_decisions.rs`) so an operator can drive
   intent into in-flight engineer work without restarting the daemon.

Additionally, the meeting close pipeline now **writes goal records
directly** to `goal_store.json` before writing the handoff artifact
(issue #2182). This gives immediate query visibility to consumers that
read the file-backed goal store (meeting backend, engineer loop,
dashboard probes) without waiting for the OODA curate phase. See
[Meeting close lifecycle](../reference/meeting-close-lifecycle.md#direct-goal-record-writes-issue-2182)
for details.

This is the canonical mechanism for moving operator intent into
Simard's autonomous workstream.

---

## Starting a Meeting

```bash
simard meeting repl <topic-words>
```

`<topic-words>` is one or more words; the CLI joins them into a topic
string (see `src/operator_cli/meeting.rs`). The first word may also be
a `repl` subcommand keyword, in which case the topic is the remaining
words.

Examples:

```bash
simard meeting repl daily backup policy
simard meeting repl "Sprint planning 2026-05-09"
```

A meeting with **no** `repl` subcommand and no topic operates against
the most recent in-progress session if one exists.

---

## REPL Commands

The REPL is a thin loop over `MeetingBackend` (see
`src/meeting_repl/repl.rs` and `src/meeting_backend/`). The following
commands are recognized; everything else is treated as natural
conversation with the bound LLM agent.

| Command | Effect |
|---|---|
| (any text) | Sent to the brain as a prompt; response printed inline |
| `/help` | Show the command list |
| `/status` | Show session info (topic, started_at, current decision/action counts) |
| `/template` | List meeting templates |
| `/template <name>` | Apply a template (`standup`, `1on1`, `retro`, `planning`) |
| `/theme <text>` | Record a theme for this meeting |
| `/recap` | Color-coded session recap |
| `/preview` | Preview the handoff artifact before closing |
| `/export` | Export the meeting as markdown |
| `/close` | Finalize and write `meeting_handoff.json`; exit the REPL |

The brain implicitly extracts decisions, action items, and open
questions from free-form discussion; explicit operator intent
(e.g., a clearly phrased decision in chat) is folded in at `/close`
time.

> **Important**: the REPL **requires** a working LLM agent. If
> `SIMARD_LLM_PROVIDER` is unset or auth fails, the REPL aborts with
> `ActionExecutionFailed { action: "meeting-repl", reason: "No LLM
> agent backend available..." }`. There is no silent degradation to
> note-taking mode.

---

## Handoff JSON Schema

The handoff filename is the well-known constant
`MEETING_HANDOFF_FILENAME = "meeting_handoff.json"` defined in
`src/meeting_facilitator/handoff/mod.rs`. There is one such file per
handoff directory; the directory itself can be partitioned per session
by setting `SIMARD_HANDOFF_DIR` to a session-specific path before
launching the REPL.

| Setting | Default | Override |
|---|---|---|
| Handoff directory | `<state-root>/meeting_handoffs` (state-root defaults to `~/.simard`) | `SIMARD_HANDOFF_DIR=<path>` (narrow) or `SIMARD_STATE_ROOT=<path>` (broad) |
| Per-meeting bundle directory | `<state-root>/meetings/<id>/` | `SIMARD_MEETINGS_DIR=<path>` (narrow) or `SIMARD_STATE_ROOT=<path>` (broad) |

The full env-var resolution ladder (narrow wins over broad; broad
wins over default) is documented in
[State-root resolution](../reference/state-root-resolution.md).
`CARGO_MANIFEST_DIR` is **no longer** consulted at runtime; previously
it leaked into release binaries via `default_handoff_dir()`.

Schema (the `MeetingHandoff` struct in
`src/meeting_facilitator/handoff/mod.rs`):

```json
{
  "topic": "daily backup policy",
  "started_at": "2026-05-09T17:23:16Z",
  "closed_at":  "2026-05-09T17:31:42Z",
  "decisions": [
    {
      "description": "Adopt a daily backup-verification job that opens the latest backup read-only and runs search_facts.",
      "rationale": "Catches silent corruption from lbug WAL issues like the 2026-05-09 incident.",
      "participants": ["operator", "simard"]
    }
  ],
  "action_items": [
    {
      "description": "Implement daily verification job under operator_commands/backup_verify.",
      "owner": "simard",
      "priority": 1,
      "due_description": "Within next sprint"
    },
    {
      "description": "Review verification-job alert wiring once landed.",
      "owner": "operator",
      "priority": 2,
      "due_description": null
    }
  ],
  "open_questions": [],
  "processed": false,
  "duration_secs": 506,
  "transcript": ["operator: ...", "simard: ..."],
  "participants": ["operator:azureuser", "simard:rustyclawd"],
  "themes": ["backup verification", "WAL durability"]
}
```

### Empty handoff filtering

When a meeting produces zero decisions **and** zero action items, the
close pipeline skips writing to `meeting_handoffs/` entirely (issue
#2269). The per-meeting archival bundle under `~/.simard/meetings/` is
still written regardless. This means a `handoff-*.json` file in the
handoff directory always contains at least one decision or action item.

The `processed` flag must be `false` for a freshly written handoff;
ingestion flips it to `true` via `mark_meeting_handoff_processed`.

### Atomic writes

`meeting_handoff.json`, `meeting_handoff.md`, and `transcript.json`
are written via a tmp-file + `fsync` + `rename` sequence. A reader
that opens the file mid-close sees either the previous content (or
no file) or the new full content — never a half-written JSON
document. This invariant holds **even when the close pipeline hits
a timeout**; see [Meeting close
lifecycle](../reference/meeting-close-lifecycle.md).

### Permissions

Newly-created state directories are created with mode `0o700` and
the handoff files themselves with mode `0o600` on unix. Pre-existing
directories are not retroactively `chmod`'d. On non-unix targets the
file inherits the umask of the writing process; set `umask 0077`
before running the REPL if owner-only access matters.

### Close timeout & partial handoffs

`/close` is bounded by a master timeout (default 60s, configurable
via `SIMARD_MEETING_CLOSE_TIMEOUT_SECS`, clamped to `[1, 600]`)
plus an inner agent-close budget (default 45s, configurable via
`SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS`, clamped to `[1, 120]`)
plus a ~2s subprocess SIGTERM→SIGKILL grace. The combined
worst-case wall-clock ceiling is therefore 107s (well under the
documented 90s public ceiling). If any phase (agent shutdown,
summary extraction, cognitive-memory flush) exceeds its inner
budget, the close still writes a deserialize-valid bundle to disk
and emits:

```
WARN handoff_partial=true reason=<close_timeout|agent_close_timeout|summary_empty|bridge_timeout|persistence_error> meeting_id=<id>
```

The on-disk JSON schema is unchanged on a partial close — existing
consumers parse it without modification. A partial close is
identifiable by the tracing line above, by the REPL's post-close
banner (literal prefix `[meeting] WARNING: partial close (reason=`),
or by empty `decisions`/`action_items` from a productive
conversation. See
[Recover from a meeting close timeout](../howto/recover-from-meeting-close-timeout.md)
for the operator playbook and
[Meeting close lifecycle](../reference/meeting-close-lifecycle.md)
for the full timing contract.

---

## Empty Handoff Filtering (Issue #2269)

The meeting close pipeline **skips** writing to the handoff directory
when the meeting produced zero decisions **and** zero action items.
This prevents empty or automated meetings from accumulating inert
`handoff-{timestamp}.json` files that the OODA daemon must parse and
discard.

The filter is applied in `write_handoff_with_explicit`
(`src/meeting_backend/persist/mod.rs`) **after** enrichment/extraction
— the LLM has already attempted to extract structured output from the
conversation. Only meetings that genuinely produced no actionable
content are filtered. The per-meeting bundle in `~/.simard/meetings/`
is still written for archival regardless.

When the filter fires, the close pipeline emits:

```
INFO Skipping empty meeting handoff (0 decisions, 0 action items)
```

The REPL `/close` output is unchanged — the operator sees the bundle
paths but no handoff-directory path for an empty meeting.

---

## Ingestion by the OODA Daemon

At the start of every OODA cycle, the daemon (see
`src/ooda_loop/cycle.rs:104`):

1. Calls `default_handoff_dir()` to compute the handoff directory
   (`SIMARD_HANDOFF_DIR` if set, else
   `state_root::resolve_subdir("meeting_handoffs")` which honors
   `SIMARD_STATE_ROOT`, defaulting to `~/.simard/meeting_handoffs`).
   See [State-root resolution](../reference/state-root-resolution.md).
2. Calls `check_meeting_handoffs(&mut state.active_goals,
   &handoff_dir, &state_root)`, which now **batch-processes up to 10 unprocessed
   handoffs per cycle** (oldest-first FIFO):
   - Calls `find_unprocessed_handoffs(handoff_dir, 10)` to list up to
     10 unprocessed handoff files (timestamped `handoff-*.json` and
     the legacy `meeting_handoff.json` — both count toward the limit).
   - For each handoff in the batch:
     - Parses the JSON. On parse failure: logs `warn!`, skips to next.
     - If `decisions` and `action_items` are both empty: marks
       processed immediately, skips ingestion.
     - Otherwise: converts each `MeetingDecision` into one or more
       goals and each `ActionItem` into a backlog item on the live
       `active_goals` board.
     - Calls `mark_handoff_processed_in_place` to flip `processed` to
       `true`.
   - Returns the **total** count of goals/backlog items created across
     all handoffs in the batch.
3. Logs:

   ```
   [simard] OODA start: ingested N goal/backlog item(s) from M meeting handoff(s)
   ```

   on success. On individual handoff errors, processing continues with
   the remaining files:

   ```
   WARN Failed to process handoff <path>: <error>; continuing with remaining handoffs
   ```

   A batch that encounters errors in every file logs:

   ```
   [simard] OODA start: meeting handoff check failed for all N handoff(s)
   ```

### Batch sizing

The batch limit of 10 is a compile-time constant. Under normal
operation (meetings producing 1–3 handoffs between cycles), the daemon
processes all pending handoffs in a single cycle. The limit prevents a
large historical backlog (e.g., 3,000+ files from pre-filter
accumulation) from stalling a single OODA cycle — the queue drains
over successive cycles at 10 per iteration.

### Verifying ingestion

```bash
journalctl -u simard-ooda --since "5 min ago" \
  | grep "ingested .* meeting handoff"
```

If you do not see this line within ~1 cycle of `/close`, check:

- The daemon is running (`systemctl status simard-ooda`).
- `SIMARD_HANDOFF_DIR` matches what `simard meeting repl` wrote (or
  both rely on the default).
- The handoff JSON has at least one decision or action item (empty
  meetings no longer write to the handoff directory).
- `processed` is still `false` (a previous OODA cycle may already have
  ingested it).

---

## Handoff Reaper (Automatic Cleanup)

The OODA daemon's resource cleanup phase (at the end of each cycle)
now runs a **handoff reaper** that deletes processed handoff files
older than 7 days. This prevents long-running daemons from
accumulating thousands of stale `handoff-*.json` files.

The reaper is implemented as `reap_processed_handoffs(dir, max_age)`
in `src/meeting_facilitator/handoff/persistence.rs` and called from
`src/ooda_loop/cycle.rs` with `Duration::from_secs(7 * 86400)`.

### Double-guard before deletion

A handoff file is deleted only when **both** conditions are true:

1. **`processed == true`** — the file's JSON `processed` field is
   `true`, confirming the OODA daemon or another consumer has already
   ingested it.
2. **mtime > 7 days** — the file's last-modified time is older than
   the configured `max_age` (`std::fs::metadata().modified()`).

Files that fail to parse, have `processed == false`, or have a recent
mtime are left untouched. The well-known `meeting_handoff.json`
(legacy singleton path) is **excluded** from reaping — it is used by
multiple consumers and should not be auto-deleted.

### Observability

Each deletion is logged at `info!` level:

```
INFO Reaped processed handoff: handoff-20260601T120000.json (age: 8d)
```

The reaper returns the count of deleted files. The cleanup block logs:

```
[simard] resource cleanup: reaped N stale meeting handoff(s)
```

### Filesystem error handling

If `metadata().modified()` fails (e.g., on a filesystem that does not
support mtime), the reaper logs `warn!` and skips that file. It never
panics or aborts the cleanup phase.

---

## Engineer-loop ingestion

The engineer loop also scans the handoff directory on startup
(`src/engineer_loop/meeting_decisions.rs:24`). This means an operator
can issue intent through `simard meeting repl` and have an in-flight
engineer pick it up at its next loop iteration without waiting for the
OODA daemon to cycle. The engineer-loop ingestion path uses the same
`load_meeting_handoff` and `mark_meeting_handoff_processed` helpers,
so a single handoff is ingested by whichever consumer reaches it
first.

---

## End-to-End Verification (PR Evidence)

For PRs that touch the meeting REPL, dashboard `/ws/chat`, OODA
ingestion in `src/ooda_loop/cycle.rs`, or the engineer-loop ingestion
in `src/engineer_loop/meeting_decisions.rs`, the PR body MUST include:

1. **Full meeting transcript** (or 30 KB head + 10 KB tail; full
   transcript committed under
   `docs/evidence/<date>-meeting-transcript.txt` if it exceeds
   60 KB).
2. **Handoff JSON validation**: confirm `decisions` array non-empty
   and `action_items` non-empty (the schema invariants).
3. **Journal line**: `OODA start: ingested N goal/backlog item(s)
   from meeting handoff`.
4. **Dashboard `/ws/chat` exchange** (see below): a real cycle-count
   or recent-action prompt and the agent's response, plus
   `simard memory search-facts <token>` showing a fact created during
   the exchange.

Smoke tests that only verify Unicode rendering or HTTP status codes
are **not sufficient** evidence — they could not detect (and did not
detect) the WAL-checkpoint data-loss bug from 2026-05-09.

---

## Example Session

```text
$ simard meeting repl daily backup policy
Simard v0.5.0 — meeting mode
Topic: daily backup policy
Simard is listening. Speak naturally — /help for commands, /close to end.

simard:meeting> I propose we add a daily cognitive-memory backup
                verification job that periodically opens the latest backup
                read-only, runs search_facts, and alerts if results are
                empty or panic.

[simard] Agreed in principle. The existing periodic backup loop
  (every 5 min, 24-snapshot retention) creates verified snapshots but
  there is no separate scheduled re-verification later. A daily
  read-only verification job would catch silent corruption from lbug
  WAL issues like the 2026-05-09 incident. I propose:
    - daily systemd timer (or in-daemon scheduler)
    - opens ~/.simard/backups/cognitive_memory.ladybug.<latest>
      with read_only=true
    - runs search_facts("simard") and asserts >= 1 result
    - on empty or panic: journal error! + dashboard banner

simard:meeting> /preview
[meeting] decisions: 1, action_items: 0, open_questions: 1

simard:meeting> Let's record that as a decision and an action for me.

simard:meeting> /close
[meeting] handoff written: <SIMARD_HANDOFF_DIR>/meeting_handoff.json

$ # Wait for the next OODA cycle
$ journalctl -u simard-ooda --since "2 min ago" | grep "ingested"
2026-05-09T17:32:07 [simard] OODA start: ingested 2 goal/backlog item(s) from meeting handoff
```

---

## Dashboard `/ws/chat` for Substantive Prompts

The dashboard exposes a WebSocket chat endpoint that routes prompts
through the same brain pipeline. This is the recommended way to ask
questions about current state (cycle count, recent actions, active
goals) without spawning a full meeting.

The dashboard listens on `SIMARD_DASHBOARD_PORT` (default `8080`,
overridable via `--dashboard-port=N` on `simard ooda daemon`). The
WebSocket route is `/ws/chat` (see
`src/operator_commands_dashboard/routes.rs`).

```javascript
// Browser console (dashboard tab open at the daemon's port)
const port  = location.port || '8080';
const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
const ws = new WebSocket(`${proto}//${location.hostname}:${port}/ws/chat`);
ws.onmessage = e => console.log(JSON.parse(e.data));
ws.onopen = () => ws.send(JSON.stringify({
  prompt: 'What is the current OODA cycle count and what action did Simard most recently dispatch?'
}));
```

The response is a substantive answer (≥ 100 chars, references real
state pulled from cognitive memory) and a fact recording the Q&A pair
is stored in cognitive memory automatically. Verify with:

```bash
simard memory search-facts "OODA cycle"
```

---

## Acting on decisions outside an OODA cycle

The CLI exposes `simard act-on-decisions` (see
`src/operator_cli/decisions.rs`). This is the offline complement to
OODA-loop ingestion: it loads the latest handoff via
`load_meeting_handoff(default_handoff_dir())`, calls
`gh issue create` for each `MeetingDecision` (titled `Decision: ...`)
and each `ActionItem` (titled `Action: ...`), then calls
`mark_meeting_handoff_processed` on success. Use this when the daemon
is stopped but you still want to surface a handoff to the GitHub
issue tracker by hand.

---

## See Also

- [`CONTRIBUTING.md`](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md) — PR evidence requirements
- [`docs/dashboard.md`](../dashboard.md) — dashboard overview
- [`docs/daemon-mode.md`](../daemon-mode.md) — OODA daemon overview
- [`docs/operations/cognitive-memory-durability.md`](cognitive-memory-durability.md)
  — the WAL incident that prompted the verification proposal
- [`docs/reference/meeting-close-lifecycle.md`](../reference/meeting-close-lifecycle.md)
  — timeout budgets, partial-handoff envelope, atomic writes
- [`docs/reference/meeting-handoff-schema.md`](../reference/meeting-handoff-schema.md)
  — full JSON schema and version history
- [`docs/reference/meeting-handoff-lifecycle.md`](../reference/meeting-handoff-lifecycle.md)
  — write gate, batch ingestion, and reaper API reference
- [`docs/reference/state-root-resolution.md`](../reference/state-root-resolution.md)
  — the env-var ladder shared by every Simard mode
- [`docs/howto/recover-from-meeting-close-timeout.md`](../howto/recover-from-meeting-close-timeout.md)
  — operator playbook when `handoff_partial=true` fires
- [`docs/howto/clean-stale-meeting-handoffs.md`](../howto/clean-stale-meeting-handoffs.md)
  — manual cleanup and reaper configuration
- `src/meeting_facilitator/handoff/mod.rs` — `MeetingHandoff` schema
  and helpers
- `src/ooda_loop/cycle.rs` — OODA-side ingestion
- `src/engineer_loop/meeting_decisions.rs` — engineer-loop-side ingestion
