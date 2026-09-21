//! Non-secret Agent values. Configuration does not imply an installed execution adapter.

use std::fmt;
use std::str::FromStr;

use crate::{AgentId, InvalidValue};

/// Configuration schema understood by this server, independent of runtime availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    /// Native Codex configuration; execution is a separate capability.
    Codex,
}

impl FromStr for Driver {
    type Err = InvalidValue;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "codex" => Ok(Self::Codex),
            _ => Err(InvalidValue),
        }
    }
}

impl fmt::Display for Driver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codex => f.write_str("codex"),
        }
    }
}

/// Environment-variable reference, never the credential value itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRef(String);

impl FromStr for CredentialRef {
    type Err = InvalidValue;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let suffix = value
            .strip_prefix("env:AIT_SERVER_CREDENTIAL_")
            .ok_or(InvalidValue)?;
        if suffix.is_empty()
            || suffix.len() > 64
            || !suffix.as_bytes()[0].is_ascii_uppercase()
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(InvalidValue);
        }
        Ok(Self(value.to_owned()))
    }
}

impl fmt::Display for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Positive revision number within SQLite's integer range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revision(u64);

impl Revision {
    /// Validate a persisted or requested revision.
    ///
    /// # Errors
    /// Rejects zero and values above SQLite's signed integer range.
    pub fn new(value: u64) -> Result<Self, InvalidValue> {
        if value == 0 || value > i64::MAX as u64 {
            return Err(InvalidValue);
        }
        Ok(Self(value))
    }

    /// Numeric value for persistence and transport.
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }

    /// Allocate the immediately following revision.
    ///
    /// # Errors
    /// Rejects an exhausted revision counter without wrapping.
    pub fn next(self) -> Result<Self, InvalidValue> {
        Self::new(self.0 + 1)
    }
}

/// Full replacement configuration, deliberately excluding arbitrary secret-bearing parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    name: String,
    driver: Driver,
    model: String,
    credential_ref: Option<CredentialRef>,
    enabled: bool,
}

impl AgentConfig {
    /// Validate a preset; model validation is structural, not provider discovery.
    ///
    /// # Errors
    /// Rejects blank/oversized/control-containing names and invalid model identifiers.
    pub fn new(
        name: String,
        driver: Driver,
        model: String,
        credential_ref: Option<CredentialRef>,
        enabled: bool,
    ) -> Result<Self, InvalidValue> {
        if name.trim().is_empty()
            || name.len() > 255
            || name.chars().any(char::is_control)
            || model.is_empty()
            || model.len() > 128
            || !model.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
            })
        {
            return Err(InvalidValue);
        }
        Ok(Self {
            name,
            driver,
            model,
            credential_ref,
            enabled,
        })
    }

    /// User-supplied non-secret display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Configuration schema, not an execution capability assertion.
    #[must_use]
    pub fn driver(&self) -> Driver {
        self.driver
    }

    /// Explicit model identifier; never silently replaced by a provider default.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Optional environment reference, resolved only at a future execution boundary.
    #[must_use]
    pub fn credential_ref(&self) -> Option<&CredentialRef> {
        self.credential_ref.as_ref()
    }

    /// Whether the preset may be explicitly selected for future work.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// Immutable revision snapshot; an Agent's current head is stored separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSnapshot {
    id: AgentId,
    revision: Revision,
    config: AgentConfig,
    recorded_at: u64,
}

impl AgentSnapshot {
    /// Restore or construct an immutable revision recorded at Unix epoch milliseconds.
    ///
    /// # Errors
    /// Rejects timestamps outside SQLite's integer range.
    pub fn new(
        id: AgentId,
        revision: Revision,
        config: AgentConfig,
        recorded_at: u64,
    ) -> Result<Self, InvalidValue> {
        if recorded_at > i64::MAX as u64 {
            return Err(InvalidValue);
        }
        Ok(Self {
            id,
            revision,
            config,
            recorded_at,
        })
    }
    /// Stable preset ID.
    #[must_use]
    pub fn id(&self) -> AgentId {
        self.id
    }
    /// Immutable revision number.
    #[must_use]
    pub fn revision(&self) -> Revision {
        self.revision
    }
    /// Frozen configuration.
    #[must_use]
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }
    /// Time this revision was recorded.
    #[must_use]
    pub fn recorded_at(&self) -> u64 {
        self.recorded_at
    }
}

/// A creation or a conditional replacement; partial update targets cannot be represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTarget {
    /// Allocate a stable ID and first revision atomically with the receipt.
    Create,
    /// Append a revision only if the current head still matches.
    Update {
        /// Existing Agent identity.
        id: AgentId,
        /// Revision observed by the caller.
        expected: Revision,
    },
}

#[cfg(test)]
mod tests;
