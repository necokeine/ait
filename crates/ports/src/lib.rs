//! Abstract ports consumed by the domain and application layers.

mod message;
mod project;
mod run;
mod scheduler;

pub use message::{MessageStore, MessageStoreError, SessionAdvance};
pub use project::{
    CreateSessionRoot, DiscoveredInstructions, EnvironmentError, ProjectEnvironment, ProjectStore,
    StoreError,
};
pub use run::{
    AgentInvocation, AgentResponse, ApprovalDecision, ApprovalRequest, CompletionResult, RunAgent,
    RunApproval, RunClock, RunIdGenerator, RunStore, RunStoreError, RunTool, ToolInvocation,
    ToolOutcome, ToolRecovery,
};
pub use scheduler::{
    ActiveCronRun, ClaimCronFire, CronClaimResult, CronStore, RunStartResult, RunStartTrigger,
    RunStarter, StartRunRequest,
};
