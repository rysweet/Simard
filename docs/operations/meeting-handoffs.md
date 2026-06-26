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
`src/meeting_repl/repl.rs` and `src/meeting_backend/`). Commands are grouped
below by purpose; everything that is **not** a recognized command is treated
as natural conversation with the bound LLM agent. The command set is the
single source of truth in `src/meeting_backend/command.rs` (`HELP_GROUPS`),
shared by the CLI REPL and the dashboard chat.

**Meeting control**

| Command | Effect |
|---|---|
| `/help` | Show the grouped, colorized command list |
| `/close` (alias `/done`) | Finalize and write `meeting_handoff.json`; exit the REPL |
| `/status` | Show session info (topic, started_at, current decision/action counts) |
| `/export` | Export the meeting transcript as markdown |
| `/recap` | Color-coded session recap |
| `/preview` | Preview the handoff artifact before closing |
| `/state` | Show current decisions, questions, actions, risks, disagreements |

**Capture**

| Command | Effect |
|---|---|
| `/decision <text> [--rationale <why>]` | Record a decision (optional rationale) |
| `/action <text>` | Record an action item (assignee/deadline parsed inline) |
| `/question <text>` | Record an open question |
| `/risk <text>` | Record an identified risk |
| `/disagree <text>` | Record a disagreement or dissenting view |
| `/theme <text>` | Record a theme for this meeting |
| `/owner <name>` | Name the next agent/persona/human to action this handoff |
| `/goal <text>` | Set the meeting's overarching objective |

**Templates**

| Command | Effect |
|---|---|
| `/template` | List meeting templates |
| `/template <name>` | Apply a template (`standup`, `1on1`, `retro`, `planning`) |

Any other text is sent to the brain as a prompt and the response is printed
inline.

### Grouped, colorized `/help` and unknown-command suggestions (#2321)

`/help` renders the three groups above as titled, color-coded sections (the
titles honor `NO_COLOR` via `src/meeting_repl/color.rs`, matching how `/recap`
and `/state` colorize their headers).

If you type a single slash token that looks like a command but matches none
(e.g. a typo), the REPL **does not** forward it to the LLM. Instead it prints a
"did you mean?" hint, suggesting the closest known command within Levenshtein
distance 2:

```text
simard:meeting> /colse
Unknown command '/colse'. Did you mean '/close'?
```

When nothing is close enough, it lists the available commands instead:

```text
simard:meeting> /zzzz
Unknown command '/zzzz'. Type /help for the full list.

── Meeting control ──
  ...
```

This affordance is deliberately narrow so normal input is never hijacked:

- Multi-token slash lines (`/foo some text`) stay conversation.
- File paths and markdown that start with `/` (e.g. `/home/user/notes.md`,
  `/etc/hosts`) stay conversation — only a `/` followed solely by ASCII
  letters is treated as a command attempt.
- Bare single-component absolute paths that are well-known filesystem roots
  (`/home`, `/tmp`, `/var`, …, the FHS top-level directories) stay
  conversation, so a path like `/home` is never mistaken for a `/done` typo.
- Bare, argument-requiring commands typed without a payload (`/decision`,
  `/theme`, …) still fall through to conversation, unchanged.
- Only *argument-less* single slash-tokens are treated as command attempts.
  A typo that carries a payload (e.g. `/decison Adopt TDD`) is left as
  conversation rather than suggested — distinguishing a typo+payload from an
  intentional `/word args` line is ambiguous and could hijack real input.

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

### Write guard for empty handoffs (#2268)

As of issue #2268, the handoff queue file is **not written** to
`meeting_handoffs/` when both `decisions` and `action_items` are empty.
The per-meeting bundle under `~/.simard/meetings/<id>/` is still
written regardless — it serves as the durable meeting record.

This prevents empty handoffs (from casual chats, interrupted meetings,
or dashboard close events with no extracted decisions) from
accumulating in the processing queue. The OODA daemon and engineer
loop no longer need to spend cycles loading, parsing, and marking
empty files.

The write guard checks `facilitator_decisions.is_empty() &&
facilitator_actions.is_empty()` in `write_handoff_with_explicit`
(after the `MeetingHandoff` struct is fully built but before the write
to the handoff queue directory).

### Required fields (content-bearing handoffs)

For handoffs that pass the write guard (at least one decision or one
action item), the `processed` flag must be `false` for a freshly
written handoff; ingestion flips it to `true` via
`mark_meeting_handoff_processed`.

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

## Ready-to-file issue stubs (`issues/`) — #2309

On `/close`, the bundle writer renders each `MeetingDecision` and each
`ActionItem` into a reviewable GitHub issue **draft** under
`~/.simard/meetings/<meeting_id>/issues/`:

```
~/.simard/meetings/<meeting_id>/
├── meeting_handoff.json   # canonical structured artifact
├── meeting_handoff.md     # human-readable report (now links the stubs)
├── transcript.json        # verbatim conversation
└── issues/                # NEW (#2309) — one stub per decision / action item
    ├── 01-adopt-structured-handoff-bundles.md
    └── 02-wire-bundle-writer-into-close-flow.md
```

Stubs are the **reviewable middle ground** between re-typing decisions by hand
and `simard act-on-decisions`, which auto-files issues immediately with no
review step. Each stub is a ready-to-edit issue draft you file when (and if)
you want to:

```bash
cd ~/.simard/meetings/<meeting_id>/issues
gh issue create --title "$(head -1 01-*.md | sed 's/^# //')" --body-file 01-*.md
```

### What each stub contains

- A clear title: `Action: <description>` or `Decision: <description>`.
- **Context** — the decision/action text, plus a **Rationale** subsection for
  decisions.
- **Details** — owner/decided-by, priority, due date, and the meeting goal.
- A starter **acceptance-criteria** checklist (`- [ ]` items) to refine before
  filing.
- **Provenance** — meeting id, topic, and closed-at timestamp.

### Behavior and guarantees

| Property | Behavior |
|---|---|
| Empty handoff | **No `issues/` directory** is created — mirrors the empty-handoff write guard ([#2268](#write-guard-for-empty-handoffs-2268)). |
| Ordering | Decisions first, then action items, numbered `01`, `02`, … |
| Filenames | Filesystem-safe slug (`[a-z0-9-]`, length-capped); the `NN-` prefix guarantees uniqueness. No path traversal — non-alphanumerics are collapsed to `-`, so `/`, `\`, and `.` can never appear. |
| Idempotent | Regeneration clears stale `*.md` stubs first, so re-closing (or a partial-then-full close) never leaves orphaned stubs behind. |
| Permissions | Each stub is written `0o600` on unix (operator-private), matching the other bundle files. |

The bundle's `meeting_handoff.md` gains an **`## Issue stubs`** section listing
the generated files with relative links (`issues/NN-slug.md`) so operators can
find them from the report.

> **Scope note**: issue stubs are a *handoff-enrichment* artifact only. They are
> **not** filed automatically and do **not** affect OODA or engineer-loop
> ingestion (which read `meeting_handoff.json`, not the stubs). Filing remains a
> deliberate operator action via `gh` or `simard act-on-decisions`.

The renderer lives in
`src/meeting_facilitator/handoff/issue_stubs.rs`
(`plan_issue_stubs` is a pure renderer; `write_issue_stubs` performs the I/O)
and is wired into the single bundle chokepoint
`write_meeting_bundle` so every close path emits stubs.

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
   &handoff_dir, &state_root)`, which processes **up to 10 handoffs
   per cycle** in FIFO order (oldest unprocessed first). For each:
   - Loads the handoff via `find_oldest_unprocessed_handoff` and
     `serde_json::from_str`.
   - Skips it if `processed == true`.
   - **Fast-marks empty handoffs:** If both `decisions` and
     `action_items` are empty, marks the handoff processed immediately
     and continues to the next file without creating goals. This
     drains legacy empty handoffs from pre-#2268 builds.
   - For content-bearing handoffs: converts each `MeetingDecision`
     into one or more goals and each `ActionItem` into a backlog item
     on the live `active_goals` board.
   - Calls `mark_handoff_processed_in_place` to flip `processed` to
     `true` so the same handoff is not re-ingested.
   - Stops after 10 handoffs or when no unprocessed handoffs remain.
3. Logs:

   ```
   [simard] OODA start: ingested N goal/backlog item(s) from meeting handoff
   ```

   on success (logged via `eprintln!`, captured by the systemd
   journal). The count reflects only content-bearing handoffs; empty
   handoffs that were fast-marked do not contribute. On error:

   ```
   [simard] OODA start: meeting handoff check failed: <error>
   ```

### Batch processing rationale (#2268)

Prior to issue #2268, the daemon processed exactly one handoff per
cycle. This created a bottleneck: if meetings were closed faster than
the OODA cycle interval (default 60s), handoff files would accumulate
indefinitely. Combined with empty handoffs from dashboard chats and
interrupted meetings, directories of 100+ files were observed in
production.

The batch cap of 10 balances two concerns:
- **Drain speed:** A backlog of 100 files clears in 10 cycles
  (~10 minutes at the default interval).
- **Cycle budget:** Processing 10 handoffs adds negligible overhead to
  a cycle that already runs disk health checks, memory consolidation,
  and agent dispatch.

See [Handoff lifecycle API](../reference/handoff-lifecycle-api.md) for
the full API reference.

### Verifying ingestion

```bash
journalctl -u simard-ooda --since "5 min ago" \
  | grep "ingested .* meeting handoff"
```

If you do not see this line within ~1 cycle of `/close`, check:

- The daemon is running (`systemctl status simard-ooda`).
- `SIMARD_HANDOFF_DIR` matches what `simard meeting repl` wrote (or
  both rely on the default).
- The handoff JSON has at least one decision or action item. (Empty
  handoffs are not written to the queue as of #2268; if the meeting
  produced no decisions, no handoff file exists.)
- `processed` is still `false` (a previous OODA cycle may already have
  ingested it).

### Automatic reaping of old handoffs (#2268)

Processed handoff files are automatically deleted after 7 days. The
`reap_old_handoffs` function runs during the resource cleanup phase of
each OODA cycle (after `handle_cleanup` in `cycle.rs`). Only files
matching `handoff-*.json` with `processed == true` and filesystem mtime
older than 7 days are deleted.

This complements the disk health recipe (which handles larger cleanup
targets like worktrees and cargo caches) with targeted cleanup for the
handoff directory. See
[Diagnose handoff accumulation](../howto/diagnose-handoff-accumulation.md)
for the operator guide.

---

## Engineer-loop ingestion

The engineer loop also scans the handoff directory on startup
(`src/engineer_loop/meeting_decisions.rs:24`). It uses
`find_oldest_unprocessed_handoff` to process **one** handoff at a time
(not the batch `check_meeting_handoffs` path used by the OODA daemon).
This means an operator can issue intent through `simard meeting repl`
and have an in-flight engineer pick it up at its next loop iteration
without waiting for the OODA daemon to cycle. Because both consumers
use FIFO ordering and the atomic `processed` flag, a single handoff is
ingested by whichever consumer reaches it first — there is no
double-processing.

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

For a **reviewable** alternative that does *not* file anything
automatically, see the per-meeting
[issue stubs](#ready-to-file-issue-stubs-issues--2309) written into the
bundle's `issues/` directory on `/close`.

---

## See Also

- [`CONTRIBUTING.md`](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md) — PR evidence requirements
- [`docs/dashboard.md`](../dashboard.md) — dashboard overview
- [`docs/daemon-mode.md`](../daemon-mode.md) — OODA daemon overview
- [`docs/operations/cognitive-memory-durability.md`](cognitive-memory-durability.md)
  — the WAL incident that prompted the verification proposal
- [`docs/reference/meeting-close-lifecycle.md`](../reference/meeting-close-lifecycle.md)
  — timeout budgets, partial-handoff envelope, atomic writes
- [`docs/reference/state-root-resolution.md`](../reference/state-root-resolution.md)
  — the env-var ladder shared by every Simard mode
- [`docs/reference/handoff-lifecycle-api.md`](../reference/handoff-lifecycle-api.md)
  — write guard, batch processing, and reaping APIs (#2268)
- [`docs/howto/diagnose-handoff-accumulation.md`](../howto/diagnose-handoff-accumulation.md)
  — operator guide for handoff accumulation (#2268)
- [`docs/howto/recover-from-meeting-close-timeout.md`](../howto/recover-from-meeting-close-timeout.md)
  — operator playbook when `handoff_partial=true` fires
- `src/meeting_facilitator/handoff/mod.rs` — `MeetingHandoff` schema
  and helpers
- `src/meeting_facilitator/handoff/issue_stubs.rs` — issue-stub renderer
  (`plan_issue_stubs` / `write_issue_stubs`, #2309)
- `src/ooda_loop/cycle.rs` — OODA-side ingestion
- `src/ooda_loop/curate.rs` — batch processing and reaping
- `src/engineer_loop/meeting_decisions.rs` — engineer-loop-side ingestion
