//! Application use cases coordinating domain behavior through ports.

mod project;

pub use project::{
    ExternalInstruction, InstructionLayer, ProjectError, ProjectRegistration, ProjectService,
};
