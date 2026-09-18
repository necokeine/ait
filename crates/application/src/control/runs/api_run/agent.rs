//! Provider and no-tool adapters used by the in-process API Run coordinator.
use std::sync::Arc;

use ait_domain::{DomainError, ErrorCode, ToolExecution};
use ait_ports::{
    AgentInvocation, AgentProviderGateway, AgentResponse, RunAgent, RunTool, ToolInvocation,
    ToolOutcome, ToolRecovery,
};
use async_trait::async_trait;
use serde_json::Value;

use crate::control::runs::RunRecord;

#[derive(Clone)]
pub(super) struct ProviderAgent {
    pub(super) gateway: Arc<dyn AgentProviderGateway>,
    pub(super) view: RunRecord,
    pub(super) credential: String,
    pub(super) names: Vec<String>,
}

#[async_trait]
impl RunAgent for ProviderAgent {
    async fn invoke(&self, request: AgentInvocation) -> Result<AgentResponse, DomainError> {
        self.gateway
            .complete_turn(
                &self.view.provider,
                &self.credential,
                &self.view.config,
                request,
                self.names.clone(),
            )
            .await
    }
}

pub(super) struct NoTools;

#[async_trait]
impl RunTool for NoTools {
    fn requires_approval(&self, _: &str, _: &Value) -> bool {
        false
    }

    async fn execute(&self, _: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        Err(DomainError::invariant(
            ErrorCode::ToolExecutionFailed,
            "host tool is unavailable",
        ))
    }

    async fn reconcile(&self, _: &ToolExecution) -> Result<ToolRecovery, DomainError> {
        Ok(ToolRecovery::Unknown)
    }
}
