//! Abstract ports consumed by the domain and application layers.

mod message;
mod project;

pub use message::{MessageStore, MessageStoreError, SessionAdvance};
pub use project::{
    CreateSessionRoot, DiscoveredInstructions, EnvironmentError, ProjectEnvironment, ProjectStore,
    StoreError,
};
