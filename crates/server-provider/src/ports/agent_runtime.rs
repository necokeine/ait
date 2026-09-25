//! Persistence boundary for Paseo Agent runtime snapshots.

use std::fmt::Debug;

use server_domain::agent_runtime::PersistedAgentRuntimeRecord;

/// Failure while reading or changing the Agent runtime registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentRuntimeRegistryError {
    /// A caller supplied a record that cannot be stored safely.
    #[error("invalid Agent runtime record")]
    InvalidRecord,
    /// An existing registry record cannot be decoded.
    #[error("invalid Agent runtime registry")]
    InvalidFile,
    /// Filesystem persistence failed.
    #[error("Agent runtime registry I/O failed")]
    Io,
}

/// Blocking durable registry for provider runtime snapshots.
pub trait AgentRuntimeRegistry: Debug + Send + Sync {
    /// Load and validate all existing records.
    ///
    /// # Errors
    /// Returns an error when a record is invalid or storage cannot be read.
    fn initialize(&self) -> Result<(), AgentRuntimeRegistryError>;

    /// Return every record, including internal and archived records.
    ///
    /// # Errors
    /// Returns an error when initialization or storage access fails.
    fn list(&self) -> Result<Vec<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError>;

    /// Return one record by its full identity.
    ///
    /// # Errors
    /// Returns an error when initialization or storage access fails.
    fn get(
        &self,
        agent_id: &str,
    ) -> Result<Option<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError>;

    /// Atomically insert or replace one record.
    ///
    /// # Errors
    /// Returns an error when validation or persistence fails.
    fn upsert(&self, record: &PersistedAgentRuntimeRecord)
    -> Result<(), AgentRuntimeRegistryError>;

    /// Transform the latest record under the registry lock.
    ///
    /// # Errors
    /// Returns an error when validation or persistence fails.
    fn update(
        &self,
        agent_id: &str,
        update: &dyn Fn(&PersistedAgentRuntimeRecord) -> PersistedAgentRuntimeRecord,
    ) -> Result<Option<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError>;

    /// Permanently remove one record. Missing identities are idempotent.
    ///
    /// # Errors
    /// Returns an error when persistence fails.
    fn remove(&self, agent_id: &str) -> Result<bool, AgentRuntimeRegistryError>;
}
