//! Self-update command: downloads the latest simard binary from GitHub Releases.

mod download;
mod platform;
mod release;
mod update;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_download;

// Re-export all public items so `crate::cmd_self_update::X` still works.
pub use update::{handle_self_test, handle_self_update};
