mod lock;
#[cfg(test)]
mod tests;

pub use lock::{BuildLock, BuildLockGuard};
