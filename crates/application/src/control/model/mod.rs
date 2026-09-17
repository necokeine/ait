//! Application aggregates and explicit contract projections.
mod run;
pub(in crate::control) use run::{ApiRunState, RunLifecycle, RunState};
mod entities;
pub(in crate::control) use entities::{
    AgentState, CronState, MessageState, ProjectState, SessionState,
};
mod message;
pub(in crate::control) use entities::ProviderState;
pub(in crate::control) use message::domain_path;
pub(in crate::control) use run::cancellation_requested;
mod approval;
mod interaction;
pub(in crate::control) use approval::NativeApprovalState;
pub(in crate::control) use interaction::ToolInteractionState;
