//! Abstract ports consumed by the domain and application layers.

mod agent;
/// Reusable fixtures and assertions for Agent adapter contract tests.
pub mod agent_conformance;
mod control;
mod message;
mod project;
mod run;
mod scheduler;
mod session;

pub use agent::{
    AgentActivity, AgentActivityKind, AgentApprovalMode, AgentCallId, AgentCallLimits,
    AgentCapabilities, AgentCapability, AgentCheckpoint, AgentCheckpointCompatibility, AgentError,
    AgentErrorClassificationError, AgentErrorKind, AgentEvent, AgentEventStream,
    AgentExecutionProfile, AgentInput, AgentInvoker, AgentOutputContract, AgentPurpose,
    AgentRequest, AgentResolver, AgentRevisionSnapshot, AgentStopReason, AgentToolMode,
    CredentialRef, DirectTaskKind, OperationId, ResolvedAgent, RetryDirective, ToolDescriptor,
    WorkspaceAccess, preflight_request,
};
pub use control::{ControlSnapshot, ControlStore, ControlStoreError, DurableEvent, PendingEvent};
pub use message::{MessageStore, MessageStoreError};
pub use project::{
    CreateSessionRoot, DiscoveredInstructions, EnvironmentError, ProjectEnvironment, ProjectStore,
    StoreError,
};
pub use run::{
    AgentInvocation, AgentResponse, ApprovalDecision, ApprovalRequest, CompletionResult,
    GeneratedSessionTitle, RunAgent, RunApproval, RunClock, RunIdGenerator, RunStore,
    RunStoreError, RunTool, SessionTitleGenerator, SessionTitleRequest, ToolInvocation,
    ToolOutcome, ToolRecovery, WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse,
};
pub use scheduler::{
    ActiveCronRun, ClaimCronFire, CronClaimResult, CronStore, RunStartResult, RunStartTrigger,
    RunStarter, StartRunRequest,
};
pub use session::{SessionAdvance, SessionStore, SessionStoreError};
