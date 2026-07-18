---
title: "E2BIG errno → signal alignment (#4289)"
description: >
  How the signal/errno parser aligns the E2BIG (errno 7, "argument list too
  long") case consistently across classify_spawn_cause, classify_cause, and the
  emitted Signal::StepFailureDiagnosed, so the diagnosed FailureCause matches the
  authoritative POSIX signal/errno definition on every path.
last_updated: 2026-07-18
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/e2big-elimination.md
  - ./terminal-failure-diagnosis-api.md
  - ./overseer-signal-jsonrpc-transport.md
  - ../howto/diagnose-and-recover-ooda-step-failures.md
  - ../../src/overseer/diagnosis.rs
  - ../../src/overseer/signal.rs
---

# E2BIG errno → signal alignment (#4289)

> **Status: implemented (mapping) + regression-guarded.** The errno-7 →
> `FailureCause::ArgListTooLong` mapping is **already consistent** across
> `classify_spawn_cause` (`diagnosis.rs:144`), `classify_cause`
> (`diagnosis.rs:171`), and the `Signal::StepFailureDiagnosed` payload (which
> carries the diagnosed `FailureCause` verbatim) in
> [`src/overseer/diagnosis.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/diagnosis.rs)
> and
> [`src/overseer/signal.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/signal.rs).
> The deliverable for [#4289](https://github.com/rysweet/Simard/issues/4289) is a
> **regression-guard test** that pins this consistency so a future refactor cannot
> silently reintroduce a divergence.

## Why this needs guarding

E2BIG (`errno 7`, "argument list too long") is diagnosed on more than one path: a
spawn failure with no child (`classify_spawn_cause`) and an exit-code/transcript
failure (`classify_cause`). Both must agree, and the
`Signal::StepFailureDiagnosed` payload the Overseer receives must carry the same
authoritative cause. If any of these three sites drifted, the Overseer could act
on a "not executable" reading instead of the real E2BIG root cause. The mapping is
currently correct on all three sites; the risk is **regression**, which the guard
test below prevents.

## The authoritative definition

E2BIG is `errno 7`. The OS errno is the **authoritative** signal; a message-string
match is only a fallback for platforms/wrappers that surface no numeric errno.
All three sites now agree on this ordering.

### `classify_spawn_cause` (spawn-time, no child)

A spawn failure has no child process, so there is no exit code. The classifier
keys off the raw OS errno first, then falls back to the message marker:

```rust
fn classify_spawn_cause(err: &std::io::Error) -> FailureCause {
    if let Some(errno) = err.raw_os_error() {
        match errno {
            7  => return FailureCause::ArgListTooLong, // E2BIG — authoritative
            28 => return FailureCause::DiskFull,        // ENOSPC
            12 => return FailureCause::OutOfMemory,     // ENOMEM
            _  => {}
        }
    }
    let lower = err.to_string().to_ascii_lowercase();
    if lower.contains("argument list too long") || lower.contains("e2big") {
        return FailureCause::ArgListTooLong; // fallback marker
    }
    FailureCause::Unknown // structured, never a silent drop
}
```

### `classify_cause` (exit-code + transcript)

When the child ran and left a transcript, the arg-list marker wins over a bare
exit-126 "not executable" reading, so an E2BIG surfaced by the shell diagnoses as
`ArgListTooLong`, not a misleading "not executable":

```rust
if has("argument list too long") || has("e2big") {
    return FailureCause::ArgListTooLong;
}
```

### `Signal::StepFailureDiagnosed`

The emitted signal carries the diagnosed `FailureCause` unchanged, so the E2BIG
mapping the classifiers produce is exactly what the Overseer receives — the
signal's `stable_key` / display for the arg-list-too-long case maps to the E2BIG
fix, never to a generic exec error.

## Guarantees

- **Consistency:** all three sites map errno 7 → `FailureCause::ArgListTooLong`.
- **No silent drop:** an unmapped errno classifies as `FailureCause::Unknown`
  with the same bounded evidence, so the Overseer always receives *something*
  structured to act on.
- **Errno-first:** the numeric errno is authoritative; the message marker is a
  fallback only.

Evidence strings are treated as **data only** — logged and carried in the signal,
never re-executed.

## Tests (regression guard)

The regression-guard unit test in
[`src/overseer/tests_diagnosis.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/tests_diagnosis.rs)
pins the E2BIG parse path so the three-site consistency cannot silently
regress: an `io::Error` with `raw_os_error() == Some(7)` classifies as
`ArgListTooLong` via `classify_spawn_cause`, a transcript carrying
"argument list too long" classifies the same via `classify_cause`, and the
resulting `Signal::StepFailureDiagnosed` carries the aligned cause.

## See also

- [Comprehensive E2BIG elimination](../concepts/e2big-elimination.md)
- [Terminal-failure diagnosis API](./terminal-failure-diagnosis-api.md)
- [Diagnose and recover OODA step failures](../howto/diagnose-and-recover-ooda-step-failures.md)
