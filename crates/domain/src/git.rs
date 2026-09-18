use serde::{Deserialize, Serialize};

use crate::{DomainError, ErrorCode};

/// Immutable Git commit identity captured at a Project or Message boundary.
///
/// Both SHA-1 and SHA-256 object formats are accepted so repositories can
/// migrate hash algorithms without changing the domain model.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct GitCommit(String);

impl GitCommit {
    /// Parses a full lowercase hexadecimal Git object identity.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidProject`] when `value` is not a full SHA-1
    /// or SHA-256 object identity.
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if is_git_commit(&value) {
            Ok(Self(value))
        } else {
            Err(DomainError::invariant(
                ErrorCode::InvalidProject,
                "Git commit must be a full lowercase SHA-1 or SHA-256 object id",
            ))
        }
    }

    /// Returns the full hexadecimal object identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Reports whether this value remains a valid full Git object identity.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        is_git_commit(&self.0)
    }
}

impl<'de> Deserialize<'de> for GitCommit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

fn is_git_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests;
