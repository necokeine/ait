//! Workspace agents regression coverage.
#![allow(clippy::pedantic)]
#![allow(dead_code)]
#![allow(missing_docs)]

use ait_domain::{DomainError, ErrorCode};
use ait_ports::{WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse};
use async_trait::async_trait;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::Semaphore;

pub(crate) struct BlockingAgent {
    pub(crate) entered: Semaphore,
    pub(crate) release: Semaphore,
    pub(crate) requests: Mutex<Vec<(String, Option<String>)>>,
}

impl BlockingAgent {
    pub(crate) fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            requests: Mutex::default(),
        }
    }
    pub(crate) async fn started(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}

#[async_trait]
impl WorkspaceAgent for BlockingAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let cancellation = request.cancellation.clone();
        self.requests
            .lock()
            .unwrap()
            .push((request.model, request.reasoning_effort));
        self.entered.add_permits(1);
        tokio::select! {
            permit = self.release.acquire() => permit.unwrap().forget(),
            () = cancellation.cancelled() => {
                return Err(DomainError::invariant(ErrorCode::RunCancelled, "cancelled"));
            }
        }
        Ok(WorkspaceAgentResponse {
            assistant_text: "done".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

#[derive(Default)]
pub(crate) struct CapturingWorkspaceAgent(pub(crate) Mutex<Vec<WorkspaceAgentInvocation>>);

#[async_trait]
impl WorkspaceAgent for CapturingWorkspaceAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.0.lock().unwrap().push(request);
        Ok(WorkspaceAgentResponse {
            assistant_text: "native result".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}
