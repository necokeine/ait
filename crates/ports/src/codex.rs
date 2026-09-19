//! Exclusive native Codex writer ownership and authoritative history exchange.
use crate::{CodexThreadSnapshot, WorkspaceApproval, WorkspaceProgressReporter};
use ait_domain::{DomainError, RunPermissionProfile};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

/// One prepared writer for a persistent native Codex Thread.
#[derive(Clone)]
pub struct CodexThreadInvocation {
    /// Project execution capability carried across the worker boundary.
    pub project_execution: Option<Arc<dyn ProjectExecution>>,
    /// Stable Ait Run identity and native input correlation value.
    pub request_id: String,
    /// Existing native Thread identity; absent creates a persistent Thread.
    pub thread_id: Option<String>,
    /// Developer instructions for a new Thread, never applied during resume.
    pub developer_instructions: Option<String>,
    /// New user input only; the resumed Thread already owns its history.
    pub prompt: String,
    /// Native working directory retained for cross-client interoperability.
    pub cwd: PathBuf,
    /// Model fixed by the bound Ait Agent policy.
    pub model: String,
    /// Optional reasoning effort fixed for this Ait Run.
    pub reasoning_effort: Option<String>,
    /// Effective permission policy snapshotted for the Run.
    pub permission_profile: RunPermissionProfile,
    /// Run-scoped native approval boundary.
    pub approvals: Arc<dyn WorkspaceApproval>,
    /// Cooperative cancellation shared with the application supervisor.
    pub cancellation: CancellationToken,
}

/// Lifetime evidence and durable process claims for a native execution.
#[async_trait]
pub trait ProjectExecution: Send + Sync {
    /// Acquisition identity stamped into every worker frame.
    fn owner(&self) -> ait_domain::ProjectOwner;
    /// Persist a process group before any request can produce side effects.
    async fn register_process(&self, pid: u32) -> Result<(), DomainError>;
    /// Remove the claim after the supervisor proves the process tree stopped.
    async fn release_process(&self, pid: u32) -> Result<(), DomainError>;
}

impl std::fmt::Debug for CodexThreadInvocation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexThreadInvocation")
            .field(
                "project_execution",
                &self
                    .project_execution
                    .as_ref()
                    .map(|project| project.owner()),
            )
            .field("request_id", &self.request_id)
            .field("thread_id", &self.thread_id)
            .field("prompt", &self.prompt)
            .field("developer_instructions", &self.developer_instructions)
            .field("cwd", &self.cwd)
            .field("model", &self.model)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("permission_profile", &self.permission_profile)
            .field("approvals", &"<workspace approval port>")
            .field("cancellation", &self.cancellation)
            .finish()
    }
}

/// Effective context verified after exclusive resume, before any input is sent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodexPreparedThread {
    /// Full history observed while owning the writer.
    pub history: CodexThreadSnapshot,
    /// Actual model returned by app-server.
    pub model: String,
    /// Actual native model provider.
    pub model_provider: String,
    /// Effective reasoning effort, including an explicit turn override.
    pub reasoning_effort: Option<String>,
}

/// A dedicated app-server process holding one native Thread's writer.
#[async_trait]
pub trait CodexThreadConnection: Send {
    /// Returns the verified admission snapshot; no new input has been sent.
    fn prepared(&self) -> &CodexPreparedThread;
    /// Sends the admitted input once and rereads authoritative history, including failed Turns.
    /// The caller must durably mark the input send-unknown before calling this method.
    async fn start(
        &mut self,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<CodexThreadSnapshot, DomainError>;
    /// Reads history under the same writer without replaying input.
    async fn read(&mut self) -> Result<CodexThreadSnapshot, DomainError>;
    /// Terminates and reaps the owned process before application ownership is released.
    async fn close(&mut self);
}

/// Writable native Thread boundary, separate from managed Git settlement.
#[async_trait]
pub trait CodexThreadWriter: Send + Sync {
    /// Acquires exclusive writer ownership and verifies history/configuration without sending input.
    /// Failure must release the process; a successful connection must be closed by its owner.
    async fn open(
        &self,
        request: CodexThreadInvocation,
    ) -> Result<Box<dyn CodexThreadConnection>, DomainError>;
}
