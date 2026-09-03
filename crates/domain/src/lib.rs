//! Pure domain types and invariants for AIT.

mod project;

pub use project::{
    InstructionSnapshot, InstructionSourceSummary, MessageId, Project, ProjectId, Session,
    SessionId, SessionRoot, SystemMessage,
};
