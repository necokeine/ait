use std::{collections::HashMap, fmt, sync::RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{ProviderError, ProviderErrorKind, RetryDirective};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialRef(pub String);

#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only deliberate plaintext boundary. Callers must never log the result.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

#[async_trait]
pub trait CredentialResolver: Send + Sync {
    async fn resolve(&self, reference: &CredentialRef) -> Result<SecretValue, ProviderError>;
}

/// Test/local resolver. Production code should back this trait with the OS keychain.
#[derive(Debug, Default)]
pub struct InMemoryCredentialResolver {
    values: RwLock<HashMap<CredentialRef, SecretValue>>,
}

impl InMemoryCredentialResolver {
    /// Adds or replaces a credential in the in-memory resolver.
    ///
    /// # Panics
    ///
    /// Panics if an earlier thread poisoned the resolver lock.
    pub fn insert(&self, reference: CredentialRef, value: SecretValue) {
        self.values
            .write()
            .expect("credential lock poisoned")
            .insert(reference, value);
    }
}

#[async_trait]
impl CredentialResolver for InMemoryCredentialResolver {
    async fn resolve(&self, reference: &CredentialRef) -> Result<SecretValue, ProviderError> {
        self.values
            .read()
            .expect("credential lock poisoned")
            .get(reference)
            .cloned()
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorKind::Authentication,
                    format!("credential reference not found: {}", reference.0),
                    RetryDirective::Never,
                )
            })
    }
}
