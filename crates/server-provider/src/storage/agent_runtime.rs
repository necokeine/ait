//! Durable Agent snapshots backed by the shared metadata file engine.

use std::path::PathBuf;

use server_domain::agent_runtime::PersistedAgentRuntimeRecord;
use server_metadata::ports::registry::RegistryError;
use server_metadata::storage::registry::FileRegistry;

use crate::ports::agent_runtime::{AgentRuntimeRegistry, AgentRuntimeRegistryError};

/// Atomically persisted registry of Paseo Agent runtime snapshots.
#[derive(Debug, Clone)]
pub struct FileBackedAgentRuntimeRegistry {
    file: std::sync::Arc<FileRegistry<PersistedAgentRuntimeRecord>>,
}

impl FileBackedAgentRuntimeRegistry {
    /// Create a lazy registry backed by one JSON array.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            file: std::sync::Arc::new(FileRegistry::new(path, |record| &record.id)),
        }
    }
}

impl AgentRuntimeRegistry for FileBackedAgentRuntimeRegistry {
    fn initialize(&self) -> Result<(), AgentRuntimeRegistryError> {
        self.file.initialize().map_err(map_error)
    }

    fn list(&self) -> Result<Vec<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        self.file.list().map_err(map_error)
    }

    fn get(
        &self,
        agent_id: &str,
    ) -> Result<Option<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        self.file.get(agent_id).map_err(map_error)
    }

    fn upsert(
        &self,
        record: &PersistedAgentRuntimeRecord,
    ) -> Result<(), AgentRuntimeRegistryError> {
        validate_record(record)?;
        self.file
            .mutate(|records| {
                records.insert(record.id.clone(), record.clone());
                Ok(((), true))
            })
            .map_err(map_error)
    }

    fn update(
        &self,
        agent_id: &str,
        update: &dyn Fn(&PersistedAgentRuntimeRecord) -> PersistedAgentRuntimeRecord,
    ) -> Result<Option<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        self.file
            .mutate(|records| {
                let Some(current) = records.get(agent_id) else {
                    return Ok((None, false));
                };
                let next = update(current);
                validate_record(&next).map_err(unmap_error)?;
                if next.id != agent_id {
                    return Err(RegistryError::InvalidRecord);
                }
                records.insert(agent_id.to_owned(), next.clone());
                Ok((Some(next), true))
            })
            .map_err(map_error)
    }

    fn remove(&self, agent_id: &str) -> Result<bool, AgentRuntimeRegistryError> {
        self.file
            .mutate(|records| {
                let removed = records.shift_remove(agent_id).is_some();
                Ok((removed, removed))
            })
            .map_err(map_error)
    }
}

fn validate_record(record: &PersistedAgentRuntimeRecord) -> Result<(), AgentRuntimeRegistryError> {
    let required = [
        record.id.as_str(),
        record.provider.as_str(),
        record.cwd.as_str(),
        record.created_at.as_str(),
        record.updated_at.as_str(),
    ];
    if required
        .iter()
        .any(|value| value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control))
        || record.id.len() > 128
    {
        return Err(AgentRuntimeRegistryError::InvalidRecord);
    }
    Ok(())
}

const fn map_error(error: RegistryError) -> AgentRuntimeRegistryError {
    match error {
        RegistryError::InvalidRecord => AgentRuntimeRegistryError::InvalidRecord,
        RegistryError::InvalidFile => AgentRuntimeRegistryError::InvalidFile,
        RegistryError::Io | RegistryError::Frozen | RegistryError::Observer => {
            AgentRuntimeRegistryError::Io
        }
    }
}

const fn unmap_error(error: AgentRuntimeRegistryError) -> RegistryError {
    match error {
        AgentRuntimeRegistryError::InvalidRecord => RegistryError::InvalidRecord,
        AgentRuntimeRegistryError::InvalidFile => RegistryError::InvalidFile,
        AgentRuntimeRegistryError::Io => RegistryError::Io,
    }
}

#[cfg(test)]
mod tests;
