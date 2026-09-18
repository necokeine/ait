//! Record-store access shared by use cases and the runtime bridge.
use crate::control::LocalControlService;
use crate::control::catalog::ProviderContext;
use crate::control::conversation::SessionTitleContext;
use crate::control::persistence::transaction::{RecordContext, RecordTransaction};
use crate::control::runs::RunContext;
use crate::control::use_cases::transaction::CommandTransaction;
use ait_contracts::{ApiError, Command};
use ait_ports::{ControlStore, ControlStoreError, PendingEvent};
use std::sync::Arc;

#[derive(Clone)]
pub(in crate::control) struct RecordAccess {
    pub(in crate::control) store: Arc<dyn ControlStore>,
}
impl LocalControlService {
    pub(in crate::control) fn records(&self) -> RecordAccess {
        RecordAccess {
            store: Arc::clone(&self.store),
        }
    }
    pub(in crate::control) async fn read_command_records(
        &self,
        command: &Command,
    ) -> Result<CommandTransaction, ApiError> {
        self.records().read_command_records(command).await
    }
    pub(in crate::control) async fn read_provider_records(
        &self,
        provider_id: &str,
        include_agents: bool,
    ) -> Result<RecordTransaction<ProviderContext>, ApiError> {
        self.records()
            .read_provider_records(provider_id, include_agents)
            .await
    }
    pub(in crate::control) async fn read_session_title_records(
        &self,
        session_id: &str,
    ) -> Result<RecordTransaction<SessionTitleContext>, ApiError> {
        self.records().read_session_title_records(session_id).await
    }
    pub(in crate::control) async fn read_run_records(
        &self,
        run_id: &str,
    ) -> Result<RecordTransaction<RunContext>, ApiError> {
        self.records().read_run_records(run_id).await
    }
    pub(in crate::control) async fn persist_records<C: RecordContext>(
        &self,
        loaded: &RecordTransaction<C>,
        updated: &C,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        loaded.commit(self.store.as_ref(), updated, events).await
    }
}
