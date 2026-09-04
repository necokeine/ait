use async_trait::async_trait;
use serde_json::Value;

/// Optimistically versioned, transport-neutral control-plane snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlSnapshot {
    /// Revision used by compare-and-swap persistence.
    pub revision: u64,
    /// Application-owned serialized state.
    pub value: Value,
}

/// Event to append atomically with a snapshot change.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingEvent {
    /// Stable event name.
    pub kind: String,
    /// Optional aggregate identity.
    pub entity_id: Option<String>,
    /// Versioned event payload.
    pub body: Value,
    /// Unix timestamp in milliseconds.
    pub created_at: i64,
}

/// Durable event returned to a reconnecting client.
#[derive(Clone, Debug, PartialEq)]
pub struct DurableEvent {
    /// Monotonically increasing replay cursor.
    pub cursor: u64,
    /// Stable event name.
    pub kind: String,
    /// Optional aggregate identity.
    pub entity_id: Option<String>,
    /// Versioned event payload.
    pub body: Value,
    /// Unix timestamp in milliseconds.
    pub created_at: i64,
}

/// Failures exposed by control-plane persistence adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlStoreError {
    /// Another writer committed a newer snapshot.
    Conflict,
    /// Safe adapter failure text.
    Other(String),
}

impl std::fmt::Display for ControlStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict => formatter.write_str("control snapshot conflict"),
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ControlStoreError {}

/// Persistence seam for the local control plane and its durable event outbox.
#[async_trait]
pub trait ControlStore: Send + Sync {
    /// Loads the latest committed application snapshot.
    async fn load(&self) -> Result<ControlSnapshot, ControlStoreError>;

    /// Atomically replaces the expected snapshot and appends its events.
    async fn commit(
        &self,
        expected_revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
    ) -> Result<ControlSnapshot, ControlStoreError>;

    /// Replays durable events strictly after `cursor` in cursor order.
    async fn replay(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError>;
}
