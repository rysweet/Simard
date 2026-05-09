//! Cross-cutting utility modules.
//!
//! Currently exports:
//!
//! - [`string_truncate`] — a stable-Rust char-boundary-safe replacement for
//!   `String::truncate(N)` at every site where `N` is a byte budget rather
//!   than a code-point count. See
//!   `docs/reference/string-truncation-helpers.md`.

pub mod string_truncate;
