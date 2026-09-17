//! Abstract ports consumed by the domain and application layers.

mod control;
mod dispatch;
mod project;
mod provider;
mod run;
mod scheduler;
mod workspace;
pub use workspace::{GitBaseline, ProjectWorkspace, WorkspaceLease, WorkspacePathFacts};

pub use control::{
    ControlChange, ControlFilter, ControlRead, ControlRecord, ControlRecordKind, ControlStore,
    ControlStoreError, DurableEvent, DurableEventPage, EventBounds, PendingEvent,
    ProgressCheckpoint,
};
pub use dispatch::{
    ApiRunDispatch, RunDispatcher, RunMutation, RunReceipt, WorkerLease, WorkspaceWorkerOperation,
};
pub use project::{EnvironmentError, ProjectDirectoryCreator, ProjectEnvironment};
pub use provider::{AgentProviderGateway, HostProviderModelCatalog, ProviderMessage};
pub use run::{
    AgentInvocation, AgentResponse, ApprovalDecision, ApprovalRequest, CompletionResult,
    CompositeRunTool, DenyWorkspaceApprovals, GeneratedSessionTitle, RunAgent, RunApproval,
    RunClock, RunIdGenerator, RunStore, RunStoreError, RunTool, RunToolFactory, RunToolInteraction,
    SessionTitleGenerator, SessionTitleRequest, ToolInvocation, ToolOutcome, ToolRecovery,
    ToolUsageRecorder, WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse,
    WorkspaceApproval, WorkspaceApprovalDecision, WorkspaceApprovalRequest,
    WorkspaceIntegrationCheckpoint, WorkspaceIntegrationGate, WorkspaceOperation,
    WorkspaceOutputItem, WorkspaceProgressEvent, WorkspaceProgressReporter, WorkspaceResultSink,
};
pub use scheduler::{
    ActiveCronRun, ClaimCronFire, CronClaimResult, CronStore, RunStartResult, RunStartTrigger,
    RunStarter, StartRunRequest,
};

#[cfg(feature = "contract-tests")]
pub mod workspace_contract;
