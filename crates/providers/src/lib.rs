//! Provider-neutral contracts for a local-first AI harness.
//!
//! The crate intentionally owns no Session or Run state. A Run pins an
//! [`AgentRevision`], resolves its credential just in time, validates declared
//! capabilities, and delegates one model turn to a [`ProviderAdapter`].

#![allow(missing_docs)]

pub mod agent;
pub mod contract;
pub mod error;
pub mod mock;
pub mod openai;
pub mod protocol;
pub mod provider;
pub mod secret;

pub use agent::{AgentCatalog, AgentDefinition, AgentRevision, CatalogError};
pub use error::{ProviderError, ProviderErrorKind, RetryDirective};
pub use protocol::*;
pub use provider::{ProviderAdapter, ProviderInvocation, ProviderStream, validate_request};
pub use secret::{CredentialRef, CredentialResolver, SecretValue};
