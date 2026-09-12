//! Application injection point for supervised execution and atomic RPC receipts.
use crate::{RunStore, RunStoreError};
use ait_domain::{
    AgentConfiguration, AgentProvider, DomainError, Message, Run, RunAttempt, RunId,
    RunPermissionProfile, SandboxAccess, ToolExecution,
};
use async_trait::async_trait;
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

/// One atomic mutation of the existing `RunStore`, not a second lifecycle.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[allow(
    clippy::large_enum_variant,
    reason = "one bounded, transient mutation mirrors the existing owned RunStore methods"
)]
pub enum RunMutation {
    /// State transition.
    SaveRun(Run),
    /// Attempt transition.
    SaveAttempt(Run, RunAttempt),
    /// Immutable output and Session advancement.
    AppendMessage(Run, Message),
    /// Tool intent or known outcome.
    SaveTool(Run, ToolExecution),
    /// Unique tool result and Session advancement.
    AppendToolResult(Run, ToolExecution, Message),
    /// Terminal barrier.
    Complete(Run, u64),
    /// Existing queue drain.
    DrainQueue(Run),
}
/// Durable receipt returned only after commit.
#[derive(Clone, Debug, PartialEq)]
pub struct RunReceipt {
    /// Authoritative state at this operation's commit.
    pub run: Run,
    /// Terminal barrier outcome, absent for other operations.
    pub completed: Option<bool>,
}
/// Lease fencing tuple kept outside the domain aggregate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerLease {
    /// Fixed Run.
    pub run_id: RunId,
    /// Unique spawned executor.
    pub instance_id: String,
    /// Monotonically increasing persisted epoch.
    pub epoch: u64,
}
/// Identity of one durable native-harness mutation, independent of wire DTOs.
#[derive(Clone, Debug)]
pub struct WorkspaceWorkerOperation {
    /// Fenced process identity.
    pub lease: WorkerLease,
    /// Stable retry identity within the lease.
    pub operation_id: String,
}
/// Fixed production API execution context. Debug deliberately omits credentials.
pub struct ApiRunDispatch {
    /// Run identity.
    pub run_id: RunId,
    /// Fixed Project capability.
    pub workdir: PathBuf,
    /// Permission snapshot.
    pub permission: RunPermissionProfile,
    /// Current administrator ceiling.
    pub maximum_sandbox: SandboxAccess,
    /// Fixed provider configuration.
    pub provider: AgentProvider,
    /// Fixed model configuration.
    pub config: AgentConfiguration,
    /// Minimum-scope in-memory grant; never logged or persisted.
    pub credential: String,
    /// Sole durable writer's adapter.
    pub store: Arc<dyn RunStore>,
    /// Cancellation after durable intent.
    pub cancellation: CancellationToken,
}
/// Executes the same `RunCoordinator` behind an application-selected boundary.
#[async_trait]
pub trait RunDispatcher: Send + Sync {
    /// Return only after the worker and its owned process tree stop.
    async fn dispatch(&self, request: ApiRunDispatch) -> Result<(), DomainError>;
}

/// Fail-closed default for stores that have not implemented atomic receipts.
pub fn unsupported_worker_store() -> RunStoreError {
    RunStoreError::Other("store does not support supervised worker commits".into())
}
