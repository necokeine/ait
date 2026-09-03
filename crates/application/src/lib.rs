//! Application use cases coordinating domain behavior through ports.

mod control;
mod message;
mod project;

pub use control::LocalControlService;
pub use message::{MessageService, MessageServiceError, SessionView};
pub use project::{
    ExternalInstruction, InstructionLayer, ProjectError, ProjectRegistration, ProjectService,
};
