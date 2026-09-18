//! Provider-neutral contracts for a local-first AI harness.
//!
//! The crate intentionally owns no Session or Run state. A Run pins an
//! [`AgentRevision`], resolves its credential just in time, validates declared
//! capabilities, and delegates one model turn to a [`ProviderAdapter`].

/// Versioned agent definitions and the in-memory catalog.
pub mod agent;
pub mod contract;
/// Provider-neutral error types and retry guidance.
pub mod error;
/// Deterministic provider implementation for local and contract tests.
pub mod mock;
/// OpenAI-compatible streaming provider adapter.
pub mod openai;
/// Provider-neutral request, response, and capability types.
pub mod protocol;
/// Provider invocation boundary and adapter trait.
pub mod provider;
/// Credential references, secret values, and resolvers.
pub mod secret;

pub use agent::{AgentCatalog, AgentDefinition, AgentRevision, CatalogError};
pub use error::{ProviderError, ProviderErrorKind, RetryDirective};
pub use protocol::*;
pub use provider::{ProviderAdapter, ProviderInvocation, ProviderStream, validate_request};
pub use secret::{CredentialRef, CredentialResolver, SecretValue};
