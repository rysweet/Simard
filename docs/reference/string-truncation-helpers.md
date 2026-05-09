---
title: String truncation helpers
description: Design reference for truncate_to_char_boundary — a planned char-boundary-safe helper that will replace String::truncate(N) at every site where N is a byte budget rather than a code-point count.
last_updated: 2026-05-09
owner: simard
doc_type: reference
related:
  - ./meeting-backend-api.md
  - ./terminal-session-idle-detection.md
---

# String truncation helpers

> **Status: design — not yet implemented.** This document describes a
> helper module that the issue
> [#1590](https://github.com/rysweet/Simard/issues/1590) follow-up
> regression-fix work will introduce at `src/util/string_truncate.rs`. The
> three call sites listed in [Why this exists](#why-this-exists) currently
> still call `String::truncate` directly. Update this document to drop the
> "design" banner and the "planned" qualifiers when the helper lands.

The planned `src/util/string_truncate.rs` module will export a single
helper used wherever a `String` must fit inside a byte budget (evidence
buffers, transcript previews, log lines):

```rust
pub fn truncate_to_char_boundary(s: &mut String, max_bytes: usize)
```

## Why this exists

`String::truncate(new_len)` panics if `new_len` is not a UTF-8 character
boundary — the standard library asserts `self.is_char_boundary(new_len)`
before truncating. Three call sites in Simard currently call
`normalized.truncate(N)` with `N` as a byte budget rather than a
code-point count, so any input where a multi-byte sequence (em-dash, CJK,
emoji) crosses byte `N` will panic the runtime worker:

- `terminal_session::evidence::transcript_preview` — uses `512`.
- `terminal_session::evidence::compact_terminal_evidence_value` — uses a
  caller-supplied `limit`.
- `copilot_task_submit::transcript` (visible-fragment join) — uses `512`.

These sites are the regression target. `truncate_to_char_boundary`
provides a stable-Rust replacement that is safe for any UTF-8 string at
any byte budget; the three sites above will move to it as part of the
same follow-up commit that adds the helper.

## Contract

`truncate_to_char_boundary(s, max_bytes)`:

1. If `s.len() <= max_bytes`, returns without modifying `s`.
2. Otherwise finds the largest `i ≤ max_bytes` such that
   `s.is_char_boundary(i)` and calls `s.truncate(i)`.

Because byte 0 is always a valid char boundary, the loop always
terminates — even if `max_bytes` falls inside a multi-byte sequence the
helper backs up to the start of that sequence rather than panicking.

The helper is a thin wrapper over `String::truncate`:

```rust
pub fn truncate_to_char_boundary(s: &mut String, max_bytes: usize) {
    if s.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    s.truncate(boundary);
}
```

## Stability and toolchain

The implementation uses only stable-Rust APIs — `String::len`,
`str::is_char_boundary`, and `String::truncate`. It does **not** use the
nightly-only `floor_char_boundary`, so the helper compiles on the same
toolchain as the rest of the codebase.

## Usage convention

Every place that currently chains
`if normalized.len() > N { normalized.truncate(N); normalized.push_str("...");
}` will be rewritten to:

```rust
use crate::util::string_truncate::truncate_to_char_boundary;

if normalized.len() > N {
    truncate_to_char_boundary(&mut normalized, N);
    normalized.push_str("...");
}
```

The ellipsis push remains the caller's responsibility because some sites
want a different sentinel (`"…"`, `"[truncated]"`, `""`).

## Tests

The helper module ships with unit tests covering at minimum:

- ASCII shorter than budget — no-op.
- ASCII at exactly the budget — no-op.
- ASCII longer than budget — truncate at the boundary, no panic.
- Em-dash (`—`, 3 bytes) straddling the budget — backs up to the
  previous char boundary and produces valid UTF-8.
- CJK characters (3 bytes each) straddling the budget — same.
- 4-byte emoji (`🎉`) straddling the budget — same.
- Empty string — no-op.
- `max_bytes = 0` — truncates to empty without panic.

These cases exercise every branch of the helper plus the three byte-width
classes (2-byte Latin-1 supplement, 3-byte BMP, 4-byte SMP) that real
input contains.

## Related reading

- [Terminal session idle detection](./terminal-session-idle-detection.md)
  — one of the consumers via `compact_terminal_evidence_value`.
- [Meeting backend API](./meeting-backend-api.md) — the chat WebSocket
  path that surfaces evidence text into the chat preview pipeline.
