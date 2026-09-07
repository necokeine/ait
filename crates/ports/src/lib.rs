//! Abstract ports consumed by the domain and application layers.

mod control;
mod message;
mod project;
mod provider;
mod run;
mod scheduler;
mod session;

pub use control::{
    ControlSnapshot, ControlStore, ControlStoreError, DurableEvent, DurableEventPage, EventBounds,
    MAX_TERMINAL_OUTPUT_ARCHIVE_BYTES, PendingEvent, ProgressCheckpoint, RunOutputArchive,
};
pub use message::{MessageStore, MessageStoreError};
pub use project::{
    CreateSessionRoot, DiscoveredInstructions, EnvironmentError, ProjectEnvironment, ProjectStore,
    StoreError,
};
pub use provider::{AgentProviderGateway, HostProviderModelCatalog, ProviderMessage};
pub use run::{
    AgentInvocation, AgentResponse, ApprovalDecision, ApprovalRequest, CompletionResult,
    GeneratedSessionTitle, RunAgent, RunApproval, RunClock, RunIdGenerator, RunStore,
    RunStoreError, RunTool, SessionTitleGenerator, SessionTitleRequest, ToolInvocation,
    ToolOutcome, ToolRecovery, WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse,
    WorkspaceOperation, WorkspaceOutputItem, WorkspaceProgressEvent, WorkspaceProgressReporter,
};
pub use scheduler::{
    ActiveCronRun, ClaimCronFire, CronClaimResult, CronStore, RunStartResult, RunStartTrigger,
    RunStarter, StartRunRequest,
};
pub use session::{SessionAdvance, SessionStore, SessionStoreError};
