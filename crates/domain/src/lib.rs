//! Pure domain types and invariants for AIT.

mod message;
mod project;

pub use message::{
    Message, MessageKind, MessageOrigin, MessageRole, MessageValidationError, ProjectedMessage,
    RunId, StoredMessage, SubMessage, ToolResult, ToolResultStatus,
};
pub use project::{
    InstructionSnapshot, InstructionSourceSnapshot, InstructionSourceSummary, MessageId, Project,
    ProjectId, Session, SessionId, SessionRoot, SystemMessage, SystemMessageComponent,
};
