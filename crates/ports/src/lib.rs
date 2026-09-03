//! Abstract ports consumed by the domain and application layers.

mod project;

pub use project::{
    CreateSessionRoot, DiscoveredInstructions, EnvironmentError, ProjectEnvironment, ProjectStore,
    StoreError,
};
