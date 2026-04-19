# Azlin Hosts: Self-Membership Indicator

The Azlin Hosts dashboard panel marks the local daemon's host as **joined**
when it appears in the cluster's membership set. This makes it visually
obvious that the node currently serving the dashboard is itself a member of
the cluster it is reporting on.

## Overview

A host is considered a "cluster member" if it appears in either:

1. The configured hosts file (`~/.simard/hosts.json`), or
2. The discovered VM list returned by `azlin list`.

When the local hostname matches an entry in either list, that entry is
flagged as `is_local: true` in the API response, and the dashboard renders
a green `joined` badge next to it.

> **Note:** The `is_local` flag is a UI hint only. It MUST NOT be used for
> authorization decisions or trust boundaries.

## API

### `GET /api/azlin/hosts`

Returns the union of configured and discovered hosts, with self-membership
annotations.

**Response shape:**

```json
{
  "local_hostname": "node-01",
  "configured": [
    {
      "hostname": "node-01.example.internal",
      "user": "azureuser",
      "is_local": true
    },
    {
      "hostname": "node-02.example.internal",
      "user": "azureuser",
      "is_local": false
    }
  ],
  "discovered": [
    {
      "name": "node-01",
      "ip": "10.0.0.4",
      "is_local": true
    },
    {
      "name": "node-03",
      "ip": "10.0.0.6",
      "is_local": false
    }
  ]
}
```

**Fields added by this feature:**

| Field                    | Type    | Description                                                 |
| ------------------------ | ------- | ----------------------------------------------------------- |
| `local_hostname`         | string  | Short hostname of the daemon host (from `/etc/hostname`).   |
| `configured[].is_local`  | boolean | True if the entry's `hostname` matches the local hostname.  |
| `discovered[].is_local`  | boolean | True if the entry's `name` matches the local hostname.      |

The fields are always present. If the local hostname cannot be determined,
`local_hostname` is `"unknown"` and no entry will be flagged `is_local: true`
(empty inputs never match).

## Match Semantics

Self-membership is decided by the pure helper `is_local_host(local, name)`:

- **Case-insensitive** comparison.
- **Short-form normalization:** Everything from the first `.` onward is
  stripped on both sides before comparison. So `node-01` matches
  `node-01.example.internal` and vice versa.
- **Empty inputs never match.** If either side is empty, the result is
  `false`.

**Examples:**

| Local        | Entry                          | Match? |
| ------------ | ------------------------------ | ------ |
| `node-01`    | `node-01`                      | ✅     |
| `NODE-01`    | `node-01`                      | ✅     |
| `node-01`    | `node-01.example.internal`     | ✅     |
| `node-01.x`  | `node-01.y`                    | ✅     |
| `node-01`    | `node-02`                      | ❌     |
| `""`         | `node-01`                      | ❌     |
| `node-01`    | `""`                           | ❌     |

> Different FQDN suffixes still match because both reduce to the same short
> form (e.g. `node-01.x` and `node-01.y` both normalize to `node-01`). This
> is intentional — short-form equality is the contract.

## UI Behavior

In the **Azlin Hosts** panel, each row that corresponds to the local daemon
host renders an additional badge:

```html
<span class="ok">joined</span>
```

The badge uses the existing `.ok` style (green) for visual consistency with
other "healthy" status indicators in the dashboard. Non-local rows render
unchanged.

## Configuration

No configuration is required. The feature activates automatically as soon
as the daemon can read its hostname from `/etc/hostname`. If `/etc/hostname`
is unreadable, no entries are flagged and no `joined` badges render — the
panel functions normally otherwise.

If you want to ensure the local node always appears as configured (rather
than only discovered), add it to `~/.simard/hosts.json`:

```json
{
  "hosts": [
    { "hostname": "node-01", "user": "azureuser" }
  ]
}
```

## Troubleshooting

**The local host is not marked as `joined`.**

1. Check the dashboard response (default port `8080`):
   `curl -s http://localhost:8080/api/azlin/hosts | jq .local_hostname`.
   If this returns `"unknown"`, the daemon could not read `/etc/hostname`.
   See [`docs/howto/run-ooda-daemon.md`](../howto/run-ooda-daemon.md) for
   the actual port your daemon is bound to.
2. Confirm the local hostname appears in either `~/.simard/hosts.json` or
   `azlin list`. If neither contains it, the host is not a cluster member
   and the badge will (correctly) not appear.
3. Verify short-form names align. `node-01.foo.bar` and `node-01.baz.qux`
   match (both short to `node-01`), but `node01` and `node-01` do not.
4. **Discovered name vs. OS hostname mismatch.** The `discovered[].name`
   field comes from Azure metadata and may be the VM resource name rather
   than the OS hostname in `/etc/hostname`. If they differ, the
   `discovered` entry will not be flagged `is_local`, even though the same
   node may also appear (correctly flagged) in `configured`. This is
   expected — add the node to `~/.simard/hosts.json` using its OS hostname
   to get a flagged entry.

**The badge appears next to the wrong host.**

This indicates two hosts share the same short hostname. Rename one of them
in `/etc/hostname` and restart the daemon, or use distinct short names.

## Security Considerations

- `is_local` is a **UI hint only**. Never use it for access control,
  authorization, or any trust-establishing decision.
- The endpoint inherits the dashboard's existing authentication; no new
  attack surface is introduced.
- Hostname strings are not interpolated into HTML — only the static
  `joined` badge literal is rendered conditionally, so there is no XSS
  vector from malformed hostnames.
- Empty/missing hostnames never match, preventing accidental "everything
  is local" failure modes.

## See Also

- [`docs/reference/dashboard-e2e-tests.md`](dashboard-e2e-tests.md)
- [`docs/howto/run-ooda-daemon.md`](../howto/run-ooda-daemon.md)
