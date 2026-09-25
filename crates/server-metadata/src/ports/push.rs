//! Storage contract for durable push token leases.

use serde_json::Value;
use std::fmt;

/// Safe subscription failure, without token or filesystem contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PushError {
    /// Invalid input or persisted document.
    #[error("invalid push subscription")]
    Invalid,
    /// Persistence could not complete.
    #[error("push subscription storage failed")]
    Io,
    /// The bounded subscription store is full.
    #[error("push subscription capacity exhausted")]
    Capacity,
}

/// Storage boundary for a complete subscription snapshot.
pub trait TokenStore: Send + fmt::Debug {
    /// Load Paseo's current or legacy JSON document; absent storage returns an empty object.
    /// # Errors
    /// Returns a safe read or document error.
    fn load(&self) -> Result<Value, PushError>;
    /// Atomically persist the new document before changing in-memory state.
    /// # Errors
    /// Returns a safe persistence error; the previous document must remain intact.
    fn save(&self, document: &Value) -> Result<(), PushError>;
}
