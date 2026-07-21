---
title: WSS-connect log token redaction (rysweet/azlin)
description: Reference for the WSS-connect log redaction in the rysweet/azlin repository, where the connect-path warn! log statement emitted the full wss_url — embedding the websocket_token — and leaked the secret into logs. The URL is now redacted before logging: only scheme+host+path is retained and the websocket_token query value is replaced with a redaction marker, applied to the warn! call and every adjacent log line on the connect path. Additive and non-breaking — the websocket connection itself is unchanged. A regression test asserts the emitted log field contains neither the raw wss_url nor the websocket_token. This feature lives in the separate rysweet/azlin checkout, not in the Simard tree.
last_updated: 2026-07-21
owner: simard
doc_type: reference
status: reference
related:
  - ../ecosystem-map.md
  - ../reference/azlin-hosts-self-membership.md
  - ../operator-dashboard/azlin-tmux-sessions.md
  - ../reference/rpc-wire-protocol.md
---

# WSS-connect log token redaction (rysweet/azlin)

> **Issue [rysweet/azlin#1056](https://github.com/rysweet/azlin/issues/1056).**
> The WSS-connect path's `warn!` log statement emitted the **full `wss_url`**,
> which embeds the `websocket_token`, leaking the secret into logs. The URL is
> now **redacted** before it reaches any log sink.

> **Cross-repo note.** This is a **credential-protection fix in the separate
> `rysweet/azlin` repository**, not in the Simard tree. `azlin` is an ecosystem
> repo Simard stewards (see the [ecosystem map](../ecosystem-map.md)); the fix is
> implemented and shipped in the `rysweet/azlin` checkout. This page is the
> ecosystem-level reference for the redaction contract so the behavior is
> documented alongside Simard's other credential-hygiene guarantees.

When `azlin` opens its WebSocket-Secure (WSS) connection, the connect path built
a `wss_url` of the form:

```text
wss://gateway.example.com/ws?websocket_token=SUPER_SECRET_TOKEN_VALUE
```

and logged it at `warn!` level on connect (plus on adjacent diagnostic lines).
Because the `websocket_token` is a query parameter of that URL, every such log
line wrote the live secret to disk, to the terminal, and to any log aggregator —
a durable credential leak.

---

## The fix: `redact_wss_url`

A single redaction helper is applied to **every** log site on the WSS-connect
path that would otherwise echo the URL. It preserves the diagnostic value of the
message (scheme, host, path) while removing the secret:

```rust
/// Redact the `websocket_token` (and any other sensitive query value) from a
/// `wss_url` before it is logged. Retains scheme + host + path for diagnostics;
/// replaces the token query value with a fixed redaction marker.
fn redact_wss_url(wss_url: &str) -> String;
```

### Redaction contract

| Input component | In redacted output |
|-----------------|--------------------|
| Scheme (`wss://`) | Retained. |
| Host (`gateway.example.com`) | Retained. |
| Path (`/ws`) | Retained. |
| `websocket_token=<value>` query | Value replaced with a marker, e.g. `websocket_token=REDACTED`. |
| Any adjacent sensitive query value | Redacted with the same marker. |

Example:

```text
in:  wss://gateway.example.com/ws?websocket_token=SUPER_SECRET_TOKEN_VALUE
out: wss://gateway.example.com/ws?websocket_token=REDACTED
```

The connect-path `warn!` call and every adjacent log line that echoed the same
URL now log `redact_wss_url(&wss_url)` instead of the raw `wss_url`.

---

## Invariants

- **No secret at any log level.** Neither the `websocket_token` value nor the
  raw `wss_url` appears in any emitted log record on the connect path —
  `trace!`/`debug!`/`info!`/`warn!`/`error!` alike.
- **Diagnostics preserved.** Operators can still see *which endpoint* the client
  connected to (scheme + host + path); only the credential is masked.
- **No `Debug` leak.** The token type does not derive a `Debug` impl that would
  print its value, so a stray `{:?}` cannot re-expose it.
- **Connection behavior unchanged.** Redaction touches only the **logged**
  string; the actual URL used to open the socket is unaffected. The change is
  additive and non-breaking.

---

## Regression test

A regression test on the WSS-connect path asserts the **negative property**:

- capture the emitted log record for a connect attempt with a known
  `websocket_token`;
- assert the record contains **neither** the `websocket_token` value **nor** the
  raw `wss_url`;
- assert it **does** still contain the host/path so the message stays useful.

---

## Security considerations

- **Core credential-protection fix.** Treats the `websocket_token` as a secret
  that must never be logged; the redaction is applied at every log site on the
  path, not just the one `warn!` that triggered the report.
- **Fixed marker, no partial disclosure.** The token value is replaced wholesale
  with a marker — no prefix/suffix of the secret is retained.
- **Structured `tracing` + OTel only.** No `print!`/`println!` is used; the
  redacted URL flows through structured fields so downstream sinks also receive
  the masked form.

---

## Related reading

- [Ecosystem map](../ecosystem-map.md) — the repos under `~/src/` Simard
  stewards, including `azlin`.
- [azlin hosts self-membership reference](./azlin-hosts-self-membership.md)
  — related `azlin` operational contract.
- [RPC wire protocol reference](./rpc-wire-protocol.md) — the broader
  transport/logging-hygiene conventions this redaction follows.
