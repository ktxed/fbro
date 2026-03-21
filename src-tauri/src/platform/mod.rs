pub mod linux;

/// Abstraction over platform-specific environment queries.
pub trait Platform: Send + Sync {
    /// Returns the current user's home directory.
    fn home_dir(&self) -> String;
}
