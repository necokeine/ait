use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Milliseconds since the Unix epoch.
///
/// The domain uses an integer representation so persistence, IPC, and UI
/// layers do not need to agree on a date-time library.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TimestampMs(pub i64);

impl TimestampMs {
    /// Returns the raw Unix timestamp in milliseconds.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Non-secret, JSON-compatible extension data carried across layers.
///
/// Credentials, tokens, and provider secrets must never be placed here.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DomainMetadata(pub BTreeMap<String, Value>);

impl DomainMetadata {
    /// Creates empty metadata.
    #[must_use]
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Returns whether no extension fields are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A duration expressed in milliseconds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DurationMs(pub u64);

impl DurationMs {
    /// Returns the number of milliseconds.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Monetary amount in millionths of the configured billing currency.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CostMicros(pub u64);

impl CostMicros {
    /// Returns the raw micro-unit amount.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}
