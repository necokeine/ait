//! Pure Agent identities and configuration for the independent server.

pub mod agent;
pub mod agent_runtime;

use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

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

identity!(OperationId, "Identity of a durable operation receipt.");
identity!(AgentId, "Stable identity of a catalog Agent preset.");

#[cfg(test)]
mod tests;
