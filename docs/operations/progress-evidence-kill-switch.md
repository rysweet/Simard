# Progress-evidence kill switch (`SIMARD_PROGRESS_EVIDENCE`)

This page documents the environment variable that disables the
progress-evidence gate at daemon boot. The gate itself is described
conceptually in
[Progress-evidence gating](../concepts/progress-evidence-gating.md) and
the API is documented in
[Progress-evidence API](../reference/progress-evidence-api.md).

> The gate is **enabled by default** on all production deployments. The
> kill switch exists for incident recovery and short-lived debugging
> sessions, not for steady-state operation.

---

## What this variable does

| Value | Behavior |
|---|---|
| Unset, or any value other than `off` (case-insensitive) | `OodaBridges.progress_evidence` is wired to `DefaultProgressEvidenceChecker` against the daemon's `repo_root` and `rysweet/Simard`. Progress increases require git evidence. |
| `off` (case-insensitive) | `OodaBridges.progress_evidence` is wired to `NoopProgressEvidenceChecker`. Every progress claim is accepted. **No `"goal progress accepted:"` or `"brain hallucination detected:"` audit episodes are emitted.** |

The variable is read once, at daemon startup, in the bridge-construction
path. Changing it during a daemon run has no effect — restart the daemon
to pick up a new value.

---

## When to use `off`

There are exactly three legitimate uses. If you find yourself reaching for
the kill switch outside of these, file a bug instead.

1. **`gh` outage or auth failure on the daemon host.** When
   `gh pr list` fails for an extended period, every increase attempt that
   would have matched rule (2) or (3) will be rejected. Setting `off`
   restores pre-#1967 behavior while you re-auth `gh` or wait for the
   outage to clear.
2. **Investigating a regression in the gate itself.** If you suspect the
   checker is rejecting a legitimate claim and you need a quick
   side-by-side comparison of the daemon's behavior with and without the
   gate, toggle the variable on a non-production daemon and re-run the
   cycle.
3. **Bisecting a daemon-level bug whose cause is unrelated to progress
   accounting.** Removing the gate eliminates one variable from the
   investigation. Restore it as soon as bisection completes.

---

## When NOT to use `off`

- **"To unblock production while #1967 is being fixed."** #1967 *is* the
  fix. The gate is the fix. Disabling it re-opens the meta-bug.
- **"Because the dashboard is showing too many hallucination alerts."**
  Each alert is evidence the brain is producing fictional progress —
  silencing the alerts does not change the underlying behavior.
- **"Because a goal seems stuck at the same percent."** Goals stay at the
  same percent because no engineer activity has occurred. The fix is to
  spawn an engineer that produces commits, not to disable the gate.
- **"To get a quick demo to look better."** The demo will lie. Use the
  dashboard operator override (`PUT /api/goals/<id>/progress`) instead —
  it bypasses the gate by design and is auditable as an operator action.

---

## How to set it

### One-shot for an interactive run

```bash
SIMARD_PROGRESS_EVIDENCE=off simard daemon
```

### Persistent across daemon restarts (systemd unit)

The Simard daemon ships with a reference unit file at
[`scripts/simard-ooda.service`](https://github.com/rysweet/Simard/blob/main/scripts/simard-ooda.service)
and is typically installed as a **user-level** unit at
`~/.config/systemd/user/simard-ooda.service`. Operators who install it
system-wide (`/etc/systemd/system/`) should drop the `--user` flag from
every command below.

Add the override to the unit's `[Service]` section:

```ini
[Service]
Environment="SIMARD_PROGRESS_EVIDENCE=off"
```

Then reload and restart:

```bash
systemctl --user daemon-reload
systemctl --user restart simard-ooda
```

To remove the override, delete the `Environment=` line, `daemon-reload`,
and restart.

For system-level installs, prefer `systemctl edit simard-ooda` (with
`sudo`) so the override lands in `/etc/systemd/system/simard-ooda.service.d/override.conf`
rather than being merged into the upstream unit file:

```bash
sudo systemctl edit simard-ooda
# add the same [Service] / Environment= snippet
sudo systemctl daemon-reload
sudo systemctl restart simard-ooda
```

---

## Verifying which mode the daemon is running in

The daemon logs the active checker at boot. The format is pinned by
[design §4.1](../concepts/progress-evidence-gating.md) — the
`progress-evidence:` substring and the `enabled` / `DISABLED` words
are stable; the parenthetical detail may evolve.

```
[simard] progress-evidence: enabled (DefaultProgressEvidenceChecker, repo_root=/home/azureuser/src/Simard, remote=rysweet/Simard)
```

Or:

```
[simard] progress-evidence: DISABLED (NoopProgressEvidenceChecker — SIMARD_PROGRESS_EVIDENCE=off)
```

Grep the daemon log to confirm (drop `--user` for a system-level install):

```bash
journalctl --user -u simard-ooda -n 200 | grep 'progress-evidence:'
```

You can also probe live behavior by inspecting cognitive memory: when the
gate is enabled there will be at least one episode beginning with
`"goal progress accepted:"` or `"brain hallucination detected:"` per cycle
in which a progress claim was made. When the gate is disabled, zero such
episodes are emitted.

```bash
curl -s -X POST http://localhost:8080/api/memory/search \
  -H 'Content-Type: application/json' \
  -d '{"query":"brain hallucination detected"}' | jq '.results'

curl -s -X POST http://localhost:8080/api/memory/search \
  -H 'Content-Type: application/json' \
  -d '{"query":"goal progress accepted"}' | jq '.results'
```

---

## Failure modes the kill switch addresses

Reject reason-string substrings are pinned by the runner error contract
documented in design §2.7. Triage by `grep`'ing the
`"brain hallucination detected:"` episode body for the substring in the
left column:

| Substring in `reason` | Likely cause | Remedy |
|---|---|---|
| `gh: command not found` | `gh` is not installed or not on the daemon's `PATH`. | Install `gh`; remove the kill switch. |
| `gh: authentication required` | The daemon's `gh` token expired or was revoked. | `gh auth login` as the daemon user; remove the kill switch. |
| `git: not a git repository` | `repo_root` resolves to a non-repo path (e.g. daemon launched from `/`). | Launch the daemon from the repo root, or override via the daemon-boot wiring. |
| `git: io error` / `gh: io error` | Catch-all process-spawn failure (broken pipe, ENOSPC, etc.). | Inspect the trailing `<detail>` portion of the reason string. |
| Genuine engineer commits exist but the gate still rejects | Branch name does not match `engineer/<slug>-*` pattern. | File a bug. Do not paper over with the kill switch. |

---

## Removing the kill switch

When the underlying issue is resolved, remove the environment variable
and restart the daemon. Confirm via the boot log line above that
`DefaultProgressEvidenceChecker` is active. The next cycle that involves
a progress claim should produce either an `Accept` or a `Reject` audit
episode in cognitive memory.

---

## Related

- [Progress-evidence gating (concept)](../concepts/progress-evidence-gating.md)
- [Progress-evidence API (reference)](../reference/progress-evidence-api.md)
- [Diagnose rejected progress claims (how-to)](../howto/diagnose-rejected-progress-claims.md)
- [Cognitive memory durability](cognitive-memory-durability.md)
