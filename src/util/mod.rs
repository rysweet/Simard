//! Cross-cutting utility modules.
//!
//! Currently exports:
//!
//! - [`string_truncate`] — a stable-Rust char-boundary-safe replacement for
//!   `String::truncate(N)` at every site where `N` is a byte budget rather
//!   than a code-point count. See
//!   `docs/reference/string-truncation-helpers.md`.
//! - [`durable_append`] — the single, loss-free source of truth for
//!   `O_APPEND` JSONL writes shared by the cost ledger and self-metrics
//!   stream. See `docs/reference/durable-append-api.md`.
//! - [`spawn_retry`] — bounded-backoff retry for transient fork/exec failures
//!   (`ETXTBSY`/`EAGAIN`/`ENOMEM`) hit when spawning subprocesses under high
//!   host concurrency. See `docs/reference/spawn-retry-api.md`.

pub mod durable_append;
pub mod spawn_retry;
pub mod string_truncate;
