mod daemon;
mod persistence;

// Re-export all public items so `crate::operator_commands_ooda::X` still works.
pub use daemon::run_ooda_daemon;

#[cfg(test)]
mod tests;
