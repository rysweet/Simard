---
title: String truncation helpers
description: Reference for truncate_to_char_boundary — the char-boundary-safe helper that replaces String::truncate(N) at every site where N is a byte budget rather than a code-point count.
last_updated: 2026-05-09
owner: simard
doc_type: reference
related:
  - ./meeting-backend-api.md
  - ./terminal-session-idle-detection.md
---

# String truncation helpers

`src/util/string_truncate.rs` exports a single helper used wherever a
`String` must fit inside a byte budget (evidence buffers, transcript
previews, log lines):

```rust
pub fn truncate_to_char_boundary(s: &mut String, max_bytes: usize)
```

## Why this exists

`String::truncate(new_len)` panics if `new_len` is not a UTF-8 character
boundary:

```text
thread 'tokio-rt-worker' panicked at src/terminal_session/evidence.rs:10:
assertion failed: self.is_char_boundary(new_len)
```

This regression surfaced from the live daemon when a chat-tab message
contained an em-dash (`—`, three UTF-8 bytes) at byte offset 512. Before
this helper landed, three call sites called `normalized.truncate(512)`
directly:

- `src/terminal_session/evidence.rs:10` — `transcript_preview`
- `src/terminal_session/evidence.rs:109` — `compact_terminal_evidence_value`
- `src/copilot_task_submit/transcript.rs:350` — visible-fragment join

Any of them could panic the runtime worker on multi-byte input crossing
the budget. `truncate_to_char_boundary` provides a stable-Rust replacement
that is safe for any UTF-8 string at any byte budget.

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

## Use this everywhere a byte budget is enforced

Every place that previously chained
`if normalized.len() > N { normalized.truncate(N); normalized.push_str("...");
}` now calls:

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

`src/util/string_truncate.rs` ships unit tests covering:

- ASCII shorter than budget — no-op.
- ASCII at exactly the budget — no-op.
- ASCII longer than budget — truncate at the boundary, no panic.
- Em-dash (`—`, 3 bytes) straddling the budget — backs up to the previous
  char boundary and produces valid UTF-8.
- CJK characters (3 bytes each) at the budget — same.
- 4-byte emoji (`🎉`) at the budget — same.
- Empty string — no-op.
- `max_bytes = 0` — truncates to empty without panic.

## Related reading

- [Terminal session idle detection](./terminal-session-idle-detection.md) —
  one of the consumers via `compact_terminal_evidence_value`.
- [Meeting backend API](./meeting-backend-api.md) — the chat WebSocket path
  that surfaced the original UTF-8 panic.
