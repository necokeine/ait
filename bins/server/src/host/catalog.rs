use std::sync::Arc;

use server_domain::AgentId;
use server_domain::agent::{AgentSnapshot, Revision};
use server_ports::agent::{
    AgentCatalog, AgentError, AgentReceipt, ConfigureAgent, DefaultReceipt, DefaultSelection,
    SelectDefault,
};
use server_storage::SqliteCatalog;

use crate::instance::InstanceLease;

/// Keep the host lease until storage closes, including detached blocking jobs during shutdown.
#[derive(Debug)]
pub(super) struct OwnedCatalog {
    pub catalog: SqliteCatalog,
    pub _instance: Arc<InstanceLease>,
}

impl AgentCatalog for OwnedCatalog {
    fn configure(&mut self, command: &ConfigureAgent) -> Result<AgentReceipt, AgentError> {
        self.catalog.configure(command)
    }
    fn get_agent(
        &mut self,
        id: AgentId,
        revision: Option<Revision>,
    ) -> Result<AgentSnapshot, AgentError> {
        self.catalog.get_agent(id, revision)
    }
    fn list_agents(
        &mut self,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<Vec<AgentSnapshot>, AgentError> {
        self.catalog.list_agents(after, limit)
    }
    fn get_default(&mut self) -> Result<DefaultSelection, AgentError> {
        self.catalog.get_default()
    }
    fn set_default(&mut self, command: &SelectDefault) -> Result<DefaultReceipt, AgentError> {
        self.catalog.set_default(command)
    }
}
