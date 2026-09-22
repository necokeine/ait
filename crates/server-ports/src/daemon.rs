//! Outbound daemon configuration persistence boundary.

use serde_json::Value;

/// Configuration paths classified while rereading the persisted file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfigReload {
    /// New normalized mutable configuration.
    pub config: Value,
    /// Paths applied to live owners.
    pub applied_paths: Vec<String>,
    /// Paths requiring restart.
    pub restart_required_paths: Vec<String>,
    /// Paths controlled by launch overrides.
    pub override_controlled_paths: Vec<String>,
}

/// Safe configuration storage failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DaemonConfigStoreError {
    /// The persisted document is not a supported JSON configuration.
    #[error("invalid daemon configuration")]
    Invalid,
    /// The configuration file could not be read or atomically replaced.
    #[error("daemon configuration I/O failed")]
    Io,
}

/// Persistent, transactional storage for the daemon's mutable configuration.
pub trait DaemonConfigStore: Send + Sync + std::fmt::Debug {
    /// Return the current in-memory configuration, initializing storage if absent.
    ///
    /// # Errors
    /// Returns an error when the persisted document is invalid or unavailable.
    fn get(&self) -> Result<Value, DaemonConfigStoreError>;

    /// Merge one validated Paseo mutable patch and persist before publishing it.
    ///
    /// # Errors
    /// Returns an error when the file cannot be read, validated, or replaced.
    fn patch(&self, patch: &Value) -> Result<Value, DaemonConfigStoreError>;

    /// Reread externally edited configuration and classify changed paths.
    ///
    /// # Errors
    /// Returns an error when the persisted document is invalid or unavailable.
    fn reload(&self) -> Result<DaemonConfigReload, DaemonConfigStoreError>;
}
