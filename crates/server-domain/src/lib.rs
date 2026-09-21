//! Pure project facts and versioned Agent configuration for the independent server.

pub mod agent;
pub mod registry;

use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Maximum UTF-8 bytes in an initial instruction snapshot.
pub const MAX_INSTRUCTION_BYTES: usize = 128 * 1024;

/// A persisted or supplied domain value violates an invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid domain value")]
pub struct InvalidValue;

macro_rules! identity {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(Uuid);
        impl $name {
            /// Allocate a new UUID identity.
            #[must_use]
            pub fn generate() -> Self {
                Self(Uuid::new_v4())
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
        impl FromStr for $name {
            type Err = InvalidValue;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let id = Uuid::parse_str(value).map_err(|_| InvalidValue)?;
                if id.is_nil() {
                    return Err(InvalidValue);
                }
                Ok(Self(id))
            }
        }
    };
}

identity!(
    ProjectId,
    "Stable identity stored in the project's own database."
);
identity!(MessageId, "Identity of an immutable Message node.");
identity!(OperationId, "Identity of a durable operation receipt.");
identity!(AgentId, "Stable identity of a catalog Agent preset.");

/// Validated complete Git commit object ID (SHA-1 or SHA-256).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommit(String);

impl FromStr for GitCommit {
    type Err = InvalidValue;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(InvalidValue);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }
}

impl fmt::Display for GitCommit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Ownership generation; zero represents an initialized but never acquired project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerEpoch(u64);

impl OwnerEpoch {
    /// Validate a nonnegative database generation within SQLite's signed integer range.
    ///
    /// # Errors
    /// Rejects values which cannot be stored as a SQLite integer.
    pub fn new(value: u64) -> Result<Self, InvalidValue> {
        if value > i64::MAX as u64 {
            return Err(InvalidValue);
        }
        Ok(Self(value))
    }

    /// Return the numeric generation for transport and persistence boundaries.
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Immutable system Message snapshot with no parent and no Session ownership.
/// Only this root shape is introduced in the project-opening slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootMessage {
    id: MessageId,
    text: String,
    created_at: u64,
}

impl RootMessage {
    /// Create or restore an immutable snapshot at Unix epoch milliseconds.
    ///
    /// # Errors
    /// Rejects oversized instruction content or timestamps outside SQLite's integer range.
    pub fn new(id: MessageId, text: String, created_at: u64) -> Result<Self, InvalidValue> {
        if text.len() > MAX_INSTRUCTION_BYTES || created_at > i64::MAX as u64 {
            return Err(InvalidValue);
        }
        Ok(Self {
            id,
            text,
            created_at,
        })
    }
    /// Immutable Message identity.
    #[must_use]
    pub fn id(&self) -> MessageId {
        self.id
    }
    /// Frozen instruction text, never refreshed when reopening a project.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Creation time in Unix epoch milliseconds.
    #[must_use]
    pub fn created_at(&self) -> u64 {
        self.created_at
    }
}

/// Immutable project creation facts; the local path belongs to the rebuildable catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    id: ProjectId,
    name: String,
    base_commit: GitCommit,
    root: RootMessage,
}

impl Project {
    /// Validate initial or persisted project facts and their root system Message.
    ///
    /// # Errors
    /// Rejects empty, oversized, or control-character-containing display names.
    pub fn new(
        id: ProjectId,
        name: String,
        base_commit: GitCommit,
        root: RootMessage,
    ) -> Result<Self, InvalidValue> {
        if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
            return Err(InvalidValue);
        }
        Ok(Self {
            id,
            name,
            base_commit,
            root,
        })
    }
    /// Stable portable project identity.
    #[must_use]
    pub fn id(&self) -> ProjectId {
        self.id
    }
    /// Display name captured on initialization.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Git HEAD frozen at initial registration.
    #[must_use]
    pub fn base_commit(&self) -> &GitCommit {
        &self.base_commit
    }
    /// Initial root system Message, belonging to this Project.
    #[must_use]
    pub fn root(&self) -> &RootMessage {
        &self.root
    }
}

#[cfg(test)]
mod tests;
