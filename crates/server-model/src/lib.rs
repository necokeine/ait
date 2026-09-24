//! Concrete request context and shared Tokio runtime resources for server capability crates.

mod context;
mod message;
pub mod outbound;
pub mod runtime;
pub mod server;
pub mod subscription;

pub use context::{Context, Request};
pub use message::{ErrorCode, ServerMessage};
pub use runtime::{LifecycleIntent, Runtime};
pub use server::{Lifecycle, Limits, ServerInfo, VERSION};

/// Validate a bounded, nonempty correlation or diagnostic identifier.
#[must_use]
pub fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests;
