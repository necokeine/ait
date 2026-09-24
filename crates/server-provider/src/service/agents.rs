//! Catalog Agent use cases; provider execution and secret resolution are separate boundaries.

use std::time::{SystemTime, UNIX_EPOCH};

use server_domain::AgentId;
use server_domain::agent::{AgentConfig, AgentSnapshot, AgentTarget, Revision};

use crate::ports::agent::{
    AgentCatalog, AgentReceipt, ConfigureAgent, DefaultReceipt, DefaultSelection, SelectDefault,
};

pub use crate::ports::agent::AgentError;

/// Maximum number of bounded current-revision snapshots per page.
pub const MAX_AGENT_PAGE: usize = 50;

/// Serialized, blocking catalog service with no provider initialization side effects.
#[derive(Debug)]
pub struct Agents {
    catalog: Box<dyn AgentCatalog>,
}

impl Agents {
    /// Compose an independent catalog adapter; it must retain its host lease while alive.
    #[must_use]
    pub fn new(catalog: Box<dyn AgentCatalog>) -> Self {
        Self { catalog }
    }

    /// Publish a configuration with a method-scoped retry key.
    ///
    /// # Errors
    /// Rejects invalid keys, stale edits, default disabling, conflicting retries, or storage errors.
    pub fn configure(
        &mut self,
        target: AgentTarget,
        config: AgentConfig,
        key: &str,
    ) -> Result<AgentReceipt, AgentError> {
        validate_key(key)?;
        let recorded_at = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| AgentError::Io)?
                .as_millis(),
        )
        .map_err(|_| AgentError::Invalid)?;
        self.catalog.configure(&ConfigureAgent {
            target,
            config,
            key: key.to_owned(),
            recorded_at,
        })
    }

    /// Read an exact immutable revision or the current head.
    ///
    /// # Errors
    /// Returns missing Agent/revision or storage errors.
    pub fn get(
        &mut self,
        id: AgentId,
        revision: Option<Revision>,
    ) -> Result<AgentSnapshot, AgentError> {
        self.catalog.get_agent(id, revision)
    }

    /// Read current heads ordered by stable ID without resolving secrets.
    ///
    /// # Errors
    /// Rejects limits outside 1–50 or storage failures.
    pub fn list(
        &mut self,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<Vec<AgentSnapshot>, AgentError> {
        if !(1..=MAX_AGENT_PAGE).contains(&limit) {
            return Err(AgentError::Invalid);
        }
        self.catalog.list_agents(after, limit)
    }

    /// Read the explicit catalog default, which may be empty.
    ///
    /// # Errors
    /// Returns storage failures.
    pub fn get_default(&mut self) -> Result<DefaultSelection, AgentError> {
        self.catalog.get_default()
    }

    /// Select an enabled Agent, or explicitly clear the default, using its observed version.
    ///
    /// # Errors
    /// Rejects malformed keys/versions, stale versions, disabled/missing presets, or storage errors.
    pub fn set_default(
        &mut self,
        agent_id: Option<AgentId>,
        expected_version: u64,
        key: &str,
    ) -> Result<DefaultReceipt, AgentError> {
        validate_key(key)?;
        if expected_version > i64::MAX as u64 {
            return Err(AgentError::Invalid);
        }
        self.catalog.set_default(&SelectDefault {
            agent_id,
            expected_version,
            key: key.to_owned(),
        })
    }
}

fn validate_key(key: &str) -> Result<(), AgentError> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(AgentError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
